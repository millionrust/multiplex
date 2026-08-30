use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use termirust_client::LocalEndpoint;
use termirust_domain::HostedSessionId;
use termirust_domain::ProjectStatus;
use termirust_store::{StoreError, StoreHealth, load_fleet_read_only};

use crate::model::{
    FleetGroup, FleetHealth, FleetProject, FleetRevision, FleetSession, FleetSnapshot,
    MAX_PROJECTS, MAX_VISIBLE_SESSIONS, ProjectAvailability, TuiDiagnostic,
};

const STORE_DIR_NAME: &str = "agent-workspace";
const MAX_USER_TEXT_SCALARS: usize = 256;

#[derive(Clone, Default)]
pub struct FleetCancellation {
    cancelled: Arc<AtomicBool>,
}

impl FleetCancellation {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

pub trait FleetSource: Send + Sync {
    fn load(&self, cancellation: &FleetCancellation) -> Result<FleetSnapshot, FleetLoadError>;

    fn local_endpoint(&self, _session_id: HostedSessionId) -> Option<LocalEndpoint> {
        None
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FleetLoadError {
    pub diagnostic: TuiDiagnostic,
    pub recovery_required: bool,
}

impl FleetLoadError {
    fn cancelled() -> Self {
        Self {
            diagnostic: TuiDiagnostic {
                code: "cancelled",
                summary: "Refresh cancelled",
                recovery: "Press r to refresh again.",
            },
            recovery_required: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct LocalFleetSource {
    config_root: PathBuf,
    metadata_root: PathBuf,
}

impl LocalFleetSource {
    pub fn discover() -> Result<Self, FleetLoadError> {
        let config_root = match std::env::var_os("TERMIRUST_CONFIG_DIR") {
            Some(path) if !path.is_empty() => Some(PathBuf::from(path)),
            Some(_) => None,
            None => dirs::config_dir().map(|root| root.join("termirust")),
        }
        .ok_or(FleetLoadError {
            diagnostic: TuiDiagnostic {
                code: "config-unavailable",
                summary: "TermiRust data is unavailable",
                recovery: "Set TERMIRUST_CONFIG_DIR to the existing TermiRust data directory.",
            },
            recovery_required: false,
        })?;
        Ok(Self::from_config_root(config_root))
    }

    pub fn new(metadata_root: impl Into<PathBuf>) -> Self {
        let metadata_root = metadata_root.into();
        let config_root = metadata_root
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| metadata_root.clone());
        Self {
            config_root,
            metadata_root,
        }
    }

    pub fn from_config_root(config_root: impl Into<PathBuf>) -> Self {
        let config_root = config_root.into();
        Self {
            metadata_root: config_root.join(STORE_DIR_NAME),
            config_root,
        }
    }

    pub fn metadata_root(&self) -> &Path {
        &self.metadata_root
    }
}

impl FleetSource for LocalFleetSource {
    fn load(&self, cancellation: &FleetCancellation) -> Result<FleetSnapshot, FleetLoadError> {
        if cancellation.is_cancelled() {
            return Err(FleetLoadError::cancelled());
        }
        let stored = load_fleet_read_only(&self.metadata_root).map_err(map_store_error)?;
        if cancellation.is_cancelled() {
            return Err(FleetLoadError::cancelled());
        }

        let mut skipped_records = 0usize;
        let project_count = stored.projects.projects.len();
        let mut projects = Vec::with_capacity(project_count.min(MAX_PROJECTS));
        let mut project_indexes = BTreeMap::<String, usize>::new();
        for summary in stored.projects.projects.into_iter().take(MAX_PROJECTS) {
            let id = summary.project.id.to_string();
            project_indexes.insert(id.clone(), projects.len());
            projects.push(FleetProject {
                id,
                name: safe_user_text(summary.project.display_name.as_str()),
                availability: match summary.status {
                    ProjectStatus::Available => ProjectAvailability::Available,
                    ProjectStatus::Unavailable => ProjectAvailability::Unavailable,
                    ProjectStatus::PermissionDenied => ProjectAvailability::PermissionDenied,
                },
                groups: Vec::new(),
            });
        }
        skipped_records =
            skipped_records.saturating_add(project_count.saturating_sub(projects.len()));

        let mut group_ids = BTreeSet::new();
        for group in stored.projects.groups {
            let project_id = group.project_id.to_string();
            let Some(index) = project_indexes.get(&project_id).copied() else {
                skipped_records = skipped_records.saturating_add(1);
                continue;
            };
            let id = group.id.to_string();
            group_ids.insert(id.clone());
            projects[index].groups.push(FleetGroup {
                id,
                project_id,
                name: safe_user_text(group.name.as_str()),
            });
        }

        let mut sessions =
            Vec::with_capacity(stored.sessions.sessions.len().min(MAX_VISIBLE_SESSIONS));
        for session in stored.sessions.sessions {
            if cancellation.is_cancelled() {
                return Err(FleetLoadError::cancelled());
            }
            if sessions.len() >= MAX_VISIBLE_SESSIONS {
                skipped_records = skipped_records.saturating_add(1);
                continue;
            }
            let project_id = session.project_id.to_string();
            if !project_indexes.contains_key(&project_id) {
                skipped_records = skipped_records.saturating_add(1);
                continue;
            }
            let group_id = session.group_id.map(|id| id.to_string());
            if group_id.as_ref().is_some_and(|id| !group_ids.contains(id)) {
                skipped_records = skipped_records.saturating_add(1);
                continue;
            }
            sessions.push(FleetSession {
                id: session.id.to_string(),
                project_id,
                group_id,
                title: safe_user_text(session.title.as_str()),
                state: session_state(session.lifecycle).to_string(),
                activity: activity_state(session.activity.state).to_string(),
                unread: session.unread(),
                archived: session.archived_at.is_some(),
                revision: session.revision.get(),
            });
        }

        let recovered = stored.projects.health == StoreHealth::RecoveredLastGood
            || stored.sessions.health == StoreHealth::RecoveredLastGood;
        Ok(FleetSnapshot {
            revision: FleetRevision {
                projects: stored.projects.revision.get(),
                sessions: stored.sessions.revision.get(),
            },
            projects,
            sessions,
            health: if recovered {
                FleetHealth::RecoveredLastGood
            } else if skipped_records > 0 {
                FleetHealth::Partial
            } else {
                FleetHealth::Healthy
            },
            skipped_records,
        })
    }

    fn local_endpoint(&self, session_id: HostedSessionId) -> Option<LocalEndpoint> {
        Some(LocalEndpoint::for_config_root(
            &self.config_root,
            session_id,
        ))
    }
}

fn safe_user_text(value: &str) -> String {
    value
        .chars()
        .filter(|character| {
            !character.is_control()
                && *character != '\u{202a}'
                && *character != '\u{202b}'
                && *character != '\u{202d}'
                && *character != '\u{202e}'
                && *character != '\u{202c}'
                && *character != '\u{2066}'
                && *character != '\u{2067}'
                && *character != '\u{2068}'
                && *character != '\u{2069}'
        })
        .take(MAX_USER_TEXT_SCALARS)
        .collect()
}

fn map_store_error(error: StoreError) -> FleetLoadError {
    let (code, summary, recovery, recovery_required) = match error {
        StoreError::StoreNewer { .. } => (
            "store-newer",
            "This data requires a newer TermiRust version",
            "Update TermiRust, then press r to refresh.",
            true,
        ),
        StoreError::Corrupt { .. } => (
            "store-corrupt",
            "TermiRust metadata could not be read safely",
            "Open desktop diagnostics before attempting recovery.",
            true,
        ),
        StoreError::Io {
            kind: io::ErrorKind::PermissionDenied,
            ..
        } => (
            "permission-denied",
            "TermiRust metadata permission was denied",
            "Restore read access to the existing data directory, then press r.",
            false,
        ),
        StoreError::Io { .. } => (
            "store-unavailable",
            "TermiRust metadata is unavailable",
            "Open the desktop app once or verify TERMIRUST_CONFIG_DIR, then press r.",
            false,
        ),
        StoreError::UnsafeEntry { .. } | StoreError::TooLarge { .. } => (
            "unsafe-metadata",
            "TermiRust metadata failed a safety check",
            "Inspect desktop diagnostics; the TUI will not rewrite this data.",
            true,
        ),
        StoreError::InvalidInstanceId
        | StoreError::Domain(_)
        | StoreError::GroupDomain(_)
        | StoreError::PresetDomain(_)
        | StoreError::SessionDomain(_)
        | StoreError::WorktreeDomain(_) => (
            "invalid-metadata",
            "TermiRust metadata is inconsistent",
            "Inspect desktop diagnostics; the TUI will not repair this data.",
            true,
        ),
    };
    FleetLoadError {
        diagnostic: TuiDiagnostic {
            code,
            summary,
            recovery,
        },
        recovery_required,
    }
}

fn session_state(state: termirust_domain::HostedSessionState) -> &'static str {
    use termirust_domain::HostedSessionState as State;
    match state {
        State::Draft => "draft",
        State::Validating => "validating",
        State::Starting => "starting",
        State::Provisioning => "provisioning",
        State::Attaching => "attaching",
        State::Replaying => "replaying",
        State::Live => "live",
        State::RecordingPaused => "recording_paused",
        State::Stopping => "stopping",
        State::Offline => "offline",
        State::Orphaned => "orphaned",
        State::Gap => "gap",
        State::PermissionDenied => "permission_denied",
        State::Incompatible => "incompatible",
        State::RunningAppAttached => "running_app_attached",
        State::Failed => "failed",
        State::Cancelled => "cancelled",
        State::Exited => "exited",
    }
}

fn activity_state(state: termirust_domain::ActivityState) -> &'static str {
    use termirust_domain::ActivityState as State;
    match state {
        State::Unknown => "unknown",
        State::Idle => "idle",
        State::Busy => "busy",
        State::NeedsInput => "needs_input",
        State::Done => "done",
        State::Failed => "failed",
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;
    use termirust_domain::{AddProject, ProjectId, Revision};
    use termirust_store::{ProjectRepository, SessionRepository};

    use super::*;

    #[test]
    fn source_reads_typed_store_without_changing_files() {
        let fixture = TempDir::new().unwrap();
        let metadata = fixture.path().join("metadata");
        let data = fixture.path().join("session-data");
        let project_dir = fixture.path().join("project");
        fs::create_dir(&project_dir).unwrap();
        let projects = ProjectRepository::open(&metadata).unwrap();
        projects
            .add_project(AddProject {
                id: ProjectId::new(),
                root: project_dir,
                display_name: Some("Alpha\u{1b}[31m".into()),
                expected: Revision::ZERO,
            })
            .unwrap();
        SessionRepository::open(&metadata, data).unwrap();
        let before = fs::read(metadata.join("projects.json")).unwrap();

        let snapshot = LocalFleetSource::new(&metadata)
            .load(&FleetCancellation::default())
            .unwrap();

        assert_eq!(snapshot.projects.len(), 1);
        assert_eq!(snapshot.projects[0].name, "Alpha[31m");
        assert_eq!(fs::read(metadata.join("projects.json")).unwrap(), before);
    }

    #[test]
    fn source_fails_closed_when_cancelled_or_store_is_missing() {
        let fixture = TempDir::new().unwrap();
        let source = LocalFleetSource::new(fixture.path().join("missing"));
        let cancellation = FleetCancellation::default();
        cancellation.cancel();
        assert_eq!(
            source.load(&cancellation).unwrap_err().diagnostic.code,
            "cancelled"
        );
        let error = source.load(&FleetCancellation::default()).unwrap_err();
        assert_eq!(error.diagnostic.code, "store-unavailable");
        assert!(!source.metadata_root().exists());
    }

    #[test]
    fn source_reports_recovered_newer_and_unsafe_metadata_without_repairing() {
        let fixture = TempDir::new().unwrap();
        let metadata = fixture.path().join("metadata");
        let data = fixture.path().join("session-data");
        ProjectRepository::open(&metadata).unwrap();
        SessionRepository::open(&metadata, data).unwrap();
        fs::write(metadata.join("projects.json"), b"not json").unwrap();
        let before = fs::read(metadata.join("projects.json")).unwrap();
        let recovered = LocalFleetSource::new(&metadata)
            .load(&FleetCancellation::default())
            .unwrap();
        assert_eq!(recovered.health, FleetHealth::RecoveredLastGood);
        assert_eq!(fs::read(metadata.join("projects.json")).unwrap(), before);

        fs::write(
            metadata.join("format.json"),
            br#"{"format_version":99,"minimum_reader":99,"instance_id":"00000000-0000-4000-8000-000000000001"}"#,
        )
        .unwrap();
        let newer = LocalFleetSource::new(&metadata)
            .load(&FleetCancellation::default())
            .unwrap_err();
        assert_eq!(newer.diagnostic.code, "store-newer");
        assert!(newer.recovery_required);
    }

    #[test]
    fn hostile_store_errors_map_to_allowlisted_diagnostics() {
        let permission = map_store_error(StoreError::Io {
            operation: "read fixture",
            kind: io::ErrorKind::PermissionDenied,
        });
        assert_eq!(permission.diagnostic.code, "permission-denied");
        assert!(!permission.recovery_required);

        let unsafe_entry = map_store_error(StoreError::UnsafeEntry {
            name: "projects.json",
        });
        assert_eq!(unsafe_entry.diagnostic.code, "unsafe-metadata");
        assert!(unsafe_entry.recovery_required);
        assert!(!unsafe_entry.diagnostic.summary.contains("projects.json"));
    }
}
