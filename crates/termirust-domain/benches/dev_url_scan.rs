use std::time::{Duration, Instant};

use termirust_domain::{DevUrlCancellation, DevUrlDetector};

fn main() {
    let mut bytes = Vec::with_capacity(8 * 1024 * 1024);
    for index in 0..65_536 {
        bytes.extend_from_slice(b"server output without an action ");
        if index % 1024 == 0 {
            bytes.extend_from_slice(b"http://localhost:3000/path\n");
        } else {
            bytes.extend_from_slice(b"https://example.com/rejected\n");
        }
    }
    let cancellation = DevUrlCancellation::default();
    let mut samples = Vec::with_capacity(20);
    for _ in 0..20 {
        let mut detector = DevUrlDetector::default();
        let started = Instant::now();
        let mut count = detector
            .observe(&bytes, &cancellation)
            .expect("benchmark scan should complete")
            .len();
        count += detector
            .finish(&cancellation)
            .expect("benchmark finish should complete")
            .len();
        std::hint::black_box(count);
        samples.push(started.elapsed());
    }
    samples.sort_unstable();
    let p50 = samples[samples.len() / 2];
    let p95 = samples[(samples.len() * 95 / 100).min(samples.len() - 1)];
    println!(
        "dev_url_scan bytes={} samples={} p50_ms={:.3} p95_ms={:.3}",
        bytes.len(),
        samples.len(),
        p50.as_secs_f64() * 1000.0,
        p95.as_secs_f64() * 1000.0
    );
    assert!(
        p95 <= Duration::from_millis(250),
        "bounded URL scan p95 {p95:?} exceeded 250 ms"
    );
}
