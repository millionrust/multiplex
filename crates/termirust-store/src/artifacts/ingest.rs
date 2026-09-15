use std::fs::{self, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _};

use sha2::{Digest as _, Sha256};
use termirust_domain::{
    ArtifactCancellation, ArtifactDisplayName, ArtifactError, ArtifactMediaType, ArtifactMetadata,
    ArtifactOrigin, ArtifactPreviewKind, ArtifactSha256, ArtifactState,
};

use super::metadata::hash_file;
use super::{
    ArtifactIngestProgress, ArtifactIngestRequest, ArtifactRepository, ArtifactStoreError,
    DATA_FILE, IO_CHUNK_BYTES, METADATA_FILE, READY_DIR, STAGING_DIR, io_error, open_regular_file,
    sync_directory,
};

impl ArtifactRepository {
    pub fn ingest<F>(
        &self,
        request: ArtifactIngestRequest,
        cancellation: &ArtifactCancellation,
        mut progress: F,
    ) -> Result<ArtifactMetadata, ArtifactStoreError>
    where
        F: FnMut(ArtifactIngestProgress),
    {
        cancellation.check()?;
        let _lock = self.acquire_lock()?;
        self.prepare_session_directories(request.scope)?;
        if self
            .scan_session_locked(request.scope)?
            .iter()
            .any(|item| item.metadata.id == request.id)
        {
            return Err(ArtifactError::Conflict.into());
        }
        let usage = self.scan_usage_locked()?;
        if usage.count_for(request.scope.session_id) >= self.limits.artifacts_per_session
            || usage.global_count >= self.limits.global_artifacts
        {
            return Err(ArtifactError::CountQuotaExceeded.into());
        }

        let source_metadata = source_metadata(&request.source)?;
        if source_metadata.len() > self.limits.item_bytes {
            return Err(ArtifactError::ItemQuotaExceeded.into());
        }
        let source_identity = SourceIdentity::new(&source_metadata);
        let display_source = request
            .display_name
            .as_deref()
            .or_else(|| request.source.file_name().and_then(|name| name.to_str()));
        let display_name =
            ArtifactDisplayName::new(display_source.ok_or(ArtifactError::InvalidDisplayName)?)?;

        let staging = self
            .bucket_path(request.scope, STAGING_DIR)
            .join(request.id.to_string());
        create_new_user_only_directory(&staging)?;
        let result = self.ingest_to_staging(
            &request,
            &display_name,
            &source_identity,
            &usage,
            &staging,
            cancellation,
            &mut progress,
        );
        if result.is_err() {
            let _ = remove_owned_staging(&staging);
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn ingest_to_staging<F>(
        &self,
        request: &ArtifactIngestRequest,
        display_name: &ArtifactDisplayName,
        source_identity: &SourceIdentity,
        usage: &super::metadata::ArtifactUsage,
        staging: &Path,
        cancellation: &ArtifactCancellation,
        progress: &mut F,
    ) -> Result<ArtifactMetadata, ArtifactStoreError>
    where
        F: FnMut(ArtifactIngestProgress),
    {
        let mut source = open_regular_file(&request.source, "import source")?;
        let opened_source = source
            .metadata()
            .map_err(|error| io_error("inspect opened import source", error))?;
        if !source_identity.matches(&opened_source) {
            return Err(ArtifactError::SourceChanged.into());
        }
        let data_path = staging.join(DATA_FILE);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut destination = options
            .open(&data_path)
            .map_err(|error| io_error("create staging data", error))?;
        let mut hasher = Sha256::new();
        let mut prefix = Vec::with_capacity(512);
        let mut written = 0_u64;
        let mut buffer = [0_u8; IO_CHUNK_BYTES];
        loop {
            cancellation.check()?;
            let read = source
                .read(&mut buffer)
                .map_err(|error| io_error("read import source", error))?;
            if read == 0 {
                break;
            }
            written = written
                .checked_add(read as u64)
                .ok_or(ArtifactError::ItemQuotaExceeded)?;
            enforce_ingest_quotas(self, usage, request.scope.session_id, written)?;
            let prefix_remaining = 512_usize.saturating_sub(prefix.len());
            prefix.extend_from_slice(&buffer[..read.min(prefix_remaining)]);
            hasher.update(&buffer[..read]);
            destination
                .write_all(&buffer[..read])
                .map_err(|error| io_error("write staging data", error))?;
            progress(ArtifactIngestProgress {
                bytes: written,
                item_limit: self.limits.item_bytes,
                session_used: usage.bytes_for(request.scope.session_id),
                session_limit: self.limits.session_bytes,
                global_used: usage.global_bytes,
                global_limit: self.limits.global_bytes,
            });
        }
        destination
            .sync_all()
            .map_err(|error| io_error("sync staging data", error))?;
        drop(destination);
        drop(source);
        let final_source = source_metadata(&request.source)?;
        if !source_identity.matches(&final_source) || final_source.len() != written {
            return Err(ArtifactError::SourceChanged.into());
        }
        let sha256 = ArtifactSha256::new(hasher.finalize().into());
        let media_type = classify_media(&prefix, &data_path, cancellation)?;
        let preview_kind = match media_type {
            ArtifactMediaType::TextPlainUtf8 => ArtifactPreviewKind::Text,
            ArtifactMediaType::ImagePng | ArtifactMediaType::ImageJpeg => {
                ArtifactPreviewKind::Raster
            }
            ArtifactMediaType::MetadataOnly => ArtifactPreviewKind::MetadataOnly,
        };

        if let Some(duplicate) = self
            .scan_session_locked(request.scope)?
            .into_iter()
            .find(|item| item.metadata_valid && item.metadata.sha256 == sha256)
        {
            let duplicate_data = duplicate.directory.join(DATA_FILE);
            let (verified, length) =
                hash_file(&duplicate_data, self.limits.item_bytes, cancellation)?;
            if verified == sha256 && length == written {
                fs::remove_file(&data_path)
                    .map_err(|error| io_error("replace duplicate staging data", error))?;
                fs::hard_link(&duplicate_data, &data_path)
                    .map_err(|error| io_error("deduplicate artifact data", error))?;
            }
        }

        let metadata = ArtifactMetadata {
            id: request.id,
            scope: request.scope,
            display_name: display_name.clone(),
            origin: ArtifactOrigin::ExplicitImport,
            media_type,
            byte_len: written,
            sha256,
            created_at: request.created_at,
            preview_kind,
            state: ArtifactState::Ready,
        };
        metadata.validate(self.limits)?;
        let encoded =
            serde_json::to_vec_pretty(&metadata).map_err(|_| ArtifactStoreError::Corrupt {
                entry: "metadata serialization",
            })?;
        if encoded.len() as u64 > super::MAX_METADATA_BYTES {
            return Err(ArtifactStoreError::TooLarge {
                entry: "metadata",
                limit: super::MAX_METADATA_BYTES,
            });
        }
        self.writer
            .write(&staging.join(METADATA_FILE), &encoded)
            .map_err(|error| io_error("commit staging metadata", error))?;
        sync_directory(staging)?;
        cancellation.check()?;
        let ready = self
            .bucket_path(request.scope, READY_DIR)
            .join(request.id.to_string());
        if ready.exists() {
            return Err(ArtifactError::Conflict.into());
        }
        fs::rename(staging, &ready).map_err(|error| io_error("commit artifact", error))?;
        // The rename is already the commit point. A directory-sync failure cannot be
        // reported as retryable without risking a duplicate after a successful commit.
        let _ = sync_directory(ready.parent().ok_or(ArtifactStoreError::UnsafeEntry {
            entry: "ready directory",
        })?);
        Ok(metadata)
    }
}

fn enforce_ingest_quotas(
    repository: &ArtifactRepository,
    usage: &super::metadata::ArtifactUsage,
    session_id: termirust_domain::HostedSessionId,
    incoming: u64,
) -> Result<(), ArtifactStoreError> {
    if incoming > repository.limits.item_bytes {
        return Err(ArtifactError::ItemQuotaExceeded.into());
    }
    if usage
        .bytes_for(session_id)
        .checked_add(incoming)
        .is_none_or(|total| total > repository.limits.session_bytes)
    {
        return Err(ArtifactError::SessionQuotaExceeded.into());
    }
    if usage
        .global_bytes
        .checked_add(incoming)
        .is_none_or(|total| total > repository.limits.global_bytes)
    {
        return Err(ArtifactError::GlobalQuotaExceeded.into());
    }
    Ok(())
}

fn classify_media(
    prefix: &[u8],
    path: &Path,
    cancellation: &ArtifactCancellation,
) -> Result<ArtifactMediaType, ArtifactStoreError> {
    if prefix.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Ok(ArtifactMediaType::ImagePng);
    }
    if prefix.starts_with(&[0xff, 0xd8, 0xff]) {
        return Ok(ArtifactMediaType::ImageJpeg);
    }
    if !looks_like_container(prefix)
        && is_utf8_file(path, cancellation)?
        && !looks_like_active_text(prefix)
    {
        return Ok(ArtifactMediaType::TextPlainUtf8);
    }
    Ok(ArtifactMediaType::MetadataOnly)
}

fn looks_like_active_text(prefix: &[u8]) -> bool {
    let lower = String::from_utf8_lossy(prefix)
        .trim_start()
        .trim_start_matches('\u{feff}')
        .trim_start()
        .to_ascii_lowercase();
    lower.starts_with("<?xml")
        || lower.starts_with("%pdf-")
        || lower.contains("<!doctype html")
        || lower.contains("<html")
        || lower.contains("<svg")
        || lower.contains("<script")
}

fn looks_like_container(prefix: &[u8]) -> bool {
    prefix.starts_with(b"PK\x03\x04")
        || prefix.starts_with(b"PK\x05\x06")
        || prefix.starts_with(b"PK\x07\x08")
        || prefix.starts_with(b"\x1f\x8b")
        || prefix.starts_with(b"7z\xbc\xaf\x27\x1c")
        || prefix.starts_with(b"Rar!\x1a\x07")
        || prefix.starts_with(b"BZh")
        || prefix.starts_with(b"\xfd7zXZ\0")
        || prefix.get(257..262) == Some(b"ustar")
}

fn is_utf8_file(
    path: &Path,
    cancellation: &ArtifactCancellation,
) -> Result<bool, ArtifactStoreError> {
    let mut file = open_regular_file(path, "staged data")?;
    let mut pending = Vec::with_capacity(IO_CHUNK_BYTES + 4);
    let mut buffer = [0_u8; IO_CHUNK_BYTES];
    loop {
        cancellation.check()?;
        let read = file
            .read(&mut buffer)
            .map_err(|error| io_error("read staged data", error))?;
        if read == 0 {
            return Ok(std::str::from_utf8(&pending).is_ok());
        }
        if buffer[..read].contains(&0) {
            return Ok(false);
        }
        pending.extend_from_slice(&buffer[..read]);
        match std::str::from_utf8(&pending) {
            Ok(_) => pending.clear(),
            Err(error) if error.error_len().is_none() => {
                let valid = error.valid_up_to();
                pending.drain(..valid);
                if pending.len() > 3 {
                    return Ok(false);
                }
            }
            Err(_) => return Ok(false),
        }
    }
}

fn source_metadata(path: &Path) -> Result<fs::Metadata, ArtifactStoreError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| io_error("inspect import source", error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(ArtifactError::UnsupportedSource.into());
    }
    Ok(metadata)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SourceIdentity {
    len: u64,
    modified: Option<std::time::SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    change_seconds: i64,
    #[cfg(unix)]
    change_nanoseconds: i64,
}

impl SourceIdentity {
    fn new(metadata: &fs::Metadata) -> Self {
        Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
            #[cfg(unix)]
            change_seconds: metadata.ctime(),
            #[cfg(unix)]
            change_nanoseconds: metadata.ctime_nsec(),
        }
    }

    fn matches(&self, metadata: &fs::Metadata) -> bool {
        self.len == metadata.len() && self.modified == metadata.modified().ok() && {
            #[cfg(unix)]
            {
                self.device == metadata.dev()
                    && self.inode == metadata.ino()
                    && self.change_seconds == metadata.ctime()
                    && self.change_nanoseconds == metadata.ctime_nsec()
            }
            #[cfg(not(unix))]
            {
                true
            }
        }
    }
}

fn create_new_user_only_directory(path: &Path) -> Result<(), ArtifactStoreError> {
    fs::create_dir(path).map_err(|error| io_error("create staging entry", error))?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| io_error("secure staging entry", error))?;
    Ok(())
}

fn remove_owned_staging(path: &Path) -> Result<(), ArtifactStoreError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| io_error("inspect staging", error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ArtifactStoreError::UnsafeEntry {
            entry: "staging entry",
        });
    }
    fs::remove_dir_all(path).map_err(|error| io_error("remove staging", error))
}
