mod common;

use std::sync::{Arc, Mutex};

use common::*;
use termirust_cli::{Cancellation, CliData, CliError, ErrorCode, ManagementCommand};
use termirust_domain::{CommandId, HostInstanceId, HostLifecycle, HostedSessionState, Revision};
use termirust_store::{HostLease, HostMetadata};

#[test]
fn management_facade_launches_and_applies_revision_scoped_metadata_commands() {
    let seed = seed_store();
    let launcher = Arc::new(Mutex::new(FakeLauncherState::default()));
    let controller = Arc::new(Mutex::new(FakeControllerState::default()));
    let launch_service = service(&seed, launcher.clone(), controller);
    let cancellation = Cancellation::default();

    let launch_command_id = CommandId::new();
    let launched = launch_service
        .execute_management(
            ManagementCommand::Launch {
                command_id: launch_command_id,
                project_id: PROJECT_ID,
                preset_id: PRESET_ID,
                group_id: None,
            },
            &cancellation,
        )
        .unwrap();
    let CliData::Mutation(launched) = launched else {
        panic!("expected launch mutation");
    };
    assert_eq!(launched.outcome, "launched");
    assert_eq!(launcher.lock().unwrap().calls, 1);
    let replayed = launch_service
        .execute_management(
            ManagementCommand::Launch {
                command_id: launch_command_id,
                project_id: PROJECT_ID,
                preset_id: PRESET_ID,
                group_id: None,
            },
            &cancellation,
        )
        .unwrap();
    assert_eq!(mutation(replayed).outcome, "launched");
    assert_eq!(launcher.lock().unwrap().calls, 1);

    let seed = seed_store();
    let session = insert_session(&seed, HostedSessionState::Exited, false);
    let service = service(
        &seed,
        Arc::new(Mutex::new(FakeLauncherState::default())),
        Arc::new(Mutex::new(FakeControllerState::default())),
    );
    let rename_command_id = CommandId::new();
    let renamed = mutation(
        service
            .execute_management(
                ManagementCommand::Rename {
                    command_id: rename_command_id,
                    session_id: SESSION_ID,
                    expected_revision: session.revision,
                    title: "Reviewed title".into(),
                },
                &cancellation,
            )
            .unwrap(),
    );
    assert_eq!(renamed.outcome, "renamed");
    let rename_replay = mutation(
        service
            .execute_management(
                ManagementCommand::Rename {
                    command_id: rename_command_id,
                    session_id: SESSION_ID,
                    expected_revision: session.revision,
                    title: "Reviewed title".into(),
                },
                &cancellation,
            )
            .unwrap(),
    );
    assert_eq!(rename_replay.session.revision, renamed.session.revision);
    let pinned = mutation(
        service
            .execute_management(
                ManagementCommand::SetPinned {
                    command_id: CommandId::new(),
                    session_id: SESSION_ID,
                    expected_revision: Revision::new(renamed.session.revision),
                    pinned: true,
                },
                &cancellation,
            )
            .unwrap(),
    );
    assert_eq!(pinned.outcome, "pinned");
    let marked = mutation(
        service
            .execute_management(
                ManagementCommand::MarkRead {
                    command_id: CommandId::new(),
                    session_id: SESSION_ID,
                    expected_revision: Revision::new(pinned.session.revision),
                },
                &cancellation,
            )
            .unwrap(),
    );
    let archived = mutation(
        service
            .execute_management(
                ManagementCommand::Archive {
                    command_id: CommandId::new(),
                    session_id: SESSION_ID,
                    expected_revision: Revision::new(marked.session.revision),
                },
                &cancellation,
            )
            .unwrap(),
    );
    assert!(archived.session.archived);
    let restored = mutation(
        service
            .execute_management(
                ManagementCommand::Restore {
                    command_id: CommandId::new(),
                    session_id: SESSION_ID,
                    expected_revision: Revision::new(archived.session.revision),
                },
                &cancellation,
            )
            .unwrap(),
    );
    assert!(!restored.session.archived);

    let error = service
        .execute_management(
            ManagementCommand::Rename {
                command_id: CommandId::new(),
                session_id: SESSION_ID,
                expected_revision: session.revision,
                title: "Stale write".into(),
            },
            &cancellation,
        )
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Conflict);
    assert_eq!(error.current_revision, Some(restored.session.revision));
}

#[test]
fn management_stop_passes_the_exact_host_identity_to_the_controller() {
    let seed = seed_store();
    let session = insert_session(&seed, HostedSessionState::Live, false);
    let host_id = HostInstanceId::new();
    let _lease = write_host_metadata(&seed, host_id);
    let controller = Arc::new(Mutex::new(FakeControllerState::default()));
    let service = service(
        &seed,
        Arc::new(Mutex::new(FakeLauncherState::default())),
        controller.clone(),
    );

    let stopped = mutation(
        service
            .execute_management(
                ManagementCommand::Stop {
                    command_id: CommandId::new(),
                    session_id: SESSION_ID,
                    expected_revision: session.revision,
                },
                &Cancellation::default(),
            )
            .unwrap(),
    );
    assert_eq!(stopped.outcome, "stopped");
    assert_eq!(
        controller.lock().unwrap().expected_hosts,
        vec![Some(host_id)]
    );
}

#[test]
fn failed_stop_never_commits_the_following_archive() {
    let seed = seed_store();
    let session = insert_session(&seed, HostedSessionState::Live, false);
    let _lease = write_host_metadata(&seed, HostInstanceId::new());
    let controller = Arc::new(Mutex::new(FakeControllerState {
        calls: 0,
        expected_hosts: Vec::new(),
        result: Some(Err(CliError::new(
            ErrorCode::Timeout,
            "fixture stop timeout",
            "inspect the Session",
        ))),
    }));
    let service = service(
        &seed,
        Arc::new(Mutex::new(FakeLauncherState::default())),
        controller,
    );
    let error = service
        .execute_management(
            ManagementCommand::StopAndArchive {
                command_id: CommandId::new(),
                session_id: SESSION_ID,
                expected_revision: session.revision,
            },
            &Cancellation::default(),
        )
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Timeout);
    let stored = seed.sessions().load().unwrap();
    let stored = stored
        .sessions
        .iter()
        .find(|candidate| candidate.id == SESSION_ID)
        .unwrap();
    assert_eq!(stored.lifecycle, HostedSessionState::Stopping);
    assert!(stored.archived_at.is_none());
}

#[test]
fn management_command_debug_redacts_user_title() {
    let command = ManagementCommand::Rename {
        command_id: CommandId::new(),
        session_id: SESSION_ID,
        expected_revision: Revision::new(1),
        title: "PRIVATE-TITLE-CANARY".into(),
    };
    assert!(!format!("{command:?}").contains("PRIVATE-TITLE-CANARY"));
}

fn write_host_metadata(seed: &SeededStore, host_id: HostInstanceId) -> HostLease {
    let lease = HostLease::acquire(
        seed.config_root
            .join("durable-sessions")
            .join(SESSION_ID.to_string()),
        host_id,
    )
    .unwrap();
    lease
        .write_metadata(&HostMetadata {
            format_version: HostMetadata::FORMAT_VERSION,
            session_id: SESSION_ID,
            host_instance_id: host_id,
            process_token: None,
            runtime_recognition: None,
            activity: Default::default(),
            lifecycle: HostLifecycle::Ready,
            endpoint_name: "fixture.sock".into(),
            heartbeat_monotonic_nanos: 1,
            durability_watermark: None,
        })
        .unwrap();
    lease
}

fn mutation(data: CliData) -> termirust_cli::SessionMutationData {
    let CliData::Mutation(mutation) = data else {
        panic!("expected mutation response");
    };
    mutation
}
