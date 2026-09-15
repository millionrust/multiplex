use serde_json::{Value, json};
use termirust_domain::{
    ActivityAggregate, HostedSession, HostedSessionId, HostedSessionState, OutputSequence,
    PositionKey, PresetId, ProjectId, Revision, SessionMutation, SessionTitle, TitleSource,
};
use termirust_store::SessionSnapshot;

const FIXTURE: &str = include_str!("../fixtures/controller/local-session-mutation-v1.json");
const MAX_FIXTURE_BYTES: usize = 16 * 1024;
pub const ARCHIVE_AT: u64 = 100;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FixtureMutation {
    Rename(String),
    SetPinned(bool),
    MarkRead,
    Archive,
    Restore,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FixtureStep {
    pub mutation: FixtureMutation,
    pub expected: Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StaleFixture {
    pub captured_session_revision: u64,
    pub intervening: FixtureMutation,
    pub attempt: FixtureMutation,
    pub expected: Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConformanceFixture {
    pub initial: HostedSession,
    pub expected_initial: Value,
    pub steps: Vec<FixtureStep>,
    pub stale: StaleFixture,
}

pub fn load_fixture() -> ConformanceFixture {
    assert!(FIXTURE.len() <= MAX_FIXTURE_BYTES);
    let document: Value = serde_json::from_str(FIXTURE).expect("fixture JSON should parse");
    assert_eq!(document["schema_version"], 1);
    let initial = &document["initial"];
    let session_id = parse_id::<HostedSessionId>(&initial["session_id"]);
    let project_id = parse_id::<ProjectId>(&initial["project_id"]);
    let preset_id = parse_id::<PresetId>(&initial["preset_id"]);
    let last_output_sequence = number(&initial["last_output_sequence"]);
    let read_through_sequence = number(&initial["read_through_sequence"]);
    let unread_sequence = number(&initial["unread_sequence"]);
    assert!(read_through_sequence <= last_output_sequence);
    assert!(unread_sequence <= last_output_sequence);
    assert_eq!(string(&initial["lifecycle"]), "exited");

    let steps = document["steps"]
        .as_array()
        .expect("fixture steps should be an array")
        .iter()
        .map(|step| FixtureStep {
            mutation: parse_mutation(step, "operation", "value"),
            expected: step["expected"].clone(),
        })
        .collect::<Vec<_>>();
    assert_eq!(steps.len(), 5);

    let stale = &document["stale"];
    ConformanceFixture {
        initial: HostedSession {
            id: session_id,
            project_id,
            group_id: None,
            preset_id: Some(preset_id),
            title: SessionTitle::new(string(&initial["title"])).expect("fixture title is valid"),
            title_source: TitleSource::Manual,
            lifecycle: HostedSessionState::Exited,
            activity: ActivityAggregate::default(),
            pinned: false,
            position: PositionKey::FIRST,
            last_output_sequence: OutputSequence::new(last_output_sequence),
            read_through_sequence: OutputSequence::new(read_through_sequence),
            unread_sequence: Some(OutputSequence::new(unread_sequence)),
            archived_at: None,
            created_at: 1,
            updated_at: 1,
            revision: Revision::ZERO,
        },
        expected_initial: document["expected_initial"].clone(),
        steps,
        stale: StaleFixture {
            captured_session_revision: number(&stale["captured_session_revision"]),
            intervening: parse_mutation(stale, "intervening_operation", "intervening_value"),
            attempt: parse_mutation(stale, "attempt_operation", "attempt_value"),
            expected: stale["expected"].clone(),
        },
    }
}

pub fn domain_mutation(mutation: &FixtureMutation) -> SessionMutation {
    match mutation {
        FixtureMutation::Rename(value) => {
            SessionMutation::Rename(SessionTitle::new(value).expect("fixture title is valid"))
        }
        FixtureMutation::SetPinned(value) => SessionMutation::SetPinned(*value),
        FixtureMutation::MarkRead => SessionMutation::MarkRead {
            through: OutputSequence::new(7),
        },
        FixtureMutation::Archive => SessionMutation::Archive { at: ARCHIVE_AT },
        FixtureMutation::Restore => SessionMutation::Restore,
    }
}

pub fn normalized_snapshot(snapshot: &SessionSnapshot) -> Value {
    assert_eq!(snapshot.sessions.len(), 1);
    let session = &snapshot.sessions[0];
    json!({
        "repository_revision": snapshot.revision.get(),
        "session": {
            "id": session.id.to_string(),
            "project_id": session.project_id.to_string(),
            "preset_id": session.preset_id.map(|id| id.to_string()),
            "title": session.title.as_str(),
            "title_source": serde_json::to_value(session.title_source).unwrap(),
            "lifecycle": serde_json::to_value(session.lifecycle).unwrap(),
            "activity": serde_json::to_value(session.activity.state).unwrap(),
            "pinned": session.pinned,
            "last_output_sequence": session.last_output_sequence.get(),
            "read_through_sequence": session.read_through_sequence.get(),
            "unread_sequence": session.unread_sequence.map(OutputSequence::get),
            "unread": session.unread(),
            "archived": session.archived_at.is_some(),
            "session_revision": session.revision.get()
        }
    })
}

fn parse_mutation(value: &Value, operation_key: &str, value_key: &str) -> FixtureMutation {
    match string(&value[operation_key]) {
        "rename" => FixtureMutation::Rename(string(&value[value_key]).to_string()),
        "set_pinned" => FixtureMutation::SetPinned(
            value[value_key]
                .as_bool()
                .expect("set_pinned value should be boolean"),
        ),
        "mark_read" => FixtureMutation::MarkRead,
        "archive" => FixtureMutation::Archive,
        "restore" => FixtureMutation::Restore,
        other => panic!("unsupported fixture mutation {other}"),
    }
}

fn parse_id<T>(value: &Value) -> T
where
    T: std::str::FromStr,
    T::Err: std::fmt::Debug,
{
    string(value).parse().expect("fixture ID should parse")
}

fn string(value: &Value) -> &str {
    value.as_str().expect("fixture value should be a string")
}

fn number(value: &Value) -> u64 {
    value.as_u64().expect("fixture value should be unsigned")
}
