use std::path::{Path, PathBuf};
use termirust_browser_engine_spike::{
    SpikeError, generate_fixture_only_report, run_child, write_report_atomic,
};

struct RunArgs {
    fixtures: PathBuf,
    runs: u32,
    output: PathBuf,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("browser spike failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), SpikeError> {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    if arguments.first().and_then(|arg| arg.to_str()) == Some("--child") {
        let kind = arguments
            .get(1)
            .and_then(|arg| arg.to_str())
            .ok_or(SpikeError::InvalidArgument("child kind is required"))?;
        let profile = arguments
            .windows(2)
            .find(|pair| pair[0] == "--profile")
            .map(|pair| Path::new(&pair[1]));
        return run_child(kind, profile);
    }

    let args = parse_run_args(&arguments)?;
    let generated_at = std::env::var("TERMIRUST_SPIKE_TIMESTAMP")
        .map_err(|_| SpikeError::InvalidArgument("TERMIRUST_SPIKE_TIMESTAMP is required"))?;
    let rustc = std::env::var("TERMIRUST_SPIKE_RUSTC")
        .map_err(|_| SpikeError::InvalidArgument("TERMIRUST_SPIKE_RUSTC is required"))?;
    let executable = std::env::current_exe()?;
    let scratch = args
        .output
        .parent()
        .ok_or(SpikeError::InvalidArgument(
            "output needs a parent directory",
        ))?
        .join("scratch");
    let report = generate_fixture_only_report(
        &args.fixtures,
        args.runs,
        &executable,
        &scratch,
        &generated_at,
        &rustc,
    )?;
    write_report_atomic(&args.output, &report)?;
    println!(
        "wrote fixture-only No-Go report to {}",
        args.output.display()
    );
    Ok(())
}

fn parse_run_args(arguments: &[std::ffi::OsString]) -> Result<RunArgs, SpikeError> {
    if arguments.first().and_then(|arg| arg.to_str()) != Some("--fixture-only") {
        return Err(SpikeError::InvalidArgument("--fixture-only is required"));
    }
    let mut fixtures = None;
    let mut runs = None;
    let mut output = None;
    let mut index = 1;
    while index < arguments.len() {
        let name = arguments[index]
            .to_str()
            .ok_or(SpikeError::InvalidArgument("arguments must be UTF-8"))?;
        let value = arguments
            .get(index + 1)
            .ok_or(SpikeError::InvalidArgument("option value is missing"))?;
        match name {
            "--fixtures" => fixtures = Some(PathBuf::from(value)),
            "--runs" => {
                runs = Some(
                    value
                        .to_str()
                        .and_then(|value| value.parse().ok())
                        .ok_or(SpikeError::InvalidArgument("runs must be an integer"))?,
                )
            }
            "--output" => output = Some(PathBuf::from(value)),
            _ => return Err(SpikeError::InvalidArgument("unknown option")),
        }
        index += 2;
    }
    Ok(RunArgs {
        fixtures: fixtures.ok_or(SpikeError::InvalidArgument("--fixtures is required"))?,
        runs: runs.ok_or(SpikeError::InvalidArgument("--runs is required"))?,
        output: output.ok_or(SpikeError::InvalidArgument("--output is required"))?,
    })
}
