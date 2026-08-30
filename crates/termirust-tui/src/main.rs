use std::io;
use std::sync::Arc;

use termirust_tui::localization::TuiLocale;
use termirust_tui::source::LocalFleetSource;
use termirust_tui::terminal::{RunOptions, run};

fn main() {
    match parse_options(std::env::args().skip(1)) {
        Ok(ParseOutcome::Run(options)) => match LocalFleetSource::discover() {
            Ok(source) => {
                if let Err(error) = run(Arc::new(source), options) {
                    eprintln!("termirust-tui: {error}");
                    std::process::exit(3);
                }
            }
            Err(error) => {
                eprintln!(
                    "termirust-tui: {}: {}",
                    error.diagnostic.code, error.diagnostic.summary
                );
                std::process::exit(3);
            }
        },
        Ok(ParseOutcome::Help) => print_help(),
        Err(error) => {
            eprintln!("termirust-tui: {error}");
            eprintln!("Run termirust-tui --help for usage.");
            std::process::exit(2);
        }
    }
}

enum ParseOutcome {
    Run(RunOptions),
    Help,
}

fn parse_options(arguments: impl Iterator<Item = String>) -> io::Result<ParseOutcome> {
    let mut options = RunOptions::default();
    let mut arguments = arguments.peekable();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--inline" => options.inline = true,
            "--no-color" => options.no_color = true,
            "--recording-friendly" => options.recording_friendly = true,
            "--locale" => {
                let value = arguments.next().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "--locale requires a value")
                })?;
                options.locale = TuiLocale::parse(&value).ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "locale must be en-US, en-XA, or ar-XB",
                    )
                })?;
            }
            "--help" | "-h" => return Ok(ParseOutcome::Help),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("unknown option {argument}"),
                ));
            }
        }
    }
    Ok(ParseOutcome::Run(options))
}

fn print_help() {
    println!("termirust-tui [--inline] [--no-color] [--recording-friendly] [--locale LOCALE]");
    println!();
    println!("Project and Session fleet navigation with one durable terminal attachment.");
    println!("Locales: en-US, en-XA, ar-XB");
    println!("Fleet keys: arrows/j/k, Left/Right, Tab/Shift+Tab, Enter, /, Esc, i, r, ?, q");
    println!("Terminal escape: Ctrl+Space then Esc. Ctrl+Space then Space sends NUL.");
    println!("This binary cannot launch, stop, archive, or modify session metadata.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_strict_and_no_color_honors_explicit_flag() {
        let ParseOutcome::Run(options) = parse_options(
            ["--inline", "--no-color", "--locale", "en-XA"]
                .into_iter()
                .map(str::to_string),
        )
        .unwrap() else {
            panic!("expected run options");
        };
        assert!(options.inline);
        assert!(options.no_color);
        assert_eq!(options.locale, TuiLocale::PseudoExpanded);
        assert!(parse_options(["--write".to_string()].into_iter()).is_err());
    }
}
