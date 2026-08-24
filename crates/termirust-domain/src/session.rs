use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    ExecutableSpec, FileIdentity, HostedSessionId, LaunchPreset, PermissionPolicy, PresetId,
    PresetRisk, Project, ProjectId, Revision, WorkingDirectoryRule,
};

pub const MAX_PATH_SEARCH_DIRECTORIES: usize = 256;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionLaunchRoute {
    LegacyAppAttached,
    DurableHost,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SessionOrigin {
    pub project_id: ProjectId,
    pub preset_id: PresetId,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostedSessionState {
    Draft,
    Validating,
    Starting,
    Provisioning,
    Attaching,
    Replaying,
    Live,
    RecordingPaused,
    Offline,
    Orphaned,
    Gap,
    PermissionDenied,
    Incompatible,
    RunningAppAttached,
    Failed,
    Cancelled,
    Exited,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LaunchResolutionError {
    ProjectChanged,
    ProjectUnavailable,
    WorkingDirectoryUnavailable,
    WorkingDirectoryEscapesProject,
    HomeUnavailable,
    ExecutableMissing,
    ExecutableNotRegularFile,
    ExecutableNotRunnable,
    ExecutableChanged,
    PresetDisabled,
}

impl fmt::Display for LaunchResolutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ProjectChanged => "project changed; review the project and try again",
            Self::ProjectUnavailable => "project folder is unavailable",
            Self::WorkingDirectoryUnavailable => "working directory is unavailable",
            Self::WorkingDirectoryEscapesProject => {
                "working directory resolves outside the selected project"
            }
            Self::HomeUnavailable => "platform home directory is unavailable",
            Self::ExecutableMissing => "preset executable was not found",
            Self::ExecutableNotRegularFile => "preset executable is not a regular file",
            Self::ExecutableNotRunnable => "preset executable is not runnable",
            Self::ExecutableChanged => "preset executable changed during launch validation",
            Self::PresetDisabled => "preset is disabled",
        })
    }
}

impl std::error::Error for LaunchResolutionError {}

#[derive(Clone, Eq, PartialEq)]
pub struct ResolvedLaunch {
    pub session_id: HostedSessionId,
    pub route: SessionLaunchRoute,
    pub origin: SessionOrigin,
    pub project_revision: Revision,
    pub preset_revision: Revision,
    pub permission_policy: PermissionPolicy,
    pub risk: PresetRisk,
    pub runtime: Option<String>,
    executable: PathBuf,
    executable_identity: FileIdentity,
    arguments: Vec<String>,
    working_directory: PathBuf,
    working_directory_identity: FileIdentity,
}

impl fmt::Debug for ResolvedLaunch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedLaunch")
            .field("session_id", &self.session_id)
            .field("route", &self.route)
            .field("origin", &self.origin)
            .field("project_revision", &self.project_revision)
            .field("preset_revision", &self.preset_revision)
            .field("permission_policy", &self.permission_policy)
            .field("risk", &self.risk.is_risky())
            .field("runtime", &self.runtime)
            .field("executable", &"<redacted>")
            .field("arguments", &"<redacted>")
            .field("working_directory", &"<redacted>")
            .finish()
    }
}

impl ResolvedLaunch {
    pub fn executable(&self) -> &Path {
        &self.executable
    }

    pub fn arguments(&self) -> &[String] {
        &self.arguments
    }

    pub fn working_directory(&self) -> &Path {
        &self.working_directory
    }

    pub fn revalidate(&self) -> Result<(), LaunchResolutionError> {
        let cwd = canonical_directory(&self.working_directory)?;
        if identity_for(&cwd)? != self.working_directory_identity {
            return Err(LaunchResolutionError::ProjectChanged);
        }
        let executable = canonical_executable(&self.executable)?;
        if identity_for(&executable)? != self.executable_identity {
            return Err(LaunchResolutionError::ExecutableChanged);
        }
        Ok(())
    }
}

pub fn resolve_launch(
    session_id: HostedSessionId,
    project: &Project,
    preset: &LaunchPreset,
    path_snapshot: &[PathBuf],
    platform_home: Option<&Path>,
) -> Result<ResolvedLaunch, LaunchResolutionError> {
    if !preset.enabled {
        return Err(LaunchResolutionError::PresetDisabled);
    }

    let project_root = canonical_directory(project.canonical_root.as_path())?;
    if identity_for(&project_root)? != *project.canonical_root.identity() {
        return Err(LaunchResolutionError::ProjectChanged);
    }

    let working_directory = match &preset.working_directory {
        WorkingDirectoryRule::ProjectRoot => project_root.clone(),
        WorkingDirectoryRule::ContainedSubdirectory(relative) => {
            let resolved = canonical_directory(&project_root.join(relative))?;
            if !resolved.starts_with(&project_root) {
                return Err(LaunchResolutionError::WorkingDirectoryEscapesProject);
            }
            resolved
        }
        WorkingDirectoryRule::PlatformHome => {
            canonical_directory(platform_home.ok_or(LaunchResolutionError::HomeUnavailable)?)?
        }
    };

    let executable = match &preset.executable {
        ExecutableSpec::Absolute(value) => canonical_executable(Path::new(value))?,
        ExecutableSpec::SearchPath(value) => path_snapshot
            .iter()
            .take(MAX_PATH_SEARCH_DIRECTORIES)
            .filter(|directory| directory.is_absolute())
            .find_map(|directory| canonical_executable(&directory.join(value)).ok())
            .ok_or(LaunchResolutionError::ExecutableMissing)?,
    };

    Ok(ResolvedLaunch {
        session_id,
        route: SessionLaunchRoute::DurableHost,
        origin: SessionOrigin {
            project_id: project.id,
            preset_id: preset.id,
        },
        project_revision: project.revision,
        preset_revision: preset.revision,
        permission_policy: preset.permission_policy,
        risk: preset.risk.clone(),
        runtime: preset
            .runtime
            .as_ref()
            .map(|runtime| runtime.as_str().to_string()),
        executable_identity: identity_for(&executable)?,
        arguments: preset
            .args
            .iter()
            .map(|argument| argument.as_str().to_string())
            .collect(),
        working_directory_identity: identity_for(&working_directory)?,
        executable,
        working_directory,
    })
}

fn canonical_directory(path: &Path) -> Result<PathBuf, LaunchResolutionError> {
    let canonical =
        fs::canonicalize(path).map_err(|_| LaunchResolutionError::WorkingDirectoryUnavailable)?;
    let metadata =
        fs::metadata(&canonical).map_err(|_| LaunchResolutionError::WorkingDirectoryUnavailable)?;
    if !metadata.is_dir() {
        return Err(LaunchResolutionError::WorkingDirectoryUnavailable);
    }
    fs::read_dir(&canonical).map_err(|_| LaunchResolutionError::WorkingDirectoryUnavailable)?;
    Ok(canonical)
}

fn canonical_executable(path: &Path) -> Result<PathBuf, LaunchResolutionError> {
    let canonical = fs::canonicalize(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            LaunchResolutionError::ExecutableMissing
        } else {
            LaunchResolutionError::ExecutableNotRunnable
        }
    })?;
    let metadata =
        fs::metadata(&canonical).map_err(|_| LaunchResolutionError::ExecutableNotRunnable)?;
    if !metadata.is_file() {
        return Err(LaunchResolutionError::ExecutableNotRegularFile);
    }
    if !is_executable(&metadata) {
        return Err(LaunchResolutionError::ExecutableNotRunnable);
    }
    Ok(canonical)
}

#[cfg(unix)]
fn is_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &fs::Metadata) -> bool {
    true
}

#[cfg(unix)]
fn identity_for(path: &Path) -> Result<FileIdentity, LaunchResolutionError> {
    use std::os::unix::fs::MetadataExt as _;
    let metadata = fs::metadata(path).map_err(|_| LaunchResolutionError::ProjectUnavailable)?;
    Ok(FileIdentity::Unix {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(not(unix))]
fn identity_for(path: &Path) -> Result<FileIdentity, LaunchResolutionError> {
    let canonical =
        fs::canonicalize(path).map_err(|_| LaunchResolutionError::ProjectUnavailable)?;
    let encoded = canonical
        .to_str()
        .ok_or(LaunchResolutionError::ProjectUnavailable)?;
    #[cfg(target_os = "windows")]
    let comparison_key = encoded.to_lowercase();
    #[cfg(not(target_os = "windows"))]
    let comparison_key = encoded.to_string();
    Ok(FileIdentity::CanonicalPath { comparison_key })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LocalizedUserText, PositionKey};
    use std::io::Write as _;
    use uuid::Uuid;

    fn project(root: &Path) -> Project {
        Project {
            id: ProjectId::from_uuid(Uuid::from_u128(1)),
            display_name: LocalizedUserText::new("Fixture").unwrap(),
            canonical_root: crate::CanonicalPath::resolve(root).unwrap(),
            position: PositionKey::FIRST,
            revision: Revision::new(7),
        }
    }

    fn preset(executable: &Path, working_directory: WorkingDirectoryRule) -> LaunchPreset {
        crate::PresetDraft {
            id: PresetId::from_uuid(Uuid::from_u128(2)),
            label: "Fixture CLI".to_string(),
            executable: executable.to_string_lossy().to_string(),
            args: vec![
                "argument with spaces".to_string(),
                "$(not-a-shell)".to_string(),
            ],
            working_directory,
            runtime: None,
            enabled: true,
            favorite: false,
            permission_policy: PermissionPolicy::AskAsNeeded,
            origin: crate::PresetOrigin::User,
            confirm_risky_favorite: false,
        }
        .validate(PositionKey::FIRST, Revision::new(11))
        .unwrap()
    }

    #[cfg(unix)]
    fn executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt as _;
        let mut file = fs::File::create(path).unwrap();
        writeln!(file, "#!/bin/sh").unwrap();
        let mut permissions = file.metadata().unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(path, permissions).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn session_resolves_literal_argv_and_revalidates_identities() {
        let fixture = tempfile::tempdir().unwrap();
        let tool = fixture.path().join("fixture tool");
        executable(&tool);
        let project = project(fixture.path());
        let resolved = resolve_launch(
            HostedSessionId::from_uuid(Uuid::from_u128(3)),
            &project,
            &preset(&tool, WorkingDirectoryRule::ProjectRoot),
            &[],
            None,
        )
        .unwrap();
        assert_eq!(resolved.route, SessionLaunchRoute::DurableHost);
        assert_eq!(
            resolved.arguments(),
            ["argument with spaces", "$(not-a-shell)"]
        );
        assert_eq!(
            resolved.working_directory(),
            fixture.path().canonicalize().unwrap()
        );
        assert_eq!(resolved.revalidate(), Ok(()));
        assert!(!format!("{resolved:?}").contains("fixture tool"));
    }

    #[cfg(unix)]
    #[test]
    fn session_rejects_contained_symlink_escape_and_non_executable_file() {
        use std::os::unix::fs::symlink;
        let project_dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), project_dir.path().join("escape")).unwrap();
        let tool = project_dir.path().join("plain-file");
        fs::write(&tool, b"not executable").unwrap();
        let project = project(project_dir.path());
        let escape = resolve_launch(
            HostedSessionId::new(),
            &project,
            &preset(
                &std::env::current_exe().unwrap(),
                WorkingDirectoryRule::ContainedSubdirectory("escape".to_string()),
            ),
            &[],
            None,
        );
        assert_eq!(
            escape,
            Err(LaunchResolutionError::WorkingDirectoryEscapesProject)
        );
        let unusable = resolve_launch(
            HostedSessionId::new(),
            &project,
            &preset(&tool, WorkingDirectoryRule::ProjectRoot),
            &[],
            None,
        );
        assert_eq!(unusable, Err(LaunchResolutionError::ExecutableNotRunnable));
    }

    #[cfg(unix)]
    #[test]
    fn session_bounded_path_search_resolves_regular_executable() {
        let fixture = tempfile::tempdir().unwrap();
        let bin = fixture.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let tool = bin.join("fixture-cli");
        executable(&tool);
        let mut preset = preset(&tool, WorkingDirectoryRule::ProjectRoot);
        preset.executable = ExecutableSpec::SearchPath("fixture-cli".to_string());
        let resolved = resolve_launch(
            HostedSessionId::new(),
            &project(fixture.path()),
            &preset,
            &[bin],
            None,
        )
        .unwrap();
        assert_eq!(resolved.executable(), tool.canonicalize().unwrap());
    }
}
