mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::*;
use termirust_cli::{
    Cancellation, CliCommand, CliPaths, CommandService, ErrorCode, LocalCommandService,
};

#[test]
fn missing_and_newer_stores_have_stable_safe_errors() {
    let missing = tempfile::tempdir().unwrap();
    let mut missing_service = LocalCommandService::open(CliPaths::new(
        missing.path().join("missing"),
        missing.path().join("missing-host"),
    ));
    let missing_error = missing_service
        .execute(CliCommand::Status, &Cancellation::default())
        .unwrap_err();
    assert_eq!(missing_error.code, ErrorCode::Unavailable);
    assert!(
        !missing_error
            .message
            .contains(&missing.path().display().to_string())
    );

    let seed = seed_store();
    std::fs::write(
        seed.metadata_root.join("format.json"),
        br#"{"format_version":99,"minimum_reader":99,"instance_id":"00000000-0000-0000-0000-000000000001"}"#,
    )
    .unwrap();
    let mut newer_service = service(
        &seed,
        Arc::new(Mutex::new(FakeLauncherState::default())),
        Arc::new(Mutex::new(FakeControllerState::default())),
    );
    let newer_error = newer_service
        .execute(CliCommand::ProjectList, &Cancellation::default())
        .unwrap_err();
    assert_eq!(newer_error.code, ErrorCode::Incompatible);
    assert!(!newer_error.message.contains("format.json"));
}

#[cfg(unix)]
#[test]
fn metadata_lock_contention_times_out_without_hanging() {
    use std::fs::OpenOptions;
    use std::os::fd::AsRawFd as _;

    let seed = seed_store();
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(seed.metadata_root.join("metadata.lock"))
        .unwrap();
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
    let mut service = service(
        &seed,
        Arc::new(Mutex::new(FakeLauncherState::default())),
        Arc::new(Mutex::new(FakeControllerState::default())),
    );
    let started = Instant::now();
    let error = service
        .execute(CliCommand::ProjectList, &Cancellation::default())
        .unwrap_err();
    let elapsed = started.elapsed();
    assert_eq!(error.code, ErrorCode::Timeout);
    assert!(elapsed >= Duration::from_millis(1_900));
    assert!(elapsed < Duration::from_secs(4));
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_UN) }, 0);
}

#[test]
fn debug_views_do_not_expose_local_paths_or_launch_arguments() {
    let seed = seed_store();
    let path_debug = format!("{:?}", seed.paths());
    assert!(path_debug.contains("<redacted>"));
    assert!(!path_debug.contains(&seed.project_root.display().to_string()));

    let launcher = Arc::new(Mutex::new(FakeLauncherState::default()));
    let mut service = service(
        &seed,
        launcher,
        Arc::new(Mutex::new(FakeControllerState::default())),
    );
    service
        .execute(
            CliCommand::SessionLaunch {
                project_id: PROJECT_ID,
                preset_id: PRESET_ID,
                group_id: None,
            },
            &Cancellation::default(),
        )
        .unwrap();
}
