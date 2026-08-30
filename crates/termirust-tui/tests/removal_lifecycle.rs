use std::fs;
use std::time::Instant;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use termirust_cli::Cancellation;
use termirust_domain::{
    ActivityAggregate, AddProject, HostedSession, HostedSessionId, HostedSessionState,
    OutputSequence, PositionKey, ProjectId, Revision, SessionTitle, TitleSource,
};
use termirust_store::{ProjectRepository, SessionRepository};
use termirust_tui::{
    CommandProgress, FleetSession, LocalManagementExecutor, ManagementCommand, ManagementEffect,
    ManagementExecutor, ManagementIntent, ManagementModel,
};

#[test]
fn removal_lifecycle_previews_confirms_and_quarantines_exact_session() {
    let fixture = tempfile::tempdir().unwrap();
    let config_root = fixture.path().join("config");
    let metadata_root = config_root.join("agent-workspace");
    let session_data_root = config_root.join("durable-sessions");
    let project_root = fixture.path().join("project");
    fs::create_dir_all(&project_root).unwrap();
    let project_id = ProjectId::new();
    ProjectRepository::open(&metadata_root)
        .unwrap()
        .add_project(AddProject {
            id: project_id,
            root: project_root,
            display_name: Some("Removal fixture".into()),
            expected: Revision::ZERO,
        })
        .unwrap();
    let sessions = SessionRepository::open(&metadata_root, &session_data_root).unwrap();
    let session_id = HostedSessionId::new();
    let session = sessions
        .create_session(
            HostedSession {
                id: session_id,
                project_id,
                group_id: None,
                preset_id: None,
                title: SessionTitle::new("Reviewed removal").unwrap(),
                title_source: TitleSource::Manual,
                lifecycle: HostedSessionState::Exited,
                activity: ActivityAggregate::default(),
                pinned: false,
                position: PositionKey::FIRST,
                last_output_sequence: OutputSequence::ZERO,
                read_through_sequence: OutputSequence::ZERO,
                unread_sequence: None,
                archived_at: Some(1),
                created_at: 1,
                updated_at: 1,
                revision: Revision::ZERO,
            },
            sessions.load().unwrap().revision,
        )
        .unwrap();
    let owned_root = session_data_root.join(session_id.to_string());
    fs::create_dir_all(owned_root.join("artifacts")).unwrap();
    fs::write(owned_root.join("artifacts").join("result.txt"), b"fixture").unwrap();
    let sentinel = fixture.path().join("unrelated-sentinel");
    fs::write(&sentinel, b"must-survive").unwrap();

    let fleet = FleetSession {
        id: session_id.to_string(),
        project_id: project_id.to_string(),
        group_id: None,
        title: session.title.as_str().into(),
        state: "exited".into(),
        activity: "idle".into(),
        unread: false,
        pinned: false,
        archived: true,
        revision: session.revision.get(),
    };
    let executor = LocalManagementExecutor::with_host_executable(
        config_root.clone(),
        fixture.path().join("unused-host"),
    );
    let mut model = ManagementModel::default();
    assert!(matches!(
        model.begin_session(ManagementIntent::Remove, &fleet),
        ManagementEffect::LoadRemovalPreview { .. }
    ));
    let generation = model.generation();
    let preview = executor
        .removal_preview(&fleet.id, fleet.revision, &model.cancellation())
        .unwrap();
    assert_eq!(preview.manifest.artifact_bytes, 7);
    model.removal_preview_loaded(generation, Ok(preview));
    model.append_paste(&fleet.title);
    let ManagementEffect::Execute(command @ ManagementCommand::Remove { .. }) = model.handle_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        Instant::now(),
    ) else {
        panic!("reviewed removal command expected");
    };
    let result = executor.execute(command, &Cancellation::default()).unwrap();
    model.completed(generation, Ok(result), Instant::now());

    assert!(matches!(
        model.progress(),
        CommandProgress::Succeeded {
            undo_deadline: None,
            ..
        }
    ));
    assert!(sessions.load().unwrap().sessions.is_empty());
    assert!(!owned_root.exists());
    assert!(
        config_root
            .join("durable-session-quarantine")
            .join(session_id.to_string())
            .is_dir()
    );
    assert_eq!(fs::read(sentinel).unwrap(), b"must-survive");
}
