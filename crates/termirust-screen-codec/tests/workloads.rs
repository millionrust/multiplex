//! Bytes on the wire for synthetic workloads, held to the gate targets in section 4.8 of
//! `docs/remote-screens-implementation-plan.md`. Print the full table with
//! `cargo run -p termirust-screen-codec --release --example workload_report`.

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
