use std::fs;

#[cfg(unix)]
use std::os::unix::fs::{PermissionsExt as _, symlink};

use termirust_store::{
    MetadataFileKind, MetadataRecoveryService, PresetRepository, ProjectRepository,
    RecoveryCancellation, RecoveryErrorCode, RecoveryFaultPoint, RecoveryResult, SessionRepository,
};

fn initialized_store() -> tempfile::TempDir {
    let fixture = tempfile::tempdir().unwrap();
    ProjectRepository::open(fixture.path()).unwrap();
    SessionRepository::open(fixture.path(), fixture.path().join("session-data")).unwrap();
    PresetRepository::open(fixture.path()).unwrap();
    fixture
}

fn corrupt_projects(fixture: &tempfile::TempDir) -> Vec<u8> {
    let corrupt = b"{corrupt-current".to_vec();
    fs::write(fixture.path().join("projects.json"), &corrupt).unwrap();
    corrupt
}

#[test]
fn restores_only_invalid_metadata_and_retains_verified_private_backup() {
    let fixture = initialized_store();
    let corrupt = corrupt_projects(&fixture);
    let last_good = fs::read(fixture.path().join("projects.last-good.json")).unwrap();
    let service = MetadataRecoveryService::open(fixture.path()).unwrap();
    let plan = service.plan_restore_last_good().unwrap();
    assert_eq!(plan.files.len(), 1);
    assert_eq!(plan.files[0].kind, MetadataFileKind::Projects);
    assert_eq!(plan.unchanged_files.len(), 2);
    let backup_path = plan.files[0].current_backup_path.clone();

    let receipt = service
        .restore(plan, &RecoveryCancellation::default())
        .unwrap();
    assert_eq!(receipt.result, RecoveryResult::Restored);
    assert_eq!(
        fs::read(fixture.path().join("projects.json")).unwrap(),
        last_good
    );
    assert_eq!(fs::read(&backup_path).unwrap(), corrupt);
    assert_eq!(
        fs::read(fixture.path().join("projects.last-good.json")).unwrap(),
        last_good
    );
    assert!(
        fixture
            .path()
            .join("derived-indexes/project-session-v1.json")
            .is_file()
    );
    assert!(
        fixture
            .path()
            .join("derived-indexes/palette-v1.json")
            .is_file()
    );
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(backup_path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn healthy_store_is_a_no_change_operation() {
    let fixture = initialized_store();
    let service = MetadataRecoveryService::open(fixture.path()).unwrap();
    let plan = service.plan_restore_last_good().unwrap();
    assert!(plan.files.is_empty());
    let receipt = service
        .restore(plan, &RecoveryCancellation::default())
        .unwrap();
    assert_eq!(receipt.result, RecoveryResult::NoChange);
    assert!(!fixture.path().join("recovery").exists());
}

#[test]
fn cancellation_and_stale_hash_fail_before_activation() {
    let fixture = initialized_store();
    let corrupt = corrupt_projects(&fixture);
    let service = MetadataRecoveryService::open(fixture.path()).unwrap();
    let plan = service.plan_restore_last_good().unwrap();
    let cancellation = RecoveryCancellation::default();
    cancellation.cancel();
    assert_eq!(
        service.restore(plan, &cancellation).unwrap_err().code,
        RecoveryErrorCode::Cancelled
    );
    assert_eq!(
        fs::read(fixture.path().join("projects.json")).unwrap(),
        corrupt
    );

    let plan = service.plan_restore_last_good().unwrap();
    fs::write(
        fixture.path().join("projects.json"),
        b"{different-corruption",
    )
    .unwrap();
    assert_eq!(
        service
            .restore(plan, &RecoveryCancellation::default())
            .unwrap_err()
            .code,
        RecoveryErrorCode::StaleRevision
    );
    assert!(!fixture.path().join("recovery/active-v1.json").exists());
}

#[test]
fn missing_corrupt_and_future_sources_fail_closed() {
    let fixture = initialized_store();
    corrupt_projects(&fixture);
    fs::remove_file(fixture.path().join("projects.last-good.json")).unwrap();
    let service = MetadataRecoveryService::open(fixture.path()).unwrap();
    assert_eq!(
        service.plan_restore_last_good().unwrap_err().code,
        RecoveryErrorCode::NoLastGood
    );

    fs::write(fixture.path().join("projects.last-good.json"), b"invalid").unwrap();
    assert_eq!(
        service.plan_restore_last_good().unwrap_err().code,
        RecoveryErrorCode::CorruptLastGood
    );

    let format_path = fixture.path().join("format.json");
    let mut format: serde_json::Value =
        serde_json::from_slice(&fs::read(&format_path).unwrap()).unwrap();
    format["format_version"] = serde_json::json!(65_535);
    fs::write(format_path, serde_json::to_vec(&format).unwrap()).unwrap();
    assert_eq!(
        service.plan_restore_last_good().unwrap_err().code,
        RecoveryErrorCode::NewerFormat
    );
}

#[test]
fn every_journaled_crash_rolls_back_exact_current_bytes_on_restart() {
    for fault in [
        RecoveryFaultPoint::AfterJournal,
        RecoveryFaultPoint::AfterFirstPublish,
        RecoveryFaultPoint::AfterAllPublish,
        RecoveryFaultPoint::AfterVerification,
    ] {
        let fixture = initialized_store();
        let corrupt = corrupt_projects(&fixture);
        let service = MetadataRecoveryService::open(fixture.path()).unwrap();
        let plan = service.plan_restore_last_good().unwrap();
        assert_eq!(
            service
                .restore_with_fault(plan, &RecoveryCancellation::default(), Some(fault))
                .unwrap_err()
                .code,
            RecoveryErrorCode::InjectedCrash
        );
        let restarted = MetadataRecoveryService::open(fixture.path()).unwrap();
        let receipt = restarted.recover_interrupted_restore().unwrap().unwrap();
        assert_eq!(receipt.result, RecoveryResult::RolledBack);
        assert_eq!(
            fs::read(fixture.path().join("projects.json")).unwrap(),
            corrupt
        );
        assert!(restarted.recover_interrupted_restore().unwrap().is_none());
    }
}

#[test]
fn pre_journal_crash_never_changes_authoritative_metadata() {
    let fixture = initialized_store();
    let corrupt = corrupt_projects(&fixture);
    let service = MetadataRecoveryService::open(fixture.path()).unwrap();
    let plan = service.plan_restore_last_good().unwrap();
    assert_eq!(
        service
            .restore_with_fault(
                plan,
                &RecoveryCancellation::default(),
                Some(RecoveryFaultPoint::AfterBackup),
            )
            .unwrap_err()
            .code,
        RecoveryErrorCode::InjectedCrash
    );
    assert_eq!(
        fs::read(fixture.path().join("projects.json")).unwrap(),
        corrupt
    );
    assert!(service.recover_interrupted_restore().unwrap().is_none());
}

#[cfg(unix)]
#[test]
fn recovery_rejects_symlinked_backup_and_marker_roots() {
    let fixture = initialized_store();
    corrupt_projects(&fixture);
    let outside = fixture.path().join("outside");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, fixture.path().join("recovery")).unwrap();
    let service = MetadataRecoveryService::open(fixture.path()).unwrap();
    let plan = service.plan_restore_last_good().unwrap();
    assert_eq!(
        service
            .restore(plan, &RecoveryCancellation::default())
            .unwrap_err()
            .code,
        RecoveryErrorCode::UnsafeEntry
    );
    assert!(fs::read_dir(outside).unwrap().next().is_none());
}
