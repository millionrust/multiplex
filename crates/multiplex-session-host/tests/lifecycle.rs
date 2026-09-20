#![cfg(unix)]

use std::collections::BTreeMap;
use std::io::{BufRead as _, BufReader, Write as _};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use multiplex_client::{ConnectOptions, HostClient, LocalEndpoint};
use multiplex_domain::{
    ActivityState, CommandId, HostInstanceId, HostedSession, HostedSessionId, HostedSessionState,
    OutputSequence, PositionKey, PresetId, ProjectId, Revision, SessionMutation, SessionStateError,
    SessionTitle, TitleSource,
};
use multiplex_host_protocol::wire;
use multiplex_session_host::{LaunchDescriptor, StopDeadlines};
use multiplex_store::{JournalLimits, SessionRepository, StoreError, read_host_metadata};
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn activity_lifecycle_stop_archive_restore_requires_confirmed_host_exit() {
    let fixture = tempfile::tempdir().unwrap();
    let session_id = HostedSessionId::new();
    let session_dir = fixture
        .path()
        .join("durable-sessions")
        .join(session_id.to_string());
    let repository = SessionRepository::open(
        fixture.path().join("metadata"),
        fixture.path().join("durable-sessions"),
    )
    .unwrap();
    let descriptor = LaunchDescriptor {
        format_version: LaunchDescriptor::FORMAT_VERSION,
        session_id,
        host_instance_id: HostInstanceId::new(),
        expected_occupant_generation: None,
        runtime_root: fixture.path().join("runtime"),
        session_dir: session_dir.clone(),
        executable: std::fs::canonicalize("/bin/sh").unwrap(),
        runtime_detection: None,
        arguments: vec![
            "-c".to_string(),
            "trap 'exit 0' TERM INT; while :; do sleep 1; done".to_string(),
        ],
        environment: BTreeMap::from([
            ("PATH".to_string(), "/usr/bin:/bin".to_string()),
            ("TERM".to_string(), "xterm-256color".to_string()),
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
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    serde_json::to_writer(process.stdin.as_mut().unwrap(), &descriptor).unwrap();
    process.stdin.take().unwrap().flush().unwrap();
    let mut ready = String::new();
    BufReader::new(process.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert!(ready.contains("host_ready"), "Host readiness was {ready:?}");
    let live = repository
        .create_session(
            HostedSession {
                id: session_id,
                project_id: ProjectId::new(),
                group_id: None,
                preset_id: Some(PresetId::new()),
                title: SessionTitle::new("Lifecycle fixture").unwrap(),
                title_source: TitleSource::Manual,
                lifecycle: HostedSessionState::Live,
                activity: multiplex_domain::ActivityAggregate {
                    state: ActivityState::Idle,
                    ..multiplex_domain::ActivityAggregate::default()
                },
                pinned: false,
                position: PositionKey::FIRST,
                last_output_sequence: OutputSequence::ZERO,
                read_through_sequence: OutputSequence::ZERO,
                unread_sequence: None,
                archived_at: None,
                created_at: 1,
                updated_at: 1,
                revision: Revision::ZERO,
            },
            Revision::ZERO,
        )
        .unwrap();
    assert!(matches!(
        repository.mutate_session(
            session_id,
            live.revision,
            SessionMutation::Archive { at: 2 },
            2,
        ),
        Err(StoreError::SessionDomain(
            SessionStateError::StopRequiredBeforeArchive
        ))
    ));

    let stopping = repository
        .mutate_session(
            session_id,
            live.revision,
            SessionMutation::SetLifecycle(HostedSessionState::Stopping),
            3,
        )
        .unwrap();
    let endpoint = LocalEndpoint::new(&descriptor.runtime_root, session_id);
    let cancel = CancellationToken::new();
    let mut client = HostClient::connect(
        endpoint,
        ConnectOptions::local(session_id, [7_u8; 32]),
        &cancel,
    )
    .await
    .unwrap();
    client
        .stop(CommandId::new(), wire::StopMode::Graceful, &cancel)
        .await
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(6);
    while process.try_wait().unwrap().is_none() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(process.try_wait().unwrap().is_some());
    let host_metadata = read_host_metadata(&session_dir).unwrap();
    assert_eq!(host_metadata.activity.state, ActivityState::Done);
    assert_eq!(
        host_metadata.activity.confidence,
        multiplex_domain::ActivityConfidence::Verified
    );

    let exited = repository
        .mutate_session(
            session_id,
            stopping.revision,
            SessionMutation::SetLifecycle(HostedSessionState::Exited),
            4,
        )
        .unwrap();
    let archived = repository
        .mutate_session(
            session_id,
            exited.revision,
            SessionMutation::Archive { at: 5 },
            5,
        )
        .unwrap();
    assert!(archived.archived_at.is_some());
    assert!(session_dir.is_dir());

    let restored = repository
        .mutate_session(session_id, archived.revision, SessionMutation::Restore, 6)
        .unwrap();
    assert_eq!(restored.archived_at, None);
    assert_eq!(restored.lifecycle, HostedSessionState::Exited);
    assert!(process.try_wait().unwrap().is_some());
    assert!(session_dir.is_dir());
}

/// The runtime root lives under a per-user directory in the temporary directory, which nothing
/// has created on a machine that has never run a durable session. Starting one has to make it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_first_session_on_a_machine_creates_its_runtime_root() {
    let fixture = tempfile::tempdir().unwrap();
    // A Unix socket path is limited to about 100 bytes, so stay out of long temp dirs.
    let runtime = tempfile::Builder::new()
        .prefix("tr-first-")
        .tempdir_in("/tmp")
        .unwrap();
    let session_id = HostedSessionId::new();
    let descriptor = LaunchDescriptor {
        format_version: LaunchDescriptor::FORMAT_VERSION,
        session_id,
        host_instance_id: HostInstanceId::new(),
        expected_occupant_generation: None,
        // Neither this directory nor its parent exists yet.
        runtime_root: runtime.path().join("never-used").join("r"),
        session_dir: fixture.path().join("durable-sessions").join("session"),
        executable: std::fs::canonicalize("/bin/sh").unwrap(),
        runtime_detection: None,
        arguments: vec![
            "-c".to_string(),
            "trap 'exit 0' TERM INT; while :; do sleep 1; done".to_string(),
        ],
        environment: BTreeMap::from([("PATH".to_string(), "/usr/bin:/bin".to_string())]),
        cwd: Some(fixture.path().to_path_buf()),
        columns: 80,
        rows: 24,
        journal_limits: JournalLimits::default(),
        stop_deadlines: StopDeadlines::default(),
    };
    let runtime_root = descriptor.runtime_root.clone();

    let host = multiplex_session_host::start(descriptor)
        .await
        .expect("the Host creates the runtime root it was given");
    assert!(runtime_root.is_dir());
    host.shutdown().await.unwrap();
}
