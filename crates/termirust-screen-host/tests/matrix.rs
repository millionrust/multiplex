//! The M5 network matrix, for the two pass criteria that do not need a device.
//!
//! Print the table with
//! `cargo run -p termirust-screen-host --release --example network_matrix`.

#[path = "support/matrix.rs"]
mod matrix;

use matrix::run_all;

#[test]
fn no_profile_stalls_longer_than_a_second_or_takes_three_to_come_back() {
    let cells = run_all();
    assert_eq!(
        cells.len(),
        24,
        "four profiles, three workloads, two stages"
    );
    for cell in &cells {
        let stage = if cell.stage_b { "Stage B" } else { "Stage A" };
        assert!(
            cell.longest_stall_millis <= 1_000,
            "{stage} {} on {} stalled for {} ms",
            cell.workload.name(),
            cell.profile,
            cell.longest_stall_millis
        );
        assert!(
            cell.caught_up_after_millis <= 3_000,
            "{stage} {} on {} took {} ms to show the current screen again after a ten-second \
             outage",
            cell.workload.name(),
            cell.profile,
            cell.caught_up_after_millis
        );
    }
}

#[test]
fn every_cell_delivers_a_picture_at_all() {
    // The floor beneath the other criteria: a profile where nothing ever reached the viewer would
    // report no stalls and instant convergence, because both are measured on a screen that never
    // moves. This is what stops the table being vacuously green.
    for cell in run_all() {
        let stage = if cell.stage_b { "Stage B" } else { "Stage A" };
        assert!(
            cell.bytes > 0,
            "{stage} {} on {} sent nothing at all",
            cell.workload.name(),
            cell.profile
        );
        assert!(
            cell.exact_after_millis < u64::MAX / 1_000,
            "{stage} {} on {} never reached an exact screen within thirty seconds",
            cell.workload.name(),
            cell.profile
        );
    }
}
