#![cfg(unix)]

use std::collections::BTreeMap;
use std::process::{Child, Command};
use std::time::Duration;

use termirust_client::{ClientErrorCode, ConnectOptions, HostClient, LocalEndpoint};
use termirust_domain::{CommandId, HostInstanceId, HostedSessionId, OutputSequence};
use termirust_host_protocol::wire;
use termirust_session_host::{LaunchDescriptor, StopDeadlines, start};
use termirust_store::{HostLease, JournalLimits, JournalStore, read_host_metadata};
use tokio_util::sync::CancellationToken;

fn descriptor(fixture: &tempfile::TempDir, session_id: HostedSessionId) -> LaunchDescriptor {
    LaunchDescriptor {
        format_version: LaunchDescriptor::FORMAT_VERSION,
        session_id,
        host_instance_id: HostInstanceId::new(),
        expected_occupant_generation: None,
        runtime_root: fixture.path().join("runtime"),
        session_dir: fixture.path().join("session"),
        executable: "/bin/sh".into(),
        runtime_detection: None,
        arguments: vec![
            "-c".to_string(),
            "trap '' INT TERM; printf 'HOST-READY\\n'; while IFS= read -r line; do printf 'HOST-OUT:%s\\n' \"$line\"; done".to_string(),
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

async fn wait_for_sequence(client: &mut HostClient, minimum: u64, cancel: &CancellationToken) {
    for _ in 0..100 {
        if client
            .get_state(cancel)
            .await
            .is_ok_and(|state| state.latest_sequence >= minimum)
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("real Host output did not reach sequence {minimum}");
}

fn stop_sentinel(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_host_survives_detach_replays_output_and_stops_only_owned_group() {
    use std::os::unix::process::CommandExt as _;

    let fixture = tempfile::tempdir().unwrap();
    let session_id = HostedSessionId::new();
    let descriptor = descriptor(&fixture, session_id);
    let host_instance_id = descriptor.host_instance_id;
    let host = start(descriptor.clone()).await.unwrap();
    let endpoint = LocalEndpoint::new(host.runtime_root(), session_id);
    let cancel = CancellationToken::new();
    let mut client = HostClient::connect(
        endpoint.clone(),
        ConnectOptions::local(session_id, [1; 32]),
        &cancel,
    )
    .await
    .unwrap();
    assert_eq!(client.host_instance_id(), Some(host_instance_id));
    wait_for_sequence(&mut client, 1, &cancel).await;
    let mut reader = HostClient::connect(
        endpoint.clone(),
        ConnectOptions::local_read_only(session_id, [2; 32]),
        &cancel,
    )
    .await
    .unwrap();
    assert!(!reader.get_state(&cancel).await.unwrap().has_writer_lease);
    assert!(
        reader
            .attach(OutputSequence::ZERO, 80, 24, &cancel)
            .await
            .is_ok()
    );
    assert!(
        !reader
            .set_writer_lease(CommandId::new(), true, &cancel)
            .await
            .unwrap()
    );
    assert!(
        !client
            .set_writer_lease(CommandId::new(), false, &cancel)
            .await
            .unwrap()
    );
    assert!(
        reader
            .set_writer_lease(CommandId::new(), true, &cancel)
            .await
            .unwrap()
    );
    assert!(
        reader
            .input(CommandId::new(), b"reader-owned\n".to_vec(), &cancel)
            .await
            .unwrap()
    );
    assert!(
        !reader
            .set_writer_lease(CommandId::new(), false, &cancel)
            .await
            .unwrap()
    );
    assert!(
        client
            .set_writer_lease(CommandId::new(), true, &cancel)
            .await
            .unwrap()
    );
    assert!(client.get_state(&cancel).await.unwrap().has_writer_lease);
    assert_eq!(
        reader
            .input(CommandId::new(), b"denied\n".to_vec(), &cancel)
            .await
            .unwrap_err()
            .code,
        ClientErrorCode::PermissionDenied
    );
    assert!(
        client
            .input(CommandId::new(), b"first-input\n".to_vec(), &cancel)
            .await
            .unwrap()
    );
    wait_for_sequence(&mut client, 3, &cancel).await;
    let before_detach = client
        .attach(OutputSequence::ZERO, 80, 24, &cancel)
        .await
        .unwrap();
    let before_bytes = before_detach
        .iter()
        .flat_map(|output| output.bytes.iter().copied())
        .collect::<Vec<_>>();
    assert!(String::from_utf8_lossy(&before_bytes).contains("HOST-OUT:first-input"));
    client.disconnect();
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(!host.stats().await.recording_paused);

    let mut reattached = HostClient::connect(
        endpoint,
        ConnectOptions::local(session_id, [3; 32]),
        &cancel,
    )
    .await
    .unwrap();
    let replay = reattached
        .attach(OutputSequence::ZERO, 100, 30, &cancel)
        .await
        .unwrap();
    let replay_bytes = replay
        .iter()
        .flat_map(|output| output.bytes.iter().copied())
        .collect::<Vec<_>>();
    assert!(String::from_utf8_lossy(&replay_bytes).contains("HOST-OUT:first-input"));
    assert!(
        reattached
            .resize(CommandId::new(), 100, 30, &cancel)
            .await
            .unwrap()
    );

    let mut sentinel = Command::new("/bin/sleep")
        .arg("30")
        .process_group(0)
        .spawn()
        .unwrap();
    assert!(
        reattached
            .stop(CommandId::new(), wire::StopMode::Graceful, &cancel)
            .await
            .unwrap()
    );
    assert!(sentinel.try_wait().unwrap().is_none());
    stop_sentinel(&mut sentinel);
    host.wait().await.unwrap();
    assert_eq!(
        read_host_metadata(&descriptor.session_dir)
            .unwrap()
            .lifecycle,
        termirust_domain::HostLifecycle::Exited
    );

    let lease = HostLease::acquire(&descriptor.session_dir, HostInstanceId::new()).unwrap();
    let journal = JournalStore::open(&lease, JournalLimits::default()).unwrap();
    let retained = journal.read_from(OutputSequence::ZERO).unwrap();
    assert!(!retained.frames.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "repository 1,000 detach and reattach stress"]
async fn one_thousand_detach_reattach_cycles_keep_one_host() {
    let fixture = tempfile::tempdir().unwrap();
    let session_id = HostedSessionId::new();
    let mut descriptor = descriptor(&fixture, session_id);
    descriptor.arguments = vec!["-c".to_string(), "sleep 30".to_string()];
    let host = start(descriptor).await.unwrap();
    let endpoint = LocalEndpoint::new(host.runtime_root(), session_id);
    let cancel = CancellationToken::new();
    for index in 1_u64..=1_000 {
        let mut nonce = [0_u8; 32];
        nonce[..8].copy_from_slice(&index.to_be_bytes());
        let mut client = HostClient::connect(
            endpoint.clone(),
            ConnectOptions::local(session_id, nonce),
            &cancel,
        )
        .await
        .unwrap();
        assert_eq!(
            client
                .get_state(&cancel)
                .await
                .unwrap()
                .host_instance_id
                .len(),
            16
        );
        client.disconnect();
    }
    host.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_host_compacts_to_snapshot_and_quota_pause_keeps_pty_alive() {
    let fixture = tempfile::tempdir().unwrap();
    let session_id = HostedSessionId::new();
    let mut compacting = descriptor(&fixture, session_id);
    let compact_output_done = fixture.path().join("compact-output-done");
    compacting.arguments = vec![
        "-c".to_string(),
        "head -c 4194304 /dev/zero | tr '\\000' x; : > compact-output-done; sleep 30".to_string(),
    ];
    compacting.journal_limits = JournalLimits {
        segment_bytes: 1024 * 1024,
        hard_bytes: 8 * 1024 * 1024,
        retained_segments: 4,
    };
    let snapshot_path = compacting.session_dir.join("snapshot.trs");
    let host = start(compacting).await.unwrap();
    let endpoint = LocalEndpoint::new(host.runtime_root(), session_id);
    let cancel = CancellationToken::new();
    let mut client = HostClient::connect(
        endpoint,
        ConnectOptions::local(session_id, [8; 32]),
        &cancel,
    )
    .await
    .unwrap();
    for _ in 0..300 {
        if snapshot_path.exists() && compact_output_done.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(snapshot_path.exists());
    assert!(compact_output_done.exists());
    let replay = client
        .attach(OutputSequence::ZERO, 80, 24, &cancel)
        .await
        .unwrap();
    let snapshot = client.take_last_snapshot().unwrap();
    assert!(snapshot.boundary_sequence > 0);
    assert!(!snapshot.terminal_bytes.is_empty());
    assert!(
        replay
            .iter()
            .all(|output| output.sequence.get() > snapshot.boundary_sequence)
    );
    assert!(
        replay
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence)
    );
    client.disconnect();
    host.shutdown().await.unwrap();

    let quota_fixture = tempfile::tempdir().unwrap();
    let quota_session = HostedSessionId::new();
    let mut quota = descriptor(&quota_fixture, quota_session);
    let quota_output_done = quota_fixture.path().join("quota-output-done");
    quota.arguments = vec![
        "-c".to_string(),
        "head -c 2097152 /dev/zero | tr '\\000' q; : > quota-output-done; sleep 30".to_string(),
    ];
    quota.journal_limits = JournalLimits {
        segment_bytes: 1024 * 1024,
        hard_bytes: 1024 * 1024,
        retained_segments: 1,
    };
    let quota_host = start(quota).await.unwrap();
    for _ in 0..200 {
        let stats = quota_host.stats().await;
        if stats.recording_paused {
            assert_eq!(stats.lifecycle, termirust_domain::HostLifecycle::Ready);
            assert!(stats.latest_sequence.get() > 16);
            if quota_output_done.exists() {
                quota_host.shutdown().await.unwrap();
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    quota_host.shutdown().await.unwrap();
    panic!("real Host did not enter bounded recording pause");
}
