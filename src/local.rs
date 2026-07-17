use anyhow::{Context, Result, bail};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::Command as ProcessCommand;
use std::sync::mpsc::{self, TryRecvError as StdTryRecvError};
use std::thread;
use std::time::Duration;
use tokio::sync::mpsc::{self as tokio_mpsc, error::TryRecvError as TokioTryRecvError};

use crate::models::{ConnectRequest, LocalShellConfig};
use crate::ssh::{SessionCommand, SessionRuntimeHandle, SshEvent};

enum ReaderEvent {
    Data(Vec<u8>),
    Closed(String),
}

pub fn spawn_local_session(
    request: ConnectRequest,
    event_tx: std::sync::mpsc::Sender<SshEvent>,
) -> SessionRuntimeHandle {
    let (command_tx, mut command_rx) = tokio_mpsc::unbounded_channel();
    let session_id = request.session_id;
    let thread_name = format!("local-session-{session_id}");
    let fallback_tx = event_tx.clone();
    let fallback_thread_tx = fallback_tx.clone();

    let spawn_result = thread::Builder::new().name(thread_name).spawn(move || {
        let result = run_local_session(request, &mut command_rx, event_tx);
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
    command_rx: &mut tokio_mpsc::UnboundedReceiver<SessionCommand>,
    event_tx: std::sync::mpsc::Sender<SshEvent>,
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

    let command = build_command(&request, &shell)?;
    let mut child = pair
        .slave
        .spawn_command(command)
        .context("Unable to launch the local shell")?;
    drop(pair.slave);

    let mut writer = pair
        .master
        .take_writer()
        .context("Unable to attach a PTY writer")?;
    let mut reader = pair
        .master
        .try_clone_reader()
        .context("Unable to attach a PTY reader")?;
    let master = pair.master;

    let (reader_tx, reader_rx) = mpsc::channel();
    thread::Builder::new()
        .name(format!("local-session-reader-{session_id}"))
        .spawn(move || {
            let mut buffer = vec![0_u8; 65536];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => {
                        let _ =
                            reader_tx.send(ReaderEvent::Closed("Local shell closed".to_string()));
                        break;
                    }
                    Ok(bytes_read) => {
                        let _ = reader_tx.send(ReaderEvent::Data(buffer[..bytes_read].to_vec()));
                    }
                    Err(error) => {
                        let _ = reader_tx.send(ReaderEvent::Closed(format!(
                            "Unable to read from the local shell: {error}"
                        )));
                        break;
                    }
                }
            }
        })
        .context("Unable to spawn the local PTY reader")?;

    let _ = event_tx.send(SshEvent::Connected {
        session_id,
        trusted_new_host: false,
    });

    loop {
        loop {
            match command_rx.try_recv() {
                Ok(SessionCommand::Input(data)) => {
                    writer
                        .write_all(&data)
                        .context("Unable to write to the local shell")?;
                    let _ = writer.flush();
                }
                Ok(SessionCommand::Resize(size)) => {
                    master
                        .resize(PtySize {
                            rows: size.rows,
                            cols: size.cols,
                            pixel_width: size.pixel_width,
                            pixel_height: size.pixel_height,
                        })
                        .context("Unable to resize the local PTY")?;
                }
                Ok(SessionCommand::KillTmuxSession { session_name }) => {
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
                Ok(SessionCommand::Disconnect) => {
                    let _ = child.kill();
                    let _ = event_tx.send(SshEvent::Disconnected {
                        session_id,
                        message: "Local shell closed".to_string(),
                    });
                    return Ok(());
                }
                Err(TokioTryRecvError::Empty) => break,
                Err(TokioTryRecvError::Disconnected) => {
                    let _ = child.kill();
                    return Ok(());
                }
            }
        }

        loop {
            match reader_rx.try_recv() {
                Ok(ReaderEvent::Data(data)) => {
                    let _ = event_tx.send(SshEvent::Output { session_id, data });
                }
                Ok(ReaderEvent::Closed(message)) => {
                    let _ = child.try_wait();
                    let _ = event_tx.send(SshEvent::Disconnected {
                        session_id,
                        message,
                    });
                    return Ok(());
                }
                Err(StdTryRecvError::Empty) => break,
                Err(StdTryRecvError::Disconnected) => {
                    let _ = event_tx.send(SshEvent::Disconnected {
                        session_id,
                        message: "Local shell closed".to_string(),
                    });
                    return Ok(());
                }
            }
        }

        thread::sleep(Duration::from_millis(10));
    }
}

fn build_command(request: &ConnectRequest, shell: &LocalShellConfig) -> Result<CommandBuilder> {
    if request.persistent_session {
        let session_name = request
            .persistent_session_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Local tmux session name is empty"))?;
        let (tmux, _) = local_tmux_probe()?;
        let mut command = CommandBuilder::new(tmux);
        for argument in persistent_tmux_arguments(
            session_name,
            request.persistent_session_detach_others,
            shell.cwd.as_deref(),
        ) {
            command.arg(argument);
        }
        return Ok(command);
    }
    if shell.program.trim().is_empty() {
        bail!("Local shell program is empty");
    }

    let mut command = CommandBuilder::new(shell.program.clone());
    for arg in &shell.args {
        command.arg(arg);
    }
    if let Some(cwd) = shell.cwd.as_ref().filter(|cwd| !cwd.trim().is_empty()) {
        command.cwd(cwd);
    } else if let Some(home) = dirs::home_dir() {
        // Default to the user's home directory; otherwise the shell inherits
        // the directory the app was launched from.
        command.cwd(home);
    }
    Ok(command)
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
    let mut candidates = Vec::new();
    if let Some(path) = std::env::var_os("TERMIRUST_TMUX_PATH") {
        candidates.push(PathBuf::from(path));
    }
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|directory| directory.join("tmux")));
    }
    candidates.extend(
        [
            "/opt/homebrew/bin/tmux",
            "/usr/local/bin/tmux",
            "/usr/bin/tmux",
        ]
        .into_iter()
        .map(PathBuf::from),
    );
    candidates.dedup();
    let tmux = candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| {
            anyhow::anyhow!("tmux is not installed or is not available in the app's PATH")
        })?;
    let output = ProcessCommand::new(&tmux)
        .arg("-V")
        .output()
        .with_context(|| format!("Unable to run {}", tmux.display()))?;
    if !output.status.success() {
        bail!("tmux -V exited with {}", output.status);
    }
    Ok((
        tmux,
        String::from_utf8_lossy(&output.stdout).trim().to_string(),
    ))
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
        local_tmux_install_guidance, local_tmux_probe, local_tmux_version,
        persistent_tmux_arguments, spawn_local_session,
    };
    use crate::models::{ConnectRequest, LocalShellConfig};
    use crate::ssh::{SessionCommand, SshEvent};
    use std::process::Command as ProcessCommand;
    use std::sync::mpsc::Receiver;
    use std::time::{Duration, Instant};

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

    #[test]
    fn local_tmux_session_survives_disconnect_and_reattaches() {
        let Ok((tmux, _)) = local_tmux_probe() else {
            eprintln!("skipping local tmux integration test: tmux is unavailable");
            return;
        };
        let suffix = crate::ui::util::current_unix_millis();
        let session_name = format!("tr-local-test-{suffix}");
        let fixture = std::env::temp_dir().join(format!("termirust-local-tmux-{suffix}"));
        std::fs::create_dir_all(&fixture).unwrap();
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
            first_tx,
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
        first
            .command_tx
            .send(SessionCommand::Input(
                format!("printf '%s\\n' \"$$\" > {}\n", first_pid.display()).into_bytes(),
            ))
            .unwrap();
        let original_pid = wait_for_file(&first_pid, Instant::now() + Duration::from_secs(5));
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
                .args(["has-session", "-t", &session_name])
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
            second_tx,
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
        second
            .command_tx
            .send(SessionCommand::Input(
                format!("printf '%s\\n' \"$$\" > {}\n", second_pid.display()).into_bytes(),
            ))
            .unwrap();
        let reattached_pid = wait_for_file(&second_pid, Instant::now() + Duration::from_secs(5));
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
                .args(["has-session", "-t", &session_name])
                .status()
                .unwrap()
                .success()
        );
        let _ = std::fs::remove_dir_all(fixture);
    }
}
