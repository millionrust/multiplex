//! Stable, bounded, one-shot local command surface for TermiRust.

mod args;
mod contract;
mod local;
mod remote_ssh;
mod render;

use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub use args::{
    ApprovalDecision, CliCommand, ControllerSshAction, ControllerSshCommand, Invocation,
    SessionListFilter, parse_args,
};
pub use contract::*;
pub use local::{
    CliClock, CliIds, CliInstallationStatus, CliPaths, HostController, HostLaunchOutcome,
    HostLauncher, LocalCommandService, SshControllerCommandExecutor, cli_installation_status,
};
pub use remote_ssh::SystemSshControllerExecutor;
pub use render::{RenderOptions, render_failure, render_success};

#[derive(Clone, Default)]
pub struct Cancellation {
    cancelled: Arc<AtomicBool>,
}

impl Cancellation {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

pub trait CommandService {
    fn execute(
        &mut self,
        command: CliCommand,
        cancellation: &Cancellation,
    ) -> Result<CliData, CliError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: i32,
}

pub fn write_output(
    output: &RunOutput,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> io::Result<()> {
    stdout.write_all(&output.stdout)?;
    stderr.write_all(&output.stderr)
}

pub fn run(
    service: &mut dyn CommandService,
    arguments: Vec<String>,
    terminal_width: usize,
    cancellation: &Cancellation,
) -> RunOutput {
    let wants_json = arguments.iter().any(|argument| argument == "--json");
    let invocation = match parse_args(arguments) {
        Ok(invocation) => invocation,
        Err(error) => return failure_output(error, wants_json, terminal_width),
    };
    run_parsed(Some(service), invocation, terminal_width, cancellation)
}

pub fn run_parsed(
    service: Option<&mut dyn CommandService>,
    invocation: Invocation,
    terminal_width: usize,
    cancellation: &Cancellation,
) -> RunOutput {
    let result = if invocation.command == CliCommand::Help {
        Ok(help_data())
    } else {
        service.map_or_else(
            || {
                Err(CliError::new(
                    ErrorCode::OperationFailed,
                    "command service is unavailable",
                    "Retry the command after TermiRust is installed correctly.",
                ))
            },
            |service| service.execute(invocation.command, cancellation),
        )
    };
    match result {
        Ok(data) => match render_success(
            &data,
            &[],
            RenderOptions {
                json: invocation.json,
                terminal_width,
            },
        ) {
            Ok(stdout) => RunOutput {
                stdout,
                stderr: Vec::new(),
                exit_code: 0,
            },
            Err(error) => failure_output(error, invocation.json, terminal_width),
        },
        Err(error) => failure_output(error, invocation.json, terminal_width),
    }
}

pub fn failure_output(error: CliError, json: bool, terminal_width: usize) -> RunOutput {
    let rendered = render_failure(&error, json, terminal_width);
    let (stdout, stderr) = if json {
        (rendered, Vec::new())
    } else {
        (Vec::new(), rendered)
    };
    RunOutput {
        stdout,
        stderr,
        exit_code: error.code.exit_code(),
    }
}

pub(crate) fn help_data() -> CliData {
    CliData::Help(HelpData {
        commands: vec![
            "status [--json]".into(),
            "project list [--json]".into(),
            "preset list --project <ProjectId> [--json]".into(),
            "session list [--project <id>] [--group <id>] [--state <value>] [--archived] [--json]".into(),
            "session show <HostedSessionId> [--json]".into(),
            "session launch --project <id> --preset <id> [--group <id>] [--json]".into(),
            "session stop <id> [--expected-revision N] --yes [--json]".into(),
            "session archive <id> [--expected-revision N] [--json]".into(),
            "session restore <id> [--expected-revision N] [--json]".into(),
            "controller ssh --host <host> [--user <user>] [--port <port>] pair [--json]".into(),
            "controller ssh --host <host> [--user <user>] [--port <port>] sessions [--json]".into(),
            "controller ssh --host <host> [--user <user>] [--port <port>] attach --session <id> --generation N [--from-sequence N] [--columns N] [--rows N] [--write] [--json]".into(),
            "controller ssh --host <host> [--user <user>] [--port <port>] input --session <id> --generation N [--json] < stdin".into(),
            "controller ssh --host <host> [--user <user>] [--port <port>] resize --session <id> --generation N --columns N --rows N [--json]".into(),
            "controller ssh --host <host> [--user <user>] [--port <port>] approval --session <id> --generation N --approval <id> --decision <allow|deny> [--json]".into(),
            "controller ssh --host <host> [--user <user>] [--port <port>] detach --session <id> --generation N [--json]".into(),
        ],
        safety: "Local metadata and authenticated Host commands only. Stop requires --yes. SSH Controller input is read only from stdin. Mutations never silently retry conflicts or unknown completion. Output can contain user-chosen project, preset, and session titles and may be sensitive.".into(),
        exit_codes: vec![
            "0 success".into(),
            "2 usage or validation".into(),
            "3 unavailable or incompatible".into(),
            "4 permission denied or interaction required".into(),
            "5 stale revision or conflict".into(),
            "6 resource or quota limit".into(),
            "7 operation failure or timeout".into(),
            "130 signal cancellation".into(),
        ],
    })
}
