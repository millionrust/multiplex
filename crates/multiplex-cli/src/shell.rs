//! `multiplex-cli shell [-- PROGRAM ARGS...]`: a person's own shell, in a durable session their
//! paired devices can reach.
//!
//! A terminal profile (Windows Terminal, VS Code, and the like) runs this in place of the shell.
//! It starts a Session Host running the real shell in the folder the window opened in, records the
//! session where the Controller listener looks for them, and attaches this window to it. Typing,
//! resizing, and Ctrl-C reach the shell as they would directly. When the shell exits, the window
//! closes; when the window closes first, the session keeps running for paired devices, which is
//! the point.
//!
//! Nothing outside the profile changes: scripts, `cmd /c`, `powershell -Command`, and every other
//! program that starts a shell keep starting the plain one. When this runs without a terminal, or
//! inside another Multiplex shell, it runs the program directly rather than wrapping it.

use std::collections::BTreeMap;
use std::io::IsTerminal as _;
use std::path::{Path, PathBuf};

use multiplex_domain::{HostInstanceId, HostedSessionId, OutputSequence};
use multiplex_session_host::{LaunchDescriptor, StopDeadlines};
use multiplex_store::{
    CONSOLE_SESSIONS_DIR, ConsoleSessionRecord, JournalLimits, write_console_session,
};

use crate::local::{CliPaths, HostAttachRequest, HostLauncher as _, ProcessHostLauncher};
use crate::local_attach::{AttachStyle, run_attach};
use crate::{Cancellation, CliError, ErrorCode};

/// Set in every shell this starts, so a profile that runs the launcher again from inside one
/// runs the program plainly instead of nesting a session in a session.
pub const SHELL_SESSION_ENV: &str = "MULTIPLEX_SHELL_SESSION";
pub struct ShellLauncher {
    paths: CliPaths,
}

impl ShellLauncher {
    pub fn new(paths: CliPaths) -> Self {
        Self { paths }
    }

    /// Runs `program` (or this platform's shell) in a durable session and attaches to it, or runs
    /// it directly where a session would not help. Returns the exit code to leave with.
    pub fn execute(
        &self,
        program: Option<String>,
        arguments: Vec<String>,
        cancellation: &Cancellation,
    ) -> Result<i32, CliError> {
        let (program, arguments) = match program {
            Some(program) => (program, arguments),
            None => default_shell(),
        };
        let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
        let nested = std::env::var_os(SHELL_SESSION_ENV).is_some();
        if !interactive || nested || !self.paths.host_executable().is_file() {
            return run_directly(&program, &arguments);
        }
        let executable = resolve_program(&program).ok_or_else(|| {
            CliError::new(
                ErrorCode::Unavailable,
                "the shell to run was not found",
                "Name the shell by its full path, or check PATH.",
            )
        })?;
        let working_directory = std::env::current_dir().map_err(|_| {
            CliError::new(
                ErrorCode::Unavailable,
                "the current folder is unavailable",
                "Open the terminal in a folder that still exists.",
            )
        })?;
        // A terminal that reports no size, as some do before their window is drawn, gets the
        // classic one; the first resize puts the real size right.
        let (columns, rows) = crossterm::terminal::size()
            .ok()
            .filter(|&(columns, rows)| columns >= 10 && rows >= 2)
            .unwrap_or((80, 24));
        let columns = columns.min(1_000);
        let rows = rows.min(1_000);
        let session_id = HostedSessionId::new();
        let host_instance_id = HostInstanceId::new();
        let session_dir = self
            .paths
            .config_root()
            .join(CONSOLE_SESSIONS_DIR)
            .join(session_id.to_string());
        std::fs::create_dir_all(&session_dir).map_err(|_| unavailable_storage())?;
        let runtime_root = self.paths.runtime_parent().join(session_id.to_string());
        // The shell gets this terminal's environment, as it would started directly, and learns
        // that it runs in a Multiplex session.
        let mut environment: BTreeMap<String, String> = std::env::vars().collect();
        environment.insert(SHELL_SESSION_ENV.to_owned(), session_id.to_string());
        let descriptor = LaunchDescriptor {
            format_version: LaunchDescriptor::FORMAT_VERSION,
            session_id,
            host_instance_id,
            expected_occupant_generation: None,
            runtime_root: runtime_root.clone(),
            session_dir: session_dir.clone(),
            executable,
            runtime_detection: None,
            arguments,
            environment,
            cwd: Some(working_directory.clone()),
            columns,
            rows,
            journal_limits: JournalLimits::default(),
            stop_deadlines: StopDeadlines::default(),
        };
        ProcessHostLauncher { detached: true }.launch(
            &descriptor,
            self.paths.host_executable(),
            cancellation,
        )?;
        let record = ConsoleSessionRecord {
            schema_version: ConsoleSessionRecord::SCHEMA_VERSION,
            session_id,
            program: display_name(&program),
            working_directory,
            started_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_secs())
                .unwrap_or_default(),
        };
        write_console_session(&session_dir, &record).map_err(|_| unavailable_storage())?;

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| {
                CliError::new(
                    ErrorCode::Unavailable,
                    "unable to start the terminal session",
                    "Try opening the terminal again.",
                )
            })?;
        let attached = runtime.block_on(run_attach(
            session_id,
            crate::local::ValidatedSessionAttach {
                runtime_root,
                request: HostAttachRequest {
                    expected_host_instance_id: host_instance_id,
                    from_sequence: OutputSequence::ZERO,
                    columns,
                    rows,
                    request_control: true,
                },
            },
            cancellation,
            AttachStyle::Shell,
        ));
        attached.map(|()| 0)
    }
}

/// The shell this platform's terminals open by default.
fn default_shell() -> (String, Vec<String>) {
    #[cfg(windows)]
    {
        for candidate in ["pwsh.exe", "powershell.exe"] {
            if resolve_program(candidate).is_some() {
                return (candidate.to_owned(), vec!["-NoLogo".to_owned()]);
            }
        }
        (
            std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_owned()),
            Vec::new(),
        )
    }
    #[cfg(not(windows))]
    {
        (
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned()),
            vec!["-l".to_owned()],
        )
    }
}

/// A program named on its own is looked up on PATH, as a terminal would; a path is taken as is.
/// The Host records a resolved, canonical path.
fn resolve_program(program: &str) -> Option<PathBuf> {
    let named = Path::new(program);
    if named.components().count() > 1 || named.is_absolute() {
        return std::fs::canonicalize(named).ok();
    }
    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(program);
        if candidate.is_file() {
            return std::fs::canonicalize(candidate).ok();
        }
        #[cfg(windows)]
        {
            let with_exe = directory.join(format!("{program}.exe"));
            if with_exe.is_file() {
                return std::fs::canonicalize(with_exe).ok();
            }
        }
    }
    None
}

fn display_name(program: &str) -> String {
    Path::new(program)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(program)
        .to_owned()
}

/// Runs the program in this terminal, as if the profile had named it directly.
fn run_directly(program: &str, arguments: &[String]) -> Result<i32, CliError> {
    std::process::Command::new(program)
        .args(arguments)
        .status()
        .map(|status| status.code().unwrap_or(1))
        .map_err(|_| {
            CliError::new(
                ErrorCode::Unavailable,
                "the shell to run could not be started",
                "Name the shell by its full path, or check PATH.",
            )
        })
}

fn unavailable_storage() -> CliError {
    CliError::new(
        ErrorCode::Unavailable,
        "Multiplex could not record the terminal session",
        "Check that the Multiplex data folder is writable.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_is_named_without_its_path_or_extension() {
        assert_eq!(display_name("pwsh.exe"), "pwsh");
        assert_eq!(display_name("/bin/zsh"), "zsh");
    }
}
