use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

use serde::{Deserialize, Serialize};
use termirust_domain::{ControllerDeviceAuthority, ControllerDeviceError, DeviceStoreRevision};

use crate::{AtomicWriter, Durability, SystemAtomicWriter, file_lock};

const CONTROLLER_DEVICES_FILE: &str = "controller-devices.json";
const CONTROLLER_DEVICES_LOCK_FILE: &str = "controller-devices.lock";
const MAX_CONTROLLER_DOCUMENT_BYTES: u64 = 512 * 1024;
const CURRENT_CONTROLLER_FORMAT: u16 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ControllerDeviceSnapshot {
    pub revision: DeviceStoreRevision,
    pub authority: ControllerDeviceAuthority,
    pub durability: Durability,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerDeviceStoreError {
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
        expected: DeviceStoreRevision,
        actual: DeviceStoreRevision,
    },
    RevisionOverflow,
    Domain(ControllerDeviceError),
}

impl fmt::Display for ControllerDeviceStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { operation, kind } => {
                write!(
                    formatter,
                    "controller device store {operation} failed ({kind:?})"
                )
            }
            Self::UnsafeEntry => {
                formatter.write_str("controller device store entry is not a safe regular file")
            }
            Self::TooLarge => formatter.write_str("controller device store exceeds its limit"),
            Self::Corrupt => formatter.write_str("controller device store is corrupt"),
            Self::Newer { found, supported } => write!(
                formatter,
                "controller device store format {found} is newer than supported format {supported}"
            ),
            Self::StaleRevision { expected, actual } => write!(
                formatter,
                "controller device store revision is stale (expected {}, actual {})",
                expected.get(),
                actual.get()
            ),
            Self::RevisionOverflow => formatter.write_str("controller store revision overflow"),
            Self::Domain(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ControllerDeviceStoreError {}

impl From<ControllerDeviceError> for ControllerDeviceStoreError {
    fn from(error: ControllerDeviceError) -> Self {
        Self::Domain(error)
    }
}

#[derive(Clone)]
pub struct ControllerDeviceRepository {
    root: PathBuf,
    writer: Arc<dyn AtomicWriter>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ControllerDeviceDocument {
    format_version: u16,
    revision: DeviceStoreRevision,
    authority: ControllerDeviceAuthority,
}

impl Default for ControllerDeviceDocument {
    fn default() -> Self {
        Self {
            format_version: CURRENT_CONTROLLER_FORMAT,
            revision: DeviceStoreRevision::ZERO,
            authority: ControllerDeviceAuthority::default(),
        }
    }
}

impl ControllerDeviceRepository {
    pub fn inspect(
        root: impl Into<PathBuf>,
    ) -> Result<Option<ControllerDeviceSnapshot>, ControllerDeviceStoreError> {
        let repository = Self {
            root: root.into(),
            writer: Arc::new(SystemAtomicWriter),
        };
        match fs::symlink_metadata(&repository.root) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(ControllerDeviceStoreError::UnsafeEntry);
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io_error("inspect", error)),
        }
        match fs::symlink_metadata(repository.path()) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(ControllerDeviceStoreError::UnsafeEntry);
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io_error("inspect", error)),
        }
        let document = repository.read_document()?;
        Ok(Some(ControllerDeviceSnapshot {
            revision: document.revision,
            authority: document.authority,
            durability: Durability::Full,
        }))
    }

    pub fn open(root: impl Into<PathBuf>) -> Result<Self, ControllerDeviceStoreError> {
        Self::open_with(root, Arc::new(SystemAtomicWriter))
    }

    pub fn open_with(
        root: impl Into<PathBuf>,
        writer: Arc<dyn AtomicWriter>,
    ) -> Result<Self, ControllerDeviceStoreError> {
        let repository = Self {
            root: root.into(),
            writer,
        };
        repository.ensure_root()?;
        let _lock = repository.acquire_lock()?;
        if !repository.path().exists() {
            repository.write_document(&ControllerDeviceDocument::default())?;
        }
        Ok(repository)
    }

    pub fn load(&self) -> Result<ControllerDeviceSnapshot, ControllerDeviceStoreError> {
        let _lock = self.acquire_lock()?;
        self.load_locked()
    }

    pub fn save(
        &self,
        expected: DeviceStoreRevision,
        authority: ControllerDeviceAuthority,
    ) -> Result<ControllerDeviceSnapshot, ControllerDeviceStoreError> {
        authority.validate()?;
        let _lock = self.acquire_lock()?;
        let current = self.read_document()?;
        if current.revision != expected {
            return Err(ControllerDeviceStoreError::StaleRevision {
                expected,
                actual: current.revision,
            });
        }
        let revision = current
            .revision
            .next()
            .ok_or(ControllerDeviceStoreError::RevisionOverflow)?;
        let document = ControllerDeviceDocument {
            format_version: CURRENT_CONTROLLER_FORMAT,
            revision,
            authority,
        };
        let durability = self.write_document(&document)?;
        Ok(ControllerDeviceSnapshot {
            revision,
            authority: document.authority,
            durability,
        })
    }

    pub fn update<F>(
        &self,
        expected: DeviceStoreRevision,
        operation: F,
    ) -> Result<ControllerDeviceSnapshot, ControllerDeviceStoreError>
    where
        F: FnOnce(&mut ControllerDeviceAuthority) -> Result<(), ControllerDeviceError>,
    {
        let _lock = self.acquire_lock()?;
        let mut current = self.read_document()?;
        if current.revision != expected {
            return Err(ControllerDeviceStoreError::StaleRevision {
                expected,
                actual: current.revision,
            });
        }
        operation(&mut current.authority)?;
        current.authority.validate()?;
        current.revision = current
            .revision
            .next()
            .ok_or(ControllerDeviceStoreError::RevisionOverflow)?;
        let durability = self.write_document(&current)?;
        Ok(ControllerDeviceSnapshot {
            revision: current.revision,
            authority: current.authority,
            durability,
        })
    }

    pub fn reset_after_corruption(
        &self,
    ) -> Result<ControllerDeviceSnapshot, ControllerDeviceStoreError> {
        let _lock = self.acquire_lock()?;
        match self.read_document() {
            Ok(_) => return self.load_locked(),
            Err(ControllerDeviceStoreError::Corrupt | ControllerDeviceStoreError::TooLarge) => {}
            Err(error) => return Err(error),
        }
        self.write_document(&ControllerDeviceDocument::default())?;
        self.load_locked()
    }

    pub fn metadata_path(&self) -> PathBuf {
        self.path()
    }

    fn load_locked(&self) -> Result<ControllerDeviceSnapshot, ControllerDeviceStoreError> {
        let document = self.read_document()?;
        Ok(ControllerDeviceSnapshot {
            revision: document.revision,
            authority: document.authority,
            durability: Durability::Full,
        })
    }

    fn path(&self) -> PathBuf {
        self.root.join(CONTROLLER_DEVICES_FILE)
    }

    fn ensure_root(&self) -> Result<(), ControllerDeviceStoreError> {
        match fs::symlink_metadata(&self.root) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(ControllerDeviceStoreError::UnsafeEntry);
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

    fn acquire_lock(&self) -> Result<ControllerDeviceStoreLock, ControllerDeviceStoreError> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        options.mode(0o600);
        let file = options
            .open(self.root.join(CONTROLLER_DEVICES_LOCK_FILE))
            .map_err(|error| io_error("open lock", error))?;
        file_lock::exclusive(&file).map_err(|error| io_error("lock", error))?;
        Ok(ControllerDeviceStoreLock { file })
    }

    fn read_document(&self) -> Result<ControllerDeviceDocument, ControllerDeviceStoreError> {
        let path = self.path();
        reject_unsafe_target(&path)?;
        let metadata = fs::metadata(&path).map_err(|error| io_error("metadata", error))?;
        if metadata.len() > MAX_CONTROLLER_DOCUMENT_BYTES {
            return Err(ControllerDeviceStoreError::TooLarge);
        }
        let bytes = fs::read(path).map_err(|error| io_error("read", error))?;
        let document: ControllerDeviceDocument =
            serde_json::from_slice(&bytes).map_err(|_| ControllerDeviceStoreError::Corrupt)?;
        if document.format_version > CURRENT_CONTROLLER_FORMAT {
            return Err(ControllerDeviceStoreError::Newer {
                found: document.format_version,
                supported: CURRENT_CONTROLLER_FORMAT,
            });
        }
        document
            .authority
            .validate()
            .map_err(ControllerDeviceStoreError::Domain)?;
        Ok(document)
    }

    fn write_document(
        &self,
        document: &ControllerDeviceDocument,
    ) -> Result<Durability, ControllerDeviceStoreError> {
        document.authority.validate()?;
        let bytes =
            serde_json::to_vec_pretty(document).map_err(|_| ControllerDeviceStoreError::Corrupt)?;
        if bytes.len() as u64 > MAX_CONTROLLER_DOCUMENT_BYTES {
            return Err(ControllerDeviceStoreError::TooLarge);
        }
        self.writer
            .write(&self.path(), &bytes)
            .map_err(|error| io_error("write", error))
    }
}

struct ControllerDeviceStoreLock {
    file: File,
}

impl Drop for ControllerDeviceStoreLock {
    fn drop(&mut self) {
        file_lock::release(&self.file);
    }
}

fn reject_unsafe_target(path: &Path) -> Result<(), ControllerDeviceStoreError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| io_error("inspect", error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(ControllerDeviceStoreError::UnsafeEntry);
    }
    Ok(())
}

fn io_error(operation: &'static str, error: io::Error) -> ControllerDeviceStoreError {
    ControllerDeviceStoreError::Io {
        operation,
        kind: error.kind(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use termirust_domain::{
        ControllerCapabilities, ControllerDeviceId, ControllerProtocolRange, DevicePublicKey,
        HostIdentityGeneration, HostIdentityPublic, HostIdentitySecretRef, HostIdentityState,
        HostPublicKey, MAX_PAIRED_DEVICES, PairedDeviceRecord, PairedDeviceStatus, PairingOfferId,
    };

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

    fn ready_authority() -> ControllerDeviceAuthority {
        ControllerDeviceAuthority {
            identity: Some(HostIdentityPublic::new(
                HostIdentityGeneration::INITIAL,
                HostPublicKey([4; 32]),
            )),
            secret_ref: Some(HostIdentitySecretRef::new("identity:test").unwrap()),
            state: HostIdentityState::Ready,
            ..ControllerDeviceAuthority::default()
        }
    }

    fn device(index: usize) -> PairedDeviceRecord {
        PairedDeviceRecord {
            device_id: ControllerDeviceId::new(),
            public_key: DevicePublicKey([index as u8; 32]),
            display_name: format!("Device {index}"),
            capabilities: ControllerCapabilities::default(),
            protocol_range: ControllerProtocolRange::V1,
            created_at: index as u64,
            last_seen_at: None,
            revocation_epoch: 0,
            identity_generation: HostIdentityGeneration::INITIAL,
            status: PairedDeviceStatus::Offline,
            source_offer_id: PairingOfferId::new(),
        }
    }

    #[test]
    fn controller_devices_store_survives_restart_and_rejects_stale_revision() {
        let fixture = tempfile::tempdir().unwrap();
        let first = ControllerDeviceRepository::open(fixture.path()).unwrap();
        let second = ControllerDeviceRepository::open(fixture.path()).unwrap();
        let first_snapshot = first.load().unwrap();
        let second_snapshot = second.load().unwrap();
        let saved = first
            .save(first_snapshot.revision, ready_authority())
            .unwrap();
        assert_eq!(
            second
                .save(second_snapshot.revision, ready_authority())
                .unwrap_err(),
            ControllerDeviceStoreError::StaleRevision {
                expected: DeviceStoreRevision::ZERO,
                actual: saved.revision,
            }
        );
        drop(first);
        assert_eq!(
            ControllerDeviceRepository::open(fixture.path())
                .unwrap()
                .load()
                .unwrap()
                .authority
                .state,
            HostIdentityState::Ready
        );
    }

    #[test]
    fn controller_devices_inspect_never_creates_missing_state() {
        let fixture = tempfile::tempdir().unwrap();
        let missing = fixture.path().join("missing");
        assert_eq!(ControllerDeviceRepository::inspect(&missing).unwrap(), None);
        assert!(!missing.exists());

        let empty = fixture.path().join("empty");
        fs::create_dir(&empty).unwrap();
        assert_eq!(ControllerDeviceRepository::inspect(&empty).unwrap(), None);
        assert!(fs::read_dir(&empty).unwrap().next().is_none());

        let repository = ControllerDeviceRepository::open(&empty).unwrap();
        let expected = repository.load().unwrap();
        assert_eq!(
            ControllerDeviceRepository::inspect(&empty).unwrap(),
            Some(expected)
        );
    }

    #[test]
    fn controller_devices_store_failed_write_preserves_last_good_document() {
        let fixture = tempfile::tempdir().unwrap();
        let repository = ControllerDeviceRepository::open_with(
            fixture.path(),
            Arc::new(FailingWriter::default()),
        )
        .unwrap();
        assert!(
            repository
                .save(DeviceStoreRevision::ZERO, ready_authority())
                .is_err()
        );
        assert_eq!(
            repository.load().unwrap().revision,
            DeviceStoreRevision::ZERO
        );
    }

    #[test]
    fn controller_devices_store_enforces_device_bound() {
        let fixture = tempfile::tempdir().unwrap();
        let repository = ControllerDeviceRepository::open(fixture.path()).unwrap();
        let mut authority = ready_authority();
        authority.devices = (0..MAX_PAIRED_DEVICES).map(device).collect();
        let saved = repository
            .save(DeviceStoreRevision::ZERO, authority.clone())
            .unwrap();
        authority.devices.push(device(MAX_PAIRED_DEVICES));
        assert_eq!(
            repository.save(saved.revision, authority).unwrap_err(),
            ControllerDeviceStoreError::Domain(ControllerDeviceError::DeviceLimit)
        );
    }

    #[test]
    fn controller_devices_store_rejects_corrupt_newer_and_unsafe_documents() {
        let corrupt = tempfile::tempdir().unwrap();
        let repository = ControllerDeviceRepository::open(corrupt.path()).unwrap();
        fs::write(repository.metadata_path(), b"not-json").unwrap();
        assert_eq!(
            repository.load().unwrap_err(),
            ControllerDeviceStoreError::Corrupt
        );

        let newer = tempfile::tempdir().unwrap();
        let repository = ControllerDeviceRepository::open(newer.path()).unwrap();
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(repository.metadata_path()).unwrap()).unwrap();
        value["format_version"] = serde_json::json!(2);
        fs::write(
            repository.metadata_path(),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        assert_eq!(
            repository.load().unwrap_err(),
            ControllerDeviceStoreError::Newer {
                found: 2,
                supported: 1,
            }
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let unsafe_root = tempfile::tempdir().unwrap();
            let repository = ControllerDeviceRepository::open(unsafe_root.path()).unwrap();
            let target = repository.metadata_path();
            fs::remove_file(&target).unwrap();
            let outside = unsafe_root.path().join("outside");
            fs::write(&outside, b"sentinel").unwrap();
            symlink(&outside, &target).unwrap();
            assert_eq!(
                repository.load().unwrap_err(),
                ControllerDeviceStoreError::UnsafeEntry
            );
            assert_eq!(fs::read(outside).unwrap(), b"sentinel");
        }
    }

    #[cfg(unix)]
    #[test]
    fn controller_devices_store_uses_private_permissions() {
        use std::os::unix::fs::PermissionsExt as _;

        let fixture = tempfile::tempdir().unwrap();
        let repository = ControllerDeviceRepository::open(fixture.path()).unwrap();
        let directory_mode = fs::metadata(fixture.path()).unwrap().permissions().mode() & 0o777;
        let file_mode = fs::metadata(repository.metadata_path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(directory_mode, 0o700);
        assert_eq!(file_mode, 0o600);
    }
}
