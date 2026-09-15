use std::fs::{self, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::UNIX_EPOCH;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt as _;

use sha2::{Digest as _, Sha256};
use termirust_domain::{
    ArtifactCancellation, ArtifactError, ArtifactId, ArtifactMetadata, ArtifactScope,
    ArtifactSha256, ArtifactState,
};

use super::metadata::{hash_file, read_metadata, safe_file_metadata};
use super::{
    ArtifactRepository, ArtifactStoreError, ArtifactSweepResult, DATA_FILE, IO_CHUNK_BYTES,
    MAX_METADATA_BYTES, METADATA_FILE, QUARANTINE_DIR, READY_DIR, STAGING_DIR, io_error,
    open_regular_file, sync_directory,
};

impl ArtifactRepository {
    pub fn quarantine(
        &self,
        scope: ArtifactScope,
        id: ArtifactId,
    ) -> Result<ArtifactMetadata, ArtifactStoreError> {
        let _lock = self.acquire_lock()?;
        self.transition_bucket(
            scope,
            id,
            READY_DIR,
            QUARANTINE_DIR,
            ArtifactState::Quarantined,
        )
    }

    pub fn restore(
        &self,
        scope: ArtifactScope,
        id: ArtifactId,
        cancellation: &ArtifactCancellation,
    ) -> Result<ArtifactMetadata, ArtifactStoreError> {
        let _lock = self.acquire_lock()?;
        let source = self.bucket_path(scope, QUARANTINE_DIR).join(id.to_string());
        let metadata = read_metadata(&source.join(METADATA_FILE))?;
        metadata.validate(self.limits)?;
        let (digest, length) = hash_file(
            &source.join(DATA_FILE),
            self.limits.item_bytes,
            cancellation,
        )?;
        if digest != metadata.sha256 || length != metadata.byte_len {
            return Err(ArtifactError::Corrupt.into());
        }
        self.transition_bucket(scope, id, QUARANTINE_DIR, READY_DIR, ArtifactState::Ready)
    }

    pub fn purge(
        &self,
        scope: ArtifactScope,
        id: ArtifactId,
        cancellation: &ArtifactCancellation,
    ) -> Result<(), ArtifactStoreError> {
        let _lock = self.acquire_lock()?;
        cancellation.check()?;
        let directory = self.bucket_path(scope, QUARANTINE_DIR).join(id.to_string());
        validate_owned_artifact_directory(&directory, id, scope, self.limits)?;
        // Cancellation is deliberately not checked after destructive deletion begins.
        fs::remove_dir_all(&directory).map_err(|error| io_error("purge artifact", error))?;
        let _ = sync_directory(directory.parent().ok_or(ArtifactStoreError::UnsafeEntry {
            entry: "quarantine directory",
        })?);
        Ok(())
    }

    pub fn export_copy(
        &self,
        scope: ArtifactScope,
        id: ArtifactId,
        destination: &Path,
        cancellation: &ArtifactCancellation,
    ) -> Result<(), ArtifactStoreError> {
        let _lock = self.acquire_lock()?;
        cancellation.check()?;
        if fs::symlink_metadata(destination).is_ok() {
            return Err(ArtifactError::Conflict.into());
        }
        let parent = destination
            .parent()
            .ok_or(ArtifactStoreError::UnsafeEntry {
                entry: "export destination",
            })?;
        let parent_metadata = fs::symlink_metadata(parent)
            .map_err(|error| io_error("inspect export directory", error))?;
        if parent_metadata.file_type().is_symlink() || !parent_metadata.is_dir() {
            return Err(ArtifactStoreError::UnsafeEntry {
                entry: "export directory",
            });
        }
        let stored = self.find_stored_locked(scope, id)?;
        if stored.metadata.state == ArtifactState::Corrupt {
            return Err(ArtifactError::Corrupt.into());
        }
        let source_path = stored.directory.join(DATA_FILE);
        safe_file_metadata(&source_path, "data")?;

        static NEXT_EXPORT: AtomicU64 = AtomicU64::new(1);
        let nonce = NEXT_EXPORT.fetch_add(1, Ordering::Relaxed);
        let temporary = parent.join(format!(
            ".termirust-export-{}-{}-{nonce}",
            std::process::id(),
            id
        ));
        let result = (|| {
            let mut source = open_regular_file(&source_path, "export source")?;
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            options.mode(0o600);
            let mut output = options
                .open(&temporary)
                .map_err(|error| io_error("create export staging", error))?;
            let mut hasher = Sha256::new();
            let mut written = 0_u64;
            let mut buffer = [0_u8; IO_CHUNK_BYTES];
            loop {
                cancellation.check()?;
                let read = source
                    .read(&mut buffer)
                    .map_err(|error| io_error("read export source", error))?;
                if read == 0 {
                    break;
                }
                written = written
                    .checked_add(read as u64)
                    .ok_or(ArtifactError::ItemQuotaExceeded)?;
                if written > self.limits.item_bytes || written > stored.metadata.byte_len {
                    return Err(ArtifactError::Corrupt.into());
                }
                hasher.update(&buffer[..read]);
                output
                    .write_all(&buffer[..read])
                    .map_err(|error| io_error("write export staging", error))?;
            }
            output
                .sync_all()
                .map_err(|error| io_error("sync export staging", error))?;
            drop(output);
            let digest = ArtifactSha256::new(hasher.finalize().into());
            if written != stored.metadata.byte_len || digest != stored.metadata.sha256 {
                return Err(ArtifactError::Corrupt.into());
            }
            fs::hard_link(&temporary, destination)
                .map_err(|error| io_error("publish export", error))?;
            // Publishing the no-overwrite hard link is the commit point. Cleanup and
            // directory sync are best-effort after that point so retries cannot collide
            // with a copy that was already verified and published.
            let _ = fs::remove_file(&temporary);
            let _ = sync_directory(parent);
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    pub fn sweep_staging(
        &self,
        now_millis: u64,
    ) -> Result<ArtifactSweepResult, ArtifactStoreError> {
        let _lock = self.acquire_lock()?;
        let mut result = ArtifactSweepResult {
            removed_entries: 0,
            removed_bytes: 0,
        };
        let sessions = fs::read_dir(&self.root).map_err(|error| io_error("read root", error))?;
        for session in sessions {
            let session = session.map_err(|error| io_error("read root entry", error))?;
            let Some(name) = session.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let Ok(session_id) = name.parse() else {
                continue;
            };
            let staging = self.bucket_path(ArtifactScope { session_id }, STAGING_DIR);
            let entries = match fs::read_dir(&staging) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(io_error("read staging", error)),
            };
            for entry in entries {
                let entry = entry.map_err(|error| io_error("read staging entry", error))?;
                if entry
                    .file_name()
                    .to_str()
                    .and_then(|value| value.parse::<ArtifactId>().ok())
                    .is_none()
                {
                    return Err(ArtifactStoreError::UnsafeEntry {
                        entry: "staging identity",
                    });
                }
                let metadata = fs::symlink_metadata(entry.path())
                    .map_err(|error| io_error("inspect staging entry", error))?;
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(ArtifactStoreError::UnsafeEntry {
                        entry: "staging entry",
                    });
                }
                let modified = metadata
                    .modified()
                    .ok()
                    .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                    .map(|value| value.as_millis() as u64)
                    .unwrap_or(now_millis);
                if now_millis.saturating_sub(modified) < super::STAGING_RETENTION_MILLIS {
                    continue;
                }
                result.removed_bytes = result
                    .removed_bytes
                    .saturating_add(directory_regular_bytes(&entry.path())?);
                fs::remove_dir_all(entry.path())
                    .map_err(|error| io_error("sweep staging entry", error))?;
                result.removed_entries = result.removed_entries.saturating_add(1);
            }
        }
        Ok(result)
    }

    fn transition_bucket(
        &self,
        scope: ArtifactScope,
        id: ArtifactId,
        from: &str,
        to: &str,
        state: ArtifactState,
    ) -> Result<ArtifactMetadata, ArtifactStoreError> {
        let source = self.bucket_path(scope, from).join(id.to_string());
        let destination = self.bucket_path(scope, to).join(id.to_string());
        if destination.exists() {
            return Err(ArtifactError::Conflict.into());
        }
        let mut metadata = read_metadata(&source.join(METADATA_FILE))?;
        if metadata.id != id || metadata.scope != scope {
            return Err(ArtifactError::Corrupt.into());
        }
        metadata.state = state;
        metadata.validate(self.limits)?;
        let bytes =
            serde_json::to_vec_pretty(&metadata).map_err(|_| ArtifactStoreError::Corrupt {
                entry: "metadata serialization",
            })?;
        if bytes.len() as u64 > MAX_METADATA_BYTES {
            return Err(ArtifactStoreError::TooLarge {
                entry: "metadata",
                limit: MAX_METADATA_BYTES,
            });
        }
        self.writer
            .write(&source.join(METADATA_FILE), &bytes)
            .map_err(|error| io_error("commit artifact state", error))?;
        if let Err(error) = fs::rename(&source, &destination) {
            return Err(io_error("move artifact state", error));
        }
        let _ = sync_directory(source.parent().ok_or(ArtifactStoreError::UnsafeEntry {
            entry: "artifact bucket",
        })?);
        let _ = sync_directory(
            destination
                .parent()
                .ok_or(ArtifactStoreError::UnsafeEntry {
                    entry: "artifact bucket",
                })?,
        );
        Ok(metadata)
    }
}

fn validate_owned_artifact_directory(
    directory: &Path,
    id: ArtifactId,
    scope: ArtifactScope,
    limits: termirust_domain::ArtifactLimits,
) -> Result<(), ArtifactStoreError> {
    let directory_metadata = fs::symlink_metadata(directory)
        .map_err(|error| io_error("inspect artifact entry", error))?;
    if directory_metadata.file_type().is_symlink() || !directory_metadata.is_dir() {
        return Err(ArtifactStoreError::UnsafeEntry {
            entry: "artifact entry",
        });
    }
    let metadata = read_metadata(&directory.join(METADATA_FILE))?;
    if metadata.id != id || metadata.scope != scope || metadata.validate(limits).is_err() {
        return Err(ArtifactError::Corrupt.into());
    }
    safe_file_metadata(&directory.join(DATA_FILE), "data")?;
    for entry in fs::read_dir(directory).map_err(|error| io_error("read artifact entry", error))? {
        let entry = entry.map_err(|error| io_error("read artifact entry", error))?;
        if !matches!(entry.file_name().to_str(), Some(DATA_FILE | METADATA_FILE)) {
            return Err(ArtifactStoreError::UnsafeEntry {
                entry: "artifact child",
            });
        }
    }
    Ok(())
}

fn directory_regular_bytes(directory: &Path) -> Result<u64, ArtifactStoreError> {
    let mut total = 0_u64;
    for entry in fs::read_dir(directory).map_err(|error| io_error("read staging entry", error))? {
        let entry = entry.map_err(|error| io_error("read staging child", error))?;
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|error| io_error("inspect staging child", error))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(ArtifactStoreError::UnsafeEntry {
                entry: "staging child",
            });
        }
        total = total.saturating_add(metadata.len());
    }
    Ok(total)
}
