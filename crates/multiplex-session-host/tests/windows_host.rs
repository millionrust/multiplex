#![cfg(windows)]
//! A durable session on Windows, end to end: the Host binary started the way the app starts it,
//! a program running in its pseudo-console, a client reaching it through the named pipe and
//! reading what the program printed, and a stop that ends the program and the Host.

use std::collections::BTreeMap;
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use multiplex_client::{ConnectOptions, HostClient, LocalEndpoint};
use multiplex_domain::{CommandId, HostInstanceId, HostedSessionId, OutputSequence};
use multiplex_host_protocol::wire;
use multiplex_session_host::{LaunchDescriptor, StopDeadlines};
use multiplex_store::JournalLimits;
use tokio_util::sync::CancellationToken;

const MARKER: &str = "MULTIPLEX_WINDOWS_HOST_MARKER";

fn system_root() -> PathBuf {
    std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_windows_session_is_reached_through_its_pipe_and_stopped() {
    let fixture = tempfile::tempdir().unwrap();
    let session_id = HostedSessionId::new();
    let cmd = system_root().join(r"System32\cmd.exe");
    let descriptor = LaunchDescriptor {
        format_version: LaunchDescriptor::FORMAT_VERSION,
        session_id,
        host_instance_id: HostInstanceId::new(),
        expected_occupant_generation: None,
        runtime_root: fixture.path().join("runtime"),
        session_dir: fixture.path().join("durable-sessions").join("session"),
        executable: std::fs::canonicalize(&cmd).unwrap(),
        runtime_detection: None,
        // /k keeps Command Prompt running after the echo, as an interactive shell would.
        arguments: vec![
            "/d".to_string(),
            "/q".to_string(),
            "/k".to_string(),
            format!("echo {MARKER}"),
        ],
        // The Host starts its program with an empty environment; Command Prompt needs these.
        environment: BTreeMap::from([
            (
                "SystemRoot".to_string(),
                system_root().display().to_string(),
            ),
            (
                "PATH".to_string(),
                system_root().join("System32").display().to_string(),
            ),
            ("COMSPEC".to_string(), cmd.display().to_string()),
        ]),
        cwd: Some(fixture.path().to_path_buf()),
        columns: 100,
        rows: 30,
        journal_limits: JournalLimits::default(),
        stop_deadlines: StopDeadlines::default(),
    };
    let mut process = Command::new(env!("CARGO_BIN_EXE_multiplex-session-host"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    serde_json::to_writer(process.stdin.as_mut().unwrap(), &descriptor).unwrap();
    process.stdin.take().unwrap().flush().unwrap();
    let mut ready = String::new();
    BufReader::new(process.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    if !ready.contains("host_ready") {
        let mut error = String::new();
        let _ = BufReader::new(process.stderr.take().unwrap()).read_line(&mut error);
        panic!("Host readiness was {ready:?}; it said {error:?}");
    }

    let cancel = CancellationToken::new();
    let mut client = HostClient::connect(
        LocalEndpoint::new(&descriptor.runtime_root, session_id),
        ConnectOptions::local(session_id, [7_u8; 32]),
        &cancel,
    )
    .await
    .unwrap();

    // The echo lands in the journal shortly after the shell starts; attach until it is there.
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut printed = String::new();
    while !printed.contains(MARKER) {
        assert!(
            Instant::now() < deadline,
            "the marker never arrived; the session printed {printed:?}"
        );
        let outputs = client
            .attach(OutputSequence::ZERO, 100, 30, &cancel)
            .await
            .unwrap();
        printed = outputs
            .iter()
            .map(|output| String::from_utf8_lossy(&output.bytes).into_owned())
            .collect();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    client
        .stop(CommandId::new(), wire::StopMode::Graceful, &cancel)
        .await
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while process.try_wait().unwrap().is_none() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        process.try_wait().unwrap().is_some(),
        "the Host was still running after the stop"
    );
    // The endpoint is gone with the Host, so nothing can connect to a session that ended.
    assert!(
        HostClient::connect(
            LocalEndpoint::new(&descriptor.runtime_root, session_id),
            ConnectOptions::local(session_id, [8_u8; 32]),
            &cancel,
        )
        .await
        .is_err()
    );
}
