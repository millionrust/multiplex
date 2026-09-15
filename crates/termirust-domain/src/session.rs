use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation as _;

use crate::{
    ActivityAggregate, ExecutableSpec, FileIdentity, GroupId, HostedSessionId, LaunchPreset,
    OutputSequence, PermissionPolicy, PositionKey, PresetId, PresetRisk, Project, ProjectId,
    ReadWatermark, Revision, WorkingDirectoryRule,
};

pub const MAX_PATH_SEARCH_DIRECTORIES: usize = 256;
pub const MAX_SESSIONS_PER_PROJECT: usize = 10_000;
pub const MAX_SESSION_TITLE_SCALARS: usize = 256;
pub const MAX_AUTOMATIC_TITLE_GRAPHEMES: usize = 80;

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
    Stopping,
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

impl HostedSessionState {
    pub const fn is_running(self) -> bool {
        matches!(
            self,
            Self::Provisioning
                | Self::Attaching
                | Self::Replaying
                | Self::Live
                | Self::RecordingPaused
                | Self::Stopping
                | Self::RunningAppAttached
        )
    }

    pub const fn can_stop(self) -> bool {
        matches!(
            self,
            Self::Provisioning
                | Self::Attaching
                | Self::Replaying
                | Self::Live
                | Self::RecordingPaused
                | Self::RunningAppAttached
        )
    }

    pub const fn is_exited(self) -> bool {
        matches!(self, Self::Exited)
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TitleSource {
    #[default]
    Default,
    Automatic,
    Imported,
    Manual,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SessionTitle(String);

impl SessionTitle {
    pub fn new(value: &str) -> Result<Self, SessionStateError> {
        let value = value.trim();
        if value.is_empty() {
            return Err(SessionStateError::EmptyTitle);
        }
        if value.contains('\0')
            || value
                .chars()
                .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
        {
            return Err(SessionStateError::InvalidTitle);
        }
        if value.chars().count() > MAX_SESSION_TITLE_SCALARS {
            return Err(SessionStateError::TitleTooLong);
        }
        Ok(Self(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SessionTitle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SessionTitle(<redacted>)")
    }
}

impl fmt::Display for SessionTitle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostedSession {
    pub id: HostedSessionId,
    pub project_id: ProjectId,
    pub group_id: Option<GroupId>,
    pub preset_id: Option<PresetId>,
    pub title: SessionTitle,
    pub title_source: TitleSource,
    pub lifecycle: HostedSessionState,
    #[serde(default)]
    pub activity: ActivityAggregate,
    pub pinned: bool,
    pub position: PositionKey,
    pub last_output_sequence: OutputSequence,
    pub read_through_sequence: OutputSequence,
    pub unread_sequence: Option<OutputSequence>,
    pub archived_at: Option<u64>,
    pub created_at: u64,
    pub updated_at: u64,
    pub revision: Revision,
}

impl HostedSession {
    pub fn unread(&self) -> bool {
        self.unread_sequence
            .is_some_and(|sequence| sequence > self.read_through_sequence)
    }

    pub fn can_remove(&self) -> bool {
        self.archived_at.is_some() && self.lifecycle.is_exited()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionMutation {
    Rename(SessionTitle),
    ApplyAutomaticTitle(SessionTitle),
    SetPinned(bool),
    Move {
        group_id: Option<GroupId>,
        position: PositionKey,
    },
    MarkRead {
        through: OutputSequence,
    },
    MarkUnread {
        at: OutputSequence,
    },
    ObserveOutput {
        through: OutputSequence,
    },
    SetActivity(ActivityAggregate),
    ApplyActivity {
        activity: ActivityAggregate,
        visible_through: Option<ReadWatermark>,
    },
    SetLifecycle(HostedSessionState),
    Reconcile {
        lifecycle: HostedSessionState,
        activity: ActivityAggregate,
        through: OutputSequence,
    },
    Archive {
        at: u64,
    },
    Restore,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionStateError {
    EmptyTitle,
    InvalidTitle,
    TitleTooLong,
    InvalidLifecycleTransition {
        from: HostedSessionState,
        to: HostedSessionState,
    },
    StopRequiredBeforeArchive,
    NotArchived,
    RemoveRequiresExitedArchive,
    InvalidSequence,
    StaleRevision {
        expected: Revision,
        actual: Revision,
    },
    RevisionOverflow,
    ResourceLimit {
        limit: usize,
    },
    Unavailable,
    Store {
        code: &'static str,
    },
}

impl fmt::Display for SessionStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyTitle => formatter.write_str("session title is required"),
            Self::InvalidTitle => formatter.write_str("session title contains invalid characters"),
            Self::TitleTooLong => formatter.write_str("session title is too long"),
            Self::InvalidLifecycleTransition { .. } => {
                formatter.write_str("session lifecycle transition is not allowed")
            }
            Self::StopRequiredBeforeArchive => {
                formatter.write_str("stop the running session before archiving")
            }
            Self::NotArchived => formatter.write_str("session is not archived"),
            Self::RemoveRequiresExitedArchive => {
                formatter.write_str("only an exited archived session can be removed")
            }
            Self::InvalidSequence => formatter.write_str("session sequence is invalid"),
            Self::StaleRevision { .. } => formatter.write_str("session metadata changed"),
            Self::RevisionOverflow => formatter.write_str("session revision overflow"),
            Self::ResourceLimit { limit } => {
                write!(formatter, "session limit of {limit} reached")
            }
            Self::Unavailable => formatter.write_str("session is unavailable"),
            Self::Store { code } => write!(formatter, "session store error ({code})"),
        }
    }
}

impl std::error::Error for SessionStateError {}

pub fn reduce_session(
    session: &mut HostedSession,
    mutation: SessionMutation,
) -> Result<bool, SessionStateError> {
    match mutation {
        SessionMutation::Rename(title) => {
            if session.title == title && session.title_source == TitleSource::Manual {
                return Ok(false);
            }
            session.title = title;
            session.title_source = TitleSource::Manual;
        }
        SessionMutation::ApplyAutomaticTitle(title) => {
            if session.title_source > TitleSource::Default {
                return Ok(false);
            }
            session.title = title;
            session.title_source = TitleSource::Automatic;
        }
        SessionMutation::SetPinned(pinned) => {
            if session.pinned == pinned {
                return Ok(false);
            }
            session.pinned = pinned;
        }
        SessionMutation::Move { group_id, position } => {
            if session.group_id == group_id && session.position == position {
                return Ok(false);
            }
            session.group_id = group_id;
            session.position = position;
        }
        SessionMutation::MarkRead { through } => {
            let through = through.min(session.last_output_sequence);
            if through <= session.read_through_sequence {
                return Ok(false);
            }
            session.read_through_sequence = through;
            if session
                .unread_sequence
                .is_some_and(|value| value <= through)
            {
                session.unread_sequence = None;
            }
        }
        SessionMutation::MarkUnread { at } => {
            if at == OutputSequence::ZERO || at > session.last_output_sequence {
                return Err(SessionStateError::InvalidSequence);
            }
            if session.unread_sequence == Some(at) && session.read_through_sequence < at {
                return Ok(false);
            }
            session.unread_sequence = Some(at);
            session.read_through_sequence = session
                .read_through_sequence
                .min(OutputSequence::new(at.get().saturating_sub(1)));
        }
        SessionMutation::ObserveOutput { through } => {
            if through < session.last_output_sequence {
                return Ok(false);
            }
            if through == session.last_output_sequence {
                return Ok(false);
            }
            session.last_output_sequence = through;
        }
        SessionMutation::SetActivity(activity) => {
            if session.activity == activity {
                return Ok(false);
            }
            apply_activity(session, activity, None);
        }
        SessionMutation::ApplyActivity {
            activity,
            visible_through,
        } => {
            let before = session.clone();
            apply_activity(session, activity, visible_through);
            if *session == before {
                return Ok(false);
            }
        }
        SessionMutation::SetLifecycle(lifecycle) => {
            if session.lifecycle == lifecycle {
                return Ok(false);
            }
            if !legal_lifecycle_transition(session.lifecycle, lifecycle) {
                return Err(SessionStateError::InvalidLifecycleTransition {
                    from: session.lifecycle,
                    to: lifecycle,
                });
            }
            session.lifecycle = lifecycle;
        }
        SessionMutation::Reconcile {
            lifecycle,
            activity,
            through,
        } => {
            if session.lifecycle != lifecycle
                && !legal_lifecycle_transition(session.lifecycle, lifecycle)
            {
                return Err(SessionStateError::InvalidLifecycleTransition {
                    from: session.lifecycle,
                    to: lifecycle,
                });
            }
            let through = through.max(session.last_output_sequence);
            if session.lifecycle == lifecycle
                && session.activity == activity
                && session.last_output_sequence == through
            {
                return Ok(false);
            }
            session.lifecycle = lifecycle;
            session.last_output_sequence = through;
            apply_activity(session, activity, None);
        }
        SessionMutation::Archive { at } => {
            if session.archived_at.is_some() {
                return Ok(false);
            }
            if !session.lifecycle.is_exited() {
                return Err(SessionStateError::StopRequiredBeforeArchive);
            }
            session.archived_at = Some(at);
        }
        SessionMutation::Restore => {
            if session.archived_at.is_none() {
                return Ok(false);
            }
            session.archived_at = None;
        }
    }
    Ok(true)
}

fn apply_activity(
    session: &mut HostedSession,
    activity: ActivityAggregate,
    visible_through: Option<ReadWatermark>,
) {
    let attention_sequence = activity
        .state
        .requires_attention()
        .then_some(activity.attention_sequence)
        .flatten()
        .map(|sequence| sequence.min(session.last_output_sequence));
    session.activity = activity;
    if let Some(ReadWatermark(visible)) = visible_through {
        let visible = visible.min(session.last_output_sequence);
        session.read_through_sequence = session.read_through_sequence.max(visible);
        if session
            .unread_sequence
            .is_some_and(|sequence| sequence <= visible)
        {
            session.unread_sequence = None;
        }
    }
    if let Some(attention_sequence) = attention_sequence
        && attention_sequence > session.read_through_sequence
    {
        session.unread_sequence = Some(
            session
                .unread_sequence
                .map_or(attention_sequence, |current| {
                    current.min(attention_sequence)
                }),
        );
    }
}

pub fn automatic_title_from_explicit_input(
    input: &str,
    session_id: HostedSessionId,
) -> SessionTitle {
    let clean = strip_ansi_and_controls(input);
    let first_line = clean
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    let sensitive = contains_sensitive_assignment(&first_line);
    let candidate = if first_line.is_empty() || sensitive {
        format!("Untitled session {}", &session_id.to_string()[..8])
    } else {
        let graphemes = first_line.graphemes(true).collect::<Vec<_>>();
        if graphemes.len() > MAX_AUTOMATIC_TITLE_GRAPHEMES {
            format!("{}...", graphemes[..MAX_AUTOMATIC_TITLE_GRAPHEMES].concat())
        } else {
            first_line
        }
    };
    SessionTitle::new(&candidate).unwrap_or_else(|_| {
        SessionTitle(format!("Untitled session {}", &session_id.to_string()[..8]))
    })
}

fn strip_ansi_and_controls(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                let _ = chars.next();
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            continue;
        }
        if character.is_control() {
            if matches!(character, '\n' | '\r' | '\t') {
                result.push(character);
            }
            continue;
        }
        result.push(character);
    }
    result
}

fn contains_sensitive_assignment(value: &str) -> bool {
    let lowercase = value.to_ascii_lowercase();
    ["password", "passwd", "token", "api_key", "apikey", "secret"]
        .into_iter()
        .any(|name| {
            lowercase.contains(&format!("{name}=")) || lowercase.contains(&format!("{name}:"))
        })
}

fn legal_lifecycle_transition(from: HostedSessionState, to: HostedSessionState) -> bool {
    use HostedSessionState as State;
    match from {
        State::Draft => matches!(to, State::Validating | State::Cancelled),
        State::Validating => matches!(to, State::Provisioning | State::Failed | State::Cancelled),
        State::Starting => matches!(to, State::Live | State::Failed | State::Cancelled),
        State::Provisioning => matches!(
            to,
            State::Attaching
                | State::RunningAppAttached
                | State::Failed
                | State::Cancelled
                | State::Offline
                | State::Stopping
        ),
        State::Attaching => matches!(
            to,
            State::Replaying
                | State::Live
                | State::RecordingPaused
                | State::Offline
                | State::Orphaned
                | State::Gap
                | State::PermissionDenied
                | State::Incompatible
                | State::Exited
                | State::Stopping
        ),
        State::Replaying => matches!(
            to,
            State::Live
                | State::RecordingPaused
                | State::Offline
                | State::Orphaned
                | State::Gap
                | State::PermissionDenied
                | State::Incompatible
                | State::Exited
                | State::Stopping
        ),
        State::Live | State::RecordingPaused => matches!(
            to,
            State::Live
                | State::RecordingPaused
                | State::Attaching
                | State::Stopping
                | State::Offline
                | State::Orphaned
                | State::Gap
                | State::Exited
        ),
        State::Stopping => matches!(to, State::Exited | State::Failed | State::Orphaned),
        State::Offline | State::Gap | State::PermissionDenied | State::Incompatible => {
            matches!(to, State::Attaching | State::Orphaned | State::Exited)
        }
        State::Orphaned => matches!(to, State::Attaching | State::Exited),
        State::RunningAppAttached => matches!(to, State::Stopping | State::Exited | State::Failed),
        State::Failed => matches!(to, State::Starting),
        State::Cancelled | State::Exited => false,
    }
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

    fn hosted_session(state: HostedSessionState) -> HostedSession {
        HostedSession {
            id: HostedSessionId::from_uuid(Uuid::from_u128(30)),
            project_id: ProjectId::from_uuid(Uuid::from_u128(31)),
            group_id: None,
            preset_id: Some(PresetId::from_uuid(Uuid::from_u128(32))),
            title: SessionTitle::new("Default title").unwrap(),
            title_source: TitleSource::Default,
            lifecycle: state,
            activity: ActivityAggregate::default(),
            pinned: false,
            position: PositionKey::FIRST,
            last_output_sequence: OutputSequence::ZERO,
            read_through_sequence: OutputSequence::ZERO,
            unread_sequence: None,
            archived_at: None,
            created_at: 10,
            updated_at: 10,
            revision: Revision::ZERO,
        }
    }

    #[test]
    fn session_state_machine_requires_confirmed_exit_before_archive_and_remove() {
        for unconfirmed in [HostedSessionState::Failed, HostedSessionState::Cancelled] {
            let mut session = hosted_session(unconfirmed);
            assert_eq!(
                reduce_session(&mut session, SessionMutation::Archive { at: 20 }),
                Err(SessionStateError::StopRequiredBeforeArchive)
            );
            assert!(!session.can_remove());
        }

        let mut session = hosted_session(HostedSessionState::Live);
        assert_eq!(
            reduce_session(&mut session, SessionMutation::Archive { at: 20 }),
            Err(SessionStateError::StopRequiredBeforeArchive)
        );
        assert_eq!(
            reduce_session(
                &mut session,
                SessionMutation::SetLifecycle(HostedSessionState::Stopping)
            ),
            Ok(true)
        );
        assert_eq!(
            reduce_session(
                &mut session,
                SessionMutation::SetLifecycle(HostedSessionState::Exited)
            ),
            Ok(true)
        );
        assert_eq!(
            reduce_session(&mut session, SessionMutation::Archive { at: 20 }),
            Ok(true)
        );
        assert!(session.can_remove());
        assert_eq!(
            reduce_session(&mut session, SessionMutation::Restore),
            Ok(true)
        );
        assert_eq!(session.archived_at, None);
        assert!(!session.can_remove());
    }

    #[test]
    fn session_state_unread_is_sequence_based_and_mark_read_is_bounded() {
        let mut session = hosted_session(HostedSessionState::Live);
        assert_eq!(
            reduce_session(
                &mut session,
                SessionMutation::ObserveOutput {
                    through: OutputSequence::new(8),
                }
            ),
            Ok(true)
        );
        assert!(!session.unread());
        assert_eq!(
            reduce_session(
                &mut session,
                SessionMutation::ApplyActivity {
                    activity: ActivityAggregate {
                        state: crate::ActivityState::NeedsInput,
                        confidence: crate::ActivityConfidence::Verified,
                        effective_sequence: crate::HostSequence::new(1),
                        generation: crate::OccupantGeneration::new(1),
                        source_kind: crate::ActivitySourceKind::Approval,
                        source_id: "approval".to_string(),
                        expires_at: None,
                        stale: false,
                        attention_reason: Some(crate::AttentionReason::Approval),
                        attention_sequence: Some(OutputSequence::new(8)),
                    },
                    visible_through: None,
                }
            ),
            Ok(true)
        );
        assert!(session.unread());
        assert_eq!(
            reduce_session(
                &mut session,
                SessionMutation::MarkRead {
                    through: OutputSequence::new(100),
                }
            ),
            Ok(true)
        );
        assert_eq!(session.read_through_sequence, OutputSequence::new(8));
        assert!(!session.unread());
        assert_eq!(
            reduce_session(
                &mut session,
                SessionMutation::MarkUnread {
                    at: OutputSequence::new(8),
                }
            ),
            Ok(true)
        );
        assert_eq!(session.read_through_sequence, OutputSequence::new(7));
        assert!(session.unread());
    }

    #[test]
    fn session_state_manual_title_wins_and_automatic_title_redacts_secrets() {
        let id = HostedSessionId::from_uuid(Uuid::from_u128(0xfeed));
        let generated = automatic_title_from_explicit_input(
            "\u{1b}[31mPlease inspect this project\u{1b}[0m\nsecond line",
            id,
        );
        assert_eq!(generated.as_str(), "Please inspect this project");
        let redacted = automatic_title_from_explicit_input("token=super-secret", id);
        assert_eq!(redacted.as_str(), "Untitled session 00000000");

        let mut session = hosted_session(HostedSessionState::Live);
        let manual = SessionTitle::new("Manual title").unwrap();
        assert_eq!(
            reduce_session(&mut session, SessionMutation::Rename(manual.clone())),
            Ok(true)
        );
        assert_eq!(
            reduce_session(
                &mut session,
                SessionMutation::ApplyAutomaticTitle(generated)
            ),
            Ok(false)
        );
        assert_eq!(session.title, manual);
        assert_eq!(session.title_source, TitleSource::Manual);
        assert!(!format!("{:?}", session.title).contains("Manual title"));
    }

    #[test]
    fn session_state_illegal_recovery_transition_fails_without_mutation() {
        let mut session = hosted_session(HostedSessionState::Exited);
        let original = session.clone();
        assert_eq!(
            reduce_session(
                &mut session,
                SessionMutation::SetLifecycle(HostedSessionState::Live)
            ),
            Err(SessionStateError::InvalidLifecycleTransition {
                from: HostedSessionState::Exited,
                to: HostedSessionState::Live,
            })
        );
        assert_eq!(session, original);
    }

    #[test]
    fn session_state_live_metadata_can_reattach_without_claiming_process_restart() {
        let mut session = hosted_session(HostedSessionState::Live);
        assert_eq!(
            reduce_session(
                &mut session,
                SessionMutation::SetLifecycle(HostedSessionState::Attaching)
            ),
            Ok(true)
        );
        assert_eq!(
            reduce_session(
                &mut session,
                SessionMutation::SetLifecycle(HostedSessionState::Replaying)
            ),
            Ok(true)
        );
        assert_eq!(
            reduce_session(
                &mut session,
                SessionMutation::SetLifecycle(HostedSessionState::Live)
            ),
            Ok(true)
        );
        assert_eq!(session.archived_at, None);
    }
}
