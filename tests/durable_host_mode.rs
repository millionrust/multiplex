#![cfg(unix)]

use std::collections::BTreeMap;
use std::io::{BufRead as _, BufReader, Write as _};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use termirust_client::{ConnectOptions, HostClient, LocalEndpoint};
use termirust_domain::{CommandId, HostInstanceId, HostedSessionId, OutputSequence};
use termirust_host_protocol::wire;
use termirust_session_host::{LaunchDescriptor, StopDeadlines};
use termirust_store::JournalLimits;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn packaged_binary_host_mode_survives_gui_client_disconnect() {
    let fixture = tempfile::tempdir().unwrap();
    let session_id = HostedSessionId::new();
    let descriptor = LaunchDescriptor {
        format_version: LaunchDescriptor::FORMAT_VERSION,
        session_id,
        host_instance_id: HostInstanceId::new(),
        runtime_root: fixture.path().join("runtime"),
        session_dir: fixture.path().join("session"),
        executable: "/bin/sh".into(),
        arguments: vec![
            "-c".to_string(),
            "printf 'PACKAGED-HOST-READY\\n'; while IFS= read -r line; do printf 'PACKAGED:%s\\n' \"$line\"; done".to_string(),
        ],
        environment: BTreeMap::from([
            ("PATH".to_string(), "/usr/bin:/bin".to_string()),
            ("TERM".to_string(), "xterm-256color".to_string()),
        ]),
        cwd: Some(fixture.path().to_path_buf()),
        columns: 80,
        rows: 24,
        journal_limits: JournalLimits::default(),
        stop_deadlines: StopDeadlines::default(),
    };
    let mut process = Command::new(env!("CARGO_BIN_EXE_termirust"))
        .arg("--session-host")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    serde_json::to_writer(process.stdin.as_mut().unwrap(), &descriptor).unwrap();
    process.stdin.take().unwrap().flush().unwrap();
    let mut ready = String::new();
    BufReader::new(process.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert!(ready.contains("host_ready"));

    let endpoint = LocalEndpoint::new(&descriptor.runtime_root, session_id);
    let cancel = CancellationToken::new();
    let mut first = HostClient::connect(
        endpoint.clone(),
        ConnectOptions::local(session_id, [1; 32]),
        &cancel,
    )
    .await
    .unwrap();
    let mut watermark = OutputSequence::ZERO;
    let deadline = Instant::now() + Duration::from_secs(2);
    while watermark == OutputSequence::ZERO && Instant::now() < deadline {
        for output in first.attach(watermark, 80, 24, &cancel).await.unwrap() {
            watermark = output.sequence;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_ne!(watermark, OutputSequence::ZERO);
    first.disconnect();
    assert!(process.try_wait().unwrap().is_none());

    let mut second = HostClient::connect(
        endpoint,
        ConnectOptions::local(session_id, [2; 32]),
        &cancel,
    )
    .await
    .unwrap();
    second
        .attach(OutputSequence::ZERO, 80, 24, &cancel)
        .await
        .unwrap();
    second
        .stop(CommandId::new(), wire::StopMode::Graceful, &cancel)
        .await
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(6);
    while process.try_wait().unwrap().is_none() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(process.try_wait().unwrap().is_some());
}
