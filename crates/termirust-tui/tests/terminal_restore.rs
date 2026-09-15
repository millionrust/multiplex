#![cfg(unix)]

use std::io::{Read as _, Write as _};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use portable_pty::{CommandBuilder, ExitStatus, PtySize, native_pty_system};
use tempfile::TempDir;
use termirust_store::{ProjectRepository, SessionRepository};

fn configured_store() -> TempDir {
    let fixture = TempDir::new().unwrap();
    let metadata = fixture.path().join("agent-workspace");
    ProjectRepository::open(&metadata).unwrap();
    SessionRepository::open(&metadata, fixture.path().join("durable-sessions")).unwrap();
    fixture
}

fn run_in_pty(arguments: &[&str], environment: &[(&str, &str)], send: Option<&[u8]>) -> Vec<u8> {
    let fixture = configured_store();
    let pty = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let before = pty.master.get_termios().unwrap();
    let reader = pty.master.try_clone_reader().unwrap();
    let writer = Arc::new(Mutex::new(pty.master.take_writer().unwrap()));
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_termirust-tui"));
    command.args(arguments);
    command.env("TERM", "xterm-256color");
    command.env("TERMIRUST_CONFIG_DIR", fixture.path());
    for (key, value) in environment {
        command.env(key, value);
    }
    let mut child = pty.slave.spawn_command(command).unwrap();
    drop(pty.slave);
    let reader_thread = spawn_reader(reader, Arc::clone(&writer));
    if let Some(bytes) = send {
        thread::sleep(Duration::from_millis(150));
        let mut writer = writer.lock().unwrap();
        writer.write_all(bytes).unwrap();
        writer.flush().unwrap();
    }
    let status = wait_bounded(child.as_mut());
    drop(writer);
    let output = reader_thread.join().unwrap();
    let after = pty.master.get_termios().unwrap();
    assert_eq!(after, before, "terminal flags were not restored");
    assert!(
        status.success()
            || environment
                .iter()
                .any(|(key, _)| *key == "TERMIRUST_TUI_INJECT_PANIC_AFTER_INIT"),
        "unexpected status {status:?}; output: {}",
        String::from_utf8_lossy(&output)
    );
    output
}

fn spawn_reader(
    reader: Box<dyn std::io::Read + Send>,
    writer: Arc<Mutex<Box<dyn std::io::Write + Send>>>,
) -> thread::JoinHandle<Vec<u8>> {
    spawn_reader_with_ready(reader, writer, None)
}

fn spawn_reader_with_ready(
    mut reader: Box<dyn std::io::Read + Send>,
    writer: Arc<Mutex<Box<dyn std::io::Write + Send>>>,
    ready: Option<Arc<AtomicBool>>,
) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut output = Vec::new();
        let mut chunk = [0u8; 4_096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(count) => {
                    output.extend_from_slice(&chunk[..count]);
                    if output
                        .windows(b"\x1b[?1049h".len())
                        .any(|window| window == b"\x1b[?1049h")
                        && let Some(ready) = &ready
                    {
                        ready.store(true, Ordering::Release);
                    }
                    if output.ends_with(b"\x1b[6n") {
                        let mut writer = writer.lock().unwrap();
                        writer.write_all(b"\x1b[1;1R").unwrap();
                        writer.flush().unwrap();
                    }
                }
                Err(_) => break,
            }
        }
        output
    })
}

fn wait_bounded(child: &mut dyn portable_pty::Child) -> ExitStatus {
    for _ in 0..100 {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        thread::sleep(Duration::from_millis(50));
    }
    child.kill().unwrap();
    let _ = child.wait();
    panic!("TUI process did not exit within five seconds");
}

#[test]
fn normal_and_inline_exit_restore_cursor_raw_mode_and_screen() {
    let fullscreen = run_in_pty(&[], &[], Some(b"q"));
    let output = String::from_utf8_lossy(&fullscreen);
    assert!(output.contains("\u{1b}[?1049h"));
    assert!(output.contains("\u{1b}[?1049l"));
    assert!(output.contains("\u{1b}[?25h"));

    let inline = run_in_pty(
        &["--inline", "--no-color"],
        &[("TERMIRUST_TUI_EXIT_AFTER_FIRST_DRAW", "1")],
        None,
    );
    let output = String::from_utf8_lossy(&inline);
    assert!(!output.contains("\u{1b}[?1049h"));
    assert!(!output.contains("\u{1b}[?1049l"));
    assert!(output.contains("\u{1b}[?25h"));
}

#[test]
fn panic_hook_restores_terminal_once_before_reporting_failure() {
    let output = run_in_pty(&[], &[("TERMIRUST_TUI_INJECT_PANIC_AFTER_INIT", "1")], None);
    let output = String::from_utf8_lossy(&output);
    assert_eq!(
        output.matches("\u{1b}[?1049l").count(),
        1,
        "output: {output:?}"
    );
    assert_eq!(
        output.matches("\u{1b}[?25h").count(),
        1,
        "output: {output:?}"
    );
    assert!(output.contains("injected terminal restoration test"));
}

#[test]
fn sigint_restores_terminal_and_exits_cleanly() {
    let fixture = configured_store();
    let pty = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let before = pty.master.get_termios().unwrap();
    let reader = pty.master.try_clone_reader().unwrap();
    let writer = Arc::new(Mutex::new(pty.master.take_writer().unwrap()));
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_termirust-tui"));
    command.env("TERM", "xterm-256color");
    command.env("TERMIRUST_CONFIG_DIR", fixture.path());
    let mut child = pty.slave.spawn_command(command).unwrap();
    let pid = child.process_id().unwrap() as libc::pid_t;
    drop(pty.slave);
    let ready = Arc::new(AtomicBool::new(false));
    let reader_thread =
        spawn_reader_with_ready(reader, Arc::clone(&writer), Some(Arc::clone(&ready)));
    for _ in 0..100 {
        if ready.load(Ordering::Acquire) {
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        ready.load(Ordering::Acquire),
        "TUI did not initialize in time"
    );
    assert_eq!(unsafe { libc::kill(pid, libc::SIGINT) }, 0);
    let status = wait_bounded(child.as_mut());
    assert!(status.success(), "SIGINT child status: {status:?}");
    let output = reader_thread.join().unwrap();
    assert_eq!(pty.master.get_termios().unwrap(), before);
    let output = String::from_utf8_lossy(&output);
    assert_eq!(
        output.matches("\u{1b}[?1049l").count(),
        1,
        "output: {output:?}"
    );
    assert_eq!(
        output.matches("\u{1b}[?25h").count(),
        1,
        "output: {output:?}"
    );
}

#[test]
fn idle_process_has_bounded_cpu_and_memory() {
    let fixture = configured_store();
    let pty = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let before = pty.master.get_termios().unwrap();
    let reader = pty.master.try_clone_reader().unwrap();
    let writer = Arc::new(Mutex::new(pty.master.take_writer().unwrap()));
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_termirust-tui"));
    command.env("TERM", "xterm-256color");
    command.env("TERMIRUST_CONFIG_DIR", fixture.path());
    let mut child = pty.slave.spawn_command(command).unwrap();
    let pid = child.process_id().unwrap();
    drop(pty.slave);
    let reader_thread = spawn_reader(reader, Arc::clone(&writer));

    thread::sleep(Duration::from_secs(2));
    let sample = Command::new("ps")
        .args(["-o", "rss=,%cpu=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    assert!(sample.status.success());
    let sample = String::from_utf8(sample.stdout).unwrap();
    let mut values = sample.split_whitespace();
    let rss_kib = values.next().unwrap().parse::<u64>().unwrap();
    let cpu_percent = values.next().unwrap().parse::<f64>().unwrap();
    eprintln!("idle TUI sample: rss={rss_kib} KiB cpu={cpu_percent:.1}%");
    assert!(rss_kib <= 128 * 1024, "idle RSS was {rss_kib} KiB");
    assert!(cpu_percent <= 5.0, "idle CPU was {cpu_percent:.1}%");

    {
        let mut writer = writer.lock().unwrap();
        writer.write_all(b"q").unwrap();
        writer.flush().unwrap();
    }
    assert!(wait_bounded(child.as_mut()).success());
    drop(writer);
    let _ = reader_thread.join().unwrap();
    assert_eq!(pty.master.get_termios().unwrap(), before);
}
