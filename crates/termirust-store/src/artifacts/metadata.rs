use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::str::FromStr as _;

use sha2::{Digest as _, Sha256};
use termirust_domain::{
    ArtifactCancellation, ArtifactDisplayName, ArtifactError, ArtifactId, ArtifactMediaType,
    ArtifactMetadata, ArtifactOrigin, ArtifactPreviewKind, ArtifactScope, ArtifactSha256,
    ArtifactState, HostedSessionId,
};

use super::{
    ARTIFACTS_DIR, ArtifactPayload, ArtifactRepository, ArtifactSnapshot, ArtifactStoreError,
    DATA_FILE, MAX_METADATA_BYTES, METADATA_FILE, QUARANTINE_DIR, READY_DIR, STAGING_DIR,
    create_user_only_directory, io_error, open_regular_file,
};

#[derive(Clone)]
pub(super) struct StoredArtifact {
    pub metadata: ArtifactMetadata,
    pub directory: PathBuf,
    pub metadata_valid: bool,
}

#[derive(Default)]
pub(super) struct ArtifactUsage {
    pub global_bytes: u64,
    pub global_count: usize,
    pub session_bytes: HashMap<HostedSessionId, u64>,
    pub session_count: HashMap<HostedSessionId, usize>,
}

impl ArtifactUsage {
    pub fn bytes_for(&self, session_id: HostedSessionId) -> u64 {
        self.session_bytes.get(&session_id).copied().unwrap_or(0)
    }

    pub fn count_for(&self, session_id: HostedSessionId) -> usize {
        self.session_count.get(&session_id).copied().unwrap_or(0)
    }
}

impl ArtifactRepository {
    pub(super) fn snapshot_locked(
        &self,
        scope: ArtifactScope,
    ) -> Result<ArtifactSnapshot, ArtifactStoreError> {
        let mut stored = self.scan_session_locked(scope)?;
        stored.sort_by_key(|item| {
            (
                std::cmp::Reverse(item.metadata.created_at),
                item.metadata.id,
            )
        });
        let usage = self.scan_usage_locked()?;
        Ok(ArtifactSnapshot {
            scope,
            artifacts: stored.into_iter().map(|item| item.metadata).collect(),
            session_bytes: usage.bytes_for(scope.session_id),
            session_limit: self.limits.session_bytes,
            global_bytes: usage.global_bytes,
            global_limit: self.limits.global_bytes,
            durability: crate::Durability::Full,
        })
    }

    pub(super) fn read_payload_locked(
        &self,
        scope: ArtifactScope,
        id: ArtifactId,
        cancellation: &ArtifactCancellation,
        include_quarantined: bool,
    ) -> Result<ArtifactPayload, ArtifactStoreError> {
        cancellation.check()?;
        let stored = self
            .scan_session_locked(scope)?
            .into_iter()
            .find(|item| item.metadata.id == id)
            .ok_or(ArtifactError::Unavailable)?;
        if stored.metadata.state == ArtifactState::Corrupt
            || (!include_quarantined && stored.metadata.state != ArtifactState::Ready)
        {
            return Err(ArtifactError::InvalidState.into());
        }
        let data_path = stored.directory.join(DATA_FILE);
        let metadata = safe_file_metadata(&data_path, "data")?;
        if metadata.len() != stored.metadata.byte_len
            || metadata.len() > self.limits.item_bytes
            || metadata.len() > usize::MAX as u64
        {
            return Err(ArtifactError::Corrupt.into());
        }
        let mut file = open_regular_file(&data_path, "data")?;
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        let mut hasher = Sha256::new();
        let mut buffer = [0_u8; super::IO_CHUNK_BYTES];
        loop {
            cancellation.check()?;
            let read = file
                .read(&mut buffer)
                .map_err(|error| io_error("read data", error))?;
            if read == 0 {
                break;
            }
            if bytes.len().saturating_add(read) > self.limits.item_bytes as usize {
                return Err(ArtifactError::ItemQuotaExceeded.into());
            }
            hasher.update(&buffer[..read]);
            bytes.extend_from_slice(&buffer[..read]);
        }
        let digest = ArtifactSha256::new(hasher.finalize().into());
        if bytes.len() as u64 != stored.metadata.byte_len || digest != stored.metadata.sha256 {
            return Err(ArtifactError::Corrupt.into());
        }
        Ok(ArtifactPayload {
            metadata: stored.metadata,
            bytes,
        })
    }

    pub(super) fn scan_session_locked(
        &self,
        scope: ArtifactScope,
    ) -> Result<Vec<StoredArtifact>, ArtifactStoreError> {
        let root = self.artifact_root(scope);
        match fs::symlink_metadata(&root) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(ArtifactStoreError::UnsafeEntry {
                    entry: "session artifact directory",
                });
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(io_error("inspect artifacts", error)),
        }
        let mut artifacts = Vec::new();
        self.scan_bucket(
            scope,
            &root.join(READY_DIR),
            ArtifactState::Ready,
            &mut artifacts,
        )?;
        self.scan_bucket(
            scope,
            &root.join(QUARANTINE_DIR),
            ArtifactState::Quarantined,
            &mut artifacts,
        )?;
        if artifacts.len() > self.limits.artifacts_per_session {
            return Err(ArtifactError::CountQuotaExceeded.into());
        }
        let mut ids = HashSet::with_capacity(artifacts.len());
        if artifacts.iter().any(|item| !ids.insert(item.metadata.id)) {
            return Err(ArtifactStoreError::Corrupt {
                entry: "duplicate identity",
            });
        }
        Ok(artifacts)
    }

    fn scan_bucket(
        &self,
        scope: ArtifactScope,
        bucket: &Path,
        location_state: ArtifactState,
        artifacts: &mut Vec<StoredArtifact>,
    ) -> Result<(), ArtifactStoreError> {
        let entries = match fs::read_dir(bucket) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(io_error("read artifact bucket", error)),
        };
        for entry in entries {
            if artifacts.len() >= self.limits.artifacts_per_session {
                return Err(ArtifactError::CountQuotaExceeded.into());
            }
            let entry = entry.map_err(|error| io_error("read artifact entry", error))?;
            let id = entry
                .file_name()
                .to_str()
                .and_then(|name| ArtifactId::from_str(name).ok())
                .ok_or(ArtifactStoreError::UnsafeEntry {
                    entry: "artifact identity",
                })?;
            let directory = entry.path();
            let directory_metadata = fs::symlink_metadata(&directory)
                .map_err(|error| io_error("inspect artifact entry", error))?;
            if directory_metadata.file_type().is_symlink() || !directory_metadata.is_dir() {
                return Err(ArtifactStoreError::UnsafeEntry {
                    entry: "artifact entry",
                });
            }
            artifacts.push(self.read_stored_artifact(scope, id, directory, location_state)?);
        }
        Ok(())
    }

    fn read_stored_artifact(
        &self,
        scope: ArtifactScope,
        id: ArtifactId,
        directory: PathBuf,
        location_state: ArtifactState,
    ) -> Result<StoredArtifact, ArtifactStoreError> {
        let data_len = match safe_file_metadata(&directory.join(DATA_FILE), "data") {
            Ok(metadata) => metadata.len(),
            Err(ArtifactStoreError::Io {
                kind: std::io::ErrorKind::NotFound,
                ..
            }) => 0,
            Err(error) => return Err(error),
        };
        let parsed = read_metadata(&directory.join(METADATA_FILE));
        let (mut metadata, metadata_valid) = match parsed {
            Ok(metadata)
                if metadata.id == id
                    && metadata.scope == scope
                    && metadata.validate(self.limits).is_ok() =>
            {
                (metadata, true)
            }
            _ => (corrupt_metadata(id, scope, data_len), false),
        };
        if metadata_valid {
            metadata.state = location_state;
            if data_len != metadata.byte_len {
                metadata.state = ArtifactState::Corrupt;
            }
        }
        Ok(StoredArtifact {
            metadata,
            directory,
            metadata_valid,
        })
    }

    pub(super) fn scan_usage_locked(&self) -> Result<ArtifactUsage, ArtifactStoreError> {
        let mut usage = ArtifactUsage::default();
        let entries = fs::read_dir(&self.root).map_err(|error| io_error("read root", error))?;
        for entry in entries {
            let entry = entry.map_err(|error| io_error("read root entry", error))?;
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let Ok(session_id) = HostedSessionId::from_str(&name) else {
                continue;
            };
            let entry_metadata = fs::symlink_metadata(entry.path())
                .map_err(|error| io_error("inspect session artifact root", error))?;
            if entry_metadata.file_type().is_symlink() || !entry_metadata.is_dir() {
                return Err(ArtifactStoreError::UnsafeEntry {
                    entry: "session artifact root",
                });
            }
            let artifacts = self.scan_session_locked(ArtifactScope { session_id })?;
            let mut unique = HashSet::new();
            let mut session_bytes = 0_u64;
            for artifact in &artifacts {
                let count_bytes =
                    !artifact.metadata_valid || unique.insert(artifact.metadata.sha256);
                if count_bytes {
                    session_bytes = session_bytes
                        .checked_add(artifact.metadata.byte_len)
                        .ok_or(ArtifactError::GlobalQuotaExceeded)?;
                }
            }
            usage.global_bytes = usage
                .global_bytes
                .checked_add(session_bytes)
                .ok_or(ArtifactError::GlobalQuotaExceeded)?;
            usage.global_count = usage
                .global_count
                .checked_add(artifacts.len())
                .ok_or(ArtifactError::CountQuotaExceeded)?;
            usage.session_bytes.insert(session_id, session_bytes);
            usage.session_count.insert(session_id, artifacts.len());
            if usage.global_count > self.limits.global_artifacts {
                return Err(ArtifactError::CountQuotaExceeded.into());
            }
        }
        Ok(usage)
    }

    pub(super) fn prepare_session_directories(
        &self,
        scope: ArtifactScope,
    ) -> Result<(), ArtifactStoreError> {
        let session = self.session_root(scope);
        create_user_only_directory(&session, "session directory")?;
        let artifacts = self.artifact_root(scope);
        create_user_only_directory(&artifacts, "artifact directory")?;
        create_user_only_directory(&artifacts.join(READY_DIR), "ready directory")?;
        create_user_only_directory(&artifacts.join(QUARANTINE_DIR), "quarantine directory")?;
        create_user_only_directory(&artifacts.join(STAGING_DIR), "staging directory")?;
        Ok(())
    }

    pub(super) fn session_root(&self, scope: ArtifactScope) -> PathBuf {
        self.root.join(scope.session_id.to_string())
    }

    pub(super) fn artifact_root(&self, scope: ArtifactScope) -> PathBuf {
        self.session_root(scope).join(ARTIFACTS_DIR)
    }

    pub(super) fn bucket_path(&self, scope: ArtifactScope, bucket: &str) -> PathBuf {
        self.artifact_root(scope).join(bucket)
    }

    pub(super) fn find_stored_locked(
        &self,
        scope: ArtifactScope,
        id: ArtifactId,
    ) -> Result<StoredArtifact, ArtifactStoreError> {
        self.scan_session_locked(scope)?
            .into_iter()
            .find(|item| item.metadata.id == id)
            .ok_or_else(|| ArtifactError::Unavailable.into())
    }
}

pub(super) fn read_metadata(path: &Path) -> Result<ArtifactMetadata, ArtifactStoreError> {
    let file = open_regular_file(path, "metadata")?;
    let metadata = file
        .metadata()
        .map_err(|error| io_error("inspect metadata", error))?;
    if metadata.len() > MAX_METADATA_BYTES {
        return Err(ArtifactStoreError::TooLarge {
            entry: "metadata",
            limit: MAX_METADATA_BYTES,
        });
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_METADATA_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| io_error("read metadata", error))?;
    if bytes.len() as u64 > MAX_METADATA_BYTES {
        return Err(ArtifactStoreError::TooLarge {
            entry: "metadata",
            limit: MAX_METADATA_BYTES,
        });
    }
    serde_json::from_slice(&bytes).map_err(|_| ArtifactStoreError::Corrupt { entry: "metadata" })
}

pub(super) fn safe_file_metadata(
    path: &Path,
    entry: &'static str,
) -> Result<fs::Metadata, ArtifactStoreError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| io_error("inspect file", error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(ArtifactStoreError::UnsafeEntry { entry });
    }
    Ok(metadata)
}

pub(super) fn hash_file(
    path: &Path,
    limit: u64,
    cancellation: &ArtifactCancellation,
) -> Result<(ArtifactSha256, u64), ArtifactStoreError> {
    let mut file = open_regular_file(path, "data")?;
    let metadata = file
        .metadata()
        .map_err(|error| io_error("inspect data", error))?;
    if metadata.len() > limit {
        return Err(ArtifactError::ItemQuotaExceeded.into());
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|error| io_error("seek data", error))?;
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; super::IO_CHUNK_BYTES];
    loop {
        cancellation.check()?;
        let read = file
            .read(&mut buffer)
            .map_err(|error| io_error("read data", error))?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or(ArtifactError::ItemQuotaExceeded)?;
        if total > limit {
            return Err(ArtifactError::ItemQuotaExceeded.into());
        }
        hasher.update(&buffer[..read]);
    }
    Ok((ArtifactSha256::new(hasher.finalize().into()), total))
}

fn corrupt_metadata(id: ArtifactId, scope: ArtifactScope, byte_len: u64) -> ArtifactMetadata {
    ArtifactMetadata {
        id,
        scope,
        display_name: ArtifactDisplayName::default(),
        origin: ArtifactOrigin::ExplicitImport,
        media_type: ArtifactMediaType::MetadataOnly,
        byte_len,
        sha256: ArtifactSha256::new([0; 32]),
        created_at: 0,
        preview_kind: ArtifactPreviewKind::MetadataOnly,
        state: ArtifactState::Corrupt,
    }
}
