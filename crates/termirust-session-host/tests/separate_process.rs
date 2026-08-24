#![cfg(unix)]

use std::collections::BTreeMap;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use termirust_client::{ConnectOptions, HostClient, LocalEndpoint};
use termirust_domain::{CommandId, HostInstanceId, HostedSessionId, OutputSequence};
use termirust_host_protocol::wire;
use termirust_session_host::{LaunchDescriptor, StopDeadlines};
use termirust_store::{
    HostLease, JournalLimits, JournalStore, ReconciliationResult, reconcile_host,
};
use tokio_util::sync::CancellationToken;

fn descriptor(fixture: &tempfile::TempDir, session_id: HostedSessionId) -> LaunchDescriptor {
    LaunchDescriptor {
        format_version: LaunchDescriptor::FORMAT_VERSION,
        session_id,
        host_instance_id: HostInstanceId::new(),
        runtime_root: fixture.path().join("runtime"),
        session_dir: fixture.path().join("session"),
        executable: "/bin/sh".into(),
        arguments: vec![
            "-c".to_string(),
            "trap '' INT TERM; printf 'SEPARATE-READY\\n'; while IFS= read -r line; do printf 'SEPARATE:%s\\n' \"$line\"; done".to_string(),
        ],
        environment: BTreeMap::from([("PATH".to_string(), "/usr/bin:/bin".to_string())]),
        cwd: Some(fixture.path().to_path_buf()),
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

async fn wait_for_output(client: &mut HostClient, cancel: &CancellationToken) {
    for _ in 0..100 {
        if client
            .get_state(cancel)
            .await
            .is_ok_and(|state| state.latest_sequence > 0)
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("separate Host produced no journaled output");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inherited_pipe_host_survives_client_exit_and_emits_content_free_json() {
    let fixture = tempfile::tempdir().unwrap();
    let session_id = HostedSessionId::new();
    let descriptor = descriptor(&fixture, session_id);
    let mut process = Command::new(env!("CARGO_BIN_EXE_termirust-session-host"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    serde_json::to_writer(process.stdin.as_mut().unwrap(), &descriptor).unwrap();
    process.stdin.take().unwrap().flush().unwrap();

    let stdout = process.stdout.take().unwrap();
    let mut stdout = BufReader::new(stdout);
    let mut ready = String::new();
    stdout.read_line(&mut ready).unwrap();
    let ready_json: serde_json::Value = serde_json::from_str(&ready).unwrap();
    assert_eq!(ready_json["lifecycle"], "ready");
    assert_eq!(ready_json["code"], "host_ready");
    assert!(!ready.contains("SEPARATE"));

    let endpoint = LocalEndpoint::new(&descriptor.runtime_root, session_id);
    let cancel = CancellationToken::new();
    let mut first = HostClient::connect(
        endpoint.clone(),
        ConnectOptions::local(session_id, [1; 32]),
        &cancel,
    )
    .await
    .unwrap();
    wait_for_output(&mut first, &cancel).await;
    first.disconnect();
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(process.try_wait().unwrap().is_none());

    let mut second = HostClient::connect(
        endpoint,
        ConnectOptions::local(session_id, [2; 32]),
        &cancel,
    )
    .await
    .unwrap();
    let replay = second
        .attach(OutputSequence::ZERO, 80, 24, &cancel)
        .await
        .unwrap();
    assert!(!replay.is_empty());
    second
        .stop(CommandId::new(), wire::StopMode::Graceful, &cancel)
        .await
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(3);
    while process.try_wait().unwrap().is_none() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(process.try_wait().unwrap().is_some());
    let mut stderr = String::new();
    process
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(!stderr.contains("SEPARATE"));

    let lease = HostLease::acquire(&descriptor.session_dir, HostInstanceId::new()).unwrap();
    let journal = JournalStore::open(&lease, JournalLimits::default()).unwrap();
    assert!(
        !journal
            .read_from(OutputSequence::ZERO)
            .unwrap()
            .frames
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abrupt_host_death_preserves_journal_and_reconciles_without_signaling_child() {
    let fixture = tempfile::tempdir().unwrap();
    let session_id = HostedSessionId::new();
    let mut descriptor = descriptor(&fixture, session_id);
    descriptor.arguments = vec![
        "-c".to_string(),
        "printf 'CRASH-RETAINED\\n'; sleep 1".to_string(),
    ];
    let mut process = Command::new(env!("CARGO_BIN_EXE_termirust-session-host"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    serde_json::to_writer(process.stdin.as_mut().unwrap(), &descriptor).unwrap();
    process.stdin.take();
    let mut ready = String::new();
    BufReader::new(process.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert!(ready.contains("host_ready"));

    let endpoint = LocalEndpoint::new(&descriptor.runtime_root, session_id);
    let cancel = CancellationToken::new();
    let mut client = HostClient::connect(
        endpoint,
        ConnectOptions::local(session_id, [7; 32]),
        &cancel,
    )
    .await
    .unwrap();
    wait_for_output(&mut client, &cancel).await;
    client.disconnect();
    process.kill().unwrap();
    process.wait().unwrap();

    assert_eq!(
        reconcile_host(&descriptor.session_dir).unwrap(),
        ReconciliationResult::Orphaned
    );
    let lease = HostLease::acquire(&descriptor.session_dir, descriptor.host_instance_id).unwrap();
    let journal = JournalStore::open(&lease, JournalLimits::default()).unwrap();
    let bytes = journal
        .read_from(OutputSequence::ZERO)
        .unwrap()
        .frames
        .into_iter()
        .flat_map(|frame| frame.payload)
        .collect::<Vec<_>>();
    assert!(String::from_utf8_lossy(&bytes).contains("CRASH-RETAINED"));
    tokio::time::sleep(Duration::from_millis(1_100)).await;
}
