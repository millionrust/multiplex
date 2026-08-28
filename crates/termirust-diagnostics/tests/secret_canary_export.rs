mod common;

use std::fs;

use termirust_diagnostics::{
    DiagnosticBundle, DiagnosticCode, DiagnosticPolicy, ExportCancellation, ExportErrorCode,
};

use common::{runtime, safe_diagnostic};

const CANARIES: &[&str] = &[
    "TERMIRUST_TERMINAL_CANARY_7EFA",
    "TERMIRUST_PROMPT_CANARY_1A92",
    "password=client-secret",
    "Bearer client-token",
    "/Users/private/client-project",
    "client-user@production.internal",
    "-----BEGIN OPENSSH PRIVATE KEY-----",
];

#[test]
fn preview_and_export_contain_zero_content_canaries() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = runtime(temp.path(), DiagnosticPolicy::default());
    let handle = runtime.handle();
    assert!(handle.record(safe_diagnostic(10, DiagnosticCode::AppStarted)));
    let prepared = handle.prepare_export().unwrap();
    assert_eq!(prepared.manifest().total_entries, 1);
    assert_eq!(prepared.manifest().redactions, 0);
    assert!(
        prepared
            .manifest()
            .excluded_classes
            .iter()
            .any(|class| class.contains("terminal input"))
    );

    let destination = temp.path().join("support-bundle.json");
    prepared
        .publish(&destination, &ExportCancellation::default())
        .unwrap();
    let bytes = fs::read(&destination).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    for canary in CANARIES {
        assert!(!text.contains(canary), "bundle leaked canary {canary}");
    }
    let bundle: DiagnosticBundle = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(bundle.manifest.total_entries, 1);
    assert_eq!(bundle.files[0].entries.len(), 1);
}

#[test]
fn unknown_or_secret_bearing_source_entry_fails_closed() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = runtime(temp.path(), DiagnosticPolicy::default());
    let handle = runtime.handle();
    handle.flush().unwrap();
    fs::write(
        temp.path().join("diagnostics-0.jsonl"),
        br#"{"schema_version":1,"occurred_at_unix_ms":10,"code":"app_started","severity":"info","user_message_id":"app_lifecycle","recovery":[],"correlation_id":"00000000000000000000000000000000","safe_context":{},"terminal_output":"TERMIRUST_TERMINAL_CANARY_7EFA"}
"#,
    )
    .unwrap();
    let error = match handle.prepare_export() {
        Ok(_) => panic!("unsafe source unexpectedly prepared"),
        Err(error) => error,
    };
    assert_eq!(error.code, ExportErrorCode::MalformedEntry);
}

#[test]
fn exported_schema_rejects_unknown_fields() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = runtime(temp.path(), DiagnosticPolicy::default());
    let handle = runtime.handle();
    handle.record(safe_diagnostic(10, DiagnosticCode::AppStarted));
    let prepared = handle.prepare_export().unwrap();
    let destination = temp.path().join("bundle.json");
    prepared
        .publish(&destination, &ExportCancellation::default())
        .unwrap();
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(destination).unwrap()).unwrap();
    value["terminal_output"] = serde_json::Value::String("secret".into());
    assert!(serde_json::from_value::<DiagnosticBundle>(value).is_err());
}
