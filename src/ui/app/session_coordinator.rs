use std::sync::mpsc::Sender;

use termirust_domain::{
    ActivityAggregate, HostedSession, HostedSessionId, HostedSessionState, OccupantGeneration,
    OutputSequence,
};
use termirust_store::StoreError;

use super::hosted_session::{
    DurableContinuityCommit, DurableLaunch, DurableSessionPaths, DurableSessionSpec,
    spawn_durable_session,
};
use super::session_library::SessionLibraryState;
use crate::models::SavedState;
use crate::ssh::{SessionRuntimeHandle, SshEvent};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum HostedDevUrlAction {
    Preserve,
    MarkGap,
    MarkUnavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct HostedPaneStatusProjection {
    pub last_sequence: u64,
    pub has_writer_lease: bool,
    pub dev_url_action: HostedDevUrlAction,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct HostedStatusInput {
    pub hosted_session_id: HostedSessionId,
    pub state: HostedSessionState,
    pub last_sequence: u64,
    pub durable_sequence: u64,
    pub activity: ActivityAggregate,
    pub has_writer_lease: bool,
    pub visibly_focused: bool,
}

impl HostedStatusInput {
    pub fn pane_projection(&self) -> HostedPaneStatusProjection {
        let dev_url_action = if self.state == HostedSessionState::Gap {
            HostedDevUrlAction::MarkGap
        } else if hosted_state_marks_host_unavailable(self.state) {
            HostedDevUrlAction::MarkUnavailable
        } else {
            HostedDevUrlAction::Preserve
        };
        HostedPaneStatusProjection {
            last_sequence: self.last_sequence,
            has_writer_lease: self.has_writer_lease,
            dev_url_action,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PendingArchiveAction {
    None,
    Archive,
    Fail,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct HostedStatusWarnings {
    pub compatibility_save: Option<String>,
    pub activity_observation: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct HostedStatusProjection {
    pub pending_archive: PendingArchiveAction,
    pub warnings: HostedStatusWarnings,
}

pub(super) trait SessionActivityObserver {
    fn observe_session_transition(
        &mut self,
        previous: Option<&HostedSession>,
        current: &HostedSession,
        visibly_focused: bool,
    ) -> Result<(), String>;
}

pub(super) enum SessionStartRequest {
    Launch(Box<SessionLaunchRequest>),
    Attach(SessionAttachRequest),
}

pub(super) struct SessionLaunchRequest {
    pane_id: u64,
    session_id: HostedSessionId,
    paths: DurableSessionPaths,
    launch: DurableLaunch,
    expected_occupant_generation: Option<OccupantGeneration>,
    continuity: Option<DurableContinuityCommit>,
}

pub(super) struct SessionAttachRequest {
    pane_id: u64,
    session_id: HostedSessionId,
    paths: DurableSessionPaths,
    from_sequence: OutputSequence,
}

impl SessionStartRequest {
    pub(super) fn launch(
        pane_id: u64,
        session_id: HostedSessionId,
        paths: DurableSessionPaths,
        launch: DurableLaunch,
        expected_occupant_generation: Option<OccupantGeneration>,
        continuity: Option<DurableContinuityCommit>,
    ) -> Self {
        Self::Launch(Box::new(SessionLaunchRequest {
            pane_id,
            session_id,
            paths,
            launch,
            expected_occupant_generation,
            continuity,
        }))
    }

    pub(super) fn attach(
        pane_id: u64,
        session_id: HostedSessionId,
        paths: DurableSessionPaths,
        from_sequence: OutputSequence,
    ) -> Self {
        Self::Attach(SessionAttachRequest {
            pane_id,
            session_id,
            paths,
            from_sequence,
        })
    }

    fn into_spec(self) -> DurableSessionSpec {
        match self {
            Self::Launch(request) => {
                let SessionLaunchRequest {
                    pane_id,
                    session_id,
                    paths,
                    launch,
                    expected_occupant_generation,
                    continuity,
                } = *request;
                DurableSessionSpec {
                    pane_id,
                    session_id,
                    paths,
                    launch: Some(launch),
                    from_sequence: OutputSequence::ZERO,
                    expected_occupant_generation,
                    continuity,
                }
            }
            Self::Attach(SessionAttachRequest {
                pane_id,
                session_id,
                paths,
                from_sequence,
            }) => DurableSessionSpec {
                pane_id,
                session_id,
                paths,
                launch: None,
                from_sequence,
                expected_occupant_generation: None,
                continuity: None,
            },
        }
    }
}

pub(super) struct SessionCoordinator {
    event_tx: Sender<SshEvent>,
}

impl SessionCoordinator {
    pub fn new(event_tx: Sender<SshEvent>) -> Self {
        Self { event_tx }
    }

    pub fn start(&self, request: SessionStartRequest) -> SessionRuntimeHandle {
        spawn_durable_session(request.into_spec(), self.event_tx.clone())
    }

    pub fn project_hosted_status<E>(
        &self,
        input: HostedStatusInput,
        saved: &mut SavedState,
        session_library: &mut SessionLibraryState,
        activity_observer: &mut impl SessionActivityObserver,
        persist_compatibility_projection: impl FnOnce(&SavedState) -> Result<(), E>,
    ) -> Result<HostedStatusProjection, StoreError>
    where
        E: std::fmt::Display,
    {
        let previous_session = session_library.session(input.hosted_session_id).cloned();
        if let Some(host) = saved
            .app_attached_sessions
            .iter_mut()
            .find(|session| session.id == input.hosted_session_id)
            .and_then(|session| session.durable_host.as_mut())
        {
            host.last_sequence = host.last_sequence.max(input.last_sequence);
            host.durable_sequence = host.durable_sequence.max(input.durable_sequence);
        }

        let session = session_library.reconcile(
            saved,
            input.hosted_session_id,
            input.state,
            input.activity,
            OutputSequence::new(input.last_sequence),
        )?;
        let compatibility_save = persist_compatibility_projection(saved)
            .err()
            .map(|error| error.to_string());
        let activity_observation = activity_observer
            .observe_session_transition(previous_session.as_ref(), &session, input.visibly_focused)
            .err();

        Ok(HostedStatusProjection {
            pending_archive: pending_archive_action(
                session_library,
                input.hosted_session_id,
                input.state,
            ),
            warnings: HostedStatusWarnings {
                compatibility_save,
                activity_observation,
            },
        })
    }
}

fn hosted_state_marks_host_unavailable(state: HostedSessionState) -> bool {
    matches!(
        state,
        HostedSessionState::Exited
            | HostedSessionState::Failed
            | HostedSessionState::Orphaned
            | HostedSessionState::Offline
            | HostedSessionState::PermissionDenied
            | HostedSessionState::Incompatible
    )
}

fn pending_archive_action(
    session_library: &SessionLibraryState,
    id: HostedSessionId,
    state: HostedSessionState,
) -> PendingArchiveAction {
    if session_library.pending_archive_after_stop != Some(id) {
        return PendingArchiveAction::None;
    }
    if state == HostedSessionState::Exited {
        PendingArchiveAction::Archive
    } else if matches!(
        state,
        HostedSessionState::Failed
            | HostedSessionState::Offline
            | HostedSessionState::Orphaned
            | HostedSessionState::Gap
            | HostedSessionState::PermissionDenied
            | HostedSessionState::Incompatible
    ) {
        PendingArchiveAction::Fail
    } else {
        PendingArchiveAction::None
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::mpsc;

    use termirust_domain::{
        ActivityAggregate, CommandId, ContinuityLink, HostedSession, HostedSessionId,
        HostedSessionState, OccupantGeneration, OutputSequence, PositionKey, PresetId, ProjectId,
        Revision, RuntimeId, SessionLaunchRoute, SessionOrigin, TitleSource,
    };
    use termirust_store::SessionRepository;

    use super::{
        DurableContinuityCommit, DurableLaunch, DurableSessionPaths, HostedDevUrlAction,
        HostedStatusInput, PendingArchiveAction, SessionActivityObserver, SessionCoordinator,
        SessionStartRequest,
    };
    use crate::models::{SavedAppAttachedSession, SavedDurableHost, SavedState};
    use crate::ui::app::session_library::SessionLibraryState;

    #[derive(Default)]
    struct RecordingActivityObserver {
        observations: Vec<(Option<HostedSession>, HostedSession, bool)>,
        failure: Option<String>,
    }

    impl SessionActivityObserver for RecordingActivityObserver {
        fn observe_session_transition(
            &mut self,
            previous: Option<&HostedSession>,
            current: &HostedSession,
            visibly_focused: bool,
        ) -> Result<(), String> {
            self.observations
                .push((previous.cloned(), current.clone(), visibly_focused));
            if let Some(failure) = self.failure.as_ref() {
                Err(failure.clone())
            } else {
                Ok(())
            }
        }
    }

    fn saved_session(id: HostedSessionId, state: HostedSessionState) -> SavedAppAttachedSession {
        SavedAppAttachedSession {
            id,
            route: SessionLaunchRoute::DurableHost,
            origin: SessionOrigin {
                project_id: ProjectId::new(),
                preset_id: PresetId::new(),
            },
            state,
            project_label: "Project".to_string(),
            preset_label: "Codex".to_string(),
            title: "Investigate parser".to_string(),
            title_source: TitleSource::Manual,
            activity: ActivityAggregate::default(),
            pinned: false,
            read_through_sequence: 0,
            unread_sequence: None,
            archived_at: None,
            revision: Revision::ZERO,
            durable_host: Some(SavedDurableHost {
                runtime_root: "/synthetic/runtime".to_string(),
                session_dir: "/synthetic/session".to_string(),
                last_sequence: 20,
                durable_sequence: 30,
                ..SavedDurableHost::default()
            }),
            group_id: None,
            position: PositionKey::FIRST,
            started_at: 1,
            updated_at: 1,
        }
    }

    fn session_library(saved: &mut SavedState, root: &Path) -> SessionLibraryState {
        let repository = SessionRepository::open(root.join("metadata"), root.join("sessions"))
            .expect("session repository should open");
        SessionLibraryState::open_repository(repository, saved)
    }

    fn coordinator() -> SessionCoordinator {
        let (event_tx, _event_rx) = mpsc::channel();
        SessionCoordinator::new(event_tx)
    }

    #[test]
    fn launch_request_preserves_resume_identity_paths_generation_and_continuity() {
        let source_session_id = HostedSessionId::new();
        let replacement_session_id = HostedSessionId::new();
        let prior_generation = OccupantGeneration::new(4);
        let replacement_generation = OccupantGeneration::new(5);
        let continuity = DurableContinuityCommit {
            store_root: PathBuf::from("/synthetic/store"),
            expected_revision: Revision::new(7),
            link: ContinuityLink {
                command_id: CommandId::new(),
                source_session_id,
                replacement_session_id,
                runtime_id: RuntimeId::new("codex").unwrap(),
                prior_generation,
                replacement_generation,
                committed_at: 11,
            },
        };
        let spec = SessionStartRequest::launch(
            42,
            replacement_session_id,
            DurableSessionPaths {
                runtime_root: PathBuf::from("/synthetic/runtime"),
                session_dir: PathBuf::from("/synthetic/session"),
            },
            DurableLaunch {
                executable: PathBuf::from("/synthetic/bin/codex"),
                arguments: vec!["resume".to_string(), "opaque-id".to_string()],
                cwd: PathBuf::from("/synthetic/project"),
                runtime_detection: None,
            },
            Some(replacement_generation),
            Some(continuity),
        )
        .into_spec();

        assert_eq!(spec.pane_id, 42);
        assert_eq!(spec.session_id, replacement_session_id);
        assert_eq!(spec.paths.runtime_root, Path::new("/synthetic/runtime"));
        assert_eq!(spec.paths.session_dir, Path::new("/synthetic/session"));
        assert_eq!(spec.from_sequence, OutputSequence::ZERO);
        assert_eq!(
            spec.expected_occupant_generation,
            Some(replacement_generation)
        );

        let launch = spec.launch.unwrap();
        assert_eq!(launch.executable, Path::new("/synthetic/bin/codex"));
        assert_eq!(launch.arguments, ["resume", "opaque-id"]);
        assert_eq!(launch.cwd, Path::new("/synthetic/project"));

        let continuity = spec.continuity.unwrap();
        assert_eq!(continuity.store_root, Path::new("/synthetic/store"));
        assert_eq!(continuity.expected_revision, Revision::new(7));
        assert_eq!(continuity.link.source_session_id, source_session_id);
        assert_eq!(
            continuity.link.replacement_session_id,
            replacement_session_id
        );
        assert_eq!(continuity.link.prior_generation, prior_generation);
        assert_eq!(
            continuity.link.replacement_generation,
            replacement_generation
        );
    }

    #[test]
    fn attach_request_preserves_watermark_without_creating_launch_state() {
        let session_id = HostedSessionId::new();
        let spec = SessionStartRequest::attach(
            9,
            session_id,
            DurableSessionPaths {
                runtime_root: PathBuf::from("/synthetic/runtime"),
                session_dir: PathBuf::from("/synthetic/session"),
            },
            OutputSequence::new(81),
        )
        .into_spec();

        assert_eq!(spec.pane_id, 9);
        assert_eq!(spec.session_id, session_id);
        assert_eq!(spec.from_sequence, OutputSequence::new(81));
        assert!(spec.launch.is_none());
        assert!(spec.expected_occupant_generation.is_none());
        assert!(spec.continuity.is_none());
    }

    #[test]
    fn pane_projection_preserves_gap_and_unavailable_semantics() {
        let session_id = HostedSessionId::new();
        let input = |state| HostedStatusInput {
            hosted_session_id: session_id,
            state,
            last_sequence: 81,
            durable_sequence: 80,
            activity: ActivityAggregate::default(),
            has_writer_lease: true,
            visibly_focused: false,
        };

        let live = input(HostedSessionState::Live).pane_projection();
        assert_eq!(live.last_sequence, 81);
        assert!(live.has_writer_lease);
        assert_eq!(live.dev_url_action, HostedDevUrlAction::Preserve);
        assert_eq!(
            input(HostedSessionState::Gap)
                .pane_projection()
                .dev_url_action,
            HostedDevUrlAction::MarkGap
        );
        assert_eq!(
            input(HostedSessionState::Exited)
                .pane_projection()
                .dev_url_action,
            HostedDevUrlAction::MarkUnavailable
        );
    }

    #[test]
    fn hosted_status_projection_advances_only_max_watermarks_and_observes_transition() {
        let fixture = tempfile::tempdir().unwrap();
        let session_id = HostedSessionId::new();
        let mut saved = SavedState {
            app_attached_sessions: vec![saved_session(session_id, HostedSessionState::Live)],
            ..SavedState::default()
        };
        let mut library = session_library(&mut saved, fixture.path());
        library.pending_archive_after_stop = Some(session_id);
        let persisted = Cell::new(false);
        let mut observer = RecordingActivityObserver::default();

        let projection = coordinator()
            .project_hosted_status(
                HostedStatusInput {
                    hosted_session_id: session_id,
                    state: HostedSessionState::Exited,
                    last_sequence: 12,
                    durable_sequence: 40,
                    activity: ActivityAggregate::default(),
                    has_writer_lease: false,
                    visibly_focused: true,
                },
                &mut saved,
                &mut library,
                &mut observer,
                |_| {
                    persisted.set(true);
                    Ok::<_, std::io::Error>(())
                },
            )
            .unwrap();

        let host = saved.app_attached_sessions[0]
            .durable_host
            .as_ref()
            .unwrap();
        assert_eq!(host.last_sequence, 20);
        assert_eq!(host.durable_sequence, 40);
        assert_eq!(
            library.session(session_id).unwrap().lifecycle,
            HostedSessionState::Exited
        );
        assert!(persisted.get());
        assert_eq!(observer.observations.len(), 1);
        assert_eq!(
            observer.observations[0].0.as_ref().unwrap().lifecycle,
            HostedSessionState::Live
        );
        assert_eq!(
            observer.observations[0].1.lifecycle,
            HostedSessionState::Exited
        );
        assert!(observer.observations[0].2);
        assert_eq!(projection.pending_archive, PendingArchiveAction::Archive);
        assert_eq!(projection.warnings, Default::default());
    }

    #[test]
    fn hosted_status_projection_reports_non_fatal_save_and_activity_failures() {
        let fixture = tempfile::tempdir().unwrap();
        let session_id = HostedSessionId::new();
        let mut saved = SavedState {
            app_attached_sessions: vec![saved_session(session_id, HostedSessionState::Live)],
            ..SavedState::default()
        };
        let mut library = session_library(&mut saved, fixture.path());
        library.pending_archive_after_stop = Some(session_id);
        let mut observer = RecordingActivityObserver {
            failure: Some("notifications unavailable".to_string()),
            ..RecordingActivityObserver::default()
        };

        let projection = coordinator()
            .project_hosted_status(
                HostedStatusInput {
                    hosted_session_id: session_id,
                    state: HostedSessionState::Offline,
                    last_sequence: 21,
                    durable_sequence: 20,
                    activity: ActivityAggregate::default(),
                    has_writer_lease: false,
                    visibly_focused: false,
                },
                &mut saved,
                &mut library,
                &mut observer,
                |_| Err::<(), _>("disk full"),
            )
            .unwrap();

        assert_eq!(projection.pending_archive, PendingArchiveAction::Fail);
        assert_eq!(
            projection.warnings.compatibility_save.as_deref(),
            Some("disk full")
        );
        assert_eq!(
            projection.warnings.activity_observation.as_deref(),
            Some("notifications unavailable")
        );
        assert_eq!(observer.observations.len(), 1);
    }

    #[test]
    fn hosted_status_projection_stops_after_authoritative_reconcile_failure() {
        let fixture = tempfile::tempdir().unwrap();
        let existing_session_id = HostedSessionId::new();
        let missing_session_id = HostedSessionId::new();
        let mut saved = SavedState {
            app_attached_sessions: vec![saved_session(
                existing_session_id,
                HostedSessionState::Live,
            )],
            ..SavedState::default()
        };
        let mut library = session_library(&mut saved, fixture.path());
        let persisted = Cell::new(false);
        let mut observer = RecordingActivityObserver::default();

        let result = coordinator().project_hosted_status(
            HostedStatusInput {
                hosted_session_id: missing_session_id,
                state: HostedSessionState::Offline,
                last_sequence: 21,
                durable_sequence: 20,
                activity: ActivityAggregate::default(),
                has_writer_lease: false,
                visibly_focused: false,
            },
            &mut saved,
            &mut library,
            &mut observer,
            |_| {
                persisted.set(true);
                Ok::<_, std::io::Error>(())
            },
        );

        assert!(result.is_err());
        assert!(!persisted.get());
        assert!(observer.observations.is_empty());
        let host = saved.app_attached_sessions[0]
            .durable_host
            .as_ref()
            .unwrap();
        assert_eq!(host.last_sequence, 20);
        assert_eq!(host.durable_sequence, 30);
    }

    #[test]
    fn session_coordinator_is_the_only_ui_durable_worker_boundary() {
        let app_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/app");
        for entry in fs::read_dir(app_dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|value| value.to_str()) != Some("rs") {
                continue;
            }
            let file_name = path.file_name().and_then(|value| value.to_str()).unwrap();
            if matches!(file_name, "hosted_session.rs" | "session_coordinator.rs") {
                continue;
            }
            let source = fs::read_to_string(&path).unwrap();
            assert!(
                !source.contains("spawn_durable_session("),
                "{file_name} bypasses SessionCoordinator"
            );
        }
    }

    #[test]
    fn hosted_status_reconciliation_does_not_drift_back_into_app_event_loop() {
        let app_source = include_str!("mod.rs");
        assert!(!app_source.contains(".session_library.reconcile("));
        assert!(!app_source.contains(".activity_center.observe_transition("));
    }

    #[test]
    fn coordinator_module_has_no_ui_framework_dependency() {
        let forbidden_crate = ["gp", "ui"].concat();
        assert!(!include_str!("session_coordinator.rs").contains(&forbidden_crate));
    }
}
