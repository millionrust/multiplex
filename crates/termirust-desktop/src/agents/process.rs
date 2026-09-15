use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use termirust_domain::{PermissionPolicy, PresetRisk, ResolvedLaunch, classify_argument_strings};

use crate::agents::adapter::provider_descriptor;
use crate::models::{
    AgentBackendKind, AgentPermissionPolicy, AgentProvider, LocalShellConfig, SavedAgentDefinition,
};

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

pub fn build_app_attached_launch_config(resolved: &ResolvedLaunch) -> Result<LocalShellConfig> {
    resolved
        .revalidate()
        .context("Launch target changed during validation")?;
    let mut args =
        permission_arguments_for_runtime(resolved.runtime.as_deref(), resolved.permission_policy)?;
    args.extend(resolved.arguments().iter().cloned());
    Ok(LocalShellConfig {
        program: resolved.executable().to_string_lossy().to_string(),
        args,
        cwd: Some(resolved.working_directory().to_string_lossy().to_string()),
    })
}

fn permission_arguments_for_runtime(
    runtime: Option<&str>,
    policy: PermissionPolicy,
) -> Result<Vec<String>> {
    let runtime = runtime.unwrap_or_default();
    let values: &[&str] = match (runtime, policy) {
        (_, PermissionPolicy::AskAsNeeded) => &[],
        ("codex", PermissionPolicy::ReadOnly) => &["--sandbox", "read-only"],
        ("codex", PermissionPolicy::WorkspaceWrite) => &["--sandbox", "workspace-write"],
        ("claude" | "claude-code", PermissionPolicy::ReadOnly) => &["--permission-mode", "plan"],
        ("claude" | "claude-code", PermissionPolicy::WorkspaceWrite) => &[],
        ("gemini" | "gemini-cli", PermissionPolicy::ReadOnly) => &["--approval-mode", "plan"],
        ("gemini" | "gemini-cli", PermissionPolicy::WorkspaceWrite) => &[],
        _ => bail!(
            "This runtime cannot enforce the selected permission policy. Choose Ask as needed or use a supported runtime preset."
        ),
    };
    Ok(values.iter().map(|value| (*value).to_string()).collect())
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
    if !descriptor.launch_modes.interactive_pty {
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
    if let Some(directory) = &working_directory
        && !directory.is_dir()
    {
        bail!("Working directory does not exist: {}", directory.display());
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
    if !descriptor.launch_modes.interactive_pty {
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

pub fn build_remote_structured_command(
    definition: &SavedAgentDefinition,
    arguments: &[String],
    environment: &[(String, String)],
) -> Result<String> {
    if definition.backend != AgentBackendKind::Structured {
        bail!("The selected agent backend is not structured");
    }
    let descriptor = provider_descriptor(definition.provider);
    if !descriptor.launch_modes.structured_events || !descriptor.launch_modes.remote {
        bail!(
            "{} does not support remote structured sessions",
            definition.provider.label()
        );
    }
    validate_safe_arguments(definition.provider, &definition.arguments)?;
    let executable = definition
        .executable_override
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or(descriptor.executable)
        .context("Choose an executable for this provider")?;
    let working_directory = definition
        .working_directory
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .context("Choose a remote working directory")?;

    let executable_arg = shell_single_quote(executable);
    let directory_arg = shell_single_quote(working_directory);
    let guidance = shell_single_quote(descriptor.install_guidance);
    let version_error = shell_single_quote(&format!(
        "TermiRust found {executable}, but its version check failed. Update or repair the CLI and select Check again."
    ));
    let directory_error = shell_single_quote(&format!(
        "TermiRust cannot access remote working directory: {working_directory}"
    ));
    let mut lines = Vec::new();
    for (key, value) in environment {
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        if !is_shell_environment_name(key) {
            bail!("Remote environment variable name is invalid: {key:?}");
        }
        lines.push(format!("export {key}={}", shell_single_quote(value)));
    }
    lines.extend([
        format!(
            "if ! command -v {executable_arg} >/dev/null 2>&1; then printf '%s\\n' {guidance} >&2; exit 127; fi"
        ),
        format!(
            "if ! {executable_arg} {} >/dev/null 2>&1; then printf '%s\\n' {version_error} >&2; exit 126; fi",
            shell_single_quote(descriptor.version_argument)
        ),
        format!(
            "if [ ! -d {directory_arg} ] || [ ! -r {directory_arg} ] || [ ! -x {directory_arg} ]; then printf '%s\\n' {directory_error} >&2; exit 125; fi"
        ),
        format!(
            "if ! cd -- {directory_arg}; then printf '%s\\n' {directory_error} >&2; exit 125; fi"
        ),
    ]);
    let mut command = format!("exec {executable_arg}");
    for argument in arguments {
        command.push(' ');
        command.push_str(&shell_single_quote(argument));
    }
    lines.push(command);
    Ok(lines.join("\n"))
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn is_shell_environment_name(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
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
    let runtime = match provider {
        AgentProvider::Codex => Some("codex"),
        AgentProvider::ClaudeCode => Some("claude"),
        AgentProvider::Gemini => Some("gemini"),
        AgentProvider::CustomCli | AgentProvider::GroqApi => None,
    };
    if let PresetRisk::Risky(argument) = classify_argument_strings(runtime, arguments) {
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
        build_remote_structured_command, detect_agent_executable, permission_arguments_for_runtime,
    };
    use crate::models::{
        AgentBackendKind, AgentPermissionPolicy, AgentProvider, SavedAgentDefinition,
        SavedWorktreePolicy,
    };
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};
    use termirust_domain::PermissionPolicy;

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
    fn app_attached_policy_arguments_are_explicit_and_argv_only() {
        assert_eq!(
            permission_arguments_for_runtime(Some("codex"), PermissionPolicy::ReadOnly).unwrap(),
            ["--sandbox", "read-only"]
        );
        assert_eq!(
            permission_arguments_for_runtime(Some("claude"), PermissionPolicy::WorkspaceWrite)
                .unwrap(),
            Vec::<String>::new()
        );
        assert!(
            permission_arguments_for_runtime(Some("custom"), PermissionPolicy::WorkspaceWrite)
                .is_err()
        );
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
    fn remote_structured_command_quotes_every_shell_boundary() {
        let definition = SavedAgentDefinition {
            provider: AgentProvider::Codex,
            backend: AgentBackendKind::Structured,
            executable_override: Some("/opt/agent's bin/codex".to_string()),
            working_directory: Some("/srv/repo's; touch /tmp/nope".to_string()),
            arguments: vec!["--feature".to_string()],
            worktree: SavedWorktreePolicy::SharedDirectory,
            ..SavedAgentDefinition::default()
        };
        let command = build_remote_structured_command(
            &definition,
            &["app-server".to_string(), "value; $(false)".to_string()],
            &[("TERMIRUST_VALUE".to_string(), "token's value".to_string())],
        )
        .unwrap();

        assert!(command.contains("export TERMIRUST_VALUE='token'\"'\"'s value'"));
        assert!(command.contains("'/opt/agent'\"'\"'s bin/codex'"));
        assert!(command.contains("'/srv/repo'\"'\"'s; touch /tmp/nope'"));
        assert!(command.ends_with("'app-server' 'value; $(false)'"));
    }

    #[test]
    fn remote_structured_command_rejects_invalid_environment_names() {
        let definition = SavedAgentDefinition {
            provider: AgentProvider::Gemini,
            backend: AgentBackendKind::Structured,
            working_directory: Some("/tmp".to_string()),
            worktree: SavedWorktreePolicy::SharedDirectory,
            ..SavedAgentDefinition::default()
        };
        let error = build_remote_structured_command(
            &definition,
            &["--output-format".to_string(), "stream-json".to_string()],
            &[("BAD-NAME".to_string(), "value".to_string())],
        )
        .unwrap_err();
        assert!(error.to_string().contains("variable name is invalid"));
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
