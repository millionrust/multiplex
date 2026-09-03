use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use termirust_domain::{
    MAX_REPLICATION_REPLICAS, ReplicaAuthorization, ReplicationPolicy, ReplicationReplicaId,
    ReplicationWorkspaceId,
};
use zeroize::{Zeroize, Zeroizing};

use crate::{
    OsReplicationEntropy, ReplicationAuthorityPrivateKey, ReplicationAuthorityPublicKey,
    ReplicationDevicePublicKey, ReplicationEntropy, ReplicationEpochKey, ReplicationKeyEpoch,
    ReplicationKeyWrapContext, ReplicationKeyWrappingError, WrappedReplicationEpochKey,
    wrap_replication_epoch_key_with_entropy,
};

pub const REPLICATION_AUTHORITY_STATE_VERSION: u16 = 1;
pub const MAX_REPLICATION_AUTHORITY_STATE_BYTES: usize = 64 * 1024;
const INITIAL_AUTHORITY_REVISION: u64 = 1;
const INITIAL_KEY_EPOCH: u64 = 1;
const REPLICATION_EPOCH_KEY_BYTES: usize = 32;

#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ReplicationAuthorityRevision(u64);

impl ReplicationAuthorityRevision {
    pub fn new(value: u64) -> Result<Self, ReplicationAuthorityError> {
        if value == 0 {
            return Err(ReplicationAuthorityError::InvalidRevision);
        }
        Ok(Self(value))
    }

    pub fn get(self) -> u64 {
        self.0
    }

    fn next(self) -> Result<Self, ReplicationAuthorityError> {
        Self::new(
            self.0
                .checked_add(1)
                .ok_or(ReplicationAuthorityError::RevisionOverflow)?,
        )
    }
}

impl fmt::Debug for ReplicationAuthorityRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("ReplicationAuthorityRevision")
            .field(&self.0)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplicationAuthorityDeviceStatus {
    Active,
    Revoked {
        accepted_through: u64,
        revoked_at_revision: ReplicationAuthorityRevision,
        revoked_at_epoch: ReplicationKeyEpoch,
    },
}

#[derive(Clone, Eq, PartialEq)]
pub struct ReplicationAuthorityDevice {
    replica_id: ReplicationReplicaId,
    public_key: ReplicationDevicePublicKey,
    status: ReplicationAuthorityDeviceStatus,
}

impl ReplicationAuthorityDevice {
    pub fn replica_id(&self) -> &ReplicationReplicaId {
        &self.replica_id
    }

    pub fn public_key(&self) -> &ReplicationDevicePublicKey {
        &self.public_key
    }

    pub fn status(&self) -> ReplicationAuthorityDeviceStatus {
        self.status
    }
}

impl fmt::Debug for ReplicationAuthorityDevice {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplicationAuthorityDevice")
            .field("replica_id", &"<redacted>")
            .field("public_key", &"<redacted>")
            .field("status", &self.status)
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct ReplicationAuthorityState {
    version: u16,
    workspace_id: ReplicationWorkspaceId,
    authority_public_key: ReplicationAuthorityPublicKey,
    revision: ReplicationAuthorityRevision,
    key_epoch: ReplicationKeyEpoch,
    devices: BTreeMap<ReplicationReplicaId, ReplicationAuthorityDevice>,
}

impl ReplicationAuthorityState {
    pub fn version(&self) -> u16 {
        self.version
    }

    pub fn workspace_id(&self) -> &ReplicationWorkspaceId {
        &self.workspace_id
    }

    pub fn authority_public_key(&self) -> &ReplicationAuthorityPublicKey {
        &self.authority_public_key
    }

    pub fn revision(&self) -> ReplicationAuthorityRevision {
        self.revision
    }

    pub fn key_epoch(&self) -> ReplicationKeyEpoch {
        self.key_epoch
    }

    pub fn device_count(&self) -> usize {
        self.devices.len()
    }

    pub fn active_device_count(&self) -> usize {
        self.devices
            .values()
            .filter(|device| device.status == ReplicationAuthorityDeviceStatus::Active)
            .count()
    }

    pub fn device(&self, replica_id: &ReplicationReplicaId) -> Option<&ReplicationAuthorityDevice> {
        self.devices.get(replica_id)
    }

    pub fn devices(
        &self,
    ) -> impl ExactSizeIterator<Item = &ReplicationAuthorityDevice> + DoubleEndedIterator {
        self.devices.values()
    }

    pub fn replication_policy(&self) -> Result<ReplicationPolicy, ReplicationAuthorityError> {
        self.validate()?;
        ReplicationPolicy::new(self.devices.iter().map(|(replica_id, device)| {
            let authorization = match device.status {
                ReplicationAuthorityDeviceStatus::Active => ReplicaAuthorization::Active,
                ReplicationAuthorityDeviceStatus::Revoked {
                    accepted_through, ..
                } => ReplicaAuthorization::Revoked { accepted_through },
            };
            (replica_id.clone(), authorization)
        }))
        .map_err(|_| ReplicationAuthorityError::InvalidState)
    }

    pub fn to_canonical_bytes(&self) -> Result<Vec<u8>, ReplicationAuthorityError> {
        self.validate()?;
        let stored = StoredAuthorityState::from_state(self);
        let bytes =
            serde_json::to_vec(&stored).map_err(|_| ReplicationAuthorityError::InvalidState)?;
        if bytes.len() > MAX_REPLICATION_AUTHORITY_STATE_BYTES {
            return Err(ReplicationAuthorityError::StateTooLarge);
        }
        Ok(bytes)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, ReplicationAuthorityError> {
        if bytes.len() > MAX_REPLICATION_AUTHORITY_STATE_BYTES {
            return Err(ReplicationAuthorityError::StateTooLarge);
        }
        let stored: StoredAuthorityState =
            serde_json::from_slice(bytes).map_err(|_| ReplicationAuthorityError::InvalidState)?;
        let state = stored.into_state()?;
        if state.to_canonical_bytes()?.as_slice() != bytes {
            return Err(ReplicationAuthorityError::NonCanonicalState);
        }
        Ok(state)
    }

    pub fn validate(&self) -> Result<(), ReplicationAuthorityError> {
        if self.version != REPLICATION_AUTHORITY_STATE_VERSION {
            return Err(ReplicationAuthorityError::UnsupportedStateVersion);
        }
        self.workspace_id
            .validate()
            .map_err(|_| ReplicationAuthorityError::InvalidState)?;
        if self.devices.is_empty() || self.devices.len() > MAX_REPLICATION_REPLICAS {
            return Err(ReplicationAuthorityError::InvalidState);
        }

        let mut active_devices = 0_usize;
        let mut public_keys = BTreeSet::new();
        for (replica_id, device) in &self.devices {
            if replica_id != &device.replica_id {
                return Err(ReplicationAuthorityError::InvalidState);
            }
            replica_id
                .validate()
                .map_err(|_| ReplicationAuthorityError::InvalidState)?;
            if !public_keys.insert(*device.public_key.as_bytes()) {
                return Err(ReplicationAuthorityError::DuplicateDeviceKey);
            }
            match device.status {
                ReplicationAuthorityDeviceStatus::Active => active_devices += 1,
                ReplicationAuthorityDeviceStatus::Revoked {
                    revoked_at_revision,
                    revoked_at_epoch,
                    ..
                } => {
                    if revoked_at_revision > self.revision || revoked_at_epoch > self.key_epoch {
                        return Err(ReplicationAuthorityError::InvalidState);
                    }
                }
            }
        }
        if active_devices == 0 {
            return Err(ReplicationAuthorityError::LastActiveDevice);
        }
        Ok(())
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredAuthorityState {
    version: u16,
    workspace_id: String,
    authority_public_key: [u8; 32],
    revision: u64,
    key_epoch: u64,
    devices: Vec<StoredAuthorityDevice>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredAuthorityDevice {
    replica_id: String,
    public_key: [u8; 32],
    status: StoredAuthorityDeviceStatus,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum StoredAuthorityDeviceStatus {
    Active,
    Revoked {
        accepted_through: u64,
        revoked_at_revision: u64,
        revoked_at_epoch: u64,
    },
}

impl StoredAuthorityState {
    fn from_state(state: &ReplicationAuthorityState) -> Self {
        Self {
            version: state.version,
            workspace_id: state.workspace_id.as_str().to_string(),
            authority_public_key: *state.authority_public_key.as_bytes(),
            revision: state.revision.get(),
            key_epoch: state.key_epoch.get(),
            devices: state
                .devices
                .values()
                .map(|device| StoredAuthorityDevice {
                    replica_id: device.replica_id.as_str().to_string(),
                    public_key: *device.public_key.as_bytes(),
                    status: match device.status {
                        ReplicationAuthorityDeviceStatus::Active => {
                            StoredAuthorityDeviceStatus::Active
                        }
                        ReplicationAuthorityDeviceStatus::Revoked {
                            accepted_through,
                            revoked_at_revision,
                            revoked_at_epoch,
                        } => StoredAuthorityDeviceStatus::Revoked {
                            accepted_through,
                            revoked_at_revision: revoked_at_revision.get(),
                            revoked_at_epoch: revoked_at_epoch.get(),
                        },
                    },
                })
                .collect(),
        }
    }

    fn into_state(self) -> Result<ReplicationAuthorityState, ReplicationAuthorityError> {
        if self.version != REPLICATION_AUTHORITY_STATE_VERSION {
            return Err(ReplicationAuthorityError::UnsupportedStateVersion);
        }
        let workspace_id = ReplicationWorkspaceId::new(self.workspace_id)
            .map_err(|_| ReplicationAuthorityError::InvalidState)?;
        let authority_public_key =
            ReplicationAuthorityPublicKey::from_bytes(self.authority_public_key)
                .map_err(|_| ReplicationAuthorityError::InvalidState)?;
        let revision = ReplicationAuthorityRevision::new(self.revision)?;
        let key_epoch = ReplicationKeyEpoch::new(self.key_epoch)
            .map_err(|_| ReplicationAuthorityError::InvalidState)?;
        let mut devices = BTreeMap::new();
        for stored in self.devices {
            let replica_id = ReplicationReplicaId::new(stored.replica_id)
                .map_err(|_| ReplicationAuthorityError::InvalidState)?;
            let public_key = ReplicationDevicePublicKey::from_bytes(stored.public_key)
                .map_err(|_| ReplicationAuthorityError::InvalidState)?;
            let status = match stored.status {
                StoredAuthorityDeviceStatus::Active => ReplicationAuthorityDeviceStatus::Active,
                StoredAuthorityDeviceStatus::Revoked {
                    accepted_through,
                    revoked_at_revision,
                    revoked_at_epoch,
                } => ReplicationAuthorityDeviceStatus::Revoked {
                    accepted_through,
                    revoked_at_revision: ReplicationAuthorityRevision::new(revoked_at_revision)?,
                    revoked_at_epoch: ReplicationKeyEpoch::new(revoked_at_epoch)
                        .map_err(|_| ReplicationAuthorityError::InvalidState)?,
                },
            };
            let device = ReplicationAuthorityDevice {
                replica_id: replica_id.clone(),
                public_key,
                status,
            };
            if devices.insert(replica_id, device).is_some() {
                return Err(ReplicationAuthorityError::InvalidState);
            }
        }
        let state = ReplicationAuthorityState {
            version: self.version,
            workspace_id,
            authority_public_key,
            revision,
            key_epoch,
            devices,
        };
        state.validate()?;
        Ok(state)
    }
}

impl fmt::Debug for ReplicationAuthorityState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplicationAuthorityState")
            .field("version", &self.version)
            .field("workspace_id", &"<redacted>")
            .field("authority_public_key", &"<redacted>")
            .field("revision", &self.revision)
            .field("key_epoch", &self.key_epoch)
            .field("device_count", &self.device_count())
            .field("active_device_count", &self.active_device_count())
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct ReplicationRecipientKeyPackage {
    recipient: ReplicationReplicaId,
    package: WrappedReplicationEpochKey,
}

impl ReplicationRecipientKeyPackage {
    pub fn recipient(&self) -> &ReplicationReplicaId {
        &self.recipient
    }

    pub fn package(&self) -> &WrappedReplicationEpochKey {
        &self.package
    }
}

impl fmt::Debug for ReplicationRecipientKeyPackage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplicationRecipientKeyPackage")
            .field("recipient", &"<redacted>")
            .field("package", &self.package)
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct ReplicationEpochDistribution {
    authority_revision: ReplicationAuthorityRevision,
    key_epoch: ReplicationKeyEpoch,
    packages: Vec<ReplicationRecipientKeyPackage>,
}

impl ReplicationEpochDistribution {
    pub fn authority_revision(&self) -> ReplicationAuthorityRevision {
        self.authority_revision
    }

    pub fn key_epoch(&self) -> ReplicationKeyEpoch {
        self.key_epoch
    }

    pub fn packages(&self) -> &[ReplicationRecipientKeyPackage] {
        &self.packages
    }
}

impl fmt::Debug for ReplicationEpochDistribution {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplicationEpochDistribution")
            .field("authority_revision", &self.authority_revision)
            .field("key_epoch", &self.key_epoch)
            .field("recipient_count", &self.packages.len())
            .field("packages", &"<redacted>")
            .finish()
    }
}

pub struct ReplicationAuthorityTransition {
    base_revision: Option<ReplicationAuthorityRevision>,
    state: ReplicationAuthorityState,
    epoch_key: ReplicationEpochKey,
    distribution: ReplicationEpochDistribution,
}

impl ReplicationAuthorityTransition {
    pub fn base_revision(&self) -> Option<ReplicationAuthorityRevision> {
        self.base_revision
    }

    pub fn state(&self) -> &ReplicationAuthorityState {
        &self.state
    }

    pub fn distribution(&self) -> &ReplicationEpochDistribution {
        &self.distribution
    }

    pub fn into_parts(
        self,
    ) -> (
        ReplicationAuthorityState,
        ReplicationEpochKey,
        ReplicationEpochDistribution,
    ) {
        (self.state, self.epoch_key, self.distribution)
    }
}

impl fmt::Debug for ReplicationAuthorityTransition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplicationAuthorityTransition")
            .field("base_revision", &self.base_revision)
            .field("state", &self.state)
            .field("epoch_key", &"<redacted>")
            .field("distribution", &self.distribution)
            .finish()
    }
}

pub fn bootstrap_replication_authority(
    workspace_id: ReplicationWorkspaceId,
    authority_private: &ReplicationAuthorityPrivateKey,
    initial_replica: ReplicationReplicaId,
    initial_device_public_key: ReplicationDevicePublicKey,
) -> Result<ReplicationAuthorityTransition, ReplicationAuthorityError> {
    bootstrap_replication_authority_with_entropy(
        workspace_id,
        authority_private,
        initial_replica,
        initial_device_public_key,
        &mut OsReplicationEntropy,
    )
}

pub fn bootstrap_replication_authority_with_entropy(
    workspace_id: ReplicationWorkspaceId,
    authority_private: &ReplicationAuthorityPrivateKey,
    initial_replica: ReplicationReplicaId,
    initial_device_public_key: ReplicationDevicePublicKey,
    entropy: &mut impl ReplicationEntropy,
) -> Result<ReplicationAuthorityTransition, ReplicationAuthorityError> {
    workspace_id
        .validate()
        .map_err(|_| ReplicationAuthorityError::InvalidState)?;
    initial_replica
        .validate()
        .map_err(|_| ReplicationAuthorityError::InvalidState)?;
    let revision = ReplicationAuthorityRevision::new(INITIAL_AUTHORITY_REVISION)?;
    let key_epoch = ReplicationKeyEpoch::new(INITIAL_KEY_EPOCH)
        .map_err(|_| ReplicationAuthorityError::InvalidState)?;
    let device = ReplicationAuthorityDevice {
        replica_id: initial_replica.clone(),
        public_key: initial_device_public_key,
        status: ReplicationAuthorityDeviceStatus::Active,
    };
    let state = ReplicationAuthorityState {
        version: REPLICATION_AUTHORITY_STATE_VERSION,
        workspace_id,
        authority_public_key: authority_private.public_key(),
        revision,
        key_epoch,
        devices: BTreeMap::from([(initial_replica, device)]),
    };
    state.validate()?;
    issue_transition(None, state, authority_private, entropy)
}

pub fn enroll_replication_device(
    state: &ReplicationAuthorityState,
    authority_private: &ReplicationAuthorityPrivateKey,
    expected_revision: ReplicationAuthorityRevision,
    replica_id: ReplicationReplicaId,
    public_key: ReplicationDevicePublicKey,
) -> Result<ReplicationAuthorityTransition, ReplicationAuthorityError> {
    enroll_replication_device_with_entropy(
        state,
        authority_private,
        expected_revision,
        replica_id,
        public_key,
        &mut OsReplicationEntropy,
    )
}

pub fn enroll_replication_device_with_entropy(
    state: &ReplicationAuthorityState,
    authority_private: &ReplicationAuthorityPrivateKey,
    expected_revision: ReplicationAuthorityRevision,
    replica_id: ReplicationReplicaId,
    public_key: ReplicationDevicePublicKey,
    entropy: &mut impl ReplicationEntropy,
) -> Result<ReplicationAuthorityTransition, ReplicationAuthorityError> {
    validate_transition_request(state, authority_private, expected_revision)?;
    replica_id
        .validate()
        .map_err(|_| ReplicationAuthorityError::InvalidState)?;
    if state.devices.contains_key(&replica_id) {
        return Err(ReplicationAuthorityError::DeviceAlreadyExists);
    }
    if state.devices.len() == MAX_REPLICATION_REPLICAS {
        return Err(ReplicationAuthorityError::TooManyDevices);
    }
    if state
        .devices
        .values()
        .any(|device| device.public_key == public_key)
    {
        return Err(ReplicationAuthorityError::DuplicateDeviceKey);
    }

    let mut next = advance_state(state)?;
    let device = ReplicationAuthorityDevice {
        replica_id: replica_id.clone(),
        public_key,
        status: ReplicationAuthorityDeviceStatus::Active,
    };
    next.devices.insert(replica_id, device);
    next.validate()?;
    issue_transition(Some(state.revision), next, authority_private, entropy)
}

pub fn rotate_replication_epoch(
    state: &ReplicationAuthorityState,
    authority_private: &ReplicationAuthorityPrivateKey,
    expected_revision: ReplicationAuthorityRevision,
) -> Result<ReplicationAuthorityTransition, ReplicationAuthorityError> {
    rotate_replication_epoch_with_entropy(
        state,
        authority_private,
        expected_revision,
        &mut OsReplicationEntropy,
    )
}

pub fn rotate_replication_epoch_with_entropy(
    state: &ReplicationAuthorityState,
    authority_private: &ReplicationAuthorityPrivateKey,
    expected_revision: ReplicationAuthorityRevision,
    entropy: &mut impl ReplicationEntropy,
) -> Result<ReplicationAuthorityTransition, ReplicationAuthorityError> {
    validate_transition_request(state, authority_private, expected_revision)?;
    let next = advance_state(state)?;
    issue_transition(Some(state.revision), next, authority_private, entropy)
}

pub fn revoke_replication_device(
    state: &ReplicationAuthorityState,
    authority_private: &ReplicationAuthorityPrivateKey,
    expected_revision: ReplicationAuthorityRevision,
    replica_id: &ReplicationReplicaId,
    accepted_through: u64,
) -> Result<ReplicationAuthorityTransition, ReplicationAuthorityError> {
    revoke_replication_device_with_entropy(
        state,
        authority_private,
        expected_revision,
        replica_id,
        accepted_through,
        &mut OsReplicationEntropy,
    )
}

pub fn revoke_replication_device_with_entropy(
    state: &ReplicationAuthorityState,
    authority_private: &ReplicationAuthorityPrivateKey,
    expected_revision: ReplicationAuthorityRevision,
    replica_id: &ReplicationReplicaId,
    accepted_through: u64,
    entropy: &mut impl ReplicationEntropy,
) -> Result<ReplicationAuthorityTransition, ReplicationAuthorityError> {
    validate_transition_request(state, authority_private, expected_revision)?;
    let device = state
        .devices
        .get(replica_id)
        .ok_or(ReplicationAuthorityError::UnknownDevice)?;
    if device.status != ReplicationAuthorityDeviceStatus::Active {
        return Err(ReplicationAuthorityError::DeviceAlreadyRevoked);
    }
    if state.active_device_count() == 1 {
        return Err(ReplicationAuthorityError::LastActiveDevice);
    }

    let mut next = advance_state(state)?;
    let next_device = next
        .devices
        .get_mut(replica_id)
        .ok_or(ReplicationAuthorityError::UnknownDevice)?;
    next_device.status = ReplicationAuthorityDeviceStatus::Revoked {
        accepted_through,
        revoked_at_revision: next.revision,
        revoked_at_epoch: next.key_epoch,
    };
    next.validate()?;
    issue_transition(Some(state.revision), next, authority_private, entropy)
}

fn validate_transition_request(
    state: &ReplicationAuthorityState,
    authority_private: &ReplicationAuthorityPrivateKey,
    expected_revision: ReplicationAuthorityRevision,
) -> Result<(), ReplicationAuthorityError> {
    state.validate()?;
    if state.revision != expected_revision {
        return Err(ReplicationAuthorityError::StaleRevision);
    }
    if state.authority_public_key != authority_private.public_key() {
        return Err(ReplicationAuthorityError::AuthorityMismatch);
    }
    Ok(())
}

fn advance_state(
    state: &ReplicationAuthorityState,
) -> Result<ReplicationAuthorityState, ReplicationAuthorityError> {
    let mut next = state.clone();
    next.revision = state.revision.next()?;
    next.key_epoch = ReplicationKeyEpoch::new(
        state
            .key_epoch
            .get()
            .checked_add(1)
            .ok_or(ReplicationAuthorityError::EpochOverflow)?,
    )
    .map_err(|_| ReplicationAuthorityError::EpochOverflow)?;
    Ok(next)
}

fn issue_transition(
    base_revision: Option<ReplicationAuthorityRevision>,
    state: ReplicationAuthorityState,
    authority_private: &ReplicationAuthorityPrivateKey,
    entropy: &mut impl ReplicationEntropy,
) -> Result<ReplicationAuthorityTransition, ReplicationAuthorityError> {
    state.validate()?;
    if state.authority_public_key != authority_private.public_key() {
        return Err(ReplicationAuthorityError::AuthorityMismatch);
    }

    let epoch_key = generate_epoch_key(state.key_epoch, entropy)?;
    let mut packages = Vec::with_capacity(state.active_device_count());
    for device in state
        .devices
        .values()
        .filter(|device| device.status == ReplicationAuthorityDeviceStatus::Active)
    {
        let package = wrap_replication_epoch_key_with_entropy(
            ReplicationKeyWrapContext {
                workspace_id: &state.workspace_id,
                recipient: &device.replica_id,
            },
            authority_private,
            &device.public_key,
            &epoch_key,
            entropy,
        )
        .map_err(map_wrapping_error)?;
        packages.push(ReplicationRecipientKeyPackage {
            recipient: device.replica_id.clone(),
            package,
        });
    }
    if packages.is_empty() {
        return Err(ReplicationAuthorityError::LastActiveDevice);
    }

    let distribution = ReplicationEpochDistribution {
        authority_revision: state.revision,
        key_epoch: state.key_epoch,
        packages,
    };
    Ok(ReplicationAuthorityTransition {
        base_revision,
        state,
        epoch_key,
        distribution,
    })
}

fn generate_epoch_key(
    epoch: ReplicationKeyEpoch,
    entropy: &mut impl ReplicationEntropy,
) -> Result<ReplicationEpochKey, ReplicationAuthorityError> {
    let mut bytes = Zeroizing::new([0_u8; REPLICATION_EPOCH_KEY_BYTES]);
    entropy
        .fill(bytes.as_mut())
        .map_err(|_| ReplicationAuthorityError::RandomUnavailable)?;
    if bytes.iter().all(|byte| *byte == 0) {
        return Err(ReplicationAuthorityError::RandomUnavailable);
    }
    let result = ReplicationEpochKey::from_bytes(epoch, *bytes)
        .map_err(|_| ReplicationAuthorityError::RandomUnavailable);
    bytes.zeroize();
    result
}

fn map_wrapping_error(error: ReplicationKeyWrappingError) -> ReplicationAuthorityError {
    match error {
        ReplicationKeyWrappingError::RandomUnavailable => {
            ReplicationAuthorityError::RandomUnavailable
        }
        _ => ReplicationAuthorityError::WrappingFailed,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplicationAuthorityError {
    InvalidRevision,
    UnsupportedStateVersion,
    InvalidState,
    StateTooLarge,
    NonCanonicalState,
    StaleRevision,
    RevisionOverflow,
    EpochOverflow,
    AuthorityMismatch,
    DeviceAlreadyExists,
    DuplicateDeviceKey,
    TooManyDevices,
    UnknownDevice,
    DeviceAlreadyRevoked,
    LastActiveDevice,
    RandomUnavailable,
    WrappingFailed,
}

impl fmt::Display for ReplicationAuthorityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidRevision => "replication authority revision is invalid",
            Self::UnsupportedStateVersion => "replication authority state version is unsupported",
            Self::InvalidState => "replication authority state is invalid",
            Self::StateTooLarge => "replication authority state exceeds its byte limit",
            Self::NonCanonicalState => "replication authority state is not canonical",
            Self::StaleRevision => "replication authority revision is stale",
            Self::RevisionOverflow => "replication authority revision overflowed",
            Self::EpochOverflow => "replication key epoch overflowed",
            Self::AuthorityMismatch => "replication authority key does not match",
            Self::DeviceAlreadyExists => "replication device already exists",
            Self::DuplicateDeviceKey => "replication device key is already enrolled",
            Self::TooManyDevices => "replication authority has too many devices",
            Self::UnknownDevice => "replication device is unknown",
            Self::DeviceAlreadyRevoked => "replication device is already revoked",
            Self::LastActiveDevice => "replication authority requires an active device",
            Self::RandomUnavailable => "secure randomness is unavailable",
            Self::WrappingFailed => "replication epoch distribution failed",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for ReplicationAuthorityError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ReplicationDevicePrivateKey;

    fn authority_state() -> ReplicationAuthorityState {
        let workspace = ReplicationWorkspaceId::new("workspace-persistence").expect("workspace");
        let replica = ReplicationReplicaId::new("desktop-primary").expect("replica");
        let authority = ReplicationAuthorityPrivateKey::from_bytes([1; 32]).expect("authority key");
        let device = ReplicationDevicePrivateKey::from_bytes([2; 32]).expect("device key");
        bootstrap_replication_authority(workspace, &authority, replica, device.public_key())
            .expect("bootstrap")
            .into_parts()
            .0
    }

    #[test]
    fn authority_state_canonical_round_trip_rejects_ambiguous_or_hostile_documents() {
        let state = authority_state();
        let canonical = state.to_canonical_bytes().expect("canonical state");
        let restored = ReplicationAuthorityState::from_canonical_bytes(&canonical)
            .expect("restore canonical state");
        assert_eq!(restored.to_canonical_bytes().unwrap(), canonical);
        assert_eq!(restored.replication_policy(), state.replication_policy());

        let mut noncanonical = canonical.clone();
        noncanonical.push(b'\n');
        assert_eq!(
            ReplicationAuthorityState::from_canonical_bytes(&noncanonical),
            Err(ReplicationAuthorityError::NonCanonicalState)
        );

        let mut unknown: serde_json::Value = serde_json::from_slice(&canonical).unwrap();
        unknown["unexpected"] = serde_json::json!(true);
        assert_eq!(
            ReplicationAuthorityState::from_canonical_bytes(&serde_json::to_vec(&unknown).unwrap()),
            Err(ReplicationAuthorityError::InvalidState)
        );

        let mut duplicate: serde_json::Value = serde_json::from_slice(&canonical).unwrap();
        let first = duplicate["devices"][0].clone();
        duplicate["devices"].as_array_mut().unwrap().push(first);
        assert_eq!(
            ReplicationAuthorityState::from_canonical_bytes(
                &serde_json::to_vec(&duplicate).unwrap()
            ),
            Err(ReplicationAuthorityError::InvalidState)
        );

        assert_eq!(
            ReplicationAuthorityState::from_canonical_bytes(&vec![
                b'x';
                MAX_REPLICATION_AUTHORITY_STATE_BYTES
                    + 1
            ]),
            Err(ReplicationAuthorityError::StateTooLarge)
        );
    }

    #[test]
    fn authority_revision_and_epoch_overflow_fail_closed() {
        let revision = ReplicationAuthorityRevision::new(u64::MAX).expect("maximum revision");
        assert_eq!(
            revision.next(),
            Err(ReplicationAuthorityError::RevisionOverflow)
        );

        let workspace_id = ReplicationWorkspaceId::new("workspace-overflow").expect("workspace");
        let replica_id = ReplicationReplicaId::new("device-a").expect("replica");
        let private = ReplicationAuthorityPrivateKey::from_bytes([1; 32]).expect("authority key");
        let device_key = ReplicationDevicePublicKey::from_bytes([2; 32]).expect("device key");
        let device = ReplicationAuthorityDevice {
            replica_id: replica_id.clone(),
            public_key: device_key,
            status: ReplicationAuthorityDeviceStatus::Active,
        };
        let state = ReplicationAuthorityState {
            version: REPLICATION_AUTHORITY_STATE_VERSION,
            workspace_id,
            authority_public_key: private.public_key(),
            revision: ReplicationAuthorityRevision::new(1).expect("revision"),
            key_epoch: ReplicationKeyEpoch::new(u64::MAX).expect("maximum epoch"),
            devices: BTreeMap::from([(replica_id, device)]),
        };
        assert_eq!(
            advance_state(&state),
            Err(ReplicationAuthorityError::EpochOverflow)
        );
    }
}
