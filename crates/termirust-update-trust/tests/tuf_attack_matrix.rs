mod common;

use std::fs;

use common::{FIXED_NOW, FixedClock, MemoryStateStore, copy_fixture, fixture, reference_request};
use serde_json::{Value, json};
use termirust_update_trust::{
    MAX_DELEGATED_ROLES, MAX_ROLE_BYTES, TrustErrorCode, UpdateChannel, VerificationRequest,
    verify_and_commit,
};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn valid_repository_returns_only_verified_metadata() {
    let state = MemoryStateStore::default();
    let target = verify_and_commit(
        &fixture("valid-v1"),
        &reference_request(),
        &state,
        &FixedClock(FIXED_NOW),
        &CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(target.version, "1.2.3");
    assert_eq!(target.platform, "macos");
    assert_eq!(target.arch, "aarch64");
    assert_eq!(target.rollout, 100);
    assert_eq!(target.hashes["sha256"].len(), 64);
    assert_eq!(state.current().unwrap().timestamp_version, 1);
}

#[tokio::test]
async fn valid_terminating_delegation_returns_the_exact_target() {
    let state = MemoryStateStore::default();
    let target = verify_and_commit(
        &fixture("valid-delegated"),
        &reference_request(),
        &state,
        &FixedClock(FIXED_NOW),
        &CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(target.name.as_str(), reference_request().target.as_str());
    assert_eq!(target.version, "1.2.3");
    assert_eq!(state.current().unwrap().targets_version, 1);
}

#[tokio::test]
async fn tampered_signed_metadata_is_rejected_without_state() {
    let original = fixture("valid-v1");
    let (_temporary, source) = copy_fixture(&original);
    let targets = source.metadata_dir.join("1.targets.json");
    let bytes = fs::read(&targets).unwrap();
    let mut document: Value = serde_json::from_slice(&bytes).unwrap();
    document["signed"]["targets"]["stable/macos/aarch64/termirust-1.2.3.tar.zst"]["custom"]["termirust"]
        ["version"] = json!("9.9.9");
    fs::write(targets, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
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

    assert!(
        matches!(
            error.code,
            TrustErrorCode::Tampered | TrustErrorCode::InvalidSignature
        ),
        "unexpected tamper error: {error:?}"
    );
    assert!(state.current().is_none());
}

#[tokio::test]
async fn expired_metadata_is_a_freeze_failure_without_state() {
    let original = fixture("valid-v1");
    let (_temporary, source) = copy_fixture(&original);
    let targets = source.metadata_dir.join("1.targets.json");
    let mut document: Value = serde_json::from_slice(&fs::read(&targets).unwrap()).unwrap();
    document["signed"]["expires"] = json!("2020-01-01T00:00:00Z");
    fs::write(targets, serde_json::to_vec(&document).unwrap()).unwrap();
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

    assert_eq!(error.code, TrustErrorCode::Expired);
    assert!(state.current().is_none());
}

#[tokio::test]
async fn wrong_platform_and_incompatible_versions_do_not_advance_state() {
    let source = fixture("valid-v1");
    for request in [
        VerificationRequest::new(
            reference_request().target,
            UpdateChannel::Stable,
            "linux",
            "aarch64",
            2,
            1,
        )
        .unwrap(),
        VerificationRequest::new(
            reference_request().target,
            UpdateChannel::Stable,
            "macos",
            "aarch64",
            99,
            1,
        )
        .unwrap(),
    ] {
        let state = MemoryStateStore::default();
        let error = verify_and_commit(
            &source,
            &request,
            &state,
            &FixedClock(FIXED_NOW),
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error.code,
            TrustErrorCode::WrongTarget | TrustErrorCode::Incompatible
        ));
        assert!(state.current().is_none());
    }
}

#[tokio::test]
async fn metadata_size_and_delegation_count_are_bounded_before_crypto() {
    let original = fixture("valid-v1");
    let (_temporary, source) = copy_fixture(&original);
    fs::write(
        source.metadata_dir.join("oversized.json"),
        vec![b' '; MAX_ROLE_BYTES as usize + 1],
    )
    .unwrap();
    let error = verify_and_commit(
        &source,
        &reference_request(),
        &MemoryStateStore::default(),
        &FixedClock(FIXED_NOW),
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, TrustErrorCode::ResourceLimit);

    let original = fixture("valid-v1");
    let (_temporary, source) = copy_fixture(&original);
    for index in 0..=MAX_DELEGATED_ROLES {
        fs::write(
            source.metadata_dir.join(format!("role-{index}.json")),
            serde_json::to_vec(&json!({
                "signed": {
                    "_type": "targets",
                    "expires": "2099-01-01T00:00:00Z",
                    "targets": {}
                },
                "signatures": []
            }))
            .unwrap(),
        )
        .unwrap();
    }
    let error = verify_and_commit(
        &source,
        &reference_request(),
        &MemoryStateStore::default(),
        &FixedClock(FIXED_NOW),
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, TrustErrorCode::ResourceLimit);
}

#[tokio::test]
async fn cancellation_returns_no_target_and_no_state() {
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let state = MemoryStateStore::default();
    let error = verify_and_commit(
        &fixture("valid-v1"),
        &reference_request(),
        &state,
        &FixedClock(FIXED_NOW),
        &cancellation,
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, TrustErrorCode::Cancelled);
    assert!(state.current().is_none());
}

#[tokio::test]
async fn missing_snapshot_and_duplicate_signature_threshold_fail_closed() {
    let original = fixture("valid-v1");
    let (_temporary, source) = copy_fixture(&original);
    fs::remove_file(source.metadata_dir.join("1.snapshot.json")).unwrap();
    let state = MemoryStateStore::default();
    let missing = verify_and_commit(
        &source,
        &reference_request(),
        &state,
        &FixedClock(FIXED_NOW),
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(missing.code, TrustErrorCode::MissingMetadata);
    assert!(state.current().is_none());

    let original = fixture("valid-v1");
    let (_temporary, mut source) = copy_fixture(&original);
    source.trusted_root = common::workspace_root()
        .join("tests/fixtures/update-tuf/hostile/duplicate-signatures-root.json");
    let threshold = verify_and_commit(
        &source,
        &reference_request(),
        &state,
        &FixedClock(FIXED_NOW),
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert_eq!(threshold.code, TrustErrorCode::InvalidSignature);
    assert!(state.current().is_none());
}

#[tokio::test]
async fn target_count_over_limit_is_rejected_before_signature_work() {
    let original = fixture("valid-v1");
    let (_temporary, source) = copy_fixture(&original);
    let targets = (0..=10_000)
        .map(|index| (format!("target-{index}"), json!({})))
        .collect::<serde_json::Map<_, _>>();
    fs::write(
        source.metadata_dir.join("delegated.json"),
        serde_json::to_vec(&json!({
            "signed": {
                "_type": "targets",
                "expires": "2099-01-01T00:00:00Z",
                "targets": targets
            },
            "signatures": []
        }))
        .unwrap(),
    )
    .unwrap();
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
    assert_eq!(error.code, TrustErrorCode::ResourceLimit);
    assert!(state.current().is_none());
}
