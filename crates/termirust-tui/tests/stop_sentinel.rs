#![cfg(unix)]

use std::collections::BTreeMap;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use termirust_client::LocalEndpoint;
use termirust_domain::{
    ActivityAggregate, AddProject, CommandId, HostInstanceId, HostedSession, HostedSessionId,
    HostedSessionState, OutputSequence, PositionKey, ProjectId, Revision, SessionTitle,
    TitleSource,
};
use termirust_session_host::{LaunchDescriptor, StopDeadlines, start};
use termirust_store::{JournalLimits, ProjectRepository, SessionRepository};
use termirust_tui::{LocalManagementExecutor, ManagementCommand, ManagementExecutor};

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn management_stop_targets_only_the_owned_host_and_preserves_sentinel() {
    let fixture = tempfile::tempdir().unwrap();
    let config_root = fixture.path().join("config");
    let metadata_root = config_root.join("agent-workspace");
    let session_data_root = config_root.join("durable-sessions");
    let project_root = fixture.path().join("project");
    std::fs::create_dir_all(&project_root).unwrap();
    let project_id = ProjectId::new();
    let session_id = HostedSessionId::new();
    ProjectRepository::open(&metadata_root)
        .unwrap()
        .add_project(AddProject {
            id: project_id,
            root: project_root.clone(),
            display_name: Some("Stop fixture".into()),
            expected: Revision::ZERO,
        })
        .unwrap();
    let sessions = SessionRepository::open(&metadata_root, &session_data_root).unwrap();
    let session = sessions
        .create_session(
            HostedSession {
                id: session_id,
                project_id,
                group_id: None,
                preset_id: None,
                title: SessionTitle::new("Owned Host").unwrap(),
                title_source: TitleSource::Manual,
                lifecycle: HostedSessionState::Live,
                activity: ActivityAggregate::default(),
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
            sessions.load().unwrap().revision,
        )
        .unwrap();

    let endpoint = LocalEndpoint::for_config_root(&config_root, session_id);
    let host = start(LaunchDescriptor {
        format_version: LaunchDescriptor::FORMAT_VERSION,
        session_id,
        host_instance_id: HostInstanceId::new(),
        expected_occupant_generation: None,
        runtime_root: endpoint.runtime_root().to_path_buf(),
        session_dir: session_data_root.join(session_id.to_string()),
        executable: "/bin/sh".into(),
        runtime_detection: None,
        arguments: vec![
            "-c".into(),
            "printf 'MANAGED-HOST-READY\\n'; while IFS= read -r line; do printf '%s\\n' \"$line\"; done".into(),
        ],
        environment: BTreeMap::from([("PATH".into(), "/usr/bin:/bin".into())]),
        cwd: Some(project_root),
        columns: 80,
        rows: 24,
        journal_limits: JournalLimits::default(),
        stop_deadlines: StopDeadlines {
            interrupt_millis: 50,
            terminate_millis: 100,
            total_millis: 500,
        },
    })
    .await
    .unwrap();

    let mut sentinel = ChildGuard(
        Command::new("/bin/sh")
            .args(["-c", "exec sleep 60"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    assert!(sentinel.0.try_wait().unwrap().is_none());

    let executor = LocalManagementExecutor::with_host_executable(
        config_root,
        fixture.path().join("unused-host"),
    );
    let stopped = tokio::task::spawn_blocking(move || {
        executor.execute(
            ManagementCommand::StopAndArchive {
                command_id: CommandId::new(),
                session_id: session_id.to_string(),
                expected_revision: session.revision.get(),
            },
            &termirust_cli::Cancellation::default(),
        )
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(stopped.outcome, "stopped_and_archived");
    tokio::time::timeout(Duration::from_secs(3), host.wait())
        .await
        .expect("owned Host did not exit before the management deadline")
        .unwrap();

    assert!(
        sentinel.0.try_wait().unwrap().is_none(),
        "stopping the selected Session must not terminate an unrelated process"
    );
    let stored = sessions.load().unwrap();
    let stored = stored
        .sessions
        .iter()
        .find(|candidate| candidate.id == session_id)
        .unwrap();
    assert_eq!(stored.lifecycle, HostedSessionState::Exited);
    assert!(stored.archived_at.is_some());
}
