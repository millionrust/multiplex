use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use termirust_controller_bindings::{
    AuthorizationDecision, ControllerBindingError, ControllerCapability, ControllerFrameKind,
    ControllerSecurityEngine, PairingConfirmation, PairingRole, PairingStartRequest,
    SecureBlobError, SecureBlobStore,
};

#[derive(Deserialize)]
struct Vector {
    offer_hex: String,
    host_static_private_hex: String,
    host_ephemeral_private_hex: String,
    device_static_private_hex: String,
    device_ephemeral_private_hex: String,
    message_1_hex: String,
    message_2_hex: String,
    message_3_hex: String,
    handshake_hash_hex: String,
    sas_display: String,
    first_frame_hex: String,
}

#[derive(Default)]
struct MemoryBlobStore {
    values: Mutex<HashMap<String, Vec<u8>>>,
    failure: Mutex<Option<SecureBlobError>>,
}

impl MemoryBlobStore {
    fn fail_with(&self, failure: SecureBlobError) {
        *self
            .failure
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(failure);
    }

    fn failure(&self) -> Result<(), SecureBlobError> {
        match *self
            .failure
            .lock()
            .map_err(|_| SecureBlobError::Unavailable)?
        {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

impl SecureBlobStore for MemoryBlobStore {
    fn load(&self, key_id: String) -> Result<Option<Vec<u8>>, SecureBlobError> {
        self.failure()?;
        Ok(self
            .values
            .lock()
            .map_err(|_| SecureBlobError::Unavailable)?
            .get(&key_id)
            .cloned())
    }

    fn store(&self, key_id: String, value: Vec<u8>) -> Result<(), SecureBlobError> {
        self.failure()?;
        self.values
            .lock()
            .map_err(|_| SecureBlobError::Unavailable)?
            .insert(key_id, value);
        Ok(())
    }

    fn delete(&self, key_id: String) -> Result<(), SecureBlobError> {
        self.failure()?;
        self.values
            .lock()
            .map_err(|_| SecureBlobError::Unavailable)?
            .remove(&key_id);
        Ok(())
    }
}

fn vector() -> Vector {
    serde_json::from_str(include_str!(
        "../../termirust-controller-security/tests/vectors/controller-v1.json"
    ))
    .unwrap_or_else(|error| panic!("vector: {error}"))
}

fn bytes(hex_value: &str) -> Vec<u8> {
    hex::decode(hex_value).unwrap_or_else(|error| panic!("hex: {error}"))
}

#[test]
fn ffi_contract_matches_every_controller_v1_pairing_and_frame_byte() {
    let vector = vector();
    let device_store = Arc::new(MemoryBlobStore::default());
    let host_store = Arc::new(MemoryBlobStore::default());
    let device = ControllerSecurityEngine::new(device_store)
        .unwrap_or_else(|error| panic!("device engine: {error}"));
    let host = ControllerSecurityEngine::new(host_store)
        .unwrap_or_else(|error| panic!("host engine: {error}"));
    device
        .store_secure_blob(
            "fixture-device".into(),
            bytes(&vector.device_static_private_hex),
        )
        .unwrap_or_else(|error| panic!("store device: {error}"));
    host.store_secure_blob(
        "fixture-host".into(),
        bytes(&vector.host_static_private_hex),
    )
    .unwrap_or_else(|error| panic!("store host: {error}"));

    let offer = bytes(&vector.offer_hex);
    let summary = device
        .decode_offer_summary(offer.clone())
        .unwrap_or_else(|error| panic!("summary: {error}"));
    assert_eq!((summary.version.major, summary.version.minor), (1, 0));
    assert_eq!(summary.capability_bits, 7);

    let device_session = device
        .pairing_start(PairingStartRequest {
            role: PairingRole::DeviceInitiator,
            offer_bytes: offer.clone(),
            static_key_id: "fixture-device".into(),
            ephemeral_private_key: bytes(&vector.device_ephemeral_private_hex),
            now_millis: 1_000,
            now_unix_seconds: 1_000,
        })
        .unwrap_or_else(|error| panic!("device start: {error}"));
    let host_session = host
        .pairing_start(PairingStartRequest {
            role: PairingRole::HostResponder,
            offer_bytes: offer,
            static_key_id: "fixture-host".into(),
            ephemeral_private_key: bytes(&vector.host_ephemeral_private_hex),
            now_millis: 1_000,
            now_unix_seconds: 1_000,
        })
        .unwrap_or_else(|error| panic!("host start: {error}"));

    let message_1 = device_session
        .pairing_outbound(1_001)
        .unwrap_or_else(|error| panic!("message 1: {error}"));
    assert_eq!(message_1, bytes(&vector.message_1_hex));
    host_session
        .pairing_receive(message_1, 1_002)
        .unwrap_or_else(|error| panic!("host receive 1: {error}"));

    let message_2 = host_session
        .pairing_outbound(1_003)
        .unwrap_or_else(|error| panic!("message 2: {error}"));
    assert_eq!(message_2, bytes(&vector.message_2_hex));
    device_session
        .pairing_receive(message_2, 1_004)
        .unwrap_or_else(|error| panic!("device receive 2: {error}"));

    let message_3 = device_session
        .pairing_outbound(1_005)
        .unwrap_or_else(|error| panic!("message 3: {error}"));
    assert_eq!(message_3, bytes(&vector.message_3_hex));
    host_session
        .pairing_receive(message_3, 1_006)
        .unwrap_or_else(|error| panic!("host receive 3: {error}"));

    for session in [&device_session, &host_session] {
        assert_eq!(
            session
                .sas()
                .unwrap_or_else(|error| panic!("sas: {error}"))
                .value,
            vector.sas_display
        );
        assert_eq!(
            session
                .handshake_hash()
                .unwrap_or_else(|error| panic!("hash: {error}")),
            bytes(&vector.handshake_hash_hex)
        );
        session
            .confirm_or_reject(PairingConfirmation::Confirm, vector.sas_display.clone(), 4)
            .unwrap_or_else(|error| panic!("confirm: {error}"));
        assert_eq!(
            session
                .authorize(ControllerCapability::ObserveSessions, 4)
                .unwrap_or_else(|error| panic!("authorize: {error}")),
            AuthorizationDecision::Allow
        );
        assert_eq!(
            session
                .authorize(ControllerCapability::Resize, 4)
                .unwrap_or_else(|error| panic!("deny: {error}")),
            AuthorizationDecision::Deny
        );
    }

    let frame = device_session
        .seal_frame(
            ControllerFrameKind::Control,
            ControllerCapability::ObserveSessions,
            4,
            b"controller-v1-first".to_vec(),
        )
        .unwrap_or_else(|error| panic!("seal: {error}"));
    assert_eq!(frame, bytes(&vector.first_frame_hex));
    let opened = host_session
        .open_frame(frame)
        .unwrap_or_else(|error| panic!("open: {error}"));
    assert_eq!(opened.sequence, 0);
    assert_eq!(opened.payload, b"controller-v1-first");
}

#[test]
fn ffi_contract_contains_callback_size_cancel_and_disposal_failures() {
    let store = Arc::new(MemoryBlobStore::default());
    let engine = ControllerSecurityEngine::new(store.clone())
        .unwrap_or_else(|error| panic!("engine: {error}"));
    store.fail_with(SecureBlobError::Locked);
    assert_eq!(
        engine.secure_blob_status("device".into()),
        Err(ControllerBindingError::SecureBlobLocked)
    );
    *store
        .failure
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = None;
    assert_eq!(
        engine.store_secure_blob("device".into(), vec![0; 4 * 1024 + 1]),
        Err(ControllerBindingError::SecureBlobInvalid)
    );

    let vector = vector();
    engine
        .store_secure_blob("device".into(), bytes(&vector.device_static_private_hex))
        .unwrap_or_else(|error| panic!("store: {error}"));
    let session = engine
        .pairing_start(PairingStartRequest {
            role: PairingRole::DeviceInitiator,
            offer_bytes: bytes(&vector.offer_hex),
            static_key_id: "device".into(),
            ephemeral_private_key: bytes(&vector.device_ephemeral_private_hex),
            now_millis: 1_000,
            now_unix_seconds: 1_000,
        })
        .unwrap_or_else(|error| panic!("start: {error}"));
    session
        .cancel()
        .unwrap_or_else(|error| panic!("cancel: {error}"));
    assert_eq!(session.sas(), Err(ControllerBindingError::Disposed));
    assert!(session.finish().is_ok());
}
