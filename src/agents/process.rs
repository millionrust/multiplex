use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Result, bail};

use crate::agents::adapter::provider_descriptor;
use crate::models::{AgentBackendKind, AgentPermissionPolicy, AgentProvider, SavedAgentDefinition};

const MAX_VERSION_OUTPUT_BYTES: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentExecutableStatus {
    Available {
        path: PathBuf,
        version: Option<String>,
    },
    Missing {
        requested: OsString,
        guidance: &'static str,
    },
    Unusable {
        path: PathBuf,
        error: String,
        guidance: &'static str,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentLaunchSpec {
    pub provider: AgentProvider,
    pub executable: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_directory: Option<PathBuf>,
}

pub fn detect_agent_executable(definition: &SavedAgentDefinition) -> AgentExecutableStatus {
    let descriptor = provider_descriptor(definition.provider);
    let requested = definition
        .executable_override
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(OsString::from)
        .or_else(|| descriptor.executable.map(OsString::from));
    let Some(requested) = requested else {
        return AgentExecutableStatus::Missing {
            requested: OsString::new(),
            guidance: descriptor.install_guidance,
        };
    };
    let Some(path) = resolve_executable(&requested) else {
        return AgentExecutableStatus::Missing {
            requested,
            guidance: descriptor.install_guidance,
        };
    };
    match read_version(&path, descriptor.version_argument) {
        Ok(version) => AgentExecutableStatus::Available { path, version },
        Err(_error) if definition.provider == AgentProvider::CustomCli => {
            AgentExecutableStatus::Available {
                path,
                version: None,
            }
        }
        Err(error) => AgentExecutableStatus::Unusable {
            path,
            error,
            guidance: descriptor.install_guidance,
        },
    }
}

pub fn build_interactive_launch_spec(definition: &SavedAgentDefinition) -> Result<AgentLaunchSpec> {
    if definition.backend != AgentBackendKind::InteractivePty {
        bail!("The selected agent backend is not an interactive terminal");
    }
    let descriptor = provider_descriptor(definition.provider);
    if !descriptor.capabilities.interactive_pty {
        bail!(
            "{} does not provide an interactive CLI backend",
            definition.provider.label()
        );
    }
    let path = match detect_agent_executable(definition) {
        AgentExecutableStatus::Available { path, .. } => path,
        AgentExecutableStatus::Missing { .. } => bail!(
            "{} is not installed or could not be resolved",
            definition.provider.label()
        ),
        AgentExecutableStatus::Unusable { path, error, .. } => bail!(
            "{} was found at {}, but its version check failed: {error}",
            definition.provider.label(),
            path.display()
        ),
    };
    let working_directory = definition
        .working_directory
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from);
    if let Some(directory) = &working_directory {
        if !directory.is_dir() {
            bail!("Working directory does not exist: {}", directory.display());
        }
    }
    let mut arguments = permission_arguments(definition)?;
    validate_safe_arguments(definition.provider, &definition.arguments)?;
    arguments.extend(definition.arguments.iter().map(OsString::from));
    Ok(AgentLaunchSpec {
        provider: definition.provider,
        executable: path,
        arguments,
        working_directory,
    })
}

pub fn build_remote_interactive_arguments(
    definition: &SavedAgentDefinition,
) -> Result<Vec<String>> {
    if definition.backend != AgentBackendKind::InteractivePty {
        bail!("The selected agent backend is not an interactive terminal");
    }
    let descriptor = provider_descriptor(definition.provider);
    if !descriptor.capabilities.interactive_pty {
        bail!(
            "{} does not provide an interactive CLI backend",
            definition.provider.label()
        );
    }
    validate_safe_arguments(definition.provider, &definition.arguments)?;
    let mut arguments = permission_arguments(definition)?
        .into_iter()
        .map(|argument| {
            argument
                .into_string()
                .map_err(|_| anyhow::anyhow!("Provider argument is not valid UTF-8"))
        })
        .collect::<Result<Vec<_>>>()?;
    arguments.extend(definition.arguments.clone());
    Ok(arguments)
}

fn permission_arguments(definition: &SavedAgentDefinition) -> Result<Vec<OsString>> {
    let values: &[&str] = match (definition.provider, definition.permission_policy) {
        (_, AgentPermissionPolicy::ProviderDefault) => &[],
        (AgentProvider::Codex, AgentPermissionPolicy::ReadOnly) => &["--sandbox", "read-only"],
        (AgentProvider::Codex, AgentPermissionPolicy::WorkspaceWrite) => {
            &["--sandbox", "workspace-write"]
        }
        (AgentProvider::ClaudeCode, AgentPermissionPolicy::ReadOnly) => {
            &["--permission-mode", "plan"]
        }
        (AgentProvider::ClaudeCode, AgentPermissionPolicy::WorkspaceWrite) => &[],
        (AgentProvider::Gemini, AgentPermissionPolicy::ReadOnly) => &["--approval-mode", "plan"],
        (AgentProvider::Gemini, AgentPermissionPolicy::WorkspaceWrite) => &[],
        (AgentProvider::CustomCli, _) => {
            bail!(
                "Custom CLI permission policies cannot be enforced. Choose Ask as needed and configure the CLI itself."
            )
        }
        (AgentProvider::GroqApi, _) => bail!("Groq API agents are not available"),
    };
    Ok(values.iter().map(OsString::from).collect())
}

fn validate_safe_arguments(provider: AgentProvider, arguments: &[String]) -> Result<()> {
    let lowered: Vec<_> = arguments
        .iter()
        .map(|argument| argument.to_ascii_lowercase())
        .collect();
    let forbidden = match provider {
        AgentProvider::Codex => [
            "--dangerously-bypass-approvals-and-sandbox",
            "danger-full-access",
        ]
        .as_slice(),
        AgentProvider::ClaudeCode => ["--dangerously-skip-permissions"].as_slice(),
        AgentProvider::Gemini => ["--yolo", "yolo"].as_slice(),
        AgentProvider::CustomCli | AgentProvider::GroqApi => [].as_slice(),
    };
    if let Some(argument) = lowered
        .iter()
        .find(|argument| forbidden.iter().any(|value| argument.contains(value)))
    {
        bail!(
            "Unsafe permission-bypass argument is not allowed for {}: {argument}",
            provider.label()
        );
    }
    Ok(())
}

fn resolve_executable(requested: &OsStr) -> Option<PathBuf> {
    let requested_path = Path::new(requested);
    if requested_path.is_absolute() || requested_path.components().count() > 1 {
        return is_executable_file(requested_path).then(|| canonical_or_original(requested_path));
    }
    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(requested_path);
        if is_executable_file(&candidate) {
            return Some(canonical_or_original(&candidate));
        }
        #[cfg(windows)]
        for extension in windows_executable_extensions() {
            let candidate = directory.join(format!(
                "{}{}",
                requested_path.to_string_lossy(),
                extension.to_string_lossy()
            ));
            if is_executable_file(&candidate) {
                return Some(canonical_or_original(&candidate));
            }
        }
    }
    None
}

fn canonical_or_original(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(windows)]
fn windows_executable_extensions() -> Vec<OsString> {
    std::env::var_os("PATHEXT")
        .map(|value| {
            value
                .to_string_lossy()
                .split(';')
                .filter(|extension| !extension.is_empty())
                .map(OsString::from)
                .collect()
        })
        .unwrap_or_else(|| vec![OsString::from(".exe"), OsString::from(".cmd")])
}

fn read_version(executable: &Path, version_argument: &str) -> Result<Option<String>, String> {
    let output = Command::new(executable)
        .arg(version_argument)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .output()
        .map_err(|error| format!("unable to run {}: {error}", executable.display()))?;
    let bytes = if output.stdout.is_empty() {
        &output.stderr
    } else {
        &output.stdout
    };
    let truncated = &bytes[..bytes.len().min(MAX_VERSION_OUTPUT_BYTES)];
    let version = String::from_utf8_lossy(truncated).trim().to_string();
    if !output.status.success() {
        let detail = if version.is_empty() {
            output.status.to_string()
        } else {
            format!("{}: {version}", output.status)
        };
        return Err(detail);
    }
    Ok((!version.is_empty()).then_some(version))
}

#[cfg(test)]
mod tests {
    use super::{
        AgentExecutableStatus, build_interactive_launch_spec, build_remote_interactive_arguments,
        detect_agent_executable,
    };
    use crate::models::{
        AgentBackendKind, AgentPermissionPolicy, AgentProvider, SavedAgentDefinition,
        SavedWorktreePolicy,
    };
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[cfg(unix)]
    fn executable_fixture(name: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("termirust-agent-{unique}"));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(name);
        fs::write(&path, "#!/bin/sh\nprintf 'fixture 1.2.3\\n'\n").unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&path, permissions).unwrap();
        path
    }

    #[test]
    #[cfg(unix)]
    fn launch_spec_preserves_arguments_without_shell_parsing() {
        let executable = executable_fixture("agent with spaces");
        let working_directory = executable.parent().unwrap().to_path_buf();
        let definition = SavedAgentDefinition {
            provider: AgentProvider::CustomCli,
            backend: AgentBackendKind::InteractivePty,
            executable_override: Some(executable.display().to_string()),
            arguments: vec![
                "argument with spaces".to_string(),
                "$(touch should-not-run)".to_string(),
                "single'quote".to_string(),
            ],
            working_directory: Some(working_directory.display().to_string()),
            worktree: SavedWorktreePolicy::SharedDirectory,
            ..SavedAgentDefinition::default()
        };

        let spec = build_interactive_launch_spec(&definition).unwrap();
        assert_eq!(spec.executable, executable.canonicalize().unwrap());
        assert_eq!(
            spec.arguments,
            vec![
                "argument with spaces",
                "$(touch should-not-run)",
                "single'quote"
            ]
        );
        assert_eq!(spec.working_directory, Some(working_directory));
    }

    #[test]
    fn missing_override_returns_provider_guidance() {
        let definition = SavedAgentDefinition {
            provider: AgentProvider::CustomCli,
            executable_override: Some("/definitely/not/a/termirust-agent".to_string()),
            ..SavedAgentDefinition::default()
        };
        let status = detect_agent_executable(&definition);
        assert!(matches!(status, AgentExecutableStatus::Missing { .. }));
        if let AgentExecutableStatus::Missing { guidance, .. } = status {
            assert!(guidance.contains("will not install"));
        }
    }

    #[test]
    #[cfg(unix)]
    fn failed_provider_version_check_is_not_reported_as_available() {
        use std::os::unix::fs::PermissionsExt as _;

        let executable = executable_fixture("broken-claude");
        fs::write(
            &executable,
            "#!/bin/sh\nprintf 'broken install\\n' >&2\nexit 7\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&executable, permissions).unwrap();
        let definition = SavedAgentDefinition {
            provider: AgentProvider::ClaudeCode,
            executable_override: Some(executable.display().to_string()),
            ..SavedAgentDefinition::default()
        };
        assert!(matches!(
            detect_agent_executable(&definition),
            AgentExecutableStatus::Unusable { error, .. }
                if error.contains("broken install")
        ));
    }

    #[test]
    fn structured_definition_is_rejected_by_interactive_builder() {
        let definition = SavedAgentDefinition {
            provider: AgentProvider::Codex,
            backend: AgentBackendKind::Structured,
            ..SavedAgentDefinition::default()
        };
        assert!(
            build_interactive_launch_spec(&definition)
                .unwrap_err()
                .to_string()
                .contains("not an interactive terminal")
        );
    }

    #[test]
    #[cfg(unix)]
    fn provider_bypass_flags_are_rejected() {
        let executable = executable_fixture("claude");
        let definition = SavedAgentDefinition {
            provider: AgentProvider::ClaudeCode,
            executable_override: Some(executable.display().to_string()),
            arguments: vec!["--dangerously-skip-permissions".to_string()],
            ..SavedAgentDefinition::default()
        };
        assert!(
            build_interactive_launch_spec(&definition)
                .unwrap_err()
                .to_string()
                .contains("Unsafe permission-bypass")
        );
    }

    #[test]
    #[cfg(unix)]
    fn read_only_policy_maps_to_provider_arguments() {
        let executable = executable_fixture("codex");
        let definition = SavedAgentDefinition {
            provider: AgentProvider::Codex,
            executable_override: Some(executable.display().to_string()),
            permission_policy: AgentPermissionPolicy::ReadOnly,
            ..SavedAgentDefinition::default()
        };
        let spec = build_interactive_launch_spec(&definition).unwrap();
        assert_eq!(spec.arguments, vec!["--sandbox", "read-only"]);
    }

    #[test]
    fn remote_arguments_preserve_values_and_reject_bypass_flags() {
        let definition = SavedAgentDefinition {
            provider: AgentProvider::Codex,
            permission_policy: AgentPermissionPolicy::WorkspaceWrite,
            arguments: vec![
                "argument with spaces".to_string(),
                "$(touch must-not-run)".to_string(),
                "single'quote".to_string(),
            ],
            ..SavedAgentDefinition::default()
        };
        assert_eq!(
            build_remote_interactive_arguments(&definition).unwrap(),
            vec![
                "--sandbox",
                "workspace-write",
                "argument with spaces",
                "$(touch must-not-run)",
                "single'quote",
            ]
        );

        let unsafe_definition = SavedAgentDefinition {
            arguments: vec!["--dangerously-bypass-approvals-and-sandbox".to_string()],
            ..definition
        };
        assert!(
            build_remote_interactive_arguments(&unsafe_definition)
                .unwrap_err()
                .to_string()
                .contains("Unsafe permission-bypass")
        );
    }
}
