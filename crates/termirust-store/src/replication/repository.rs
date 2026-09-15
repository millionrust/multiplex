use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use termirust_domain::{ReplicationDocument, ReplicationPolicy, ReplicationWorkspaceId};
use termirust_replication_security::{ReplicationSecretBackend, ReplicationSecretRef};

use crate::{AtomicWriter, Durability, SystemAtomicWriter};

use super::{
    AdvisoryLock, CURRENT_REPLICATION_JOURNAL_FORMAT, CURRENT_REPLICATION_REPOSITORY_FORMAT,
    MAX_REPLICATION_JOURNAL_BYTES, MAX_REPLICATION_REPOSITORY_BYTES, ReplicationCustodyMetadata,
    ReplicationRepositoryRevision, ReplicationStoreError, ReplicationTransactionJournal,
    StoredCustodyMetadata, StoredReplicationDocument, canonical_replication,
    decode_secret_references, io_error, read_bounded_regular_file, reject_unsafe_file,
    reject_unsafe_file_if_present, validate_replication_document, validate_workspace,
};

const REPOSITORY_FILE: &str = "replica.json";
const LAST_GOOD_FILE: &str = "replica.last-good.json";
const TRANSACTION_FILE: &str = "replica.transaction.json";
const RECOVERY_EVIDENCE_FILE: &str = "replica.recovery-evidence.json";
const LOCK_FILE: &str = "replica.lock";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplicationRepositorySource {
    Primary,
    LastGood,
}

#[derive(Clone)]
pub struct ReplicationRepositorySnapshot {
    pub revision: ReplicationRepositoryRevision,
    pub document: ReplicationDocument,
    pub custody: ReplicationCustodyMetadata,
    pub source: ReplicationRepositorySource,
    pub durability: Durability,
    pub retirement_pending: bool,
}

impl std::fmt::Debug for ReplicationRepositorySnapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ReplicationRepositorySnapshot")
            .field("revision", &self.revision)
            .field("entry_count", &self.document.entries.len())
            .field("custody", &self.custody)
            .field("source", &self.source)
            .field("durability", &self.durability)
            .field("retirement_pending", &self.retirement_pending)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplicationRetirementOutcome {
    NothingPending,
    AbandonedUncommitted {
        reference_count: usize,
        durability: Durability,
    },
    Completed {
        deleted: usize,
        already_missing: usize,
        durability: Durability,
    },
}

#[derive(Clone)]
pub struct ReplicationRecoveryOutcome {
    pub snapshot: ReplicationRepositorySnapshot,
    pub evidence_durability: Durability,
}

impl std::fmt::Debug for ReplicationRecoveryOutcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ReplicationRecoveryOutcome")
            .field("snapshot", &self.snapshot)
            .field("evidence_durability", &self.evidence_durability)
            .finish()
    }
}

#[derive(Clone)]
pub struct ReplicationRepository {
    root: PathBuf,
    writer: Arc<dyn AtomicWriter>,
}

struct ParsedRepository {
    document: StoredReplicationDocument,
    bytes: Vec<u8>,
}

#[derive(Deserialize)]
struct FormatProbe {
    format_version: u16,
}

impl ReplicationRepository {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, ReplicationStoreError> {
        Self::open_with(root, Arc::new(SystemAtomicWriter))
    }

    pub fn open_with(
        root: impl Into<PathBuf>,
        writer: Arc<dyn AtomicWriter>,
    ) -> Result<Self, ReplicationStoreError> {
        let repository = Self {
            root: root.into(),
            writer,
        };
        repository.ensure_root()?;
        let _lock = repository.lock()?;
        Ok(repository)
    }

    pub fn create(
        &self,
        document: ReplicationDocument,
        custody: ReplicationCustodyMetadata,
        policy: &ReplicationPolicy,
    ) -> Result<ReplicationRepositorySnapshot, ReplicationStoreError> {
        let _lock = self.lock()?;
        if reject_unsafe_file_if_present(&self.primary_path())? {
            return Err(ReplicationStoreError::AlreadyExists);
        }
        let (document, _) = canonical_replication(&document, policy)?;
        let stored = StoredReplicationDocument {
            format_version: CURRENT_REPLICATION_REPOSITORY_FORMAT,
            revision: ReplicationRepositoryRevision::INITIAL,
            replication: document,
            custody: StoredCustodyMetadata::from_metadata(&custody),
        };
        let durability = self.write_repository(&self.primary_path(), &stored)?;
        snapshot(
            stored,
            ReplicationRepositorySource::Primary,
            durability,
            false,
        )
    }

    pub fn load(
        &self,
        expected_workspace: &ReplicationWorkspaceId,
        policy: &ReplicationPolicy,
    ) -> Result<ReplicationRepositorySnapshot, ReplicationStoreError> {
        let _lock = self.lock()?;
        match self.read_repository(&self.primary_path(), expected_workspace, policy) {
            Ok(parsed) => snapshot(
                parsed.document,
                ReplicationRepositorySource::Primary,
                Durability::Full,
                reject_unsafe_file_if_present(&self.transaction_path())?,
            ),
            Err(
                primary_error @ (ReplicationStoreError::Corrupt | ReplicationStoreError::TooLarge),
            ) => match self.read_repository(&self.last_good_path(), expected_workspace, policy) {
                Ok(backup) => snapshot(
                    backup.document,
                    ReplicationRepositorySource::LastGood,
                    Durability::Full,
                    false,
                ),
                Err(
                    ReplicationStoreError::Missing
                    | ReplicationStoreError::Corrupt
                    | ReplicationStoreError::TooLarge,
                ) => Err(primary_error),
                Err(error) => Err(error),
            },
            Err(error) => Err(error),
        }
    }

    pub fn commit(
        &self,
        expected_revision: ReplicationRepositoryRevision,
        document: ReplicationDocument,
        custody: ReplicationCustodyMetadata,
        retired_references: &[ReplicationSecretRef],
        policy: &ReplicationPolicy,
    ) -> Result<ReplicationRepositorySnapshot, ReplicationStoreError> {
        validate_retirements(&custody, retired_references)?;
        let _lock = self.lock()?;
        if reject_unsafe_file_if_present(&self.transaction_path())? {
            return Err(ReplicationStoreError::PendingRetirement);
        }
        let current = self.read_repository(&self.primary_path(), &document.workspace_id, policy)?;
        if current.document.revision != expected_revision {
            return Err(ReplicationStoreError::StaleRepositoryRevision {
                expected: expected_revision,
                actual: current.document.revision,
            });
        }
        let current_custody = current.document.custody.clone().into_metadata()?;
        validate_custody_transition(&current_custody, &custody, retired_references)?;
        validate_workspace(
            &current.document.replication.workspace_id,
            &document.workspace_id,
        )?;
        let (document, _) = canonical_replication(&document, policy)?;
        let next = StoredReplicationDocument {
            format_version: CURRENT_REPLICATION_REPOSITORY_FORMAT,
            revision: current.document.revision.next()?,
            replication: document,
            custody: StoredCustodyMetadata::from_metadata(&custody),
        };

        self.writer
            .write(&self.last_good_path(), &current.bytes)
            .map_err(|error| io_error("write last-good", error))?;
        if !retired_references.is_empty() {
            let mut retired_references = retired_references.to_vec();
            retired_references.sort();
            let journal = ReplicationTransactionJournal {
                format_version: CURRENT_REPLICATION_JOURNAL_FORMAT,
                committed_revision: next.revision,
                retired_references: retired_references
                    .iter()
                    .map(|reference| reference.to_bytes().to_vec())
                    .collect(),
            };
            self.write_journal(&journal)?;
        }
        let durability = self.write_repository(&self.primary_path(), &next)?;
        snapshot(
            next,
            ReplicationRepositorySource::Primary,
            durability,
            !retired_references.is_empty(),
        )
    }

    pub fn retire_pending<B: ReplicationSecretBackend>(
        &self,
        backend: &B,
        expected_workspace: &ReplicationWorkspaceId,
        policy: &ReplicationPolicy,
    ) -> Result<ReplicationRetirementOutcome, ReplicationStoreError> {
        let _lock = self.lock()?;
        if !reject_unsafe_file_if_present(&self.transaction_path())? {
            return Ok(ReplicationRetirementOutcome::NothingPending);
        }
        let journal = self.read_journal()?;
        let references = decode_secret_references(&journal.retired_references)?;
        let current = self.read_repository(&self.primary_path(), expected_workspace, policy)?;
        if current.document.revision < journal.committed_revision {
            let durability = self.remove_journal()?;
            return Ok(ReplicationRetirementOutcome::AbandonedUncommitted {
                reference_count: references.len(),
                durability,
            });
        }
        let custody = current.document.custody.clone().into_metadata()?;
        if references
            .iter()
            .any(|reference| custody.contains(reference))
        {
            return Err(ReplicationStoreError::RetirementStillReferenced);
        }

        let mut deleted = 0_usize;
        let mut already_missing = 0_usize;
        for reference in &references {
            match backend
                .delete(reference)
                .map_err(termirust_replication_security::ReplicationSecretCustodyError::from)?
            {
                true => deleted += 1,
                false => already_missing += 1,
            }
        }
        let durability = self.remove_journal()?;
        Ok(ReplicationRetirementOutcome::Completed {
            deleted,
            already_missing,
            durability,
        })
    }

    pub fn recover_last_good(
        &self,
        expected_workspace: &ReplicationWorkspaceId,
        policy: &ReplicationPolicy,
    ) -> Result<ReplicationRecoveryOutcome, ReplicationStoreError> {
        let _lock = self.lock()?;
        match self.read_repository(&self.primary_path(), expected_workspace, policy) {
            Ok(_) => return Err(ReplicationStoreError::RecoveryNotRequired),
            Err(ReplicationStoreError::Corrupt | ReplicationStoreError::TooLarge) => {}
            Err(error) => return Err(error),
        }
        let last_good = self.read_repository(&self.last_good_path(), expected_workspace, policy)?;
        let retirement_pending = reject_unsafe_file_if_present(&self.transaction_path())?;
        let evidence_path = self.recovery_evidence_path();
        if reject_unsafe_file_if_present(&evidence_path)? {
            if !same_file(&self.primary_path(), &evidence_path)? {
                return Err(ReplicationStoreError::RecoveryEvidenceExists);
            }
        } else {
            fs::hard_link(self.primary_path(), &evidence_path)
                .map_err(|error| io_error("preserve recovery evidence", error))?;
            #[cfg(unix)]
            if let Err(error) =
                fs::set_permissions(&evidence_path, fs::Permissions::from_mode(0o600))
            {
                let _ = fs::remove_file(&evidence_path);
                return Err(io_error("set recovery evidence permissions", error));
            }
        }
        let evidence_durability = self.sync_root("sync recovery evidence directory")?;
        let durability = self.write_repository(&self.primary_path(), &last_good.document)?;
        Ok(ReplicationRecoveryOutcome {
            snapshot: snapshot(
                last_good.document,
                ReplicationRepositorySource::Primary,
                durability,
                retirement_pending,
            )?,
            evidence_durability,
        })
    }

    pub fn metadata_path(&self) -> PathBuf {
        self.primary_path()
    }

    pub fn transaction_path(&self) -> PathBuf {
        self.root.join(TRANSACTION_FILE)
    }

    pub fn recovery_evidence_path(&self) -> PathBuf {
        self.root.join(RECOVERY_EVIDENCE_FILE)
    }

    fn ensure_root(&self) -> Result<(), ReplicationStoreError> {
        match fs::symlink_metadata(&self.root) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(ReplicationStoreError::UnsafeEntry);
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir_all(&self.root).map_err(|error| io_error("create root", error))?;
            }
            Err(error) => return Err(io_error("inspect root", error)),
        }
        #[cfg(unix)]
        fs::set_permissions(&self.root, fs::Permissions::from_mode(0o700))
            .map_err(|error| io_error("set root permissions", error))?;
        Ok(())
    }

    fn lock(&self) -> Result<AdvisoryLock, ReplicationStoreError> {
        AdvisoryLock::acquire(&self.root.join(LOCK_FILE))
    }

    fn primary_path(&self) -> PathBuf {
        self.root.join(REPOSITORY_FILE)
    }

    fn last_good_path(&self) -> PathBuf {
        self.root.join(LAST_GOOD_FILE)
    }

    fn read_repository(
        &self,
        path: &Path,
        expected_workspace: &ReplicationWorkspaceId,
        policy: &ReplicationPolicy,
    ) -> Result<ParsedRepository, ReplicationStoreError> {
        reject_unsafe_file(path).map_err(|error| match error {
            ReplicationStoreError::Io {
                kind: io::ErrorKind::NotFound,
                ..
            } => ReplicationStoreError::Missing,
            other => other,
        })?;
        let bytes =
            read_bounded_regular_file(path, MAX_REPLICATION_REPOSITORY_BYTES, "read repository")?;
        let document: StoredReplicationDocument = match serde_json::from_slice(&bytes) {
            Ok(document) => document,
            Err(_) => {
                if let Ok(probe) = serde_json::from_slice::<FormatProbe>(&bytes)
                    && probe.format_version > CURRENT_REPLICATION_REPOSITORY_FORMAT
                {
                    return Err(ReplicationStoreError::Newer {
                        found: probe.format_version,
                        supported: CURRENT_REPLICATION_REPOSITORY_FORMAT,
                    });
                }
                return Err(ReplicationStoreError::Corrupt);
            }
        };
        if document.format_version > CURRENT_REPLICATION_REPOSITORY_FORMAT {
            return Err(ReplicationStoreError::Newer {
                found: document.format_version,
                supported: CURRENT_REPLICATION_REPOSITORY_FORMAT,
            });
        }
        if document.format_version != CURRENT_REPLICATION_REPOSITORY_FORMAT {
            return Err(ReplicationStoreError::Corrupt);
        }
        validate_replication_document(&document.replication, policy)?;
        validate_workspace(&document.replication.workspace_id, expected_workspace)?;
        document.custody.clone().into_metadata()?;
        let (_, canonical) = canonical_replication(&document.replication, policy)?;
        let stored_canonical = serde_json::to_vec(&document.replication)
            .map_err(|_| ReplicationStoreError::Corrupt)?;
        if stored_canonical != canonical {
            return Err(ReplicationStoreError::Corrupt);
        }
        Ok(ParsedRepository { document, bytes })
    }

    fn write_repository(
        &self,
        path: &Path,
        document: &StoredReplicationDocument,
    ) -> Result<Durability, ReplicationStoreError> {
        let bytes = serde_json::to_vec(document).map_err(|_| ReplicationStoreError::Corrupt)?;
        if bytes.len() as u64 > MAX_REPLICATION_REPOSITORY_BYTES {
            return Err(ReplicationStoreError::TooLarge);
        }
        self.writer
            .write(path, &bytes)
            .map_err(|error| io_error("write repository", error))
    }

    fn write_journal(
        &self,
        journal: &ReplicationTransactionJournal,
    ) -> Result<Durability, ReplicationStoreError> {
        let bytes = serde_json::to_vec(journal).map_err(|_| ReplicationStoreError::Corrupt)?;
        if bytes.len() as u64 > MAX_REPLICATION_JOURNAL_BYTES {
            return Err(ReplicationStoreError::TooLarge);
        }
        self.writer
            .write(&self.transaction_path(), &bytes)
            .map_err(|error| io_error("write transaction", error))
    }

    fn read_journal(&self) -> Result<ReplicationTransactionJournal, ReplicationStoreError> {
        let bytes = read_bounded_regular_file(
            &self.transaction_path(),
            MAX_REPLICATION_JOURNAL_BYTES,
            "read transaction",
        )?;
        let journal: ReplicationTransactionJournal =
            serde_json::from_slice(&bytes).map_err(|_| ReplicationStoreError::Corrupt)?;
        if journal.format_version > CURRENT_REPLICATION_JOURNAL_FORMAT {
            return Err(ReplicationStoreError::Newer {
                found: journal.format_version,
                supported: CURRENT_REPLICATION_JOURNAL_FORMAT,
            });
        }
        if journal.format_version != CURRENT_REPLICATION_JOURNAL_FORMAT {
            return Err(ReplicationStoreError::Corrupt);
        }
        decode_secret_references(&journal.retired_references)?;
        Ok(journal)
    }

    fn remove_journal(&self) -> Result<Durability, ReplicationStoreError> {
        fs::remove_file(self.transaction_path())
            .map_err(|error| io_error("remove transaction", error))?;
        self.sync_root("sync transaction directory")
    }

    fn sync_root(&self, operation: &'static str) -> Result<Durability, ReplicationStoreError> {
        match File::open(&self.root).and_then(|directory| directory.sync_all()) {
            Ok(()) => Ok(Durability::Full),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::Unsupported
                        | io::ErrorKind::InvalidInput
                        | io::ErrorKind::PermissionDenied
                ) =>
            {
                Ok(Durability::RenameOnly)
            }
            Err(error) => Err(io_error(operation, error)),
        }
    }
}

#[cfg(unix)]
fn same_file(left: &Path, right: &Path) -> Result<bool, ReplicationStoreError> {
    let left = fs::metadata(left).map_err(|error| io_error("inspect recovery source", error))?;
    let right =
        fs::metadata(right).map_err(|error| io_error("inspect recovery evidence", error))?;
    Ok(left.dev() == right.dev() && left.ino() == right.ino())
}

#[cfg(all(not(unix), not(windows)))]
fn same_file(_left: &Path, _right: &Path) -> Result<bool, ReplicationStoreError> {
    Err(ReplicationStoreError::UnsupportedPlatform)
}

#[cfg(windows)]
fn same_file(left: &Path, right: &Path) -> Result<bool, ReplicationStoreError> {
    same_file::is_same_file(left, right)
        .map_err(|error| io_error("compare recovery evidence", error))
}

fn snapshot(
    stored: StoredReplicationDocument,
    source: ReplicationRepositorySource,
    durability: Durability,
    retirement_pending: bool,
) -> Result<ReplicationRepositorySnapshot, ReplicationStoreError> {
    Ok(ReplicationRepositorySnapshot {
        revision: stored.revision,
        document: stored.replication,
        custody: stored.custody.into_metadata()?,
        source,
        durability,
        retirement_pending,
    })
}

fn validate_retirements(
    custody: &ReplicationCustodyMetadata,
    references: &[ReplicationSecretRef],
) -> Result<(), ReplicationStoreError> {
    if references.len() > termirust_replication_security::MAX_REPLICATION_RETAINED_EPOCH_KEYS {
        return Err(ReplicationStoreError::TooManyRetirements);
    }
    let mut unique = std::collections::BTreeSet::new();
    for reference in references {
        if reference.kind() != termirust_replication_security::ReplicationSecretKind::EpochKey {
            return Err(ReplicationStoreError::InvalidCustodyTransition);
        }
        if !unique.insert(reference) {
            return Err(ReplicationStoreError::Corrupt);
        }
        if custody.contains(reference) {
            return Err(ReplicationStoreError::RetirementStillReferenced);
        }
    }
    Ok(())
}

fn validate_custody_transition(
    current: &ReplicationCustodyMetadata,
    next: &ReplicationCustodyMetadata,
    retired: &[ReplicationSecretRef],
) -> Result<(), ReplicationStoreError> {
    if current.authority_reference() != next.authority_reference()
        || current.device_reference() != next.device_reference()
    {
        return Err(ReplicationStoreError::InvalidCustodyTransition);
    }
    let current_epochs = current
        .historical()
        .references()
        .collect::<std::collections::BTreeSet<_>>();
    let next_epochs = next
        .historical()
        .references()
        .collect::<std::collections::BTreeSet<_>>();
    let removed = current_epochs
        .difference(&next_epochs)
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    let declared = retired.iter().collect::<std::collections::BTreeSet<_>>();
    if removed != declared {
        return Err(ReplicationStoreError::InvalidCustodyTransition);
    }
    Ok(())
}
