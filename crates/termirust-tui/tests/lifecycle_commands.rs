use termirust_cli::Cancellation;
use termirust_domain::{
    ActivityAggregate, AddProject, CommandId, HostedSession, HostedSessionId, HostedSessionState,
    OutputSequence, PermissionPolicy, PositionKey, PresetDraft, PresetId, PresetOrigin, ProjectId,
    Revision, SessionTitle, TitleSource, WorkingDirectoryRule,
};
use termirust_store::{PresetRepository, ProjectRepository, SessionRepository};
use termirust_tui::{
    LocalManagementExecutor, ManagementCommand, ManagementExecutor, ManagementFailure,
};

#[test]
fn management_lifecycle_commands_use_typed_revisions_and_preserve_metadata() {
    let fixture = tempfile::tempdir().unwrap();
    let config_root = fixture.path().join("config");
    let metadata_root = config_root.join("agent-workspace");
    let project_root = fixture.path().join("project");
    std::fs::create_dir_all(&project_root).unwrap();
    let project_id = ProjectId::new();
    let preset_id = PresetId::new();
    let session_id = HostedSessionId::new();

    ProjectRepository::open(&metadata_root)
        .unwrap()
        .add_project(AddProject {
            id: project_id,
            root: project_root,
            display_name: Some("Management fixture".into()),
            expected: Revision::ZERO,
        })
        .unwrap();
    PresetRepository::open(&metadata_root)
        .unwrap()
        .save_preset(
            PresetDraft {
                id: preset_id,
                label: "Safe shell".into(),
                executable: "/bin/sh".into(),
                args: vec!["-c".into(), "printf ready".into()],
                working_directory: WorkingDirectoryRule::ProjectRoot,
                runtime: None,
                enabled: true,
                favorite: false,
                permission_policy: PermissionPolicy::AskAsNeeded,
                origin: PresetOrigin::User,
                confirm_risky_favorite: false,
            },
            Revision::ZERO,
        )
        .unwrap();
    let sessions =
        SessionRepository::open(&metadata_root, config_root.join("durable-sessions")).unwrap();
    let created = sessions
        .create_session(
            HostedSession {
                id: session_id,
                project_id,
                group_id: None,
                preset_id: Some(preset_id),
                title: SessionTitle::new("Original").unwrap(),
                title_source: TitleSource::Manual,
                lifecycle: HostedSessionState::Exited,
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

    let executor = LocalManagementExecutor::with_host_executable(
        config_root,
        fixture.path().join("unused-host"),
    );
    let cancellation = Cancellation::default();
    let choices = executor
        .launch_choices(&project_id.to_string(), &cancellation)
        .unwrap();
    assert_eq!(choices.len(), 1);
    assert!(choices[0].enabled && choices[0].safe);

    let renamed = execute(
        &executor,
        ManagementCommand::Rename {
            command_id: CommandId::new(),
            session_id: session_id.to_string(),
            expected_revision: created.revision.get(),
            title: "Renamed".into(),
        },
    );
    assert_eq!(renamed.outcome, "renamed");
    let pinned = execute(
        &executor,
        ManagementCommand::SetPinned {
            command_id: CommandId::new(),
            session_id: session_id.to_string(),
            expected_revision: renamed.revision,
            pinned: true,
        },
    );
    let read = execute(
        &executor,
        ManagementCommand::MarkRead {
            command_id: CommandId::new(),
            session_id: session_id.to_string(),
            expected_revision: pinned.revision,
        },
    );
    let archived = execute(
        &executor,
        ManagementCommand::Archive {
            command_id: CommandId::new(),
            session_id: session_id.to_string(),
            expected_revision: read.revision,
        },
    );
    assert!(archived.archived);
    let restored = execute(
        &executor,
        ManagementCommand::Restore {
            command_id: CommandId::new(),
            session_id: session_id.to_string(),
            expected_revision: archived.revision,
        },
    );
    assert_eq!(restored.title, "Renamed");
    assert!(!restored.archived);

    let stored = sessions.load().unwrap();
    let stored = stored
        .sessions
        .iter()
        .find(|session| session.id == session_id)
        .unwrap();
    assert_eq!(stored.title.as_str(), "Renamed");
    assert!(stored.pinned);
    assert_eq!(stored.lifecycle, HostedSessionState::Exited);
    assert!(stored.archived_at.is_none());

    let stale = executor
        .execute(
            ManagementCommand::Rename {
                command_id: CommandId::new(),
                session_id: session_id.to_string(),
                expected_revision: created.revision.get(),
                title: "Stale".into(),
            },
            &cancellation,
        )
        .unwrap_err();
    assert_eq!(stale.code, "conflict");
    assert_eq!(stale.conflict_revision, Some(restored.revision));
}

fn execute(
    executor: &LocalManagementExecutor,
    command: ManagementCommand,
) -> termirust_tui::ManagementResult {
    executor
        .execute(command, &Cancellation::default())
        .unwrap_or_else(|error: ManagementFailure| panic!("{}: {}", error.code, error.summary))
}
