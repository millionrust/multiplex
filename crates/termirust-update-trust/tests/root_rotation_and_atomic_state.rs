mod common;

use std::fs;

use common::{FIXED_NOW, FixedClock, MemoryStateStore, fixture, reference_request, workspace_root};
use termirust_update_trust::{
    FileTrustStateStore, RepositorySource, TRUST_STATE_SCHEMA_VERSION, TrustErrorCode,
    TrustStateInspection, TrustStateStore, TrustedState, verify_and_commit,
};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn versions_advance_once_then_replay_and_rollback_fail_closed() {
    let state = MemoryStateStore::default();
    verify_and_commit(
        &fixture("valid-v1"),
        &reference_request(),
        &state,
        &FixedClock(FIXED_NOW),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    verify_and_commit(
        &fixture("valid-v2"),
        &reference_request(),
        &state,
        &FixedClock(FIXED_NOW + 1),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let accepted = state.current().unwrap();
    assert_eq!(accepted.timestamp_version, 2);

    let replay = verify_and_commit(
        &fixture("valid-v2"),
        &reference_request(),
        &state,
        &FixedClock(FIXED_NOW + 2),
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(replay.code, TrustErrorCode::Replay);
    assert_eq!(state.current().unwrap(), accepted);

    let rollback = verify_and_commit(
        &fixture("valid-v1"),
        &reference_request(),
        &state,
        &FixedClock(FIXED_NOW + 3),
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(rollback.code, TrustErrorCode::Rollback);
    assert_eq!(state.current().unwrap(), accepted);
}

#[tokio::test]
async fn clock_rollback_and_commit_failure_preserve_last_state() {
    let state = MemoryStateStore::default();
    verify_and_commit(
        &fixture("valid-v1"),
        &reference_request(),
        &state,
        &FixedClock(FIXED_NOW),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let accepted = state.current().unwrap();
    let error = verify_and_commit(
        &fixture("valid-v2"),
        &reference_request(),
        &state,
        &FixedClock(FIXED_NOW - 1),
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, TrustErrorCode::ClockRollback);
    assert_eq!(state.current().unwrap(), accepted);

    let failing = MemoryStateStore::failing();
    let error = verify_and_commit(
        &fixture("valid-v1"),
        &reference_request(),
        &failing,
        &FixedClock(FIXED_NOW),
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, TrustErrorCode::StateIo);
    assert!(failing.current().is_none());
}

#[test]
fn file_state_is_private_atomic_and_recovery_is_read_only() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("trust/state.json");
    let store = FileTrustStateStore::new(&path);
    let state = TrustedState {
        schema_version: TRUST_STATE_SCHEMA_VERSION,
        root_version: 1,
        timestamp_version: 2,
        snapshot_version: 2,
        targets_version: 2,
        observed_unix_seconds: FIXED_NOW,
    };
    store.commit(&state).unwrap();
    assert_eq!(store.load().unwrap(), Some(state.clone()));
    assert_eq!(store.inspect().unwrap(), TrustStateInspection::Valid(state));
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    fs::write(&path, b"{broken").unwrap();
    assert!(matches!(
        store.inspect().unwrap(),
        TrustStateInspection::Corrupt(bytes) if bytes == b"{broken"
    ));
    assert_eq!(store.load().unwrap_err().code, TrustErrorCode::CorruptState);

    fs::write(
        &path,
        br#"{"schema_version":999,"root_version":1,"timestamp_version":1,"snapshot_version":1,"targets_version":1,"observed_unix_seconds":1}"#,
    )
    .unwrap();
    assert!(matches!(
        store.inspect().unwrap(),
        TrustStateInspection::Newer(_)
    ));
    assert_eq!(store.load().unwrap_err().code, TrustErrorCode::NewerState);

    fs::write(
        &path,
        vec![b'x'; termirust_update_trust::MAX_TRUST_STATE_BYTES as usize + 1],
    )
    .unwrap();
    assert_eq!(store.inspect().unwrap(), TrustStateInspection::Oversized);
    assert_eq!(store.load().unwrap_err().code, TrustErrorCode::CorruptState);
}

#[tokio::test]
async fn cross_signed_root_rotation_is_accepted_before_target_selection() {
    let root = workspace_root().join("tests/fixtures/update-tuf/root-rotation");
    let source = RepositorySource {
        trusted_root: root.join("1.root.json"),
        metadata_dir: root,
    };
    let error = verify_and_commit(
        &source,
        &reference_request(),
        &MemoryStateStore::default(),
        &FixedClock(FIXED_NOW),
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, TrustErrorCode::TargetNotFound);
}

#[tokio::test]
async fn invalid_root_rotation_returns_no_target_and_no_state() {
    let source_root = workspace_root().join("tests/fixtures/update-tuf/root-rotation");
    let temporary = tempfile::tempdir().unwrap();
    for entry in fs::read_dir(&source_root).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), temporary.path().join(entry.file_name())).unwrap();
    }
    let root_two = temporary.path().join("2.root.json");
    let mut document: serde_json::Value =
        serde_json::from_slice(&fs::read(&root_two).unwrap()).unwrap();
    document["signed"]["version"] = serde_json::json!(3);
    fs::write(root_two, serde_json::to_vec(&document).unwrap()).unwrap();
    let source = RepositorySource {
        trusted_root: temporary.path().join("1.root.json"),
        metadata_dir: temporary.path().to_path_buf(),
    };
    let state = MemoryStateStore::default();
    let error = verify_and_commit(
        &source,
        &reference_request(),
        &state,
        &FixedClock(FIXED_NOW),
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error.code,
        TrustErrorCode::InvalidSignature | TrustErrorCode::Rollback | TrustErrorCode::Tampered
    ));
    assert!(state.current().is_none());
}
