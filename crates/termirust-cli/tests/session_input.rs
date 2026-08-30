#![cfg(unix)]

mod common;

use std::collections::BTreeMap;
use std::io::Write as _;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use common::*;
use termirust_cli::{
    Cancellation, CliCommand, CliData, CommandService, ErrorCode, MAX_SESSION_INPUT_BYTES,
    SessionInput, read_session_input, run_parsed,
};
use termirust_client::{ConnectOptions, HostClient, LocalEndpoint};
use termirust_domain::{HostInstanceId, HostedSessionState, OutputSequence};
use termirust_session_host::{LaunchDescriptor, StopDeadlines, start};
use termirust_store::JournalLimits;
use tokio_util::sync::CancellationToken;

const INPUT_CANARY: &[u8] = b"PRIVATE-SESSION-INPUT-CANARY\n";
static REAL_HOST_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[test]
fn stdin_payload_is_binary_bounded_nonempty_and_debug_redacted() {
    let binary = vec![0, 0xff, b'\n'];
    let input = read_session_input(&mut binary.as_slice()).unwrap();
    assert_eq!(input.byte_len(), 3);
    assert!(!format!("{input:?}").contains("255"));

    assert_eq!(
        read_session_input(&mut [].as_slice()).unwrap_err().code,
        ErrorCode::Validation
    );
    let maximum = vec![b'x'; MAX_SESSION_INPUT_BYTES];
    assert_eq!(
        read_session_input(&mut maximum.as_slice())
            .unwrap()
            .byte_len(),
        MAX_SESSION_INPUT_BYTES as u64
    );
    let oversized = vec![b'x'; MAX_SESSION_INPUT_BYTES + 1];
    assert_eq!(
        read_session_input(&mut oversized.as_slice())
            .unwrap_err()
            .code,
        ErrorCode::ResourceLimit
    );
}

#[test]
fn local_service_validates_session_then_sends_once_without_disclosing_payload() {
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
    let command = CliCommand::SessionInput {
        session_id: SESSION_ID,
        input_stdin: true,
        input: Some(SessionInput::new(INPUT_CANARY.to_vec()).unwrap()),
    };
    assert!(!format!("{command:?}").contains("PRIVATE-SESSION-INPUT-CANARY"));
    let data = service
        .execute(command.clone(), &Cancellation::default())
        .unwrap();
    let CliData::Input(data) = data else {
        panic!("expected input response");
    };
    assert_eq!(data.session_id, SESSION_ID.to_string());
    assert_eq!(data.accepted_bytes, INPUT_CANARY.len() as u64);
    assert!(data.applied);
    let state = controller.lock().unwrap();
    assert_eq!(state.input_calls, 1);
    assert_eq!(state.inputs, vec![INPUT_CANARY]);
    assert_eq!(state.expected_hosts, vec![Some(host_id)]);
    drop(state);

    let rendered = run_parsed(
        Some(&mut service),
        termirust_cli::Invocation {
            json: true,
            command,
        },
        80,
        &Cancellation::default(),
    );
    assert_eq!(rendered.exit_code, 0);
    assert!(
        !rendered
            .stdout
            .windows(INPUT_CANARY.len())
            .any(|w| w == INPUT_CANARY)
    );
    assert!(
        !rendered
            .stderr
            .windows(INPUT_CANARY.len())
            .any(|w| w == INPUT_CANARY)
    );
}

#[test]
fn invalid_state_missing_metadata_and_pre_dispatch_cancellation_never_call_host() {
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
        let error = service
            .execute(input_command(), &Cancellation::default())
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Validation);
        assert_eq!(controller.lock().unwrap().input_calls, 0);
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
            .execute(input_command(), &Cancellation::default())
            .unwrap_err()
            .code,
        ErrorCode::Unavailable
    );
    let _lease = write_ready_host_metadata(&seed, HostInstanceId::new());
    let cancelled = Cancellation::default();
    cancelled.cancel();
    assert_eq!(
        service
            .execute(input_command(), &cancelled)
            .unwrap_err()
            .code,
        ErrorCode::Cancelled
    );
    assert_eq!(controller.lock().unwrap().input_calls, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn packaged_cli_sends_once_and_disconnect_leaves_durable_host_running() {
    let _fixture_guard = REAL_HOST_TEST_LOCK.lock().await;
    let seed = seed_store();
    insert_session(&seed, HostedSessionState::Live, false);
    let descriptor = descriptor(
        &seed,
        seed.config_root
            .join("durable-sessions")
            .join(SESSION_ID.to_string()),
    );
    let host = start(descriptor).await.unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_termirust-cli"))
        .args([
            "session",
            "input",
            &SESSION_ID.to_string(),
            "--input-stdin",
            "--json",
        ])
        .env("TERMIRUST_CONFIG_DIR", &seed.config_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(INPUT_CANARY).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output
            .stdout
            .windows(INPUT_CANARY.len())
            .any(|w| w == INPUT_CANARY)
    );
    assert!(
        !output
            .stderr
            .windows(INPUT_CANARY.len())
            .any(|w| w == INPUT_CANARY)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["data"]["accepted_bytes"], INPUT_CANARY.len() as u64);

    let endpoint = LocalEndpoint::for_config_root(&seed.config_root, SESSION_ID);
    let cancel = CancellationToken::new();
    let mut reader = HostClient::connect(
        endpoint,
        ConnectOptions::local_read_only(SESSION_ID, [7; 32]),
        &cancel,
    )
    .await
    .unwrap();
    let mut observed = Vec::new();
    for _ in 0..100 {
        observed = reader
            .attach(OutputSequence::ZERO, 80, 24, &cancel)
            .await
            .unwrap()
            .into_iter()
            .flat_map(|record| record.bytes)
            .collect();
        if observed
            .windows(b"ECHO:PRIVATE-SESSION-INPUT-CANARY".len())
            .any(|window| window == b"ECHO:PRIVATE-SESSION-INPUT-CANARY")
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        observed
            .windows(b"ECHO:PRIVATE-SESSION-INPUT-CANARY".len())
            .any(|window| { window == b"ECHO:PRIVATE-SESSION-INPUT-CANARY" })
    );
    assert_eq!(
        host.stats().await.lifecycle,
        termirust_domain::HostLifecycle::Ready
    );
    reader.disconnect();
    host.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn writer_lease_contention_fails_without_sending_or_stealing() {
    let _fixture_guard = REAL_HOST_TEST_LOCK.lock().await;
    let seed = seed_store();
    insert_session(&seed, HostedSessionState::Live, false);
    let descriptor = descriptor(
        &seed,
        seed.config_root
            .join("durable-sessions")
            .join(SESSION_ID.to_string()),
    );
    let host = start(descriptor).await.unwrap();
    let endpoint = LocalEndpoint::for_config_root(&seed.config_root, SESSION_ID);
    let cancel = CancellationToken::new();
    let mut owner = HostClient::connect(
        endpoint,
        ConnectOptions::local(SESSION_ID, [8; 32]),
        &cancel,
    )
    .await
    .unwrap();
    assert!(owner.get_state(&cancel).await.unwrap().has_writer_lease);

    let mut child = Command::new(env!("CARGO_BIN_EXE_termirust-cli"))
        .args(["session", "input", &SESSION_ID.to_string(), "--input-stdin"])
        .env("TERMIRUST_CONFIG_DIR", &seed.config_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(INPUT_CANARY).unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&output.stderr).contains("writer lease"));
    assert!(owner.get_state(&cancel).await.unwrap().has_writer_lease);
    owner.disconnect();
    host.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_recorded_host_identity_is_rejected_before_input_dispatch() {
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

    let mut child = Command::new(env!("CARGO_BIN_EXE_termirust-cli"))
        .args(["session", "input", &SESSION_ID.to_string(), "--input-stdin"])
        .env("TERMIRUST_CONFIG_DIR", &seed.config_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(INPUT_CANARY).unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&output.stderr).contains("identity changed"));
    assert!(
        !output
            .stderr
            .windows(INPUT_CANARY.len())
            .any(|w| w == INPUT_CANARY)
    );

    let endpoint = LocalEndpoint::for_config_root(&seed.config_root, SESSION_ID);
    let cancel = CancellationToken::new();
    let mut reader = HostClient::connect(
        endpoint,
        ConnectOptions::local_read_only(SESSION_ID, [9; 32]),
        &cancel,
    )
    .await
    .unwrap();
    let replay = reader
        .attach(OutputSequence::ZERO, 80, 24, &cancel)
        .await
        .unwrap()
        .into_iter()
        .flat_map(|record| record.bytes)
        .collect::<Vec<_>>();
    assert!(
        !replay
            .windows(INPUT_CANARY.len())
            .any(|w| w == INPUT_CANARY)
    );
    reader.disconnect();
    host.shutdown().await.unwrap();
}

fn input_command() -> CliCommand {
    CliCommand::SessionInput {
        session_id: SESSION_ID,
        input_stdin: true,
        input: Some(SessionInput::new(INPUT_CANARY.to_vec()).unwrap()),
    }
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
            "printf 'READY\\n'; while IFS= read -r line; do printf 'ECHO:%s\\n' \"$line\"; done"
                .into(),
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
