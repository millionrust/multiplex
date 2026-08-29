use serde_json::to_writer_pretty;
use std::env;
use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use termirust_relay_spike::{MachineReport, SpikeReport, benchmark};

fn main() {
    if let Err(error) = run() {
        eprintln!("relay_spike_failed:{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let mut pair_counts = None;
    let mut runs = None;
    let mut output = None;
    let mut local_only = false;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--local-only" => local_only = true,
            "--pairs" => pair_counts = args.next(),
            "--runs" => runs = args.next(),
            "--output" => output = args.next().map(PathBuf::from),
            _ => return Err("unknown argument".into()),
        }
    }
    if !local_only {
        return Err("--local-only is required".into());
    }
    let pair_counts: Vec<usize> = pair_counts
        .ok_or("--pairs is required")?
        .split(',')
        .map(str::parse)
        .collect::<Result<_, _>>()?;
    let runs: usize = runs.ok_or("--runs is required")?.parse()?;
    let output = output.ok_or("--output is required")?;
    fs::create_dir_all(&output)?;
    let report = benchmark(
        &pair_counts,
        runs,
        "2026-08-29".to_string(),
        machine_report(),
    )?;
    write_report(output.join("relay-spike-report.json"), &report)?;
    println!(
        "relay_spike_complete scenarios={} runs={} output={}",
        report.scenarios.len(),
        report.runs_per_scenario,
        output.display()
    );
    Ok(())
}

fn write_report(path: PathBuf, report: &SpikeReport) -> Result<(), Box<dyn std::error::Error>> {
    let mut file = File::create(path)?;
    to_writer_pretty(&mut file, report)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

fn machine_report() -> MachineReport {
    MachineReport {
        os: env::consts::OS.to_string(),
        arch: env::consts::ARCH.to_string(),
        hardware: command_output("uname", &["-a"]),
        toolchain: command_output("rustc", &["--version"]),
        build_profile: if cfg!(debug_assertions) {
            "debug".to_string()
        } else {
            "release".to_string()
        },
    }
}

fn command_output(command: &str, arguments: &[&str]) -> String {
    Command::new(command)
        .args(arguments)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|output| output.trim().chars().take(512).collect())
        .unwrap_or_else(|| "unavailable".to_string())
}
