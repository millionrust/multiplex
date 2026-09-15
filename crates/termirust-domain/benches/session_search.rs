use std::time::{Duration, Instant};

use termirust_domain::{
    HostedSessionId, PositionKey, ProjectId, SearchAction, SearchCancellation, SearchDocument,
    SearchDocumentId, SearchDocumentInput, SearchIndex, SearchQuery, SearchStatus,
};
use uuid::Uuid;

fn main() {
    let project = ProjectId::from_uuid(Uuid::from_u128(1));
    let mut index = SearchIndex::default();
    for ordinal in 0..10_000u128 {
        let id = HostedSessionId::from_uuid(Uuid::from_u128(ordinal + 1));
        index
            .insert(
                SearchDocument::new(SearchDocumentInput {
                    id: SearchDocumentId::Session(id),
                    title: format!("Session {ordinal:05} parser workspace"),
                    project_id: Some(project),
                    project_label: Some(format!("Project {}", ordinal % 100)),
                    group_label: Some(format!("Group {}", ordinal % 32)),
                    preset_label: Some("Codex safe".to_string()),
                    runtime_label: Some("codex".to_string()),
                    status: match ordinal % 5 {
                        0 => SearchStatus::Attention,
                        1 => SearchStatus::Busy,
                        2 => SearchStatus::Done,
                        3 => SearchStatus::Running,
                        _ => SearchStatus::Idle,
                    },
                    pinned: ordinal % 17 == 0,
                    archived: ordinal % 7 == 0,
                    position: PositionKey::new((ordinal as u64 + 1) * 1024),
                    meaningful_activity_at: ordinal as u64,
                    action: SearchAction::OpenSession(id),
                })
                .expect("benchmark document should be bounded"),
            )
            .expect("10k session benchmark should fit the exact cap");
    }

    let queries = [
        "session 09999",
        "parser project:project",
        "is:running codex",
        "is:attention group",
        "runtime:codex workspace",
        "is:archived parser",
        "ssn999",
    ]
    .map(|query| SearchQuery::parse(query).expect("benchmark query should parse"));
    let cancellation = SearchCancellation::default();
    let mut samples = Vec::with_capacity(70);
    for query in queries.iter().cycle().take(70) {
        let started = Instant::now();
        let page = index
            .search(query, Some(project), &cancellation)
            .expect("benchmark search should complete");
        std::hint::black_box(page);
        samples.push(started.elapsed());
    }
    samples.sort_unstable();
    let p50 = samples[samples.len() / 2];
    let p95 = samples[(samples.len() * 95 / 100).min(samples.len() - 1)];
    println!(
        "session_search documents=10000 samples={} p50_ms={:.3} p95_ms={:.3}",
        samples.len(),
        p50.as_secs_f64() * 1000.0,
        p95.as_secs_f64() * 1000.0
    );
    assert!(
        p95 <= Duration::from_millis(50),
        "10k search p95 {:?} exceeded the 50 ms target",
        p95
    );
}
