use std::collections::BTreeSet;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt as _;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use termirust_domain::{
    ReplicatedVersion, ReplicationDocument, ReplicationEntry, ReplicationOperation,
    ReplicationRecordKey, ReplicationReplicaId, ReplicationWorkspaceId, SealedReplicationPayload,
};
use termirust_replication_security::{
    MAX_REPLICATION_AUTHORITY_STATE_BYTES, MAX_REPLICATION_PLAINTEXT_BYTES,
    MAX_REPLICATION_RETAINED_EPOCH_KEYS, OpenedReplicationOperation,
    ReplicationAuthorityDeviceStatus, ReplicationAuthorityError, ReplicationAuthorityState,
    ReplicationAuthorityTransition, ReplicationCryptoError, ReplicationDevicePublicKey,
    ReplicationHistoricalKeyIndex, ReplicationHistoricalKeyLimit, ReplicationKeyWrapContext,
    ReplicationKeyWrappingError, ReplicationOperationKind, ReplicationSealContext,
    ReplicationSecretBackend, ReplicationSecretCustodyError, ReplicationSecretRef,
    ReplicationSecretVault, WrappedReplicationEpochKey, bootstrap_replication_authority,
    enroll_replication_device, generate_replication_authority_private_key,
    generate_replication_device_private_key, open as open_envelope,
    open_wrapped_replication_epoch_key, revoke_replication_device, rotate_replication_epoch,
    seal_delete, seal_put,
};
use uuid::Uuid;

use crate::{AtomicWriter as _, SystemAtomicWriter};

use super::{
    ReplicationConflictResolution, ReplicationCustodyMetadata, ReplicationRecoveryOutcome,
    ReplicationRepository, ReplicationRepositoryRevision, ReplicationStoreError,
    ReplicationSyncCoordinator, ReplicationSyncOutcome, ReplicationSyncPlan,
    SharedFolderReplicationTransport, SharedFolderSlot, io_error, read_bounded_regular_file,
    reject_unsafe_file_if_present,
};

const PRODUCT_FORMAT_VERSION: u16 = 1;
const PRODUCT_PROFILE_FILE: &str = "profile.json";
const PRODUCT_REPOSITORY_DIR: &str = "repository";
const PRODUCT_TRANSACTION_FILE: &str = "authority.transaction.json";
const PRODUCT_UPDATE_OUTBOX_FILE: &str = "authority-update.json";
const PENDING_ENROLLMENT_FILE: &str = "pending-enrollment.json";
const ENROLLMENT_TRANSACTION_FILE: &str = "enrollment.transaction.json";
const DELETION_TRANSACTION_FILE: &str = "deletion.transaction.json";
const PRODUCT_LOCK_FILE: &str = "product.lock";
const MAX_PRODUCT_PROFILE_BYTES: u64 = 192 * 1024;
const MAX_PRODUCT_TRANSACTION_BYTES: u64 = 256 * 1024;
const MAX_ENROLLMENT_REQUEST_BYTES: usize = 4 * 1024;
const MAX_ENROLLMENT_BUNDLE_BYTES: usize = 192 * 1024;
const ENROLLMENT_CODE_DOMAIN: &[u8] = b"termirust-replication-enrollment-code-v1\0";
const DELETION_PLAN_DOMAIN: &[u8] = b"termirust-replication-deletion-plan-v1\0";
const DELETION_CONFIRMATION: &str = "DELETE REPLICATION";

#[derive(Clone, Eq, PartialEq)]
pub struct ReplicationEnrollmentRequest {
    replica_id: ReplicationReplicaId,
    public_key: ReplicationDevicePublicKey,
}

impl ReplicationEnrollmentRequest {
    pub fn replica_id(&self) -> &ReplicationReplicaId {
        &self.replica_id
    }

    pub fn public_key(&self) -> &ReplicationDevicePublicKey {
        &self.public_key
    }

    pub fn to_canonical_bytes(&self) -> Result<Vec<u8>, ReplicationProductError> {
        canonical_json(
            &StoredEnrollmentRequest::from_request(self),
            MAX_ENROLLMENT_REQUEST_BYTES,
        )
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, ReplicationProductError> {
        let stored: StoredEnrollmentRequest =
            decode_canonical_json(bytes, MAX_ENROLLMENT_REQUEST_BYTES)?;
        stored.into_request()
    }
}

impl fmt::Debug for ReplicationEnrollmentRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplicationEnrollmentRequest")
            .field("replica_id", &"<redacted>")
            .field("public_key", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct ReplicationEnrollmentBundle {
    workspace_id: ReplicationWorkspaceId,
    recipient: ReplicationReplicaId,
    transport_slot: SharedFolderSlot,
    authority_state: Vec<u8>,
    package: Vec<u8>,
}

impl ReplicationEnrollmentBundle {
    pub fn workspace_id(&self) -> &ReplicationWorkspaceId {
        &self.workspace_id
    }

    pub fn recipient(&self) -> &ReplicationReplicaId {
        &self.recipient
    }

    pub fn verification_code(
        &self,
        request: &ReplicationEnrollmentRequest,
    ) -> Result<String, ReplicationProductError> {
        if request.replica_id() != &self.recipient {
            return Err(ReplicationProductError::EnrollmentMismatch);
        }
        let mut digest = Sha256::new();
        digest.update(ENROLLMENT_CODE_DOMAIN);
        digest.update(request.to_canonical_bytes()?);
        digest.update(&self.authority_state);
        digest.update(&self.package);
        digest.update(self.transport_slot.file_component().as_bytes());
        let digest = digest.finalize();
        Ok(format!(
            "{:02X}{:02X}{:02X}-{:02X}{:02X}{:02X}",
            digest[0], digest[1], digest[2], digest[3], digest[4], digest[5]
        ))
    }

    pub fn to_canonical_bytes(&self) -> Result<Vec<u8>, ReplicationProductError> {
        canonical_json(
            &StoredEnrollmentBundle::from_bundle(self),
            MAX_ENROLLMENT_BUNDLE_BYTES,
        )
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, ReplicationProductError> {
        let stored: StoredEnrollmentBundle =
            decode_canonical_json(bytes, MAX_ENROLLMENT_BUNDLE_BYTES)?;
        stored.into_bundle()
    }
}

impl fmt::Debug for ReplicationEnrollmentBundle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplicationEnrollmentBundle")
            .field("workspace_id", &"<redacted>")
            .field("recipient", &"<redacted>")
            .field("transport_slot", &"<redacted>")
            .field("authority_state", &"<redacted>")
            .field("package", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct ReplicationDeviceKeyPackage {
    recipient: ReplicationReplicaId,
    package: Vec<u8>,
}

impl ReplicationDeviceKeyPackage {
    pub fn recipient(&self) -> &ReplicationReplicaId {
        &self.recipient
    }

    pub fn package(&self) -> &[u8] {
        &self.package
    }
}

impl fmt::Debug for ReplicationDeviceKeyPackage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplicationDeviceKeyPackage")
            .field("recipient", &"<redacted>")
            .field("package", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct ReplicationAuthorityUpdate {
    pub authority_revision: u64,
    pub key_epoch: u64,
    authority_state: Vec<u8>,
    packages: Vec<ReplicationDeviceKeyPackage>,
}

impl ReplicationAuthorityUpdate {
    pub fn authority_state(&self) -> &[u8] {
        &self.authority_state
    }

    pub fn packages(&self) -> &[ReplicationDeviceKeyPackage] {
        &self.packages
    }

    pub fn package_for(
        &self,
        replica_id: &ReplicationReplicaId,
    ) -> Option<&ReplicationDeviceKeyPackage> {
        self.packages
            .iter()
            .find(|package| package.recipient() == replica_id)
    }

    pub fn to_canonical_bytes(&self) -> Result<Vec<u8>, ReplicationProductError> {
        canonical_json(
            &StoredAuthorityUpdate {
                format_version: PRODUCT_FORMAT_VERSION,
                authority_state_hex: encode_hex(self.authority_state()),
                packages: stored_packages(self),
            },
            MAX_PRODUCT_TRANSACTION_BYTES as usize,
        )
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, ReplicationProductError> {
        let stored: StoredAuthorityUpdate =
            decode_canonical_json(bytes, MAX_PRODUCT_TRANSACTION_BYTES as usize)?;
        if stored.format_version != PRODUCT_FORMAT_VERSION {
            return Err(ReplicationProductError::InvalidProfile);
        }
        let authority_bytes = decode_hex(&stored.authority_state_hex)?;
        let authority = ReplicationAuthorityState::from_canonical_bytes(&authority_bytes)?;
        decode_authority_update(&authority, &stored.authority_state_hex, &stored.packages)
    }
}

impl fmt::Debug for ReplicationAuthorityUpdate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplicationAuthorityUpdate")
            .field("authority_revision", &self.authority_revision)
            .field("key_epoch", &self.key_epoch)
            .field("authority_state", &"<redacted>")
            .field("package_count", &self.packages.len())
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplicationProductStatus {
    pub repository_revision: ReplicationRepositoryRevision,
    pub authority_revision: u64,
    pub key_epoch: u64,
    pub active_devices: usize,
    pub total_devices: usize,
    pub record_count: usize,
    pub recovery_required: bool,
    pub secret_retirement_pending: bool,
    pub authority_owner: bool,
}

pub struct ReplicationProductRecord {
    key: ReplicationRecordKey,
    value: Option<Vec<u8>>,
}

impl ReplicationProductRecord {
    pub fn key(&self) -> &ReplicationRecordKey {
        &self.key
    }

    pub fn value(&self) -> Option<&[u8]> {
        self.value.as_deref()
    }
}

impl fmt::Debug for ReplicationProductRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplicationProductRecord")
            .field("key", &"<redacted>")
            .field(
                "operation",
                &if self.value.is_some() {
                    "put"
                } else {
                    "delete"
                },
            )
            .field("value", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct ReplicationDeletionPlan {
    token: [u8; 32],
    pub record_count: usize,
    pub secret_count: usize,
    pub authority_owner: bool,
}

impl ReplicationDeletionPlan {
    pub fn confirmation_phrase() -> &'static str {
        DELETION_CONFIRMATION
    }
}

impl fmt::Debug for ReplicationDeletionPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplicationDeletionPlan")
            .field("token", &"<redacted>")
            .field("record_count", &self.record_count)
            .field("secret_count", &self.secret_count)
            .field("authority_owner", &self.authority_owner)
            .finish()
    }
}

pub struct ReplicationConflictCandidate {
    author: ReplicationReplicaId,
    value: Option<Vec<u8>>,
}

impl ReplicationConflictCandidate {
    pub fn author(&self) -> &ReplicationReplicaId {
        &self.author
    }

    pub fn value(&self) -> Option<&[u8]> {
        self.value.as_deref()
    }
}

impl fmt::Debug for ReplicationConflictCandidate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplicationConflictCandidate")
            .field("author", &"<redacted>")
            .field(
                "operation",
                &if self.value.is_some() {
                    "put"
                } else {
                    "delete"
                },
            )
            .field("value", &"<redacted>")
            .finish()
    }
}

pub struct ReplicationConflictChoice {
    key: ReplicationRecordKey,
    value: Option<Vec<u8>>,
}

impl ReplicationConflictChoice {
    pub fn put(key: ReplicationRecordKey, value: Vec<u8>) -> Result<Self, ReplicationProductError> {
        if value.is_empty() {
            return Err(ReplicationProductError::EmptyPut);
        }
        if value.len() > MAX_REPLICATION_PLAINTEXT_BYTES {
            return Err(ReplicationProductError::Crypto(
                ReplicationCryptoError::PlaintextTooLarge,
            ));
        }
        Ok(Self {
            key,
            value: Some(value),
        })
    }

    pub fn delete(key: ReplicationRecordKey) -> Self {
        Self { key, value: None }
    }

    pub fn key(&self) -> &ReplicationRecordKey {
        &self.key
    }
}

impl fmt::Debug for ReplicationConflictChoice {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplicationConflictChoice")
            .field("key", &"<redacted>")
            .field(
                "operation",
                &if self.value.is_some() {
                    "put"
                } else {
                    "delete"
                },
            )
            .field("value", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplicationProductError {
    Store(ReplicationStoreError),
    Authority(ReplicationAuthorityError),
    Crypto(ReplicationCryptoError),
    Wrapping(ReplicationKeyWrappingError),
    Custody(ReplicationSecretCustodyError),
    AlreadyConfigured,
    NotConfigured,
    InvalidProfile,
    NewerProfile { found: u16, supported: u16 },
    InvalidPath,
    RecordConflict,
    EmptyPut,
    LocalDeviceRevocation,
    PendingAuthorityTransition,
    PendingAuthorityUpdate,
    StaleAuthorityUpdate,
    AuthorityOwnerRequired,
    EnrollmentMismatch,
    VerificationCodeMismatch,
    DeletionConfirmationRequired,
    StaleDeletionPlan,
}

impl fmt::Display for ReplicationProductError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => error.fmt(formatter),
            Self::Authority(error) => error.fmt(formatter),
            Self::Crypto(error) => error.fmt(formatter),
            Self::Wrapping(error) => error.fmt(formatter),
            Self::Custody(error) => error.fmt(formatter),
            Self::AlreadyConfigured => formatter.write_str("replication is already configured"),
            Self::NotConfigured => formatter.write_str("replication is not configured"),
            Self::InvalidProfile => formatter.write_str("replication profile is invalid"),
            Self::NewerProfile { found, supported } => write!(
                formatter,
                "replication profile format {found} is newer than supported format {supported}"
            ),
            Self::InvalidPath => formatter.write_str("replication path is invalid"),
            Self::RecordConflict => {
                formatter.write_str("replication record requires conflict review")
            }
            Self::EmptyPut => formatter.write_str("replication record payload cannot be empty"),
            Self::LocalDeviceRevocation => {
                formatter.write_str("this device cannot revoke its own replication access")
            }
            Self::PendingAuthorityTransition => {
                formatter.write_str("replication authority transition requires recovery")
            }
            Self::PendingAuthorityUpdate => {
                formatter.write_str("replication authority update must be delivered first")
            }
            Self::StaleAuthorityUpdate => {
                formatter.write_str("replication authority update is stale")
            }
            Self::AuthorityOwnerRequired => {
                formatter.write_str("this replication action requires the owner device")
            }
            Self::EnrollmentMismatch => {
                formatter.write_str("replication enrollment does not match this device")
            }
            Self::VerificationCodeMismatch => {
                formatter.write_str("replication enrollment verification code does not match")
            }
            Self::DeletionConfirmationRequired => {
                formatter.write_str("replication deletion requires exact confirmation")
            }
            Self::StaleDeletionPlan => formatter.write_str("replication deletion plan is stale"),
        }
    }
}

impl std::error::Error for ReplicationProductError {}

impl From<ReplicationStoreError> for ReplicationProductError {
    fn from(error: ReplicationStoreError) -> Self {
        Self::Store(error)
    }
}

impl From<ReplicationAuthorityError> for ReplicationProductError {
    fn from(error: ReplicationAuthorityError) -> Self {
        Self::Authority(error)
    }
}

impl From<ReplicationCryptoError> for ReplicationProductError {
    fn from(error: ReplicationCryptoError) -> Self {
        Self::Crypto(error)
    }
}

impl From<ReplicationKeyWrappingError> for ReplicationProductError {
    fn from(error: ReplicationKeyWrappingError) -> Self {
        Self::Wrapping(error)
    }
}

impl From<ReplicationSecretCustodyError> for ReplicationProductError {
    fn from(error: ReplicationSecretCustodyError) -> Self {
        Self::Custody(error)
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredProductProfile {
    format_version: u16,
    workspace_id: String,
    local_replica_id: String,
    shared_folder: String,
    transport_slot: String,
    authority_state_hex: String,
}

#[derive(Deserialize)]
struct ProductFormatProbe {
    format_version: u16,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredEnrollmentRequest {
    format_version: u16,
    replica_id: String,
    public_key_hex: String,
}

impl StoredEnrollmentRequest {
    fn from_request(request: &ReplicationEnrollmentRequest) -> Self {
        Self {
            format_version: PRODUCT_FORMAT_VERSION,
            replica_id: request.replica_id.as_str().to_string(),
            public_key_hex: encode_hex(request.public_key.as_bytes()),
        }
    }

    fn into_request(self) -> Result<ReplicationEnrollmentRequest, ReplicationProductError> {
        if self.format_version != PRODUCT_FORMAT_VERSION {
            return Err(ReplicationProductError::InvalidProfile);
        }
        let replica_id = ReplicationReplicaId::new(self.replica_id)
            .map_err(|_| ReplicationProductError::InvalidProfile)?;
        let public_key =
            ReplicationDevicePublicKey::from_bytes(decode_hex_array(&self.public_key_hex)?)?;
        Ok(ReplicationEnrollmentRequest {
            replica_id,
            public_key,
        })
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredEnrollmentBundle {
    format_version: u16,
    workspace_id: String,
    recipient: String,
    transport_slot: String,
    authority_state_hex: String,
    package_hex: String,
}

impl StoredEnrollmentBundle {
    fn from_bundle(bundle: &ReplicationEnrollmentBundle) -> Self {
        Self {
            format_version: PRODUCT_FORMAT_VERSION,
            workspace_id: bundle.workspace_id.as_str().to_string(),
            recipient: bundle.recipient.as_str().to_string(),
            transport_slot: bundle.transport_slot.file_component().to_string(),
            authority_state_hex: encode_hex(&bundle.authority_state),
            package_hex: encode_hex(&bundle.package),
        }
    }

    fn into_bundle(self) -> Result<ReplicationEnrollmentBundle, ReplicationProductError> {
        if self.format_version != PRODUCT_FORMAT_VERSION {
            return Err(ReplicationProductError::InvalidProfile);
        }
        let workspace_id = ReplicationWorkspaceId::new(self.workspace_id)
            .map_err(|_| ReplicationProductError::InvalidProfile)?;
        let recipient = ReplicationReplicaId::new(self.recipient)
            .map_err(|_| ReplicationProductError::InvalidProfile)?;
        let transport_slot = SharedFolderSlot::new(self.transport_slot)
            .map_err(|_| ReplicationProductError::InvalidProfile)?;
        let authority_state = decode_hex(&self.authority_state_hex)?;
        let authority = ReplicationAuthorityState::from_canonical_bytes(&authority_state)?;
        if authority.workspace_id() != &workspace_id
            || authority
                .device(&recipient)
                .is_none_or(|device| device.status() != ReplicationAuthorityDeviceStatus::Active)
        {
            return Err(ReplicationProductError::EnrollmentMismatch);
        }
        let package = decode_hex(&self.package_hex)?;
        if WrappedReplicationEpochKey::from_bytes(&package)?.key_epoch() != authority.key_epoch() {
            return Err(ReplicationProductError::EnrollmentMismatch);
        }
        Ok(ReplicationEnrollmentBundle {
            workspace_id,
            recipient,
            transport_slot,
            authority_state,
            package,
        })
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredPendingEnrollment {
    format_version: u16,
    shared_folder: String,
    request: StoredEnrollmentRequest,
    device_reference_hex: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredEnrollmentTransaction {
    format_version: u16,
    epoch_reference_hex: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredDeletionTransaction {
    format_version: u16,
    references_hex: Vec<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredAuthorityTransaction {
    format_version: u16,
    base_authority_revision: u64,
    base_repository_revision: u64,
    next_authority_state_hex: String,
    new_epoch_reference_hex: String,
    packages: Vec<StoredDeviceKeyPackage>,
    publish_update: bool,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredDeviceKeyPackage {
    recipient: String,
    package_hex: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredAuthorityUpdate {
    format_version: u16,
    authority_state_hex: String,
    packages: Vec<StoredDeviceKeyPackage>,
}

pub struct ReplicationProductService<B> {
    root: PathBuf,
    shared_folder: PathBuf,
    workspace_id: ReplicationWorkspaceId,
    local_replica_id: ReplicationReplicaId,
    authority: ReplicationAuthorityState,
    transport_slot: SharedFolderSlot,
    repository: ReplicationRepository,
    transport: SharedFolderReplicationTransport,
    vault: ReplicationSecretVault<B>,
}

impl<B> fmt::Debug for ReplicationProductService<B> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplicationProductService")
            .field("root", &"<redacted>")
            .field("shared_folder", &"<redacted>")
            .field("workspace_id", &"<redacted>")
            .field("local_replica_id", &"<redacted>")
            .field("authority", &self.authority)
            .finish_non_exhaustive()
    }
}

impl<B: ReplicationSecretBackend> ReplicationProductService<B> {
    pub fn prepare_enrollment(
        root: impl Into<PathBuf>,
        shared_folder: impl Into<PathBuf>,
        backend: B,
    ) -> Result<ReplicationEnrollmentRequest, ReplicationProductError> {
        let root = root.into();
        let shared_folder = shared_folder.into();
        validate_existing_directory(&shared_folder)?;
        match fs::symlink_metadata(&root) {
            Ok(_) => return Err(ReplicationProductError::AlreadyConfigured),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(profile_io("inspect enrollment", error)),
        }
        let parent = root.parent().ok_or(ReplicationProductError::InvalidPath)?;
        validate_existing_directory(parent)?;
        let name = root
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or(ReplicationProductError::InvalidPath)?;
        let shared_folder = shared_folder
            .to_str()
            .ok_or(ReplicationProductError::InvalidPath)?
            .to_string();
        let staging = parent.join(format!(".{name}.enrollment-{}", Uuid::new_v4()));
        create_private_directory(&staging)?;
        let vault = ReplicationSecretVault::new(backend);
        let mut device_reference = None;
        let result = (|| {
            let device_private = generate_replication_device_private_key()?;
            let request = ReplicationEnrollmentRequest {
                replica_id: ReplicationReplicaId::new(format!("device-{}", Uuid::new_v4()))
                    .map_err(ReplicationStoreError::from)?,
                public_key: device_private.public_key(),
            };
            let reference = vault.store_device_key(&device_private)?;
            device_reference = Some(reference.clone());
            let pending = StoredPendingEnrollment {
                format_version: PRODUCT_FORMAT_VERSION,
                shared_folder,
                request: StoredEnrollmentRequest::from_request(&request),
                device_reference_hex: encode_hex(&reference.to_bytes()),
            };
            write_canonical_file(
                &staging.join(PENDING_ENROLLMENT_FILE),
                &pending,
                MAX_PRODUCT_PROFILE_BYTES as usize,
                "write pending enrollment",
            )?;
            fs::rename(&staging, &root)
                .map_err(|error| profile_io("publish pending enrollment", error))?;
            sync_directory(parent)?;
            Ok(request)
        })();
        if result.is_err() && staging.exists() {
            let _ = fs::remove_dir_all(&staging);
            if let Some(reference) = &device_reference {
                let _ = vault.delete(reference);
            }
        }
        result
    }

    pub fn pending_enrollment_request(
        root: impl AsRef<Path>,
    ) -> Result<ReplicationEnrollmentRequest, ReplicationProductError> {
        read_pending_enrollment(root.as_ref())?
            .request
            .into_request()
    }

    pub fn bootstrap(
        root: impl Into<PathBuf>,
        shared_folder: impl Into<PathBuf>,
        backend: B,
    ) -> Result<Self, ReplicationProductError> {
        let root = root.into();
        let shared_folder = shared_folder.into();
        validate_existing_directory(&shared_folder)?;
        match fs::symlink_metadata(&root) {
            Ok(_) => return Err(ReplicationProductError::AlreadyConfigured),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(profile_io("inspect profile", error)),
        }
        let parent = root.parent().ok_or(ReplicationProductError::InvalidPath)?;
        validate_existing_directory(parent)?;
        let file_name = root
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or(ReplicationProductError::InvalidPath)?;
        let shared_folder_text = shared_folder
            .to_str()
            .ok_or(ReplicationProductError::InvalidPath)?
            .to_string();

        let workspace_id = ReplicationWorkspaceId::new(format!("workspace-{}", Uuid::new_v4()))
            .map_err(ReplicationStoreError::from)?;
        let local_replica_id = ReplicationReplicaId::new(format!("device-{}", Uuid::new_v4()))
            .map_err(ReplicationStoreError::from)?;
        let slot_text = format!(
            "{:032x}{:032x}",
            Uuid::new_v4().as_u128(),
            Uuid::new_v4().as_u128()
        );
        let slot = SharedFolderSlot::new(slot_text.clone())?;
        let staging = parent.join(format!(".{file_name}.bootstrap-{}", Uuid::new_v4()));
        create_private_directory(&staging)?;

        let vault = ReplicationSecretVault::new(backend);
        let mut created_references = Vec::new();
        let mut published = false;
        let setup = (|| {
            let authority_key = generate_replication_authority_private_key()?;
            let device_key = generate_replication_device_private_key()?;
            let transition = bootstrap_replication_authority(
                workspace_id.clone(),
                &authority_key,
                local_replica_id.clone(),
                device_key.public_key(),
            )?;
            let (authority, epoch_key, _distribution) = transition.into_parts();

            let authority_reference = vault.store_authority_key(&authority_key)?;
            created_references.push(authority_reference.clone());
            let device_reference = vault.store_device_key(&device_key)?;
            created_references.push(device_reference.clone());
            let epoch_reference = vault.store_epoch_key(&epoch_key)?;
            created_references.push(epoch_reference.clone());

            let historical = ReplicationHistoricalKeyIndex::from_retained(
                ReplicationHistoricalKeyLimit::new(MAX_REPLICATION_RETAINED_EPOCH_KEYS)?,
                [epoch_reference],
            )?;
            let custody =
                ReplicationCustodyMetadata::new(authority_reference, device_reference, historical)?;
            let policy = authority.replication_policy()?;
            let document = ReplicationDocument::new(workspace_id.clone(), Vec::new(), &policy)
                .map_err(ReplicationStoreError::from)?;
            let repository = ReplicationRepository::open(staging.join(PRODUCT_REPOSITORY_DIR))?;
            repository.create(document, custody, &policy)?;

            let profile = StoredProductProfile {
                format_version: PRODUCT_FORMAT_VERSION,
                workspace_id: workspace_id.as_str().to_string(),
                local_replica_id: local_replica_id.as_str().to_string(),
                shared_folder: shared_folder_text,
                transport_slot: slot_text,
                authority_state_hex: encode_hex(&authority.to_canonical_bytes()?),
            };
            write_profile(&staging.join(PRODUCT_PROFILE_FILE), &profile)?;
            fs::rename(&staging, &root).map_err(|error| profile_io("publish profile", error))?;
            published = true;
            sync_directory(parent)?;
            Ok(authority)
        })();

        let authority = match setup {
            Ok(authority) => authority,
            Err(error) => {
                if !published {
                    let _ = fs::remove_dir_all(&staging);
                    for reference in created_references.iter().rev() {
                        let _ = vault.delete(reference);
                    }
                }
                return Err(error);
            }
        };

        let repository = ReplicationRepository::open(root.join(PRODUCT_REPOSITORY_DIR))?;
        let transport = SharedFolderReplicationTransport::open(
            &shared_folder,
            workspace_id.clone(),
            slot.clone(),
        )?;
        Ok(Self {
            root,
            shared_folder,
            workspace_id,
            local_replica_id,
            authority,
            transport_slot: slot,
            repository,
            transport,
            vault,
        })
    }

    pub fn open(root: impl Into<PathBuf>, backend: B) -> Result<Self, ReplicationProductError> {
        let root = root.into();
        validate_existing_directory(&root).map_err(|error| match error {
            ReplicationProductError::Store(ReplicationStoreError::Io {
                kind: io::ErrorKind::NotFound,
                ..
            }) => ReplicationProductError::NotConfigured,
            other => other,
        })?;
        let vault = ReplicationSecretVault::new(backend);
        if continue_pending_deletion(&root, &vault)? {
            return Err(ReplicationProductError::NotConfigured);
        }
        recover_enrollment_activation(&root, &vault)?;
        let profile = read_profile(&root.join(PRODUCT_PROFILE_FILE))?;
        let workspace_id = ReplicationWorkspaceId::new(profile.workspace_id.clone())
            .map_err(|_| ReplicationProductError::InvalidProfile)?;
        let local_replica_id = ReplicationReplicaId::new(profile.local_replica_id.clone())
            .map_err(|_| ReplicationProductError::InvalidProfile)?;
        let shared_folder = PathBuf::from(&profile.shared_folder);
        validate_existing_directory(&shared_folder)?;
        let slot = SharedFolderSlot::new(profile.transport_slot.clone())
            .map_err(|_| ReplicationProductError::InvalidProfile)?;
        let authority_bytes = decode_hex(&profile.authority_state_hex)?;
        if authority_bytes.len() > MAX_REPLICATION_AUTHORITY_STATE_BYTES {
            return Err(ReplicationProductError::InvalidProfile);
        }
        let mut authority = ReplicationAuthorityState::from_canonical_bytes(&authority_bytes)?;
        if authority.workspace_id() != &workspace_id
            || authority.device(&local_replica_id).is_none()
        {
            return Err(ReplicationProductError::InvalidProfile);
        }
        let repository = ReplicationRepository::open(root.join(PRODUCT_REPOSITORY_DIR))?;
        authority = recover_authority_transaction(
            &root,
            &profile,
            authority,
            &workspace_id,
            &repository,
            &vault,
        )?;
        let policy = authority.replication_policy()?;
        let snapshot = repository.load(&workspace_id, &policy)?;
        if snapshot.custody.historical().current_epoch() != authority.key_epoch() {
            return Err(ReplicationProductError::InvalidProfile);
        }
        if let Some(reference) = snapshot.custody.authority_reference() {
            vault.load_authority_key(reference)?;
        }
        vault.load_device_key(snapshot.custody.device_reference())?;
        vault.load_epoch_key(
            snapshot
                .custody
                .historical()
                .reference_for(authority.key_epoch())?,
            authority.key_epoch(),
        )?;
        let transport = SharedFolderReplicationTransport::open(
            &shared_folder,
            workspace_id.clone(),
            slot.clone(),
        )?;
        Ok(Self {
            root,
            shared_folder,
            workspace_id,
            local_replica_id,
            authority,
            transport_slot: slot,
            repository,
            transport,
            vault,
        })
    }

    pub fn workspace_id(&self) -> &ReplicationWorkspaceId {
        &self.workspace_id
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn local_replica_id(&self) -> &ReplicationReplicaId {
        &self.local_replica_id
    }

    pub fn shared_folder(&self) -> &Path {
        &self.shared_folder
    }

    pub fn authority(&self) -> &ReplicationAuthorityState {
        &self.authority
    }

    pub fn pending_authority_update(
        &self,
    ) -> Result<Option<ReplicationAuthorityUpdate>, ReplicationProductError> {
        let update = read_authority_update(&self.root)?;
        if let Some(update) = &update
            && update.authority_state() != self.authority.to_canonical_bytes()?.as_slice()
        {
            return Err(ReplicationProductError::InvalidProfile);
        }
        Ok(update)
    }

    pub fn acknowledge_authority_update(
        &self,
        authority_revision: u64,
    ) -> Result<bool, ReplicationProductError> {
        let _lock = ProductAdvisoryLock::acquire(&self.root.join(PRODUCT_LOCK_FILE))?;
        let Some(update) = self.pending_authority_update()? else {
            return Ok(false);
        };
        if update.authority_revision != authority_revision {
            return Err(ReplicationProductError::StaleAuthorityUpdate);
        }
        fs::remove_file(self.root.join(PRODUCT_UPDATE_OUTBOX_FILE))
            .map_err(|error| profile_io("remove authority update", error))?;
        sync_directory(&self.root)?;
        Ok(true)
    }

    pub fn rotate_keys(&mut self) -> Result<ReplicationAuthorityUpdate, ReplicationProductError> {
        let policy = self.authority.replication_policy()?;
        let snapshot = self.repository.load(&self.workspace_id, &policy)?;
        let authority_private = self.vault.load_authority_key(
            snapshot
                .custody
                .authority_reference()
                .ok_or(ReplicationProductError::AuthorityOwnerRequired)?,
        )?;
        let transition = rotate_replication_epoch(
            &self.authority,
            &authority_private,
            self.authority.revision(),
        )?;
        self.commit_authority_transition(snapshot, transition, false)
    }

    pub fn enroll_device(
        &mut self,
        replica_id: ReplicationReplicaId,
        public_key: ReplicationDevicePublicKey,
    ) -> Result<ReplicationAuthorityUpdate, ReplicationProductError> {
        let policy = self.authority.replication_policy()?;
        let snapshot = self.repository.load(&self.workspace_id, &policy)?;
        let authority_private = self.vault.load_authority_key(
            snapshot
                .custody
                .authority_reference()
                .ok_or(ReplicationProductError::AuthorityOwnerRequired)?,
        )?;
        let transition = enroll_replication_device(
            &self.authority,
            &authority_private,
            self.authority.revision(),
            replica_id,
            public_key,
        )?;
        self.commit_authority_transition(snapshot, transition, true)
    }

    pub fn enroll_request(
        &mut self,
        request: &ReplicationEnrollmentRequest,
    ) -> Result<ReplicationEnrollmentBundle, ReplicationProductError> {
        let update = self.enroll_device(request.replica_id.clone(), request.public_key.clone())?;
        let package = update
            .package_for(&request.replica_id)
            .ok_or(ReplicationProductError::EnrollmentMismatch)?
            .package
            .clone();
        Ok(ReplicationEnrollmentBundle {
            workspace_id: self.workspace_id.clone(),
            recipient: request.replica_id.clone(),
            transport_slot: self.transport_slot.clone(),
            authority_state: update.authority_state,
            package,
        })
    }

    pub fn accept_enrollment(
        root: impl Into<PathBuf>,
        backend: B,
        bundle: &ReplicationEnrollmentBundle,
        expected_verification_code: &str,
    ) -> Result<Self, ReplicationProductError> {
        let root = root.into();
        validate_existing_directory(&root)?;
        let vault = ReplicationSecretVault::new(backend);
        recover_enrollment_activation(&root, &vault)?;
        let _lock = ProductAdvisoryLock::acquire(&root.join(PRODUCT_LOCK_FILE))?;
        if reject_unsafe_file_if_present(&root.join(PRODUCT_PROFILE_FILE))? {
            return Err(ReplicationProductError::AlreadyConfigured);
        }
        if reject_unsafe_file_if_present(&root.join(ENROLLMENT_TRANSACTION_FILE))?
            || path_exists(&root.join(PRODUCT_REPOSITORY_DIR))?
        {
            return Err(ReplicationProductError::PendingAuthorityTransition);
        }
        let pending = read_pending_enrollment(&root)?;
        let request = pending.request.into_request()?;
        if request.replica_id() != bundle.recipient()
            || bundle.verification_code(&request)? != expected_verification_code.trim()
        {
            return Err(ReplicationProductError::VerificationCodeMismatch);
        }
        let authority = ReplicationAuthorityState::from_canonical_bytes(&bundle.authority_state)?;
        let enrolled_public = authority
            .device(bundle.recipient())
            .filter(|device| device.status() == ReplicationAuthorityDeviceStatus::Active)
            .map(|device| device.public_key())
            .ok_or(ReplicationProductError::EnrollmentMismatch)?;
        if enrolled_public != request.public_key() {
            return Err(ReplicationProductError::EnrollmentMismatch);
        }
        let device_reference =
            ReplicationSecretRef::from_bytes(&decode_hex(&pending.device_reference_hex)?)?;
        let device_private = vault.load_device_key(&device_reference)?;
        if &device_private.public_key() != request.public_key() {
            return Err(ReplicationProductError::EnrollmentMismatch);
        }
        let wrapped = WrappedReplicationEpochKey::from_bytes(&bundle.package)?;
        let epoch_key = open_wrapped_replication_epoch_key(
            ReplicationKeyWrapContext {
                workspace_id: bundle.workspace_id(),
                recipient: bundle.recipient(),
            },
            authority.authority_public_key(),
            &device_private,
            authority.key_epoch(),
            &wrapped,
        )?;
        let epoch_reference = vault.store_epoch_key(&epoch_key)?;
        let transaction = StoredEnrollmentTransaction {
            format_version: PRODUCT_FORMAT_VERSION,
            epoch_reference_hex: encode_hex(&epoch_reference.to_bytes()),
        };
        write_canonical_file(
            &root.join(ENROLLMENT_TRANSACTION_FILE),
            &transaction,
            MAX_PRODUCT_TRANSACTION_BYTES as usize,
            "write enrollment transaction",
        )?;
        let mut profile_written = false;
        let activation = (|| {
            let historical = ReplicationHistoricalKeyIndex::from_retained(
                ReplicationHistoricalKeyLimit::new(MAX_REPLICATION_RETAINED_EPOCH_KEYS)?,
                [epoch_reference.clone()],
            )?;
            let custody = ReplicationCustodyMetadata::new_member(device_reference, historical)?;
            let policy = authority.replication_policy()?;
            let document =
                ReplicationDocument::new(bundle.workspace_id.clone(), Vec::new(), &policy)
                    .map_err(ReplicationStoreError::from)?;
            let repository = ReplicationRepository::open(root.join(PRODUCT_REPOSITORY_DIR))?;
            repository.create(document, custody, &policy)?;
            let profile = StoredProductProfile {
                format_version: PRODUCT_FORMAT_VERSION,
                workspace_id: bundle.workspace_id.as_str().to_string(),
                local_replica_id: bundle.recipient.as_str().to_string(),
                shared_folder: pending.shared_folder,
                transport_slot: bundle.transport_slot.file_component().to_string(),
                authority_state_hex: encode_hex(&bundle.authority_state),
            };
            write_profile(&root.join(PRODUCT_PROFILE_FILE), &profile)?;
            profile_written = true;
            remove_regular_file_if_present(
                &root.join(PENDING_ENROLLMENT_FILE),
                "remove pending enrollment",
            )?;
            remove_regular_file_if_present(
                &root.join(ENROLLMENT_TRANSACTION_FILE),
                "remove enrollment transaction",
            )?;
            sync_directory(&root)?;
            Ok(())
        })();
        if let Err(error) = activation {
            if !profile_written {
                let _ = vault.delete(&epoch_reference);
                let _ = remove_owned_directory(&root.join(PRODUCT_REPOSITORY_DIR));
                let _ = remove_regular_file_if_present(
                    &root.join(ENROLLMENT_TRANSACTION_FILE),
                    "remove enrollment transaction",
                );
            }
            return Err(error);
        }
        drop(_lock);
        Self::open(root, vault.into_backend())
    }

    pub fn revoke_device(
        &mut self,
        replica_id: &ReplicationReplicaId,
    ) -> Result<ReplicationAuthorityUpdate, ReplicationProductError> {
        if replica_id == &self.local_replica_id {
            return Err(ReplicationProductError::LocalDeviceRevocation);
        }
        let policy = self.authority.replication_policy()?;
        let snapshot = self.repository.load(&self.workspace_id, &policy)?;
        let accepted_through = snapshot
            .document
            .entries
            .iter()
            .flat_map(|entry| &entry.candidates)
            .map(|candidate| candidate.vector.counter(replica_id))
            .max()
            .unwrap_or_default();
        let authority_private = self.vault.load_authority_key(
            snapshot
                .custody
                .authority_reference()
                .ok_or(ReplicationProductError::AuthorityOwnerRequired)?,
        )?;
        let transition = revoke_replication_device(
            &self.authority,
            &authority_private,
            self.authority.revision(),
            replica_id,
            accepted_through,
        )?;
        self.commit_authority_transition(snapshot, transition, false)
    }

    pub fn apply_authority_update(
        &mut self,
        update: &ReplicationAuthorityUpdate,
    ) -> Result<(), ReplicationProductError> {
        let _lock = ProductAdvisoryLock::acquire(&self.root.join(PRODUCT_LOCK_FILE))?;
        if reject_unsafe_file_if_present(&self.root.join(PRODUCT_TRANSACTION_FILE))? {
            return Err(ReplicationProductError::PendingAuthorityTransition);
        }
        let current_policy = self.authority.replication_policy()?;
        let snapshot = self.repository.load(&self.workspace_id, &current_policy)?;
        if snapshot.custody.is_authority_owner() {
            return Err(ReplicationProductError::EnrollmentMismatch);
        }
        let next_authority =
            ReplicationAuthorityState::from_canonical_bytes(update.authority_state())?;
        if next_authority.workspace_id() != &self.workspace_id
            || next_authority.authority_public_key() != self.authority.authority_public_key()
            || next_authority.revision().get()
                != self
                    .authority
                    .revision()
                    .get()
                    .checked_add(1)
                    .ok_or(ReplicationProductError::InvalidProfile)?
            || next_authority.key_epoch().get()
                != self
                    .authority
                    .key_epoch()
                    .get()
                    .checked_add(1)
                    .ok_or(ReplicationProductError::InvalidProfile)?
        {
            return Err(ReplicationProductError::StaleAuthorityUpdate);
        }
        let device = next_authority
            .device(&self.local_replica_id)
            .filter(|device| device.status() == ReplicationAuthorityDeviceStatus::Active)
            .ok_or(ReplicationProductError::EnrollmentMismatch)?;
        let package = update
            .package_for(&self.local_replica_id)
            .ok_or(ReplicationProductError::EnrollmentMismatch)?;
        let wrapped = WrappedReplicationEpochKey::from_bytes(package.package())?;
        let device_private = self
            .vault
            .load_device_key(snapshot.custody.device_reference())?;
        if &device_private.public_key() != device.public_key() {
            return Err(ReplicationProductError::EnrollmentMismatch);
        }
        let epoch_key = open_wrapped_replication_epoch_key(
            ReplicationKeyWrapContext {
                workspace_id: &self.workspace_id,
                recipient: &self.local_replica_id,
            },
            self.authority.authority_public_key(),
            &device_private,
            next_authority.key_epoch(),
            &wrapped,
        )?;
        let epoch_reference = self.vault.store_epoch_key(&epoch_key)?;
        let historical_update = snapshot
            .custody
            .historical()
            .append(epoch_reference.clone())?;
        let (historical, retired) = historical_update.into_parts();
        let next_custody = ReplicationCustodyMetadata::new_member(
            snapshot.custody.device_reference().clone(),
            historical,
        )?;
        let next_policy = next_authority.replication_policy()?;
        let transaction = StoredAuthorityTransaction {
            format_version: PRODUCT_FORMAT_VERSION,
            base_authority_revision: self.authority.revision().get(),
            base_repository_revision: snapshot.revision.get(),
            next_authority_state_hex: encode_hex(update.authority_state()),
            new_epoch_reference_hex: encode_hex(&epoch_reference.to_bytes()),
            packages: stored_packages(update),
            publish_update: false,
        };
        if let Err(error) = write_authority_transaction(&self.root, &transaction) {
            let _ = self.vault.delete(&epoch_reference);
            return Err(error);
        }
        if let Err(error) = self.repository.commit(
            snapshot.revision,
            snapshot.document,
            next_custody,
            &retired,
            &next_policy,
        ) {
            let _ = self.repository.retire_pending(
                self.vault.backend(),
                &self.workspace_id,
                &next_policy,
            );
            let _ = self.vault.delete(&epoch_reference);
            let _ = remove_authority_transaction(&self.root);
            return Err(error.into());
        }
        let profile = self.stored_profile(&next_authority)?;
        write_profile(&self.root.join(PRODUCT_PROFILE_FILE), &profile)?;
        self.authority = next_authority;
        self.repository
            .retire_pending(self.vault.backend(), &self.workspace_id, &next_policy)?;
        remove_authority_transaction(&self.root)
    }

    pub fn status(&self) -> Result<ReplicationProductStatus, ReplicationProductError> {
        let policy = self.authority.replication_policy()?;
        let snapshot = self.repository.load(&self.workspace_id, &policy)?;
        Ok(ReplicationProductStatus {
            repository_revision: snapshot.revision,
            authority_revision: self.authority.revision().get(),
            key_epoch: self.authority.key_epoch().get(),
            active_devices: self.authority.active_device_count(),
            total_devices: self.authority.device_count(),
            record_count: snapshot.document.entries.len(),
            recovery_required: snapshot.source == super::ReplicationRepositorySource::LastGood,
            secret_retirement_pending: snapshot.retirement_pending,
            authority_owner: snapshot.custody.is_authority_owner(),
        })
    }

    pub fn records(&self) -> Result<Vec<ReplicationProductRecord>, ReplicationProductError> {
        let policy = self.authority.replication_policy()?;
        let snapshot = self.repository.load(&self.workspace_id, &policy)?;
        snapshot
            .document
            .entries
            .iter()
            .map(|entry| {
                if entry.candidates.len() != 1 {
                    return Err(ReplicationProductError::RecordConflict);
                }
                Ok(ReplicationProductRecord {
                    key: entry.key.clone(),
                    value: self.open_candidate(&snapshot, &entry.key, &entry.candidates[0])?,
                })
            })
            .collect()
    }

    pub fn deletion_plan(&self) -> Result<ReplicationDeletionPlan, ReplicationProductError> {
        let policy = self.authority.replication_policy()?;
        let snapshot = self.repository.load(&self.workspace_id, &policy)?;
        let references = custody_references(&snapshot.custody);
        let mut digest = Sha256::new();
        digest.update(DELETION_PLAN_DOMAIN);
        digest.update(self.authority.to_canonical_bytes()?);
        digest.update(snapshot.revision.get().to_be_bytes());
        digest.update((snapshot.document.entries.len() as u64).to_be_bytes());
        for reference in &references {
            digest.update(reference.to_bytes());
        }
        Ok(ReplicationDeletionPlan {
            token: digest.finalize().into(),
            record_count: snapshot.document.entries.len(),
            secret_count: references.len(),
            authority_owner: snapshot.custody.is_authority_owner(),
        })
    }

    pub fn delete_local_replica(
        self,
        plan: &ReplicationDeletionPlan,
        confirmation: &str,
    ) -> Result<(), ReplicationProductError> {
        if confirmation != DELETION_CONFIRMATION {
            return Err(ReplicationProductError::DeletionConfirmationRequired);
        }
        let root = self.root.clone();
        {
            let _lock = ProductAdvisoryLock::acquire(&root.join(PRODUCT_LOCK_FILE))?;
            let current = self.deletion_plan()?;
            if current.token != plan.token {
                return Err(ReplicationProductError::StaleDeletionPlan);
            }
            let policy = self.authority.replication_policy()?;
            let snapshot = self.repository.load(&self.workspace_id, &policy)?;
            let references = custody_references(&snapshot.custody);
            let transaction = StoredDeletionTransaction {
                format_version: PRODUCT_FORMAT_VERSION,
                references_hex: references
                    .iter()
                    .map(|reference| encode_hex(&reference.to_bytes()))
                    .collect(),
            };
            write_canonical_file(
                &root.join(DELETION_TRANSACTION_FILE),
                &transaction,
                MAX_PRODUCT_TRANSACTION_BYTES as usize,
                "write deletion transaction",
            )?;
            for reference in &references {
                self.vault.delete(reference)?;
            }
        }
        remove_owned_directory(&root)
    }

    pub fn review_sync(&self) -> Result<ReplicationSyncPlan, ReplicationProductError> {
        let policy = self.authority.replication_policy()?;
        Ok(self.sync_coordinator().review(&policy)?)
    }

    pub fn apply_sync(
        &self,
        plan: &ReplicationSyncPlan,
    ) -> Result<ReplicationSyncOutcome, ReplicationProductError> {
        let policy = self.authority.replication_policy()?;
        Ok(self.sync_coordinator().apply(plan, &policy)?)
    }

    pub fn conflict_candidates(
        &self,
        plan: &ReplicationSyncPlan,
        key: &ReplicationRecordKey,
    ) -> Result<Vec<ReplicationConflictCandidate>, ReplicationProductError> {
        let policy = self.authority.replication_policy()?;
        let snapshot = self.repository.load(&self.workspace_id, &policy)?;
        let candidates = plan
            .candidates_for(key)
            .filter(|candidates| candidates.len() > 1)
            .ok_or(ReplicationProductError::RecordConflict)?;
        candidates
            .iter()
            .map(|candidate| {
                Ok(ReplicationConflictCandidate {
                    author: candidate.author.clone(),
                    value: self.open_candidate(&snapshot, key, candidate)?,
                })
            })
            .collect()
    }

    pub fn resolve_sync(
        &self,
        plan: &ReplicationSyncPlan,
        choices: Vec<ReplicationConflictChoice>,
    ) -> Result<ReplicationSyncOutcome, ReplicationProductError> {
        let policy = self.authority.replication_policy()?;
        let snapshot = self.repository.load(&self.workspace_id, &policy)?;
        let epoch = snapshot.custody.historical().current_epoch();
        let epoch_key = self
            .vault
            .load_epoch_key(snapshot.custody.historical().reference_for(epoch)?, epoch)?;
        let mut resolutions = Vec::with_capacity(choices.len());
        for choice in choices {
            let context = plan.resolution_context(&choice.key, &self.local_replica_id, &policy)?;
            let operation_kind = if choice.value.is_some() {
                ReplicationOperationKind::Put
            } else {
                ReplicationOperationKind::Delete
            };
            let seal_context = ReplicationSealContext {
                workspace_id: &self.workspace_id,
                record_key: &choice.key,
                author: context.author(),
                vector: context.vector(),
                operation: operation_kind,
            };
            let sealed_payload = match choice.value {
                Some(value) => seal_put(seal_context, &epoch_key, &value)?.to_sealed_payload()?,
                None => seal_delete(seal_context, &epoch_key)?.to_sealed_payload()?,
            };
            let operation = match operation_kind {
                ReplicationOperationKind::Put => ReplicationOperation::Put { sealed_payload },
                ReplicationOperationKind::Delete => ReplicationOperation::Delete { sealed_payload },
            };
            let version = ReplicatedVersion::new(
                context.author().clone(),
                context.vector().clone(),
                operation,
                &policy,
            )
            .map_err(ReplicationStoreError::from)?;
            resolutions.push(ReplicationConflictResolution::new(choice.key, version));
        }
        Ok(self
            .sync_coordinator()
            .resolve(plan, &self.local_replica_id, resolutions, &policy)?)
    }

    pub fn recover_repository(
        &self,
    ) -> Result<ReplicationRecoveryOutcome, ReplicationProductError> {
        let policy = self.authority.replication_policy()?;
        Ok(self
            .repository
            .recover_last_good(&self.workspace_id, &policy)?)
    }

    pub fn put_record(
        &self,
        key: ReplicationRecordKey,
        plaintext: &[u8],
    ) -> Result<ReplicationRepositoryRevision, ReplicationProductError> {
        if plaintext.is_empty() {
            return Err(ReplicationProductError::EmptyPut);
        }
        if plaintext.len() > MAX_REPLICATION_PLAINTEXT_BYTES {
            return Err(ReplicationProductError::Crypto(
                ReplicationCryptoError::PlaintextTooLarge,
            ));
        }
        self.mutate_record(key, Some(plaintext))
    }

    pub fn delete_record(
        &self,
        key: ReplicationRecordKey,
    ) -> Result<ReplicationRepositoryRevision, ReplicationProductError> {
        self.mutate_record(key, None)
    }

    pub fn read_record(
        &self,
        key: &ReplicationRecordKey,
    ) -> Result<Option<Vec<u8>>, ReplicationProductError> {
        let policy = self.authority.replication_policy()?;
        let snapshot = self.repository.load(&self.workspace_id, &policy)?;
        let Some(entry) = snapshot
            .document
            .entries
            .iter()
            .find(|entry| &entry.key == key)
        else {
            return Ok(None);
        };
        if entry.candidates.len() != 1 {
            return Err(ReplicationProductError::RecordConflict);
        }
        self.open_candidate(&snapshot, key, &entry.candidates[0])
    }

    fn open_candidate(
        &self,
        snapshot: &super::ReplicationRepositorySnapshot,
        key: &ReplicationRecordKey,
        candidate: &ReplicatedVersion,
    ) -> Result<Option<Vec<u8>>, ReplicationProductError> {
        let (kind, payload) = match &candidate.operation {
            ReplicationOperation::Put { sealed_payload } => {
                (ReplicationOperationKind::Put, sealed_payload)
            }
            ReplicationOperation::Delete { sealed_payload } => {
                (ReplicationOperationKind::Delete, sealed_payload)
            }
        };
        let envelope =
            termirust_replication_security::ReplicationEnvelope::from_sealed_payload(payload)?;
        let reference = snapshot
            .custody
            .historical()
            .reference_for(envelope.key_epoch())?;
        let epoch_key = self.vault.load_epoch_key(reference, envelope.key_epoch())?;
        let opened = open_envelope(
            ReplicationSealContext {
                workspace_id: &self.workspace_id,
                record_key: key,
                author: &candidate.author,
                vector: &candidate.vector,
                operation: kind,
            },
            &epoch_key,
            &envelope,
        )?;
        match opened {
            OpenedReplicationOperation::Put(value) => Ok(Some(value.as_bytes().to_vec())),
            OpenedReplicationOperation::Delete => Ok(None),
        }
    }

    fn mutate_record(
        &self,
        key: ReplicationRecordKey,
        plaintext: Option<&[u8]>,
    ) -> Result<ReplicationRepositoryRevision, ReplicationProductError> {
        key.validate().map_err(ReplicationStoreError::from)?;
        let policy = self.authority.replication_policy()?;
        let snapshot = self.repository.load(&self.workspace_id, &policy)?;
        if snapshot.source != super::ReplicationRepositorySource::Primary {
            return Err(ReplicationProductError::Store(
                ReplicationStoreError::RecoveryRequired,
            ));
        }
        if snapshot.retirement_pending {
            return Err(ReplicationProductError::Store(
                ReplicationStoreError::PendingRetirement,
            ));
        }
        let observed = snapshot
            .document
            .entries
            .iter()
            .find(|entry| entry.key == key)
            .map(|entry| {
                entry
                    .candidates
                    .iter()
                    .map(|candidate| candidate.vector.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let placeholder =
            SealedReplicationPayload::new(vec![1]).map_err(ReplicationStoreError::from)?;
        let placeholder_operation = match plaintext {
            Some(_) => ReplicationOperation::Put {
                sealed_payload: placeholder,
            },
            None => ReplicationOperation::Delete {
                sealed_payload: placeholder,
            },
        };
        let next = policy
            .next_version(&self.local_replica_id, &observed, placeholder_operation)
            .map_err(ReplicationStoreError::from)?;
        let operation_kind = if plaintext.is_some() {
            ReplicationOperationKind::Put
        } else {
            ReplicationOperationKind::Delete
        };
        let epoch = snapshot.custody.historical().current_epoch();
        let epoch_reference = snapshot.custody.historical().reference_for(epoch)?;
        let epoch_key = self.vault.load_epoch_key(epoch_reference, epoch)?;
        let context = ReplicationSealContext {
            workspace_id: &self.workspace_id,
            record_key: &key,
            author: &self.local_replica_id,
            vector: &next.vector,
            operation: operation_kind,
        };
        let sealed_payload = match plaintext {
            Some(value) => seal_put(context, &epoch_key, value)?.to_sealed_payload()?,
            None => seal_delete(context, &epoch_key)?.to_sealed_payload()?,
        };
        let operation = match plaintext {
            Some(_) => ReplicationOperation::Put { sealed_payload },
            None => ReplicationOperation::Delete { sealed_payload },
        };
        let version = ReplicatedVersion::new(
            self.local_replica_id.clone(),
            next.vector,
            operation,
            &policy,
        )
        .map_err(ReplicationStoreError::from)?;
        let mut entries = snapshot.document.entries;
        match entries.iter_mut().find(|entry| entry.key == key) {
            Some(entry) => entry.candidates = vec![version],
            None => entries.push(ReplicationEntry {
                key,
                candidates: vec![version],
            }),
        }
        let document = ReplicationDocument::new(self.workspace_id.clone(), entries, &policy)
            .map_err(ReplicationStoreError::from)?;
        let committed =
            self.repository
                .commit(snapshot.revision, document, snapshot.custody, &[], &policy)?;
        Ok(committed.revision)
    }

    fn commit_authority_transition(
        &mut self,
        snapshot: super::ReplicationRepositorySnapshot,
        transition: ReplicationAuthorityTransition,
        rekey_for_new_member: bool,
    ) -> Result<ReplicationAuthorityUpdate, ReplicationProductError> {
        let _lock = ProductAdvisoryLock::acquire(&self.root.join(PRODUCT_LOCK_FILE))?;
        if reject_unsafe_file_if_present(&self.root.join(PRODUCT_TRANSACTION_FILE))? {
            return Err(ReplicationProductError::PendingAuthorityTransition);
        }
        if reject_unsafe_file_if_present(&self.root.join(PRODUCT_UPDATE_OUTBOX_FILE))? {
            return Err(ReplicationProductError::PendingAuthorityUpdate);
        }
        let current = self
            .repository
            .load(&self.workspace_id, &self.authority.replication_policy()?)?;
        if current.revision != snapshot.revision {
            return Err(ReplicationProductError::Store(
                ReplicationStoreError::StaleRepositoryRevision {
                    expected: snapshot.revision,
                    actual: current.revision,
                },
            ));
        }

        let (next_authority, epoch_key, distribution) = transition.into_parts();
        let next_policy = next_authority.replication_policy()?;
        let next_document = if rekey_for_new_member {
            self.rekey_document(&snapshot, &epoch_key, &next_policy)?
        } else {
            snapshot.document.clone()
        };
        let authority_update = ReplicationAuthorityUpdate {
            authority_revision: next_authority.revision().get(),
            key_epoch: next_authority.key_epoch().get(),
            authority_state: next_authority.to_canonical_bytes()?,
            packages: distribution
                .packages()
                .iter()
                .map(|package| ReplicationDeviceKeyPackage {
                    recipient: package.recipient().clone(),
                    package: package.package().to_bytes(),
                })
                .collect(),
        };
        let epoch_reference = self.vault.store_epoch_key(&epoch_key)?;
        let historical_update = snapshot
            .custody
            .historical()
            .append(epoch_reference.clone())?;
        let (historical, retired) = historical_update.into_parts();
        let next_custody = ReplicationCustodyMetadata::new(
            snapshot
                .custody
                .authority_reference()
                .ok_or(ReplicationProductError::AuthorityOwnerRequired)?
                .clone(),
            snapshot.custody.device_reference().clone(),
            historical,
        )?;
        let transaction = StoredAuthorityTransaction {
            format_version: PRODUCT_FORMAT_VERSION,
            base_authority_revision: self.authority.revision().get(),
            base_repository_revision: snapshot.revision.get(),
            next_authority_state_hex: encode_hex(&next_authority.to_canonical_bytes()?),
            new_epoch_reference_hex: encode_hex(&epoch_reference.to_bytes()),
            packages: stored_packages(&authority_update),
            publish_update: true,
        };
        if let Err(error) = write_authority_transaction(&self.root, &transaction) {
            let _ = self.vault.delete(&epoch_reference);
            return Err(error);
        }
        if let Err(error) = self.repository.commit(
            snapshot.revision,
            next_document,
            next_custody,
            &retired,
            &next_policy,
        ) {
            let _ = self.repository.retire_pending(
                self.vault.backend(),
                &self.workspace_id,
                &next_policy,
            );
            let _ = self.vault.delete(&epoch_reference);
            let _ = remove_authority_transaction(&self.root);
            return Err(error.into());
        }

        let profile = self.stored_profile(&next_authority)?;
        write_profile(&self.root.join(PRODUCT_PROFILE_FILE), &profile)?;
        self.authority = next_authority;
        self.repository
            .retire_pending(self.vault.backend(), &self.workspace_id, &next_policy)?;
        write_authority_update(&self.root, &authority_update)?;
        remove_authority_transaction(&self.root)?;
        Ok(authority_update)
    }

    fn rekey_document(
        &self,
        snapshot: &super::ReplicationRepositorySnapshot,
        next_epoch_key: &termirust_replication_security::ReplicationEpochKey,
        next_policy: &termirust_domain::ReplicationPolicy,
    ) -> Result<ReplicationDocument, ReplicationProductError> {
        let mut entries = Vec::with_capacity(snapshot.document.entries.len());
        for entry in &snapshot.document.entries {
            if entry.candidates.len() != 1 {
                return Err(ReplicationProductError::Store(
                    ReplicationStoreError::ConflictResolutionRequired,
                ));
            }
            let candidate = &entry.candidates[0];
            let (operation_kind, old_payload) = match &candidate.operation {
                ReplicationOperation::Put { sealed_payload } => {
                    (ReplicationOperationKind::Put, sealed_payload)
                }
                ReplicationOperation::Delete { sealed_payload } => {
                    (ReplicationOperationKind::Delete, sealed_payload)
                }
            };
            let old_envelope =
                termirust_replication_security::ReplicationEnvelope::from_sealed_payload(
                    old_payload,
                )?;
            let old_reference = snapshot
                .custody
                .historical()
                .reference_for(old_envelope.key_epoch())?;
            let old_epoch_key = self
                .vault
                .load_epoch_key(old_reference, old_envelope.key_epoch())?;
            let opened = open_envelope(
                ReplicationSealContext {
                    workspace_id: &self.workspace_id,
                    record_key: &entry.key,
                    author: &candidate.author,
                    vector: &candidate.vector,
                    operation: operation_kind,
                },
                &old_epoch_key,
                &old_envelope,
            )?;
            let placeholder =
                SealedReplicationPayload::new(vec![1]).map_err(ReplicationStoreError::from)?;
            let placeholder_operation = match operation_kind {
                ReplicationOperationKind::Put => ReplicationOperation::Put {
                    sealed_payload: placeholder,
                },
                ReplicationOperationKind::Delete => ReplicationOperation::Delete {
                    sealed_payload: placeholder,
                },
            };
            let observed = [candidate.vector.clone()];
            let next = next_policy
                .next_version(&self.local_replica_id, &observed, placeholder_operation)
                .map_err(ReplicationStoreError::from)?;
            let context = ReplicationSealContext {
                workspace_id: &self.workspace_id,
                record_key: &entry.key,
                author: &self.local_replica_id,
                vector: &next.vector,
                operation: operation_kind,
            };
            let sealed_payload = match opened {
                OpenedReplicationOperation::Put(plaintext) => {
                    seal_put(context, next_epoch_key, plaintext.as_bytes())?.to_sealed_payload()?
                }
                OpenedReplicationOperation::Delete => {
                    seal_delete(context, next_epoch_key)?.to_sealed_payload()?
                }
            };
            let operation = match operation_kind {
                ReplicationOperationKind::Put => ReplicationOperation::Put { sealed_payload },
                ReplicationOperationKind::Delete => ReplicationOperation::Delete { sealed_payload },
            };
            entries.push(ReplicationEntry {
                key: entry.key.clone(),
                candidates: vec![
                    ReplicatedVersion::new(
                        self.local_replica_id.clone(),
                        next.vector,
                        operation,
                        next_policy,
                    )
                    .map_err(ReplicationStoreError::from)?,
                ],
            });
        }
        ReplicationDocument::new(self.workspace_id.clone(), entries, next_policy)
            .map_err(ReplicationStoreError::from)
            .map_err(Into::into)
    }

    fn stored_profile(
        &self,
        authority: &ReplicationAuthorityState,
    ) -> Result<StoredProductProfile, ReplicationProductError> {
        let mut profile = read_profile(&self.root.join(PRODUCT_PROFILE_FILE))?;
        if profile.workspace_id != self.workspace_id.as_str()
            || profile.local_replica_id != self.local_replica_id.as_str()
            || profile.transport_slot != self.transport_slot.file_component()
        {
            return Err(ReplicationProductError::InvalidProfile);
        }
        profile.authority_state_hex = encode_hex(&authority.to_canonical_bytes()?);
        Ok(profile)
    }

    fn sync_coordinator(&self) -> ReplicationSyncCoordinator {
        ReplicationSyncCoordinator::new(self.repository.clone(), self.transport.clone())
    }
}

fn read_pending_enrollment(
    root: &Path,
) -> Result<StoredPendingEnrollment, ReplicationProductError> {
    let path = root.join(PENDING_ENROLLMENT_FILE);
    if !reject_unsafe_file_if_present(&path)? {
        return Err(ReplicationProductError::NotConfigured);
    }
    let bytes = read_bounded_regular_file(&path, MAX_PRODUCT_PROFILE_BYTES, "read enrollment")?;
    decode_canonical_json(&bytes, MAX_PRODUCT_PROFILE_BYTES as usize)
}

fn custody_references(custody: &ReplicationCustodyMetadata) -> Vec<ReplicationSecretRef> {
    let mut references = Vec::with_capacity(custody.historical().len() + 2);
    if let Some(authority) = custody.authority_reference() {
        references.push(authority.clone());
    }
    references.push(custody.device_reference().clone());
    references.extend(custody.historical().references().cloned());
    references.sort();
    references
}

fn continue_pending_deletion<B: ReplicationSecretBackend>(
    root: &Path,
    vault: &ReplicationSecretVault<B>,
) -> Result<bool, ReplicationProductError> {
    let path = root.join(DELETION_TRANSACTION_FILE);
    if !reject_unsafe_file_if_present(&path)? {
        return Ok(false);
    }
    {
        let _lock = ProductAdvisoryLock::acquire(&root.join(PRODUCT_LOCK_FILE))?;
        let bytes = read_bounded_regular_file(
            &path,
            MAX_PRODUCT_TRANSACTION_BYTES,
            "read deletion transaction",
        )?;
        let transaction: StoredDeletionTransaction =
            decode_canonical_json(&bytes, MAX_PRODUCT_TRANSACTION_BYTES as usize)?;
        if transaction.format_version != PRODUCT_FORMAT_VERSION
            || transaction.references_hex.len()
                > MAX_REPLICATION_RETAINED_EPOCH_KEYS.saturating_add(2)
        {
            return Err(ReplicationProductError::InvalidProfile);
        }
        let mut unique = BTreeSet::new();
        let mut references = Vec::with_capacity(transaction.references_hex.len());
        for encoded in transaction.references_hex {
            let reference = ReplicationSecretRef::from_bytes(&decode_hex(&encoded)?)?;
            if !unique.insert(reference.clone()) {
                return Err(ReplicationProductError::InvalidProfile);
            }
            references.push(reference);
        }
        for reference in &references {
            vault.delete(reference)?;
        }
    }
    remove_owned_directory(root)?;
    Ok(true)
}

fn recover_enrollment_activation<B: ReplicationSecretBackend>(
    root: &Path,
    vault: &ReplicationSecretVault<B>,
) -> Result<(), ReplicationProductError> {
    let transaction_path = root.join(ENROLLMENT_TRANSACTION_FILE);
    if !reject_unsafe_file_if_present(&transaction_path)? {
        return Ok(());
    }
    let _lock = ProductAdvisoryLock::acquire(&root.join(PRODUCT_LOCK_FILE))?;
    let bytes = read_bounded_regular_file(
        &transaction_path,
        MAX_PRODUCT_TRANSACTION_BYTES,
        "read enrollment transaction",
    )?;
    let transaction: StoredEnrollmentTransaction =
        decode_canonical_json(&bytes, MAX_PRODUCT_TRANSACTION_BYTES as usize)?;
    if transaction.format_version != PRODUCT_FORMAT_VERSION {
        return Err(ReplicationProductError::InvalidProfile);
    }
    let epoch_reference =
        ReplicationSecretRef::from_bytes(&decode_hex(&transaction.epoch_reference_hex)?)?;
    if reject_unsafe_file_if_present(&root.join(PRODUCT_PROFILE_FILE))? {
        remove_regular_file_if_present(
            &root.join(PENDING_ENROLLMENT_FILE),
            "remove pending enrollment",
        )?;
        remove_regular_file_if_present(&transaction_path, "remove enrollment transaction")?;
        sync_directory(root)?;
        return Ok(());
    }

    remove_owned_directory(&root.join(PRODUCT_REPOSITORY_DIR))?;
    vault.delete(&epoch_reference)?;
    remove_regular_file_if_present(&transaction_path, "remove enrollment transaction")?;
    sync_directory(root)
}

fn path_exists(path: &Path) -> Result<bool, ReplicationProductError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(profile_io("inspect product entry", error)),
    }
}

fn remove_owned_directory(path: &Path) -> Result<(), ReplicationProductError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => Err(
            ReplicationProductError::Store(ReplicationStoreError::UnsafeEntry),
        ),
        Ok(_) => fs::remove_dir_all(path)
            .map_err(|error| profile_io("remove enrollment repository", error)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(profile_io("inspect enrollment repository", error)),
    }
}

fn remove_regular_file_if_present(
    path: &Path,
    operation: &'static str,
) -> Result<(), ReplicationProductError> {
    if reject_unsafe_file_if_present(path)? {
        fs::remove_file(path).map_err(|error| profile_io(operation, error))?;
    }
    Ok(())
}

fn canonical_json<T: Serialize>(
    value: &T,
    maximum: usize,
) -> Result<Vec<u8>, ReplicationProductError> {
    let bytes = serde_json::to_vec(value).map_err(|_| ReplicationProductError::InvalidProfile)?;
    if bytes.len() > maximum {
        return Err(ReplicationProductError::InvalidProfile);
    }
    Ok(bytes)
}

fn decode_canonical_json<T: DeserializeOwned + Serialize>(
    bytes: &[u8],
    maximum: usize,
) -> Result<T, ReplicationProductError> {
    if bytes.len() > maximum {
        return Err(ReplicationProductError::InvalidProfile);
    }
    let value: T =
        serde_json::from_slice(bytes).map_err(|_| ReplicationProductError::InvalidProfile)?;
    if canonical_json(&value, maximum)? != bytes {
        return Err(ReplicationProductError::InvalidProfile);
    }
    Ok(value)
}

fn write_canonical_file<T: Serialize>(
    path: &Path,
    value: &T,
    maximum: usize,
    operation: &'static str,
) -> Result<(), ReplicationProductError> {
    let bytes = canonical_json(value, maximum)?;
    SystemAtomicWriter
        .write(path, &bytes)
        .map_err(|error| profile_io(operation, error))?;
    Ok(())
}

fn recover_authority_transaction<B: ReplicationSecretBackend>(
    root: &Path,
    profile: &StoredProductProfile,
    current_authority: ReplicationAuthorityState,
    workspace_id: &ReplicationWorkspaceId,
    repository: &ReplicationRepository,
    vault: &ReplicationSecretVault<B>,
) -> Result<ReplicationAuthorityState, ReplicationProductError> {
    let transaction_path = root.join(PRODUCT_TRANSACTION_FILE);
    if !reject_unsafe_file_if_present(&transaction_path)? {
        return Ok(current_authority);
    }
    let _lock = ProductAdvisoryLock::acquire(&root.join(PRODUCT_LOCK_FILE))?;
    let transaction = read_authority_transaction(root)?;
    if transaction.base_authority_revision == u64::MAX {
        return Err(ReplicationProductError::InvalidProfile);
    }
    let next_authority_bytes = decode_hex(&transaction.next_authority_state_hex)?;
    let next_authority = ReplicationAuthorityState::from_canonical_bytes(&next_authority_bytes)?;
    let current_is_base = current_authority.revision().get() == transaction.base_authority_revision;
    let current_is_next = current_authority == next_authority;
    if next_authority.workspace_id() != workspace_id
        || next_authority.revision().get()
            != transaction
                .base_authority_revision
                .checked_add(1)
                .ok_or(ReplicationProductError::InvalidProfile)?
        || (!current_is_base && !current_is_next)
        || (current_is_base
            && next_authority.key_epoch().get()
                != current_authority
                    .key_epoch()
                    .get()
                    .checked_add(1)
                    .ok_or(ReplicationProductError::InvalidProfile)?)
    {
        return Err(ReplicationProductError::InvalidProfile);
    }
    let authority_update = decode_authority_update(
        &next_authority,
        &transaction.next_authority_state_hex,
        &transaction.packages,
    )?;
    let reference_bytes = decode_hex(&transaction.new_epoch_reference_hex)?;
    let epoch_reference = ReplicationSecretRef::from_bytes(&reference_bytes)?;
    if epoch_reference.key_epoch() != Some(next_authority.key_epoch()) {
        return Err(ReplicationProductError::InvalidProfile);
    }
    let next_policy = next_authority.replication_policy()?;
    let snapshot = repository.load(workspace_id, &next_policy)?;
    let base_revision = ReplicationRepositoryRevision::new(transaction.base_repository_revision)?;
    let next_repository_revision = ReplicationRepositoryRevision::new(
        transaction
            .base_repository_revision
            .checked_add(1)
            .ok_or(ReplicationProductError::InvalidProfile)?,
    )?;

    if snapshot.revision == base_revision {
        if current_authority.revision().get() != transaction.base_authority_revision {
            return Err(ReplicationProductError::InvalidProfile);
        }
        repository.retire_pending(vault.backend(), workspace_id, &next_policy)?;
        vault.delete(&epoch_reference)?;
        remove_authority_transaction(root)?;
        return Ok(current_authority);
    }
    if snapshot.revision != next_repository_revision {
        return Err(ReplicationProductError::InvalidProfile);
    }

    if current_authority.revision().get() == transaction.base_authority_revision {
        let mut next_profile = profile.clone();
        next_profile.authority_state_hex = transaction.next_authority_state_hex.clone();
        write_profile(&root.join(PRODUCT_PROFILE_FILE), &next_profile)?;
    }
    repository.retire_pending(vault.backend(), workspace_id, &next_policy)?;
    if transaction.publish_update {
        write_authority_update(root, &authority_update)?;
    }
    remove_authority_transaction(root)?;
    Ok(next_authority)
}

fn write_authority_transaction(
    root: &Path,
    transaction: &StoredAuthorityTransaction,
) -> Result<(), ReplicationProductError> {
    let bytes =
        serde_json::to_vec(transaction).map_err(|_| ReplicationProductError::InvalidProfile)?;
    if bytes.len() as u64 > MAX_PRODUCT_TRANSACTION_BYTES {
        return Err(ReplicationProductError::InvalidProfile);
    }
    SystemAtomicWriter
        .write(&root.join(PRODUCT_TRANSACTION_FILE), &bytes)
        .map_err(|error| profile_io("write authority transaction", error))?;
    Ok(())
}

fn stored_packages(update: &ReplicationAuthorityUpdate) -> Vec<StoredDeviceKeyPackage> {
    update
        .packages()
        .iter()
        .map(|package| StoredDeviceKeyPackage {
            recipient: package.recipient().as_str().to_string(),
            package_hex: encode_hex(package.package()),
        })
        .collect()
}

fn decode_authority_update(
    authority: &ReplicationAuthorityState,
    authority_state_hex: &str,
    stored: &[StoredDeviceKeyPackage],
) -> Result<ReplicationAuthorityUpdate, ReplicationProductError> {
    if stored.len() != authority.active_device_count() {
        return Err(ReplicationProductError::InvalidProfile);
    }
    let mut recipients = BTreeSet::new();
    let mut packages = Vec::with_capacity(stored.len());
    for item in stored {
        let recipient = ReplicationReplicaId::new(item.recipient.clone())
            .map_err(|_| ReplicationProductError::InvalidProfile)?;
        if !recipients.insert(recipient.clone())
            || authority
                .device(&recipient)
                .is_none_or(|device| device.status() != ReplicationAuthorityDeviceStatus::Active)
        {
            return Err(ReplicationProductError::InvalidProfile);
        }
        let package = decode_hex(&item.package_hex)?;
        let parsed = WrappedReplicationEpochKey::from_bytes(&package)?;
        if parsed.key_epoch() != authority.key_epoch() {
            return Err(ReplicationProductError::InvalidProfile);
        }
        packages.push(ReplicationDeviceKeyPackage { recipient, package });
    }
    Ok(ReplicationAuthorityUpdate {
        authority_revision: authority.revision().get(),
        key_epoch: authority.key_epoch().get(),
        authority_state: decode_hex(authority_state_hex)?,
        packages,
    })
}

fn write_authority_update(
    root: &Path,
    update: &ReplicationAuthorityUpdate,
) -> Result<(), ReplicationProductError> {
    let stored = StoredAuthorityUpdate {
        format_version: PRODUCT_FORMAT_VERSION,
        authority_state_hex: encode_hex(update.authority_state()),
        packages: stored_packages(update),
    };
    let bytes = serde_json::to_vec(&stored).map_err(|_| ReplicationProductError::InvalidProfile)?;
    if bytes.len() as u64 > MAX_PRODUCT_TRANSACTION_BYTES {
        return Err(ReplicationProductError::InvalidProfile);
    }
    SystemAtomicWriter
        .write(&root.join(PRODUCT_UPDATE_OUTBOX_FILE), &bytes)
        .map_err(|error| profile_io("write authority update", error))?;
    Ok(())
}

fn read_authority_update(
    root: &Path,
) -> Result<Option<ReplicationAuthorityUpdate>, ReplicationProductError> {
    let path = root.join(PRODUCT_UPDATE_OUTBOX_FILE);
    if !reject_unsafe_file_if_present(&path)? {
        return Ok(None);
    }
    let bytes = read_bounded_regular_file(
        &path,
        MAX_PRODUCT_TRANSACTION_BYTES,
        "read authority update",
    )?;
    let stored: StoredAuthorityUpdate =
        serde_json::from_slice(&bytes).map_err(|_| ReplicationProductError::InvalidProfile)?;
    if stored.format_version != PRODUCT_FORMAT_VERSION
        || serde_json::to_vec(&stored).map_err(|_| ReplicationProductError::InvalidProfile)?
            != bytes
    {
        return Err(ReplicationProductError::InvalidProfile);
    }
    let authority_bytes = decode_hex(&stored.authority_state_hex)?;
    let authority = ReplicationAuthorityState::from_canonical_bytes(&authority_bytes)?;
    decode_authority_update(&authority, &stored.authority_state_hex, &stored.packages).map(Some)
}

fn read_authority_transaction(
    root: &Path,
) -> Result<StoredAuthorityTransaction, ReplicationProductError> {
    let path = root.join(PRODUCT_TRANSACTION_FILE);
    let bytes = read_bounded_regular_file(
        &path,
        MAX_PRODUCT_TRANSACTION_BYTES,
        "read authority transaction",
    )?;
    let transaction: StoredAuthorityTransaction =
        serde_json::from_slice(&bytes).map_err(|_| ReplicationProductError::InvalidProfile)?;
    if transaction.format_version != PRODUCT_FORMAT_VERSION
        || serde_json::to_vec(&transaction).map_err(|_| ReplicationProductError::InvalidProfile)?
            != bytes
    {
        return Err(ReplicationProductError::InvalidProfile);
    }
    Ok(transaction)
}

fn remove_authority_transaction(root: &Path) -> Result<(), ReplicationProductError> {
    fs::remove_file(root.join(PRODUCT_TRANSACTION_FILE))
        .map_err(|error| profile_io("remove authority transaction", error))?;
    sync_directory(root)
}

struct ProductAdvisoryLock {
    _process_guard: MutexGuard<'static, ()>,
    file: File,
}

static PRODUCT_PROCESS_LOCK: Mutex<()> = Mutex::new(());

impl ProductAdvisoryLock {
    fn acquire(path: &Path) -> Result<Self, ReplicationProductError> {
        let process_guard = PRODUCT_PROCESS_LOCK
            .lock()
            .map_err(|_| ReplicationProductError::InvalidProfile)?;
        reject_unsafe_file_if_present(path)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        #[cfg(windows)]
        options.custom_flags(0x0020_0000);
        let file = options
            .open(path)
            .map_err(|error| profile_io("open product lock", error))?;
        if !file
            .metadata()
            .map_err(|error| profile_io("inspect product lock", error))?
            .is_file()
        {
            return Err(ReplicationProductError::Store(
                ReplicationStoreError::UnsafeEntry,
            ));
        }
        #[cfg(unix)]
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|error| profile_io("set product lock permissions", error))?;
        crate::file_lock::exclusive(&file).map_err(|error| profile_io("lock product", error))?;
        Ok(Self {
            _process_guard: process_guard,
            file,
        })
    }
}

impl Drop for ProductAdvisoryLock {
    fn drop(&mut self) {
        crate::file_lock::release(&self.file);
    }
}

fn validate_existing_directory(path: &Path) -> Result<(), ReplicationProductError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| profile_io("inspect directory", error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ReplicationProductError::Store(
            ReplicationStoreError::UnsafeEntry,
        ));
    }
    Ok(())
}

fn create_private_directory(path: &Path) -> Result<(), ReplicationProductError> {
    fs::create_dir(path).map_err(|error| profile_io("create profile", error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|error| profile_io("set profile permissions", error))?;
    }
    Ok(())
}

fn write_profile(
    path: &Path,
    profile: &StoredProductProfile,
) -> Result<(), ReplicationProductError> {
    let bytes = serde_json::to_vec(profile).map_err(|_| ReplicationProductError::InvalidProfile)?;
    if bytes.len() as u64 > MAX_PRODUCT_PROFILE_BYTES {
        return Err(ReplicationProductError::InvalidProfile);
    }
    SystemAtomicWriter
        .write(path, &bytes)
        .map_err(|error| profile_io("write profile", error))?;
    Ok(())
}

fn read_profile(path: &Path) -> Result<StoredProductProfile, ReplicationProductError> {
    if !reject_unsafe_file_if_present(path)? {
        return Err(ReplicationProductError::NotConfigured);
    }
    let bytes = read_bounded_regular_file(path, MAX_PRODUCT_PROFILE_BYTES, "read profile")?;
    let profile: StoredProductProfile = match serde_json::from_slice(&bytes) {
        Ok(profile) => profile,
        Err(_) => {
            if let Ok(probe) = serde_json::from_slice::<ProductFormatProbe>(&bytes)
                && probe.format_version > PRODUCT_FORMAT_VERSION
            {
                return Err(ReplicationProductError::NewerProfile {
                    found: probe.format_version,
                    supported: PRODUCT_FORMAT_VERSION,
                });
            }
            return Err(ReplicationProductError::InvalidProfile);
        }
    };
    if profile.format_version > PRODUCT_FORMAT_VERSION {
        return Err(ReplicationProductError::NewerProfile {
            found: profile.format_version,
            supported: PRODUCT_FORMAT_VERSION,
        });
    }
    if profile.format_version != PRODUCT_FORMAT_VERSION
        || serde_json::to_vec(&profile).map_err(|_| ReplicationProductError::InvalidProfile)?
            != bytes
    {
        return Err(ReplicationProductError::InvalidProfile);
    }
    Ok(profile)
}

fn sync_directory(path: &Path) -> Result<(), ReplicationProductError> {
    match File::open(path).and_then(|directory| directory.sync_all()) {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::Unsupported
                    | io::ErrorKind::InvalidInput
                    | io::ErrorKind::PermissionDenied
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(profile_io("sync profile directory", error)),
    }
}

fn profile_io(operation: &'static str, error: io::Error) -> ReplicationProductError {
    ReplicationProductError::Store(io_error(operation, error))
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn decode_hex(value: &str) -> Result<Vec<u8>, ReplicationProductError> {
    if !value.len().is_multiple_of(2)
        || value.len() > MAX_REPLICATION_AUTHORITY_STATE_BYTES.saturating_mul(2)
    {
        return Err(ReplicationProductError::InvalidProfile);
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = decode_nibble(pair[0])?;
            let low = decode_nibble(pair[1])?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn decode_hex_array<const N: usize>(value: &str) -> Result<[u8; N], ReplicationProductError> {
    decode_hex(value)?
        .try_into()
        .map_err(|_| ReplicationProductError::InvalidProfile)
}

fn decode_nibble(value: u8) -> Result<u8, ReplicationProductError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        _ => Err(ReplicationProductError::InvalidProfile),
    }
}
