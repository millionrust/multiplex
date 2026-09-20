//! Bytes on the wire for synthetic workloads, held to the gate targets in section 4.8 of
//! `docs/remote-screens-implementation-plan.md`. Print the full table with
//! `cargo run -p multiplex-screen-codec --release --example workload_report`.

#[path = "support/workloads.rs"]
mod workloads;

#[test]
fn idle_desktops_send_nothing() {
    let report = workloads::idle();
    assert_eq!(
        report.bytes, 0,
        "idle frames produced {} bytes",
        report.bytes
    );
}

#[test]
fn typing_stays_under_forty_kilobytes_a_second() {
    let report = workloads::typing();
    assert!(
        report.kb_per_second() < 40.0,
        "typing used {:.1} KB/s",
        report.kb_per_second()
    );
    // The upper bound alone was not enough. This workload once drew the typed line and nothing
    // else, measured a tenth of its target, and passed — a thin workload looks exactly like a fast
    // codec from here. A real editor sends something on most keystrokes, so if this count
    // collapses the workload has stopped representing typing and the number above means nothing.
    assert!(
        report.batches >= 60,
        "typing sent only {} batches over {:.1} s, so the workload is not typing any more",
        report.batches,
        report.seconds
    );
}

#[test]
fn a_video_region_on_the_tile_path_stays_under_its_stage_a_target() {
    // 600 kbps is 75 KB/s. Measured on ordinary moving picture, which is what the target is about;
    // `video_noise` bounds the pathological case and is deliberately not held to this.
    let report = workloads::video_smooth();
    assert!(
        report.kb_per_second() < 75.0,
        "a video region used {:.1} KB/s ({:.0} kbps)",
        report.kb_per_second(),
        report.kb_per_second() * 8.0
    );
    assert!(
        report.batches >= 20,
        "the video workload sent only {} batches, so it is not playing",
        report.batches
    );
}

#[test]
fn scrolling_stays_under_one_hundred_twenty_kilobytes_a_second() {
    let report = workloads::scrolling();
    assert!(
        report.kb_per_second() < 120.0,
        "scrolling used {:.1} KB/s",
        report.kb_per_second()
    );
}

#[test]
fn switching_back_to_a_window_comes_from_cache() {
    let report = workloads::window_switching();
    assert!(
        report.largest_batch <= 300_000,
        "largest switch was {} bytes",
        report.largest_batch
    );
    let switches: Vec<usize> = (0..report.per_batch.len())
        .filter(|index| (index + 1) % 60 == 0)
        .map(|index| report.per_batch[index])
        .collect();
    let first_back = switches[1];
    let later_back = switches[3];
    assert!(
        later_back * 4 < switches[0].max(1) || later_back < 2_000,
        "switching back cost {later_back} bytes after {first_back} (first switch {})",
        switches[0]
    );
}
