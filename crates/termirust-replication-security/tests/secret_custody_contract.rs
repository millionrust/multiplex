use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::Deserialize;
use termirust_replication_security::{
    MAX_REPLICATION_RETAINED_EPOCH_KEYS, REPLICATION_STORED_SECRET_BYTES,
    REPLICATION_STORED_SECRET_VERSION, ReplicationAuthorityPrivateKey, ReplicationDevicePrivateKey,
    ReplicationEntropy, ReplicationEntropyError, ReplicationEpochKey,
    ReplicationHistoricalKeyIndex, ReplicationHistoricalKeyLimit, ReplicationKeyEpoch,
    ReplicationSecretBackend, ReplicationSecretCustodyError, ReplicationSecretKind,
    ReplicationSecretRef, ReplicationSecretStoreError, ReplicationSecretVault,
};
use zeroize::{ZeroizeOnDrop, Zeroizing};

#[derive(Debug, Deserialize)]
struct CustodyFixture {
    schema_version: u16,
    key_epoch: u64,
    authority_key_hex: String,
    authority_public_hex: String,
    device_key_hex: String,
    device_public_hex: String,
    epoch_key_hex: String,
    authority_reference_hex: String,
    device_reference_hex: String,
    epoch_reference_hex: String,
    authority_account: String,
    device_account: String,
    epoch_account: String,
    authority_envelope_hex: String,
    device_envelope_hex: String,
    epoch_envelope_hex: String,
}

#[derive(Default)]
struct MemoryBackend {
    entries: Mutex<BTreeMap<String, Vec<u8>>>,
    failure: Mutex<Option<ReplicationSecretStoreError>>,
}

impl MemoryBackend {
    fn fail_with(&self, failure: ReplicationSecretStoreError) {
        *self.failure.lock().expect("failure lock") = Some(failure);
    }

    fn clear_failure(&self) {
        *self.failure.lock().expect("failure lock") = None;
    }

    fn bytes(&self, reference: &ReplicationSecretRef) -> Vec<u8> {
        self.entries
            .lock()
            .expect("entries lock")
            .get(&reference.expose_opaque_account())
            .expect("stored fixture secret")
            .clone()
    }

    fn replace(&self, reference: &ReplicationSecretRef, bytes: Vec<u8>) {
        self.entries
            .lock()
            .expect("entries lock")
            .insert(reference.expose_opaque_account(), bytes);
    }

    fn failure(&self) -> Result<(), ReplicationSecretStoreError> {
        match *self.failure.lock().expect("failure lock") {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

impl ReplicationSecretBackend for MemoryBackend {
    fn put(
        &self,
        reference: &ReplicationSecretRef,
        secret: &[u8],
    ) -> Result<(), ReplicationSecretStoreError> {
        self.failure()?;
        let mut entries = self.entries.lock().expect("entries lock");
        let account = reference.expose_opaque_account();
        if entries.contains_key(&account) {
            return Err(ReplicationSecretStoreError::Collision);
        }
        entries.insert(account, secret.to_vec());
        Ok(())
    }

    fn get(
        &self,
        reference: &ReplicationSecretRef,
    ) -> Result<Zeroizing<Vec<u8>>, ReplicationSecretStoreError> {
        self.failure()?;
        self.entries
            .lock()
            .expect("entries lock")
            .get(&reference.expose_opaque_account())
            .cloned()
            .map(Zeroizing::new)
            .ok_or(ReplicationSecretStoreError::Missing)
    }

    fn delete(
        &self,
        reference: &ReplicationSecretRef,
    ) -> Result<bool, ReplicationSecretStoreError> {
        self.failure()?;
        Ok(self
            .entries
            .lock()
            .expect("entries lock")
            .remove(&reference.expose_opaque_account())
            .is_some())
    }
}

struct FixedEntropy([u8; 32]);

impl ReplicationEntropy for FixedEntropy {
    fn fill(&mut self, destination: &mut [u8]) -> Result<(), ReplicationEntropyError> {
        assert_eq!(destination.len(), self.0.len());
        destination.copy_from_slice(&self.0);
        Ok(())
    }
}

struct FailingEntropy;

impl ReplicationEntropy for FailingEntropy {
    fn fill(&mut self, _destination: &mut [u8]) -> Result<(), ReplicationEntropyError> {
        Err(ReplicationEntropyError)
    }
}

fn fixture() -> CustodyFixture {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/replication/secret-custody-contract-v1.json");
    serde_json::from_slice(&std::fs::read(path).expect("custody fixture should be readable"))
        .expect("custody fixture should parse")
}

fn bytes32(value: &str) -> [u8; 32] {
    hex::decode(value)
        .expect("fixture hex")
        .try_into()
        .expect("fixture 32-byte value")
}

fn epoch(value: u64) -> ReplicationKeyEpoch {
    ReplicationKeyEpoch::new(value).expect("nonzero epoch")
}

fn epoch_reference(value: u64, marker: u8) -> ReplicationSecretRef {
    ReplicationSecretRef::from_identifier(
        ReplicationSecretKind::EpochKey,
        Some(epoch(value)),
        [marker; 32],
    )
    .expect("epoch reference")
}

fn assert_custody_error<T>(
    result: Result<T, ReplicationSecretCustodyError>,
    expected: ReplicationSecretCustodyError,
) {
    assert_eq!(result.err().expect("operation should fail"), expected);
}

#[test]
fn exact_typed_fixture_round_trips_through_opaque_references() {
    let fixture = fixture();
    assert_eq!(fixture.schema_version, REPLICATION_STORED_SECRET_VERSION);
    let backend = MemoryBackend::default();
    let vault = ReplicationSecretVault::new(backend);
    let authority = ReplicationAuthorityPrivateKey::from_bytes(bytes32(&fixture.authority_key_hex))
        .expect("authority key");
    let device = ReplicationDevicePrivateKey::from_bytes(bytes32(&fixture.device_key_hex))
        .expect("device key");
    let epoch_key =
        ReplicationEpochKey::from_bytes(epoch(fixture.key_epoch), bytes32(&fixture.epoch_key_hex))
            .expect("epoch key");

    let authority_ref = vault
        .store_authority_key_with_entropy(
            &authority,
            &mut FixedEntropy(bytes32(&fixture.authority_reference_hex)),
        )
        .expect("store authority");
    let device_ref = vault
        .store_device_key_with_entropy(
            &device,
            &mut FixedEntropy(bytes32(&fixture.device_reference_hex)),
        )
        .expect("store device");
    let epoch_ref = vault
        .store_epoch_key_with_entropy(
            &epoch_key,
            &mut FixedEntropy(bytes32(&fixture.epoch_reference_hex)),
        )
        .expect("store epoch");

    assert_eq!(
        authority_ref.expose_opaque_account(),
        fixture.authority_account
    );
    assert_eq!(device_ref.expose_opaque_account(), fixture.device_account);
    assert_eq!(epoch_ref.expose_opaque_account(), fixture.epoch_account);
    assert_eq!(
        hex::encode(vault.backend().bytes(&authority_ref)),
        fixture.authority_envelope_hex
    );
    assert_eq!(
        hex::encode(vault.backend().bytes(&device_ref)),
        fixture.device_envelope_hex
    );
    assert_eq!(
        hex::encode(vault.backend().bytes(&epoch_ref)),
        fixture.epoch_envelope_hex
    );
    assert_eq!(
        hex::encode(
            vault
                .load_authority_key(&authority_ref)
                .expect("load authority")
                .public_key()
                .as_bytes()
        ),
        fixture.authority_public_hex
    );
    assert_eq!(
        hex::encode(
            vault
                .load_device_key(&device_ref)
                .expect("load device")
                .public_key()
                .as_bytes()
        ),
        fixture.device_public_hex
    );
    assert_eq!(
        vault
            .load_epoch_key(&epoch_ref, epoch(fixture.key_epoch))
            .expect("load epoch")
            .epoch(),
        epoch(fixture.key_epoch)
    );
    assert_eq!(
        vault.backend().bytes(&epoch_ref).len(),
        REPLICATION_STORED_SECRET_BYTES
    );
}

#[test]
fn malformed_cross_role_and_cross_epoch_material_fail_closed() {
    let fixture = fixture();
    let vault = ReplicationSecretVault::new(MemoryBackend::default());
    let authority = ReplicationAuthorityPrivateKey::from_bytes(bytes32(&fixture.authority_key_hex))
        .expect("authority key");
    let authority_ref = vault
        .store_authority_key_with_entropy(
            &authority,
            &mut FixedEntropy(bytes32(&fixture.authority_reference_hex)),
        )
        .expect("store authority");
    assert_custody_error(
        vault.load_device_key(&authority_ref),
        ReplicationSecretCustodyError::SecretKindMismatch,
    );

    let epoch_key =
        ReplicationEpochKey::from_bytes(epoch(fixture.key_epoch), bytes32(&fixture.epoch_key_hex))
            .expect("epoch key");
    let epoch_ref = vault
        .store_epoch_key_with_entropy(
            &epoch_key,
            &mut FixedEntropy(bytes32(&fixture.epoch_reference_hex)),
        )
        .expect("store epoch");
    assert_custody_error(
        vault.load_epoch_key(&epoch_ref, epoch(fixture.key_epoch + 1)),
        ReplicationSecretCustodyError::KeyEpochMismatch,
    );

    let mut malformed = vault.backend().bytes(&authority_ref);
    malformed[0] ^= 0xff;
    vault.backend().replace(&authority_ref, malformed);
    assert_custody_error(
        vault.load_authority_key(&authority_ref),
        ReplicationSecretCustodyError::InvalidEnvelope,
    );

    for invalid in [Vec::new(), vec![0; REPLICATION_STORED_SECRET_BYTES + 1]] {
        vault.backend().replace(&authority_ref, invalid);
        assert_custody_error(
            vault.load_authority_key(&authority_ref),
            ReplicationSecretCustodyError::InvalidEnvelope,
        );
    }

    let exact_authority = hex::decode(&fixture.authority_envelope_hex).expect("fixture envelope");
    let mut wrong_role = exact_authority.clone();
    wrong_role[6] = ReplicationSecretKind::DevicePrivateKey as u8;
    vault.backend().replace(&authority_ref, wrong_role);
    assert_custody_error(
        vault.load_authority_key(&authority_ref),
        ReplicationSecretCustodyError::SecretKindMismatch,
    );

    let mut invalid_authority_epoch = exact_authority.clone();
    invalid_authority_epoch[14] = 1;
    vault
        .backend()
        .replace(&authority_ref, invalid_authority_epoch);
    assert_custody_error(
        vault.load_authority_key(&authority_ref),
        ReplicationSecretCustodyError::InvalidEnvelope,
    );

    let mut zero_key = exact_authority;
    zero_key[15..].fill(0);
    vault.backend().replace(&authority_ref, zero_key);
    assert_custody_error(
        vault.load_authority_key(&authority_ref),
        ReplicationSecretCustodyError::InvalidEnvelope,
    );

    let mut wrong_epoch = hex::decode(&fixture.epoch_envelope_hex).expect("fixture epoch envelope");
    wrong_epoch[14] = (fixture.key_epoch + 1) as u8;
    vault.backend().replace(&epoch_ref, wrong_epoch);
    assert_custody_error(
        vault.load_epoch_key(&epoch_ref, epoch(fixture.key_epoch)),
        ReplicationSecretCustodyError::KeyEpochMismatch,
    );
}

#[test]
fn backend_failure_collision_entropy_and_delete_states_remain_explicit() {
    let fixture = fixture();
    let vault = ReplicationSecretVault::new(MemoryBackend::default());
    let authority = ReplicationAuthorityPrivateKey::from_bytes(bytes32(&fixture.authority_key_hex))
        .expect("authority key");
    let identifier = bytes32(&fixture.authority_reference_hex);
    let reference = vault
        .store_authority_key_with_entropy(&authority, &mut FixedEntropy(identifier))
        .expect("store authority");
    assert_custody_error(
        vault.store_authority_key_with_entropy(&authority, &mut FixedEntropy(identifier)),
        ReplicationSecretCustodyError::Store(ReplicationSecretStoreError::Collision),
    );
    assert_custody_error(
        vault.store_authority_key_with_entropy(&authority, &mut FailingEntropy),
        ReplicationSecretCustodyError::EntropyUnavailable,
    );

    for failure in [
        ReplicationSecretStoreError::AccessDeniedOrLocked,
        ReplicationSecretStoreError::Unavailable,
        ReplicationSecretStoreError::Invalid,
    ] {
        vault.backend().fail_with(failure);
        assert_custody_error(
            vault.load_authority_key(&reference),
            ReplicationSecretCustodyError::Store(failure),
        );
        vault.backend().clear_failure();
    }
    assert!(vault.delete(&reference).expect("delete stored secret"));
    assert!(!vault.delete(&reference).expect("delete missing secret"));
    assert_custody_error(
        vault.load_authority_key(&reference),
        ReplicationSecretCustodyError::Store(ReplicationSecretStoreError::Missing),
    );
}

#[test]
fn historical_index_recovers_exact_epochs_and_plans_oldest_retirement() {
    let limit = ReplicationHistoricalKeyLimit::new(2).expect("limit");
    let restored = ReplicationHistoricalKeyIndex::from_retained(
        limit,
        [epoch_reference(6, 6), epoch_reference(5, 5)],
    )
    .expect("restart reconstruction sorts exact epochs");
    assert_eq!(restored.current_epoch(), epoch(6));
    assert_eq!(
        restored
            .reference_for(epoch(5))
            .expect("epoch 5")
            .key_epoch(),
        Some(epoch(5))
    );

    let update = restored
        .append(epoch_reference(7, 7))
        .expect("exact successor");
    assert_eq!(update.index().len(), 2);
    assert_eq!(update.index().current_epoch(), epoch(7));
    assert_eq!(update.retired().len(), 1);
    assert_eq!(update.retired()[0].key_epoch(), Some(epoch(5)));
    assert_custody_error(
        update.index().reference_for(epoch(5)),
        ReplicationSecretCustodyError::KeyEpochNotRetained,
    );
    assert_custody_error(
        restored.append(epoch_reference(8, 8)),
        ReplicationSecretCustodyError::NonContiguousHistory,
    );
}

#[test]
fn historical_limits_and_references_cover_smallest_largest_and_invalid_bounds() {
    assert_custody_error(
        ReplicationHistoricalKeyLimit::new(0),
        ReplicationSecretCustodyError::InvalidHistoricalLimit,
    );
    assert_custody_error(
        ReplicationHistoricalKeyLimit::new(MAX_REPLICATION_RETAINED_EPOCH_KEYS + 1),
        ReplicationSecretCustodyError::InvalidHistoricalLimit,
    );
    assert_custody_error(
        ReplicationSecretRef::from_identifier(
            ReplicationSecretKind::AuthorityPrivateKey,
            None,
            [0; 32],
        ),
        ReplicationSecretCustodyError::InvalidReference,
    );
    assert_custody_error(
        ReplicationSecretRef::from_identifier(
            ReplicationSecretKind::AuthorityPrivateKey,
            Some(epoch(1)),
            [1; 32],
        ),
        ReplicationSecretCustodyError::InvalidReference,
    );

    let one = ReplicationHistoricalKeyLimit::new(1).expect("smallest limit");
    let first = ReplicationHistoricalKeyIndex::from_retained(one, [epoch_reference(1, 1)])
        .expect("single retained key");
    let one_update = first.append(epoch_reference(2, 2)).expect("append epoch 2");
    assert_eq!(one_update.index().len(), 1);
    assert_eq!(one_update.retired().len(), 1);

    let maximum = ReplicationHistoricalKeyLimit::new(MAX_REPLICATION_RETAINED_EPOCH_KEYS)
        .expect("largest limit");
    let references = (1..=MAX_REPLICATION_RETAINED_EPOCH_KEYS)
        .map(|value| epoch_reference(value as u64, value as u8))
        .collect::<Vec<_>>();
    let full = ReplicationHistoricalKeyIndex::from_retained(maximum, references)
        .expect("largest retained index");
    let full_update = full
        .append(epoch_reference(
            MAX_REPLICATION_RETAINED_EPOCH_KEYS as u64 + 1,
            0x7f,
        ))
        .expect("append beyond largest bound");
    assert_eq!(
        full_update.index().len(),
        MAX_REPLICATION_RETAINED_EPOCH_KEYS
    );
    assert_eq!(full_update.retired()[0].key_epoch(), Some(epoch(1)));

    assert_custody_error(
        ReplicationHistoricalKeyIndex::from_retained(one, []),
        ReplicationSecretCustodyError::EmptyHistory,
    );
    assert_custody_error(
        ReplicationHistoricalKeyIndex::from_retained(
            maximum,
            [epoch_reference(1, 1), epoch_reference(3, 3)],
        ),
        ReplicationSecretCustodyError::NonContiguousHistory,
    );

    let maximum_epoch =
        ReplicationHistoricalKeyIndex::from_retained(one, [epoch_reference(u64::MAX, 0xff)])
            .expect("maximum epoch can be restored");
    assert_custody_error(
        maximum_epoch.append(epoch_reference(u64::MAX, 0xfe)),
        ReplicationSecretCustodyError::KeyEpochOverflow,
    );
}

#[test]
fn debug_errors_and_secret_types_do_not_disclose_canary_material() {
    let fixture = fixture();
    let reference = ReplicationSecretRef::from_identifier(
        ReplicationSecretKind::EpochKey,
        Some(epoch(fixture.key_epoch)),
        bytes32(&fixture.epoch_reference_hex),
    )
    .expect("reference");
    let debug = format!("{reference:?}");
    assert!(!debug.contains(&fixture.epoch_reference_hex));
    assert!(debug.contains("<opaque>"));
    let vault = ReplicationSecretVault::new(MemoryBackend::default());
    let vault_debug = format!("{vault:?}");
    assert!(vault_debug.contains("<redacted>"));
    assert!(!vault_debug.contains(&fixture.epoch_key_hex));
    for error in [
        ReplicationSecretCustodyError::InvalidEnvelope,
        ReplicationSecretCustodyError::SecretKindMismatch,
        ReplicationSecretCustodyError::Store(ReplicationSecretStoreError::AccessDeniedOrLocked),
    ] {
        let rendered = format!("{error:?} {error}");
        assert!(!rendered.contains(&fixture.epoch_reference_hex));
        assert!(!rendered.contains(&fixture.epoch_key_hex));
    }
    assert_zeroize_on_drop::<ReplicationAuthorityPrivateKey>();
    assert_zeroize_on_drop::<ReplicationDevicePrivateKey>();
    assert_zeroize_on_drop::<ReplicationEpochKey>();
    assert_zeroize_on_drop::<Zeroizing<Vec<u8>>>();
}

fn assert_zeroize_on_drop<T: ZeroizeOnDrop>() {}

#[cfg(feature = "os-keyring")]
#[test]
fn os_adapter_reports_only_explicitly_supported_targets() {
    use termirust_replication_security::OsReplicationSecretBackend;

    assert_eq!(
        OsReplicationSecretBackend::is_supported_target(),
        cfg!(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "windows",
            target_os = "linux"
        ))
    );
}
