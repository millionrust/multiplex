use multiplex_domain::{
    ActivityAggregate, CanonicalPath, HostedSession, HostedSessionId, HostedSessionState,
    IndexSourceRevisions, OutputSequence, PositionKey, Revision, SessionTitle, TitleSource,
    build_palette_index,
};
use uuid::Uuid;

fn session(folder: &CanonicalPath, value: u128, position: PositionKey) -> HostedSession {
    HostedSession {
        id: HostedSessionId::from_uuid(Uuid::from_u128(value)),
        folder: folder.clone(),
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
fn the_palette_index_is_byte_deterministic_for_reordered_inputs_at_ten_thousand_sessions() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("folder");
    std::fs::create_dir(&root).unwrap();
    let folder = CanonicalPath::resolve(&root).unwrap();
    let revisions = IndexSourceRevisions {
        sessions: Revision::new(9),
        presets: Revision::new(3),
    };
    let forward = (0..10_000_u128)
        .map(|index| {
            session(
                &folder,
                index + 10,
                PositionKey::rebalanced(index as usize).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    let mut reverse = forward.clone();
    reverse.reverse();

    let palette_forward = build_palette_index(revisions, &[], &[], &forward).unwrap();
    let palette_reverse = build_palette_index(revisions, &[], &[], &reverse).unwrap();
    assert_eq!(
        serde_json::to_vec(&palette_forward).unwrap(),
        serde_json::to_vec(&palette_reverse).unwrap()
    );
    assert_eq!(palette_forward.documents.len(), 10_000);

    let mut rotated_input = forward.clone();
    rotated_input.rotate_left(4_321);
    let rotated = build_palette_index(revisions, &[], &[], &rotated_input).unwrap();
    assert_eq!(palette_forward, rotated);
}
