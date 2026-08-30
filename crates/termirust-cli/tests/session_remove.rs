mod common;

use std::fs;
use std::io::Write as _;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use common::*;
use termirust_cli::{
    Cancellation, ErrorCode, Invocation, RemovalConfirmation, parse_args,
    read_removal_confirmation, run, run_parsed,
};
use termirust_domain::{HostedSessionState, SessionMutation};

fn arguments(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

fn json_preview(
    service: &mut termirust_cli::LocalCommandService,
    cancellation: &Cancellation,
) -> serde_json::Value {
    let output = run(
        service,
        arguments(&["session", "remove", &SESSION_ID.to_string(), "--json"]),
        80,
        cancellation,
    );
    assert_eq!(output.exit_code, 0);
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).unwrap()
}

fn committed_command(token: &str, confirmation: &str) -> Invocation {
    let invocation = parse_args(arguments(&[
        "session",
        "remove",
        &SESSION_ID.to_string(),
        "--preview-token",
        token,
        "--yes",
        "--confirmation-stdin",
        "--json",
    ]))
    .unwrap();
    Invocation {
        json: invocation.json,
        command: invocation
            .command
            .with_removal_confirmation(RemovalConfirmation::new(confirmation.into()).unwrap())
            .unwrap(),
    }
}

#[test]
fn preview_is_non_mutating_and_packaged_binary_quarantines_exact_owned_data() {
    let seed = seed_store();
    insert_session(&seed, HostedSessionState::Exited, true);
    let session_root = seed
        .config_root
        .join("durable-sessions")
        .join(SESSION_ID.to_string());
    fs::create_dir_all(session_root.join("transcripts")).unwrap();
    fs::write(session_root.join("journal.trj"), b"journal").unwrap();
    fs::write(
        session_root.join("transcripts").join("events.jsonl"),
        b"private transcript fixture",
    )
    .unwrap();
    let sentinel = seed.temp.path().join("unrelated-sentinel");
    fs::write(&sentinel, b"must-survive").unwrap();
    let binary = env!("CARGO_BIN_EXE_termirust-cli");

    let preview = Command::new(binary)
        .args(["session", "remove", &SESSION_ID.to_string(), "--json"])
        .env("TERMIRUST_CONFIG_DIR", &seed.config_root)
        .output()
        .unwrap();
    assert!(preview.status.success());
    assert!(preview.stderr.is_empty());
    let preview: serde_json::Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_eq!(preview["data"]["confirmation"], "session_title");
    assert_eq!(preview["data"]["journal_bytes"], 7);
    assert_eq!(preview["data"]["transcript_bytes"], 26);
    let token = preview["data"]["preview_token"].as_str().unwrap();
    assert!(!token.contains("Counter session"));
    assert_eq!(seed.sessions().load().unwrap().sessions.len(), 1);

    let mut child = Command::new(binary)
        .args([
            "session",
            "remove",
            &SESSION_ID.to_string(),
            "--preview-token",
            token,
            "--yes",
            "--confirmation-stdin",
            "--json",
        ])
        .env("TERMIRUST_CONFIG_DIR", &seed.config_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"Counter session\n")
        .unwrap();
    let committed = child.wait_with_output().unwrap();
    assert!(committed.status.success());
    assert!(committed.stderr.is_empty());
    let committed: serde_json::Value = serde_json::from_slice(&committed.stdout).unwrap();
    assert_eq!(committed["data"]["outcome"], "removed");
    assert!(seed.sessions().load().unwrap().sessions.is_empty());
    assert!(!session_root.exists());
    assert!(
        seed.config_root
            .join("durable-session-quarantine")
            .join(SESSION_ID.to_string())
            .is_dir()
    );
    assert_eq!(fs::read(sentinel).unwrap(), b"must-survive");
}

#[test]
fn commit_requires_injected_stdin_confirmation_and_rejects_manifest_race() {
    let seed = seed_store();
    insert_session(&seed, HostedSessionState::Exited, true);
    let session_root = seed
        .config_root
        .join("durable-sessions")
        .join(SESSION_ID.to_string());
    fs::create_dir_all(&session_root).unwrap();
    let mut service = service(
        &seed,
        Arc::new(Mutex::new(FakeLauncherState::default())),
        Arc::new(Mutex::new(FakeControllerState::default())),
    );
    let cancellation = Cancellation::default();
    let preview = json_preview(&mut service, &cancellation);
    let token = preview["data"]["preview_token"].as_str().unwrap();

    let missing_input = run(
        &mut service,
        arguments(&[
            "session",
            "remove",
            &SESSION_ID.to_string(),
            "--preview-token",
            token,
            "--yes",
            "--confirmation-stdin",
            "--json",
        ]),
        80,
        &cancellation,
    );
    assert_eq!(
        missing_input.exit_code,
        ErrorCode::InteractionRequired.exit_code()
    );
    assert_eq!(seed.sessions().load().unwrap().sessions.len(), 1);

    fs::write(session_root.join("changed-after-preview"), b"changed").unwrap();
    let stale = run_parsed(
        Some(&mut service),
        committed_command(token, "Counter session"),
        80,
        &cancellation,
    );
    assert_eq!(stale.exit_code, ErrorCode::Conflict.exit_code());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&stale.stdout).unwrap()["error"]["code"],
        "conflict"
    );
    assert_eq!(seed.sessions().load().unwrap().sessions.len(), 1);
    assert!(session_root.is_dir());
}

#[test]
fn commit_rejects_metadata_race_malformed_tokens_and_cancellation() {
    let seed = seed_store();
    insert_session(&seed, HostedSessionState::Exited, true);
    let mut service = service(
        &seed,
        Arc::new(Mutex::new(FakeLauncherState::default())),
        Arc::new(Mutex::new(FakeControllerState::default())),
    );
    let cancellation = Cancellation::default();
    let preview = json_preview(&mut service, &cancellation);
    let token = preview["data"]["preview_token"].as_str().unwrap();

    let repository = seed.sessions();
    let current = repository.load().unwrap().sessions[0].clone();
    repository
        .mutate_session(
            SESSION_ID,
            repository.load().unwrap().revision,
            SessionMutation::SetPinned(true),
            current.updated_at + 1,
        )
        .unwrap();
    let stale = run_parsed(
        Some(&mut service),
        committed_command(token, "REMOVE"),
        80,
        &cancellation,
    );
    assert_eq!(stale.exit_code, ErrorCode::Conflict.exit_code());

    let malformed = run_parsed(
        Some(&mut service),
        committed_command("tr-remove-v1:01:0:0:0:0:0", "REMOVE"),
        80,
        &cancellation,
    );
    assert_eq!(malformed.exit_code, ErrorCode::Validation.exit_code());

    let cancelled = Cancellation::default();
    cancelled.cancel();
    let output = run(
        &mut service,
        arguments(&["session", "remove", &SESSION_ID.to_string(), "--json"]),
        80,
        &cancelled,
    );
    assert_eq!(output.exit_code, ErrorCode::Cancelled.exit_code());
    assert_eq!(repository.load().unwrap().sessions.len(), 1);
}

#[test]
fn metadata_only_cli_removal_requires_literal_remove() {
    let seed = seed_store();
    insert_session(&seed, HostedSessionState::Exited, true);
    let mut service = service(
        &seed,
        Arc::new(Mutex::new(FakeLauncherState::default())),
        Arc::new(Mutex::new(FakeControllerState::default())),
    );
    let cancellation = Cancellation::default();
    let preview = json_preview(&mut service, &cancellation);
    assert_eq!(preview["data"]["confirmation"], "remove");
    let token = preview["data"]["preview_token"].as_str().unwrap();

    let mismatch = run_parsed(
        Some(&mut service),
        committed_command(token, "Counter session"),
        80,
        &cancellation,
    );
    assert_eq!(mismatch.exit_code, ErrorCode::Validation.exit_code());
    assert_eq!(seed.sessions().load().unwrap().sessions.len(), 1);

    let removed = run_parsed(
        Some(&mut service),
        committed_command(token, "REMOVE"),
        80,
        &cancellation,
    );
    assert_eq!(removed.exit_code, 0);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&removed.stdout).unwrap()["data"]["outcome"],
        "removed"
    );
    assert!(seed.sessions().load().unwrap().sessions.is_empty());
}

#[test]
fn stdin_confirmation_is_bounded_utf8_single_line_and_debug_redacted() {
    let confirmation =
        read_removal_confirmation(&mut &b"PRIVATE-CONFIRMATION-CANARY\r\n"[..]).unwrap();
    assert_eq!(confirmation.expose(), "PRIVATE-CONFIRMATION-CANARY");
    assert!(!format!("{confirmation:?}").contains("PRIVATE-CONFIRMATION-CANARY"));
    let command = parse_args(arguments(&[
        "session",
        "remove",
        &SESSION_ID.to_string(),
        "--preview-token",
        "tr-remove-v1:1:0:0:0:0:0",
        "--yes",
        "--confirmation-stdin",
    ]))
    .unwrap()
    .command
    .with_removal_confirmation(confirmation)
    .unwrap();
    assert!(!format!("{command:?}").contains("PRIVATE-CONFIRMATION-CANARY"));

    let exact = "x".repeat(256);
    assert_eq!(
        read_removal_confirmation(&mut format!("{exact}\n").as_bytes())
            .unwrap()
            .expose(),
        exact
    );
    assert_eq!(
        read_removal_confirmation(&mut format!("{}\n", "x".repeat(257)).as_bytes())
            .unwrap_err()
            .code,
        ErrorCode::Validation
    );
    assert_eq!(
        read_removal_confirmation(&mut &b"first\nsecond\n"[..])
            .unwrap_err()
            .code,
        ErrorCode::Validation
    );
    assert_eq!(
        read_removal_confirmation(&mut &[0xff, b'\n'][..])
            .unwrap_err()
            .code,
        ErrorCode::Validation
    );
}
