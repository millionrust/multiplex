//! Prints bytes on the wire for synthetic workloads against the targets in section 4.8 of
//! `docs/remote-screens-implementation-plan.md`.
//!
//! `cargo run -p multiplex-screen-codec --release --example workload_report`

#[path = "../tests/support/workloads.rs"]
mod workloads;

fn main() {
    let reports = [
        workloads::idle(),
        workloads::typing(),
        workloads::scrolling(),
        workloads::window_switching(),
        workloads::video_smooth(),
        workloads::video_noise(),
    ];
    println!(
        "Surface {} x {}, 30 frames a second, synthetic content\n",
        workloads::WIDTH,
        workloads::HEIGHT
    );
    println!("| Workload | Seconds | Batches sent | Total KB | KB/s | Largest batch KB | Target |");
    println!("|---|---:|---:|---:|---:|---:|---|");
    for report in reports {
        println!(
            "| {} | {:.1} | {} | {:.1} | {:.1} | {:.1} | {} |",
            report.name,
            report.seconds,
            report.batches,
            report.bytes as f64 / 1_000.0,
            report.kb_per_second(),
            report.largest_batch as f64 / 1_000.0,
            report.target,
        );
    }
}
