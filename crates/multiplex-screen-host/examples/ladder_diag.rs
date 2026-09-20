//! Traces what the rate estimator and the ladder do in one cell of the network matrix.
//!
//! `cargo run -p multiplex-screen-host --release --example ladder_diag -- [profile] [workload]`
//!
//! `profile` is 0 to 3 across section 7's four profiles, worst last; `workload` is typing,
//! scrolling or video. Defaults to the worst profile with video, which is the cell most likely to
//! be interesting.
//!
//! Written because a matrix that only reports a final rung cannot tell you *why* it is that rung.
//! The first thing this found was the harness's own fault rather than the ladder's: with a link
//! that handed each message over at a single instant there was no arrival spread to time, the
//! estimate stayed `None` for whole runs, and the ladder never moved because it was never told
//! anything.

#[path = "../tests/support/matrix.rs"]
mod matrix;

use multiplex_screen_protocol::FeatureSet;

fn main() {
    let mut args = std::env::args().skip(1);
    let profile = args
        .next()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(3)
        .min(matrix::PROFILES.len() - 1);
    let workload = match args.next().as_deref() {
        Some("typing") => matrix::Workload::Typing,
        Some("scrolling") => matrix::Workload::Scrolling,
        _ => matrix::Workload::Video,
    };
    let profile = matrix::PROFILES[profile];
    println!(
        "{} - {} - Stage B, tracing the estimate and the rung\n",
        profile.name,
        workload.name()
    );
    let cell = matrix::run_traced(profile, workload, FeatureSet::from_bits(FeatureSet::KNOWN));
    println!(
        "\nsettled at {:?} after {} bytes; stall {} ms, caught up {} ms",
        cell.rung, cell.bytes, cell.longest_stall_millis, cell.caught_up_after_millis
    );
}
