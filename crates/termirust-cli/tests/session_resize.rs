#![cfg(unix)]

mod common;

use std::collections::BTreeMap;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use common::*;
use termirust_cli::{
    Cancellation, CliCommand, CliData, CommandService, ErrorCode, RenderOptions, render_success,
};
use termirust_client::{ConnectOptions, HostClient, LocalEndpoint};
use termirust_domain::{CommandId, HostInstanceId, HostedSessionState, OutputSequence};
use termirust_session_host::{LaunchDescriptor, StopDeadlines, start};
use termirust_store::JournalLimits;
use tokio_util::sync::CancellationToken;

const COLUMNS: u16 = 101;
const ROWS: u16 = 31;
static REAL_HOST_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[test]
fn local_service_validates_session_then_resizes_once_with_stable_output() {
    let seed = seed_store();
    insert_session(&seed, HostedSessionState::Live, false);
    let host_id = HostInstanceId::new();
    let _lease = write_ready_host_metadata(&seed, host_id);
    let controller = Arc::new(Mutex::new(FakeControllerState::default()));
    let mut service = service(
        &seed,
        Arc::new(Mutex::new(FakeLauncherState::default())),
        controller.clone(),
    );
    let data = service
        .execute(resize_command(), &Cancellation::default())
        .unwrap();
    let CliData::Resize(data) = data else {
        panic!("expected resize response");
    };
    assert_eq!(data.session_id, SESSION_ID.to_string());
    assert_eq!((data.columns, data.rows), (COLUMNS, ROWS));
    assert!(data.applied);
    let state = controller.lock().unwrap();
    assert_eq!(state.resize_calls, 1);
    assert_eq!(state.resize_requests, vec![(COLUMNS, ROWS)]);
    assert_eq!(state.expected_hosts, vec![Some(host_id)]);
    drop(state);

    let human = render_success(
        &CliData::Resize(data),
        &[],
        RenderOptions {
            json: false,
            terminal_width: 80,
        },
    )
    .unwrap();
    let human = String::from_utf8(human).unwrap();
    assert!(human.contains("Columns: 101"));
    assert!(human.contains("Rows: 31"));
}

#[test]
fn invalid_state_missing_metadata_and_pre_dispatch_cancellation_never_call_host() {
    let seed = seed_store();
    insert_session(&seed, HostedSessionState::Live, false);
    let controller = Arc::new(Mutex::new(FakeControllerState::default()));
    let mut invalid_service = service(
        &seed,
        Arc::new(Mutex::new(FakeLauncherState::default())),
        controller.clone(),
    );
    assert_eq!(
        invalid_service
            .execute(
                CliCommand::SessionResize {
                    session_id: SESSION_ID,
                    columns: 1_001,
                    rows: ROWS,
                },
                &Cancellation::default(),
            )
            .unwrap_err()
            .code,
        ErrorCode::Validation
    );
    assert_eq!(controller.lock().unwrap().resize_calls, 0);

    for state in [
        HostedSessionState::Exited,
        HostedSessionState::RunningAppAttached,
    ] {
        let seed = seed_store();
        insert_session(&seed, state, false);
        let controller = Arc::new(Mutex::new(FakeControllerState::default()));
        let mut service = service(
            &seed,
            Arc::new(Mutex::new(FakeLauncherState::default())),
            controller.clone(),
        );
        assert_eq!(
            service
                .execute(resize_command(), &Cancellation::default())
                .unwrap_err()
                .code,
            ErrorCode::Validation
        );
        assert_eq!(controller.lock().unwrap().resize_calls, 0);
    }

    let seed = seed_store();
    insert_session(&seed, HostedSessionState::Live, false);
    let controller = Arc::new(Mutex::new(FakeControllerState::default()));
    let mut service = service(
        &seed,
        Arc::new(Mutex::new(FakeLauncherState::default())),
        controller.clone(),
    );
    assert_eq!(
        service
            .execute(resize_command(), &Cancellation::default())
            .unwrap_err()
            .code,
        ErrorCode::Unavailable
    );
    let _lease = write_ready_host_metadata(&seed, HostInstanceId::new());
    let cancelled = Cancellation::default();
    cancelled.cancel();
    assert_eq!(
        service
            .execute(resize_command(), &cancelled)
            .unwrap_err()
            .code,
        ErrorCode::Cancelled
    );
    assert_eq!(controller.lock().unwrap().resize_calls, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn packaged_cli_changes_real_pty_dimensions_and_leaves_host_running() {
    let _fixture_guard = REAL_HOST_TEST_LOCK.lock().await;
    let seed = seed_store();
    insert_session(&seed, HostedSessionState::Live, false);
    let host = start(descriptor(
        &seed,
        seed.config_root
            .join("durable-sessions")
            .join(SESSION_ID.to_string()),
    ))
    .await
    .unwrap();

    let output = packaged_resize(&seed);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["data"]["columns"], COLUMNS);
    assert_eq!(json["data"]["rows"], ROWS);
    assert_eq!(json["data"]["applied"], true);

    let endpoint = LocalEndpoint::for_config_root(&seed.config_root, SESSION_ID);
    let cancel = CancellationToken::new();
    let mut writer = HostClient::connect(
        endpoint.clone(),
        ConnectOptions::local(SESSION_ID, [7; 32]),
        &cancel,
    )
    .await
    .unwrap();
    assert!(writer.get_state(&cancel).await.unwrap().has_writer_lease);
    assert!(
        writer
            .input(CommandId::new(), b"size\n".to_vec(), &cancel)
            .await
            .unwrap()
    );
    writer.disconnect();

    let mut reader = HostClient::connect(
        endpoint,
        ConnectOptions::local_read_only(SESSION_ID, [8; 32]),
        &cancel,
    )
    .await
    .unwrap();
    let mut observed = Vec::new();
    for _ in 0..100 {
        observed = reader
            .attach(
                OutputSequence::ZERO,
                u32::from(COLUMNS),
                u32::from(ROWS),
                &cancel,
            )
            .await
            .unwrap()
            .into_iter()
            .flat_map(|record| record.bytes)
            .collect();
        if observed
            .windows(b"SIZE:31 101".len())
            .any(|w| w == b"SIZE:31 101")
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        observed
            .windows(b"SIZE:31 101".len())
            .any(|w| w == b"SIZE:31 101"),
        "{}",
        String::from_utf8_lossy(&observed)
    );
    assert_eq!(
        host.stats().await.lifecycle,
        termirust_domain::HostLifecycle::Ready
    );
    reader.disconnect();
    host.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn writer_lease_contention_fails_without_resizing_or_stealing() {
    let _fixture_guard = REAL_HOST_TEST_LOCK.lock().await;
    let seed = seed_store();
    insert_session(&seed, HostedSessionState::Live, false);
    let host = start(descriptor(
        &seed,
        seed.config_root
            .join("durable-sessions")
            .join(SESSION_ID.to_string()),
    ))
    .await
    .unwrap();
    let endpoint = LocalEndpoint::for_config_root(&seed.config_root, SESSION_ID);
    let cancel = CancellationToken::new();
    let mut owner = HostClient::connect(
        endpoint,
        ConnectOptions::local(SESSION_ID, [9; 32]),
        &cancel,
    )
    .await
    .unwrap();
    assert!(owner.get_state(&cancel).await.unwrap().has_writer_lease);

    let output = packaged_resize(&seed);
    assert_eq!(output.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&output.stdout).contains("writer lease"));
    assert!(owner.get_state(&cancel).await.unwrap().has_writer_lease);
    owner.disconnect();
    host.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_recorded_host_identity_is_rejected_before_resize_dispatch() {
    let _fixture_guard = REAL_HOST_TEST_LOCK.lock().await;
    let seed = seed_store();
    insert_session(&seed, HostedSessionState::Live, false);
    let actual_host_id = HostInstanceId::new();
    let mut descriptor = descriptor(&seed, seed.temp.path().join("actual-host-session"));
    descriptor.host_instance_id = actual_host_id;
    let host = start(descriptor).await.unwrap();
    let stale_host_id = HostInstanceId::new();
    assert_ne!(actual_host_id, stale_host_id);
    let _stale_metadata = write_ready_host_metadata(&seed, stale_host_id);

    let output = packaged_resize(&seed);
    assert_eq!(output.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&output.stdout).contains("identity changed"));
    assert_eq!(
        host.stats().await.lifecycle,
        termirust_domain::HostLifecycle::Ready
    );
    host.shutdown().await.unwrap();
}

fn resize_command() -> CliCommand {
    CliCommand::SessionResize {
        session_id: SESSION_ID,
        columns: COLUMNS,
        rows: ROWS,
    }
}

fn packaged_resize(seed: &SeededStore) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_termirust-cli"))
        .args([
            "session",
            "resize",
            &SESSION_ID.to_string(),
            "--columns",
            &COLUMNS.to_string(),
            "--rows",
            &ROWS.to_string(),
            "--json",
        ])
        .env("TERMIRUST_CONFIG_DIR", &seed.config_root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap()
}

fn descriptor(seed: &SeededStore, session_dir: std::path::PathBuf) -> LaunchDescriptor {
    LaunchDescriptor {
        format_version: LaunchDescriptor::FORMAT_VERSION,
        session_id: SESSION_ID,
        host_instance_id: HostInstanceId::new(),
        expected_occupant_generation: None,
        runtime_root: LocalEndpoint::for_config_root(&seed.config_root, SESSION_ID)
            .runtime_root()
            .to_path_buf(),
        session_dir,
        executable: "/bin/sh".into(),
        runtime_detection: None,
        arguments: vec![
            "-c".into(),
            "printf 'READY\\n'; while IFS= read -r line; do if [ \"$line\" = size ]; then printf 'SIZE:'; stty size; fi; done".into(),
        ],
        environment: BTreeMap::from([("PATH".into(), "/usr/bin:/bin".into())]),
        cwd: Some(seed.project_root.clone()),
        columns: 80,
        rows: 24,
        journal_limits: JournalLimits::default(),
        stop_deadlines: StopDeadlines {
            interrupt_millis: 50,
            terminate_millis: 100,
            total_millis: 500,
        },
    }
}
