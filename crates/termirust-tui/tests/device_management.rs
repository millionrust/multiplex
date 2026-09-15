use std::fs;

use termirust_cli::Cancellation;
use termirust_domain::{
    ControllerCapabilities, ControllerCapability, ControllerDeviceAuthority, ControllerDeviceId,
    ControllerProtocolRange, DevicePublicKey, DeviceStoreRevision, HostFingerprint,
    HostIdentityGeneration, HostIdentityPublic, HostIdentitySecretRef, HostIdentityState,
    HostPublicKey, PairedDeviceRecord, PairedDeviceStatus, PairingOfferId,
};
use termirust_store::ControllerDeviceRepository;
use termirust_tui::{DeviceExecutor, LocalDeviceExecutor};
use uuid::Uuid;

const SECRET_CANARY: &str = "identity:SECRET_CANARY";

struct Fixture {
    _temp: tempfile::TempDir,
    config_root: std::path::PathBuf,
    repository: ControllerDeviceRepository,
    device_id: ControllerDeviceId,
    other_device_id: ControllerDeviceId,
}

fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let config_root = temp.path().join("config");
    let repository = ControllerDeviceRepository::open(config_root.join("controller")).unwrap();
    let device_id = ControllerDeviceId::from_uuid(Uuid::from_u128(7));
    let other_device_id = ControllerDeviceId::from_uuid(Uuid::from_u128(8));
    repository
        .save(
            DeviceStoreRevision::ZERO,
            ControllerDeviceAuthority {
                identity: Some(HostIdentityPublic::new(
                    HostIdentityGeneration::INITIAL,
                    HostPublicKey([0x11; 32]),
                )),
                secret_ref: Some(HostIdentitySecretRef::new(SECRET_CANARY).unwrap()),
                state: HostIdentityState::Ready,
                devices: vec![
                    PairedDeviceRecord {
                        device_id,
                        public_key: DevicePublicKey([0xa5; 32]),
                        display_name: "Phone".into(),
                        capabilities: ControllerCapabilities::default()
                            .with(ControllerCapability::ObserveSessions)
                            .with(ControllerCapability::AttachOutput)
                            .with(ControllerCapability::SendInput),
                        protocol_range: ControllerProtocolRange::V1,
                        created_at: 100,
                        last_seen_at: Some(200),
                        revocation_epoch: 0,
                        identity_generation: HostIdentityGeneration::INITIAL,
                        status: PairedDeviceStatus::Online,
                        source_offer_id: PairingOfferId::from_uuid(Uuid::from_u128(9)),
                    },
                    PairedDeviceRecord {
                        device_id: other_device_id,
                        public_key: DevicePublicKey([0x5a; 32]),
                        display_name: "Tablet".into(),
                        capabilities: ControllerCapabilities::default()
                            .with(ControllerCapability::ObserveSessions),
                        protocol_range: ControllerProtocolRange::V1,
                        created_at: 101,
                        last_seen_at: None,
                        revocation_epoch: 0,
                        identity_generation: HostIdentityGeneration::INITIAL,
                        status: PairedDeviceStatus::Offline,
                        source_offer_id: PairingOfferId::from_uuid(Uuid::from_u128(10)),
                    },
                ],
                ..ControllerDeviceAuthority::default()
            },
        )
        .unwrap();
    Fixture {
        _temp: temp,
        config_root,
        repository,
        device_id,
        other_device_id,
    }
}

fn authority_bytes(fixture: &Fixture) -> Vec<u8> {
    fs::read(fixture.repository.metadata_path()).unwrap()
}

#[test]
fn devices_missing_authority_is_empty_and_never_created() {
    let temp = tempfile::tempdir().unwrap();
    let config_root = temp.path().join("missing-config");
    let controller_root = config_root.join("controller");
    let executor = LocalDeviceExecutor::new(config_root);
    let snapshot = executor.load(&Cancellation::default()).unwrap();
    assert_eq!(snapshot.repository_revision, 0);
    assert!(snapshot.devices.is_empty());
    assert!(!controller_root.exists());

    let error = executor
        .review_revoke(
            "00000000-0000-0000-0000-000000000007",
            &Cancellation::default(),
        )
        .unwrap_err();
    assert_eq!(error.code, "unavailable");
    assert!(!controller_root.exists());
}

#[test]
fn devices_list_review_and_exact_revoke_share_authoritative_state() {
    let fixture = fixture();
    let executor = LocalDeviceExecutor::new(fixture.config_root.clone());
    let before = authority_bytes(&fixture);
    let snapshot = executor.load(&Cancellation::default()).unwrap();
    assert_eq!(snapshot.repository_revision, 1);
    assert_eq!(snapshot.devices.len(), 2);
    assert_eq!(snapshot.devices[0].id, fixture.device_id.to_string());
    assert_eq!(snapshot.devices[0].status, "online");
    assert_eq!(snapshot.devices[1].id, fixture.other_device_id.to_string());
    assert_eq!(authority_bytes(&fixture), before);
    let debug = format!("{snapshot:?}");
    assert!(!debug.contains(SECRET_CANARY));
    assert!(!debug.contains(&Uuid::from_u128(9).to_string()));
    assert!(!debug.contains(&HostFingerprint::derive(HostPublicKey([0xa5; 32])).canonical()));

    let review = executor
        .review_revoke(&fixture.device_id.to_string(), &Cancellation::default())
        .unwrap();
    assert_eq!(review.repository_revision, 1);
    assert_eq!(review.device.id, fixture.device_id.to_string());
    assert!(review.active_access_will_be_revoked);
    assert!(!review.other_devices_reconnect);
    assert_eq!(authority_bytes(&fixture), before);

    let result = executor
        .revoke(
            &fixture.device_id.to_string(),
            review.repository_revision,
            &Cancellation::default(),
        )
        .unwrap();
    assert_eq!(result.repository_revision, 2);
    assert_eq!(result.device.status, "revoked");
    let authority = fixture.repository.load().unwrap();
    assert_eq!(authority.revision.get(), 2);
    assert_eq!(authority.authority.revocation_epoch, 1);
    assert_eq!(authority.authority.session_generation, 1);
    assert_eq!(
        authority
            .authority
            .devices
            .iter()
            .find(|device| device.device_id == fixture.device_id)
            .unwrap()
            .status,
        PairedDeviceStatus::Revoked
    );
    assert!(
        authority
            .authority
            .devices
            .iter()
            .all(|device| device.revocation_epoch == 1)
    );
}

#[test]
fn devices_stale_cancelled_and_repeated_revoke_never_retry_or_mutate() {
    let fixture = fixture();
    let executor = LocalDeviceExecutor::new(fixture.config_root.clone());
    let before = authority_bytes(&fixture);

    let cancellation = Cancellation::default();
    cancellation.cancel();
    assert_eq!(
        executor
            .revoke(&fixture.device_id.to_string(), 1, &cancellation)
            .unwrap_err()
            .code,
        "cancelled"
    );
    assert_eq!(authority_bytes(&fixture), before);

    assert_eq!(
        executor
            .revoke(&fixture.device_id.to_string(), 0, &Cancellation::default(),)
            .unwrap_err()
            .code,
        "conflict"
    );
    assert_eq!(authority_bytes(&fixture), before);

    executor
        .revoke(&fixture.device_id.to_string(), 1, &Cancellation::default())
        .unwrap();
    let committed = authority_bytes(&fixture);
    assert_eq!(
        executor
            .revoke(&fixture.device_id.to_string(), 1, &Cancellation::default(),)
            .unwrap_err()
            .code,
        "conflict"
    );
    assert_eq!(authority_bytes(&fixture), committed);
}

#[test]
fn devices_corrupt_newer_and_unsafe_authority_fail_closed_without_disclosure() {
    let fixture = fixture();
    let executor = LocalDeviceExecutor::new(fixture.config_root.clone());
    let path = fixture.repository.metadata_path();
    let valid = authority_bytes(&fixture);
    let full_fingerprint = HostFingerprint::derive(HostPublicKey([0xa5; 32])).canonical();

    fs::write(&path, b"SECRET_CORRUPT_CANARY").unwrap();
    let corrupt = executor.load(&Cancellation::default()).unwrap_err();
    assert_eq!(corrupt.code, "incompatible");
    assert!(!format!("{corrupt:?}").contains("SECRET_CORRUPT_CANARY"));
    assert_eq!(fs::read(&path).unwrap(), b"SECRET_CORRUPT_CANARY");

    fs::write(&path, &valid).unwrap();
    let mut newer: serde_json::Value = serde_json::from_slice(&valid).unwrap();
    newer["format_version"] = serde_json::json!(2);
    let newer = serde_json::to_vec_pretty(&newer).unwrap();
    fs::write(&path, &newer).unwrap();
    let error = executor.load(&Cancellation::default()).unwrap_err();
    assert_eq!(error.code, "incompatible");
    assert_eq!(fs::read(&path).unwrap(), newer);

    #[cfg(unix)]
    {
        fs::remove_file(&path).unwrap();
        let outside = fixture._temp.path().join("outside-authority");
        fs::write(&outside, b"SECRET_SYMLINK_CANARY").unwrap();
        std::os::unix::fs::symlink(&outside, &path).unwrap();
        let unsafe_error = executor.load(&Cancellation::default()).unwrap_err();
        assert_eq!(unsafe_error.code, "permission-denied");
        assert!(!format!("{unsafe_error:?}").contains("SECRET_SYMLINK_CANARY"));
        assert_eq!(fs::read(&outside).unwrap(), b"SECRET_SYMLINK_CANARY");
    }

    let safe_snapshot = termirust_tui::DeviceSnapshot {
        repository_revision: 1,
        devices: vec![termirust_tui::TuiDevice {
            id: fixture.device_id.to_string(),
            name: "Phone".into(),
            status: "online".into(),
            capabilities: vec!["observe_sessions".into()],
            protocol_minimum: 1,
            protocol_maximum: 1,
            created_at_unix_seconds: 1,
            last_seen_at_unix_seconds: None,
            fingerprint_suffix: HostFingerprint::derive(HostPublicKey([0xa5; 32])).row_suffix(),
            identity_generation: 1,
        }],
    };
    let debug = format!("{safe_snapshot:?}");
    assert!(!debug.contains(SECRET_CANARY));
    assert!(!debug.contains(&Uuid::from_u128(9).to_string()));
    assert!(!debug.contains(&full_fingerprint));
}
