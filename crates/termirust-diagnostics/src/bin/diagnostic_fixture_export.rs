use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use serde::Deserialize;
use termirust_diagnostics::{
    Component, Diagnostic, DiagnosticCode, DiagnosticMessageId, DiagnosticPolicy,
    DiagnosticRuntime, ExportCancellation, Operation, SafeField, SafeValue, Severity,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    schema_version: u16,
    policy: FixturePolicy,
    canaries: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FixturePolicy {
    max_file_mib: u8,
    max_files: u8,
    retention_days: u8,
    bundle_mib: u8,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => {
            eprintln!("diagnostic fixture export failed: {code}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), &'static str> {
    let mut args = std::env::args_os().skip(1);
    let fixture_path = PathBuf::from(args.next().ok_or("missing_fixture")?);
    let output_path = PathBuf::from(args.next().ok_or("missing_output")?);
    if args.next().is_some() {
        return Err("unexpected_argument");
    }
    let bytes = fs::read(&fixture_path).map_err(|_| "fixture_read")?;
    if bytes.len() > 64 * 1024 {
        return Err("fixture_oversized");
    }
    let fixture: Fixture = serde_json::from_slice(&bytes).map_err(|_| "fixture_malformed")?;
    if fixture.schema_version != 1
        || fixture.policy.max_file_mib != 10
        || fixture.policy.max_files != 5
        || fixture.policy.retention_days != 14
        || fixture.policy.bundle_mib != 50
        || fixture.canaries.is_empty()
    {
        return Err("fixture_policy_mismatch");
    }

    let root = std::env::temp_dir().join(format!(
        "termirust-diagnostic-fixture-{}-{}",
        std::process::id(),
        now_ms()
    ));
    let runtime = DiagnosticRuntime::start(
        &root,
        DiagnosticPolicy {
            max_file_bytes: u64::from(fixture.policy.max_file_mib) * 1024 * 1024,
            max_files: fixture.policy.max_files,
            retention_days: fixture.policy.retention_days,
            ..DiagnosticPolicy::default()
        },
    )
    .map_err(|_| "runtime_start")?;
    let handle = runtime.handle();
    let mut diagnostic = Diagnostic::new(
        now_ms(),
        DiagnosticCode::AppStarted,
        Severity::Info,
        DiagnosticMessageId::AppLifecycle,
    );
    diagnostic
        .insert(
            SafeField::Component,
            SafeValue::Component(Component::Application),
        )
        .map_err(|_| "schema")?;
    diagnostic
        .insert(SafeField::Operation, SafeValue::Operation(Operation::Start))
        .map_err(|_| "schema")?;
    if !handle.record(diagnostic) {
        return Err("record");
    }
    let preview = handle.prepare_export().map_err(|_| "preview")?;
    preview
        .publish(&output_path, &ExportCancellation::default())
        .map_err(|_| "publish")?;
    let output = fs::read(&output_path).map_err(|_| "output_read")?;
    if fixture.canaries.iter().any(|canary| {
        output
            .windows(canary.len())
            .any(|window| window == canary.as_bytes())
    }) {
        return Err("canary_leak");
    }
    drop(runtime);
    let _ = fs::remove_dir_all(root);
    Ok(())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(u128::from(u64::MAX)) as u64
        })
}
