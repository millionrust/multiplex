mod common;

use std::sync::{Arc, Mutex};

use common::*;
use termirust_cli::{
    Cancellation, CliData, CliError, ControllerSshAction, ControllerSshCommand, ControllerSshData,
    ErrorCode, SshControllerCommandExecutor, parse_args, run,
};

#[derive(Default)]
struct FakeRemoteState {
    operations: Vec<&'static str>,
}

struct FakeRemoteController {
    state: Arc<Mutex<FakeRemoteState>>,
}

impl SshControllerCommandExecutor for FakeRemoteController {
    fn execute(
        &self,
        command: ControllerSshCommand,
        _cancellation: &Cancellation,
    ) -> Result<CliData, CliError> {
        let operation = match command.action {
            ControllerSshAction::Pair => {
                return Err(CliError::new(
                    ErrorCode::InteractionRequired,
                    "pairing requires confirmation on both devices",
                    "Run this command in a terminal and confirm the matching code on the Host.",
                ));
            }
            ControllerSshAction::Sessions => "sessions",
            ControllerSshAction::Attach { .. } => "attach",
            ControllerSshAction::Input { .. } => "input",
            ControllerSshAction::Resize { .. } => "resize",
            ControllerSshAction::Approval { .. } => "approval",
            ControllerSshAction::Detach { .. } => "detach",
        };
        self.state.lock().unwrap().operations.push(operation);
        Ok(CliData::ControllerSsh(ControllerSshData {
            operation: operation.into(),
            route_state: "ready".into(),
            target_label: "SSH target".into(),
            ssh_host_key: "matched".into(),
            host_fingerprint_suffix: Some("7M2K".into()),
            capabilities: vec!["observe_sessions".into(), "attach_output".into()],
            session_generation: Some(7),
            writer_lease: Some("observer".into()),
            reconnect_attempt: None,
            reconnect_deadline_millis: None,
        }))
    }
}

fn base_args(action: &[&str]) -> Vec<String> {
    [
        "controller",
        "ssh",
        "--host",
        "private.example",
        "--user",
        "operator",
        "--port",
        "2202",
    ]
    .into_iter()
    .chain(action.iter().copied())
    .map(str::to_string)
    .collect()
}

#[test]
fn json_actions_are_stable_redacted_and_delegate_once() {
    let seed = seed_store();
    let remote = Arc::new(Mutex::new(FakeRemoteState::default()));
    let mut service = service(
        &seed,
        Arc::new(Mutex::new(FakeLauncherState::default())),
        Arc::new(Mutex::new(FakeControllerState::default())),
    )
    .with_ssh_controller(Arc::new(FakeRemoteController {
        state: Arc::clone(&remote),
    }));
    let session = SESSION_ID.to_string();
    let approval = "00000000-0000-0000-0000-000000000009";
    let actions = [
        vec!["sessions"],
        vec![
            "attach",
            "--session",
            &session,
            "--generation",
            "7",
            "--write",
        ],
        vec!["input", "--session", &session, "--generation", "7"],
        vec![
            "resize",
            "--session",
            &session,
            "--generation",
            "7",
            "--columns",
            "120",
            "--rows",
            "40",
        ],
        vec![
            "approval",
            "--session",
            &session,
            "--generation",
            "7",
            "--approval",
            approval,
            "--decision",
            "deny",
        ],
        vec!["detach", "--session", &session, "--generation", "7"],
    ];
    for action in actions {
        let mut arguments = base_args(&action);
        arguments.push("--json".into());
        let output = run(&mut service, arguments, 80, &Cancellation::default());
        assert_eq!(output.exit_code, 0);
        assert!(output.stderr.is_empty());
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(!text.contains("private.example"));
        assert!(!text.contains("operator"));
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["ok"], true);
        assert_eq!(value["data"]["target_label"], "SSH target");
    }
    assert_eq!(
        remote.lock().unwrap().operations,
        [
            "sessions", "attach", "input", "resize", "approval", "detach"
        ]
    );
}

#[test]
fn json_pairing_never_prompts_and_returns_interaction_required() {
    let seed = seed_store();
    let mut service = service(
        &seed,
        Arc::new(Mutex::new(FakeLauncherState::default())),
        Arc::new(Mutex::new(FakeControllerState::default())),
    )
    .with_ssh_controller(Arc::new(FakeRemoteController {
        state: Arc::new(Mutex::new(FakeRemoteState::default())),
    }));
    let mut arguments = base_args(&["pair"]);
    arguments.push("--json".into());
    let output = run(&mut service, arguments, 80, &Cancellation::default());
    assert_eq!(output.exit_code, 4);
    assert!(output.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], "interaction_required");
}

#[test]
fn parser_rejects_injection_missing_generation_and_input_argv_payload() {
    let session = SESSION_ID.to_string();
    for arguments in [
        base_args(&["sessions", "--unexpected"]),
        base_args(&["input", "--session", &session, "--generation", "0"]),
        base_args(&[
            "input",
            "--session",
            &session,
            "--generation",
            "1",
            "secret-payload",
        ]),
        base_args(&[
            "resize",
            "--session",
            &session,
            "--generation",
            "1",
            "--columns",
            "0",
            "--rows",
            "24",
        ]),
        vec![
            "controller",
            "ssh",
            "--host",
            "-oProxyCommand=bad",
            "sessions",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
    ] {
        assert_eq!(parse_args(arguments).unwrap_err().code, ErrorCode::Usage);
    }
}
