mod repository;
mod sync;
mod transport;

use std::collections::BTreeSet;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt as _;

use fs2::FileExt as _;
use serde::de::{DeserializeSeed, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest as _, Sha256};
use termirust_domain::{
    MAX_REPLICATION_DOCUMENT_BYTES, REPLICATION_SCHEMA_VERSION, ReplicationDocument,
    ReplicationError, ReplicationPolicy, ReplicationWorkspaceId, merge_replication_documents,
};
use termirust_replication_security::{
    ReplicationHistoricalKeyIndex, ReplicationHistoricalKeyLimit, ReplicationSecretCustodyError,
    ReplicationSecretKind, ReplicationSecretRef,
};

pub use repository::{
    ReplicationRecoveryOutcome, ReplicationRepository, ReplicationRepositorySnapshot,
    ReplicationRepositorySource, ReplicationRetirementOutcome,
};
pub use sync::{
    ReplicationConflictOperationMix, ReplicationConflictResolution, ReplicationConflictReview,
    ReplicationResolutionContext, ReplicationSyncCoordinator, ReplicationSyncDisposition,
    ReplicationSyncOutcome, ReplicationSyncPlan, ReplicationSyncReviewToken,
};
pub use transport::{
    MAX_REPLICATION_CONFLICT_ARTIFACTS, SharedFolderConflictArtifact,
    SharedFolderReplicationInputs, SharedFolderReplicationTransport, SharedFolderTransportSnapshot,
    SharedFolderTransportState,
};

pub const MAX_REPLICATION_REPOSITORY_BYTES: u64 = MAX_REPLICATION_DOCUMENT_BYTES as u64 + 64 * 1024;
const MAX_REPLICATION_JOURNAL_BYTES: u64 = 16 * 1024;
const CURRENT_REPLICATION_REPOSITORY_FORMAT: u16 = 1;
const CURRENT_REPLICATION_JOURNAL_FORMAT: u16 = 1;

#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ReplicationRepositoryRevision(u64);

impl ReplicationRepositoryRevision {
    pub const INITIAL: Self = Self(1);

    pub fn new(value: u64) -> Result<Self, ReplicationStoreError> {
        if value == 0 {
            return Err(ReplicationStoreError::Corrupt);
        }
        Ok(Self(value))
    }

    pub fn get(self) -> u64 {
        self.0
    }

    fn next(self) -> Result<Self, ReplicationStoreError> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(ReplicationStoreError::RevisionOverflow)
    }
}

impl fmt::Debug for ReplicationRepositoryRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("ReplicationRepositoryRevision")
            .field(&self.0)
            .finish()
    }
}

impl<'de> Deserialize<'de> for ReplicationRepositoryRevision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = u64::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

impl Serialize for ReplicationRepositoryRevision {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_u64(self.0)
    }
}

#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ReplicationContentRevision([u8; 32]);

impl ReplicationContentRevision {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for ReplicationContentRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ReplicationContentRevision(<redacted>)")
    }
}

#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SharedFolderSlot(String);

impl SharedFolderSlot {
    pub const HEX_BYTES: usize = 64;

    pub fn new(value: impl Into<String>) -> Result<Self, ReplicationStoreError> {
        let value = value.into();
        if value.len() != Self::HEX_BYTES
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(ReplicationStoreError::InvalidTransportSlot);
        }
        Ok(Self(value))
    }

    fn file_component(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SharedFolderSlot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SharedFolderSlot(<redacted>)")
    }
}

#[derive(Clone)]
pub struct ReplicationCustodyMetadata {
    authority: ReplicationSecretRef,
    device: ReplicationSecretRef,
    historical: ReplicationHistoricalKeyIndex,
}

impl ReplicationCustodyMetadata {
    pub fn new(
        authority: ReplicationSecretRef,
        device: ReplicationSecretRef,
        historical: ReplicationHistoricalKeyIndex,
    ) -> Result<Self, ReplicationStoreError> {
        if authority.kind() != ReplicationSecretKind::AuthorityPrivateKey
            || authority.key_epoch().is_some()
            || device.kind() != ReplicationSecretKind::DevicePrivateKey
            || device.key_epoch().is_some()
        {
            return Err(ReplicationStoreError::InvalidCustodyMetadata);
        }
        Ok(Self {
            authority,
            device,
            historical,
        })
    }

    pub fn authority_reference(&self) -> &ReplicationSecretRef {
        &self.authority
    }

    pub fn device_reference(&self) -> &ReplicationSecretRef {
        &self.device
    }

    pub fn historical(&self) -> &ReplicationHistoricalKeyIndex {
        &self.historical
    }

    fn contains(&self, reference: &ReplicationSecretRef) -> bool {
        &self.authority == reference
            || &self.device == reference
            || self.historical.references().any(|item| item == reference)
    }
}

impl fmt::Debug for ReplicationCustodyMetadata {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplicationCustodyMetadata")
            .field("authority", &"<opaque>")
            .field("device", &"<opaque>")
            .field("historical", &self.historical)
            .finish()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredCustodyMetadata {
    #[serde(deserialize_with = "deserialize_secret_reference")]
    authority: Vec<u8>,
    #[serde(deserialize_with = "deserialize_secret_reference")]
    device: Vec<u8>,
    historical_limit: usize,
    #[serde(deserialize_with = "deserialize_secret_references")]
    epoch_references: Vec<Vec<u8>>,
}

impl StoredCustodyMetadata {
    fn from_metadata(metadata: &ReplicationCustodyMetadata) -> Self {
        Self {
            authority: metadata.authority.to_bytes().to_vec(),
            device: metadata.device.to_bytes().to_vec(),
            historical_limit: metadata.historical.limit().get(),
            epoch_references: metadata
                .historical
                .references()
                .map(|reference| reference.to_bytes().to_vec())
                .collect(),
        }
    }

    fn into_metadata(self) -> Result<ReplicationCustodyMetadata, ReplicationStoreError> {
        let authority = ReplicationSecretRef::from_bytes(&self.authority)?;
        let device = ReplicationSecretRef::from_bytes(&self.device)?;
        let limit = ReplicationHistoricalKeyLimit::new(self.historical_limit)?;
        let references = self
            .epoch_references
            .iter()
            .map(|bytes| ReplicationSecretRef::from_bytes(bytes))
            .collect::<Result<Vec<_>, _>>()?;
        let historical = ReplicationHistoricalKeyIndex::from_retained(limit, references)?;
        ReplicationCustodyMetadata::new(authority, device, historical)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredReplicationDocument {
    format_version: u16,
    revision: ReplicationRepositoryRevision,
    replication: ReplicationDocument,
    custody: StoredCustodyMetadata,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReplicationTransactionJournal {
    format_version: u16,
    committed_revision: ReplicationRepositoryRevision,
    #[serde(deserialize_with = "deserialize_secret_references")]
    retired_references: Vec<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplicationStoreError {
    Io {
        operation: &'static str,
        kind: io::ErrorKind,
    },
    UnsupportedPlatform,
    UnsafeEntry,
    Missing,
    AlreadyExists,
    TooLarge,
    Corrupt,
    Newer {
        found: u16,
        supported: u16,
    },
    WorkspaceMismatch,
    InvalidTransportSlot,
    InvalidCustodyMetadata,
    InvalidCustodyTransition,
    StaleRepositoryRevision {
        expected: ReplicationRepositoryRevision,
        actual: ReplicationRepositoryRevision,
    },
    StaleTransportRevision,
    RevisionOverflow,
    PendingRetirement,
    RetirementStillReferenced,
    TooManyRetirements,
    TooManyDirectoryEntries,
    TooManyConflictArtifacts,
    RecoveryNotRequired,
    RecoveryEvidenceExists,
    RecoveryRequired,
    StaleSyncPlan,
    ConflictResolutionRequired,
    InvalidConflictResolution,
    Domain(ReplicationError),
    Custody(ReplicationSecretCustodyError),
}

impl fmt::Display for ReplicationStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { operation, kind } => {
                write!(formatter, "replication store {operation} failed ({kind:?})")
            }
            Self::UnsupportedPlatform => {
                formatter.write_str("replication writer locks are unsupported on this platform")
            }
            Self::UnsafeEntry => {
                formatter.write_str("replication store entry is not a safe regular entry")
            }
            Self::Missing => formatter.write_str("replication store is missing"),
            Self::AlreadyExists => formatter.write_str("replication store already exists"),
            Self::TooLarge => formatter.write_str("replication store exceeds its byte limit"),
            Self::Corrupt => formatter.write_str("replication store is corrupt"),
            Self::Newer { found, supported } => write!(
                formatter,
                "replication store format {found} is newer than supported format {supported}"
            ),
            Self::WorkspaceMismatch => {
                formatter.write_str("replication store belongs to another workspace")
            }
            Self::InvalidTransportSlot => {
                formatter.write_str("replication transport slot is invalid")
            }
            Self::InvalidCustodyMetadata => {
                formatter.write_str("replication custody metadata is invalid")
            }
            Self::InvalidCustodyTransition => {
                formatter.write_str("replication custody transition is invalid")
            }
            Self::StaleRepositoryRevision { expected, actual } => write!(
                formatter,
                "replication repository revision is stale (expected {}, actual {})",
                expected.get(),
                actual.get()
            ),
            Self::StaleTransportRevision => {
                formatter.write_str("replication transport revision is stale")
            }
            Self::RevisionOverflow => {
                formatter.write_str("replication repository revision overflow")
            }
            Self::PendingRetirement => {
                formatter.write_str("replication secret retirement is pending")
            }
            Self::RetirementStillReferenced => {
                formatter.write_str("replication secret retirement is still referenced")
            }
            Self::TooManyRetirements => {
                formatter.write_str("replication retirement journal exceeds its item limit")
            }
            Self::TooManyDirectoryEntries => {
                formatter.write_str("replication shared folder exceeds its scan limit")
            }
            Self::TooManyConflictArtifacts => {
                formatter.write_str("replication conflict evidence exceeds its item limit")
            }
            Self::RecoveryNotRequired => {
                formatter.write_str("replication repository recovery is not required")
            }
            Self::RecoveryEvidenceExists => {
                formatter.write_str("replication recovery evidence already exists")
            }
            Self::RecoveryRequired => {
                formatter.write_str("replication repository requires explicit recovery")
            }
            Self::StaleSyncPlan => formatter.write_str("replication sync plan is stale"),
            Self::ConflictResolutionRequired => {
                formatter.write_str("replication conflicts require explicit resolution")
            }
            Self::InvalidConflictResolution => {
                formatter.write_str("replication conflict resolution is invalid")
            }
            Self::Domain(error) => error.fmt(formatter),
            Self::Custody(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ReplicationStoreError {}

impl From<ReplicationError> for ReplicationStoreError {
    fn from(error: ReplicationError) -> Self {
        Self::Domain(error)
    }
}

impl From<ReplicationSecretCustodyError> for ReplicationStoreError {
    fn from(error: ReplicationSecretCustodyError) -> Self {
        Self::Custody(error)
    }
}

fn canonical_replication(
    document: &ReplicationDocument,
    policy: &ReplicationPolicy,
) -> Result<(ReplicationDocument, Vec<u8>), ReplicationStoreError> {
    validate_replication_document(document, policy)?;
    let canonical = merge_replication_documents(document, document, policy)?.document;
    let bytes = serde_json::to_vec(&canonical).map_err(|_| ReplicationStoreError::Corrupt)?;
    if bytes.len() > MAX_REPLICATION_DOCUMENT_BYTES {
        return Err(ReplicationStoreError::TooLarge);
    }
    Ok((canonical, bytes))
}

fn validate_replication_document(
    document: &ReplicationDocument,
    policy: &ReplicationPolicy,
) -> Result<(), ReplicationStoreError> {
    if document.schema_version > REPLICATION_SCHEMA_VERSION {
        return Err(ReplicationStoreError::Newer {
            found: document.schema_version,
            supported: REPLICATION_SCHEMA_VERSION,
        });
    }
    if document.schema_version != REPLICATION_SCHEMA_VERSION {
        return Err(ReplicationStoreError::Corrupt);
    }
    document.validate(policy)?;
    Ok(())
}

fn validate_workspace(
    actual: &ReplicationWorkspaceId,
    expected: &ReplicationWorkspaceId,
) -> Result<(), ReplicationStoreError> {
    if actual != expected {
        return Err(ReplicationStoreError::WorkspaceMismatch);
    }
    Ok(())
}

fn decode_secret_references(
    encoded: &[Vec<u8>],
) -> Result<Vec<ReplicationSecretRef>, ReplicationStoreError> {
    if encoded.len() > termirust_replication_security::MAX_REPLICATION_RETAINED_EPOCH_KEYS {
        return Err(ReplicationStoreError::TooManyRetirements);
    }
    let references = encoded
        .iter()
        .map(|bytes| ReplicationSecretRef::from_bytes(bytes))
        .collect::<Result<Vec<_>, _>>()?;
    let unique = references.iter().collect::<BTreeSet<_>>();
    if unique.len() != references.len() {
        return Err(ReplicationStoreError::Corrupt);
    }
    Ok(references)
}

fn deserialize_secret_reference<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
    D: Deserializer<'de>,
{
    struct SecretReferenceVisitor;

    impl<'de> Visitor<'de> for SecretReferenceVisitor {
        type Value = Vec<u8>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a bounded encoded replication secret reference")
        }

        fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let limit = termirust_replication_security::REPLICATION_SECRET_REFERENCE_BYTES;
            let mut bytes = Vec::with_capacity(sequence.size_hint().unwrap_or(0).min(limit));
            while let Some(byte) = sequence.next_element::<u8>()? {
                if bytes.len() == limit {
                    return Err(serde::de::Error::custom(
                        "replication secret reference exceeds its byte limit",
                    ));
                }
                bytes.push(byte);
            }
            Ok(bytes)
        }
    }

    deserializer.deserialize_seq(SecretReferenceVisitor)
}

fn deserialize_secret_references<'de, D>(deserializer: D) -> Result<Vec<Vec<u8>>, D::Error>
where
    D: Deserializer<'de>,
{
    struct SecretReferenceSeed;

    impl<'de> DeserializeSeed<'de> for SecretReferenceSeed {
        type Value = Vec<u8>;

        fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserialize_secret_reference(deserializer)
        }
    }

    struct SecretReferencesVisitor;

    impl<'de> Visitor<'de> for SecretReferencesVisitor {
        type Value = Vec<Vec<u8>>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a bounded list of encoded replication secret references")
        }

        fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let limit = termirust_replication_security::MAX_REPLICATION_RETAINED_EPOCH_KEYS;
            let mut references = Vec::with_capacity(sequence.size_hint().unwrap_or(0).min(limit));
            while let Some(reference) = sequence.next_element_seed(SecretReferenceSeed)? {
                if references.len() == limit {
                    return Err(serde::de::Error::custom(
                        "replication secret reference list exceeds its item limit",
                    ));
                }
                references.push(reference);
            }
            Ok(references)
        }
    }

    deserializer.deserialize_seq(SecretReferencesVisitor)
}

fn io_error(operation: &'static str, error: io::Error) -> ReplicationStoreError {
    ReplicationStoreError::Io {
        operation,
        kind: error.kind(),
    }
}

fn reject_unsafe_file(path: &Path) -> Result<(), ReplicationStoreError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| io_error("inspect", error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(ReplicationStoreError::UnsafeEntry);
    }
    Ok(())
}

fn reject_unsafe_file_if_present(path: &Path) -> Result<bool, ReplicationStoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(ReplicationStoreError::UnsafeEntry)
        }
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_error("inspect", error)),
    }
}

fn read_bounded_regular_file(
    path: &Path,
    max_bytes: u64,
    operation: &'static str,
) -> Result<Vec<u8>, ReplicationStoreError> {
    reject_unsafe_file(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    let file = options.open(path).map_err(|error| {
        #[cfg(unix)]
        if error.raw_os_error() == Some(libc::ELOOP) {
            return ReplicationStoreError::UnsafeEntry;
        }
        io_error(operation, error)
    })?;
    let metadata = file
        .metadata()
        .map_err(|error| io_error(operation, error))?;
    if !metadata.is_file() {
        return Err(ReplicationStoreError::UnsafeEntry);
    }
    if metadata.len() > max_bytes {
        return Err(ReplicationStoreError::TooLarge);
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| io_error(operation, error))?;
    if bytes.len() as u64 > max_bytes {
        return Err(ReplicationStoreError::TooLarge);
    }
    Ok(bytes)
}

struct AdvisoryLock {
    _process_guard: MutexGuard<'static, ()>,
    file: File,
}

static REPLICATION_PROCESS_LOCK: Mutex<()> = Mutex::new(());

impl AdvisoryLock {
    fn acquire(path: &Path) -> Result<Self, ReplicationStoreError> {
        let process_guard = REPLICATION_PROCESS_LOCK
            .lock()
            .map_err(|_| ReplicationStoreError::Corrupt)?;
        reject_unsafe_file_if_present(path)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        #[cfg(windows)]
        options.custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT
        let file = options.open(path).map_err(|error| {
            #[cfg(unix)]
            if error.raw_os_error() == Some(libc::ELOOP) {
                return ReplicationStoreError::UnsafeEntry;
            }
            io_error("open lock", error)
        })?;
        if !file
            .metadata()
            .map_err(|error| io_error("inspect lock", error))?
            .is_file()
        {
            return Err(ReplicationStoreError::UnsafeEntry);
        }
        #[cfg(unix)]
        {
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|error| io_error("set lock permissions", error))?;
        }
        file.lock_exclusive()
            .map_err(|error| io_error("lock", error))?;
        Ok(Self {
            _process_guard: process_guard,
            file,
        })
    }
}

impl Drop for AdvisoryLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
    }
}
