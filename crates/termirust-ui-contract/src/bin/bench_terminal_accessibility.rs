use std::env;
use std::process::ExitCode;
use std::time::Instant;

use termirust_ui_contract::{
    MAX_TERMINAL_ACCESSIBILITY_BYTES, MAX_TERMINAL_ACCESSIBILITY_LINES, TerminalAccessibilityBuffer,
};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bench-terminal-accessibility: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut bytes = None;
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--bytes" => {
                let value = arguments
                    .next()
                    .ok_or_else(|| "--bytes requires a positive integer".to_string())?;
                bytes = Some(
                    value
                        .parse::<usize>()
                        .map_err(|_| "--bytes requires a positive integer".to_string())?,
                );
            }
            "--help" | "-h" => {
                println!("Usage: bench-terminal-accessibility --bytes 104857600");
                return Ok(());
            }
            _ => return Err(format!("unknown argument {argument:?}")),
        }
    }
    let bytes = bytes.ok_or_else(|| "--bytes is required".to_string())?;
    if bytes == 0 {
        return Err("--bytes must be greater than zero".to_string());
    }

    let fixture = b"\x1b[32mfragmented ANSI\x1b[0m unicode \xf0\x9f\x99\x82 \xd8\xa8\n\x1b[?1049hframe\x1b[?1049l\n";
    let fragments = [1_usize, 3, 7, 31, 257, 4_093];
    let started = Instant::now();
    let mut processed = 0_usize;
    let mut sequence = 0_u64;
    let mut fragment_index = 0_usize;
    let mut fixture_offset = 0_usize;
    let mut buffer = TerminalAccessibilityBuffer::new(1, "Accessibility benchmark");
    while processed < bytes {
        let fragment_len = fragments[fragment_index % fragments.len()].min(bytes - processed);
        sequence = sequence.saturating_add(1);
        let mut fragment_remaining = fragment_len;
        while fragment_remaining > 0 {
            let take = fragment_remaining.min(fixture.len() - fixture_offset);
            buffer.append(
                &fixture[fixture_offset..fixture_offset + take],
                Some(sequence),
            );
            processed += take;
            fragment_remaining -= take;
            fixture_offset = (fixture_offset + take) % fixture.len();
        }
        fragment_index += 1;
    }
    let elapsed = started.elapsed();
    let snapshot = buffer.snapshot();
    if snapshot.text.len() > MAX_TERMINAL_ACCESSIBILITY_BYTES
        || buffer.retained_lines() > MAX_TERMINAL_ACCESSIBILITY_LINES
    {
        return Err("bounded terminal accessibility limits were exceeded".to_string());
    }
    let mib_per_second = bytes as f64 / 1_048_576_f64 / elapsed.as_secs_f64().max(f64::EPSILON);
    println!(
        "bytes={bytes} elapsed_ms={} throughput_mib_s={mib_per_second:.2} retained_bytes={} retained_lines={} truncated={}",
        elapsed.as_millis(),
        snapshot.text.len(),
        buffer.retained_lines(),
        snapshot.truncated,
    );
    Ok(())
}
