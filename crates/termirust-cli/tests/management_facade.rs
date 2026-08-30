mod common;

use std::fs;
use std::sync::{Arc, Mutex};

use common::*;
use termirust_cli::{Cancellation, CliData, CliError, ErrorCode, ManagementCommand};
use termirust_domain::{
    CommandId, HostInstanceId, HostLifecycle, HostedSessionState, Revision, SessionMutation,
};
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

#[test]
fn removal_previews_exact_manifest_and_quarantines_only_owned_data() {
    let seed = seed_store();
    let session = insert_session(&seed, HostedSessionState::Exited, true);
    let session_root = seed
        .config_root
        .join("durable-sessions")
        .join(SESSION_ID.to_string());
    fs::create_dir_all(session_root.join("transcripts")).unwrap();
    fs::write(session_root.join("journal-1.trj"), b"journal").unwrap();
    fs::write(
        session_root.join("transcripts").join("records.jsonl"),
        b"private transcript fixture",
    )
    .unwrap();
    let sentinel = seed.temp.path().join("unrelated-sentinel");
    fs::write(&sentinel, b"must-survive").unwrap();
    let service = service(
        &seed,
        Arc::new(Mutex::new(FakeLauncherState::default())),
        Arc::new(Mutex::new(FakeControllerState::default())),
    );
    let cancellation = Cancellation::default();

    let preview = service
        .prepare_management_removal(SESSION_ID, session.revision, &cancellation)
        .unwrap();
    assert_eq!(preview.session_id, SESSION_ID);
    assert_eq!(preview.manifest.journal_bytes, 7);
    assert_eq!(preview.manifest.transcript_bytes, 26);
    assert!(preview.manifest.requires_title_confirmation());

    let mismatch = service
        .execute_management(
            ManagementCommand::Remove {
                command_id: CommandId::new(),
                session_id: SESSION_ID,
                expected_revision: preview.expected_revision,
                expected_manifest: preview.manifest,
                title_confirmation: Some("wrong title".into()),
            },
            &cancellation,
        )
        .unwrap_err();
    assert_eq!(mismatch.code, ErrorCode::Validation);
    assert_eq!(seed.sessions().load().unwrap().sessions.len(), 1);

    let removed = mutation(
        service
            .execute_management(
                ManagementCommand::Remove {
                    command_id: CommandId::new(),
                    session_id: SESSION_ID,
                    expected_revision: preview.expected_revision,
                    expected_manifest: preview.manifest,
                    title_confirmation: Some("Counter session".into()),
                },
                &cancellation,
            )
            .unwrap(),
    );
    assert_eq!(removed.outcome, "removed");
    assert!(seed.sessions().load().unwrap().sessions.is_empty());
    assert!(!session_root.exists());
    assert!(
        seed.config_root
            .join("durable-session-quarantine")
            .join(SESSION_ID.to_string())
            .is_dir()
    );
    assert_eq!(fs::read(sentinel).unwrap(), b"must-survive");
}

#[test]
fn metadata_only_removal_requires_the_reviewed_remove_token() {
    let seed = seed_store();
    let session = insert_session(&seed, HostedSessionState::Exited, true);
    let service = service(
        &seed,
        Arc::new(Mutex::new(FakeLauncherState::default())),
        Arc::new(Mutex::new(FakeControllerState::default())),
    );
    let cancellation = Cancellation::default();
    let preview = service
        .prepare_management_removal(SESSION_ID, session.revision, &cancellation)
        .unwrap();
    assert!(!preview.manifest.requires_title_confirmation());

    let error = service
        .execute_management(
            ManagementCommand::Remove {
                command_id: CommandId::new(),
                session_id: SESSION_ID,
                expected_revision: preview.expected_revision,
                expected_manifest: preview.manifest,
                title_confirmation: None,
            },
            &cancellation,
        )
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Validation);
    assert_eq!(seed.sessions().load().unwrap().sessions.len(), 1);

    let removed = mutation(
        service
            .execute_management(
                ManagementCommand::Remove {
                    command_id: CommandId::new(),
                    session_id: SESSION_ID,
                    expected_revision: preview.expected_revision,
                    expected_manifest: preview.manifest,
                    title_confirmation: Some("REMOVE".into()),
                },
                &cancellation,
            )
            .unwrap(),
    );
    assert_eq!(removed.outcome, "removed");
    assert!(seed.sessions().load().unwrap().sessions.is_empty());
}

#[test]
fn removal_rejects_cancelled_preview_and_manifest_race() {
    let seed = seed_store();
    let session = insert_session(&seed, HostedSessionState::Exited, true);
    let session_root = seed
        .config_root
        .join("durable-sessions")
        .join(SESSION_ID.to_string());
    fs::create_dir_all(&session_root).unwrap();
    let service = service(
        &seed,
        Arc::new(Mutex::new(FakeLauncherState::default())),
        Arc::new(Mutex::new(FakeControllerState::default())),
    );
    let cancelled = Cancellation::default();
    cancelled.cancel();
    assert_eq!(
        service
            .prepare_management_removal(SESSION_ID, session.revision, &cancelled)
            .unwrap_err()
            .code,
        ErrorCode::Cancelled
    );

    let preview = service
        .prepare_management_removal(SESSION_ID, session.revision, &Cancellation::default())
        .unwrap();
    fs::create_dir_all(session_root.join("artifacts")).unwrap();
    fs::write(
        session_root.join("artifacts").join("changed-after-preview"),
        b"changed",
    )
    .unwrap();
    let error = service
        .execute_management(
            ManagementCommand::Remove {
                command_id: CommandId::new(),
                session_id: SESSION_ID,
                expected_revision: preview.expected_revision,
                expected_manifest: preview.manifest,
                title_confirmation: None,
            },
            &Cancellation::default(),
        )
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Conflict);
    assert_eq!(seed.sessions().load().unwrap().sessions.len(), 1);
    assert!(session_root.is_dir());
}

#[test]
fn removal_rejects_metadata_revision_race() {
    let seed = seed_store();
    let session = insert_session(&seed, HostedSessionState::Exited, true);
    let service = service(
        &seed,
        Arc::new(Mutex::new(FakeLauncherState::default())),
        Arc::new(Mutex::new(FakeControllerState::default())),
    );
    let preview = service
        .prepare_management_removal(SESSION_ID, session.revision, &Cancellation::default())
        .unwrap();
    let repository = seed.sessions();
    repository
        .mutate_session(
            SESSION_ID,
            preview.expected_revision,
            SessionMutation::SetPinned(true),
            2,
        )
        .unwrap();

    let error = service
        .execute_management(
            ManagementCommand::Remove {
                command_id: CommandId::new(),
                session_id: SESSION_ID,
                expected_revision: preview.expected_revision,
                expected_manifest: preview.manifest,
                title_confirmation: None,
            },
            &Cancellation::default(),
        )
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Conflict);
    assert!(repository.load().unwrap().sessions[0].pinned);
}

#[cfg(unix)]
#[test]
fn removal_preview_rejects_symlinked_session_data_and_preserves_target() {
    use std::os::unix::fs::symlink;

    let seed = seed_store();
    let session = insert_session(&seed, HostedSessionState::Exited, true);
    let outside = seed.temp.path().join("outside-owned-root");
    fs::create_dir_all(&outside).unwrap();
    let sentinel = outside.join("sentinel");
    fs::write(&sentinel, b"must-survive").unwrap();
    let session_root = seed
        .config_root
        .join("durable-sessions")
        .join(SESSION_ID.to_string());
    fs::create_dir_all(session_root.parent().unwrap()).unwrap();
    symlink(&outside, &session_root).unwrap();
    let service = service(
        &seed,
        Arc::new(Mutex::new(FakeLauncherState::default())),
        Arc::new(Mutex::new(FakeControllerState::default())),
    );

    let error = service
        .prepare_management_removal(SESSION_ID, session.revision, &Cancellation::default())
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Unavailable);
    assert_eq!(fs::read(sentinel).unwrap(), b"must-survive");
    assert_eq!(seed.sessions().load().unwrap().sessions.len(), 1);
}

#[test]
fn removal_command_debug_redacts_confirmation() {
    let command = ManagementCommand::Remove {
        command_id: CommandId::new(),
        session_id: SESSION_ID,
        expected_revision: Revision::new(1),
        expected_manifest: Default::default(),
        title_confirmation: Some("PRIVATE-REMOVAL-CANARY".into()),
    };
    assert!(!format!("{command:?}").contains("PRIVATE-REMOVAL-CANARY"));
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
