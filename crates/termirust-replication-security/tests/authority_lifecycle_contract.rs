use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;

use serde::Deserialize;
use termirust_domain::{
    MAX_REPLICATION_REPLICAS, ReplicationError, ReplicationOperation, ReplicationReplicaId,
    ReplicationVersionVector, ReplicationWorkspaceId, SealedReplicationPayload,
};
use termirust_replication_security::{
    REPLICATION_AUTHORITY_STATE_VERSION, ReplicationAuthorityDeviceStatus,
    ReplicationAuthorityError, ReplicationAuthorityPrivateKey, ReplicationAuthorityRevision,
    ReplicationAuthorityTransition, ReplicationDevicePrivateKey, ReplicationEntropy,
    ReplicationEntropyError, ReplicationEpochKey, ReplicationKeyWrapContext,
    bootstrap_replication_authority_with_entropy, enroll_replication_device_with_entropy,
    open_wrapped_replication_epoch_key, revoke_replication_device_with_entropy,
    rotate_replication_epoch_with_entropy,
};

#[derive(Debug, Deserialize)]
struct LifecycleFixture {
    schema_version: u16,
    workspace_id: String,
    authority_private_hex: String,
    authority_public_hex: String,
    device_a: DeviceFixture,
    device_b: DeviceFixture,
    bootstrap_entropy_hex: Vec<String>,
    bootstrap_packages: BTreeMap<String, String>,
    enroll_entropy_hex: Vec<String>,
    enroll_packages: BTreeMap<String, String>,
    rotate_entropy_hex: Vec<String>,
    rotate_packages: BTreeMap<String, String>,
    revoke_entropy_hex: Vec<String>,
    revoke_packages: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct DeviceFixture {
    replica_id: String,
    private_hex: String,
    public_hex: String,
}

struct ScriptedEntropy {
    blocks: VecDeque<[u8; 32]>,
    calls: usize,
    fail_at: Option<usize>,
}

impl ScriptedEntropy {
    fn new(values: &[String]) -> Self {
        Self {
            blocks: values.iter().map(|value| bytes32(value)).collect(),
            calls: 0,
            fail_at: None,
        }
    }

    fn counter() -> Self {
        Self {
            blocks: (1_u8..=240).map(|value| [value; 32]).collect(),
            calls: 0,
            fail_at: None,
        }
    }

    fn failing(fail_at: usize) -> Self {
        Self {
            blocks: (1_u8..=16).map(|value| [value; 32]).collect(),
            calls: 0,
            fail_at: Some(fail_at),
        }
    }

    fn zero() -> Self {
        Self {
            blocks: VecDeque::from([[0; 32]]),
            calls: 0,
            fail_at: None,
        }
    }
}

impl ReplicationEntropy for ScriptedEntropy {
    fn fill(&mut self, destination: &mut [u8]) -> Result<(), ReplicationEntropyError> {
        assert_eq!(destination.len(), 32);
        let call = self.calls;
        self.calls += 1;
        if self.fail_at == Some(call) {
            return Err(ReplicationEntropyError);
        }
        let block = self.blocks.pop_front().ok_or(ReplicationEntropyError)?;
        destination.copy_from_slice(&block);
        Ok(())
    }
}

fn fixture() -> LifecycleFixture {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/replication/authority-lifecycle-v1.json");
    serde_json::from_slice(&std::fs::read(path).expect("authority fixture should be readable"))
        .expect("authority fixture should parse")
}

fn bytes32(value: &str) -> [u8; 32] {
    hex::decode(value)
        .expect("fixture hex")
        .try_into()
        .expect("fixture 32-byte value")
}

fn authority(fixture: &LifecycleFixture) -> ReplicationAuthorityPrivateKey {
    ReplicationAuthorityPrivateKey::from_bytes(bytes32(&fixture.authority_private_hex))
        .expect("fixture authority key")
}

fn device_private(device: &DeviceFixture) -> ReplicationDevicePrivateKey {
    ReplicationDevicePrivateKey::from_bytes(bytes32(&device.private_hex))
        .expect("fixture device key")
}

fn replica(device: &DeviceFixture) -> ReplicationReplicaId {
    ReplicationReplicaId::new(device.replica_id.clone()).expect("fixture replica ID")
}

fn workspace(fixture: &LifecycleFixture) -> ReplicationWorkspaceId {
    ReplicationWorkspaceId::new(fixture.workspace_id.clone()).expect("fixture workspace")
}

fn packages_hex(
    distribution: &termirust_replication_security::ReplicationEpochDistribution,
) -> BTreeMap<String, String> {
    distribution
        .packages()
        .iter()
        .map(|recipient| {
            (
                recipient.recipient().as_str().to_owned(),
                hex::encode(recipient.package().to_bytes()),
            )
        })
        .collect()
}

fn assert_exact_packages(
    label: &str,
    actual: &BTreeMap<String, String>,
    expected: &BTreeMap<String, String>,
) {
    assert_eq!(actual, expected, "{label} packages changed");
}

fn assert_device_opens(
    transition: &ReplicationAuthorityTransition,
    replica_id: &ReplicationReplicaId,
    device: &ReplicationDevicePrivateKey,
) {
    let package = transition
        .distribution()
        .packages()
        .iter()
        .find(|package| package.recipient() == replica_id)
        .expect("active recipient should have a package");
    open_wrapped_replication_epoch_key(
        ReplicationKeyWrapContext {
            workspace_id: transition.state().workspace_id(),
            recipient: replica_id,
        },
        transition.state().authority_public_key(),
        device,
        transition.state().key_epoch(),
        package.package(),
    )
    .expect("active recipient should open its package");
}

#[test]
fn exact_bootstrap_enroll_rotate_and_revoke_lifecycle_is_deterministic() {
    let fixture = fixture();
    assert_eq!(fixture.schema_version, REPLICATION_AUTHORITY_STATE_VERSION);
    let authority = authority(&fixture);
    assert_eq!(
        hex::encode(authority.public_key().as_bytes()),
        fixture.authority_public_hex
    );
    let device_a = device_private(&fixture.device_a);
    let device_b = device_private(&fixture.device_b);
    assert_eq!(
        hex::encode(device_a.public_key().as_bytes()),
        fixture.device_a.public_hex
    );
    assert_eq!(
        hex::encode(device_b.public_key().as_bytes()),
        fixture.device_b.public_hex
    );

    let bootstrap = bootstrap_replication_authority_with_entropy(
        workspace(&fixture),
        &authority,
        replica(&fixture.device_a),
        device_a.public_key(),
        &mut ScriptedEntropy::new(&fixture.bootstrap_entropy_hex),
    )
    .expect("bootstrap should succeed");
    assert_eq!(bootstrap.base_revision(), None);
    assert_eq!(bootstrap.state().revision().get(), 1);
    assert_eq!(bootstrap.state().key_epoch().get(), 1);
    assert_exact_packages(
        "bootstrap",
        &packages_hex(bootstrap.distribution()),
        &fixture.bootstrap_packages,
    );
    assert_device_opens(&bootstrap, &replica(&fixture.device_a), &device_a);
    let (state, _key, _) = bootstrap.into_parts();

    let enroll = enroll_replication_device_with_entropy(
        &state,
        &authority,
        state.revision(),
        replica(&fixture.device_b),
        device_b.public_key(),
        &mut ScriptedEntropy::new(&fixture.enroll_entropy_hex),
    )
    .expect("enrollment should succeed");
    assert_eq!(enroll.base_revision(), Some(state.revision()));
    assert_eq!(enroll.state().revision().get(), 2);
    assert_eq!(enroll.state().key_epoch().get(), 2);
    assert_exact_packages(
        "enroll",
        &packages_hex(enroll.distribution()),
        &fixture.enroll_packages,
    );
    assert_device_opens(&enroll, &replica(&fixture.device_a), &device_a);
    assert_device_opens(&enroll, &replica(&fixture.device_b), &device_b);
    let (state, _key, _) = enroll.into_parts();

    let rotate = rotate_replication_epoch_with_entropy(
        &state,
        &authority,
        state.revision(),
        &mut ScriptedEntropy::new(&fixture.rotate_entropy_hex),
    )
    .expect("rotation should succeed");
    assert_eq!(rotate.state().revision().get(), 3);
    assert_eq!(rotate.state().key_epoch().get(), 3);
    assert_exact_packages(
        "rotate",
        &packages_hex(rotate.distribution()),
        &fixture.rotate_packages,
    );
    assert_device_opens(&rotate, &replica(&fixture.device_a), &device_a);
    assert_device_opens(&rotate, &replica(&fixture.device_b), &device_b);
    let (state, _key, _) = rotate.into_parts();

    let revoke = revoke_replication_device_with_entropy(
        &state,
        &authority,
        state.revision(),
        &replica(&fixture.device_b),
        7,
        &mut ScriptedEntropy::new(&fixture.revoke_entropy_hex),
    )
    .expect("revocation should succeed");
    assert_eq!(revoke.state().revision().get(), 4);
    assert_eq!(revoke.state().key_epoch().get(), 4);
    assert_eq!(revoke.state().active_device_count(), 1);
    assert_eq!(revoke.distribution().packages().len(), 1);
    assert_exact_packages(
        "revoke",
        &packages_hex(revoke.distribution()),
        &fixture.revoke_packages,
    );
    assert_device_opens(&revoke, &replica(&fixture.device_a), &device_a);
}

#[test]
fn revoked_device_is_excluded_and_policy_rejects_post_cutoff_history() {
    let fixture = fixture();
    let authority = authority(&fixture);
    let device_a = device_private(&fixture.device_a);
    let device_b = device_private(&fixture.device_b);
    let bootstrap = bootstrap_replication_authority_with_entropy(
        workspace(&fixture),
        &authority,
        replica(&fixture.device_a),
        device_a.public_key(),
        &mut ScriptedEntropy::new(&fixture.bootstrap_entropy_hex),
    )
    .expect("bootstrap");
    let (state, _key, _) = bootstrap.into_parts();
    let enroll = enroll_replication_device_with_entropy(
        &state,
        &authority,
        state.revision(),
        replica(&fixture.device_b),
        device_b.public_key(),
        &mut ScriptedEntropy::new(&fixture.enroll_entropy_hex),
    )
    .expect("enroll");
    let (state, _key, _) = enroll.into_parts();
    let revoke = revoke_replication_device_with_entropy(
        &state,
        &authority,
        state.revision(),
        &replica(&fixture.device_b),
        7,
        &mut ScriptedEntropy::new(&fixture.revoke_entropy_hex),
    )
    .expect("revoke");

    let recipients: Vec<_> = revoke
        .distribution()
        .packages()
        .iter()
        .map(|package| package.recipient().as_str())
        .collect();
    assert_eq!(recipients, [fixture.device_a.replica_id.as_str()]);
    let package = revoke.distribution().packages()[0].package();
    open_wrapped_replication_epoch_key(
        ReplicationKeyWrapContext {
            workspace_id: revoke.state().workspace_id(),
            recipient: &replica(&fixture.device_a),
        },
        revoke.state().authority_public_key(),
        &device_a,
        revoke.state().key_epoch(),
        package,
    )
    .expect("remaining device should open the new epoch");

    let revoked = replica(&fixture.device_b);
    assert!(matches!(
        revoke
            .state()
            .device(&revoked)
            .expect("device record")
            .status(),
        ReplicationAuthorityDeviceStatus::Revoked {
            accepted_through: 7,
            ..
        }
    ));
    let policy = revoke.state().replication_policy().expect("policy");
    let operation = ReplicationOperation::Put {
        sealed_payload: SealedReplicationPayload::new(vec![1]).expect("sealed payload"),
    };
    assert_eq!(
        policy.next_version(&revoked, &[], operation),
        Err(ReplicationError::ReplicaNotActive)
    );
    let accepted = ReplicationVersionVector::new([(revoked.clone(), 7)]).expect("accepted vector");
    termirust_domain::ReplicatedVersion::new(
        revoked.clone(),
        accepted,
        ReplicationOperation::Delete {
            sealed_payload: SealedReplicationPayload::new(vec![2]).expect("sealed tombstone"),
        },
        &policy,
    )
    .expect("history at the revocation cutoff should remain valid");
    let rejected = termirust_domain::ReplicatedVersion {
        author: revoked.clone(),
        vector: ReplicationVersionVector::new([(revoked, 8)]).expect("post-cutoff vector"),
        operation: ReplicationOperation::Delete {
            sealed_payload: SealedReplicationPayload::new(vec![3]).expect("sealed tombstone"),
        },
    };
    assert_eq!(
        termirust_domain::ReplicatedVersion::new(
            rejected.author,
            rejected.vector,
            rejected.operation,
            &policy,
        ),
        Err(ReplicationError::PostRevocationCounter)
    );
}

#[test]
fn stale_duplicate_wrong_authority_and_entropy_fail_without_state_change() {
    let fixture = fixture();
    let authority = authority(&fixture);
    let device_a = device_private(&fixture.device_a);
    let device_b = device_private(&fixture.device_b);
    let bootstrap = bootstrap_replication_authority_with_entropy(
        workspace(&fixture),
        &authority,
        replica(&fixture.device_a),
        device_a.public_key(),
        &mut ScriptedEntropy::new(&fixture.bootstrap_entropy_hex),
    )
    .expect("bootstrap");
    let (state, _key, _) = bootstrap.into_parts();
    let baseline = state.clone();

    let mut unused_entropy = ScriptedEntropy::counter();
    assert!(matches!(
        enroll_replication_device_with_entropy(
            &state,
            &authority,
            ReplicationAuthorityRevision::new(2).expect("future revision"),
            replica(&fixture.device_b),
            device_b.public_key(),
            &mut unused_entropy,
        ),
        Err(ReplicationAuthorityError::StaleRevision)
    ));
    assert_eq!(unused_entropy.calls, 0);

    let wrong_authority =
        ReplicationAuthorityPrivateKey::from_bytes([0x55; 32]).expect("wrong authority");
    assert!(matches!(
        rotate_replication_epoch_with_entropy(
            &state,
            &wrong_authority,
            state.revision(),
            &mut unused_entropy,
        ),
        Err(ReplicationAuthorityError::AuthorityMismatch)
    ));
    assert_eq!(unused_entropy.calls, 0);

    assert!(matches!(
        enroll_replication_device_with_entropy(
            &state,
            &authority,
            state.revision(),
            replica(&fixture.device_a),
            device_b.public_key(),
            &mut unused_entropy,
        ),
        Err(ReplicationAuthorityError::DeviceAlreadyExists)
    ));
    assert!(matches!(
        enroll_replication_device_with_entropy(
            &state,
            &authority,
            state.revision(),
            replica(&fixture.device_b),
            device_a.public_key(),
            &mut unused_entropy,
        ),
        Err(ReplicationAuthorityError::DuplicateDeviceKey)
    ));
    assert!(matches!(
        revoke_replication_device_with_entropy(
            &state,
            &authority,
            state.revision(),
            &replica(&fixture.device_b),
            0,
            &mut unused_entropy,
        ),
        Err(ReplicationAuthorityError::UnknownDevice)
    ));
    assert!(matches!(
        revoke_replication_device_with_entropy(
            &state,
            &authority,
            state.revision(),
            &replica(&fixture.device_a),
            0,
            &mut unused_entropy,
        ),
        Err(ReplicationAuthorityError::LastActiveDevice)
    ));
    assert_eq!(state, baseline);

    for fail_at in [0, 1] {
        let mut entropy = ScriptedEntropy::failing(fail_at);
        assert!(matches!(
            rotate_replication_epoch_with_entropy(
                &state,
                &authority,
                state.revision(),
                &mut entropy,
            ),
            Err(ReplicationAuthorityError::RandomUnavailable)
        ));
        assert_eq!(state, baseline);
    }
    let mut zero_entropy = ScriptedEntropy::zero();
    assert!(matches!(
        rotate_replication_epoch_with_entropy(
            &state,
            &authority,
            state.revision(),
            &mut zero_entropy,
        ),
        Err(ReplicationAuthorityError::RandomUnavailable)
    ));
    assert_eq!(state, baseline);

    let enroll = enroll_replication_device_with_entropy(
        &state,
        &authority,
        state.revision(),
        replica(&fixture.device_b),
        device_b.public_key(),
        &mut ScriptedEntropy::new(&fixture.enroll_entropy_hex),
    )
    .expect("second active device");
    let (enrolled_state, _key, _) = enroll.into_parts();
    let enrolled_baseline = enrolled_state.clone();
    let mut fail_second_recipient = ScriptedEntropy::failing(2);
    assert!(matches!(
        rotate_replication_epoch_with_entropy(
            &enrolled_state,
            &authority,
            enrolled_state.revision(),
            &mut fail_second_recipient,
        ),
        Err(ReplicationAuthorityError::RandomUnavailable)
    ));
    assert_eq!(enrolled_state, enrolled_baseline);

    let revoked = revoke_replication_device_with_entropy(
        &enrolled_state,
        &authority,
        enrolled_state.revision(),
        &replica(&fixture.device_b),
        7,
        &mut ScriptedEntropy::new(&fixture.revoke_entropy_hex),
    )
    .expect("first revocation");
    let (revoked_state, _key, _) = revoked.into_parts();
    let mut no_entropy = ScriptedEntropy::counter();
    assert!(matches!(
        revoke_replication_device_with_entropy(
            &revoked_state,
            &authority,
            revoked_state.revision(),
            &replica(&fixture.device_b),
            7,
            &mut no_entropy,
        ),
        Err(ReplicationAuthorityError::DeviceAlreadyRevoked)
    ));
    assert_eq!(no_entropy.calls, 0);
}

#[test]
fn capacity_ordering_and_debug_output_are_bounded_and_redacted() {
    fn assert_zeroize_on_drop<T: zeroize::ZeroizeOnDrop>() {}

    assert_zeroize_on_drop::<ReplicationEpochKey>();
    assert_zeroize_on_drop::<ReplicationAuthorityPrivateKey>();
    assert_zeroize_on_drop::<ReplicationDevicePrivateKey>();

    let fixture = fixture();
    let authority = authority(&fixture);
    let device_a = device_private(&fixture.device_a);
    let mut entropy = ScriptedEntropy::counter();
    let bootstrap = bootstrap_replication_authority_with_entropy(
        workspace(&fixture),
        &authority,
        replica(&fixture.device_a),
        device_a.public_key(),
        &mut entropy,
    )
    .expect("bootstrap");
    let (mut state, _key, _) = bootstrap.into_parts();

    for index in 1..MAX_REPLICATION_REPLICAS {
        let private = ReplicationDevicePrivateKey::from_bytes([index as u8 + 2; 32])
            .expect("synthetic device private key");
        let transition = enroll_replication_device_with_entropy(
            &state,
            &authority,
            state.revision(),
            ReplicationReplicaId::new(format!("device-{index:02}")).expect("replica ID"),
            private.public_key(),
            &mut entropy,
        )
        .expect("device within capacity should enroll");
        let recipients: Vec<_> = transition
            .distribution()
            .packages()
            .iter()
            .map(|package| package.recipient().as_str())
            .collect();
        assert!(recipients.windows(2).all(|pair| pair[0] < pair[1]));
        let (next_state, _, _) = transition.into_parts();
        state = next_state;
    }
    assert_eq!(state.device_count(), MAX_REPLICATION_REPLICAS);

    let calls_before = entropy.calls;
    let overflow_key = ReplicationDevicePrivateKey::from_bytes([0xfe; 32])
        .expect("overflow device key")
        .public_key();
    assert!(matches!(
        enroll_replication_device_with_entropy(
            &state,
            &authority,
            state.revision(),
            ReplicationReplicaId::new("device-overflow").expect("overflow replica"),
            overflow_key,
            &mut entropy,
        ),
        Err(ReplicationAuthorityError::TooManyDevices)
    ));
    assert_eq!(entropy.calls, calls_before);

    let debug = format!("{state:?} {:?}", state.devices().next().expect("device"));
    for forbidden in [
        fixture.workspace_id.as_str(),
        fixture.device_a.replica_id.as_str(),
        fixture.device_a.public_hex.as_str(),
        fixture.authority_public_hex.as_str(),
    ] {
        assert!(!debug.contains(forbidden), "debug leaked {forbidden}");
    }
    assert!(!format!("{:?}", ReplicationAuthorityError::WrappingFailed).contains("device-a"));
    assert_eq!(
        ReplicationAuthorityRevision::new(0),
        Err(ReplicationAuthorityError::InvalidRevision)
    );
}
