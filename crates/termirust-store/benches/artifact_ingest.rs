use std::fs::File;
use std::io::{BufWriter, Write as _};
use std::time::{Duration, Instant};

use termirust_domain::{
    ArtifactCancellation, ArtifactId, ArtifactScope, HostedSessionId, MAX_ARTIFACT_BYTES,
};
use termirust_store::{ArtifactIngestRequest, ArtifactRepository};

fn main() {
    let fixture = tempfile::tempdir().expect("benchmark fixture should be created");
    let source = fixture.path().join("source.bin");
    let mut writer = BufWriter::new(File::create(&source).expect("source should be created"));
    let block = [0x5a_u8; 64 * 1024];
    for _ in 0..MAX_ARTIFACT_BYTES / block.len() as u64 {
        writer
            .write_all(&block)
            .expect("bounded source write should succeed");
    }
    writer.flush().expect("source should flush");

    let repository = ArtifactRepository::open(fixture.path().join("durable-sessions"))
        .expect("artifact repository should open");
    let scope = ArtifactScope {
        session_id: HostedSessionId::new(),
    };
    let cancellation = ArtifactCancellation::default();
    let started = Instant::now();
    repository
        .ingest(
            ArtifactIngestRequest {
                id: ArtifactId::new(),
                scope,
                source: source.clone(),
                display_name: Some("bounded-benchmark.bin".to_string()),
                created_at: 1,
            },
            &cancellation,
            |_| {},
        )
        .expect("maximum-size artifact should ingest");
    let first = started.elapsed();

    let started = Instant::now();
    repository
        .ingest(
            ArtifactIngestRequest {
                id: ArtifactId::new(),
                scope,
                source,
                display_name: Some("bounded-benchmark-copy.bin".to_string()),
                created_at: 2,
            },
            &cancellation,
            |_| {},
        )
        .expect("same-session duplicate should ingest");
    let duplicate = started.elapsed();

    println!(
        "artifact_ingest bytes={} first_ms={:.3} duplicate_ms={:.3}",
        MAX_ARTIFACT_BYTES,
        first.as_secs_f64() * 1000.0,
        duplicate.as_secs_f64() * 1000.0
    );
    assert!(
        first <= Duration::from_secs(10),
        "25 MiB ingest {first:?} exceeded the ten-second safety target"
    );
    assert!(
        duplicate <= Duration::from_secs(10),
        "25 MiB duplicate ingest {duplicate:?} exceeded the ten-second safety target"
    );
}
