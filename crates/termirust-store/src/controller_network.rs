use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

use serde::{Deserialize, Serialize};
use termirust_domain::{ControllerListenPolicy, ControllerNetworkError, ControllerNetworkRevision};

use crate::{AtomicWriter, Durability, SystemAtomicWriter, file_lock};

const CONTROLLER_NETWORK_FILE: &str = "controller-network.json";
const CONTROLLER_NETWORK_LOCK_FILE: &str = "controller-network.lock";
const MAX_CONTROLLER_NETWORK_DOCUMENT_BYTES: u64 = 64 * 1024;
const CURRENT_CONTROLLER_NETWORK_FORMAT: u16 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ControllerNetworkSnapshot {
    pub revision: ControllerNetworkRevision,
    pub policy: ControllerListenPolicy,
    pub durability: Durability,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerNetworkStoreError {
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
        expected: ControllerNetworkRevision,
        actual: ControllerNetworkRevision,
    },
    RevisionOverflow,
    Domain(ControllerNetworkError),
}

impl fmt::Display for ControllerNetworkStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { operation, kind } => {
                write!(
                    formatter,
                    "controller network store {operation} failed ({kind:?})"
                )
            }
            Self::UnsafeEntry => {
                formatter.write_str("controller network store entry is not a safe regular file")
            }
            Self::TooLarge => formatter.write_str("controller network store exceeds its limit"),
            Self::Corrupt => formatter.write_str("controller network store is corrupt"),
            Self::Newer { found, supported } => write!(
                formatter,
                "controller network store format {found} is newer than supported format {supported}"
            ),
            Self::StaleRevision { expected, actual } => write!(
                formatter,
                "controller network store revision is stale (expected {}, actual {})",
                expected.get(),
                actual.get()
            ),
            Self::RevisionOverflow => {
                formatter.write_str("controller network store revision overflow")
            }
            Self::Domain(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ControllerNetworkStoreError {}

impl From<ControllerNetworkError> for ControllerNetworkStoreError {
    fn from(error: ControllerNetworkError) -> Self {
        Self::Domain(error)
    }
}

#[derive(Clone)]
pub struct ControllerNetworkRepository {
    root: PathBuf,
    writer: Arc<dyn AtomicWriter>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ControllerNetworkDocument {
    format_version: u16,
    revision: ControllerNetworkRevision,
    policy: ControllerListenPolicy,
}

impl Default for ControllerNetworkDocument {
    fn default() -> Self {
        Self {
            format_version: CURRENT_CONTROLLER_NETWORK_FORMAT,
            revision: ControllerNetworkRevision::ZERO,
            policy: ControllerListenPolicy::default(),
        }
    }
}

impl ControllerNetworkRepository {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, ControllerNetworkStoreError> {
        Self::open_with(root, Arc::new(SystemAtomicWriter))
    }

    pub fn open_with(
        root: impl Into<PathBuf>,
        writer: Arc<dyn AtomicWriter>,
    ) -> Result<Self, ControllerNetworkStoreError> {
        let repository = Self {
            root: root.into(),
            writer,
        };
        repository.ensure_root()?;
        let _lock = repository.acquire_lock()?;
        if !repository.path().exists() {
            repository.write_document(&ControllerNetworkDocument::default())?;
        }
        Ok(repository)
    }

    pub fn load(&self) -> Result<ControllerNetworkSnapshot, ControllerNetworkStoreError> {
        let _lock = self.acquire_lock()?;
        let document = self.read_document()?;
        Ok(ControllerNetworkSnapshot {
            revision: document.revision,
            policy: document.policy,
            durability: Durability::Full,
        })
    }

    pub fn save(
        &self,
        expected: ControllerNetworkRevision,
        policy: ControllerListenPolicy,
    ) -> Result<ControllerNetworkSnapshot, ControllerNetworkStoreError> {
        policy.validate()?;
        let _lock = self.acquire_lock()?;
        let current = self.read_document()?;
        if current.revision != expected {
            return Err(ControllerNetworkStoreError::StaleRevision {
                expected,
                actual: current.revision,
            });
        }
        let revision = current
            .revision
            .next()
            .ok_or(ControllerNetworkStoreError::RevisionOverflow)?;
        let document = ControllerNetworkDocument {
            format_version: CURRENT_CONTROLLER_NETWORK_FORMAT,
            revision,
            policy,
        };
        let durability = self.write_document(&document)?;
        Ok(ControllerNetworkSnapshot {
            revision,
            policy: document.policy,
            durability,
        })
    }

    pub fn reset_after_corruption(
        &self,
    ) -> Result<ControllerNetworkSnapshot, ControllerNetworkStoreError> {
        let _lock = self.acquire_lock()?;
        match self.read_document() {
            Ok(document) => {
                return Ok(ControllerNetworkSnapshot {
                    revision: document.revision,
                    policy: document.policy,
                    durability: Durability::Full,
                });
            }
            Err(ControllerNetworkStoreError::Corrupt | ControllerNetworkStoreError::TooLarge) => {}
            Err(error) => return Err(error),
        }
        self.write_document(&ControllerNetworkDocument::default())?;
        let document = self.read_document()?;
        Ok(ControllerNetworkSnapshot {
            revision: document.revision,
            policy: document.policy,
            durability: Durability::Full,
        })
    }

    pub fn metadata_path(&self) -> PathBuf {
        self.path()
    }

    fn path(&self) -> PathBuf {
        self.root.join(CONTROLLER_NETWORK_FILE)
    }

    fn ensure_root(&self) -> Result<(), ControllerNetworkStoreError> {
        match fs::symlink_metadata(&self.root) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(ControllerNetworkStoreError::UnsafeEntry);
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

    fn acquire_lock(&self) -> Result<ControllerNetworkStoreLock, ControllerNetworkStoreError> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        options.mode(0o600);
        let file = options
            .open(self.root.join(CONTROLLER_NETWORK_LOCK_FILE))
            .map_err(|error| io_error("open lock", error))?;
        file_lock::exclusive(&file).map_err(|error| io_error("lock", error))?;
        Ok(ControllerNetworkStoreLock { file })
    }

    fn read_document(&self) -> Result<ControllerNetworkDocument, ControllerNetworkStoreError> {
        let path = self.path();
        reject_unsafe_target(&path)?;
        let metadata = fs::metadata(&path).map_err(|error| io_error("metadata", error))?;
        if metadata.len() > MAX_CONTROLLER_NETWORK_DOCUMENT_BYTES {
            return Err(ControllerNetworkStoreError::TooLarge);
        }
        let bytes = fs::read(path).map_err(|error| io_error("read", error))?;
        let document: ControllerNetworkDocument =
            serde_json::from_slice(&bytes).map_err(|_| ControllerNetworkStoreError::Corrupt)?;
        if document.format_version > CURRENT_CONTROLLER_NETWORK_FORMAT {
            return Err(ControllerNetworkStoreError::Newer {
                found: document.format_version,
                supported: CURRENT_CONTROLLER_NETWORK_FORMAT,
            });
        }
        document.policy.validate()?;
        Ok(document)
    }

    fn write_document(
        &self,
        document: &ControllerNetworkDocument,
    ) -> Result<Durability, ControllerNetworkStoreError> {
        document.policy.validate()?;
        let bytes = serde_json::to_vec_pretty(document)
            .map_err(|_| ControllerNetworkStoreError::Corrupt)?;
        if bytes.len() as u64 > MAX_CONTROLLER_NETWORK_DOCUMENT_BYTES {
            return Err(ControllerNetworkStoreError::TooLarge);
        }
        self.writer
            .write(&self.path(), &bytes)
            .map_err(|error| io_error("write", error))
    }
}

struct ControllerNetworkStoreLock {
    file: File,
}

impl Drop for ControllerNetworkStoreLock {
    fn drop(&mut self) {
        file_lock::release(&self.file);
    }
}

fn reject_unsafe_target(path: &Path) -> Result<(), ControllerNetworkStoreError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| io_error("inspect", error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(ControllerNetworkStoreError::UnsafeEntry);
    }
    Ok(())
}

fn io_error(operation: &'static str, error: io::Error) -> ControllerNetworkStoreError {
    ControllerNetworkStoreError::Io {
        operation,
        kind: error.kind(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;
    use termirust_domain::{AddressFamily, ControllerPort, DiscoveryPolicy, NetworkInterfaceId};

    fn policy(enabled: bool) -> ControllerListenPolicy {
        ControllerListenPolicy {
            enabled,
            interface_id: Some(NetworkInterfaceId::new("4:en0").unwrap()),
            address_family: Some(AddressFamily::Ipv4),
            selected_address: Some(IpAddr::V4(std::net::Ipv4Addr::new(192, 168, 1, 9))),
            port: Some(ControllerPort::Generated(55_555)),
            discovery: DiscoveryPolicy::Off,
        }
    }

    #[test]
    fn network_policy_defaults_off_persists_and_rejects_stale_revision() {
        let fixture = tempfile::tempdir().unwrap();
        let first = ControllerNetworkRepository::open(fixture.path()).unwrap();
        let second = ControllerNetworkRepository::open(fixture.path()).unwrap();
        assert_eq!(
            first.load().unwrap().policy,
            ControllerListenPolicy::default()
        );

        let saved = first
            .save(ControllerNetworkRevision::ZERO, policy(true))
            .unwrap();
        assert_eq!(
            second
                .save(ControllerNetworkRevision::ZERO, policy(false))
                .unwrap_err(),
            ControllerNetworkStoreError::StaleRevision {
                expected: ControllerNetworkRevision::ZERO,
                actual: saved.revision,
            }
        );
        assert!(
            ControllerNetworkRepository::open(fixture.path())
                .unwrap()
                .load()
                .unwrap()
                .policy
                .enabled
        );
    }

    #[test]
    fn network_store_rejects_invalid_corrupt_newer_and_unsafe_documents() {
        let invalid = tempfile::tempdir().unwrap();
        let repository = ControllerNetworkRepository::open(invalid.path()).unwrap();
        let incomplete = ControllerListenPolicy {
            enabled: true,
            ..ControllerListenPolicy::default()
        };
        assert!(matches!(
            repository.save(ControllerNetworkRevision::ZERO, incomplete),
            Err(ControllerNetworkStoreError::Domain(
                ControllerNetworkError::IncompletePolicy
            ))
        ));

        fs::write(repository.metadata_path(), b"not-json").unwrap();
        assert_eq!(
            repository.load().unwrap_err(),
            ControllerNetworkStoreError::Corrupt
        );

        let newer = tempfile::tempdir().unwrap();
        let repository = ControllerNetworkRepository::open(newer.path()).unwrap();
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
            ControllerNetworkStoreError::Newer {
                found: 2,
                supported: 1,
            }
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let unsafe_root = tempfile::tempdir().unwrap();
            let repository = ControllerNetworkRepository::open(unsafe_root.path()).unwrap();
            let target = repository.metadata_path();
            fs::remove_file(&target).unwrap();
            let outside = unsafe_root.path().join("outside");
            fs::write(&outside, b"sentinel").unwrap();
            symlink(&outside, &target).unwrap();
            assert_eq!(
                repository.load().unwrap_err(),
                ControllerNetworkStoreError::UnsafeEntry
            );
            assert_eq!(fs::read(outside).unwrap(), b"sentinel");
        }
    }

    #[cfg(unix)]
    #[test]
    fn network_store_uses_private_permissions() {
        use std::os::unix::fs::PermissionsExt as _;

        let fixture = tempfile::tempdir().unwrap();
        let repository = ControllerNetworkRepository::open(fixture.path()).unwrap();
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
    }
}
