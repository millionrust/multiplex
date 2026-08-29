use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(unix)]
use std::os::fd::AsRawFd as _;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

use serde::{Deserialize, Serialize};
use termirust_domain::{ContinuityLink, Revision};

use crate::{AtomicWriter, Durability, SystemAtomicWriter};

const CONTINUITY_FILE: &str = "resume-continuity.json";
const CONTINUITY_LOCK_FILE: &str = "resume-continuity.lock";
const MAX_CONTINUITY_DOCUMENT_BYTES: u64 = 1024 * 1024;
const CURRENT_CONTINUITY_FORMAT: u16 = 1;
pub const MAX_CONTINUITY_LINKS: usize = 10_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContinuitySnapshot {
    pub revision: Revision,
    pub links: Vec<ContinuityLink>,
    pub durability: Durability,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContinuityStoreError {
    Io {
        operation: &'static str,
        kind: io::ErrorKind,
    },
    UnsafeEntry,
    TooLarge,
    Corrupt,
    Newer {
        found: u16,
        supported: u16,
    },
    StaleRevision {
        expected: Revision,
        actual: Revision,
    },
    RevisionOverflow,
    Conflict,
}

impl fmt::Display for ContinuityStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { operation, kind } => {
                write!(formatter, "continuity store {operation} failed ({kind:?})")
            }
            Self::UnsafeEntry => {
                formatter.write_str("continuity store entry is not a safe regular file")
            }
            Self::TooLarge => formatter.write_str("continuity store exceeds its limit"),
            Self::Corrupt => formatter.write_str("continuity store is corrupt"),
            Self::Newer { found, supported } => write!(
                formatter,
                "continuity store format {found} is newer than supported format {supported}"
            ),
            Self::StaleRevision { expected, actual } => write!(
                formatter,
                "continuity store revision is stale (expected {}, actual {})",
                expected.get(),
                actual.get()
            ),
            Self::RevisionOverflow => formatter.write_str("continuity revision overflow"),
            Self::Conflict => formatter.write_str("session continuity already has a successor"),
        }
    }
}

impl std::error::Error for ContinuityStoreError {}

#[derive(Clone)]
pub struct ContinuityRepository {
    root: PathBuf,
    writer: Arc<dyn AtomicWriter>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ContinuityDocument {
    format_version: u16,
    revision: Revision,
    links: Vec<ContinuityLink>,
}

impl Default for ContinuityDocument {
    fn default() -> Self {
        Self {
            format_version: CURRENT_CONTINUITY_FORMAT,
            revision: Revision::ZERO,
            links: Vec::new(),
        }
    }
}

impl ContinuityRepository {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, ContinuityStoreError> {
        Self::open_with(root, Arc::new(SystemAtomicWriter))
    }

    pub fn open_with(
        root: impl Into<PathBuf>,
        writer: Arc<dyn AtomicWriter>,
    ) -> Result<Self, ContinuityStoreError> {
        let repository = Self {
            root: root.into(),
            writer,
        };
        repository.ensure_root()?;
        let _lock = repository.acquire_lock()?;
        if !repository.path().exists() {
            repository.write_document(&ContinuityDocument::default())?;
        }
        Ok(repository)
    }

    pub fn load(&self) -> Result<ContinuitySnapshot, ContinuityStoreError> {
        let _lock = self.acquire_lock()?;
        self.load_locked()
    }

    pub fn record(
        &self,
        expected: Revision,
        link: ContinuityLink,
    ) -> Result<ContinuitySnapshot, ContinuityStoreError> {
        validate_link(&link)?;
        let _lock = self.acquire_lock()?;
        let mut document = self.read_document()?;
        if let Some(existing) = document
            .links
            .iter()
            .find(|existing| existing.command_id == link.command_id)
        {
            return if existing == &link {
                Ok(snapshot(document, Durability::Full))
            } else {
                Err(ContinuityStoreError::Conflict)
            };
        }
        if document.revision != expected {
            return Err(ContinuityStoreError::StaleRevision {
                expected,
                actual: document.revision,
            });
        }
        if document.links.len() >= MAX_CONTINUITY_LINKS {
            return Err(ContinuityStoreError::TooLarge);
        }
        if document.links.iter().any(|existing| {
            existing.source_session_id == link.source_session_id
                || existing.replacement_session_id == link.replacement_session_id
                || existing.source_session_id == link.replacement_session_id
        }) {
            return Err(ContinuityStoreError::Conflict);
        }
        document.links.push(link);
        validate_links(&document.links)?;
        document.revision = document
            .revision
            .next()
            .ok_or(ContinuityStoreError::RevisionOverflow)?;
        let durability = self.write_document(&document)?;
        Ok(snapshot(document, durability))
    }

    pub fn metadata_path(&self) -> PathBuf {
        self.path()
    }

    fn load_locked(&self) -> Result<ContinuitySnapshot, ContinuityStoreError> {
        Ok(snapshot(self.read_document()?, Durability::Full))
    }

    fn path(&self) -> PathBuf {
        self.root.join(CONTINUITY_FILE)
    }

    fn ensure_root(&self) -> Result<(), ContinuityStoreError> {
        match fs::symlink_metadata(&self.root) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(ContinuityStoreError::UnsafeEntry);
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir_all(&self.root).map_err(|error| io_error("create", error))?;
            }
            Err(error) => return Err(io_error("inspect", error)),
        }
        #[cfg(unix)]
        fs::set_permissions(&self.root, fs::Permissions::from_mode(0o700))
            .map_err(|error| io_error("permissions", error))?;
        Ok(())
    }

    fn acquire_lock(&self) -> Result<ContinuityStoreLock, ContinuityStoreError> {
        let path = self.root.join(CONTINUITY_LOCK_FILE);
        reject_unsafe_optional_target(&path)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        let file = options
            .open(path)
            .map_err(|error| io_error("open lock", error))?;
        if !file
            .metadata()
            .map_err(|error| io_error("inspect lock", error))?
            .is_file()
        {
            return Err(ContinuityStoreError::UnsafeEntry);
        }
        #[cfg(unix)]
        {
            fs::set_permissions(
                self.root.join(CONTINUITY_LOCK_FILE),
                fs::Permissions::from_mode(0o600),
            )
            .map_err(|error| io_error("lock permissions", error))?;
            let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
            if result != 0 {
                return Err(io_error("lock", io::Error::last_os_error()));
            }
        }
        Ok(ContinuityStoreLock { file })
    }

    fn read_document(&self) -> Result<ContinuityDocument, ContinuityStoreError> {
        let path = self.path();
        reject_unsafe_target(&path)?;
        let metadata = fs::metadata(&path).map_err(|error| io_error("metadata", error))?;
        if metadata.len() > MAX_CONTINUITY_DOCUMENT_BYTES {
            return Err(ContinuityStoreError::TooLarge);
        }
        let bytes = fs::read(path).map_err(|error| io_error("read", error))?;
        let document: ContinuityDocument =
            serde_json::from_slice(&bytes).map_err(|_| ContinuityStoreError::Corrupt)?;
        if document.format_version > CURRENT_CONTINUITY_FORMAT {
            return Err(ContinuityStoreError::Newer {
                found: document.format_version,
                supported: CURRENT_CONTINUITY_FORMAT,
            });
        }
        validate_links(&document.links)?;
        Ok(document)
    }

    fn write_document(
        &self,
        document: &ContinuityDocument,
    ) -> Result<Durability, ContinuityStoreError> {
        validate_links(&document.links)?;
        let bytes =
            serde_json::to_vec_pretty(document).map_err(|_| ContinuityStoreError::Corrupt)?;
        if bytes.len() as u64 > MAX_CONTINUITY_DOCUMENT_BYTES {
            return Err(ContinuityStoreError::TooLarge);
        }
        self.writer
            .write(&self.path(), &bytes)
            .map_err(|error| io_error("write", error))
    }
}

fn snapshot(document: ContinuityDocument, durability: Durability) -> ContinuitySnapshot {
    ContinuitySnapshot {
        revision: document.revision,
        links: document.links,
        durability,
    }
}

fn validate_links(links: &[ContinuityLink]) -> Result<(), ContinuityStoreError> {
    if links.len() > MAX_CONTINUITY_LINKS {
        return Err(ContinuityStoreError::TooLarge);
    }
    for (index, link) in links.iter().enumerate() {
        validate_link(link)?;
        if links[..index].iter().any(|existing| {
            existing.command_id == link.command_id
                || existing.source_session_id == link.source_session_id
                || existing.replacement_session_id == link.replacement_session_id
                || existing.source_session_id == link.replacement_session_id
        }) {
            return Err(ContinuityStoreError::Corrupt);
        }
    }
    Ok(())
}

fn validate_link(link: &ContinuityLink) -> Result<(), ContinuityStoreError> {
    if link.source_session_id == link.replacement_session_id
        || link.prior_generation.get() == u64::MAX
        || link.replacement_generation != link.prior_generation.next()
    {
        return Err(ContinuityStoreError::Corrupt);
    }
    Ok(())
}

struct ContinuityStoreLock {
    file: File,
}

impl Drop for ContinuityStoreLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

fn reject_unsafe_optional_target(path: &Path) -> Result<(), ContinuityStoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(ContinuityStoreError::UnsafeEntry)
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error("inspect", error)),
    }
}

fn reject_unsafe_target(path: &Path) -> Result<(), ContinuityStoreError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| io_error("inspect", error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(ContinuityStoreError::UnsafeEntry);
    }
    Ok(())
}

fn io_error(operation: &'static str, error: io::Error) -> ContinuityStoreError {
    ContinuityStoreError::Io {
        operation,
        kind: error.kind(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use termirust_domain::{CommandId, HostedSessionId, OccupantGeneration, RuntimeId};

    use super::*;

    #[derive(Default)]
    struct FailingWriter {
        writes: Mutex<usize>,
    }

    impl AtomicWriter for FailingWriter {
        fn write(&self, target: &Path, bytes: &[u8]) -> io::Result<Durability> {
            let mut writes = self.writes.lock().unwrap();
            *writes += 1;
            if *writes > 1 {
                return Err(io::Error::other("injected write failure"));
            }
            SystemAtomicWriter.write(target, bytes)
        }
    }

    fn link(source: u128, replacement: u128, command: u128) -> ContinuityLink {
        ContinuityLink {
            command_id: CommandId::from_uuid(uuid::Uuid::from_u128(command)),
            source_session_id: HostedSessionId::from_uuid(uuid::Uuid::from_u128(source)),
            replacement_session_id: HostedSessionId::from_uuid(uuid::Uuid::from_u128(replacement)),
            runtime_id: RuntimeId::new("codex").unwrap(),
            prior_generation: OccupantGeneration::new(7),
            replacement_generation: OccupantGeneration::new(8),
            committed_at: 10,
        }
    }

    #[test]
    fn continuity_survives_restart_and_replays_identical_command_idempotently() {
        let fixture = tempfile::tempdir().unwrap();
        let repository = ContinuityRepository::open(fixture.path()).unwrap();
        let first = repository.record(Revision::ZERO, link(1, 2, 3)).unwrap();
        let replay = repository.record(Revision::ZERO, link(1, 2, 3)).unwrap();
        assert_eq!(replay.revision, first.revision);
        assert_eq!(replay.links, first.links);
        drop(repository);
        assert_eq!(
            ContinuityRepository::open(fixture.path())
                .unwrap()
                .load()
                .unwrap()
                .links,
            first.links
        );
    }

    #[test]
    fn continuity_rejects_stale_revisions_and_competing_successors() {
        let fixture = tempfile::tempdir().unwrap();
        let first = ContinuityRepository::open(fixture.path()).unwrap();
        let second = ContinuityRepository::open(fixture.path()).unwrap();
        let saved = first.record(Revision::ZERO, link(1, 2, 3)).unwrap();
        assert_eq!(
            second.record(Revision::ZERO, link(4, 5, 6)).unwrap_err(),
            ContinuityStoreError::StaleRevision {
                expected: Revision::ZERO,
                actual: saved.revision,
            }
        );
        assert_eq!(
            first.record(saved.revision, link(1, 4, 7)).unwrap_err(),
            ContinuityStoreError::Conflict
        );
        assert_eq!(
            first.record(saved.revision, link(4, 2, 8)).unwrap_err(),
            ContinuityStoreError::Conflict
        );
        assert_eq!(first.load().unwrap().links.len(), 1);
    }

    #[test]
    fn continuity_allows_forward_chains_but_rejects_cycles() {
        let fixture = tempfile::tempdir().unwrap();
        let repository = ContinuityRepository::open(fixture.path()).unwrap();
        let first = repository.record(Revision::ZERO, link(1, 2, 3)).unwrap();
        let second = repository.record(first.revision, link(2, 4, 5)).unwrap();
        assert_eq!(second.links.len(), 2);
        assert_eq!(
            repository
                .record(second.revision, link(4, 1, 6))
                .unwrap_err(),
            ContinuityStoreError::Conflict
        );
    }

    #[test]
    fn continuity_failed_write_and_corrupt_generation_preserve_last_good_state() {
        let fixture = tempfile::tempdir().unwrap();
        let repository =
            ContinuityRepository::open_with(fixture.path(), Arc::new(FailingWriter::default()))
                .unwrap();
        assert!(repository.record(Revision::ZERO, link(1, 2, 3)).is_err());
        assert!(repository.load().unwrap().links.is_empty());

        let mut invalid = link(1, 2, 3);
        invalid.replacement_generation = OccupantGeneration::new(9);
        assert_eq!(
            repository.record(Revision::ZERO, invalid).unwrap_err(),
            ContinuityStoreError::Corrupt
        );
    }

    #[test]
    fn continuity_rejects_newer_oversized_and_unsafe_documents() {
        let fixture = tempfile::tempdir().unwrap();
        let repository = ContinuityRepository::open(fixture.path()).unwrap();
        let path = repository.metadata_path();
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value["format_version"] = serde_json::json!(2);
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(
            repository.load().unwrap_err(),
            ContinuityStoreError::Newer {
                found: 2,
                supported: 1,
            }
        );

        fs::write(
            &path,
            vec![b'x'; MAX_CONTINUITY_DOCUMENT_BYTES as usize + 1],
        )
        .unwrap();
        assert_eq!(
            repository.load().unwrap_err(),
            ContinuityStoreError::TooLarge
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            fs::remove_file(&path).unwrap();
            let outside = fixture.path().join("outside");
            fs::write(&outside, b"sentinel").unwrap();
            symlink(&outside, &path).unwrap();
            assert_eq!(
                repository.load().unwrap_err(),
                ContinuityStoreError::UnsafeEntry
            );
            assert_eq!(fs::read(outside).unwrap(), b"sentinel");
        }
    }

    #[cfg(unix)]
    #[test]
    fn continuity_store_uses_private_permissions_and_rejects_lock_symlinks() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};

        let fixture = tempfile::tempdir().unwrap();
        let repository = ContinuityRepository::open(fixture.path()).unwrap();
        assert_eq!(
            fs::metadata(fixture.path()).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(repository.metadata_path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        fs::remove_file(fixture.path().join(CONTINUITY_LOCK_FILE)).unwrap();
        let outside = fixture.path().join("outside-lock");
        fs::write(&outside, b"sentinel").unwrap();
        symlink(&outside, fixture.path().join(CONTINUITY_LOCK_FILE)).unwrap();
        assert_eq!(
            repository.load().unwrap_err(),
            ContinuityStoreError::UnsafeEntry
        );
        assert_eq!(fs::read(outside).unwrap(), b"sentinel");
    }
}
