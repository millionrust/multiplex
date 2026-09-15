mod common;

use std::fs;
use termirust_relay_protocol::RelayDiagnosticCode;
use termirust_relay_server::{RelayMetadataStore, RelayStoreFault};

#[test]
fn every_atomic_commit_crash_point_recovers_old_or_new_complete_state() {
    for (fault, expects_new) in [
        (RelayStoreFault::BeforeStageWrite, false),
        (RelayStoreFault::AfterStageWrite, false),
        (RelayStoreFault::AfterStageSync, false),
        (RelayStoreFault::AfterRename, true),
        (RelayStoreFault::AfterParentSync, true),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("relay-state-v1.json");
        let store = RelayMetadataStore::acquire(&path).unwrap();
        let old = common::fixture_registration(20).0;
        let new = common::fixture_registration(21).0;
        store.commit(std::slice::from_ref(&old)).unwrap();

        let error = store
            .commit_with_fault(&[old.clone(), new.clone()], fault)
            .unwrap_err();
        assert_eq!(error.code(), RelayDiagnosticCode::StateWriteFailed);
        let expected = if expects_new {
            vec![old, new]
        } else {
            vec![old]
        };
        assert_eq!(store.load().unwrap(), expected, "fault: {fault:?}");
    }
}

#[cfg(unix)]
#[test]
fn metadata_permission_failure_is_closed_and_preserves_current_state() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("relay");
    let path = directory.join("relay-state-v1.json");
    let store = RelayMetadataStore::acquire(&path).unwrap();
    let old = common::fixture_registration(24).0;
    store.commit(std::slice::from_ref(&old)).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o500)).unwrap();

    let error = store
        .commit(&[old.clone(), common::fixture_registration(25).0])
        .unwrap_err();
    assert_eq!(error.code(), RelayDiagnosticCode::StatePermissionDenied);
    assert_eq!(store.load().unwrap(), vec![old]);

    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn corrupt_newer_locked_and_permissive_state_fail_closed_without_rewrite() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("relay-state-v1.json");
    let store = RelayMetadataStore::acquire(&path).unwrap();
    store.commit(&[common::fixture_registration(22).0]).unwrap();
    let locked = RelayMetadataStore::acquire(&path).unwrap_err();
    assert_eq!(locked.code(), RelayDiagnosticCode::StateLocked);
    drop(store);

    fs::write(
        &path,
        br#"{"format":"relay-state-v1","format_version":2,"routes":[]}"#,
    )
    .unwrap();
    set_private(&path);
    let before = fs::read(&path).unwrap();
    let store = RelayMetadataStore::acquire(&path).unwrap();
    assert_eq!(
        store.load().unwrap_err().code(),
        RelayDiagnosticCode::StateVersionUnsupported
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    drop(store);

    fs::write(&path, b"not-json").unwrap();
    set_private(&path);
    let store = RelayMetadataStore::acquire(&path).unwrap();
    assert_eq!(
        store.load().unwrap_err().code(),
        RelayDiagnosticCode::StateCorrupt
    );
    drop(store);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let store = RelayMetadataStore::acquire(&path).unwrap();
        assert_eq!(
            store.load().unwrap_err().code(),
            RelayDiagnosticCode::StatePermissionDenied
        );
    }
}

#[test]
fn metadata_store_contains_only_public_admission_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("relay-state-v1.json");
    let store = RelayMetadataStore::acquire(&path).unwrap();
    let registration = common::fixture_registration(23).0;
    store.commit(&[registration]).unwrap();
    let persisted = fs::read_to_string(&path).unwrap();
    let private_seed: [u8; 32] = core::array::from_fn(|index| 23_u8.wrapping_add(index as u8));
    let private_hex: String = private_seed
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert!(!persisted.contains(&private_hex));
    assert!(!persisted.contains("ciphertext"));
    assert!(!persisted.contains("terminal"));
    assert!(!persisted.contains("session"));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn configuration_and_error_debug_views_redact_protected_details() {
    let temp = tempfile::tempdir().unwrap();
    let config = common::config(&temp);
    let config_debug = format!("{config:?}");
    assert!(!config_debug.contains(temp.path().to_str().unwrap()));
    assert!(!config_debug.contains("127.0.0.1"));

    let error = termirust_relay_server::RelayServerError::with_source(
        RelayDiagnosticCode::StateWriteFailed,
        std::io::Error::other("/protected/canary-state-path"),
    );
    let error_debug = format!("{error:?}");
    assert!(!error_debug.contains("canary-state-path"));
    assert_eq!(error.to_string(), "relay_state_write_failed");
}

#[cfg(unix)]
fn set_private(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

#[cfg(not(unix))]
fn set_private(_path: &std::path::Path) {}
