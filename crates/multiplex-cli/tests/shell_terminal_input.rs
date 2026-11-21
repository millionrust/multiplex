#![cfg(unix)]
//! A terminal window running `multiplex-cli shell` or `multiplex-cli session attach`, driven the
//! way a person and their terminal drive it. A terminal sends far more than typed keys: mouse and focus reports, pastes, and its
//! answers to the program's own questions. Every byte of it has to reach the program as sent.
//! The shell window once parsed that stream as key events, and a reply the parser could not
//! finish (a mode report, the terminal's name, a colour ended by BEL) silently swallowed every key
//! after it, so arrows and Enter did nothing in Codex and Antigravity CLI.

mod common;

use std::collections::BTreeMap;
use std::io::{Read as _, Write as _};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use common::*;
use multiplex_client::LocalEndpoint;
use multiplex_domain::{HostInstanceId, HostedSessionState};
use multiplex_session_host::{LaunchDescriptor, SessionHostHandle, StopDeadlines, start};
use multiplex_store::JournalLimits;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};

static REAL_HOST_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Longer than the window's 1.5 s writer-lease idle, so the next key must take the lease again.
const LEASE_IDLE_PAUSE: Duration = Duration::from_millis(1_800);

/// How the bytes of one interaction leave the terminal.
#[derive(Clone, Copy)]
enum Delivery {
    /// One write, as a terminal sends one key or one report.
    Whole,
    /// One byte per write, so every sequence is split across reads.
    Bytewise,
}

struct Interaction {
    name: &'static str,
    bytes: Vec<u8>,
    delivery: Delivery,
    pause_after: Duration,
}

fn whole(name: &'static str, bytes: impl AsRef<[u8]>) -> Interaction {
    Interaction {
        name,
        bytes: bytes.as_ref().to_vec(),
        delivery: Delivery::Whole,
        pause_after: Duration::from_millis(5),
    }
}

fn bytewise(name: &'static str, bytes: impl AsRef<[u8]>) -> Interaction {
    Interaction {
        delivery: Delivery::Bytewise,
        ..whole(name, bytes)
    }
}

fn then_idle(mut interaction: Interaction) -> Interaction {
    interaction.pause_after = LEASE_IDLE_PAUSE;
    interaction
}

/// Answers a terminal gives to a program's questions, each ended the way terminals end them.
fn terminal_replies() -> Vec<Interaction> {
    vec![
        whole("cursor position report", b"\x1b[12;40R"),
        whole("primary device attributes", b"\x1b[?62;22;52c"),
        whole("secondary device attributes", b"\x1b[>1;10;0c"),
        whole("DECRQM synchronized output", b"\x1b[?2026;2$y"),
        whole(
            "DECRQM bracketed paste and ANSI mode",
            b"\x1b[?2004;1$y\x1b[2;1$y",
        ),
        whole("XTVERSION name in a DCS", b"\x1bP>|Terminal 2.14\x1b\\"),
        whole("DECRQSS reply", b"\x1bP1$r0m\x1b\\"),
        whole("XTGETTCAP reply", b"\x1bP1+r544e=787465726d\x1b\\"),
        whole(
            "foreground colour ended by ST",
            b"\x1b]10;rgb:0000/0000/0000\x1b\\",
        ),
        whole(
            "background colour ended by BEL",
            b"\x1b]11;rgb:ffff/ffff/ffff\x07",
        ),
        whole("palette colour", b"\x1b]4;1;rgb:cdcd/0000/0000\x07"),
        whole("clipboard read", b"\x1b]52;c;aGVsbG8=\x07"),
        whole("kitty keyboard flags", b"\x1b[?0u\x1b[?7u"),
        whole("kitty graphics answer in an APC", b"\x1b_Gi=1;OK\x1b\\"),
        whole(
            "window size reports",
            b"\x1b[8;24;80t\x1b[4;480;800t\x1b[6;16;8t",
        ),
        whole("colour scheme report", b"\x1b[?997;1n"),
        whole("status report", b"\x1b[0n"),
    ]
}

/// A person at a terminal, and the terminal's own reports, in the order a session might see
/// them. None of it contains the detach keys.
fn human_session() -> Vec<Interaction> {
    let mut steps = vec![
        whole("typed word", b"ls"),
        whole("space and digits", b" -la 0123456789"),
        whole("punctuation", b"~`!@#$%^&*()-_=+[{]}\\|;:'\",<.>/?"),
        whole("Enter", b"\r"),
        whole("Tab completion", b"\t"),
        whole("Shift-Tab", b"\x1b[Z"),
        whole("Backspace", b"\x7f"),
        whole("Ctrl-H", b"\x08"),
        whole("Escape alone", b"\x1b"),
        whole("Alt-b and Alt-f", b"\x1bb\x1bf"),
        whole("Alt-Backspace", b"\x1b\x7f"),
        whole("Ctrl-Space", b"\x00"),
        whole("Ctrl-A E K U W Y L R", b"\x01\x05\x0b\x15\x17\x19\x0c\x12"),
        whole("Ctrl-C", b"\x03"),
        whole("Ctrl-D", b"\x04"),
        whole("Ctrl-Z", b"\x1a"),
        whole("Ctrl-backslash", b"\x1c"),
        whole("Ctrl-S and Ctrl-Q", b"\x13\x11"),
        whole("Ctrl-caret and Ctrl-underscore", b"\x1e\x1f"),
        whole("arrows", b"\x1b[A\x1b[B\x1b[C\x1b[D"),
        whole("application-mode arrows", b"\x1bOA\x1bOB\x1bOC\x1bOD"),
        whole(
            "Home and End",
            b"\x1b[H\x1b[F\x1bOH\x1bOF\x1b[1~\x1b[4~\x1b[7~\x1b[8~",
        ),
        whole("Page Up and Page Down", b"\x1b[5~\x1b[6~"),
        whole("Insert and Delete", b"\x1b[2~\x1b[3~"),
        whole("F1 to F4", b"\x1bOP\x1bOQ\x1bOR\x1bOS"),
        whole(
            "F5 to F12",
            b"\x1b[15~\x1b[17~\x1b[18~\x1b[19~\x1b[20~\x1b[21~\x1b[23~\x1b[24~",
        ),
        whole(
            "Ctrl and Alt arrows",
            b"\x1b[1;5A\x1b[1;5B\x1b[1;3C\x1b[1;3D",
        ),
        whole("Shift-arrows and Shift-F5", b"\x1b[1;2A\x1b[15;2~"),
        whole("modifyOtherKeys Ctrl-Enter", b"\x1b[27;5;13~"),
        whole("kitty Ctrl-a and Enter", b"\x1b[97;5u\x1b[13u"),
        whole("kitty releases", b"\x1b[97;1:3u\x1b[1;1:3A\x1b[57441;2:3u"),
        whole("kitty Ctrl-d", b"\x1b[100;5u"),
        whole("accented and currency text", "café ₹ ü ñ".as_bytes()),
        whole("emoji with joiners", "👩‍💻 👍🏽 🇮🇳".as_bytes()),
        whole(
            "CJK and right-to-left text",
            "漢字 かな 한글 עברית".as_bytes(),
        ),
        whole("combining marks", "e\u{301}a\u{308}".as_bytes()),
        whole("IME commit", "日本語".as_bytes()),
        whole(
            "bracketed paste",
            b"\x1b[200~echo pasted\r\nsecond line\x1b[201~",
        ),
        whole("empty bracketed paste", b"\x1b[200~\x1b[201~"),
        whole("unbracketed paste", b"line one\rline two\r"),
        whole("focus lost and regained", b"\x1b[O\x1b[I"),
        whole("SGR click", b"\x1b[<0;10;5M\x1b[<0;10;5m"),
        whole(
            "SGR drag",
            b"\x1b[<0;10;5M\x1b[<32;11;5M\x1b[<32;12;6M\x1b[<0;12;6m",
        ),
        whole(
            "SGR right and middle click",
            b"\x1b[<2;3;4M\x1b[<2;3;4m\x1b[<1;3;4M",
        ),
        whole("SGR wheel", b"\x1b[<64;10;5M\x1b[<64;10;5M\x1b[<65;10;5M"),
        whole("SGR motion with no button", b"\x1b[<35;20;9M\x1b[<35;21;9M"),
        whole("SGR Ctrl and Shift click", b"\x1b[<16;4;4M\x1b[<4;4;4M"),
        whole("X10 click", b"\x1b[M !!\x1b[M#!!"),
        whole("urxvt click", b"\x1b[0;10;5M"),
    ];
    steps.extend(terminal_replies());
    steps.extend([
        whole(
            "keys after replies in one read",
            b"\x1b[?2026;2$y\x1bP>|Zed\x1b\\\x1b]11;rgb:1717/1919/1d1d\x07\x1b[B\r",
        ),
        whole(
            "mouse wheel then arrow in one read",
            b"\x1b[<65;10;5M\x1b[A",
        ),
        bytewise(
            "every reply split byte by byte",
            terminal_replies()
                .into_iter()
                .flat_map(|step| step.bytes)
                .collect::<Vec<_>>(),
        ),
        bytewise("arrows and Enter split byte by byte", b"\x1b[B\x1bOA\r"),
        bytewise(
            "paste split byte by byte",
            "\x1b[200~₹\r\n👩‍💻\x1b[201~".as_bytes(),
        ),
        bytewise("SGR wheel split byte by byte", b"\x1b[<64;10;5M"),
        then_idle(whole("a key before a pause", b"q")),
        whole("a key after the lease was let go", b"\x1b[B"),
        then_idle(whole("Enter before a pause", b"\r")),
        whole(
            "a reply after the lease was let go",
            b"\x1b[?2026;2$y\x1b[A",
        ),
        whole("a long typed line", "x".repeat(2_000)),
        whole("final Enter", b"\r"),
    ]);
    steps
}

/// The detach keys where they must not detach: inside a paste, or a leader followed by another
/// key. Both reach the program.
fn detach_keys_that_are_not_a_detach() -> Vec<Interaction> {
    vec![
        whole("Ctrl-] then x", b"\x1dx"),
        whole("Ctrl-] twice", b"\x1d\x1d"),
        whole("kitty Ctrl-] then Escape", b"\x1b[93;5u\x1b"),
        whole(
            "detach keys inside a paste",
            b"\x1b[200~\x1dd \x1b[93;5ud\x1b[201~",
        ),
        bytewise(
            "detach keys inside a split paste",
            b"\x1b[200~\x1dd\x1b[201~",
        ),
        whole("end", b"END"),
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_shell_window_delivers_every_human_and_terminal_input_byte_for_byte() {
    check_delivered(Bridge::ShellWindow, human_session()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_app_bridge_delivers_every_human_and_terminal_input_byte_for_byte() {
    check_delivered(Bridge::AppRaw, human_session()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_attach_delivers_every_human_and_terminal_input_byte_for_byte() {
    check_delivered(Bridge::Attach, Bridge::Attach.sendable(human_session())).await;
}

/// A read-only attach sends nothing on, and still detaches on Ctrl-C or Ctrl-] then d after
/// the replies that used to leave its key parser waiting.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_only_attach_sends_nothing_and_still_detaches_after_terminal_replies() {
    for (bridge, detach) in [
        (Bridge::AttachReadOnly, b"\x03".as_slice()),
        (Bridge::AttachReadOnly, b"\x1dd"),
        (Bridge::AttachReadOnly, b"\x1b[99;5u"),
        (Bridge::Attach, b"\x03"),
        (Bridge::Attach, b"\x1b[93;5ud"),
    ] {
        let mut console = Console::open(bridge, &recorder(1, b"")).await;
        console.wait_for(b"INPUT-READY");
        for reply in terminal_replies() {
            console.send(&reply.bytes);
        }
        if matches!(bridge, Bridge::AttachReadOnly) {
            console.send(b"must not arrive\r");
        }
        thread::sleep(Duration::from_millis(100));
        console.send(detach);
        let status = console.wait_for_exit();
        assert!(
            status.success(),
            "{bridge:?} {detach:?} exited with {status:?}"
        );
        let received = std::fs::read(&console.received).unwrap_or_default();
        if matches!(bridge, Bridge::AttachReadOnly) {
            assert!(received.is_empty(), "a read-only attach sent {received:?}");
        }
        assert!(
            console.program_is_running(),
            "{bridge:?} {detach:?} stopped the program"
        );
        console.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_shell_window_sends_detach_keys_on_when_they_are_not_a_detach() {
    check_delivered(Bridge::ShellWindow, detach_keys_that_are_not_a_detach()).await;
}

/// The handshake Codex and Antigravity CLI make at startup, then a person choosing an option:
/// the program asks, the terminal answers, and the arrow and Enter that follow must arrive.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_program_hears_the_terminal_answer_its_questions_and_then_the_keys() {
    for bridge in [Bridge::ShellWindow, Bridge::AppRaw, Bridge::Attach] {
        let questions: [(&[u8], &[u8]); 7] = [
            (b"\x1b[6n", b"\x1b[3;1R"),
            (b"\x1b[c", b"\x1b[?62;22c"),
            (b"\x1b[?u", b"\x1b[?0u"),
            (b"\x1b[?2026$p", b"\x1b[?2026;2$y"),
            (b"\x1b[>q", b"\x1bP>|Terminal 2.14\x1b\\"),
            (b"\x1b]10;?\x1b\\", b"\x1b]10;rgb:0000/0000/0000\x1b\\"),
            (b"\x1b]11;?\x07", b"\x1b]11;rgb:ffff/ffff/ffff\x07"),
        ];
        let keys: &[u8] = b"\x1b[B\x1b[B\x1b[A\r";
        let answers: Vec<u8> = questions
            .iter()
            .flat_map(|(_, answer)| *answer)
            .copied()
            .collect();
        let expected = [answers.as_slice(), keys].concat();
        let asked: Vec<u8> = questions
            .iter()
            .flat_map(|(question, _)| *question)
            .copied()
            .collect();
        let mut console = Console::open(bridge, &recorder(expected.len(), &asked)).await;
        console.wait_for(b"INPUT-READY");
        for (question, answer) in questions {
            console.wait_for(question);
            console.send(answer);
        }
        thread::sleep(Duration::from_millis(50));
        console.send(keys);
        let received = console.received(expected.len());
        console.close().await;
        assert_bytes(
            &received,
            &expected,
            bridge,
            &[("handshake", expected.len())],
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ctrl_right_bracket_then_d_detaches_a_shell_window_and_leaves_the_program_running() {
    for (leader, detach) in [
        (b"\x1d".as_slice(), b"d".as_slice()),
        (b"\x1d", b"D"),
        (b"\x1b[93;5u", b"d"),
        (b"\x1b[93;5u", b"\x1b[100u"),
        (b"\x1b[27;5;93~", b"d"),
    ] {
        let mut console = Console::open(Bridge::ShellWindow, &recorder(3, b"")).await;
        console.wait_for(b"INPUT-READY");
        console.send(b"abc");
        console.send(leader);
        thread::sleep(Duration::from_millis(20));
        console.send(detach);
        let status = console.wait_for_exit();
        assert!(status.success(), "detach exited with {status:?}");
        assert_eq!(console.received(3), b"abc");
        assert!(
            console.program_is_running(),
            "detaching {leader:?} {detach:?} stopped the program"
        );
        console.close().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resized_shell_window_resizes_the_program() {
    for bridge in [Bridge::ShellWindow, Bridge::AppRaw, Bridge::Attach] {
        let script = "stty raw -echo; printf 'INPUT-READY'; head -c 1 >/dev/null; stty size > \"$RECEIVED\"; printf 'INPUT-DONE'; sleep 30";
        let mut console = Console::open(bridge, script).await;
        console.wait_for(b"INPUT-READY");
        console.resize(37, 113);
        thread::sleep(Duration::from_millis(300));
        console.send(b"r");
        console.wait_for(b"INPUT-DONE");
        let size = String::from_utf8(console.received(0)).unwrap();
        console.close().await;
        assert_eq!(size.trim(), "37 113", "{bridge:?}");
    }
}

/// The app opens a pane's terminal at a default size and sends the real one as soon as the pane
/// is laid out, while the attach is still starting. That resize must not be lost.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_window_resized_while_the_attach_starts_resizes_the_program() {
    // The window between reading the starting size and watching for resizes is a few
    // milliseconds wide for a shell, so sweep through it; a writer attach never resized at start.
    let shell_delays: Vec<u64> = (0..=28).step_by(4).collect();
    for (bridge, delays) in [
        (Bridge::ShellWindow, shell_delays.clone()),
        (Bridge::AppRaw, shell_delays),
        (Bridge::Attach, vec![0, 10]),
    ] {
        for delay in delays {
            let script = "stty raw -echo; printf 'INPUT-READY'; head -c 1 >/dev/null; stty size > \"$RECEIVED\"; printf 'INPUT-DONE'; sleep 30";
            let mut console = Console::open(bridge, script).await;
            thread::sleep(Duration::from_millis(delay));
            console.resize(37, 113);
            console.wait_for(b"INPUT-READY");
            thread::sleep(Duration::from_millis(100));
            console.send(b"r");
            console.wait_for(b"INPUT-DONE");
            let size = String::from_utf8(console.received(0)).unwrap();
            console.close().await;
            assert_eq!(
                size.trim(),
                "37 113",
                "{bridge:?} lost a resize {delay} ms after it started"
            );
        }
    }
}

/// A window that reports 0x0, as some do before they are drawn, leaves the program a usable size
/// (the Host's own, or the classic 80x24 a shell window starts with) rather than one cell.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_window_with_no_size_yet_leaves_the_program_its_size() {
    for bridge in [Bridge::ShellWindow, Bridge::AppRaw, Bridge::Attach] {
        let script = "stty raw -echo; printf 'INPUT-READY'; head -c 1 >/dev/null; stty size > \"$RECEIVED\"; printf 'INPUT-DONE'; sleep 30";
        let mut console = Console::open_sized(bridge, script, (0, 0)).await;
        console.wait_for(b"INPUT-READY");
        thread::sleep(Duration::from_millis(200));
        console.send(b"r");
        console.wait_for(b"INPUT-DONE");
        let size = String::from_utf8(console.received(0)).unwrap();
        console.close().await;
        let (rows, columns) = size.trim().split_once(' ').unwrap();
        let (rows, columns): (u16, u16) = (rows.parse().unwrap(), columns.parse().unwrap());
        assert!(
            rows >= 2 && columns >= 10,
            "{bridge:?} shrank the program to {rows}x{columns}"
        );
    }
}

#[derive(Clone, Copy, Debug)]
enum Bridge {
    /// `multiplex-cli shell` in a terminal window.
    ShellWindow,
    /// `multiplex-cli shell --raw`, the desktop app's own bridge.
    AppRaw,
    /// `multiplex-cli session attach --write`, where Ctrl-C detaches.
    Attach,
    /// `multiplex-cli session attach`, which only watches.
    AttachReadOnly,
}

impl Bridge {
    /// Ctrl-C detaches an attach, so a session sent to one leaves it out.
    fn sendable(self, steps: Vec<Interaction>) -> Vec<Interaction> {
        match self {
            Self::Attach | Self::AttachReadOnly => steps
                .into_iter()
                .filter(|step| !step.bytes.contains(&0x03))
                .collect(),
            Self::ShellWindow | Self::AppRaw => steps,
        }
    }
}

async fn check_delivered(bridge: Bridge, steps: Vec<Interaction>) {
    let expected: Vec<u8> = steps.iter().flat_map(|step| step.bytes.clone()).collect();
    let mut console = Console::open(bridge, &recorder(expected.len(), b"")).await;
    console.wait_for(b"INPUT-READY");
    let mut spans = Vec::with_capacity(steps.len());
    for step in &steps {
        match step.delivery {
            Delivery::Whole => console.send(&step.bytes),
            Delivery::Bytewise => {
                for byte in &step.bytes {
                    console.send(&[*byte]);
                    thread::sleep(Duration::from_micros(200));
                }
            }
        }
        spans.push((step.name, step.bytes.len()));
        thread::sleep(step.pause_after);
    }
    let received = console.received(expected.len());
    console.close().await;
    assert_bytes(&received, &expected, bridge, &spans);
}

/// Names the first interaction whose bytes did not arrive as sent.
fn assert_bytes(received: &[u8], expected: &[u8], bridge: Bridge, spans: &[(&str, usize)]) {
    if received == expected {
        return;
    }
    let at = received
        .iter()
        .zip(expected)
        .position(|(a, b)| a != b)
        .unwrap_or(received.len().min(expected.len()));
    let mut start = 0;
    let name = spans
        .iter()
        .find_map(|(name, len)| {
            let found = (start..start + len).contains(&at).then_some(*name);
            start += len;
            found
        })
        .unwrap_or("after the last interaction");
    let window = |bytes: &[u8]| {
        let from = at.saturating_sub(16);
        String::from_utf8_lossy(&bytes[from.min(bytes.len())..(at + 32).min(bytes.len())])
            .escape_debug()
            .to_string()
    };
    panic!(
        "{bridge:?}: {} of {} bytes arrived; first difference at byte {at}, in {name:?}\n  sent:     {}\n  received: {}",
        received.len(),
        expected.len(),
        window(expected),
        window(received)
    );
}

/// A program that puts its terminal in raw mode, asks its questions, and keeps what it reads as
/// it reads it, so a failure shows how far the input got.
fn recorder(length: usize, questions: &[u8]) -> String {
    let questions: String = questions
        .iter()
        .map(|byte| format!("\\{byte:03o}"))
        .collect();
    format!(
        "stty raw -echo; printf 'INPUT-READY{questions}'; dd bs=1 count={length} of=\"$RECEIVED\" 2>/dev/null; printf 'INPUT-DONE'; sleep 30"
    )
}

struct Console {
    host: Option<SessionHostHandle>,
    master: Option<Box<dyn MasterPty + Send>>,
    writer: Option<Box<dyn std::io::Write + Send>>,
    output: Arc<Mutex<Vec<u8>>>,
    reader: Option<thread::JoinHandle<()>>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    received: PathBuf,
    program_pid: i32,
    _seed: SeededStore,
    _guard: tokio::sync::MutexGuard<'static, ()>,
}

impl Console {
    async fn open(bridge: Bridge, script: &str) -> Self {
        Self::open_sized(bridge, script, (24, 100)).await
    }

    /// Opens the window at `rows` by `cols`, which may be 0x0 as before a window is drawn.
    async fn open_sized(bridge: Bridge, script: &str, (rows, cols): (u16, u16)) -> Self {
        let guard = REAL_HOST_TEST_LOCK.lock().await;
        let seed = seed_store();
        let received = seed.temp.path().join("received");
        let attach = matches!(bridge, Bridge::Attach | Bridge::AttachReadOnly);
        if attach {
            insert_session(&seed, HostedSessionState::Live, false);
        }
        let session_dir = seed
            .config_root
            .join(if attach {
                "durable-sessions"
            } else {
                "console-sessions"
            })
            .join(SESSION_ID.to_string());
        let host = start(LaunchDescriptor {
            format_version: LaunchDescriptor::FORMAT_VERSION,
            session_id: SESSION_ID,
            host_instance_id: HostInstanceId::new(),
            expected_occupant_generation: None,
            runtime_root: LocalEndpoint::for_config_root(&seed.config_root, SESSION_ID)
                .runtime_root()
                .to_path_buf(),
            session_dir: session_dir.clone(),
            executable: std::fs::canonicalize("/bin/sh").unwrap(),
            runtime_detection: None,
            arguments: vec!["-c".into(), script.into()],
            environment: BTreeMap::from([
                ("PATH".into(), "/usr/bin:/bin".into()),
                ("RECEIVED".into(), received.display().to_string()),
            ]),
            cwd: Some(seed.folder.clone()),
            columns: 100,
            rows: 24,
            journal_limits: JournalLimits::default(),
            stop_deadlines: StopDeadlines::default(),
        })
        .await
        .unwrap();
        let program_pid = multiplex_store::read_host_metadata(&session_dir)
            .unwrap()
            .process_token
            .unwrap()
            .platform_identity()
            .try_into()
            .unwrap();
        if !attach {
            multiplex_store::write_console_session(
                &session_dir,
                &multiplex_store::ConsoleSessionRecord {
                    schema_version: multiplex_store::ConsoleSessionRecord::SCHEMA_VERSION,
                    session_id: SESSION_ID,
                    program: "sh".into(),
                    title_override: None,
                    working_directory: seed.folder.clone(),
                    started_at: 1,
                },
            )
            .unwrap();
        }
        let pty = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let output = Arc::new(Mutex::new(Vec::new()));
        let reader = {
            let mut reader = pty.master.try_clone_reader().unwrap();
            let output = Arc::clone(&output);
            thread::spawn(move || {
                let mut chunk = [0_u8; 4_096];
                while let Ok(count @ 1..) = reader.read(&mut chunk) {
                    output.lock().unwrap().extend_from_slice(&chunk[..count]);
                }
            })
        };
        let writer = pty.master.take_writer().unwrap();
        let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_multiplex-cli"));
        match bridge {
            Bridge::ShellWindow => {
                command.args(["shell", "--session-id", &SESSION_ID.to_string()]);
            }
            Bridge::AppRaw => {
                command.args(["shell", "--raw", "--session-id", &SESSION_ID.to_string()]);
            }
            Bridge::Attach => {
                command.args(["session", "attach", &SESSION_ID.to_string(), "--write"]);
            }
            Bridge::AttachReadOnly => {
                command.args(["session", "attach", &SESSION_ID.to_string()]);
            }
        }
        command.env_remove("MULTIPLEX_SHELL_SESSION");
        command.env("TERM", "xterm-256color");
        command.env("MULTIPLEX_CONFIG_DIR", &seed.config_root);
        let child = pty.slave.spawn_command(command).unwrap();
        drop(pty.slave);
        Self {
            host: Some(host),
            master: Some(pty.master),
            writer: Some(writer),
            output,
            reader: Some(reader),
            child,
            received,
            program_pid,
            _seed: seed,
            _guard: guard,
        }
    }

    fn send(&mut self, bytes: &[u8]) {
        let writer = self.writer.as_mut().unwrap();
        writer.write_all(bytes).unwrap();
        writer.flush().unwrap();
    }

    fn resize(&self, rows: u16, cols: u16) {
        self.master
            .as_ref()
            .unwrap()
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
    }

    /// Waits until the window has shown these bytes.
    fn wait_for(&self, expected: &[u8]) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if self
                .output
                .lock()
                .unwrap()
                .windows(expected.len())
                .any(|window| window == expected)
            {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "the window never showed {:?}; it showed {:?}",
                String::from_utf8_lossy(expected),
                String::from_utf8_lossy(&self.output.lock().unwrap())
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// What the program kept, once it has this many bytes or has had long enough.
    fn received(&self, length: usize) -> Vec<u8> {
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            let size = std::fs::metadata(&self.received).map_or(0, |m| m.len());
            if length == 0 && size > 0 || length > 0 && size >= length as u64 {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        std::fs::read(&self.received).unwrap_or_default()
    }

    fn wait_for_exit(&mut self) -> portable_pty::ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "the window did not detach");
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn program_is_running(&self) -> bool {
        // SAFETY: signal 0 only probes the Host's own child, which the Host has not reaped yet.
        unsafe { libc::kill(self.program_pid, 0) == 0 }
    }

    /// Stops the window and shuts the Host down; dropping does the rest.
    async fn close(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(host) = self.host.take() {
            host.shutdown().await.unwrap();
        }
    }
}

impl Drop for Console {
    /// A test that fails before `close` still leaves no window, reader or program behind.
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if self.host.is_some() {
            // Shutting the Host down needs an await; ending its program is what must not wait.
            // SAFETY: SIGKILL goes only to the program this Console's own Host started.
            unsafe { libc::kill(self.program_pid, libc::SIGKILL) };
        }
        self.writer.take();
        self.master.take();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}
