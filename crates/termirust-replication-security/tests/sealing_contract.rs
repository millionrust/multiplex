use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::path::PathBuf;

use rand_core::{CryptoRng, Error as RngError, RngCore};
use serde::Deserialize;
use termirust_domain::{
    MAX_REPLICATION_SEALED_PAYLOAD_BYTES, ReplicationCollectionId, ReplicationOperation,
    ReplicationRecordId, ReplicationRecordKey, ReplicationReplicaId, ReplicationVersionVector,
    ReplicationWorkspaceId,
};
use termirust_replication_security::{
    MAX_REPLICATION_PLAINTEXT_BYTES, OpenedReplicationOperation, OpenedReplicationPayload,
    REPLICATION_AUTH_TAG_BYTES, REPLICATION_ENVELOPE_VERSION, ReplicationCryptoError,
    ReplicationEnvelope, ReplicationEpochKey, ReplicationKeyEpoch, ReplicationOperationKind,
    ReplicationSealContext, open, seal_delete_with_rng, seal_put_with_rng,
};
use zeroize::ZeroizeOnDrop;

#[derive(Debug, Deserialize)]
struct SealingFixture {
    schema_version: u16,
    key_epoch: u64,
    key_hex: String,
    workspace_id: String,
    collection: String,
    record_id: String,
    author: String,
    vector: BTreeMap<String, u64>,
    put_plaintext_hex: String,
    put_nonce_hex: String,
    put_envelope_hex: String,
    delete_nonce_hex: String,
    delete_envelope_hex: String,
}

struct DomainFixture {
    workspace_id: ReplicationWorkspaceId,
    record_key: ReplicationRecordKey,
    author: ReplicationReplicaId,
    vector: ReplicationVersionVector,
}

impl DomainFixture {
    fn context(&self, operation: ReplicationOperationKind) -> ReplicationSealContext<'_> {
        ReplicationSealContext {
            workspace_id: &self.workspace_id,
            record_key: &self.record_key,
            author: &self.author,
            vector: &self.vector,
            operation,
        }
    }
}

struct FixedRng([u8; 12]);

impl RngCore for FixedRng {
    fn next_u32(&mut self) -> u32 {
        u32::from_be_bytes(self.0[..4].try_into().expect("fixed RNG prefix"))
    }

    fn next_u64(&mut self) -> u64 {
        u64::from_be_bytes(self.0[..8].try_into().expect("fixed RNG prefix"))
    }

    fn fill_bytes(&mut self, destination: &mut [u8]) {
        destination.copy_from_slice(&self.0[..destination.len()]);
    }

    fn try_fill_bytes(&mut self, destination: &mut [u8]) -> Result<(), RngError> {
        self.fill_bytes(destination);
        Ok(())
    }
}

impl CryptoRng for FixedRng {}

struct FailingRng;

impl RngCore for FailingRng {
    fn next_u32(&mut self) -> u32 {
        0
    }

    fn next_u64(&mut self) -> u64 {
        0
    }

    fn fill_bytes(&mut self, _destination: &mut [u8]) {
        panic!("sealing must use the fallible RNG entry point")
    }

    fn try_fill_bytes(&mut self, _destination: &mut [u8]) -> Result<(), RngError> {
        Err(RngError::from(
            NonZeroU32::new(RngError::CUSTOM_START).expect("custom RNG error code"),
        ))
    }
}

impl CryptoRng for FailingRng {}

fn fixture() -> SealingFixture {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/replication/sealing-contract-v1.json");
    let bytes = std::fs::read(path).expect("sealing fixture should be readable");
    serde_json::from_slice(&bytes).expect("sealing fixture should parse")
}

fn domain_fixture(fixture: &SealingFixture) -> DomainFixture {
    let author = ReplicationReplicaId::new(fixture.author.clone()).expect("fixture author");
    DomainFixture {
        workspace_id: ReplicationWorkspaceId::new(fixture.workspace_id.clone())
            .expect("fixture workspace"),
        record_key: ReplicationRecordKey::new(
            ReplicationCollectionId::new(fixture.collection.clone()).expect("fixture collection"),
            ReplicationRecordId::new(fixture.record_id.clone()).expect("fixture record"),
        ),
        author,
        vector: ReplicationVersionVector::new(fixture.vector.iter().map(|(replica, counter)| {
            (
                ReplicationReplicaId::new(replica.clone()).expect("fixture vector replica"),
                *counter,
            )
        }))
        .expect("fixture vector"),
    }
}

fn epoch_key(fixture: &SealingFixture) -> ReplicationEpochKey {
    let bytes: [u8; 32] = hex::decode(&fixture.key_hex)
        .expect("fixture key hex")
        .try_into()
        .expect("fixture key length");
    ReplicationEpochKey::from_bytes(
        ReplicationKeyEpoch::new(fixture.key_epoch).expect("fixture epoch"),
        bytes,
    )
    .expect("fixture key")
}

fn fixed_rng(hex_value: &str) -> FixedRng {
    FixedRng(
        hex::decode(hex_value)
            .expect("fixture nonce hex")
            .try_into()
            .expect("fixture nonce length"),
    )
}

fn plaintext(fixture: &SealingFixture) -> Vec<u8> {
    hex::decode(&fixture.put_plaintext_hex).expect("fixture plaintext hex")
}

fn assert_open_error(
    result: Result<OpenedReplicationOperation, ReplicationCryptoError>,
    expected: ReplicationCryptoError,
) {
    assert_eq!(result.expect_err("opening should fail closed"), expected);
}

#[test]
fn exact_put_delete_vectors_round_trip_and_fit_domain_payloads() {
    let fixture = fixture();
    assert_eq!(fixture.schema_version, REPLICATION_ENVELOPE_VERSION);
    let domain = domain_fixture(&fixture);
    let key = epoch_key(&fixture);
    let plaintext = plaintext(&fixture);

    let put = seal_put_with_rng(
        domain.context(ReplicationOperationKind::Put),
        &key,
        &plaintext,
        &mut fixed_rng(&fixture.put_nonce_hex),
    )
    .expect("fixture put should seal");
    let put_bytes = put.to_bytes().expect("put should encode");
    assert_eq!(hex::encode(&put_bytes), fixture.put_envelope_hex);
    assert_eq!(
        ReplicationEnvelope::from_bytes(&put_bytes).expect("put should decode"),
        put
    );
    assert_eq!(put.version(), REPLICATION_ENVELOPE_VERSION);
    assert_eq!(put.key_epoch().get(), fixture.key_epoch);
    let put_payload = put.to_sealed_payload().expect("put should fit the domain");
    assert_eq!(
        ReplicationEnvelope::from_sealed_payload(&put_payload).expect("domain put should decode"),
        put
    );
    let put_operation = ReplicationOperation::Put {
        sealed_payload: put_payload,
    };
    assert!(matches!(put_operation, ReplicationOperation::Put { .. }));
    match open(domain.context(ReplicationOperationKind::Put), &key, &put)
        .expect("put should authenticate")
    {
        OpenedReplicationOperation::Put(opened) => assert_eq!(opened.as_bytes(), plaintext),
        OpenedReplicationOperation::Delete => panic!("put opened as delete"),
    }

    let delete = seal_delete_with_rng(
        domain.context(ReplicationOperationKind::Delete),
        &key,
        &mut fixed_rng(&fixture.delete_nonce_hex),
    )
    .expect("fixture delete should seal");
    let delete_bytes = delete.to_bytes().expect("delete should encode");
    assert_eq!(hex::encode(&delete_bytes), fixture.delete_envelope_hex);
    assert_eq!(delete.ciphertext_len(), REPLICATION_AUTH_TAG_BYTES);
    let delete_operation = ReplicationOperation::Delete {
        sealed_payload: delete
            .to_sealed_payload()
            .expect("delete should fit the domain"),
    };
    assert!(matches!(
        delete_operation,
        ReplicationOperation::Delete { .. }
    ));
    assert!(matches!(
        open(
            domain.context(ReplicationOperationKind::Delete),
            &key,
            &delete
        ),
        Ok(OpenedReplicationOperation::Delete)
    ));
}

#[test]
fn every_authenticated_field_and_sealed_byte_tamper_fails_closed() {
    let fixture = fixture();
    let domain = domain_fixture(&fixture);
    let key = epoch_key(&fixture);
    let plaintext = plaintext(&fixture);
    let put = seal_put_with_rng(
        domain.context(ReplicationOperationKind::Put),
        &key,
        &plaintext,
        &mut fixed_rng(&fixture.put_nonce_hex),
    )
    .expect("fixture put should seal");

    let mut ciphertext_tamper = put.to_bytes().expect("put should encode");
    *ciphertext_tamper.last_mut().expect("ciphertext byte") ^= 1;
    let tampered =
        ReplicationEnvelope::from_bytes(&ciphertext_tamper).expect("shape remains valid");
    assert_open_error(
        open(
            domain.context(ReplicationOperationKind::Put),
            &key,
            &tampered,
        ),
        ReplicationCryptoError::AuthenticationFailed,
    );

    let mut nonce_tamper = put.to_bytes().expect("put should encode");
    nonce_tamper[15] ^= 1;
    let tampered = ReplicationEnvelope::from_bytes(&nonce_tamper).expect("shape remains valid");
    assert_open_error(
        open(
            domain.context(ReplicationOperationKind::Put),
            &key,
            &tampered,
        ),
        ReplicationCryptoError::AuthenticationFailed,
    );

    let other_workspace = ReplicationWorkspaceId::new("workspace-beta").expect("other workspace");
    let other_workspace_context = ReplicationSealContext {
        workspace_id: &other_workspace,
        ..domain.context(ReplicationOperationKind::Put)
    };
    assert_open_error(
        open(other_workspace_context, &key, &put),
        ReplicationCryptoError::AuthenticationFailed,
    );

    let other_record = ReplicationRecordKey::new(
        ReplicationCollectionId::new("connections").expect("other collection"),
        ReplicationRecordId::new("server-2").expect("other record"),
    );
    let other_record_context = ReplicationSealContext {
        record_key: &other_record,
        ..domain.context(ReplicationOperationKind::Put)
    };
    assert_open_error(
        open(other_record_context, &key, &put),
        ReplicationCryptoError::AuthenticationFailed,
    );

    let other_collection = ReplicationRecordKey::new(
        ReplicationCollectionId::new("projects").expect("other collection"),
        ReplicationRecordId::new("server-1").expect("same record"),
    );
    let other_collection_context = ReplicationSealContext {
        record_key: &other_collection,
        ..domain.context(ReplicationOperationKind::Put)
    };
    assert_open_error(
        open(other_collection_context, &key, &put),
        ReplicationCryptoError::AuthenticationFailed,
    );

    let other_author = ReplicationReplicaId::new("device-b").expect("other author");
    let other_author_context = ReplicationSealContext {
        author: &other_author,
        ..domain.context(ReplicationOperationKind::Put)
    };
    assert_open_error(
        open(other_author_context, &key, &put),
        ReplicationCryptoError::AuthenticationFailed,
    );

    let other_vector = ReplicationVersionVector::new([
        (ReplicationReplicaId::new("device-a").expect("replica a"), 3),
        (ReplicationReplicaId::new("device-b").expect("replica b"), 1),
    ])
    .expect("other vector");
    let other_vector_context = ReplicationSealContext {
        vector: &other_vector,
        ..domain.context(ReplicationOperationKind::Put)
    };
    assert_open_error(
        open(other_vector_context, &key, &put),
        ReplicationCryptoError::AuthenticationFailed,
    );

    assert_open_error(
        open(domain.context(ReplicationOperationKind::Delete), &key, &put),
        ReplicationCryptoError::AuthenticationFailed,
    );

    let wrong_key = ReplicationEpochKey::from_bytes(
        ReplicationKeyEpoch::new(fixture.key_epoch).expect("same epoch"),
        [0x55; 32],
    )
    .expect("wrong key material is structurally valid");
    assert_open_error(
        open(
            domain.context(ReplicationOperationKind::Put),
            &wrong_key,
            &put,
        ),
        ReplicationCryptoError::AuthenticationFailed,
    );

    let next_epoch_key = ReplicationEpochKey::from_bytes(
        ReplicationKeyEpoch::new(fixture.key_epoch + 1).expect("next epoch"),
        [0x66; 32],
    )
    .expect("next key");
    assert_open_error(
        open(
            domain.context(ReplicationOperationKind::Put),
            &next_epoch_key,
            &put,
        ),
        ReplicationCryptoError::KeyEpochMismatch,
    );

    let mut epoch_tamper = put.to_bytes().expect("put should encode");
    epoch_tamper[7..15].copy_from_slice(&(fixture.key_epoch + 1).to_be_bytes());
    let tampered = ReplicationEnvelope::from_bytes(&epoch_tamper).expect("new epoch is valid");
    let matching_tampered_epoch_key = ReplicationEpochKey::from_bytes(
        ReplicationKeyEpoch::new(fixture.key_epoch + 1).expect("next epoch"),
        hex::decode(&fixture.key_hex)
            .expect("fixture key hex")
            .try_into()
            .expect("fixture key length"),
    )
    .expect("same bytes at tampered epoch");
    assert_open_error(
        open(
            domain.context(ReplicationOperationKind::Put),
            &matching_tampered_epoch_key,
            &tampered,
        ),
        ReplicationCryptoError::AuthenticationFailed,
    );

    let mut version_tamper = put.to_bytes().expect("put should encode");
    version_tamper[4..6].copy_from_slice(&2_u16.to_be_bytes());
    assert_eq!(
        ReplicationEnvelope::from_bytes(&version_tamper),
        Err(ReplicationCryptoError::UnsupportedEnvelopeVersion)
    );
    let mut suite_tamper = put.to_bytes().expect("put should encode");
    suite_tamper[6] = 99;
    assert_eq!(
        ReplicationEnvelope::from_bytes(&suite_tamper),
        Err(ReplicationCryptoError::UnsupportedCipherSuite)
    );
    let mut magic_tamper = put.to_bytes().expect("put should encode");
    magic_tamper[0] ^= 1;
    assert_eq!(
        ReplicationEnvelope::from_bytes(&magic_tamper),
        Err(ReplicationCryptoError::MalformedEnvelope)
    );
    let mut length_tamper = put.to_bytes().expect("put should encode");
    let declared = u32::from_be_bytes(length_tamper[27..31].try_into().expect("length field"));
    length_tamper[27..31].copy_from_slice(&(declared + 1).to_be_bytes());
    assert_eq!(
        ReplicationEnvelope::from_bytes(&length_tamper),
        Err(ReplicationCryptoError::MalformedEnvelope)
    );
}

#[test]
fn hostile_limits_invalid_context_and_rng_failure_remain_bounded() {
    let fixture = fixture();
    let domain = domain_fixture(&fixture);
    let key = epoch_key(&fixture);
    assert_eq!(
        seal_put_with_rng(
            domain.context(ReplicationOperationKind::Put),
            &key,
            &[],
            &mut fixed_rng(&fixture.put_nonce_hex)
        ),
        Err(ReplicationCryptoError::EmptyPutPayload)
    );
    assert_eq!(
        seal_put_with_rng(
            domain.context(ReplicationOperationKind::Put),
            &key,
            &vec![0; MAX_REPLICATION_PLAINTEXT_BYTES + 1],
            &mut fixed_rng(&fixture.put_nonce_hex)
        ),
        Err(ReplicationCryptoError::PlaintextTooLarge)
    );

    let maximum_plaintext = vec![0x44; MAX_REPLICATION_PLAINTEXT_BYTES];
    let maximum = seal_put_with_rng(
        domain.context(ReplicationOperationKind::Put),
        &key,
        &maximum_plaintext,
        &mut fixed_rng(&fixture.put_nonce_hex),
    )
    .expect("maximum plaintext should seal");
    assert_eq!(maximum.encoded_len(), MAX_REPLICATION_SEALED_PAYLOAD_BYTES);
    assert_eq!(
        maximum
            .to_bytes()
            .expect("maximum envelope should encode")
            .len(),
        MAX_REPLICATION_SEALED_PAYLOAD_BYTES
    );
    match open(
        domain.context(ReplicationOperationKind::Put),
        &key,
        &maximum,
    )
    .expect("maximum envelope should authenticate")
    {
        OpenedReplicationOperation::Put(opened) => {
            assert_eq!(opened.as_bytes(), maximum_plaintext)
        }
        OpenedReplicationOperation::Delete => panic!("maximum put opened as delete"),
    }

    assert_eq!(
        seal_put_with_rng(
            domain.context(ReplicationOperationKind::Put),
            &key,
            b"synthetic",
            &mut FailingRng
        ),
        Err(ReplicationCryptoError::RandomUnavailable)
    );
    assert_eq!(
        seal_put_with_rng(
            domain.context(ReplicationOperationKind::Delete),
            &key,
            b"synthetic",
            &mut fixed_rng(&fixture.put_nonce_hex)
        ),
        Err(ReplicationCryptoError::OperationMismatch)
    );
    assert_eq!(
        seal_delete_with_rng(
            domain.context(ReplicationOperationKind::Put),
            &key,
            &mut fixed_rng(&fixture.delete_nonce_hex)
        ),
        Err(ReplicationCryptoError::OperationMismatch)
    );

    assert_eq!(
        ReplicationKeyEpoch::new(0),
        Err(ReplicationCryptoError::InvalidKeyEpoch)
    );
    assert!(matches!(
        ReplicationEpochKey::from_bytes(ReplicationKeyEpoch::new(1).expect("valid epoch"), [0; 32]),
        Err(ReplicationCryptoError::InvalidKeyMaterial)
    ));
    assert_eq!(
        ReplicationEnvelope::from_bytes(&[0; REPLICATION_AUTH_TAG_BYTES]),
        Err(ReplicationCryptoError::MalformedEnvelope)
    );
    assert_eq!(
        ReplicationEnvelope::from_bytes(&vec![0; MAX_REPLICATION_SEALED_PAYLOAD_BYTES + 1]),
        Err(ReplicationCryptoError::EnvelopeTooLarge)
    );

    let mut malformed = maximum.to_bytes().expect("maximum envelope should encode");
    malformed.pop();
    assert_eq!(
        ReplicationEnvelope::from_bytes(&malformed),
        Err(ReplicationCryptoError::MalformedEnvelope)
    );

    let invalid_workspace: ReplicationWorkspaceId =
        serde_json::from_str("\"contains spaces\"").expect("transparent type should decode");
    let invalid_context = ReplicationSealContext {
        workspace_id: &invalid_workspace,
        ..domain.context(ReplicationOperationKind::Put)
    };
    assert_eq!(
        seal_put_with_rng(
            invalid_context,
            &key,
            b"synthetic",
            &mut fixed_rng(&fixture.put_nonce_hex)
        ),
        Err(ReplicationCryptoError::InvalidContext)
    );
}

#[test]
fn secret_plaintext_context_and_envelope_debug_output_is_redacted() {
    fn assert_zeroize_on_drop<T: ZeroizeOnDrop>() {}
    assert_zeroize_on_drop::<ReplicationEpochKey>();
    assert_zeroize_on_drop::<OpenedReplicationPayload>();

    let fixture = fixture();
    let domain = domain_fixture(&fixture);
    let key = epoch_key(&fixture);
    let plaintext = plaintext(&fixture);
    let put = seal_put_with_rng(
        domain.context(ReplicationOperationKind::Put),
        &key,
        &plaintext,
        &mut fixed_rng(&fixture.put_nonce_hex),
    )
    .expect("fixture put should seal");

    let key_debug = format!("{key:?}");
    assert!(key_debug.contains("<redacted>"));
    assert!(!key_debug.contains(&fixture.key_hex));

    let envelope_debug = format!("{put:?}");
    assert!(envelope_debug.contains("<redacted>"));
    assert!(!envelope_debug.contains(&fixture.put_nonce_hex));
    assert!(!envelope_debug.contains(&hex::encode(put.to_bytes().expect("put should encode"))));

    let context_debug = format!("{:?}", domain.context(ReplicationOperationKind::Put));
    assert!(!context_debug.contains(&fixture.workspace_id));
    assert!(!context_debug.contains(&fixture.record_id));
    assert!(!context_debug.contains(&fixture.author));

    let opened = open(domain.context(ReplicationOperationKind::Put), &key, &put)
        .expect("put should authenticate");
    let opened_debug = format!("{opened:?}");
    assert!(opened_debug.contains("<redacted>"));
    assert!(!opened_debug.contains("Synthetic host"));

    let error_debug = format!("{:?}", ReplicationCryptoError::AuthenticationFailed);
    assert_eq!(error_debug, "AuthenticationFailed");
}
