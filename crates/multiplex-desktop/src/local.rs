use anyhow::{Context, Result, bail};
use portable_pty::{Child, CommandBuilder, PtySize, native_pty_system};
use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::sync::mpsc;
use std::thread;
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

const TERM_WITH_CLEAR_CAPABILITY: &str = "xterm-256color";

struct BuiltLocalCommand {
    command: CommandBuilder,
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
    // Unix CLI frontends acknowledge raw-mode readiness. ConPTY must still hear Connected
    // before its initial terminal queries, which it waits on before starting the child.
    let waiting_for_cli = cfg!(unix) && crate::models::local_console_session_id(&request).is_some();
    if !waiting_for_cli {
        let _ = event_tx.send(SshEvent::Connected {
            session_id,
            trusted_new_host: false,
        });
    }

    // The reader sends output straight to the app, and a second thread forwards commands,
    // so the worker blocks until there is something to do instead of polling.
    let (worker_tx, worker_rx) = mpsc::channel();
    let reader_worker_tx = worker_tx.clone();
    let reader_event_tx = event_tx.clone();
    thread::Builder::new()
        .name(format!("local-session-reader-{session_id}"))
        .spawn(move || {
            let mut buffer = vec![0_u8; 65536];
            let mut waiting_for_cli = waiting_for_cli;
            let mut pending = Vec::new();
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => {
                        if !pending.is_empty() {
                            let _ = reader_event_tx.send(SshEvent::Output {
                                session_id,
                                data: std::mem::take(&mut pending),
                            });
                        }
                        let _ = reader_worker_tx
                            .send(WorkerEvent::ReaderClosed("Local shell closed".to_string()));
                        break;
                    }
                    Ok(bytes_read) => {
                        let data = if waiting_for_cli {
                            pending.extend_from_slice(&buffer[..bytes_read]);
                            let marker = multiplex_cli::SHELL_READY_MARKER;
                            if let Some(index) = pending
                                .windows(marker.len())
                                .position(|bytes| bytes == marker)
                            {
                                pending.drain(index..index + marker.len());
                                waiting_for_cli = false;
                                let _ = reader_event_tx.send(SshEvent::Connected {
                                    session_id,
                                    trusted_new_host: false,
                                });
                                std::mem::take(&mut pending)
                            } else {
                                if pending.len() > 65536 {
                                    let _ = reader_worker_tx.send(WorkerEvent::ReaderClosed(
                                        "CLI terminal readiness response exceeded its limit".into(),
                                    ));
                                    break;
                                }
                                continue;
                            }
                        } else {
                            buffer[..bytes_read].to_vec()
                        };
                        if !data.is_empty() {
                            let _ = reader_event_tx.send(SshEvent::Output { session_id, data });
                        }
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
            WorkerEvent::Command(SessionCommand::KillTmuxSession { .. }) => {}
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
    if shell.program.trim().is_empty() {
        bail!("Local shell program is empty");
    }

    let mut command = if let Some(id) = crate::models::local_console_session_id(request) {
        let current = std::env::current_exe().context("Unable to locate Multiplex")?;
        #[cfg(test)]
        let current = if current
            .parent()
            .and_then(std::path::Path::file_name)
            .is_some_and(|name| name == "deps")
        {
            current
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join(format!("multiplex{}", std::env::consts::EXE_SUFFIX))
        } else {
            current
        };
        let directory = current
            .parent()
            .context("Multiplex has no executable directory")?;
        let cli = directory.join(format!("multiplex-cli{}", std::env::consts::EXE_SUFFIX));
        let host = directory.join(format!(
            "multiplex-session-host{}",
            std::env::consts::EXE_SUFFIX
        ));
        let mut command = if cli.is_file() && host.is_file() {
            let mut command = CommandBuilder::new(cli);
            command.arg("shell");
            command
        } else {
            let mut command = CommandBuilder::new(current);
            command.arg("--cli-shell");
            command
        };
        command.arg("--raw");
        command.arg("--session-id");
        command.arg(id.to_string());
        command.arg("--");
        command.arg(&shell.program);
        command.env_remove(multiplex_cli::SHELL_SESSION_ENV);
        command
    } else {
        CommandBuilder::new(shell.program.clone())
    };
    for arg in &shell.args {
        command.arg(arg);
    }
    for (key, value) in &request.environment {
        command.env(key, value);
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
    Ok(BuiltLocalCommand { command })
}

/// The value Multiplex's shells see in `TERM_PROGRAM`.
pub const TERMINAL_PROGRAM: &str = "Multiplex";

/// Names this app as the terminal, replacing whatever `TERM_PROGRAM` it was launched with.
/// Otherwise a shell started from Zed or iTerm2 would believe it runs there, and the
/// startup file that opens new terminals in tmux would wrap Multiplex's own tabs.
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
        TERM_WITH_CLEAR_CAPABILITY, local_pty_terminal_type, spawn_local_session,
        terminal_type_supports_clear, terminate_owned_pty_process_group,
    };
    use crate::models::{ConnectRequest, LocalShellConfig};
    use crate::ssh::{SessionCommand, SshEvent};
    use std::process::{Child as ProcessChild, Command as ProcessCommand};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc::Receiver;
    use std::time::{Duration, Instant};

    static TEST_SUFFIX: AtomicU64 = AtomicU64::new(1);

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
            "Multiplex panes are reachable without tmux and must not be wrapped"
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

    #[track_caller]
    fn wait_for_event(
        events: &Receiver<SshEvent>,
        deadline: Instant,
        predicate: impl Fn(&SshEvent) -> bool,
    ) {
        while Instant::now() < deadline {
            if let Ok(event) = events.recv_timeout(Duration::from_millis(50)) {
                if predicate(&event) {
                    return;
                }
                if let SshEvent::Output { data, .. } = &event {
                    // A failed CLI startup exposes its own bounded error before readiness.
                    if data.starts_with(b"error[") {
                        panic!("CLI startup failed: {}", String::from_utf8_lossy(data));
                    }
                }
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

    #[cfg(unix)]
    #[test]
    fn cli_console_survives_native_disconnect_and_restores_the_same_shell() {
        let fixture = std::env::temp_dir().join(format!(
            "multiplex-cli-restore-{}",
            multiplex_domain::HostedSessionId::new()
        ));
        std::fs::create_dir_all(&fixture).unwrap();
        let mut request = ConnectRequest::local_shell_with_config(
            905,
            LocalShellConfig {
                program: "/bin/sh".into(),
                args: Vec::new(),
                cwd: Some(fixture.display().to_string()),
            },
        );
        crate::models::prepare_terminal_request(&mut request);
        request
            .environment
            .push(("MULTIPLEX_CONFIG_DIR".into(), fixture.display().to_string()));
        let id = crate::models::local_console_session_id(&request).unwrap();
        let session_dir = fixture.join("console-sessions").join(id.to_string());
        let (tx, rx) = std::sync::mpsc::channel();
        let first = spawn_local_session(request.clone(), tx.into());
        wait_for_event(&rx, Instant::now() + Duration::from_secs(10), |event| {
            matches!(event, SshEvent::Connected { .. })
        });
        first
            .command_tx
            .send(SessionCommand::Input(b"echo $$ > first-pid\r".to_vec()))
            .unwrap();
        let first_pid = wait_for_file(
            &fixture.join("first-pid"),
            Instant::now() + Duration::from_secs(10),
        );
        let first_metadata = multiplex_store::read_host_metadata(&session_dir).unwrap();
        first.command_tx.send(SessionCommand::Disconnect).unwrap();
        wait_for_event(&rx, Instant::now() + Duration::from_secs(10), |event| {
            matches!(event, SshEvent::Disconnected { .. })
        });
        let runtime_parent = crate::controller_runtime_parent(&fixture);
        assert!(
            multiplex_store::live_console_sessions(
                &fixture.join("console-sessions"),
                &runtime_parent
            )
            .iter()
            .any(|session| session.record.session_id == id)
        );
        let (tx, rx) = std::sync::mpsc::channel();
        let restored = spawn_local_session(request, tx.into());
        wait_for_event(&rx, Instant::now() + Duration::from_secs(10), |event| {
            matches!(event, SshEvent::Connected { .. })
        });
        restored
            .command_tx
            .send(SessionCommand::Input(b"echo $$ > restored-pid\r".to_vec()))
            .unwrap();
        assert_eq!(
            wait_for_file(
                &fixture.join("restored-pid"),
                Instant::now() + Duration::from_secs(10)
            ),
            first_pid
        );
        assert_eq!(
            multiplex_store::read_host_metadata(&session_dir)
                .unwrap()
                .host_instance_id,
            first_metadata.host_instance_id
        );
        assert_eq!(
            std::fs::read_dir(fixture.join("console-sessions"))
                .unwrap()
                .count(),
            1
        );
        restored
            .command_tx
            .send(SessionCommand::Input(b"exit\r".to_vec()))
            .unwrap();
        wait_for_event(&rx, Instant::now() + Duration::from_secs(10), |event| {
            matches!(event, SshEvent::Disconnected { .. })
        });
        let _ = std::fs::remove_dir_all(fixture);
    }

    #[test]
    fn stale_tmux_flags_never_launch_tmux() {
        let request = ConnectRequest::persistent_local_shell_with_config(
            901,
            LocalShellConfig {
                program: "/bin/sh".into(),
                args: vec!["-l".into()],
                cwd: None,
            },
            "tr-local-old".into(),
            true,
        );
        let command = super::build_command(&request, request.local_shell.as_ref().unwrap())
            .unwrap()
            .command;
        assert_eq!(
            command.get_argv(),
            &[
                std::ffi::OsString::from("/bin/sh"),
                std::ffi::OsString::from("-l")
            ]
        );
    }

    #[test]
    fn cli_terminal_keeps_the_selected_shell_arguments_and_folder() {
        let mut request = ConnectRequest::local_shell_with_config(
            901,
            LocalShellConfig {
                program: "/bin/sh".into(),
                args: vec!["-l".into()],
                cwd: Some("/tmp/project with spaces".into()),
            },
        );
        crate::models::prepare_terminal_request(&mut request);
        request
            .environment
            .push(("MULTIPLEX_CONFIG_DIR".into(), "/tmp/isolated-config".into()));
        let command = super::build_command(&request, request.local_shell.as_ref().unwrap())
            .unwrap()
            .command;
        let args = command.get_argv();
        assert!(args.iter().any(|arg| arg == "--session-id"));
        assert_eq!(
            &args[args.len() - 3..],
            &[
                std::ffi::OsString::from("--"),
                std::ffi::OsString::from("/bin/sh"),
                std::ffi::OsString::from("-l")
            ]
        );
        assert_eq!(
            command.get_cwd(),
            Some(&std::ffi::OsString::from("/tmp/project with spaces"))
        );
        assert_eq!(
            command.get_env("MULTIPLEX_CONFIG_DIR"),
            Some(std::ffi::OsStr::new("/tmp/isolated-config"))
        );
        assert_eq!(command.get_env(multiplex_cli::SHELL_SESSION_ENV), None);
    }
}
