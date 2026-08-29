use std::sync::mpsc::Sender;

use termirust_domain::{HostedSessionId, OccupantGeneration, OutputSequence};

use super::hosted_session::{
    DurableContinuityCommit, DurableLaunch, DurableSessionPaths, DurableSessionSpec,
    spawn_durable_session,
};
use crate::ssh::{SessionRuntimeHandle, SshEvent};

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
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use termirust_domain::{
        CommandId, ContinuityLink, HostedSessionId, OccupantGeneration, OutputSequence, Revision,
        RuntimeId,
    };

    use super::{DurableContinuityCommit, DurableLaunch, DurableSessionPaths, SessionStartRequest};

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
    fn coordinator_module_has_no_ui_framework_dependency() {
        let forbidden_crate = ["gp", "ui"].concat();
        assert!(!include_str!("session_coordinator.rs").contains(&forbidden_crate));
    }
}
