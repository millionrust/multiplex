use anyhow::{Context, Result, bail};
use portable_pty::{Child, CommandBuilder, PtySize, native_pty_system};
use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::Command as ProcessCommand;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use tokio::sync::mpsc as tokio_mpsc;

use crate::models::{ConnectRequest, LocalShellConfig};
use crate::ssh::{SessionCommand, SessionRuntimeHandle, SshEvent, SshEventSender};

#[cfg(windows)]
mod console_job;

enum WorkerEvent {
    Command(SessionCommand),
    CommandsClosed,
    ReaderClosed(String),
}

const TMUX_READY_ATTEMPTS: usize = 100;
const TMUX_READY_INTERVAL: Duration = Duration::from_millis(20);
const TMUX_DIAGNOSTIC_LIMIT: usize = 512;
const TERM_WITH_CLEAR_CAPABILITY: &str = "xterm-256color";

struct BuiltLocalCommand {
    command: CommandBuilder,
    tmux_readiness: Option<TmuxReadinessTarget>,
}

struct TmuxReadinessTarget {
    executable: PathBuf,
    session_name: String,
}

pub fn spawn_local_session(
    request: ConnectRequest,
    event_tx: SshEventSender,
) -> SessionRuntimeHandle {
    let (command_tx, command_rx) = tokio_mpsc::unbounded_channel();
    let session_id = request.session_id;
    let thread_name = format!("local-session-{session_id}");
    let fallback_tx = event_tx.clone();
    let fallback_thread_tx = fallback_tx.clone();

    let spawn_result = thread::Builder::new().name(thread_name).spawn(move || {
        let result = run_local_session(request, command_rx, event_tx);
        if let Err(error) = result {
            let message = format!("{error:#}");
            let _ = fallback_thread_tx.send(SshEvent::Error {
                session_id,
                message: message.clone(),
            });
            let _ = fallback_thread_tx.send(SshEvent::Disconnected {
                session_id,
                message,
            });
        }
    });

    if let Err(error) = spawn_result {
        let _ = fallback_tx.send(SshEvent::Error {
            session_id,
            message: format!("Failed to spawn local shell thread: {error}"),
        });
    }

    SessionRuntimeHandle { command_tx }
}

fn run_local_session(
    request: ConnectRequest,
    mut command_rx: tokio_mpsc::UnboundedReceiver<SessionCommand>,
    event_tx: SshEventSender,
) -> Result<()> {
    let session_id = request.session_id;
    let shell = request
        .local_shell
        .clone()
        .ok_or_else(|| anyhow::anyhow!("Local session is missing shell configuration"))?;
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(default_pty_size())
        .context("Unable to create a local PTY")?;

    let built_command = build_command(&request, &shell)?;
    let mut child = pair
        .slave
        .spawn_command(built_command.command)
        .context("Unable to launch the local shell")?;
    drop(pair.slave);
    #[cfg(windows)]
    console_job::adopt(child.process_id());

    if let Some(target) = built_command.tmux_readiness
        && let Err(readiness_error) =
            wait_for_tmux_readiness(TMUX_READY_ATTEMPTS, TMUX_READY_INTERVAL, || {
                probe_tmux_readiness(&target)
            })
    {
        let cleanup = terminate_owned_pty_process_group(child.as_mut());
        return match cleanup {
            Ok(()) => Err(readiness_error),
            Err(cleanup_error) => Err(readiness_error.context(format!(
                "The owned local PTY process group also could not be terminated: {cleanup_error}"
            ))),
        };
    }

    let mut writer = pair
        .master
        .take_writer()
        .context("Unable to attach a PTY writer")?;
    let mut reader = pair
        .master
        .try_clone_reader()
        .context("Unable to attach a PTY reader")?;
    let master = pair.master;

    // Before the reader exists, so the app hears of the connection ahead of any output. It answers
    // a program's questions about the terminal only on a connected pane, and on Windows the
    // pseudo-console's first output is such a question, which it waits on before writing more.
    let _ = event_tx.send(SshEvent::Connected {
        session_id,
        trusted_new_host: false,
    });

    // The reader sends output straight to the app, and a second thread forwards commands,
    // so the worker blocks until there is something to do instead of polling.
    let (worker_tx, worker_rx) = mpsc::channel();
    let reader_worker_tx = worker_tx.clone();
    let reader_event_tx = event_tx.clone();
    thread::Builder::new()
        .name(format!("local-session-reader-{session_id}"))
        .spawn(move || {
            let mut buffer = vec![0_u8; 65536];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => {
                        let _ = reader_worker_tx
                            .send(WorkerEvent::ReaderClosed("Local shell closed".to_string()));
                        break;
                    }
                    Ok(bytes_read) => {
                        let _ = reader_event_tx.send(SshEvent::Output {
                            session_id,
                            data: buffer[..bytes_read].to_vec(),
                        });
                    }
                    Err(error) => {
                        let _ = reader_worker_tx.send(WorkerEvent::ReaderClosed(format!(
                            "Unable to read from the local shell: {error}"
                        )));
                        break;
                    }
                }
            }
        })
        .context("Unable to spawn the local PTY reader")?;
    thread::Builder::new()
        .name(format!("local-session-commands-{session_id}"))
        .spawn(move || {
            while let Some(command) = command_rx.blocking_recv() {
                if worker_tx.send(WorkerEvent::Command(command)).is_err() {
                    return;
                }
            }
            let _ = worker_tx.send(WorkerEvent::CommandsClosed);
        })
        .context("Unable to spawn the local command forwarder")?;

    loop {
        let Ok(event) = worker_rx.recv() else {
            return Ok(());
        };
        match event {
            WorkerEvent::Command(SessionCommand::Input(data)) => {
                writer
                    .write_all(&data)
                    .context("Unable to write to the local shell")?;
                let _ = writer.flush();
            }
            WorkerEvent::Command(SessionCommand::Resize(size)) => {
                master
                    .resize(PtySize {
                        rows: size.rows,
                        cols: size.cols,
                        pixel_width: size.pixel_width,
                        pixel_height: size.pixel_height,
                    })
                    .context("Unable to resize the local PTY")?;
            }
            WorkerEvent::Command(SessionCommand::KillTmuxSession { session_name }) => {
                match kill_local_tmux_session(&session_name) {
                    Ok(()) => {
                        let _ = event_tx.send(SshEvent::TmuxSessionKilled {
                            session_id,
                            session_name,
                        });
                    }
                    Err(error) => {
                        let _ = event_tx.send(SshEvent::Error {
                            session_id,
                            message: format!("Unable to kill local tmux session: {error:#}"),
                        });
                    }
                }
            }
            WorkerEvent::Command(SessionCommand::StopDurable | SessionCommand::Disconnect) => {
                let _ = terminate_owned_pty_process_group(child.as_mut());
                let _ = child.wait();
                let _ = event_tx.send(SshEvent::Disconnected {
                    session_id,
                    message: "Local shell closed".to_string(),
                });
                return Ok(());
            }
            WorkerEvent::CommandsClosed => {
                let _ = terminate_owned_pty_process_group(child.as_mut());
                let _ = child.wait();
                return Ok(());
            }
            WorkerEvent::ReaderClosed(message) => {
                let _ = child.try_wait();
                let _ = event_tx.send(SshEvent::Disconnected {
                    session_id,
                    message,
                });
                return Ok(());
            }
        }
    }
}

fn build_command(request: &ConnectRequest, shell: &LocalShellConfig) -> Result<BuiltLocalCommand> {
    if request.persistent_session {
        let session_name = request
            .persistent_session_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Local tmux session name is empty"))?;
        let (tmux, _) = local_tmux_probe()?;
        let mut command = CommandBuilder::new(tmux.clone());
        for argument in persistent_tmux_arguments(
            session_name,
            request.persistent_session_detach_others,
            shell.cwd.as_deref(),
        ) {
            command.arg(argument);
        }
        let terminal_type = local_pty_terminal_type(command.get_env("TERM"));
        command.env("TERM", terminal_type);
        identify_terminal_program(&mut command);
        return Ok(BuiltLocalCommand {
            command,
            tmux_readiness: Some(TmuxReadinessTarget {
                executable: tmux,
                session_name: session_name.to_string(),
            }),
        });
    }
    if shell.program.trim().is_empty() {
        bail!("Local shell program is empty");
    }

    let mut command = CommandBuilder::new(shell.program.clone());
    for arg in &shell.args {
        command.arg(arg);
    }
    let terminal_type = local_pty_terminal_type(command.get_env("TERM"));
    command.env("TERM", terminal_type);
    identify_terminal_program(&mut command);
    if let Some(cwd) = shell.cwd.as_ref().filter(|cwd| !cwd.trim().is_empty()) {
        command.cwd(cwd);
    } else if let Some(home) = dirs::home_dir() {
        // Default to the user's home directory; otherwise the shell inherits
        // the directory the app was launched from.
        command.cwd(home);
    }
    Ok(BuiltLocalCommand {
        command,
        tmux_readiness: None,
    })
}

/// The value TermiRust's shells see in `TERM_PROGRAM`.
pub const TERMINAL_PROGRAM: &str = "TermiRust";

/// Names this app as the terminal, replacing whatever `TERM_PROGRAM` it was launched with.
/// Otherwise a shell started from Zed or iTerm2 would believe it runs there, and the
/// startup file that opens new terminals in tmux would wrap TermiRust's own tabs.
fn identify_terminal_program(command: &mut CommandBuilder) {
    command.env("TERM_PROGRAM", TERMINAL_PROGRAM);
    command.env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));
    // This terminal draws 24-bit colour, which `xterm-256color` cannot say. A tmux the user
    // starts themselves reads this and turns its own RGB support on, so a program inside it
    // keeps the colours it asked for instead of the nearest of 256.
    command.env("COLORTERM", "truecolor");
}

fn local_pty_terminal_type(inherited: Option<&OsStr>) -> OsString {
    inherited
        .filter(|terminal_type| terminal_type_supports_clear(terminal_type))
        .map(OsStr::to_os_string)
        .unwrap_or_else(|| OsString::from(TERM_WITH_CLEAR_CAPABILITY))
}

fn terminal_type_supports_clear(terminal_type: &OsStr) -> bool {
    terminal_type.to_str().is_some_and(|terminal_type| {
        let terminal_type = terminal_type.trim();
        !terminal_type.is_empty()
            && !terminal_type.eq_ignore_ascii_case("dumb")
            && !terminal_type.eq_ignore_ascii_case("unknown")
    })
}

fn wait_for_tmux_readiness(
    attempts: usize,
    retry_interval: Duration,
    mut probe: impl FnMut() -> Result<bool>,
) -> Result<()> {
    if attempts == 0 {
        bail!("Local tmux readiness probe has no configured attempts");
    }

    let mut last_diagnostic = "tmux pane was not ready".to_string();
    for attempt in 0..attempts {
        match probe() {
            Ok(true) => return Ok(()),
            Ok(false) => last_diagnostic = "tmux pane was not ready".to_string(),
            Err(error) => last_diagnostic = bounded_diagnostic(&format!("{error:#}")),
        }
        if attempt + 1 < attempts {
            thread::sleep(retry_interval);
        }
    }

    bail!(
        "Local tmux session did not become ready after {attempts} probes. Last probe: {last_diagnostic}"
    )
}

fn probe_tmux_readiness(target: &TmuxReadinessTarget) -> Result<bool> {
    let exact_target = format!("={}", target.session_name);
    let status = ProcessCommand::new(&target.executable)
        .args([
            "display-message",
            "-p",
            "-t",
            exact_target.as_str(),
            "#{pane_pid}",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .with_context(|| format!("Unable to probe {}", target.executable.display()))?;
    Ok(status.success())
}

fn bounded_diagnostic(message: &str) -> String {
    if message.len() <= TMUX_DIAGNOSTIC_LIMIT {
        return message.to_string();
    }
    let mut end = TMUX_DIAGNOSTIC_LIMIT;
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &message[..end])
}

#[cfg(unix)]
fn terminate_owned_pty_process_group(child: &mut dyn Child) -> std::io::Result<()> {
    let Some(process_id) = child.process_id() else {
        return child.kill();
    };
    let process_group = i32::try_from(process_id)
        .ok()
        .and_then(|process_id| process_id.checked_neg())
        .ok_or_else(|| std::io::Error::other("owned PTY process id is outside the signal range"))?;
    let result = unsafe { libc::kill(process_group, libc::SIGKILL) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(unix))]
fn terminate_owned_pty_process_group(child: &mut dyn Child) -> std::io::Result<()> {
    child.kill()
}

fn persistent_tmux_arguments(
    session_name: &str,
    detach_others: bool,
    cwd: Option<&str>,
) -> Vec<String> {
    let mut arguments = vec!["new-session".to_string(), "-A".to_string()];
    if detach_others {
        arguments.push("-D".to_string());
    }
    arguments.extend(["-s".to_string(), session_name.to_string()]);
    if let Some(cwd) = cwd.map(str::trim).filter(|cwd| !cwd.is_empty()) {
        arguments.extend(["-c".to_string(), cwd.to_string()]);
    }
    arguments
}

pub fn local_tmux_version() -> Result<String> {
    local_tmux_probe().map(|(_, version)| version)
}

fn local_tmux_probe() -> Result<(PathBuf, String)> {
    let tmux = multiplex_tmux::Tmux::discover()?;
    Ok((tmux.executable().to_path_buf(), tmux.version().to_owned()))
}

#[cfg(test)]
fn probe_tmux_candidates_with(
    candidates: impl IntoIterator<Item = PathBuf>,
    is_file: impl FnMut(&std::path::Path) -> bool,
    mut version_probe: impl FnMut(&std::path::Path) -> Result<String>,
) -> Result<(PathBuf, String)> {
    let tmux = multiplex_tmux::Tmux::select(candidates, is_file, |candidate| {
        version_probe(candidate).map_err(|error| format!("{error:#}"))
    })?;
    Ok((tmux.executable().to_path_buf(), tmux.version().to_owned()))
}

pub fn local_tmux_install_guidance() -> &'static str {
    if cfg!(target_os = "macos") {
        "Install tmux with Homebrew (`brew install tmux`), then restart TermiRust."
    } else if cfg!(target_os = "linux") {
        "Install tmux with your system package manager, then restart TermiRust."
    } else {
        "Install tmux and make sure it is available in PATH, then restart TermiRust."
    }
}

fn kill_local_tmux_session(session_name: &str) -> Result<()> {
    let (tmux, _) = local_tmux_probe()?;
    let output = ProcessCommand::new(tmux)
        .args(["kill-session", "-t", session_name])
        .output()
        .context("Unable to start tmux kill-session")?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!(if message.is_empty() {
            format!("tmux kill-session exited with {}", output.status)
        } else {
            message
        });
    }
    Ok(())
}

fn default_pty_size() -> PtySize {
    PtySize {
        rows: 48,
        cols: 160,
        pixel_width: 0,
        pixel_height: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        TERM_WITH_CLEAR_CAPABILITY, TMUX_DIAGNOSTIC_LIMIT, bounded_diagnostic,
        local_pty_terminal_type, local_tmux_install_guidance, local_tmux_probe,
        persistent_tmux_arguments, probe_tmux_candidates_with, spawn_local_session,
        terminal_type_supports_clear, terminate_owned_pty_process_group, wait_for_tmux_readiness,
    };
    use crate::models::{ConnectRequest, LocalShellConfig};
    use crate::ssh::{SessionCommand, SshEvent};
    use std::path::PathBuf;
    use std::process::{Child as ProcessChild, Command as ProcessCommand};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc::Receiver;
    use std::time::SystemTime;
    use std::time::{Duration, Instant};

    static TEST_SUFFIX: AtomicU64 = AtomicU64::new(1);

    struct TmuxSessionGuard {
        tmux: PathBuf,
        session_name: String,
        fixture: PathBuf,
    }

    impl Drop for TmuxSessionGuard {
        fn drop(&mut self) {
            let exact_target = format!("={}", self.session_name);
            let _ = ProcessCommand::new(&self.tmux)
                .args(["kill-session", "-t", exact_target.as_str()])
                .status();
            let _ = std::fs::remove_dir_all(&self.fixture);
        }
    }

    #[cfg(unix)]
    struct ProcessGroupGuard {
        child: ProcessChild,
    }

    #[cfg(unix)]
    impl Drop for ProcessGroupGuard {
        fn drop(&mut self) {
            let _ = terminate_owned_pty_process_group(&mut self.child);
            let _ = self.child.wait();
        }
    }

    #[test]
    fn persistent_tmux_arguments_keep_names_and_paths_as_literal_arguments() {
        assert_eq!(
            persistent_tmux_arguments(
                "project; touch /tmp/not-run",
                true,
                Some("/tmp/project with spaces")
            ),
            vec![
                "new-session",
                "-A",
                "-D",
                "-s",
                "project; touch /tmp/not-run",
                "-c",
                "/tmp/project with spaces",
            ]
        );
        assert!(!local_tmux_install_guidance().is_empty());
    }

    #[test]
    fn local_pty_terminal_type_requires_a_clear_capability() {
        assert!(terminal_type_supports_clear(std::ffi::OsStr::new(
            "xterm-256color"
        )));
        assert!(!terminal_type_supports_clear(std::ffi::OsStr::new("dumb")));
        assert!(!terminal_type_supports_clear(std::ffi::OsStr::new("")));
        assert_eq!(
            local_pty_terminal_type(Some(std::ffi::OsStr::new("screen-256color"))),
            "screen-256color"
        );
        assert_eq!(
            local_pty_terminal_type(Some(std::ffi::OsStr::new("dumb"))),
            TERM_WITH_CLEAR_CAPABILITY
        );
        assert_eq!(local_pty_terminal_type(None), TERM_WITH_CLEAR_CAPABILITY);
    }

    #[test]
    fn tmux_candidate_probe_has_deterministic_available_and_unavailable_branches() {
        let candidates = [PathBuf::from("missing-tmux"), PathBuf::from("fixture-tmux")];
        let (selected, version) = probe_tmux_candidates_with(
            candidates.clone(),
            |candidate| candidate == std::path::Path::new("fixture-tmux"),
            |_| Ok("tmux fixture".to_string()),
        )
        .expect("available fixture candidate should be probed");
        assert_eq!(selected, PathBuf::from("fixture-tmux"));
        assert_eq!(version, "tmux fixture");

        let error = probe_tmux_candidates_with(
            candidates.clone(),
            |_| false,
            |_| panic!("unavailable candidates must not be executed"),
        )
        .expect_err("unavailable fixture candidates should return guidance");
        assert!(format!("{error:#}").contains("tmux is not installed"));

        let error = probe_tmux_candidates_with(
            candidates,
            |candidate| candidate == std::path::Path::new("fixture-tmux"),
            |_| anyhow::bail!("synthetic tmux -V failure"),
        )
        .expect_err("a present but broken tmux candidate must fail");
        assert!(format!("{error:#}").contains("synthetic tmux -V failure"));
    }

    #[test]
    fn tmux_readiness_probe_is_attempt_bounded_and_injected() {
        let mut probes = 0;
        wait_for_tmux_readiness(3, Duration::ZERO, || {
            probes += 1;
            Ok(probes == 3)
        })
        .expect("third deterministic readiness probe should succeed");
        assert_eq!(probes, 3);

        let diagnostic = "readiness failed: ".to_string() + &"x".repeat(2048);
        let error =
            wait_for_tmux_readiness(2, Duration::ZERO, || anyhow::bail!(diagnostic.clone()))
                .expect_err("bounded readiness probe should time out");
        let rendered = format!("{error:#}");
        assert!(rendered.contains("after 2 probes"));
        assert!(rendered.len() < TMUX_DIAGNOSTIC_LIMIT + 160);
        assert_eq!(bounded_diagnostic("ready"), "ready");
        assert!(bounded_diagnostic(&"x".repeat(2048)).ends_with("..."));
    }

    #[cfg(unix)]
    #[test]
    fn readiness_cleanup_targets_only_the_owned_process_group() {
        use std::os::unix::process::CommandExt;

        let mut fixture_command = ProcessCommand::new("/bin/sleep");
        fixture_command.arg("30").process_group(0);
        let fixture = fixture_command
            .spawn()
            .expect("fixture process should start");
        let mut fixture = ProcessGroupGuard { child: fixture };

        let mut sentinel_command = ProcessCommand::new("/bin/sleep");
        sentinel_command.arg("30").process_group(0);
        let sentinel = sentinel_command
            .spawn()
            .expect("sentinel process should start");
        let mut sentinel = ProcessGroupGuard { child: sentinel };

        terminate_owned_pty_process_group(&mut fixture.child)
            .expect("owned fixture process group should terminate");
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline && fixture.child.try_wait().unwrap().is_none() {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(fixture.child.try_wait().unwrap().is_some());
        assert!(sentinel.child.try_wait().unwrap().is_none());
    }

    #[test]
    fn local_shells_name_termirust_so_the_tmux_startup_file_leaves_them_alone() {
        use super::{TERMINAL_PROGRAM, build_command};
        use std::ffi::OsStr;

        let request = ConnectRequest::local_shell_with_config(
            904,
            LocalShellConfig {
                program: "/bin/zsh".to_string(),
                args: Vec::new(),
                cwd: None,
            },
        );
        let built = build_command(&request, request.local_shell.as_ref().unwrap()).unwrap();
        assert_eq!(
            built.command.get_env("TERM_PROGRAM"),
            Some(OsStr::new(TERMINAL_PROGRAM))
        );
        assert!(
            !multiplex_tmux::shell_integration::WRAPPED_TERMINAL_PROGRAMS
                .contains(&TERMINAL_PROGRAM),
            "TermiRust panes are reachable without tmux and must not be wrapped"
        );
    }

    /// The app answers what a program asks about the terminal only once its pane is connected,
    /// so output that arrives ahead of `Connected` goes unanswered. On Windows the pseudo-console
    /// opens by asking for the cursor position and writes nothing more until it is told, so a
    /// shell whose first output won that race showed an empty pane for good.
    #[test]
    fn a_local_session_is_connected_before_any_of_its_output_arrives() {
        for attempt in 0..10_u64 {
            let session_id = 950 + attempt;
            let request = ConnectRequest::local_shell_with_config(
                session_id,
                LocalShellConfig {
                    program: crate::test_support::test_shell_program(),
                    args: vec!["-c".to_string(), "printf ready; sleep 5".to_string()],
                    cwd: None,
                },
            );
            let (event_tx, event_rx) = std::sync::mpsc::channel();
            let runtime = spawn_local_session(request, event_tx.into());
            let first = event_rx
                .recv_timeout(Duration::from_secs(10))
                .expect("the local session reported something");
            assert!(
                matches!(first, SshEvent::Connected { .. }),
                "attempt {attempt}: the first event was not Connected: {first:?}"
            );
            let _ = runtime.command_tx.send(SessionCommand::Disconnect);
        }
    }

    #[cfg(unix)]
    #[test]
    fn disconnect_reaps_the_spawned_local_process_group_only() {
        use std::os::unix::process::CommandExt;

        let suffix = format!(
            "{}-{}",
            std::process::id(),
            TEST_SUFFIX.fetch_add(1, Ordering::Relaxed)
        );
        let fixture = std::env::temp_dir().join(format!("termirust-disconnect-{suffix}"));
        std::fs::create_dir_all(&fixture).unwrap();
        let child_pid_file = fixture.join("child-pid");

        let mut sentinel_command = ProcessCommand::new("/bin/sleep");
        sentinel_command.arg("30").process_group(0);
        let sentinel = sentinel_command
            .spawn()
            .expect("sentinel process should start");
        let mut sentinel = ProcessGroupGuard { child: sentinel };

        let command = format!(
            "sleep 30 & child=$!; printf '%s' \"$child\" > '{}'; wait",
            child_pid_file.display()
        );
        let request = ConnectRequest::local_shell_with_config(
            903,
            LocalShellConfig {
                program: crate::test_support::test_shell_program(),
                args: vec!["-c".to_string(), command],
                cwd: Some(fixture.display().to_string()),
            },
        );
        let (event_tx, event_rx) = std::sync::mpsc::channel();
        let runtime = spawn_local_session(request, event_tx.into());
        wait_for_event(
            &event_rx,
            Instant::now() + Duration::from_secs(5),
            |event| {
                matches!(
                    event,
                    SshEvent::Connected {
                        session_id: 903,
                        ..
                    }
                )
            },
        );
        let child_pid: i32 =
            wait_for_file(&child_pid_file, Instant::now() + Duration::from_secs(5))
                .parse()
                .expect("fixture should write a child pid");

        runtime.command_tx.send(SessionCommand::Disconnect).unwrap();
        wait_for_event(
            &event_rx,
            Instant::now() + Duration::from_secs(5),
            |event| {
                matches!(
                    event,
                    SshEvent::Disconnected {
                        session_id: 903,
                        ..
                    }
                )
            },
        );

        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            let result = unsafe { libc::kill(child_pid, 0) };
            if result == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(unsafe { libc::kill(child_pid, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
        assert!(sentinel.child.try_wait().unwrap().is_none());
        std::fs::remove_dir_all(fixture).unwrap();
    }

    fn wait_for_event(
        events: &Receiver<SshEvent>,
        deadline: Instant,
        predicate: impl Fn(&SshEvent) -> bool,
    ) {
        while Instant::now() < deadline {
            if let Ok(event) = events.recv_timeout(Duration::from_millis(50))
                && predicate(&event)
            {
                return;
            }
        }
        panic!("expected local session event did not arrive before timeout");
    }

    fn wait_for_file(path: &std::path::Path, deadline: Instant) -> String {
        while Instant::now() < deadline {
            if let Ok(contents) = std::fs::read_to_string(path)
                && !contents.trim().is_empty()
            {
                return contents.trim().to_string();
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("{} was not written before timeout", path.display());
    }

    /// Types `input` until the shell writes `path`. Keys can arrive while a tmux client is
    /// still attaching and be dropped, so the command, which only rewrites the file, is resent.
    fn type_until_file(
        command_tx: &tokio::sync::mpsc::UnboundedSender<SessionCommand>,
        input: String,
        path: &std::path::Path,
    ) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            command_tx
                .send(SessionCommand::Input(input.clone().into_bytes()))
                .unwrap();
            let attempt_deadline = (Instant::now() + Duration::from_secs(1)).min(deadline);
            while Instant::now() < attempt_deadline {
                if let Ok(contents) = std::fs::read_to_string(path)
                    && !contents.trim().is_empty()
                {
                    return contents.trim().to_string();
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            assert!(
                Instant::now() < deadline,
                "{} was not written before timeout",
                path.display()
            );
        }
    }

    #[test]
    fn local_tmux_session_survives_disconnect_and_reattaches() {
        let Ok((tmux, _)) = local_tmux_probe() else {
            eprintln!("skipping local tmux integration test: tmux is unavailable");
            return;
        };
        let suffix = format!(
            "{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            TEST_SUFFIX.fetch_add(1, Ordering::Relaxed)
        );
        let session_name = format!("tr-local-test-{suffix}");
        let fixture = std::env::temp_dir().join(format!("termirust-local-tmux-{suffix}"));
        std::fs::create_dir_all(&fixture).unwrap();
        let _guard = TmuxSessionGuard {
            tmux: tmux.clone(),
            session_name: session_name.clone(),
            fixture: fixture.clone(),
        };
        let first_pid = fixture.join("first-pid");
        let second_pid = fixture.join("second-pid");
        let shell = LocalShellConfig {
            program: std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string()),
            args: Vec::new(),
            cwd: Some(fixture.display().to_string()),
        };

        let (first_tx, first_rx) = std::sync::mpsc::channel();
        let first = spawn_local_session(
            ConnectRequest::persistent_local_shell_with_config(
                901,
                shell.clone(),
                session_name.clone(),
                false,
            ),
            first_tx.into(),
        );
        wait_for_event(
            &first_rx,
            Instant::now() + Duration::from_secs(5),
            |event| {
                matches!(
                    event,
                    SshEvent::Connected {
                        session_id: 901,
                        ..
                    }
                )
            },
        );
        let original_pid = type_until_file(
            &first.command_tx,
            format!("printf '%s\\n' \"$$\" > {}\n", first_pid.display()),
            &first_pid,
        );
        first.command_tx.send(SessionCommand::Disconnect).unwrap();
        wait_for_event(
            &first_rx,
            Instant::now() + Duration::from_secs(5),
            |event| {
                matches!(
                    event,
                    SshEvent::Disconnected {
                        session_id: 901,
                        ..
                    }
                )
            },
        );
        assert!(
            ProcessCommand::new(&tmux)
                .args(["has-session", "-t", format!("={session_name}").as_str()])
                .status()
                .unwrap()
                .success()
        );

        let (second_tx, second_rx) = std::sync::mpsc::channel();
        let second = spawn_local_session(
            ConnectRequest::persistent_local_shell_with_config(
                902,
                shell,
                session_name.clone(),
                false,
            ),
            second_tx.into(),
        );
        wait_for_event(
            &second_rx,
            Instant::now() + Duration::from_secs(5),
            |event| {
                matches!(
                    event,
                    SshEvent::Connected {
                        session_id: 902,
                        ..
                    }
                )
            },
        );
        let reattached_pid = type_until_file(
            &second.command_tx,
            format!("printf '%s\\n' \"$$\" > {}\n", second_pid.display()),
            &second_pid,
        );
        assert_eq!(reattached_pid, original_pid);

        second
            .command_tx
            .send(SessionCommand::KillTmuxSession {
                session_name: session_name.clone(),
            })
            .unwrap();
        wait_for_event(
            &second_rx,
            Instant::now() + Duration::from_secs(5),
            |event| {
                matches!(
                    event,
                    SshEvent::TmuxSessionKilled {
                        session_id: 902,
                        session_name: killed,
                    } if killed == &session_name
                )
            },
        );
        assert!(
            !ProcessCommand::new(tmux)
                .args(["has-session", "-t", format!("={session_name}").as_str()])
                .status()
                .unwrap()
                .success()
        );
    }
}
