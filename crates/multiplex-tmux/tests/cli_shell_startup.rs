//! Run generated hooks in actual shells with a PTY, without touching the user's dotfiles.
#![cfg(unix)]

use std::fs;
use std::io::Read as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use multiplex_tmux::cli_shell_integration::CliShellIntegration;
use multiplex_tmux::shell_integration::Shell;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn run_hook(
    shell: Shell,
    login: bool,
    environment: &[(&str, &str)],
    host_present: bool,
) -> Option<Vec<String>> {
    let executable = match shell {
        Shell::Zsh => "/bin/zsh",
        Shell::Bash => "/bin/bash",
    };
    if !Path::new(executable).is_file() {
        return None;
    }
    let home = tempfile::tempdir().unwrap();
    let binaries = home.path().join("cli's folder");
    fs::create_dir(&binaries).unwrap();
    let launcher = binaries.join("multiplex-cli");
    let log = home.path().join("called");
    fs::write(
        &launcher,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$MULTIPLEX_NO_WRAP\" \"$@\" > {}\n",
            quote(&log.to_string_lossy())
        ),
    )
    .unwrap();
    fs::set_permissions(&launcher, fs::Permissions::from_mode(0o700)).unwrap();
    if host_present {
        let host = binaries.join("multiplex-session-host");
        fs::write(&host, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(host, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let integration = CliShellIntegration::new(home.path(), &launcher);
    let script = home.path().join("hook");
    fs::write(&script, integration.startup_block(shell)).unwrap();
    let pty = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut command = CommandBuilder::new(executable);
    command.args([
        if login { "-ilc" } else { "-ic" },
        &format!(". {}; exit 0", quote(&script.to_string_lossy())),
    ]);
    command.env("HOME", home.path());
    command.env("ZDOTDIR", home.path());
    command.env("SHELL", executable);
    for name in [
        "MULTIPLEX_SHELL_SESSION",
        "MULTIPLEX_NO_WRAP",
        "TERMIRUST_NO_WRAP",
        "TMUX",
        "SSH_CONNECTION",
        "TERM_PROGRAM",
        "BASH_ENV",
        "ENV",
    ] {
        command.env_remove(name);
    }
    for &(name, value) in environment {
        command.env(name, value);
    }
    let mut child = pty.slave.spawn_command(command).unwrap();
    let mut reader = pty.master.try_clone_reader().unwrap();
    drop(pty.slave);
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut output = Vec::new();
        let _ = reader.read_to_end(&mut output);
        let _ = sender.send(output);
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("shell hook did not finish");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = receiver.recv_timeout(Duration::from_secs(1));
    Some(
        fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect(),
    )
}

#[test]
fn terminal_and_editor_shells_route_to_cli_with_the_original_login_mode() {
    for shell in Shell::ALL {
        for login in [false, true] {
            for program in ["Apple_Terminal", "vscode", "zed", ""] {
                let Some(arguments) = run_hook(shell, login, &[("TERM_PROGRAM", program)], true)
                else {
                    continue;
                };
                assert_eq!(
                    arguments,
                    [
                        "1",
                        "shell",
                        "--",
                        if shell == Shell::Zsh {
                            "/bin/zsh"
                        } else {
                            "/bin/bash"
                        },
                        if login { "-il" } else { "-i" }
                    ]
                );
            }
        }
    }
}

#[test]
fn hosted_shells_tmux_ssh_opted_out_and_app_terminals_are_not_wrapped() {
    for shell in Shell::ALL {
        for environment in [
            ("MULTIPLEX_SHELL_SESSION", "already-hosted"),
            ("MULTIPLEX_NO_WRAP", "1"),
            ("TERMIRUST_NO_WRAP", "1"),
            ("TMUX", "/tmp/existing"),
            ("SSH_CONNECTION", "remote"),
            ("TERM_PROGRAM", "Multiplex"),
        ] {
            if let Some(arguments) = run_hook(shell, false, &[environment], true) {
                assert!(arguments.is_empty(), "{environment:?}");
            }
        }
        if let Some(arguments) = run_hook(shell, false, &[], false) {
            assert!(
                arguments.is_empty(),
                "missing Session Host must leave the shell usable"
            );
        }
    }
}

#[test]
fn scripts_and_posix_profile_readers_are_left_alone() {
    let home = tempfile::tempdir().unwrap();
    let launcher = home.path().join("multiplex-cli");
    let log = home.path().join("called");
    fs::write(
        &launcher,
        format!(
            "#!/bin/sh\nprintf wrapped > {}\n",
            quote(&log.to_string_lossy())
        ),
    )
    .unwrap();
    fs::set_permissions(&launcher, fs::Permissions::from_mode(0o700)).unwrap();
    let host = home.path().join("multiplex-session-host");
    fs::write(&host, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(host, fs::Permissions::from_mode(0o700)).unwrap();
    let integration = CliShellIntegration::new(home.path(), launcher);
    let script = home.path().join("hook");
    for shell in Shell::ALL {
        fs::write(
            &script,
            format!("{}printf 'plain'\n", integration.startup_block(shell)),
        )
        .unwrap();
        for program in ["/bin/sh", "/bin/bash", "/bin/zsh"] {
            if !Path::new(program).is_file() {
                continue;
            }
            let output = std::process::Command::new(program)
                .arg(&script)
                .env("HOME", home.path())
                .env("ZDOTDIR", home.path())
                .output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(output.stdout, b"plain");
            assert!(!log.exists(), "scripts must not call the available CLI");
            assert!(
                output.stderr.is_empty(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        // An interactive shell with redirected stdin/stdout is also left alone.
        let executable = if shell == Shell::Zsh {
            "/bin/zsh"
        } else {
            "/bin/bash"
        };
        if Path::new(executable).is_file() {
            let output = std::process::Command::new(executable)
                .arg("-i")
                .arg(&script)
                .env("HOME", home.path())
                .env("ZDOTDIR", home.path())
                .output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(output.stdout, b"plain");
            assert!(!log.exists(), "interactive shells without a PTY stay plain");
        }
    }
}
