//! Prints the software half of the M5 network matrix.
//!
//! `cargo run -p termirust-screen-host --release --example network_matrix`
//!
//! Section 7 of the plan asks for four profiles x three workloads x two stages, with four pass
//! criteria. Two of them are checked here and by `tests/matrix.rs`; the other two -- input-to-glass
//! P95 and how a real phone behaves -- need the device.

#[path = "../tests/support/matrix.rs"]
mod matrix;

fn main() {
    let cells = matrix::run_all();
    let seconds = matrix::cell_seconds();
    println!(
        "{} x {} pixels, {:.1} s of screen per cell, then a 10 s outage.\n",
        1280, 800, seconds
    );
    println!(
        "| Profile | Workload | Stage | kbps | Rung | Longest stall ms | Caught up ms | Exact ms |"
    );
    println!("|---|---|---|---:|---|---:|---:|---:|");
    for cell in &cells {
        println!(
            "| {} | {} | {} | {:.0} | {:?} | {} | {} | {} |",
            cell.profile,
            cell.workload.name(),
            if cell.stage_b { "B" } else { "A" },
            cell.kbps(seconds),
            cell.rung,
            cell.longest_stall_millis,
            cell.caught_up_after_millis,
            cell.exact_after_millis,
        );
    }
    let worst_stall = cells
        .iter()
        .map(|c| c.longest_stall_millis)
        .max()
        .unwrap_or(0);
    let worst_catch = cells
        .iter()
        .map(|c| c.caught_up_after_millis)
        .max()
        .unwrap_or(0);
    let worst_exact = cells
        .iter()
        .map(|c| c.exact_after_millis)
        .max()
        .unwrap_or(0);
    println!(
        "\nWorst: stall {worst_stall} ms (bound 1000), caught up {worst_catch} ms (bound 3000), \
         exact {worst_exact} ms (no bound; refinement is link-bound)."
    );
}
