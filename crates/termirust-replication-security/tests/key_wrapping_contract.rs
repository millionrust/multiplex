use std::path::PathBuf;

use serde::Deserialize;
use termirust_domain::{ReplicationReplicaId, ReplicationWorkspaceId};
use termirust_replication_security::{
    MAX_REPLICATION_KEY_PACKAGE_BYTES, REPLICATION_KEY_PACKAGE_VERSION,
    ReplicationAuthorityPrivateKey, ReplicationAuthorityPublicKey, ReplicationDevicePrivateKey,
    ReplicationDevicePublicKey, ReplicationEntropy, ReplicationEntropyError, ReplicationEpochKey,
    ReplicationKeyEpoch, ReplicationKeyWrapContext, ReplicationKeyWrappingError,
    ReplicationKeyWrappingSuite, WrappedReplicationEpochKey,
    generate_replication_authority_private_key_with_entropy,
    generate_replication_device_private_key_with_entropy, open_wrapped_replication_epoch_key,
    wrap_replication_epoch_key_with_entropy,
};
use zeroize::ZeroizeOnDrop;

#[derive(Debug, Deserialize)]
struct WrappingFixture {
    schema_version: u16,
    suite_id: u8,
    workspace_id: String,
    recipient: String,
    key_epoch: u64,
    epoch_key_hex: String,
    authority_private_hex: String,
    authority_public_hex: String,
    device_private_hex: String,
    device_public_hex: String,
    ephemeral_ikm_hex: String,
    package_hex: String,
}

struct DomainFixture {
    workspace_id: ReplicationWorkspaceId,
    recipient: ReplicationReplicaId,
}

impl DomainFixture {
    fn context(&self) -> ReplicationKeyWrapContext<'_> {
        ReplicationKeyWrapContext {
            workspace_id: &self.workspace_id,
            recipient: &self.recipient,
        }
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

fn fixture() -> WrappingFixture {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/replication/key-wrapping-contract-v1.json");
    serde_json::from_slice(&std::fs::read(path).expect("wrapping fixture should be readable"))
        .expect("wrapping fixture should parse")
}

fn domain_fixture(fixture: &WrappingFixture) -> DomainFixture {
    DomainFixture {
        workspace_id: ReplicationWorkspaceId::new(fixture.workspace_id.clone())
            .expect("fixture workspace"),
        recipient: ReplicationReplicaId::new(fixture.recipient.clone()).expect("fixture recipient"),
    }
}

fn bytes32(value: &str) -> [u8; 32] {
    hex::decode(value)
        .expect("fixture hex")
        .try_into()
        .expect("fixture 32-byte value")
}

fn authority_private(fixture: &WrappingFixture) -> ReplicationAuthorityPrivateKey {
    ReplicationAuthorityPrivateKey::from_bytes(bytes32(&fixture.authority_private_hex))
        .expect("fixture authority private key")
}

fn device_private(fixture: &WrappingFixture) -> ReplicationDevicePrivateKey {
    ReplicationDevicePrivateKey::from_bytes(bytes32(&fixture.device_private_hex))
        .expect("fixture device private key")
}

fn epoch_key(fixture: &WrappingFixture) -> ReplicationEpochKey {
    ReplicationEpochKey::from_bytes(
        ReplicationKeyEpoch::new(fixture.key_epoch).expect("fixture epoch"),
        bytes32(&fixture.epoch_key_hex),
    )
    .expect("fixture epoch key")
}

fn fixed_entropy(value: &str) -> FixedEntropy {
    FixedEntropy(bytes32(value))
}

fn wrapped_fixture(
    fixture: &WrappingFixture,
    domain: &DomainFixture,
    authority: &ReplicationAuthorityPrivateKey,
    recipient: &ReplicationDevicePublicKey,
    key: &ReplicationEpochKey,
) -> WrappedReplicationEpochKey {
    wrap_replication_epoch_key_with_entropy(
        domain.context(),
        authority,
        recipient,
        key,
        &mut fixed_entropy(&fixture.ephemeral_ikm_hex),
    )
    .expect("fixture key should wrap")
}

fn assert_open_error(
    result: Result<ReplicationEpochKey, ReplicationKeyWrappingError>,
    expected: ReplicationKeyWrappingError,
) {
    assert_eq!(result.expect_err("opening should fail closed"), expected);
}

#[test]
fn exact_authenticated_package_round_trips_for_one_recipient() {
    let fixture = fixture();
    assert_eq!(fixture.schema_version, REPLICATION_KEY_PACKAGE_VERSION);
    assert_eq!(fixture.suite_id, 1);
    let domain = domain_fixture(&fixture);
    let authority = authority_private(&fixture);
    let authority_public = authority.public_key();
    let device = device_private(&fixture);
    let device_public = device.public_key();
    let key = epoch_key(&fixture);

    assert_eq!(
        hex::encode(authority_public.as_bytes()),
        fixture.authority_public_hex
    );
    assert_eq!(
        hex::encode(device_public.as_bytes()),
        fixture.device_public_hex
    );

    let package = wrapped_fixture(&fixture, &domain, &authority, &device_public, &key);
    let package_bytes = package.to_bytes();
    assert_eq!(hex::encode(&package_bytes), fixture.package_hex);
    assert_eq!(package.version(), REPLICATION_KEY_PACKAGE_VERSION);
    assert_eq!(package.suite() as u8, fixture.suite_id);
    assert_eq!(package.key_epoch().get(), fixture.key_epoch);
    assert!(package.encoded_len() <= MAX_REPLICATION_KEY_PACKAGE_BYTES);
    assert_eq!(
        WrappedReplicationEpochKey::from_bytes(&package_bytes)
            .expect("exact package should decode"),
        package
    );

    let opened = open_wrapped_replication_epoch_key(
        domain.context(),
        &authority_public,
        &device,
        ReplicationKeyEpoch::new(fixture.key_epoch).expect("fixture epoch"),
        &package,
    )
    .expect("exact package should authenticate");
    let rewrapped = wrapped_fixture(&fixture, &domain, &authority, &device_public, &opened);
    assert_eq!(rewrapped, package);

    let generated_authority = generate_replication_authority_private_key_with_entropy(
        &mut fixed_entropy(&fixture.authority_private_hex),
    )
    .expect("deterministic authority generation should succeed");
    let generated_authority_again = generate_replication_authority_private_key_with_entropy(
        &mut fixed_entropy(&fixture.authority_private_hex),
    )
    .expect("deterministic authority generation should repeat");
    assert_eq!(
        generated_authority.public_key(),
        generated_authority_again.public_key()
    );
    let generated_device = generate_replication_device_private_key_with_entropy(
        &mut fixed_entropy(&fixture.device_private_hex),
    )
    .expect("deterministic device generation should succeed");
    assert_ne!(
        generated_authority.public_key().as_bytes(),
        generated_device.public_key().as_bytes()
    );
}

#[test]
fn context_key_header_and_ciphertext_substitution_fails_closed() {
    let fixture = fixture();
    let domain = domain_fixture(&fixture);
    let authority = authority_private(&fixture);
    let authority_public = authority.public_key();
    let device = device_private(&fixture);
    let device_public = device.public_key();
    let key = epoch_key(&fixture);
    let package = wrapped_fixture(&fixture, &domain, &authority, &device_public, &key);
    let expected_epoch = ReplicationKeyEpoch::new(fixture.key_epoch).expect("fixture epoch");

    let other_workspace = ReplicationWorkspaceId::new("workspace-beta").expect("other workspace");
    let other_workspace_context = ReplicationKeyWrapContext {
        workspace_id: &other_workspace,
        ..domain.context()
    };
    assert_open_error(
        open_wrapped_replication_epoch_key(
            other_workspace_context,
            &authority_public,
            &device,
            expected_epoch,
            &package,
        ),
        ReplicationKeyWrappingError::AuthenticationFailed,
    );

    let other_recipient = ReplicationReplicaId::new("device-c").expect("other recipient");
    let other_recipient_context = ReplicationKeyWrapContext {
        recipient: &other_recipient,
        ..domain.context()
    };
    assert_open_error(
        open_wrapped_replication_epoch_key(
            other_recipient_context,
            &authority_public,
            &device,
            expected_epoch,
            &package,
        ),
        ReplicationKeyWrappingError::AuthenticationFailed,
    );

    let wrong_authority = ReplicationAuthorityPrivateKey::from_bytes([0x77; 32])
        .expect("wrong authority key")
        .public_key();
    assert_open_error(
        open_wrapped_replication_epoch_key(
            domain.context(),
            &wrong_authority,
            &device,
            expected_epoch,
            &package,
        ),
        ReplicationKeyWrappingError::AuthenticationFailed,
    );

    let wrong_device =
        ReplicationDevicePrivateKey::from_bytes([0x88; 32]).expect("wrong device key");
    assert_open_error(
        open_wrapped_replication_epoch_key(
            domain.context(),
            &authority_public,
            &wrong_device,
            expected_epoch,
            &package,
        ),
        ReplicationKeyWrappingError::AuthenticationFailed,
    );
    assert_open_error(
        open_wrapped_replication_epoch_key(
            domain.context(),
            &authority_public,
            &device,
            ReplicationKeyEpoch::new(fixture.key_epoch + 1).expect("next epoch"),
            &package,
        ),
        ReplicationKeyWrappingError::KeyEpochMismatch,
    );

    for index in [15_usize, package.to_bytes().len() - 1] {
        let mut tampered = package.to_bytes();
        tampered[index] ^= 1;
        let tampered = WrappedReplicationEpochKey::from_bytes(&tampered)
            .expect("tampered package shape should remain valid");
        assert_open_error(
            open_wrapped_replication_epoch_key(
                domain.context(),
                &authority_public,
                &device,
                expected_epoch,
                &tampered,
            ),
            ReplicationKeyWrappingError::AuthenticationFailed,
        );
    }

    let mut epoch_tamper = package.to_bytes();
    epoch_tamper[7..15].copy_from_slice(&(fixture.key_epoch + 1).to_be_bytes());
    let epoch_tamper = WrappedReplicationEpochKey::from_bytes(&epoch_tamper)
        .expect("tampered epoch is structurally valid");
    assert_open_error(
        open_wrapped_replication_epoch_key(
            domain.context(),
            &authority_public,
            &device,
            ReplicationKeyEpoch::new(fixture.key_epoch + 1).expect("tampered epoch"),
            &epoch_tamper,
        ),
        ReplicationKeyWrappingError::AuthenticationFailed,
    );
}

#[test]
fn malformed_limits_invalid_keys_and_entropy_failure_remain_bounded() {
    let fixture = fixture();
    let domain = domain_fixture(&fixture);
    let authority = authority_private(&fixture);
    let device = device_private(&fixture);
    let device_public = device.public_key();
    let key = epoch_key(&fixture);
    let package = wrapped_fixture(&fixture, &domain, &authority, &device_public, &key);

    assert!(matches!(
        ReplicationAuthorityPrivateKey::from_bytes([0; 32]),
        Err(ReplicationKeyWrappingError::InvalidPrivateKey)
    ));
    assert!(matches!(
        ReplicationDevicePrivateKey::from_bytes([0; 32]),
        Err(ReplicationKeyWrappingError::InvalidPrivateKey)
    ));
    assert!(matches!(
        ReplicationAuthorityPublicKey::from_bytes([0; 32]),
        Err(ReplicationKeyWrappingError::InvalidPublicKey)
    ));
    assert!(matches!(
        ReplicationDevicePublicKey::from_bytes([0; 32]),
        Err(ReplicationKeyWrappingError::InvalidPublicKey)
    ));
    assert!(matches!(
        generate_replication_authority_private_key_with_entropy(&mut FailingEntropy),
        Err(ReplicationKeyWrappingError::RandomUnavailable)
    ));
    assert!(matches!(
        generate_replication_device_private_key_with_entropy(&mut FailingEntropy),
        Err(ReplicationKeyWrappingError::RandomUnavailable)
    ));
    assert_eq!(
        wrap_replication_epoch_key_with_entropy(
            domain.context(),
            &authority,
            &device_public,
            &key,
            &mut FailingEntropy,
        ),
        Err(ReplicationKeyWrappingError::RandomUnavailable)
    );
    assert_eq!(
        wrap_replication_epoch_key_with_entropy(
            domain.context(),
            &authority,
            &device_public,
            &key,
            &mut FixedEntropy([0; 32]),
        ),
        Err(ReplicationKeyWrappingError::RandomUnavailable)
    );

    assert_eq!(
        WrappedReplicationEpochKey::from_bytes(&[]),
        Err(ReplicationKeyWrappingError::MalformedPackage)
    );
    assert_eq!(
        WrappedReplicationEpochKey::from_bytes(&vec![0; MAX_REPLICATION_KEY_PACKAGE_BYTES + 1]),
        Err(ReplicationKeyWrappingError::PackageTooLarge)
    );
    let mut truncated = package.to_bytes();
    truncated.pop();
    assert_eq!(
        WrappedReplicationEpochKey::from_bytes(&truncated),
        Err(ReplicationKeyWrappingError::MalformedPackage)
    );
    let mut magic = package.to_bytes();
    magic[0] ^= 1;
    assert_eq!(
        WrappedReplicationEpochKey::from_bytes(&magic),
        Err(ReplicationKeyWrappingError::MalformedPackage)
    );
    let mut version = package.to_bytes();
    version[4..6].copy_from_slice(&2_u16.to_be_bytes());
    assert_eq!(
        WrappedReplicationEpochKey::from_bytes(&version),
        Err(ReplicationKeyWrappingError::UnsupportedVersion)
    );
    let mut suite = package.to_bytes();
    suite[6] = 99;
    assert_eq!(
        WrappedReplicationEpochKey::from_bytes(&suite),
        Err(ReplicationKeyWrappingError::UnsupportedSuite)
    );
    let mut zero_epoch = package.to_bytes();
    zero_epoch[7..15].fill(0);
    assert_eq!(
        WrappedReplicationEpochKey::from_bytes(&zero_epoch),
        Err(ReplicationKeyWrappingError::InvalidKeyEpoch)
    );
    let mut length = package.to_bytes();
    length[47..49].copy_from_slice(&47_u16.to_be_bytes());
    assert_eq!(
        WrappedReplicationEpochKey::from_bytes(&length),
        Err(ReplicationKeyWrappingError::MalformedPackage)
    );

    let invalid_workspace: ReplicationWorkspaceId =
        serde_json::from_str("\"contains spaces\"").expect("transparent invalid workspace");
    let invalid_context = ReplicationKeyWrapContext {
        workspace_id: &invalid_workspace,
        ..domain.context()
    };
    assert_eq!(
        wrap_replication_epoch_key_with_entropy(
            invalid_context,
            &authority,
            &device_public,
            &key,
            &mut fixed_entropy(&fixture.ephemeral_ikm_hex),
        ),
        Err(ReplicationKeyWrappingError::InvalidContext)
    );
}

#[test]
fn private_epoch_and_package_debug_contracts_do_not_expose_sensitive_material() {
    fn assert_zeroize_on_drop<T: ZeroizeOnDrop>() {}
    assert_zeroize_on_drop::<ReplicationAuthorityPrivateKey>();
    assert_zeroize_on_drop::<ReplicationDevicePrivateKey>();
    assert_zeroize_on_drop::<ReplicationEpochKey>();

    let fixture = fixture();
    let domain = domain_fixture(&fixture);
    let authority = authority_private(&fixture);
    let device = device_private(&fixture);
    let key = epoch_key(&fixture);
    let package = wrapped_fixture(&fixture, &domain, &authority, &device.public_key(), &key);

    for debug in [
        format!("{authority:?}"),
        format!("{device:?}"),
        format!("{:?}", authority.public_key()),
        format!("{:?}", device.public_key()),
        format!("{:?}", domain.context()),
        format!("{package:?}"),
    ] {
        assert!(!debug.contains(&fixture.authority_private_hex));
        assert!(!debug.contains(&fixture.device_private_hex));
        assert!(!debug.contains(&fixture.epoch_key_hex));
        assert!(!debug.contains(&fixture.workspace_id));
        assert!(!debug.contains(&fixture.recipient));
        if !fixture.package_hex.is_empty() {
            assert!(!debug.contains(&fixture.package_hex));
        }
    }
    assert!(format!("{authority:?}").contains("<redacted>"));
    assert!(format!("{device:?}").contains("<redacted>"));
    assert!(format!("{package:?}").contains("<redacted>"));
    assert_eq!(
        format!("{:?}", ReplicationKeyWrappingError::AuthenticationFailed),
        "AuthenticationFailed"
    );
    assert_eq!(
        ReplicationKeyWrappingSuite::HpkeAuthX25519HkdfSha256ChaCha20Poly1305 as u8,
        fixture.suite_id
    );
}
