mod common;

use std::fs;
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use termirust_diagnostics::{
    DiagnosticBundle, DiagnosticCode, DiagnosticPolicy, DiagnosticRuntime, DiagnosticStatus,
    ExportCancellation, ExportErrorCode,
};

use common::{runtime, safe_diagnostic};

#[test]
fn bounded_nonblocking_channel_reports_drops_without_stalling_caller() {
    let temp = tempfile::tempdir().unwrap();
    let policy = DiagnosticPolicy {
        channel_capacity: 1,
        ..DiagnosticPolicy::default()
    };
    let runtime = runtime(temp.path(), policy);
    let handle = runtime.handle();
    let started = Instant::now();
    let mut dropped = 0;
    for index in 0..20_000 {
        if !handle.record(safe_diagnostic(index, DiagnosticCode::SessionStateChanged)) {
            dropped += 1;
        }
    }
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(dropped > 0);
}

#[test]
fn rotation_stays_within_configured_count_and_private_modes() {
    let temp = tempfile::tempdir().unwrap();
    let policy = DiagnosticPolicy {
        max_file_bytes: 1024,
        max_files: 3,
        ..DiagnosticPolicy::default()
    };
    let runtime = runtime(temp.path(), policy);
    let handle = runtime.handle();
    for index in 0..100 {
        let _ = handle.record(safe_diagnostic(index, DiagnosticCode::SessionStateChanged));
    }
    handle.flush().unwrap();
    let usage = handle.usage().unwrap();
    assert!((1..=3).contains(&usage.files));
    assert!(usage.bytes <= 3 * 1200);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(temp.path()).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for index in 0..3 {
            let path = temp.path().join(format!("diagnostics-{index}.jsonl"));
            if path.exists() {
                assert_eq!(
                    fs::metadata(path).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
        }
    }
}

#[test]
fn cancellation_and_existing_destination_preserve_previous_export() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = runtime(
        temp.path().join("logs").as_path(),
        DiagnosticPolicy::default(),
    );
    let handle = runtime.handle();
    handle.record(safe_diagnostic(10, DiagnosticCode::AppStarted));

    let cancelled_destination = temp.path().join("cancelled.json");
    let cancellation = ExportCancellation::default();
    cancellation.cancel();
    let error = handle
        .prepare_export()
        .unwrap()
        .publish(&cancelled_destination, &cancellation)
        .unwrap_err();
    assert_eq!(error.code, ExportErrorCode::Cancelled);
    assert!(!cancelled_destination.exists());

    let existing = temp.path().join("existing.json");
    fs::write(&existing, b"prior export").unwrap();
    let error = handle
        .prepare_export()
        .unwrap()
        .publish(&existing, &ExportCancellation::default())
        .unwrap_err();
    assert_eq!(error.code, ExportErrorCode::DestinationExists);
    assert_eq!(fs::read(existing).unwrap(), b"prior export");
}

#[test]
fn concurrent_publish_never_replaces_the_winning_bundle() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = runtime(
        temp.path().join("logs").as_path(),
        DiagnosticPolicy::default(),
    );
    let handle = runtime.handle();
    handle.record(safe_diagnostic(10, DiagnosticCode::AppStarted));
    handle.flush().unwrap();

    let first = handle.prepare_export().unwrap();
    let second = handle.prepare_export().unwrap();
    let destination = temp.path().join("concurrent.json");
    let barrier = Arc::new(Barrier::new(2));
    let (first_result, second_result) = std::thread::scope(|scope| {
        let first_barrier = Arc::clone(&barrier);
        let first_destination = destination.clone();
        let second_destination = destination.clone();
        let first_task = scope.spawn(move || {
            first_barrier.wait();
            first.publish(first_destination, &ExportCancellation::default())
        });
        let second_task = scope.spawn(move || {
            barrier.wait();
            second.publish(second_destination, &ExportCancellation::default())
        });
        (first_task.join().unwrap(), second_task.join().unwrap())
    });

    let results = [first_result, second_result];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| {
                result
                    .as_ref()
                    .is_err_and(|error| error.code == ExportErrorCode::DestinationExists)
            })
            .count(),
        1
    );
    let bundle: DiagnosticBundle = serde_json::from_slice(&fs::read(destination).unwrap()).unwrap();
    assert_eq!(bundle.manifest.total_entries, 1);
}

#[test]
fn cancelled_preview_and_invalid_destination_leave_no_partial_bundle() {
    let temp = tempfile::tempdir().unwrap();
    let runtime = runtime(temp.path(), DiagnosticPolicy::default());
    let handle = runtime.handle();
    handle.record(safe_diagnostic(10, DiagnosticCode::AppStarted));

    let cancellation = ExportCancellation::default();
    cancellation.cancel();
    let error = match handle.prepare_export_with_cancellation(&cancellation) {
        Ok(_) => panic!("cancelled preview unexpectedly prepared"),
        Err(error) => error,
    };
    assert_eq!(error.code, ExportErrorCode::Cancelled);
    assert!(!temp.path().join(".staging").exists());

    let prepared = handle.prepare_export().unwrap();
    let missing_parent = temp.path().join("missing").join("bundle.json");
    let error = prepared
        .publish(&missing_parent, &ExportCancellation::default())
        .unwrap_err();
    assert_eq!(error.code, ExportErrorCode::InvalidDestination);
    assert!(!missing_parent.exists());
}

#[cfg(unix)]
#[test]
fn symlinked_source_is_rejected_without_reading_its_target() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let runtime = runtime(temp.path(), DiagnosticPolicy::default());
    let handle = runtime.handle();
    handle.flush().unwrap();
    let target = temp.path().join("outside-secret");
    fs::write(&target, b"TERMIRUST_TERMINAL_CANARY_7EFA").unwrap();
    symlink(&target, temp.path().join("diagnostics-0.jsonl")).unwrap();

    let error = match handle.prepare_export() {
        Ok(_) => panic!("symlinked diagnostic source unexpectedly accepted"),
        Err(error) => error,
    };
    assert_eq!(error.code, ExportErrorCode::PermissionDenied);
    assert_eq!(fs::read(target).unwrap(), b"TERMIRUST_TERMINAL_CANARY_7EFA");
}

#[test]
fn restart_removes_only_marker_owned_staging() {
    let temp = tempfile::tempdir().unwrap();
    {
        let runtime = runtime(temp.path(), DiagnosticPolicy::default());
        let handle = runtime.handle();
        handle.record(safe_diagnostic(10, DiagnosticCode::AppStarted));
        let prepared = handle.prepare_export().unwrap();
        let staging = temp.path().join(".staging");
        assert!(staging.exists());
        std::mem::forget(prepared);
    }
    let unrelated = temp.path().join("unrelated-sentinel");
    fs::write(&unrelated, b"keep").unwrap();
    let runtime = runtime(temp.path(), DiagnosticPolicy::default());
    assert!(!temp.path().join(".staging").exists());
    assert_eq!(fs::read(unrelated).unwrap(), b"keep");
    drop(runtime);
}

#[test]
fn invalid_storage_root_fails_without_panicking() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("not-a-directory");
    fs::write(&file, b"fixture").unwrap();
    let error = match DiagnosticRuntime::start(&file, DiagnosticPolicy::default()) {
        Ok(_) => panic!("file root unexpectedly accepted"),
        Err(error) => error,
    };
    assert_eq!(error.code, ExportErrorCode::StorageUnavailable);
}

#[test]
fn disabled_policy_records_nothing_and_clear_is_marker_scoped() {
    let temp = tempfile::tempdir().unwrap();
    let policy = DiagnosticPolicy {
        enabled: false,
        ..DiagnosticPolicy::default()
    };
    let runtime = runtime(temp.path(), policy);
    let handle = runtime.handle();
    assert_eq!(handle.status(), DiagnosticStatus::Disabled);
    assert!(!handle.record(safe_diagnostic(10, DiagnosticCode::AppStarted)));
    assert_eq!(handle.usage().unwrap().bytes, 0);

    let sentinel = temp.path().join("user-file");
    fs::write(&sentinel, b"keep").unwrap();
    handle.clear().unwrap();
    assert_eq!(fs::read(sentinel).unwrap(), b"keep");
}

#[test]
fn restart_filters_expired_entries_without_discarding_recent_entries() {
    let temp = tempfile::tempdir().unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    {
        let runtime = runtime(temp.path(), DiagnosticPolicy::default());
        let handle = runtime.handle();
        handle.record(safe_diagnostic(1, DiagnosticCode::AppStarted));
        handle.record(safe_diagnostic(now, DiagnosticCode::SessionStateChanged));
        handle.flush().unwrap();
    }
    let runtime = runtime(temp.path(), DiagnosticPolicy::default());
    let handle = runtime.handle();
    let destination = temp.path().join("retained.json");
    let manifest = handle
        .prepare_export()
        .unwrap()
        .publish(&destination, &ExportCancellation::default())
        .unwrap();
    assert_eq!(manifest.total_entries, 1);
    let bundle: DiagnosticBundle = serde_json::from_slice(&fs::read(destination).unwrap()).unwrap();
    let entries: Vec<_> = bundle
        .files
        .iter()
        .flat_map(|file| file.entries.iter())
        .collect();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].occurred_at_unix_ms, now);
}
