use termirust_domain::{
    ActivityAggregate, CanonicalPath, HostedSession, HostedSessionId, HostedSessionState,
    IndexSourceRevisions, LocalizedUserText, OutputSequence, PositionKey, Project, ProjectId,
    Revision, SessionTitle, TitleSource, build_palette_index, build_project_session_index,
};
use uuid::Uuid;

fn session(project_id: ProjectId, value: u128, position: PositionKey) -> HostedSession {
    HostedSession {
        id: HostedSessionId::from_uuid(Uuid::from_u128(value)),
        project_id,
        group_id: None,
        preset_id: None,
        title: SessionTitle::new(&format!("Session {value}")).unwrap(),
        title_source: TitleSource::Default,
        lifecycle: HostedSessionState::Live,
        activity: ActivityAggregate::default(),
        pinned: value.is_multiple_of(2),
        position,
        last_output_sequence: OutputSequence::ZERO,
        read_through_sequence: OutputSequence::ZERO,
        unread_sequence: None,
        archived_at: None,
        created_at: value as u64,
        updated_at: value as u64,
        revision: Revision::ZERO,
    }
}

#[test]
fn both_indexes_are_byte_deterministic_for_reordered_inputs_at_ten_thousand_sessions() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("project");
    std::fs::create_dir(&root).unwrap();
    let project_id = ProjectId::from_uuid(Uuid::from_u128(1));
    let project = Project {
        id: project_id,
        display_name: LocalizedUserText::new("Deterministic project").unwrap(),
        canonical_root: CanonicalPath::resolve(&root).unwrap(),
        position: PositionKey::FIRST,
        revision: Revision::ZERO,
    };
    let revisions = IndexSourceRevisions {
        projects: Revision::new(7),
        sessions: Revision::new(9),
        presets: Revision::new(3),
    };
    let mut forward = (0..10_000_u128)
        .map(|index| {
            session(
                project_id,
                index + 10,
                PositionKey::rebalanced(index as usize).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    let mut reverse = forward.clone();
    reverse.reverse();

    let project_forward =
        build_project_session_index(revisions, std::slice::from_ref(&project), &forward).unwrap();
    let project_reverse =
        build_project_session_index(revisions, std::slice::from_ref(&project), &reverse).unwrap();
    assert_eq!(
        serde_json::to_vec(&project_forward).unwrap(),
        serde_json::to_vec(&project_reverse).unwrap()
    );

    let palette_forward = build_palette_index(
        revisions,
        std::slice::from_ref(&project),
        &[],
        &[],
        &forward,
    )
    .unwrap();
    let palette_reverse = build_palette_index(
        revisions,
        std::slice::from_ref(&project),
        &[],
        &[],
        &reverse,
    )
    .unwrap();
    assert_eq!(
        serde_json::to_vec(&palette_forward).unwrap(),
        serde_json::to_vec(&palette_reverse).unwrap()
    );
    assert_eq!(palette_forward.documents.len(), 10_001);

    forward.rotate_left(4_321);
    let rotated =
        build_project_session_index(revisions, std::slice::from_ref(&project), &forward).unwrap();
    assert_eq!(project_forward, rotated);
}
