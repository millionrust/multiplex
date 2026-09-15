#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use termirust_tmux::{Tmux, TmuxError};

/// Starts sessions on a private server and kills that server on drop, so a test never
/// touches the developer's own tmux.
struct IsolatedServer {
    tmux: Tmux,
    _sockets: tempfile::TempDir,
}

impl IsolatedServer {
    fn start() -> Option<Self> {
        let Ok(tmux) = Tmux::discover() else {
            eprintln!("skipping tmux integration test: tmux is unavailable");
            return None;
        };
        // tmux socket paths are limited to about 100 bytes, so stay out of long temp dirs.
        let sockets = tempfile::Builder::new()
            .prefix("tr-tmux-")
            .tempdir_in("/tmp")
            .unwrap();
        Some(Self {
            tmux: tmux.with_socket_directory(sockets.path()),
            _sockets: sockets,
        })
    }

    fn run(&self, arguments: &[&str]) {
        let output = self.tmux.command().args(arguments).output().unwrap();
        assert!(output.status.success(), "tmux {arguments:?}: {output:?}");
    }
}

impl Drop for IsolatedServer {
    fn drop(&mut self) {
        let _ = self.tmux.command().arg("kill-server").output();
    }
}

fn fake_tmux(directory: &Path, body: &str) -> PathBuf {
    let path = directory.join("tmux");
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}

#[test]
fn no_server_is_an_empty_listing() {
    let Some(server) = IsolatedServer::start() else {
        return;
    };
    let listing = server.tmux.list_sessions().unwrap();
    assert!(listing.sessions.is_empty());
}

#[test]
fn real_server_lists_sessions_with_hostile_names() {
    let Some(server) = IsolatedServer::start() else {
        return;
    };
    for name in ["plain", "a:b.c | pipe", "émoji 🚀 spaced"] {
        server.run(&["new-session", "-d", "-s", name, "/bin/sh"]);
    }
    let listing = server.tmux.list_sessions().unwrap();
    assert_eq!(listing.skipped_lines, 0);
    let mut names = listing
        .sessions
        .iter()
        .map(|session| session.name.clone())
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(names, ["a:b.c | pipe", "plain", "émoji 🚀 spaced"]);
    for session in &listing.sessions {
        assert!(session.id().starts_with('$'));
        assert!(session.created_unix_seconds > 0);
        assert_eq!(session.windows, 1);
        assert_eq!(session.attached_clients, 0);
        assert!(!session.current_command.is_empty());
    }
    // The id is an unambiguous target even when the name is not.
    let hostile = listing
        .sessions
        .iter()
        .find(|session| session.name == "a:b.c | pipe")
        .unwrap();
    server.run(&["has-session", "-t", hostile.session_target()]);
}

#[test]
fn listing_reflects_sessions_ending() {
    let Some(server) = IsolatedServer::start() else {
        return;
    };
    server.run(&["new-session", "-d", "-s", "short-lived", "/bin/sh"]);
    server.run(&["new-session", "-d", "-s", "kept", "/bin/sh"]);
    let listing = server.tmux.list_sessions().unwrap();
    let short = listing
        .sessions
        .iter()
        .find(|session| session.name == "short-lived")
        .unwrap();
    server.run(&["kill-session", "-t", short.session_target()]);
    let names = server
        .tmux
        .list_sessions()
        .unwrap()
        .sessions
        .into_iter()
        .map(|session| session.name)
        .collect::<Vec<_>>();
    assert_eq!(names, ["kept"]);
}

#[test]
fn canonical_executable_resolves_symlinks() {
    let fixture = tempfile::tempdir().unwrap();
    let real = fake_tmux(fixture.path(), "echo 'tmux 3.7c'");
    let link = fixture.path().join("linked-tmux");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let tmux = Tmux::at(&link).unwrap();
    assert_eq!(tmux.executable(), link);
    assert_eq!(tmux.version(), "tmux 3.7c");
    assert_eq!(
        tmux.canonical_executable().unwrap(),
        fs::canonicalize(&real).unwrap()
    );
}

#[test]
fn fake_tmux_listing_and_failures_are_typed() {
    let fixture = tempfile::tempdir().unwrap();
    let listing = fake_tmux(
        fixture.path(),
        r#"if [ "$1" = "-V" ]; then echo 'tmux 3.4'; exit 0; fi
printf '$0\t100\t1\t1\tfrom fake\tzsh\n$1\t101\t0\t2\tsecond\tvim\n'"#,
    );
    let sessions = Tmux::at(&listing)
        .unwrap()
        .list_sessions()
        .unwrap()
        .sessions;
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].name, "from fake");
    assert_eq!(sessions[1].windows, 2);

    let other = tempfile::tempdir().unwrap();
    let no_server = fake_tmux(
        other.path(),
        r#"if [ "$1" = "-V" ]; then echo 'tmux 3.4'; exit 0; fi
echo 'no server running on /tmp/tmux-0/default' >&2; exit 1"#,
    );
    assert!(
        Tmux::at(&no_server)
            .unwrap()
            .list_sessions()
            .unwrap()
            .sessions
            .is_empty()
    );

    let third = tempfile::tempdir().unwrap();
    let broken = fake_tmux(
        third.path(),
        r#"if [ "$1" = "-V" ]; then echo 'tmux 3.4'; exit 0; fi
echo 'server exploded' >&2; exit 2"#,
    );
    assert_eq!(
        Tmux::at(&broken).unwrap().list_sessions().unwrap_err(),
        TmuxError::CommandFailed {
            status: Some(2),
            diagnostic: "server exploded".to_owned(),
        }
    );

    let fourth = tempfile::tempdir().unwrap();
    let no_version = fake_tmux(fourth.path(), "exit 3");
    assert!(matches!(
        Tmux::at(&no_version).unwrap_err(),
        TmuxError::VersionProbe(message) if message.contains("status 3")
    ));
}

#[test]
fn hung_tmux_is_killed_at_the_deadline() {
    let fixture = tempfile::tempdir().unwrap();
    let hung = fake_tmux(
        fixture.path(),
        r#"if [ "$1" = "-V" ]; then echo 'tmux 3.4'; exit 0; fi
exec sleep 30"#,
    );
    let started = std::time::Instant::now();
    assert_eq!(
        Tmux::at(&hung).unwrap().list_sessions().unwrap_err(),
        TmuxError::TimedOut
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
}

#[test]
fn oversized_output_is_rejected_without_blocking() {
    let fixture = tempfile::tempdir().unwrap();
    let chatty = fake_tmux(
        fixture.path(),
        r#"if [ "$1" = "-V" ]; then echo 'tmux 3.4'; exit 0; fi
head -c 2000000 /dev/zero | tr '\0' 'x'"#,
    );
    assert_eq!(
        Tmux::at(&chatty).unwrap().list_sessions().unwrap_err(),
        TmuxError::OutputTooLarge
    );
}

#[test]
fn missing_binary_is_unavailable() {
    assert_eq!(
        Tmux::at("/definitely/not/tmux").unwrap_err(),
        TmuxError::Unavailable
    );
}

#[test]
fn verification_proves_listing_and_leaves_other_sessions_alone() {
    let Some(server) = IsolatedServer::start() else {
        return;
    };
    server.run(&["new-session", "-d", "-s", "users-work", "/bin/sh"]);
    server.tmux.verify_listing().unwrap();
    let names = server
        .tmux
        .list_sessions()
        .unwrap()
        .sessions
        .into_iter()
        .map(|session| session.name)
        .collect::<Vec<_>>();
    assert_eq!(names, ["users-work"], "only the throwaway session is ended");
}

#[test]
fn verification_reports_old_and_broken_tmux() {
    let fixture = tempfile::tempdir().unwrap();
    let old = fake_tmux(fixture.path(), "echo 'tmux 3.1c'");
    assert_eq!(
        Tmux::at(&old).unwrap().verify_listing(),
        Err(termirust_tmux::VerificationError::TooOld)
    );
    let other = tempfile::tempdir().unwrap();
    let silent = fake_tmux(
        other.path(),
        r#"if [ "$1" = "-V" ]; then echo 'tmux 3.4'; exit 0; fi
if [ "$1" = "new-session" ]; then echo '$9'; exit 0; fi
exit 0"#,
    );
    assert_eq!(
        Tmux::at(&silent).unwrap().verify_listing(),
        Err(termirust_tmux::VerificationError::NotListed)
    );
}

/// Runs `shell -i` the way a terminal app would, inside an outer tmux pane standing in for
/// the app's window, and returns the names of the sessions the server ends up with.
fn run_wrapped_shell(
    server: &IsolatedServer,
    home: &Path,
    shell: &str,
    extra_environment: &[&str],
) -> Vec<String> {
    let ready = home.join(format!("ready-{}", extra_environment.len()));
    let outer = format!("outer-{}", extra_environment.len());
    let mut command = vec![
        "env".to_owned(),
        "-u".to_owned(),
        "TMUX".to_owned(),
        format!("HOME={}", home.display()),
    ];
    command.extend(extra_environment.iter().map(|value| (*value).to_owned()));
    command.extend([shell.to_owned(), "-i".to_owned()]);
    let mut arguments = vec!["new-session", "-d", "-s", &outer, "-x", "100", "-y", "30"];
    let command_refs = command.iter().map(String::as_str).collect::<Vec<_>>();
    arguments.extend(command_refs);
    server.run(&arguments);
    // A command typed at the prompt runs only after the startup file has finished.
    server.run(&[
        "send-keys",
        "-t",
        &format!("={outer}:"),
        "-l",
        &format!("touch '{}'\n", ready.display()),
    ]);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let names = server
            .tmux
            .list_sessions()
            .unwrap()
            .sessions
            .into_iter()
            .map(|session| session.name)
            .collect::<Vec<_>>();
        let wrapped = names.iter().any(|name| name.starts_with("termirust-"));
        if ready.exists() || wrapped {
            // Either the plain shell ran the command, or the wrapper started tmux; give the
            // other outcome no chance to race by re-reading once more.
            return server
                .tmux
                .list_sessions()
                .unwrap()
                .sessions
                .into_iter()
                .map(|session| session.name)
                .collect();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{shell} neither started tmux nor reached its prompt"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[test]
fn shell_integration_starts_new_terminal_app_shells_inside_tmux() {
    use termirust_tmux::shell_integration::{Shell, ShellIntegration};

    for (shell, kind) in [("/bin/zsh", Shell::Zsh), ("/bin/bash", Shell::Bash)] {
        if !Path::new(shell).is_file() {
            eprintln!("skipping {shell}: not installed");
            continue;
        }
        let Some(server) = IsolatedServer::start() else {
            return;
        };
        let home = tempfile::tempdir().unwrap();
        let home_path = fs::canonicalize(home.path()).unwrap();
        let workspace = home_path.join("my project");
        fs::create_dir(&workspace).unwrap();
        ShellIntegration::new(&home_path, server.tmux.canonical_executable().unwrap())
            .plan_enable(&[kind])
            .unwrap()
            .apply()
            .unwrap();
        let socket = format!(
            "TMUX_TMPDIR={}",
            server
                .tmux
                .client_environment()
                .into_iter()
                .find(|(name, _)| name == "TMUX_TMPDIR")
                .unwrap()
                .1
        );

        let names = run_wrapped_shell(
            &server,
            &home_path,
            shell,
            &[&socket, "TERM_PROGRAM=Apple_Terminal"],
        );
        let wrapped = names
            .iter()
            .filter(|name| name.starts_with("termirust-"))
            .count();
        assert_eq!(
            wrapped, 1,
            "{shell}: a Terminal.app shell starts in tmux: {names:?}"
        );

        let names = run_wrapped_shell(
            &server,
            &home_path,
            shell,
            &[
                &socket,
                "TERM_PROGRAM=Apple_Terminal",
                "TERMIRUST_NO_WRAP=1",
                "X=1",
            ],
        );
        assert_eq!(
            names
                .iter()
                .filter(|name| name.starts_with("termirust-"))
                .count(),
            1,
            "{shell}: TERMIRUST_NO_WRAP keeps a shell out of tmux: {names:?}"
        );

        let names = run_wrapped_shell(
            &server,
            &home_path,
            shell,
            &[
                &socket,
                "TERM_PROGRAM=UnlistedTerminal",
                "X=1",
                "Y=1",
                "Z=1",
            ],
        );
        assert_eq!(
            names
                .iter()
                .filter(|name| name.starts_with("termirust-"))
                .count(),
            1,
            "{shell}: unlisted terminal apps are left alone: {names:?}"
        );
    }
}

#[test]
fn the_setup_hides_wrapped_status_bars_and_keeps_native_scrollback_then_undoes_both() {
    let Some(server) = IsolatedServer::start() else {
        return;
    };
    server.run(&["new-session", "-d", "-s", "termirust-terminal-4242"]);
    server.run(&["new-session", "-d", "-s", "work"]);
    let status = |session: &str| {
        let output = server
            .tmux
            .command()
            .args(["show-options", "-v", "-t", session, "status"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    };

    let overrides = || {
        let output = server
            .tmux
            .command()
            .args(["show-options", "-s", "terminal-overrides"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).into_owned()
    };

    assert_eq!(
        server.tmux.apply_wrapped_session_appearance(true).unwrap(),
        1
    );
    assert_eq!(status("termirust-terminal-4242"), "off");
    assert_eq!(status("work"), "", "other sessions keep the global setting");
    assert!(overrides().contains(&format!(
        "{} {}",
        termirust_tmux::scrollback_override_target(),
        termirust_tmux::SCROLLBACK_OVERRIDE
    )));
    // Applying again leaves a single override.
    server.tmux.apply_wrapped_session_appearance(true).unwrap();
    assert_eq!(overrides().matches("smcup@").count(), 1);

    assert_eq!(
        server.tmux.apply_wrapped_session_appearance(false).unwrap(),
        1
    );
    assert_eq!(status("termirust-terminal-4242"), "");
    assert!(!overrides().contains("smcup@"));
}
