use anyhow::{Context, Result, bail};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, TryRecvError as StdTryRecvError};
use std::thread;
use std::time::Duration;
use tokio::sync::mpsc::{self as tokio_mpsc, error::TryRecvError as TokioTryRecvError};

use crate::models::{ConnectRequest, LocalShellConfig};
use crate::ssh::{SessionCommand, SessionRuntimeHandle, SshEvent};
use crate::ui::shell::local_tmux_wrapper_script;

enum ReaderEvent {
    Data(Vec<u8>),
    Closed(String),
}

pub fn spawn_local_session(
    request: ConnectRequest,
    event_tx: std::sync::mpsc::Sender<SshEvent>,
    persistent_terminal_sessions: bool,
) -> SessionRuntimeHandle {
    let (command_tx, mut command_rx) = tokio_mpsc::unbounded_channel();
    let session_id = request.session_id;
    let thread_name = format!("local-session-{session_id}");
    let fallback_tx = event_tx.clone();
    let fallback_thread_tx = fallback_tx.clone();

    let spawn_result = thread::Builder::new().name(thread_name).spawn(move || {
        let result = run_local_session(
            request,
            &mut command_rx,
            event_tx,
            persistent_terminal_sessions,
        );
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
    persistent_terminal_sessions: bool,
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

    let command = build_command(&request, &shell, persistent_terminal_sessions)?;
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

fn build_command(
    request: &ConnectRequest,
    shell: &LocalShellConfig,
    persistent_terminal_sessions: bool,
) -> Result<CommandBuilder> {
    if shell.program.trim().is_empty() {
        bail!("Local shell program is empty");
    }

    let mut command = if persistent_terminal_sessions {
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-lc");
        command.arg(local_tmux_wrapper_script(
            request,
            &shell.program,
            &shell.args,
            bundled_tmux_path().as_ref().and_then(|path| path.to_str()),
        ));
        command
    } else {
        let mut command = CommandBuilder::new(shell.program.clone());
        for arg in &shell.args {
            command.arg(arg);
        }
        command
    };

    if let Some(cwd) = shell.cwd.as_ref().filter(|cwd| !cwd.trim().is_empty()) {
        command.cwd(cwd);
    } else if let Some(home) = dirs::home_dir() {
        // Default to the user's home directory; otherwise the shell inherits
        // the directory the app was launched from.
        command.cwd(home);
    }

    Ok(command)
}

fn bundled_tmux_path() -> Option<PathBuf> {
    std::env::var_os("TERMIRUST_TMUX_PATH")
        .map(PathBuf::from)
        .filter(|path| executable_file(path))
        .or_else(find_bundled_tmux_path)
}

fn find_bundled_tmux_path() -> Option<PathBuf> {
    let mut candidates = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            candidates.extend(tmux_candidates_from_exe_dir(exe_dir));
        }
    }

    candidates.extend(tmux_candidates_from_dir(Path::new(env!(
        "CARGO_MANIFEST_DIR"
    ))));

    candidates.into_iter().find(|path| executable_file(path))
}

fn tmux_candidates_from_exe_dir(exe_dir: &Path) -> Vec<PathBuf> {
    let mut candidates = tmux_candidates_from_dir(exe_dir);
    if let Some(contents_dir) = exe_dir.parent() {
        candidates.extend(tmux_candidates_from_dir(&contents_dir.join("Resources")));
    }
    candidates
}

fn tmux_candidates_from_dir(base: &Path) -> Vec<PathBuf> {
    bundled_tmux_relative_paths()
        .iter()
        .map(|relative| base.join(relative))
        .collect()
}

fn bundled_tmux_relative_paths() -> &'static [&'static str] {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        &[
            "bin/tmux",
            "bin/macos/tmux",
            "bin/macos/aarch64/tmux",
            "assets/bin/macos/aarch64/tmux",
        ]
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        &[
            "bin/tmux",
            "bin/macos/tmux",
            "bin/macos/x86_64/tmux",
            "assets/bin/macos/x86_64/tmux",
        ]
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        &[
            "bin/tmux",
            "bin/linux/tmux",
            "bin/linux/x86_64/tmux",
            "assets/bin/linux/x86_64/tmux",
        ]
    }
    #[cfg(not(any(
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "x86_64")
    )))]
    {
        &["bin/tmux", "assets/bin/tmux"]
    }
}

fn executable_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn default_pty_size() -> PtySize {
    PtySize {
        rows: 48,
        cols: 160,
        pixel_width: 0,
        pixel_height: 0,
    }
}
