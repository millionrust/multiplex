#![cfg(unix)]

mod common;

use std::collections::BTreeMap;
use std::io::{Read as _, Write as _};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use common::*;
use multiplex_cli::{
    Cancellation, CliCommand, CliData, CommandService, ErrorCode, RenderOptions, render_success,
};
use multiplex_client::{ConnectOptions, HostClient, LocalEndpoint};
use multiplex_domain::{HostInstanceId, HostedSessionState, OutputSequence};
use multiplex_session_host::{LaunchDescriptor, StopDeadlines, start};
use multiplex_store::JournalLimits;
use portable_pty::{CommandBuilder, ExitStatus, PtySize, native_pty_system};
use tokio_util::sync::CancellationToken;

const FROM_SEQUENCE: u64 = 3;
const COLUMNS: u16 = 101;
const ROWS: u16 = 31;
const SECRET_CANARY: &str = "ATTACH-TERMINAL-BYTES-MUST-NOT-ENTER-JSON";
static REAL_HOST_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[test]
fn local_service_validates_identity_and_returns_only_a_bounded_attach_summary() {
    let seed = seed_store();
    insert_session(&seed, HostedSessionState::Live, false);
    let host_id = HostInstanceId::new();
    let _lease = write_ready_host_metadata(&seed, host_id);
    let controller = Arc::new(Mutex::new(FakeControllerState::default()));
    let mut service = service(
        &seed,
        Arc::new(Mutex::new(FakeLauncherState::default())),
        controller.clone(),
    );
    let data = service
        .execute(attach_command(false), &Cancellation::default())
        .unwrap();
    let CliData::Attach(data) = data else {
        panic!("expected attach summary");
    };
    assert_eq!(data.session_id, SESSION_ID.to_string());
    assert_eq!(data.lifecycle, "ready");
    assert_eq!(data.from_sequence, FROM_SEQUENCE);
    assert_eq!(data.latest_sequence, 9);
    assert_eq!(data.replayed_records, 2);
    assert_eq!(data.replayed_bytes, 12);
    assert!(!data.snapshot);
    assert!(!data.writer_lease);
    let state = controller.lock().unwrap();
    assert_eq!(state.attach_calls, 1);
    assert_eq!(state.expected_hosts, vec![Some(host_id)]);
    assert_eq!(state.attach_requests.len(), 1);
    assert_eq!(state.attach_requests[0].columns, COLUMNS);
    assert_eq!(state.attach_requests[0].rows, ROWS);
    assert!(!state.attach_requests[0].request_control);
    drop(state);

    let human = String::from_utf8(
        render_success(
            &CliData::Attach(data),
            &[],
            RenderOptions {
                json: false,
                terminal_width: 80,
            },
        )
        .unwrap(),
    )
    .unwrap();
    assert!(human.contains("Session attach summary"));
    assert!(human.contains("Writer lease: read-only"));
}

#[test]
fn unsafe_state_metadata_and_pre_dispatch_cancellation_never_call_host() {
    for (state, archived) in [
        (HostedSessionState::Exited, true),
        (HostedSessionState::RunningAppAttached, false),
    ] {
        let seed = seed_store();
        insert_session(&seed, state, archived);
        let _lease = write_ready_host_metadata(&seed, HostInstanceId::new());
        let controller = Arc::new(Mutex::new(FakeControllerState::default()));
        let mut service = service(
            &seed,
            Arc::new(Mutex::new(FakeLauncherState::default())),
            controller.clone(),
        );
        assert_eq!(
            service
                .execute(attach_command(false), &Cancellation::default())
                .unwrap_err()
                .code,
            ErrorCode::Validation
        );
        assert_eq!(controller.lock().unwrap().attach_calls, 0);
    }

    let seed = seed_store();
    insert_session(&seed, HostedSessionState::Live, false);
    let controller = Arc::new(Mutex::new(FakeControllerState::default()));
    let mut service = service(
        &seed,
        Arc::new(Mutex::new(FakeLauncherState::default())),
        controller.clone(),
    );
    assert_eq!(
        service
            .execute(attach_command(false), &Cancellation::default())
            .unwrap_err()
            .code,
        ErrorCode::Unavailable
    );
    let _lease = write_ready_host_metadata(&seed, HostInstanceId::new());
    let cancelled = Cancellation::default();
    cancelled.cancel();
    assert_eq!(
        service
            .execute(attach_command(false), &cancelled)
            .unwrap_err()
            .code,
        ErrorCode::Cancelled
    );
    assert_eq!(controller.lock().unwrap().attach_calls, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn packaged_json_attach_never_emits_terminal_bytes_and_leaves_host_running() {
    let _fixture_guard = REAL_HOST_TEST_LOCK.lock().await;
    let seed = seed_store();
    insert_session(&seed, HostedSessionState::Live, false);
    let host = start(descriptor(&seed)).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let output = packaged_attach(&seed, false);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.contains(SECRET_CANARY));
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(json["data"]["session_id"], SESSION_ID.to_string());
    assert_eq!(json["data"]["lifecycle"], "ready");
    assert_eq!(json["data"]["writer_lease"], false);
    assert!(
        json["data"]["snapshot"].as_bool().unwrap()
            || json["data"]["replayed_records"].as_u64().unwrap() > 0
    );
    assert_eq!(
        host.stats().await.lifecycle,
        multiplex_domain::HostLifecycle::Ready
    );
    host.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_writer_attach_fails_without_stealing_an_existing_lease() {
    let _fixture_guard = REAL_HOST_TEST_LOCK.lock().await;
    let seed = seed_store();
    insert_session(&seed, HostedSessionState::Live, false);
    let host = start(descriptor(&seed)).await.unwrap();
    let endpoint = LocalEndpoint::for_config_root(&seed.config_root, SESSION_ID);
    let cancel = CancellationToken::new();
    let mut owner = HostClient::connect(
        endpoint,
        ConnectOptions::local(SESSION_ID, [9; 32]),
        &cancel,
    )
    .await
    .unwrap();
    assert!(owner.get_state(&cancel).await.unwrap().has_writer_lease);

    let output = packaged_attach(&seed, true);
    assert_eq!(output.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&output.stdout).contains("writer lease"));
    assert!(owner.get_state(&cancel).await.unwrap().has_writer_lease);
    owner.disconnect();
    host.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_recorded_host_identity_is_rejected_before_attach_replay() {
    let _fixture_guard = REAL_HOST_TEST_LOCK.lock().await;
    let seed = seed_store();
    insert_session(&seed, HostedSessionState::Live, false);
    let actual_host_id = HostInstanceId::new();
    let mut descriptor = descriptor(&seed);
    descriptor.host_instance_id = actual_host_id;
    descriptor.session_dir = seed.temp.path().join("actual-attach-host");
    let host = start(descriptor).await.unwrap();
    let stale_host_id = HostInstanceId::new();
    assert_ne!(actual_host_id, stale_host_id);
    let _stale_metadata = write_ready_host_metadata(&seed, stale_host_id);

    let output = packaged_attach(&seed, false);
    assert_eq!(output.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&output.stdout).contains("identity changed"));
    assert_eq!(
        host.stats().await.lifecycle,
        multiplex_domain::HostLifecycle::Ready
    );
    host.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn packaged_human_attach_replays_writes_resizes_detaches_and_restores_terminal() {
    let _fixture_guard = REAL_HOST_TEST_LOCK.lock().await;
    let seed = seed_store();
    insert_session(&seed, HostedSessionState::Live, false);
    let host = start(descriptor(&seed)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

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
    let mut writer = pty.master.take_writer().unwrap();
    let output = Arc::new(Mutex::new(Vec::new()));
    let reader_thread = spawn_pty_reader(reader, Arc::clone(&output));
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_multiplex-cli"));
    command.args(["session", "attach", &SESSION_ID.to_string(), "--write"]);
    command.env("TERM", "xterm-256color");
    command.env("MULTIPLEX_CONFIG_DIR", &seed.config_root);
    let mut child = pty.slave.spawn_command(command).unwrap();
    drop(pty.slave);

    wait_for_output(
        &output,
        b"Attached to the durable local Host",
        Duration::from_secs(5),
    );
    wait_for_output(&output, SECRET_CANARY.as_bytes(), Duration::from_secs(5));
    pty.master
        .resize(PtySize {
            rows: 33,
            cols: 111,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    thread::sleep(Duration::from_millis(100));
    writer.write_all(b"size\r").unwrap();
    writer.flush().unwrap();
    wait_for_output(&output, b"SIZE:33 111", Duration::from_secs(5));
    writer.write_all(b"hello-from-cli\r").unwrap();
    writer.flush().unwrap();
    wait_for_output(&output, b"ECHO:hello-from-cli", Duration::from_secs(5));
    writer.write_all(b"\x1d").unwrap();
    writer.flush().unwrap();
    thread::sleep(Duration::from_millis(50));
    writer.write_all(b"d").unwrap();
    writer.flush().unwrap();

    let status = wait_for_child(child.as_mut());
    assert!(status.success(), "attach exited with {status:?}");
    drop(writer);
    reader_thread.join().unwrap();
    assert_eq!(pty.master.get_termios().unwrap(), before);
    assert_eq!(
        host.stats().await.lifecycle,
        multiplex_domain::HostLifecycle::Ready
    );
    host.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn packaged_human_attach_is_read_only_by_default_and_ctrl_c_restores_terminal() {
    let _fixture_guard = REAL_HOST_TEST_LOCK.lock().await;
    let seed = seed_store();
    insert_session(&seed, HostedSessionState::Live, false);
    let host = start(descriptor(&seed)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

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
    let mut writer = pty.master.take_writer().unwrap();
    let output = Arc::new(Mutex::new(Vec::new()));
    let reader_thread = spawn_pty_reader(reader, Arc::clone(&output));
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_multiplex-cli"));
    command.args(["session", "attach", &SESSION_ID.to_string()]);
    command.env("TERM", "xterm-256color");
    command.env("MULTIPLEX_CONFIG_DIR", &seed.config_root);
    let mut child = pty.slave.spawn_command(command).unwrap();
    drop(pty.slave);

    wait_for_output(&output, b"read-only observer", Duration::from_secs(5));
    thread::sleep(Duration::from_millis(50));
    writer.write_all(b"must-not-reach-host\r").unwrap();
    writer.flush().unwrap();
    thread::sleep(Duration::from_millis(200));
    assert!(
        !output
            .lock()
            .unwrap()
            .windows(b"ECHO:must-not-reach-host".len())
            .any(|window| window == b"ECHO:must-not-reach-host")
    );
    writer.write_all(b"\x03").unwrap();
    writer.flush().unwrap();

    let status = wait_for_child(child.as_mut());
    assert!(status.success(), "read-only attach exited with {status:?}");
    drop(writer);
    reader_thread.join().unwrap();
    assert_eq!(pty.master.get_termios().unwrap(), before);
    assert_eq!(
        host.stats().await.lifecycle,
        multiplex_domain::HostLifecycle::Ready
    );
    host.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn opening_a_stopped_console_reports_that_it_ended_without_a_generic_host_error() {
    let _fixture_guard = REAL_HOST_TEST_LOCK.lock().await;
    for stale_ready in [false, true] {
        let seed = seed_store();
        let session_dir = seed
            .config_root
            .join("console-sessions")
            .join(SESSION_ID.to_string());
        let mut launch = descriptor(&seed);
        launch.session_dir = session_dir.clone();
        let host = start(launch).await.unwrap();
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
        host.shutdown().await.unwrap();
        if stale_ready {
            let mut metadata = multiplex_store::read_host_metadata(&session_dir).unwrap();
            metadata.lifecycle = multiplex_domain::HostLifecycle::Ready;
            let lease =
                multiplex_store::HostLease::acquire(&session_dir, metadata.host_instance_id)
                    .unwrap();
            lease.write_metadata(&metadata).unwrap();
        }
        let before =
            std::fs::read(session_dir.join(multiplex_store::CONSOLE_SESSION_RECORD)).unwrap();
        let pty = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 100,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let output = Arc::new(Mutex::new(Vec::new()));
        let reading = spawn_pty_reader(pty.master.try_clone_reader().unwrap(), output.clone());
        let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_multiplex-cli"));
        command.args(["shell", "--raw", "--session-id", &SESSION_ID.to_string()]);
        command.env_remove("MULTIPLEX_SHELL_SESSION");
        command.env("TERM", "xterm-256color");
        command.env("MULTIPLEX_CONFIG_DIR", &seed.config_root);
        let mut child = ReapedChild(pty.slave.spawn_command(command).unwrap());
        drop(pty.slave);
        let status = wait_for_child(child.0.as_mut());
        reading.join().unwrap();
        assert!(!status.success());
        let output = output.lock().unwrap();
        let output = String::from_utf8_lossy(&output);
        assert!(
            output.contains("saved terminal session has already ended"),
            "{output}"
        );
        assert!(!output.contains("operation_failed"), "{output}");
        assert_eq!(
            std::fs::read(session_dir.join(multiplex_store::CONSOLE_SESSION_RECORD)).unwrap(),
            before
        );
    }
}

const BROWSER_CHECKLIST: &str =
    include_str!("../../../tests/fixtures/terminal/browser-checklist.txt");

#[derive(Default)]
struct PasteScenario {
    payloads: Vec<String>,
    fragmented: bool,
    echo: bool,
    idle_between: bool,
    interrupt: bool,
}

impl PasteScenario {
    fn text(text: String) -> Self {
        Self {
            payloads: vec![text],
            ..Self::default()
        }
    }

    fn boundaries() -> Self {
        // Exercise the old CLI cap, raw stdin buffer, and negotiated wire limit on
        // both sides, including when a UTF-8 code point straddles a raw read.
        let limit = multiplex_host_protocol::MAX_OUTPUT_BYTES;
        let mut lengths = vec![0, 1, 16_383, 16_384, 16_385, limit - 1, limit, limit + 1];
        lengths.sort_unstable();
        lengths.dedup();
        Self {
            payloads: lengths
                .into_iter()
                .map(|len| {
                    if len < 4 {
                        "x".repeat(len)
                    } else {
                        format!("{}₹\n", "x".repeat(len - 4))
                    }
                })
                .chain([String::new(), "after-empty-paste\n".into()])
                .collect(),
            ..Self::default()
        }
    }

    fn repeated() -> Self {
        Self {
            payloads: (0..40)
                .map(|n| format!("paste-{n:03}\n{BROWSER_CHECKLIST}\nEND-{n:03}"))
                .collect(),
            idle_between: true,
            ..Self::default()
        }
    }

    fn megabytes() -> Self {
        Self {
            payloads: vec![BROWSER_CHECKLIST.repeat(2 * 1024 * 1024 / BROWSER_CHECKLIST.len() + 1)],
            echo: true,
            ..Self::default()
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_preserves_multiline_unicode_paste_and_bracket_markers() {
    check_shell_paste(false, PasteScenario::text(BROWSER_CHECKLIST.into())).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_streams_pastes_larger_than_one_input_request() {
    check_shell_paste(false, PasteScenario::text(BROWSER_CHECKLIST.repeat(20))).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn raw_terminal_bridge_preserves_large_multiline_unicode_paste() {
    check_shell_paste(true, PasteScenario::text(BROWSER_CHECKLIST.repeat(20))).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_preserves_empty_and_request_boundary_pastes() {
    check_shell_paste(false, PasteScenario::boundaries()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn raw_terminal_bridge_preserves_empty_and_request_boundary_pastes() {
    check_shell_paste(true, PasteScenario::boundaries()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_preserves_fragmented_markers_and_unicode() {
    check_shell_paste(
        false,
        PasteScenario {
            fragmented: true,
            ..PasteScenario::text(BROWSER_CHECKLIST.into())
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn raw_terminal_bridge_preserves_fragmented_markers_and_unicode() {
    check_shell_paste(
        true,
        PasteScenario {
            fragmented: true,
            ..PasteScenario::text(BROWSER_CHECKLIST.into())
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_preserves_repeated_pastes_across_idle_writer_release() {
    check_shell_paste(false, PasteScenario::repeated()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn raw_terminal_bridge_preserves_repeated_pastes_across_idle_writer_release() {
    check_shell_paste(true, PasteScenario::repeated()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_preserves_megabyte_paste_while_output_streams_back() {
    check_shell_paste(false, PasteScenario::megabytes()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn raw_terminal_bridge_preserves_megabyte_paste_while_output_streams_back() {
    check_shell_paste(true, PasteScenario::megabytes()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupted_raw_paste_releases_connections_and_owned_processes() {
    check_shell_paste(
        true,
        PasteScenario {
            interrupt: true,
            ..PasteScenario::megabytes()
        },
    )
    .await;
}

async fn check_shell_paste(raw: bool, scenario: PasteScenario) {
    let _fixture_guard = REAL_HOST_TEST_LOCK.lock().await;
    let seed = seed_store();
    let frames: Vec<Vec<u8>> = scenario
        .payloads
        .iter()
        .map(|text| [b"\x1b[200~".as_slice(), text.as_bytes(), b"\x1b[201~"].concat())
        .collect();
    let expected = frames.concat();
    let received = seed.temp.path().join("pasted-bytes");
    let mut launch = descriptor(&seed);
    launch.session_dir = seed
        .config_root
        .join("console-sessions")
        .join(SESSION_ID.to_string());
    let capture = if scenario.echo {
        // tee gives the bridge concurrent output pressure while head bounds the capture.
        format!("head -c {} | tee '{}'", expected.len(), received.display())
    } else {
        format!("head -c {} > '{}'", expected.len(), received.display())
    };
    launch.arguments = vec![
        "-c".into(),
        format!(
            "stty raw -echo; printf '\\033[?2004hPASTE-READY'; {capture}; printf 'PASTE-DONE'; sleep 30"
        ),
    ];
    let session_dir = launch.session_dir.clone();
    let endpoint = LocalEndpoint::new(&launch.runtime_root, SESSION_ID)
        .socket_path()
        .to_path_buf();
    let host = start(launch).await.unwrap();
    let shell_pid = multiplex_store::read_host_metadata(&session_dir)
        .unwrap()
        .process_token
        .unwrap()
        .platform_identity();
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
    let pty = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let reader = pty.master.try_clone_reader().unwrap();
    let mut writer = pty.master.take_writer().unwrap();
    let output = Arc::new(Mutex::new(Vec::new()));
    let reader_thread = spawn_pty_reader(reader, Arc::clone(&output));
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_multiplex-cli"));
    command.arg("shell");
    if raw {
        command.arg("--raw");
    }
    command.args(["--session-id", &SESSION_ID.to_string()]);
    command.env_remove("MULTIPLEX_SHELL_SESSION");
    command.env("TERM", "xterm-256color");
    command.env("MULTIPLEX_CONFIG_DIR", &seed.config_root);
    #[cfg(target_os = "macos")]
    if std::env::var_os("MULTIPLEX_PASTE_LEAKS").is_some() {
        command.env("MallocStackLogging", "1");
    }
    let mut child_guard = ReapedChild(pty.slave.spawn_command(command).unwrap());
    let child = &mut child_guard.0;
    drop(pty.slave);
    wait_for_output(&output, b"PASTE-READY", Duration::from_secs(5));
    thread::sleep(Duration::from_millis(50));
    let cli_pid = child.process_id().unwrap();
    let rss_before = process_rss_kib(cli_pid);
    let started = Instant::now();
    let mut send = || -> std::io::Result<()> {
        if scenario.interrupt {
            writer.write_all(&expected[..1024])?;
        } else {
            for (index, frame) in frames.iter().enumerate() {
                if scenario.fragmented {
                    // One-byte writes with delays force markers and multi-byte UTF-8
                    // through separate reads, rather than relying on kernel chunking.
                    for byte in frame {
                        writer.write_all(&[*byte])?;
                        thread::sleep(Duration::from_micros(100));
                    }
                } else {
                    writer.write_all(frame)?;
                }
                writer.flush()?;
                if scenario.idle_between && index == 0 {
                    thread::sleep(Duration::from_millis(1_800));
                }
            }
        }
        writer.flush()
    };
    let sent = send();
    let deadline = Instant::now() + Duration::from_secs(15);
    if !scenario.interrupt {
        while Instant::now() < deadline {
            if std::fs::metadata(&received).is_ok_and(|m| m.len() == expected.len() as u64)
                || child.try_wait().unwrap().is_some()
            {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
    let rss_after = process_rss_kib(cli_pid);
    #[cfg(target_os = "macos")]
    let leak_scan = std::env::var_os("MULTIPLEX_PASTE_LEAKS").map(|_| {
        Command::new("/usr/bin/leaks")
            .args(["--quiet", "--noContent", "--nostacks", &cli_pid.to_string()])
            .output()
            .expect("run macOS native leak scanner")
    });
    let actual = std::fs::read(&received).unwrap_or_default();
    let _ = child.kill();
    let _ = child.wait();
    drop(writer);
    reader_thread.join().unwrap();
    let cleanup_deadline = Instant::now() + Duration::from_secs(2);
    while host.stats().await.active_connections != 0 && Instant::now() < cleanup_deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let connections = host.stats().await.active_connections;
    host.shutdown().await.unwrap();
    sent.unwrap();
    #[cfg(target_os = "macos")]
    if let Some(scan) = leak_scan {
        let report = format!(
            "{}{}",
            String::from_utf8_lossy(&scan.stdout),
            String::from_utf8_lossy(&scan.stderr)
        );
        assert!(scan.status.success(), "native leak scan failed: {report}");
        eprintln!("native leak scan: {report}");
    }
    assert_eq!(
        connections, 0,
        "CLI connection leaked after its frontend exited"
    );
    assert!(!endpoint.exists(), "Host endpoint leaked after shutdown");
    assert_eq!(
        multiplex_store::probe_host_lease(&session_dir).unwrap(),
        multiplex_store::HostLeaseState::Available
    );
    // SAFETY: signal 0 only probes the known, reaped child's process id.
    assert_eq!(
        unsafe { libc::kill(shell_pid.try_into().unwrap(), 0) },
        -1,
        "owned shell leaked"
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
    if !scenario.interrupt {
        assert_eq!(actual.len(), expected.len(), "paste cropped; raw={raw}");
        assert!(
            actual == expected,
            "paste changed at byte {:?}; raw={raw}",
            actual.iter().zip(&expected).position(|(a, b)| a != b)
        );
    }
    assert!(
        rss_after.saturating_sub(rss_before) < 64 * 1024,
        "CLI RSS grew excessively: {rss_before} -> {rss_after} KiB"
    );
    eprintln!(
        "paste stress: raw={raw} bytes={} pastes={} elapsed={:?} rss={} -> {} KiB; connections={connections}; shell reaped",
        expected.len(),
        frames.len(),
        started.elapsed(),
        rss_before,
        rss_after
    );
}

struct ReapedChild(Box<dyn portable_pty::Child + Send + Sync>);

impl Drop for ReapedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn process_rss_kib(pid: u32) -> u64 {
    let result = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    assert!(result.status.success(), "CLI exited during stress test");
    String::from_utf8(result.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

fn attach_command(request_control: bool) -> CliCommand {
    CliCommand::SessionAttach {
        session_id: SESSION_ID,
        from_sequence: OutputSequence::new(FROM_SEQUENCE),
        columns: COLUMNS,
        rows: ROWS,
        request_control,
    }
}

fn packaged_attach(seed: &SeededStore, request_control: bool) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_multiplex-cli"));
    command.args([
        "session",
        "attach",
        &SESSION_ID.to_string(),
        "--columns",
        &COLUMNS.to_string(),
        "--rows",
        &ROWS.to_string(),
        "--json",
    ]);
    if request_control {
        command.arg("--write");
    }
    command
        .env("MULTIPLEX_CONFIG_DIR", &seed.config_root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap()
}

fn descriptor(seed: &SeededStore) -> LaunchDescriptor {
    LaunchDescriptor {
        format_version: LaunchDescriptor::FORMAT_VERSION,
        session_id: SESSION_ID,
        host_instance_id: HostInstanceId::new(),
        expected_occupant_generation: None,
        runtime_root: LocalEndpoint::for_config_root(&seed.config_root, SESSION_ID)
            .runtime_root()
            .to_path_buf(),
        session_dir: seed
            .config_root
            .join("durable-sessions")
            .join(SESSION_ID.to_string()),
        executable: std::fs::canonicalize("/bin/sh").unwrap(),
        runtime_detection: None,
        arguments: vec![
            "-c".into(),
            format!(
                "printf '{SECRET_CANARY}\\n'; while IFS= read -r line; do if [ \"$line\" = size ]; then printf 'SIZE:'; stty size; else printf 'ECHO:%s\\n' \"$line\"; fi; done"
            ),
        ],
        environment: BTreeMap::from([("PATH".into(), "/usr/bin:/bin".into())]),
        cwd: Some(seed.folder.clone()),
        columns: 80,
        rows: 24,
        journal_limits: JournalLimits::default(),
        stop_deadlines: StopDeadlines::default(),
    }
}

fn spawn_pty_reader(
    mut reader: Box<dyn std::io::Read + Send>,
    output: Arc<Mutex<Vec<u8>>>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut chunk = [0_u8; 4_096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    let mut output = output.lock().unwrap();
                    output.extend_from_slice(&chunk[..count]);
                    let discard = output.len().saturating_sub(64 * 1024);
                    output.drain(..discard);
                }
            }
        }
    })
}

fn wait_for_output(output: &Mutex<Vec<u8>>, expected: &[u8], timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        let found = output
            .lock()
            .unwrap()
            .windows(expected.len())
            .any(|window| window == expected);
        if found {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {:?}; output: {}",
            String::from_utf8_lossy(expected),
            String::from_utf8_lossy(&output.lock().unwrap())
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_child(child: &mut dyn portable_pty::Child) -> ExitStatus {
    for _ in 0..100 {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        thread::sleep(Duration::from_millis(50));
    }
    child.kill().unwrap();
    let _ = child.wait();
    panic!("interactive attach did not exit within five seconds");
}
