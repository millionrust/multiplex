mod common;

use std::sync::{Arc, Mutex};

use common::*;
use termirust_cli::{Cancellation, ErrorCode, LocalCommandService, RunOutput, run};
use termirust_domain::{HostFingerprint, HostPublicKey, PairedDeviceStatus};

fn execute(
    service: &mut LocalCommandService,
    arguments: &[&str],
    cancellation: &Cancellation,
) -> RunOutput {
    run(
        service,
        arguments
            .iter()
            .map(|argument| (*argument).to_string())
            .collect(),
        100,
        cancellation,
    )
}

fn test_service(seed: &SeededStore) -> LocalCommandService {
    service(
        seed,
        Arc::new(Mutex::new(FakeLauncherState::default())),
        Arc::new(Mutex::new(FakeControllerState::default())),
    )
}

fn controller_bytes(seed: &SeededStore) -> Vec<u8> {
    std::fs::read(seed.controller_devices().metadata_path()).unwrap()
}

fn assert_private_output(output: &RunOutput) {
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let offer = uuid::Uuid::from_u128(9).to_string();
    let full_fingerprint = HostFingerprint::derive(HostPublicKey([0xa5; 32])).canonical();
    assert!(!combined.contains("SECRET_CANARY"));
    assert!(!combined.contains(&offer));
    assert!(!combined.contains(&full_fingerprint));
    assert!(!combined.contains("public_key"));
    assert!(!combined.contains("secret_ref"));
    assert!(!combined.contains("source_offer_id"));
}

#[test]
fn device_list_on_missing_authority_is_empty_and_creates_nothing() {
    let seed = seed_store();
    let controller_root = seed.config_root.join("controller");
    assert!(!controller_root.exists());
    let mut service = test_service(&seed);
    let output = execute(
        &mut service,
        &["device", "list", "--json"],
        &Cancellation::default(),
    );
    assert_eq!(output.exit_code, 0);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["data"]["repository_revision"], 0);
    assert_eq!(value["data"]["devices"], serde_json::json!([]));
    assert!(!controller_root.exists());

    let missing = execute(
        &mut service,
        &["device", "show", &DEVICE_ID.to_string(), "--json"],
        &Cancellation::default(),
    );
    assert_eq!(missing.exit_code, ErrorCode::Unavailable.exit_code());
    assert!(!controller_root.exists());
}

#[test]
fn device_list_show_and_filter_are_bounded_stable_and_private() {
    let seed = seed_store();
    let snapshot = seed_controller_devices(&seed);
    let before = controller_bytes(&seed);
    let mut service = test_service(&seed);

    let list = execute(
        &mut service,
        &["device", "list", "--json"],
        &Cancellation::default(),
    );
    assert_eq!(list.exit_code, 0);
    let value: serde_json::Value = serde_json::from_slice(&list.stdout).unwrap();
    assert_eq!(
        value["data"]["repository_revision"],
        snapshot.revision.get()
    );
    assert_eq!(value["data"]["devices"].as_array().unwrap().len(), 2);
    assert_eq!(value["data"]["devices"][0]["id"], DEVICE_ID.to_string());
    assert_eq!(value["data"]["devices"][0]["name"], "Jacob's iPhone");
    assert_eq!(value["data"]["devices"][0]["status"], "online");
    assert_eq!(
        value["data"]["devices"][0]["capabilities"],
        serde_json::json!(["observe_sessions", "attach_output", "send_input"])
    );
    assert_private_output(&list);

    let filtered = execute(
        &mut service,
        &["device", "list", "--status", "offline", "--json"],
        &Cancellation::default(),
    );
    let value: serde_json::Value = serde_json::from_slice(&filtered.stdout).unwrap();
    assert_eq!(value["data"]["devices"].as_array().unwrap().len(), 1);
    assert_eq!(
        value["data"]["devices"][0]["id"],
        OTHER_DEVICE_ID.to_string()
    );
    assert_private_output(&filtered);

    let shown = execute(
        &mut service,
        &["device", "show", &DEVICE_ID.to_string()],
        &Cancellation::default(),
    );
    assert_eq!(shown.exit_code, 0);
    let text = String::from_utf8(shown.stdout.clone()).unwrap();
    assert!(text.contains("Jacob's iPhone"));
    assert!(text.contains("Fingerprint suffix"));
    assert_private_output(&shown);
    assert_eq!(controller_bytes(&seed), before);
}

#[test]
fn device_revoke_preview_is_read_only_and_exact_commit_uses_authority_contract() {
    let seed = seed_store();
    let initial = seed_controller_devices(&seed);
    let before = controller_bytes(&seed);
    let mut service = test_service(&seed);

    let preview = execute(
        &mut service,
        &["device", "revoke", &DEVICE_ID.to_string(), "--json"],
        &Cancellation::default(),
    );
    assert_eq!(preview.exit_code, 0);
    let value: serde_json::Value = serde_json::from_slice(&preview.stdout).unwrap();
    assert_eq!(value["data"]["repository_revision"], initial.revision.get());
    assert_eq!(value["data"]["confirmation_required"], true);
    assert_eq!(value["data"]["active_access_will_be_revoked"], true);
    assert_eq!(controller_bytes(&seed), before);
    assert_private_output(&preview);

    let commit = execute(
        &mut service,
        &[
            "device",
            "revoke",
            &DEVICE_ID.to_string(),
            "--expected-revision",
            &initial.revision.get().to_string(),
            "--yes",
            "--json",
        ],
        &Cancellation::default(),
    );
    assert_eq!(commit.exit_code, 0);
    let value: serde_json::Value = serde_json::from_slice(&commit.stdout).unwrap();
    assert_eq!(value["data"]["repository_revision"], 2);
    assert_eq!(value["data"]["device"]["status"], "revoked");
    assert_eq!(value["data"]["applied"], true);
    assert_private_output(&commit);

    let updated = seed.controller_devices().load().unwrap();
    assert_eq!(updated.revision.get(), 2);
    assert_eq!(updated.authority.revocation_epoch, 1);
    assert_eq!(updated.authority.session_generation, 1);
    assert_eq!(
        updated
            .authority
            .devices
            .iter()
            .find(|device| device.device_id == DEVICE_ID)
            .unwrap()
            .status,
        PairedDeviceStatus::Revoked
    );
    assert!(
        updated
            .authority
            .devices
            .iter()
            .all(|device| device.revocation_epoch == 1)
    );
}

#[test]
fn stale_cancelled_missing_and_repeated_revocations_never_mutate_or_retry() {
    let seed = seed_store();
    let initial = seed_controller_devices(&seed);
    let before = controller_bytes(&seed);
    let mut service = test_service(&seed);

    let cancelled = Cancellation::default();
    cancelled.cancel();
    let output = execute(
        &mut service,
        &[
            "device",
            "revoke",
            &DEVICE_ID.to_string(),
            "--expected-revision",
            &initial.revision.get().to_string(),
            "--yes",
            "--json",
        ],
        &cancelled,
    );
    assert_eq!(output.exit_code, ErrorCode::Cancelled.exit_code());
    assert_eq!(controller_bytes(&seed), before);

    let stale = execute(
        &mut service,
        &[
            "device",
            "revoke",
            &DEVICE_ID.to_string(),
            "--expected-revision",
            "0",
            "--yes",
            "--json",
        ],
        &Cancellation::default(),
    );
    assert_eq!(stale.exit_code, ErrorCode::Conflict.exit_code());
    assert_eq!(controller_bytes(&seed), before);

    let missing_id = uuid::Uuid::from_u128(999).to_string();
    let missing = execute(
        &mut service,
        &["device", "revoke", &missing_id, "--json"],
        &Cancellation::default(),
    );
    assert_eq!(missing.exit_code, ErrorCode::Unavailable.exit_code());
    assert_eq!(controller_bytes(&seed), before);

    let committed = execute(
        &mut service,
        &[
            "device",
            "revoke",
            &DEVICE_ID.to_string(),
            "--expected-revision",
            &initial.revision.get().to_string(),
            "--yes",
            "--json",
        ],
        &Cancellation::default(),
    );
    assert_eq!(committed.exit_code, 0);
    let committed_bytes = controller_bytes(&seed);

    let repeated_stale = execute(
        &mut service,
        &[
            "device",
            "revoke",
            &DEVICE_ID.to_string(),
            "--expected-revision",
            &initial.revision.get().to_string(),
            "--yes",
            "--json",
        ],
        &Cancellation::default(),
    );
    assert_eq!(repeated_stale.exit_code, ErrorCode::Conflict.exit_code());
    assert_eq!(controller_bytes(&seed), committed_bytes);

    let repeated_preview = execute(
        &mut service,
        &["device", "revoke", &DEVICE_ID.to_string(), "--json"],
        &Cancellation::default(),
    );
    assert_eq!(
        repeated_preview.exit_code,
        ErrorCode::Validation.exit_code()
    );
    assert_eq!(controller_bytes(&seed), committed_bytes);
}

#[test]
fn corrupt_and_unsafe_device_stores_fail_closed_without_disclosure() {
    let seed = seed_store();
    let controller_root = seed.config_root.join("controller");
    std::fs::create_dir_all(&controller_root).unwrap();
    let metadata = controller_root.join("controller-devices.json");
    std::fs::write(&metadata, b"SECRET_CORRUPT_CANARY").unwrap();
    let before = std::fs::read(&metadata).unwrap();
    let mut service = test_service(&seed);
    let corrupt = execute(
        &mut service,
        &["device", "list", "--json"],
        &Cancellation::default(),
    );
    assert_eq!(corrupt.exit_code, ErrorCode::Incompatible.exit_code());
    assert!(!String::from_utf8_lossy(&corrupt.stdout).contains("SECRET_CORRUPT_CANARY"));
    assert_eq!(std::fs::read(&metadata).unwrap(), before);

    std::fs::remove_file(&metadata).unwrap();
    let target = seed.temp.path().join("outside-device-store");
    std::fs::write(&target, b"SECRET_SYMLINK_CANARY").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &metadata).unwrap();
    let unsafe_output = execute(
        &mut service,
        &["device", "list", "--json"],
        &Cancellation::default(),
    );
    #[cfg(unix)]
    {
        assert_eq!(
            unsafe_output.exit_code,
            ErrorCode::PermissionDenied.exit_code()
        );
        assert!(!String::from_utf8_lossy(&unsafe_output.stdout).contains("SECRET_SYMLINK_CANARY"));
        assert_eq!(std::fs::read(&target).unwrap(), b"SECRET_SYMLINK_CANARY");
    }
}

#[test]
fn packaged_cli_reads_and_revokes_the_desktop_device_authority() {
    let seed = seed_store();
    let initial = seed_controller_devices(&seed);
    let binary = env!("CARGO_BIN_EXE_termirust-cli");

    let list = std::process::Command::new(binary)
        .args(["device", "list", "--json"])
        .env("TERMIRUST_CONFIG_DIR", &seed.config_root)
        .output()
        .unwrap();
    assert!(list.status.success());
    let value: serde_json::Value = serde_json::from_slice(&list.stdout).unwrap();
    assert_eq!(value["data"]["devices"].as_array().unwrap().len(), 2);
    assert!(!String::from_utf8_lossy(&list.stdout).contains("SECRET_CANARY"));

    let preview = std::process::Command::new(binary)
        .args(["device", "revoke", &DEVICE_ID.to_string(), "--json"])
        .env("TERMIRUST_CONFIG_DIR", &seed.config_root)
        .output()
        .unwrap();
    assert!(preview.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&preview.stdout).unwrap()["data"]["repository_revision"],
        initial.revision.get()
    );

    let commit = std::process::Command::new(binary)
        .args([
            "device",
            "revoke",
            &DEVICE_ID.to_string(),
            "--expected-revision",
            &initial.revision.get().to_string(),
            "--yes",
            "--json",
        ])
        .env("TERMIRUST_CONFIG_DIR", &seed.config_root)
        .output()
        .unwrap();
    assert!(
        commit.status.success(),
        "{}",
        String::from_utf8_lossy(&commit.stderr)
    );
    let updated = seed.controller_devices().load().unwrap();
    assert_eq!(updated.revision.get(), initial.revision.get() + 1);
    assert_eq!(
        updated
            .authority
            .devices
            .iter()
            .find(|device| device.device_id == DEVICE_ID)
            .unwrap()
            .status,
        PairedDeviceStatus::Revoked
    );
}
