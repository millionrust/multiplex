use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use termirust_domain::{ReplicationDocument, ReplicationPolicy, ReplicationWorkspaceId};

use crate::{AtomicWriter, Durability, SystemAtomicWriter};

use super::{
    AdvisoryLock, ReplicationContentRevision, ReplicationStoreError, SharedFolderSlot,
    canonical_replication, io_error, read_bounded_regular_file, reject_unsafe_file_if_present,
    validate_replication_document, validate_workspace,
};

const MAX_SHARED_FOLDER_ENTRIES_SCANNED: usize = 1_024;
pub const MAX_REPLICATION_CONFLICT_ARTIFACTS: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SharedFolderTransportState {
    Absent,
    Present(ReplicationContentRevision),
}

#[derive(Clone, Eq, PartialEq)]
pub struct SharedFolderTransportSnapshot {
    pub document: ReplicationDocument,
    pub revision: ReplicationContentRevision,
    pub durability: Durability,
}

impl std::fmt::Debug for SharedFolderTransportSnapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SharedFolderTransportSnapshot")
            .field("entry_count", &self.document.entries.len())
            .field("revision", &self.revision)
            .field("durability", &self.durability)
            .finish()
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct SharedFolderConflictArtifact {
    path: PathBuf,
    pub document: ReplicationDocument,
    pub revision: ReplicationContentRevision,
}

impl SharedFolderConflictArtifact {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl std::fmt::Debug for SharedFolderConflictArtifact {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SharedFolderConflictArtifact")
            .field("path", &"<redacted>")
            .field("entry_count", &self.document.entries.len())
            .field("revision", &self.revision)
            .finish()
    }
}

#[derive(Clone)]
pub struct SharedFolderReplicationTransport {
    root: PathBuf,
    workspace_id: ReplicationWorkspaceId,
    slot: SharedFolderSlot,
    writer: Arc<dyn AtomicWriter>,
}

impl SharedFolderReplicationTransport {
    pub fn open(
        root: impl Into<PathBuf>,
        workspace_id: ReplicationWorkspaceId,
        slot: SharedFolderSlot,
    ) -> Result<Self, ReplicationStoreError> {
        Self::open_with(root, workspace_id, slot, Arc::new(SystemAtomicWriter))
    }

    pub fn open_with(
        root: impl Into<PathBuf>,
        workspace_id: ReplicationWorkspaceId,
        slot: SharedFolderSlot,
        writer: Arc<dyn AtomicWriter>,
    ) -> Result<Self, ReplicationStoreError> {
        workspace_id.validate()?;
        let root = root.into();
        let metadata =
            fs::symlink_metadata(&root).map_err(|error| io_error("inspect folder", error))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(ReplicationStoreError::UnsafeEntry);
        }
        Ok(Self {
            root,
            workspace_id,
            slot,
            writer,
        })
    }

    pub fn observe(
        &self,
        policy: &ReplicationPolicy,
    ) -> Result<SharedFolderTransportState, ReplicationStoreError> {
        let _lock = self.lock()?;
        self.read_current(policy).map(|snapshot| {
            snapshot.map_or(SharedFolderTransportState::Absent, |snapshot| {
                SharedFolderTransportState::Present(snapshot.revision)
            })
        })
    }

    pub fn pull(
        &self,
        policy: &ReplicationPolicy,
    ) -> Result<Option<SharedFolderTransportSnapshot>, ReplicationStoreError> {
        let _lock = self.lock()?;
        self.read_current(policy)
    }

    pub fn publish(
        &self,
        expected: SharedFolderTransportState,
        document: &ReplicationDocument,
        policy: &ReplicationPolicy,
    ) -> Result<SharedFolderTransportSnapshot, ReplicationStoreError> {
        let (document, bytes) = canonical_replication(document, policy)?;
        validate_workspace(&document.workspace_id, &self.workspace_id)?;
        let _lock = self.lock()?;
        let actual = self
            .read_current(policy)?
            .map_or(SharedFolderTransportState::Absent, |snapshot| {
                SharedFolderTransportState::Present(snapshot.revision)
            });
        if actual != expected {
            return Err(ReplicationStoreError::StaleTransportRevision);
        }
        let durability = self
            .writer
            .write(&self.artifact_path(), &bytes)
            .map_err(|error| io_error("publish transport", error))?;
        Ok(SharedFolderTransportSnapshot {
            revision: ReplicationContentRevision::from_bytes(&bytes),
            document,
            durability,
        })
    }

    pub fn conflict_artifacts(
        &self,
        policy: &ReplicationPolicy,
    ) -> Result<Vec<SharedFolderConflictArtifact>, ReplicationStoreError> {
        let _lock = self.lock()?;
        let mut matches = Vec::new();
        let target_name = self.artifact_file_name();
        let target_stem = target_name.strip_suffix(".json").unwrap_or(&target_name);
        let entries = fs::read_dir(&self.root).map_err(|error| io_error("scan folder", error))?;
        for (index, entry) in entries.enumerate() {
            if index >= MAX_SHARED_FOLDER_ENTRIES_SCANNED {
                return Err(ReplicationStoreError::TooManyDirectoryEntries);
            }
            let entry = entry.map_err(|error| io_error("scan folder entry", error))?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if !is_conflict_name(name, target_stem) {
                continue;
            }
            if matches.len() == MAX_REPLICATION_CONFLICT_ARTIFACTS {
                return Err(ReplicationStoreError::TooManyConflictArtifacts);
            }
            let path = entry.path();
            let snapshot = self.read_artifact(&path, policy)?;
            matches.push(SharedFolderConflictArtifact {
                path,
                document: snapshot.document,
                revision: snapshot.revision,
            });
        }
        matches.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(matches)
    }

    pub fn artifact_path(&self) -> PathBuf {
        self.root.join(self.artifact_file_name())
    }

    fn artifact_file_name(&self) -> String {
        format!(".termirust-replica-{}.json", self.slot.file_component())
    }

    fn lock(&self) -> Result<AdvisoryLock, ReplicationStoreError> {
        AdvisoryLock::acquire(&self.root.join(format!(
            ".termirust-replica-{}.lock",
            self.slot.file_component()
        )))
    }

    fn read_current(
        &self,
        policy: &ReplicationPolicy,
    ) -> Result<Option<SharedFolderTransportSnapshot>, ReplicationStoreError> {
        if !reject_unsafe_file_if_present(&self.artifact_path())? {
            return Ok(None);
        }
        self.read_artifact(&self.artifact_path(), policy).map(Some)
    }

    fn read_artifact(
        &self,
        path: &Path,
        policy: &ReplicationPolicy,
    ) -> Result<SharedFolderTransportSnapshot, ReplicationStoreError> {
        let bytes = read_bounded_regular_file(
            path,
            termirust_domain::MAX_REPLICATION_DOCUMENT_BYTES as u64,
            "read transport",
        )?;
        let document: ReplicationDocument =
            serde_json::from_slice(&bytes).map_err(|_| ReplicationStoreError::Corrupt)?;
        validate_replication_document(&document, policy)?;
        validate_workspace(&document.workspace_id, &self.workspace_id)?;
        let (document, canonical) = canonical_replication(&document, policy)?;
        if bytes != canonical {
            return Err(ReplicationStoreError::Corrupt);
        }
        Ok(SharedFolderTransportSnapshot {
            revision: ReplicationContentRevision::from_bytes(&bytes),
            document,
            durability: Durability::Full,
        })
    }
}

fn is_conflict_name(name: &str, target_stem: &str) -> bool {
    name.starts_with(target_stem)
        && name.ends_with(".json")
        && (name.contains(".sync-conflict-")
            || name.contains(".conflict-")
            || name.contains("conflicted copy"))
}
