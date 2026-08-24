#![cfg(unix)]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use termirust_client::LocalEndpoint;
use termirust_domain::{HostInstanceId, HostedSessionId};
use termirust_session_host::{
    HostErrorCode, LaunchDescriptor, MAX_LIVE_HOSTS, StopDeadlines, start, start_with_cancel,
};
use termirust_store::JournalLimits;
use tokio_util::sync::CancellationToken;

fn descriptor(root: &Path, session_id: HostedSessionId, executable: &str) -> LaunchDescriptor {
    static NEXT_RUNTIME: AtomicU64 = AtomicU64::new(1);
    let runtime = NEXT_RUNTIME.fetch_add(1, Ordering::Relaxed);
    LaunchDescriptor {
        format_version: LaunchDescriptor::FORMAT_VERSION,
        session_id,
        host_instance_id: HostInstanceId::new(),
        runtime_root: root.join(format!("r{runtime}")),
        session_dir: root.join(format!("session-{session_id}")),
        executable: executable.into(),
        arguments: Vec::new(),
        environment: BTreeMap::from([("PATH".to_string(), "/usr/bin:/bin".to_string())]),
        cwd: Some(root.to_path_buf()),
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

#[tokio::test]
async fn pre_ready_cancellation_creates_no_host_state() {
    let fixture = tempfile::tempdir().unwrap();
    let descriptor = descriptor(fixture.path(), HostedSessionId::new(), "/bin/sleep");
    let cancel = CancellationToken::new();
    cancel.cancel();
    let error = start_with_cancel(descriptor.clone(), &cancel)
        .await
        .unwrap_err();
    assert_eq!(error.code, HostErrorCode::Cancelled);
    assert!(!descriptor.runtime_root.exists());
    assert!(!descriptor.session_dir.exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_joins_tasks_and_kills_owned_grandchild() {
    let fixture = tempfile::tempdir().unwrap();
    let pid_file = fixture.path().join("grandchild.pid");
    let mut descriptor = descriptor(fixture.path(), HostedSessionId::new(), "/bin/sh");
    descriptor.arguments = vec![
        "-c".to_string(),
        format!("/bin/sleep 30 & echo $! > '{}'; wait", pid_file.display()),
    ];
    let host = start(descriptor).await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let grandchild = loop {
        if let Ok(contents) = std::fs::read_to_string(&pid_file)
            && let Ok(pid) = contents.trim().parse::<i32>()
        {
            break pid;
        }
        assert!(
            Instant::now() < deadline,
            "grandchild PID was not published"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    let started = Instant::now();
    host.shutdown().await.unwrap();
    assert!(started.elapsed() < Duration::from_secs(2));
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let result = unsafe { libc::kill(grandchild, 0) };
        if result != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            break;
        }
        assert!(Instant::now() < deadline, "owned grandchild remained alive");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slow_handshake_peers_are_bounded_to_connection_limit() {
    let fixture = tempfile::tempdir().unwrap();
    let session_id = HostedSessionId::new();
    let mut descriptor = descriptor(fixture.path(), session_id, "/bin/sleep");
    descriptor.arguments = vec!["30".to_string()];
    let host = start(descriptor).await.unwrap();
    let endpoint = LocalEndpoint::new(host.runtime_root(), session_id);
    let mut peers = Vec::new();
    for _ in 0..40 {
        if let Ok(peer) = tokio::net::UnixStream::connect(endpoint.socket_path()).await {
            peers.push(peer);
        }
    }
    for _ in 0..100 {
        if host.stats().await.active_connections == 32 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(host.stats().await.active_connections, 32);
    drop(peers);
    for _ in 0..100 {
        if host.stats().await.active_connections == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(host.stats().await.active_connections, 0);
    host.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "repository 32 idle Host stress"]
async fn thirty_two_host_limit_is_exact_and_the_next_start_fails_closed() {
    let fixture = tempfile::tempdir().unwrap();
    let runtime_root = fixture.path().join("shared-runtime");
    let mut hosts = Vec::new();
    for _ in 0..MAX_LIVE_HOSTS {
        let mut descriptor = descriptor(fixture.path(), HostedSessionId::new(), "/bin/sleep");
        descriptor.runtime_root = runtime_root.clone();
        descriptor.arguments = vec!["30".to_string()];
        hosts.push(start(descriptor).await.unwrap());
    }
    let mut denied = descriptor(fixture.path(), HostedSessionId::new(), "/bin/sleep");
    denied.runtime_root = runtime_root;
    denied.arguments = vec!["30".to_string()];
    assert_eq!(
        start(denied).await.unwrap_err().code,
        HostErrorCode::ResourceLimit
    );
    for host in hosts {
        host.shutdown().await.unwrap();
    }
}
