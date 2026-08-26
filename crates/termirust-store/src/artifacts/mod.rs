mod ingest;
mod metadata;
mod quarantine;

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::fd::AsRawFd as _;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

use termirust_domain::{
    ArtifactCancellation, ArtifactError, ArtifactId, ArtifactLimits, ArtifactMetadata,
    ArtifactScope,
};

use crate::{AtomicWriter, Durability, SystemAtomicWriter};

const ARTIFACTS_DIR: &str = "artifacts";
const READY_DIR: &str = "ready";
const QUARANTINE_DIR: &str = "quarantine";
const STAGING_DIR: &str = "staging";
const DATA_FILE: &str = "data";
const METADATA_FILE: &str = "metadata.json";
const LOCK_FILE: &str = ".artifacts.lock";
const MAX_METADATA_BYTES: u64 = 16 * 1024;
const IO_CHUNK_BYTES: usize = 64 * 1024;
const LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const LOCK_RETRY: Duration = Duration::from_millis(10);
const STAGING_RETENTION_MILLIS: u64 = 24 * 60 * 60 * 1_000;

#[derive(Clone)]
pub struct ArtifactRepository {
    root: PathBuf,
    limits: ArtifactLimits,
    writer: Arc<dyn AtomicWriter>,
}

impl fmt::Debug for ArtifactRepository {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ArtifactRepository")
            .field("root", &"<redacted>")
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactSnapshot {
    pub scope: ArtifactScope,
    pub artifacts: Vec<ArtifactMetadata>,
    pub session_bytes: u64,
    pub session_limit: u64,
    pub global_bytes: u64,
    pub global_limit: u64,
    pub durability: Durability,
}

#[derive(Clone)]
pub struct ArtifactIngestRequest {
    pub id: ArtifactId,
    pub scope: ArtifactScope,
    pub source: PathBuf,
    pub display_name: Option<String>,
    pub created_at: u64,
}

impl fmt::Debug for ArtifactIngestRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ArtifactIngestRequest")
            .field("id", &self.id)
            .field("scope", &self.scope)
            .field("source", &"<redacted>")
            .field("display_name", &"<redacted>")
            .field("created_at", &self.created_at)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArtifactIngestProgress {
    pub bytes: u64,
    pub item_limit: u64,
    pub session_used: u64,
    pub session_limit: u64,
    pub global_used: u64,
    pub global_limit: u64,
}

#[derive(Clone, Eq, PartialEq)]
pub struct ArtifactPayload {
    pub metadata: ArtifactMetadata,
    pub bytes: Vec<u8>,
}

impl fmt::Debug for ArtifactPayload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ArtifactPayload")
            .field("metadata", &self.metadata)
            .field(
                "bytes",
                &format_args!("<redacted:{} bytes>", self.bytes.len()),
            )
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArtifactSweepResult {
    pub removed_entries: usize,
    pub removed_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ArtifactStoreError {
    Domain(ArtifactError),
    Io {
        operation: &'static str,
        kind: io::ErrorKind,
    },
    Corrupt {
        entry: &'static str,
    },
    UnsafeEntry {
        entry: &'static str,
    },
    TooLarge {
        entry: &'static str,
        limit: u64,
    },
}

impl fmt::Display for ArtifactStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Domain(error) => error.fmt(formatter),
            Self::Io { operation, kind } => {
                write!(formatter, "artifact store {operation} failed ({kind:?})")
            }
            Self::Corrupt { entry } => write!(formatter, "artifact {entry} is corrupt"),
            Self::UnsafeEntry { entry } => write!(formatter, "artifact {entry} is unsafe"),
            Self::TooLarge { entry, limit } => {
                write!(formatter, "artifact {entry} exceeds {limit} bytes")
            }
        }
    }
}

impl std::error::Error for ArtifactStoreError {}

impl From<ArtifactError> for ArtifactStoreError {
    fn from(error: ArtifactError) -> Self {
        Self::Domain(error)
    }
}

impl ArtifactRepository {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, ArtifactStoreError> {
        Self::open_with(
            root,
            ArtifactLimits::default(),
            Arc::new(SystemAtomicWriter),
        )
    }

    pub fn open_with(
        root: impl Into<PathBuf>,
        limits: ArtifactLimits,
        writer: Arc<dyn AtomicWriter>,
    ) -> Result<Self, ArtifactStoreError> {
        limits.validate()?;
        let root = root.into();
        create_user_only_directory(&root, "root")?;
        let root = fs::canonicalize(&root).map_err(|error| io_error("canonicalize root", error))?;
        let repository = Self {
            root,
            limits,
            writer,
        };
        let now_millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        repository.sweep_staging(now_millis)?;
        Ok(repository)
    }

    pub const fn limits(&self) -> ArtifactLimits {
        self.limits
    }

    pub fn list(&self, scope: ArtifactScope) -> Result<ArtifactSnapshot, ArtifactStoreError> {
        let _lock = self.acquire_lock()?;
        self.snapshot_locked(scope)
    }

    pub fn read_payload(
        &self,
        scope: ArtifactScope,
        id: ArtifactId,
        cancellation: &ArtifactCancellation,
    ) -> Result<ArtifactPayload, ArtifactStoreError> {
        let _lock = self.acquire_lock()?;
        self.read_payload_locked(scope, id, cancellation, false)
    }

    fn acquire_lock(&self) -> Result<ArtifactLock, ArtifactStoreError> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        let file = options
            .open(self.root.join(LOCK_FILE))
            .map_err(|error| io_error("open lock", error))?;
        if !file
            .metadata()
            .map_err(|error| io_error("inspect lock", error))?
            .is_file()
        {
            return Err(ArtifactStoreError::UnsafeEntry { entry: "lock" });
        }
        #[cfg(unix)]
        {
            let deadline = Instant::now() + LOCK_TIMEOUT;
            loop {
                let result =
                    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
                if result == 0 {
                    break;
                }
                let error = io::Error::last_os_error();
                let retryable = error
                    .raw_os_error()
                    .is_some_and(|code| code == libc::EWOULDBLOCK || code == libc::EAGAIN);
                if !retryable || Instant::now() >= deadline {
                    return Err(io_error("lock metadata", error));
                }
                thread::sleep(LOCK_RETRY);
            }
        }
        Ok(ArtifactLock { file })
    }
}

struct ArtifactLock {
    file: File,
}

impl Drop for ArtifactLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

fn io_error(operation: &'static str, error: io::Error) -> ArtifactStoreError {
    let domain = match error.kind() {
        io::ErrorKind::PermissionDenied => Some(ArtifactError::PermissionDenied),
        io::ErrorKind::StorageFull => Some(ArtifactError::StorageFull),
        io::ErrorKind::AlreadyExists => Some(ArtifactError::Conflict),
        _ => None,
    };
    domain.map_or(
        ArtifactStoreError::Io {
            operation,
            kind: error.kind(),
        },
        ArtifactStoreError::Domain,
    )
}

#[cfg(unix)]
fn create_user_only_directory(path: &Path, entry: &'static str) -> Result<(), ArtifactStoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(ArtifactStoreError::UnsafeEntry { entry });
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(path).map_err(|error| io_error("create directory", error))?;
        }
        Err(error) => return Err(io_error("inspect directory", error)),
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| io_error("secure directory", error))
}

#[cfg(not(unix))]
fn create_user_only_directory(path: &Path, entry: &'static str) -> Result<(), ArtifactStoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err(ArtifactStoreError::UnsafeEntry { entry })
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(path).map_err(|error| io_error("create directory", error))
        }
        Err(error) => Err(io_error("inspect directory", error)),
    }
}

fn sync_directory(path: &Path) -> Result<Durability, ArtifactStoreError> {
    match File::open(path).and_then(|directory| directory.sync_all()) {
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
        Err(error) => Err(io_error("sync directory", error)),
    }
}

pub(super) fn open_regular_file(
    path: &Path,
    entry: &'static str,
) -> Result<File, ArtifactStoreError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| io_error("inspect file", error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(ArtifactStoreError::UnsafeEntry { entry });
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    let file = options
        .open(path)
        .map_err(|error| io_error("open regular file", error))?;
    if !file
        .metadata()
        .map_err(|error| io_error("inspect opened file", error))?
        .is_file()
    {
        return Err(ArtifactStoreError::UnsafeEntry { entry });
    }
    Ok(file)
}

#[cfg(test)]
mod tests;
