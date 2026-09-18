#![cfg(unix)]

use std::collections::BTreeMap;
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

    fn output(&self, arguments: &[&str]) -> String {
        let output = self.tmux.command().args(arguments).output().unwrap();
        assert!(output.status.success(), "tmux {arguments:?}: {output:?}");
        String::from_utf8(output.stdout)
            .unwrap()
            .trim_end_matches('\n')
            .to_owned()
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

/// Reads the version of a tmux that was just written, waiting out "text file busy". These tests
/// run in parallel, and Linux refuses to run a program another process still holds open for
/// writing: a test that forks while this one is being written holds that handle until it runs its
/// own program. Only the version probe is retried, because these fakes record what they are asked
/// to do and running one again would be recorded too.
fn tmux_at(path: &Path) -> Result<Tmux, TmuxError> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let result = Tmux::at(path);
        let still_busy = result
            .as_ref()
            .err()
            .is_some_and(|error| error.to_string().contains("file busy"));
        if !still_busy || std::time::Instant::now() >= deadline {
            return result;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
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
    // tmux 3.2 stores `:` and `.` in a new session's name as `_`, so expect the names tmux
    // reports it created.
    let mut expected = ["plain", "a:b.c | pipe", "émoji 🚀 spaced"].map(|name| {
        server.output(&[
            "new-session",
            "-d",
            "-P",
            "-F",
            "#{session_name}",
            "-s",
            name,
            "/bin/sh",
        ])
    });
    let hostile_name = expected[1].clone();
    expected.sort();
    let listing = server.tmux.list_sessions().unwrap();
    assert_eq!(listing.skipped_lines, 0);
    let mut names = listing
        .sessions
        .iter()
        .map(|session| session.name.clone())
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(names, expected);
    assert!(hostile_name.contains(" | "));
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
        .find(|session| session.name == hostile_name)
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
    let tmux = tmux_at(&link).unwrap();
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
    let sessions = tmux_at(&listing).unwrap().list_sessions().unwrap().sessions;
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
        tmux_at(&no_server)
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
        tmux_at(&broken).unwrap().list_sessions().unwrap_err(),
        TmuxError::CommandFailed {
            status: Some(2),
            diagnostic: "server exploded".to_owned(),
        }
    );

    let fourth = tempfile::tempdir().unwrap();
    let no_version = fake_tmux(fourth.path(), "exit 3");
    assert!(matches!(
        tmux_at(&no_version).unwrap_err(),
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
        tmux_at(&hung).unwrap().list_sessions().unwrap_err(),
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
        tmux_at(&chatty).unwrap().list_sessions().unwrap_err(),
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
        tmux_at(&old).unwrap().verify_listing(),
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
        tmux_at(&silent).unwrap().verify_listing(),
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
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
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
fn the_setup_hides_wrapped_status_bars_and_clears_the_old_scrollback_override() {
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

    // An earlier version set this override; it kept output out of reach of scrolling.
    server.run(&[
        "set-option",
        "-s",
        termirust_tmux::LEGACY_SCROLLBACK_OVERRIDE_TARGET,
        "*:smcup@:rmcup@",
    ]);
    let output = |arguments: &[&str]| {
        let output = server.tmux.command().args(arguments).output().unwrap();
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    };
    let wheel_binding = || {
        output(&["list-keys", "-T", "copy-mode"])
            .lines()
            .find(|line| line.contains("WheelUpPane"))
            .unwrap_or_default()
            .to_owned()
    };

    assert_eq!(
        server.tmux.apply_wrapped_session_appearance(true).unwrap(),
        1
    );
    assert_eq!(status("termirust-terminal-4242"), "off");
    assert_eq!(status("work"), "", "other sessions keep the global setting");
    assert_eq!(
        output(&[
            "show-options",
            "-v",
            "-t",
            "termirust-terminal-4242",
            "mouse"
        ]),
        "on"
    );
    assert_eq!(output(&["show-options", "-v", "-t", "work", "mouse"]), "");
    assert_eq!(
        output(&[
            "show-options",
            "-w",
            "-v",
            "-t",
            "termirust-terminal-4242",
            "mode-style"
        ]),
        termirust_tmux::appearance::SELECTION_STYLE
    );
    assert!(wheel_binding().contains("#{m:termirust-*,#{session_name}}"));
    assert!(
        !overrides().contains("smcup@"),
        "applying clears the override"
    );

    assert_eq!(
        server.tmux.apply_wrapped_session_appearance(false).unwrap(),
        1
    );
    assert_eq!(status("termirust-terminal-4242"), "");
    assert_eq!(
        output(&[
            "show-options",
            "-v",
            "-t",
            "termirust-terminal-4242",
            "mouse"
        ]),
        ""
    );
    assert!(
        !wheel_binding().contains("if-shell"),
        "removing puts tmux's default binding back"
    );
    assert!(!overrides().contains("smcup@"));
}

/// A tmux client on a pseudo-terminal, with the writing end of that terminal so a test can
/// type into it the way the terminal app does.
struct AttachedClient {
    child: Box<dyn portable_pty::Child + Send>,
    terminal: Box<dyn std::io::Write + Send>,
}

impl AttachedClient {
    /// Sends bytes as if the user had typed them, or as Terminal.app sends a dropped file.
    fn type_bytes(&mut self, bytes: &[u8]) {
        use std::io::Write as _;
        self.terminal.write_all(bytes).expect("the terminal writes");
        self.terminal.flush().expect("the terminal flushes");
    }

    fn close(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Attaches a client to `session` on a pseudo-terminal, because copy-mode scrolling belongs to
/// a client and a detached session never scrolls.
fn attach_client(server: &IsolatedServer, session: &str) -> AttachedClient {
    let pty = portable_pty::native_pty_system()
        .openpty(portable_pty::PtySize {
            rows: 10,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("a pseudo-terminal stands in for the terminal app");
    let mut command = portable_pty::CommandBuilder::new(server.tmux.executable());
    command.args(["-u", "-f", "/dev/null", "attach-session", "-t", session]);
    for (name, value) in server.tmux.client_environment() {
        command.env(name, value);
    }
    command.env("TERM", "xterm-256color");
    // tmux refuses to attach a session from inside another one, and these tests are often run
    // from a terminal this app has already wrapped in tmux.
    command.env_remove("TMUX");
    command.env_remove("TMUX_PANE");
    let child = pty
        .slave
        .spawn_command(command)
        .expect("a tmux client should attach");
    drop(pty.slave);
    // The reader has to keep draining, or tmux blocks once the pipe fills.
    let mut reader = pty.master.try_clone_reader().expect("reader");
    std::thread::spawn(move || {
        use std::io::Read as _;
        let mut buffer = [0_u8; 4096];
        while let Ok(read) = reader.read(&mut buffer) {
            if read == 0 {
                return;
            }
        }
    });
    let terminal = pty.master.take_writer().expect("writer");
    std::mem::forget(pty.master);
    AttachedClient { child, terminal }
}

/// A click starts a selection under the mouse. At the bottom of the history it also leaves copy
/// mode, so typing reaches the program again. Further back it must not: leaving copy mode there
/// returns the view to the live screen, and the text moves out from under the click.
#[test]
fn a_click_leaves_copy_mode_only_at_the_bottom_of_the_history() {
    let Some(server) = IsolatedServer::start() else {
        return;
    };
    let session = "termirust-click";
    server.run(&["new-session", "-d", "-s", session, "-x", "80", "-y", "10"]);
    server.run(&["send-keys", "-t", session, "seq 1 200", "Enter"]);
    let client = attach_client(&server, session);

    let ask = |format: &str| server.output(&["display-message", "-p", "-t", session, format]);
    // Long enough for a machine running the whole suite at once: attaching a real client and
    // having tmux report it takes far longer there than on an idle one.
    let wait_until = |format: &str, done: &dyn Fn(&str) -> bool, what: &str| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let value = server.output(&["display-message", "-p", "-t", session, format]);
            if done(&value) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "{format} never {what}, last {value:?}"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    };
    let wait_for = |format: &str, value: &'static str| {
        wait_until(format, &move |seen: &str| seen == value, value);
    };
    // The client has to be attached before copy mode can scroll.
    wait_for("#{session_attached}", "1");

    // What the MouseDown1Pane binding runs.
    let click = || {
        server.run(&[
            "if-shell",
            "-t",
            session,
            "-F",
            "#{==:#{scroll_position},0}",
            "send-keys -X cancel",
            "send-keys -X clear-selection",
        ]);
    };

    // The shell has to have printed enough to scroll back through.
    wait_until(
        "#{history_size}",
        &|seen: &str| seen.parse::<u32>().unwrap_or(0) >= 20,
        "filled the history",
    );

    server.run(&["copy-mode", "-t", session]);
    server.run(&["send-keys", "-t", session, "-X", "-N", "5", "scroll-up"]);
    wait_for("#{scroll_position}", "5");
    click();
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert_eq!(
        ask("#{scroll_position}"),
        "5",
        "a click scrolled the view while the history was scrolled back"
    );
    assert_eq!(ask("#{pane_in_mode}"), "1", "a click left copy mode early");

    server.run(&["send-keys", "-t", session, "-X", "-N", "5", "scroll-down"]);
    wait_for("#{scroll_position}", "0");
    click();
    wait_for("#{pane_in_mode}", "0");

    client.close();
}

/// A file dropped on a tab that is scrolled back reaches the program. The terminal types the
/// path in as a bracketed paste, which copy mode would otherwise swallow whole, leaving a tab
/// that looks like it stopped taking input.
#[test]
fn a_file_dropped_on_a_scrolled_back_tab_reaches_the_program() {
    let Some(server) = IsolatedServer::start() else {
        return;
    };
    let session = "termirust-typing";
    server.run(&["new-session", "-d", "-s", session, "-x", "80", "-y", "10"]);
    server.tmux.apply_wrapped_session_appearance(true).unwrap();
    server.run(&["send-keys", "-t", session, "seq 1 200", "Enter"]);
    let mut client = attach_client(&server, session);

    let wait_until = |format: &str, done: &dyn Fn(&str) -> bool, what: &str| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let value = server.output(&["display-message", "-p", "-t", session, format]);
            if done(&value) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "{format} never {what}, last {value:?}"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    };
    let wait_for = |format: &str, value: &'static str| {
        wait_until(format, &move |seen: &str| seen == value, value);
    };
    wait_for("#{session_attached}", "1");
    wait_until(
        "#{history_size}",
        &|seen: &str| seen.parse::<u32>().unwrap_or(0) >= 20,
        "filled the history",
    );

    server.run(&["copy-mode", "-t", session]);
    server.run(&["send-keys", "-t", session, "-X", "-N", "5", "scroll-up"]);
    wait_for("#{scroll_position}", "5");

    // What a terminal sends when a file is dropped on it: the path, shell-quoted, bracketed as
    // a paste. It arrives on the client's terminal, where a real drop comes from.
    let dropped = "'/tmp/a b.txt'";
    client.type_bytes(format!("\x1b[200~{dropped} \x1b[201~").as_bytes());

    // A paste only reaches a binding on tmux 3.6 and newer. Before that copy mode consumes the
    // whole thing and the catch-all never runs, so the tab keeps a file dropped on it. Measured
    // against 3.4, 3.5a, 3.6 and 3.7c with these very bindings; the older half is held to what it
    // does today so that a tmux which starts answering is noticed rather than assumed.
    let answers_a_paste = termirust_tmux::parse_version(server.tmux.version())
        .is_none_or(|version| version >= (3, 6));
    if answers_a_paste {
        wait_for("#{pane_in_mode}", "0");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let screen = server.output(&["capture-pane", "-p", "-t", session]);
            if screen.contains(dropped) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the dropped path never reached the program, screen was {screen:?}"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    } else {
        std::thread::sleep(std::time::Duration::from_secs(2));
        assert_eq!(
            server.output(&["display-message", "-p", "-t", session, "#{pane_in_mode}"]),
            "1",
            "tmux {} answered a paste in copy mode; the version this is gated on can move",
            server.tmux.version()
        );
        let screen = server.output(&["capture-pane", "-p", "-t", session]);
        assert!(
            !screen.contains(dropped),
            "tmux {} delivered the drop after all: {screen:?}",
            server.tmux.version()
        );
    }

    client.close();
}

/// A wrapped tab has to tell tmux what its terminal can do: tmux only works that out for
/// itself from a terminal that answers its questions, and Terminal.app cannot draw a frame in
/// one piece the way the others can.
#[test]
fn a_wrapped_tab_tells_tmux_what_its_terminal_can_do() {
    use termirust_tmux::shell_integration::{Shell, ShellIntegration};

    for (shell_path, kind, init_name) in [
        ("/bin/zsh", Shell::Zsh, "shell-init.zsh"),
        ("/bin/bash", Shell::Bash, "shell-init.bash"),
    ] {
        if !Path::new(shell_path).is_file() {
            eprintln!("skipping {shell_path}: not installed");
            continue;
        }
        let home = tempfile::tempdir().unwrap();
        let home_path = fs::canonicalize(home.path()).unwrap();
        let recorded = home_path.join("arguments");
        let tmux = fake_tmux(
            &home_path,
            &format!("printf '%s\\n' \"$*\" >> '{}'", recorded.display()),
        );
        ShellIntegration::new(&home_path, &tmux)
            .plan_enable(&[kind])
            .unwrap()
            .apply()
            .unwrap();
        let init = home_path.join(".config/termirust").join(init_name);

        for (program, colorterm, expected) in [
            ("zed", Some("truecolor"), "-u -T RGB,sync"),
            ("iTerm.app", Some("24bit"), "-u -T RGB,sync"),
            ("Apple_Terminal", Some("truecolor"), "-u -T RGB"),
            ("zed", None, "-u -T sync"),
            ("Apple_Terminal", None, "-u"),
        ] {
            fs::write(&recorded, "").unwrap();
            let mut command = std::process::Command::new(shell_path);
            command
                .args(["-i", "-c", &format!(". '{}'", init.display())])
                .env("HOME", &home_path)
                .env("TERM_PROGRAM", program)
                .env_remove("TMUX")
                .env_remove("TERMIRUST_NO_WRAP");
            match colorterm {
                Some(value) => command.env("COLORTERM", value),
                None => command.env_remove("COLORTERM"),
            };
            let output = command.output().unwrap();
            let arguments = fs::read_to_string(&recorded).unwrap();
            assert!(
                arguments.starts_with(&format!("{expected} new-session -s termirust-")),
                "{shell_path}, {program}, COLORTERM={colorterm:?}: expected {expected:?}, \
                 tmux got {arguments:?} ({output:?})"
            );
        }
    }
}

/// Every option of a session or its windows, with inherited ones included. `-A` marks those
/// with a trailing `*`.
fn effective_options(
    server: &IsolatedServer,
    target: &str,
    window: bool,
) -> BTreeMap<String, String> {
    let mut arguments = vec!["show-options", "-A"];
    if window {
        arguments.push("-w");
    }
    arguments.extend(["-t", target]);
    server
        .output(&arguments)
        .lines()
        .filter_map(|line| {
            let (name, value) = line.split_once(' ').unwrap_or((line, ""));
            (!name.is_empty()).then(|| (name.trim_end_matches('*').to_owned(), value.to_owned()))
        })
        .collect()
}

fn differing_options(
    plain: &BTreeMap<String, String>,
    wrapped: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut names = plain
        .iter()
        .filter(|(name, value)| wrapped.get(*name) != Some(*value))
        .map(|(name, _)| name.clone())
        .chain(
            wrapped
                .keys()
                .filter(|name| !plain.contains_key(*name))
                .cloned(),
        )
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    names
}

/// A wrapped tab should feel like the tab it replaced, so a session the setup started may
/// differ from a plain one only where the setup intends: no status bar, the mouse on, and a
/// quiet selection. Anything else would change how the terminal behaves.
#[test]
fn a_wrapped_session_differs_from_a_plain_one_only_where_intended() {
    let Some(server) = IsolatedServer::start() else {
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("tmux.conf");
    fs::write(
        &config,
        termirust_tmux::appearance::WrappedSessionAppearance::for_this_platform()
            .configuration_file(),
    )
    .unwrap();
    server.run(&["new-session", "-d", "-s", "plain"]);
    server.run(&[
        "new-session",
        "-d",
        "-s",
        "termirust-parity",
        ";",
        "source-file",
        &config.to_string_lossy(),
    ]);

    let plain = effective_options(&server, "plain", false);
    let wrapped = effective_options(&server, "termirust-parity", false);
    assert_eq!(
        differing_options(&plain, &wrapped),
        ["mouse", "status"],
        "a wrapped session changes only the mouse and the status bar"
    );
    assert_eq!(wrapped.get("mouse").map(String::as_str), Some("on"));
    assert_eq!(wrapped.get("status").map(String::as_str), Some("off"));

    let plain_window = effective_options(&server, "plain", true);
    let wrapped_window = effective_options(&server, "termirust-parity", true);
    let mut expected = vec!["mode-style".to_owned()];
    // tmux before 3.5 has no copy-mode position counter to quieten.
    if plain_window.contains_key("copy-mode-position-format") {
        expected.insert(0, "copy-mode-position-format".to_owned());
    }
    assert_eq!(
        differing_options(&plain_window, &wrapped_window),
        expected,
        "a wrapped window changes only how a selection looks"
    );
}

#[test]
fn a_session_sourcing_the_generated_configuration_gets_it_and_others_do_not() {
    let Some(server) = IsolatedServer::start() else {
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("tmux.conf");
    fs::write(
        &config,
        termirust_tmux::appearance::WrappedSessionAppearance::new(Some("pbcopy".to_owned()))
            .configuration_file(),
    )
    .unwrap();
    server.run(&["new-session", "-d", "-s", "work"]);
    let config = config.to_string_lossy();
    server.run(&[
        "new-session",
        "-d",
        "-s",
        "termirust-demo-1",
        ";",
        "source-file",
        &config,
    ]);
    server.run(&["new-window", "-t", "termirust-demo-1"]);
    let output = |arguments: &[&str]| {
        let output = server.tmux.command().args(arguments).output().unwrap();
        assert!(output.status.success(), "tmux {arguments:?}: {output:?}");
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    };

    assert_eq!(
        output(&["show-options", "-v", "-t", "termirust-demo-1", "status"]),
        "off"
    );
    assert_eq!(
        output(&["show-options", "-v", "-t", "termirust-demo-1", "mouse"]),
        "on"
    );
    assert_eq!(output(&["show-options", "-v", "-t", "work", "mouse"]), "");
    assert_eq!(
        output(&[
            "show-options",
            "-w",
            "-v",
            "-t",
            "termirust-demo-1:1",
            "mode-style"
        ]),
        termirust_tmux::appearance::SELECTION_STYLE,
        "a new window in the session gets the selection style"
    );
    assert_eq!(
        output(&[
            "display-message",
            "-p",
            "-t",
            "termirust-demo-1",
            "#{m:termirust-*,#{session_name}}"
        ]),
        "1"
    );
    assert_eq!(
        output(&[
            "display-message",
            "-p",
            "-t",
            "work",
            "#{m:termirust-*,#{session_name}}"
        ]),
        "0"
    );
    let bindings = output(&["list-keys", "-T", "copy-mode-vi"]);
    assert!(bindings.contains("copy-pipe-no-clear pbcopy ; send-keys -X stop-selection"));
}
