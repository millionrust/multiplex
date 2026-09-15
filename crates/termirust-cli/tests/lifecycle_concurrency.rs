mod common;

use std::sync::{Arc, Mutex};

use common::*;
use termirust_cli::{Cancellation, CliCommand, CliData, CommandService, ErrorCode};
use termirust_domain::{
    HostedSessionId, HostedSessionState, Revision, SessionMutation, SessionTitle,
};
use uuid::Uuid;

#[test]
fn lifecycle_commands_launch_stop_archive_restore_and_reject_stale_revision() {
    let seed = seed_store();
    let launcher = Arc::new(Mutex::new(FakeLauncherState::default()));
    let controller = Arc::new(Mutex::new(FakeControllerState::default()));
    let mut service = service(&seed, launcher.clone(), controller.clone());
    let cancellation = Cancellation::default();

    let launched = service
        .execute(
            CliCommand::SessionLaunch {
                project_id: PROJECT_ID,
                preset_id: PRESET_ID,
                group_id: None,
            },
            &cancellation,
        )
        .unwrap();
    let CliData::Mutation(launched) = launched else {
        panic!("expected mutation");
    };
    assert_eq!(launched.outcome, "launched");
    assert_eq!(launched.session.state, "live");
    assert_eq!(launcher.lock().unwrap().calls, 1);

    let stopped = service
        .execute(
            CliCommand::SessionStop {
                session_id: LAUNCH_SESSION_ID,
                expected_revision: None,
                confirmed: true,
            },
            &cancellation,
        )
        .unwrap();
    let CliData::Mutation(stopped) = stopped else {
        panic!("expected mutation");
    };
    assert_eq!(stopped.session.state, "exited");
    assert_eq!(controller.lock().unwrap().calls, 1);

    let archived = service
        .execute(
            CliCommand::SessionArchive {
                session_id: LAUNCH_SESSION_ID,
                expected_revision: None,
            },
            &cancellation,
        )
        .unwrap();
    let CliData::Mutation(archived) = archived else {
        panic!("expected mutation");
    };
    assert!(archived.session.archived);

    let restored = service
        .execute(
            CliCommand::SessionRestore {
                session_id: LAUNCH_SESSION_ID,
                expected_revision: None,
            },
            &cancellation,
        )
        .unwrap();
    let CliData::Mutation(restored) = restored else {
        panic!("expected mutation");
    };
    assert!(!restored.session.archived);

    let repository = seed.sessions();
    let stale = repository.load().unwrap().revision;
    repository
        .mutate_session(
            LAUNCH_SESSION_ID,
            stale,
            SessionMutation::Rename(SessionTitle::new("Changed").unwrap()),
            20,
        )
        .unwrap();
    let error = service
        .execute(
            CliCommand::SessionArchive {
                session_id: LAUNCH_SESSION_ID,
                expected_revision: Some(stale),
            },
            &cancellation,
        )
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Conflict);
    assert_eq!(error.current_revision, Some(stale.get() + 1));
}

#[test]
fn pre_ready_cancellation_cleans_host_and_post_ready_cancellation_detaches_successfully() {
    let seed = seed_store();
    let launcher = Arc::new(Mutex::new(FakeLauncherState {
        calls: 0,
        descriptors: Vec::new(),
        outcome: Some(Ok(
            termirust_cli::HostLaunchOutcome::ReadyAfterPreReadyCancellation,
        )),
        cancel_after_ready: false,
    }));
    let controller = Arc::new(Mutex::new(FakeControllerState::default()));
    let mut first_service = service(&seed, launcher, controller.clone());
    let error = first_service
        .execute(
            CliCommand::SessionLaunch {
                project_id: PROJECT_ID,
                preset_id: PRESET_ID,
                group_id: None,
            },
            &Cancellation::default(),
        )
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Cancelled);
    assert_eq!(controller.lock().unwrap().calls, 1);
    assert_eq!(
        seed.sessions()
            .load()
            .unwrap()
            .sessions
            .iter()
            .find(|session| session.id == LAUNCH_SESSION_ID)
            .unwrap()
            .lifecycle,
        HostedSessionState::Cancelled
    );

    let second = seed_store();
    let cancellation = Cancellation::default();
    let launcher = Arc::new(Mutex::new(FakeLauncherState {
        calls: 0,
        descriptors: Vec::new(),
        outcome: Some(Ok(termirust_cli::HostLaunchOutcome::Ready)),
        cancel_after_ready: true,
    }));
    let mut second_service = service(
        &second,
        launcher,
        Arc::new(Mutex::new(FakeControllerState::default())),
    );
    let post_ready = second_service
        .execute(
            CliCommand::SessionLaunch {
                project_id: PROJECT_ID,
                preset_id: PRESET_ID,
                group_id: None,
            },
            &cancellation,
        )
        .unwrap();
    let CliData::Mutation(post_ready) = post_ready else {
        panic!("expected mutation");
    };
    assert_eq!(post_ready.outcome, "launched");
    assert_eq!(post_ready.session.state, "live");

    let third = seed_store();
    let mut third_service = service(
        &third,
        Arc::new(Mutex::new(FakeLauncherState::default())),
        Arc::new(Mutex::new(FakeControllerState::default())),
    );
    let pre_cancelled = Cancellation::default();
    pre_cancelled.cancel();
    let before_ready = third_service.execute(
        CliCommand::SessionLaunch {
            project_id: PROJECT_ID,
            preset_id: PRESET_ID,
            group_id: None,
        },
        &pre_cancelled,
    );
    assert_eq!(before_ready.unwrap_err().code, ErrorCode::Cancelled);
    assert!(third.sessions().load().unwrap().sessions.is_empty());
}

#[test]
fn archive_refuses_live_session_and_stop_requires_explicit_yes() {
    let seed = seed_store();
    insert_session(&seed, HostedSessionState::Live, false);
    let mut service = service(
        &seed,
        Arc::new(Mutex::new(FakeLauncherState::default())),
        Arc::new(Mutex::new(FakeControllerState::default())),
    );
    let cancellation = Cancellation::default();
    let archive = service.execute(
        CliCommand::SessionArchive {
            session_id: SESSION_ID,
            expected_revision: None,
        },
        &cancellation,
    );
    assert_eq!(archive.unwrap_err().code, ErrorCode::Validation);
    let stop = service.execute(
        CliCommand::SessionStop {
            session_id: SESSION_ID,
            expected_revision: Some(Revision::new(1)),
            confirmed: false,
        },
        &cancellation,
    );
    assert_eq!(stop.unwrap_err().code, ErrorCode::Validation);
}

#[test]
fn expected_revision_tracks_the_target_session_not_unrelated_records() {
    let seed = seed_store();
    let first = insert_session(&seed, HostedSessionState::Exited, false);
    let repository = seed.sessions();
    let snapshot = repository.load().unwrap();
    let mut second = first.clone();
    second.id = HostedSessionId::from_uuid(Uuid::from_u128(7));
    second.title = SessionTitle::new("Unrelated").unwrap();
    second.revision = Revision::ZERO;
    repository
        .create_session(second, snapshot.revision)
        .unwrap();

    let mut service = service(
        &seed,
        Arc::new(Mutex::new(FakeLauncherState::default())),
        Arc::new(Mutex::new(FakeControllerState::default())),
    );
    let archived = service
        .execute(
            CliCommand::SessionArchive {
                session_id: SESSION_ID,
                expected_revision: Some(first.revision),
            },
            &Cancellation::default(),
        )
        .unwrap();
    let CliData::Mutation(archived) = archived else {
        panic!("expected mutation");
    };
    assert!(archived.session.archived);
}
