#![cfg(unix)]

mod common;

use std::collections::BTreeMap;
use std::io::{Read as _, Write as _};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use common::*;
use portable_pty::{CommandBuilder, ExitStatus, PtySize, native_pty_system};
use termirust_cli::{
    Cancellation, CliCommand, CliData, CommandService, ErrorCode, RenderOptions, render_success,
};
use termirust_client::{ConnectOptions, HostClient, LocalEndpoint};
use termirust_domain::{HostInstanceId, HostedSessionState, OutputSequence};
use termirust_session_host::{LaunchDescriptor, StopDeadlines, start};
use termirust_store::JournalLimits;
use tokio_util::sync::CancellationToken;

const FROM_SEQUENCE: u64 = 3;
const COLUMNS: u16 = 101;
const ROWS: u16 = 31;
const SECRET_CANARY: &str = "ATTACH-TERMINAL-BYTES-MUST-NOT-ENTER-JSON";
static REAL_HOST_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[test]
fn local_service_validates_identity_and_returns_only_a_bounded_attach_summary() {
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
        .execute(attach_command(false), &Cancellation::default())
        .unwrap();
    let CliData::Attach(data) = data else {
        panic!("expected attach summary");
    };
    assert_eq!(data.session_id, SESSION_ID.to_string());
    assert_eq!(data.lifecycle, "ready");
    assert_eq!(data.from_sequence, FROM_SEQUENCE);
    assert_eq!(data.latest_sequence, 9);
    assert_eq!(data.replayed_records, 2);
    assert_eq!(data.replayed_bytes, 12);
    assert!(!data.snapshot);
    assert!(!data.writer_lease);
    let state = controller.lock().unwrap();
    assert_eq!(state.attach_calls, 1);
    assert_eq!(state.expected_hosts, vec![Some(host_id)]);
    assert_eq!(state.attach_requests.len(), 1);
    assert_eq!(state.attach_requests[0].columns, COLUMNS);
    assert_eq!(state.attach_requests[0].rows, ROWS);
    assert!(!state.attach_requests[0].request_control);
    drop(state);

    let human = String::from_utf8(
        render_success(
            &CliData::Attach(data),
            &[],
            RenderOptions {
                json: false,
                terminal_width: 80,
            },
        )
        .unwrap(),
    )
    .unwrap();
    assert!(human.contains("Session attach summary"));
    assert!(human.contains("Writer lease: read-only"));
}

#[test]
fn unsafe_state_metadata_and_pre_dispatch_cancellation_never_call_host() {
    for (state, archived) in [
        (HostedSessionState::Exited, true),
        (HostedSessionState::RunningAppAttached, false),
    ] {
        let seed = seed_store();
        insert_session(&seed, state, archived);
        let _lease = write_ready_host_metadata(&seed, HostInstanceId::new());
        let controller = Arc::new(Mutex::new(FakeControllerState::default()));
        let mut service = service(
            &seed,
            Arc::new(Mutex::new(FakeLauncherState::default())),
            controller.clone(),
        );
        assert_eq!(
            service
                .execute(attach_command(false), &Cancellation::default())
                .unwrap_err()
                .code,
            ErrorCode::Validation
        );
        assert_eq!(controller.lock().unwrap().attach_calls, 0);
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
            .execute(attach_command(false), &Cancellation::default())
            .unwrap_err()
            .code,
        ErrorCode::Unavailable
    );
    let _lease = write_ready_host_metadata(&seed, HostInstanceId::new());
    let cancelled = Cancellation::default();
    cancelled.cancel();
    assert_eq!(
        service
            .execute(attach_command(false), &cancelled)
            .unwrap_err()
            .code,
        ErrorCode::Cancelled
    );
    assert_eq!(controller.lock().unwrap().attach_calls, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn packaged_json_attach_never_emits_terminal_bytes_and_leaves_host_running() {
    let _fixture_guard = REAL_HOST_TEST_LOCK.lock().await;
    let seed = seed_store();
    insert_session(&seed, HostedSessionState::Live, false);
    let host = start(descriptor(&seed)).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let output = packaged_attach(&seed, false);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.contains(SECRET_CANARY));
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(json["data"]["session_id"], SESSION_ID.to_string());
    assert_eq!(json["data"]["lifecycle"], "ready");
    assert_eq!(json["data"]["writer_lease"], false);
    assert!(
        json["data"]["snapshot"].as_bool().unwrap()
            || json["data"]["replayed_records"].as_u64().unwrap() > 0
    );
    assert_eq!(
        host.stats().await.lifecycle,
        termirust_domain::HostLifecycle::Ready
    );
    host.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_writer_attach_fails_without_stealing_an_existing_lease() {
    let _fixture_guard = REAL_HOST_TEST_LOCK.lock().await;
    let seed = seed_store();
    insert_session(&seed, HostedSessionState::Live, false);
    let host = start(descriptor(&seed)).await.unwrap();
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

    let output = packaged_attach(&seed, true);
    assert_eq!(output.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&output.stdout).contains("writer lease"));
    assert!(owner.get_state(&cancel).await.unwrap().has_writer_lease);
    owner.disconnect();
    host.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_recorded_host_identity_is_rejected_before_attach_replay() {
    let _fixture_guard = REAL_HOST_TEST_LOCK.lock().await;
    let seed = seed_store();
    insert_session(&seed, HostedSessionState::Live, false);
    let actual_host_id = HostInstanceId::new();
    let mut descriptor = descriptor(&seed);
    descriptor.host_instance_id = actual_host_id;
    descriptor.session_dir = seed.temp.path().join("actual-attach-host");
    let host = start(descriptor).await.unwrap();
    let stale_host_id = HostInstanceId::new();
    assert_ne!(actual_host_id, stale_host_id);
    let _stale_metadata = write_ready_host_metadata(&seed, stale_host_id);

    let output = packaged_attach(&seed, false);
    assert_eq!(output.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&output.stdout).contains("identity changed"));
    assert_eq!(
        host.stats().await.lifecycle,
        termirust_domain::HostLifecycle::Ready
    );
    host.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn packaged_human_attach_replays_writes_resizes_detaches_and_restores_terminal() {
    let _fixture_guard = REAL_HOST_TEST_LOCK.lock().await;
    let seed = seed_store();
    insert_session(&seed, HostedSessionState::Live, false);
    let host = start(descriptor(&seed)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let pty = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let before = pty.master.get_termios().unwrap();
    let reader = pty.master.try_clone_reader().unwrap();
    let mut writer = pty.master.take_writer().unwrap();
    let output = Arc::new(Mutex::new(Vec::new()));
    let reader_thread = spawn_pty_reader(reader, Arc::clone(&output));
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_termirust-cli"));
    command.args(["session", "attach", &SESSION_ID.to_string(), "--write"]);
    command.env("TERM", "xterm-256color");
    command.env("TERMIRUST_CONFIG_DIR", &seed.config_root);
    let mut child = pty.slave.spawn_command(command).unwrap();
    drop(pty.slave);

    wait_for_output(
        &output,
        b"Attached to the durable local Host",
        Duration::from_secs(5),
    );
    wait_for_output(&output, SECRET_CANARY.as_bytes(), Duration::from_secs(5));
    pty.master
        .resize(PtySize {
            rows: 33,
            cols: 111,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    thread::sleep(Duration::from_millis(100));
    writer.write_all(b"size\r").unwrap();
    writer.flush().unwrap();
    wait_for_output(&output, b"SIZE:33 111", Duration::from_secs(5));
    writer.write_all(b"hello-from-cli\r").unwrap();
    writer.flush().unwrap();
    wait_for_output(&output, b"ECHO:hello-from-cli", Duration::from_secs(5));
    writer.write_all(b"\x1d").unwrap();
    writer.flush().unwrap();
    thread::sleep(Duration::from_millis(50));
    writer.write_all(b"d").unwrap();
    writer.flush().unwrap();

    let status = wait_for_child(child.as_mut());
    assert!(status.success(), "attach exited with {status:?}");
    drop(writer);
    reader_thread.join().unwrap();
    assert_eq!(pty.master.get_termios().unwrap(), before);
    assert_eq!(
        host.stats().await.lifecycle,
        termirust_domain::HostLifecycle::Ready
    );
    host.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn packaged_human_attach_is_read_only_by_default_and_ctrl_c_restores_terminal() {
    let _fixture_guard = REAL_HOST_TEST_LOCK.lock().await;
    let seed = seed_store();
    insert_session(&seed, HostedSessionState::Live, false);
    let host = start(descriptor(&seed)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let pty = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let before = pty.master.get_termios().unwrap();
    let reader = pty.master.try_clone_reader().unwrap();
    let mut writer = pty.master.take_writer().unwrap();
    let output = Arc::new(Mutex::new(Vec::new()));
    let reader_thread = spawn_pty_reader(reader, Arc::clone(&output));
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_termirust-cli"));
    command.args(["session", "attach", &SESSION_ID.to_string()]);
    command.env("TERM", "xterm-256color");
    command.env("TERMIRUST_CONFIG_DIR", &seed.config_root);
    let mut child = pty.slave.spawn_command(command).unwrap();
    drop(pty.slave);

    wait_for_output(&output, b"read-only observer", Duration::from_secs(5));
    thread::sleep(Duration::from_millis(50));
    writer.write_all(b"must-not-reach-host\r").unwrap();
    writer.flush().unwrap();
    thread::sleep(Duration::from_millis(200));
    assert!(
        !output
            .lock()
            .unwrap()
            .windows(b"ECHO:must-not-reach-host".len())
            .any(|window| window == b"ECHO:must-not-reach-host")
    );
    writer.write_all(b"\x03").unwrap();
    writer.flush().unwrap();

    let status = wait_for_child(child.as_mut());
    assert!(status.success(), "read-only attach exited with {status:?}");
    drop(writer);
    reader_thread.join().unwrap();
    assert_eq!(pty.master.get_termios().unwrap(), before);
    assert_eq!(
        host.stats().await.lifecycle,
        termirust_domain::HostLifecycle::Ready
    );
    host.shutdown().await.unwrap();
}

fn attach_command(request_control: bool) -> CliCommand {
    CliCommand::SessionAttach {
        session_id: SESSION_ID,
        from_sequence: OutputSequence::new(FROM_SEQUENCE),
        columns: COLUMNS,
        rows: ROWS,
        request_control,
    }
}

fn packaged_attach(seed: &SeededStore, request_control: bool) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_termirust-cli"));
    command.args([
        "session",
        "attach",
        &SESSION_ID.to_string(),
        "--columns",
        &COLUMNS.to_string(),
        "--rows",
        &ROWS.to_string(),
        "--json",
    ]);
    if request_control {
        command.arg("--write");
    }
    command
        .env("TERMIRUST_CONFIG_DIR", &seed.config_root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap()
}

fn descriptor(seed: &SeededStore) -> LaunchDescriptor {
    LaunchDescriptor {
        format_version: LaunchDescriptor::FORMAT_VERSION,
        session_id: SESSION_ID,
        host_instance_id: HostInstanceId::new(),
        expected_occupant_generation: None,
        runtime_root: LocalEndpoint::for_config_root(&seed.config_root, SESSION_ID)
            .runtime_root()
            .to_path_buf(),
        session_dir: seed
            .config_root
            .join("durable-sessions")
            .join(SESSION_ID.to_string()),
        executable: "/bin/sh".into(),
        runtime_detection: None,
        arguments: vec![
            "-c".into(),
            format!(
                "printf '{SECRET_CANARY}\\n'; while IFS= read -r line; do if [ \"$line\" = size ]; then printf 'SIZE:'; stty size; else printf 'ECHO:%s\\n' \"$line\"; fi; done"
            ),
        ],
        environment: BTreeMap::from([("PATH".into(), "/usr/bin:/bin".into())]),
        cwd: Some(seed.project_root.clone()),
        columns: 80,
        rows: 24,
        journal_limits: JournalLimits::default(),
        stop_deadlines: StopDeadlines::default(),
    }
}

fn spawn_pty_reader(
    mut reader: Box<dyn std::io::Read + Send>,
    output: Arc<Mutex<Vec<u8>>>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut chunk = [0_u8; 4_096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(count) => output.lock().unwrap().extend_from_slice(&chunk[..count]),
            }
        }
    })
}

fn wait_for_output(output: &Mutex<Vec<u8>>, expected: &[u8], timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        let found = output
            .lock()
            .unwrap()
            .windows(expected.len())
            .any(|window| window == expected);
        if found {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {:?}; output: {}",
            String::from_utf8_lossy(expected),
            String::from_utf8_lossy(&output.lock().unwrap())
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_child(child: &mut dyn portable_pty::Child) -> ExitStatus {
    for _ in 0..100 {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        thread::sleep(Duration::from_millis(50));
    }
    child.kill().unwrap();
    let _ = child.wait();
    panic!("interactive attach did not exit within five seconds");
}
