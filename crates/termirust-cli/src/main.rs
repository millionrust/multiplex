use std::io::Write as _;

use termirust_cli::{
    Cancellation, CliCommand, CliPaths, ControllerSshAction, LocalCommandService,
    LocalSessionAttachExecutor, SystemSshControllerExecutor, failure_output, parse_args,
    read_removal_confirmation, read_session_input, run_parsed, write_output,
};

fn main() {
    let cancellation = Cancellation::default();
    let signal_cancellation = cancellation.clone();
    if ctrlc::set_handler(move || signal_cancellation.cancel()).is_err() {
        std::process::exit(7);
    }
    let arguments = match std::env::args_os()
        .skip(1)
        .map(|argument| argument.into_string())
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(arguments) => arguments,
        Err(_) => {
            let _ = writeln!(
                std::io::stderr(),
                "error[usage]: command arguments must be valid Unicode\nhint: Run termirust-cli --help for the frozen command syntax."
            );
            std::process::exit(2);
        }
    };
    let wants_json = arguments.iter().any(|argument| argument == "--json");
    let width = std::env::var("COLUMNS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|width| (40..=500).contains(width))
        .unwrap_or(80);
    let mut invocation = match parse_args(arguments) {
        Ok(invocation) => invocation,
        Err(error) => {
            exit_with_output(failure_output(error, wants_json, width));
        }
    };
    if invocation.command == CliCommand::Help {
        exit_with_output(run_parsed(None, invocation, width, &cancellation));
    }
    let paths = match CliPaths::discover() {
        Ok(paths) => paths,
        Err(error) => {
            exit_with_output(failure_output(error, wants_json, width));
        }
    };
    let reads_removal_confirmation = matches!(
        &invocation.command,
        CliCommand::SessionRemove {
            confirmation_stdin: true,
            ..
        }
    );
    if reads_removal_confirmation {
        let confirmation = match read_removal_confirmation(&mut std::io::stdin()) {
            Ok(confirmation) => confirmation,
            Err(error) => exit_with_output(failure_output(error, invocation.json, width)),
        };
        invocation.command = match invocation
            .command
            .clone()
            .with_removal_confirmation(confirmation)
        {
            Ok(command) => command,
            Err(error) => exit_with_output(failure_output(error, invocation.json, width)),
        };
    }
    let reads_session_input = matches!(
        &invocation.command,
        CliCommand::SessionInput {
            input_stdin: true,
            ..
        }
    );
    if reads_session_input {
        let input = match read_session_input(&mut std::io::stdin()) {
            Ok(input) => input,
            Err(error) => exit_with_output(failure_output(error, invocation.json, width)),
        };
        invocation.command = match invocation.command.clone().with_session_input(input) {
            Ok(command) => command,
            Err(error) => exit_with_output(failure_output(error, invocation.json, width)),
        };
    }
    if let CliCommand::ControllerSsh(command) = &invocation.command
        && matches!(command.action, ControllerSshAction::Attach { .. })
        && !invocation.json
    {
        let executor = SystemSshControllerExecutor::new(paths.config_root());
        match executor.execute_interactive_attach(command.clone(), &cancellation) {
            Ok(()) => std::process::exit(0),
            Err(error) => exit_with_output(failure_output(error, false, width)),
        }
    }
    if matches!(invocation.command, CliCommand::SessionAttach { .. }) && !invocation.json {
        let executor = LocalSessionAttachExecutor::new(paths.clone());
        match executor.execute(invocation.command.clone(), &cancellation) {
            Ok(()) => std::process::exit(0),
            Err(error) => exit_with_output(failure_output(error, false, width)),
        }
    }
    let mut service = LocalCommandService::open(paths);
    let output = run_parsed(Some(&mut service), invocation, width, &cancellation);
    exit_with_output(output);
}

fn exit_with_output(output: termirust_cli::RunOutput) -> ! {
    if write_output(&output, &mut std::io::stdout(), &mut std::io::stderr()).is_err() {
        std::process::exit(7);
    }
    std::process::exit(output.exit_code);
}
