use std::time::{Duration, Instant};

use termirust_domain::{TranscriptCancellation, normalize_transcript_content};

fn main() {
    let mut fixture = String::with_capacity(1024 * 1024);
    while fixture.len() < 1024 * 1024 - 128 {
        fixture.push_str("User text with **markdown** and API_KEY=canary-secret\n");
    }
    let cancellation = TranscriptCancellation::default();
    let mut samples = Vec::with_capacity(20);
    for _ in 0..20 {
        let started = Instant::now();
        let normalized = normalize_transcript_content(&fixture, &cancellation)
            .expect("bounded transcript normalization should complete");
        std::hint::black_box(normalized);
        samples.push(started.elapsed());
    }
    samples.sort_unstable();
    let p50 = samples[samples.len() / 2];
    let p95 = samples[(samples.len() * 95 / 100).min(samples.len() - 1)];
    println!(
        "transcript_normalize bytes={} samples={} p50_ms={:.3} p95_ms={:.3}",
        fixture.len(),
        samples.len(),
        p50.as_secs_f64() * 1000.0,
        p95.as_secs_f64() * 1000.0
    );
    assert!(
        p95 <= Duration::from_secs(1),
        "1 MiB normalization p95 {p95:?} exceeded one second"
    );
}
