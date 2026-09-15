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
