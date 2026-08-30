mod common;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use common::*;
use termirust_cli::{
    Cancellation, CliData, LocalCommandService, RenderOptions, SessionInputData, SessionResizeData,
    render_success, run,
};

fn json(
    service: &mut LocalCommandService,
    arguments: &[&str],
    cancellation: &Cancellation,
) -> serde_json::Value {
    let mut arguments = arguments
        .iter()
        .map(|argument| (*argument).to_string())
        .collect::<Vec<_>>();
    arguments.push("--json".into());
    let output = run(service, arguments, 80, cancellation);
    assert_eq!(
        output.exit_code,
        0,
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn json_v1_golden() {
    let seed = seed_store();
    insert_session(&seed, termirust_domain::HostedSessionState::Exited, false);
    let mut service = service(
        &seed,
        Arc::new(Mutex::new(FakeLauncherState::default())),
        Arc::new(Mutex::new(FakeControllerState::default())),
    );
    let cancellation = Cancellation::default();
    let mut actual = BTreeMap::<String, serde_json::Value>::new();
    actual.insert(
        "help".into(),
        json(&mut service, &["--help"], &cancellation),
    );
    actual.insert(
        "status".into(),
        json(&mut service, &["status"], &cancellation),
    );
    actual.insert(
        "project_list".into(),
        json(&mut service, &["project", "list"], &cancellation),
    );
    actual.insert(
        "preset_list".into(),
        json(
            &mut service,
            &["preset", "list", "--project", &PROJECT_ID.to_string()],
            &cancellation,
        ),
    );
    actual.insert(
        "session_list".into(),
        json(&mut service, &["session", "list"], &cancellation),
    );
    actual.insert(
        "session_show".into(),
        json(
            &mut service,
            &["session", "show", &SESSION_ID.to_string()],
            &cancellation,
        ),
    );
    actual.insert(
        "session_wait".into(),
        json(
            &mut service,
            &[
                "session",
                "wait",
                &SESSION_ID.to_string(),
                "--state",
                "exited",
            ],
            &cancellation,
        ),
    );
    actual.insert(
        "session_archive".into(),
        json(
            &mut service,
            &["session", "archive", &SESSION_ID.to_string()],
            &cancellation,
        ),
    );
    actual.insert(
        "session_restore".into(),
        json(
            &mut service,
            &["session", "restore", &SESSION_ID.to_string()],
            &cancellation,
        ),
    );
    let input = render_success(
        &CliData::Input(SessionInputData {
            session_id: SESSION_ID.to_string(),
            accepted_bytes: 13,
            applied: true,
        }),
        &[],
        RenderOptions {
            json: true,
            terminal_width: 80,
        },
    )
    .unwrap();
    actual.insert(
        "session_input".into(),
        serde_json::from_slice(&input).unwrap(),
    );
    let resize = render_success(
        &CliData::Resize(SessionResizeData {
            session_id: SESSION_ID.to_string(),
            columns: 132,
            rows: 43,
            applied: true,
        }),
        &[],
        RenderOptions {
            json: true,
            terminal_width: 80,
        },
    )
    .unwrap();
    actual.insert(
        "session_resize".into(),
        serde_json::from_slice(&resize).unwrap(),
    );
    actual.insert(
        "session_launch".into(),
        json(
            &mut service,
            &[
                "session",
                "launch",
                "--project",
                &PROJECT_ID.to_string(),
                "--preset",
                &PRESET_ID.to_string(),
            ],
            &cancellation,
        ),
    );
    actual.insert(
        "session_stop".into(),
        json(
            &mut service,
            &["session", "stop", &LAUNCH_SESSION_ID.to_string(), "--yes"],
            &cancellation,
        ),
    );

    let expected: BTreeMap<String, serde_json::Value> = serde_json::from_str(include_str!(
        "../../../tests/fixtures/cli/v1/responses.json"
    ))
    .unwrap();
    if actual != expected {
        eprintln!("{}", serde_json::to_string_pretty(&actual).unwrap());
    }
    assert_eq!(actual, expected);
}
