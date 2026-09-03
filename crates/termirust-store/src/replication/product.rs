use std::fmt;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use termirust_domain::{
    ReplicatedVersion, ReplicationDocument, ReplicationEntry, ReplicationOperation,
    ReplicationRecordKey, ReplicationReplicaId, ReplicationWorkspaceId, SealedReplicationPayload,
};
use termirust_replication_security::{
    MAX_REPLICATION_AUTHORITY_STATE_BYTES, MAX_REPLICATION_PLAINTEXT_BYTES,
    MAX_REPLICATION_RETAINED_EPOCH_KEYS, OpenedReplicationOperation, ReplicationAuthorityError,
    ReplicationAuthorityState, ReplicationCryptoError, ReplicationHistoricalKeyIndex,
    ReplicationHistoricalKeyLimit, ReplicationKeyWrappingError, ReplicationOperationKind,
    ReplicationSealContext, ReplicationSecretBackend, ReplicationSecretCustodyError,
    ReplicationSecretVault, bootstrap_replication_authority,
    generate_replication_authority_private_key, generate_replication_device_private_key,
    open as open_envelope, seal_delete, seal_put,
};
use uuid::Uuid;

use crate::{AtomicWriter as _, SystemAtomicWriter};

use super::{
    ReplicationCustodyMetadata, ReplicationRecoveryOutcome, ReplicationRepository,
    ReplicationRepositoryRevision, ReplicationStoreError, ReplicationSyncCoordinator,
    ReplicationSyncOutcome, ReplicationSyncPlan, SharedFolderReplicationTransport,
    SharedFolderSlot, io_error, read_bounded_regular_file, reject_unsafe_file_if_present,
};

const PRODUCT_FORMAT_VERSION: u16 = 1;
const PRODUCT_PROFILE_FILE: &str = "profile.json";
const PRODUCT_REPOSITORY_DIR: &str = "repository";
const MAX_PRODUCT_PROFILE_BYTES: u64 = 192 * 1024;

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

#[derive(Deserialize, Serialize)]
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

pub struct ReplicationProductService<B> {
    root: PathBuf,
    shared_folder: PathBuf,
    workspace_id: ReplicationWorkspaceId,
    local_replica_id: ReplicationReplicaId,
    authority: ReplicationAuthorityState,
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
        let transport =
            SharedFolderReplicationTransport::open(&shared_folder, workspace_id.clone(), slot)?;
        Ok(Self {
            root,
            shared_folder,
            workspace_id,
            local_replica_id,
            authority,
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
        let profile = read_profile(&root.join(PRODUCT_PROFILE_FILE))?;
        let workspace_id = ReplicationWorkspaceId::new(profile.workspace_id)
            .map_err(|_| ReplicationProductError::InvalidProfile)?;
        let local_replica_id = ReplicationReplicaId::new(profile.local_replica_id)
            .map_err(|_| ReplicationProductError::InvalidProfile)?;
        let shared_folder = PathBuf::from(profile.shared_folder);
        validate_existing_directory(&shared_folder)?;
        let slot = SharedFolderSlot::new(profile.transport_slot)
            .map_err(|_| ReplicationProductError::InvalidProfile)?;
        let authority_bytes = decode_hex(&profile.authority_state_hex)?;
        if authority_bytes.len() > MAX_REPLICATION_AUTHORITY_STATE_BYTES {
            return Err(ReplicationProductError::InvalidProfile);
        }
        let authority = ReplicationAuthorityState::from_canonical_bytes(&authority_bytes)?;
        if authority.workspace_id() != &workspace_id
            || authority.device(&local_replica_id).is_none()
        {
            return Err(ReplicationProductError::InvalidProfile);
        }
        let policy = authority.replication_policy()?;
        let repository = ReplicationRepository::open(root.join(PRODUCT_REPOSITORY_DIR))?;
        let snapshot = repository.load(&workspace_id, &policy)?;
        if snapshot.custody.historical().current_epoch() != authority.key_epoch() {
            return Err(ReplicationProductError::InvalidProfile);
        }
        let vault = ReplicationSecretVault::new(backend);
        vault.load_authority_key(snapshot.custody.authority_reference())?;
        vault.load_device_key(snapshot.custody.device_reference())?;
        vault.load_epoch_key(
            snapshot
                .custody
                .historical()
                .reference_for(authority.key_epoch())?,
            authority.key_epoch(),
        )?;
        let transport =
            SharedFolderReplicationTransport::open(&shared_folder, workspace_id.clone(), slot)?;
        Ok(Self {
            root,
            shared_folder,
            workspace_id,
            local_replica_id,
            authority,
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
        })
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
        let candidate = &entry.candidates[0];
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

    fn sync_coordinator(&self) -> ReplicationSyncCoordinator {
        ReplicationSyncCoordinator::new(self.repository.clone(), self.transport.clone())
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

fn decode_nibble(value: u8) -> Result<u8, ReplicationProductError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        _ => Err(ReplicationProductError::InvalidProfile),
    }
}
