use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(unix)]
use std::os::fd::AsRawFd as _;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt as _;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;

use serde::{Deserialize, Serialize};
use termirust_domain::{NotificationLedger, NotificationPolicy, PermissionState, Revision};

use crate::{AtomicWriter, Durability, SystemAtomicWriter};

const NOTIFICATIONS_FILE: &str = "notifications.json";
const NOTIFICATIONS_LOCK_FILE: &str = "notifications.lock";
const MAX_NOTIFICATION_DOCUMENT_BYTES: u64 = 512 * 1024;
const CURRENT_NOTIFICATION_FORMAT: u16 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NotificationSnapshot {
    pub revision: Revision,
    pub policy: NotificationPolicy,
    pub ledger: NotificationLedger,
    pub durability: Durability,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NotificationStoreError {
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
}

impl fmt::Display for NotificationStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { operation, kind } => {
                write!(
                    formatter,
                    "notification store {operation} failed ({kind:?})"
                )
            }
            Self::UnsafeEntry => {
                formatter.write_str("notification store entry is not a safe regular file")
            }
            Self::TooLarge => formatter.write_str("notification store entry exceeds its limit"),
            Self::Corrupt => formatter.write_str("notification store entry is corrupt"),
            Self::Newer { found, supported } => write!(
                formatter,
                "notification store format {found} is newer than supported format {supported}"
            ),
            Self::StaleRevision { expected, actual } => write!(
                formatter,
                "notification store revision is stale (expected {}, actual {})",
                expected.get(),
                actual.get()
            ),
            Self::RevisionOverflow => formatter.write_str("notification revision overflow"),
        }
    }
}

impl std::error::Error for NotificationStoreError {}

#[derive(Clone)]
pub struct NotificationRepository {
    root: PathBuf,
    writer: Arc<dyn AtomicWriter>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct NotificationDocument {
    format_version: u16,
    revision: Revision,
    policy: NotificationPolicy,
    ledger: NotificationLedger,
}

impl Default for NotificationDocument {
    fn default() -> Self {
        Self {
            format_version: CURRENT_NOTIFICATION_FORMAT,
            revision: Revision::ZERO,
            policy: NotificationPolicy::default(),
            ledger: NotificationLedger::default(),
        }
    }
}

impl NotificationRepository {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, NotificationStoreError> {
        Self::open_with(root, Arc::new(SystemAtomicWriter))
    }

    pub fn open_with(
        root: impl Into<PathBuf>,
        writer: Arc<dyn AtomicWriter>,
    ) -> Result<Self, NotificationStoreError> {
        let repository = Self {
            root: root.into(),
            writer,
        };
        repository.ensure_root()?;
        let _lock = repository.acquire_lock()?;
        let target = repository.path();
        if !target.exists() {
            repository.write_document(&NotificationDocument::default())?;
        }
        Ok(repository)
    }

    pub fn load(&self) -> Result<NotificationSnapshot, NotificationStoreError> {
        let _lock = self.acquire_lock()?;
        self.load_locked()
    }

    fn load_locked(&self) -> Result<NotificationSnapshot, NotificationStoreError> {
        let document = self.read_document()?;
        Ok(NotificationSnapshot {
            revision: document.revision,
            policy: document.policy,
            ledger: document.ledger,
            durability: Durability::Full,
        })
    }

    pub fn save(
        &self,
        expected: Revision,
        policy: NotificationPolicy,
        ledger: NotificationLedger,
    ) -> Result<NotificationSnapshot, NotificationStoreError> {
        let _lock = self.acquire_lock()?;
        self.save_locked(expected, policy, ledger)
    }

    pub fn set_permission(
        &self,
        expected: Revision,
        permission: PermissionState,
    ) -> Result<NotificationSnapshot, NotificationStoreError> {
        let _lock = self.acquire_lock()?;
        let current = self.load_locked()?;
        let mut policy = current.policy;
        policy.permission = permission;
        self.save_locked(expected, policy, current.ledger)
    }

    pub fn reset_after_corruption(&self) -> Result<NotificationSnapshot, NotificationStoreError> {
        let _lock = self.acquire_lock()?;
        match self.read_document() {
            Ok(_) => return self.load_locked(),
            Err(NotificationStoreError::Corrupt | NotificationStoreError::TooLarge) => {}
            Err(error) => return Err(error),
        }
        let document = NotificationDocument::default();
        self.write_document(&document)?;
        self.load_locked()
    }

    fn save_locked(
        &self,
        expected: Revision,
        mut policy: NotificationPolicy,
        ledger: NotificationLedger,
    ) -> Result<NotificationSnapshot, NotificationStoreError> {
        let current = self.read_document()?;
        if current.revision != expected {
            return Err(NotificationStoreError::StaleRevision {
                expected,
                actual: current.revision,
            });
        }
        policy.normalize();
        ledger
            .validate()
            .map_err(|_| NotificationStoreError::Corrupt)?;
        let revision = current
            .revision
            .next()
            .ok_or(NotificationStoreError::RevisionOverflow)?;
        let document = NotificationDocument {
            format_version: CURRENT_NOTIFICATION_FORMAT,
            revision,
            policy,
            ledger,
        };
        let durability = self.write_document(&document)?;
        Ok(NotificationSnapshot {
            revision,
            policy,
            ledger: document.ledger,
            durability,
        })
    }

    fn path(&self) -> PathBuf {
        self.root.join(NOTIFICATIONS_FILE)
    }

    fn ensure_root(&self) -> Result<(), NotificationStoreError> {
        match fs::symlink_metadata(&self.root) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(NotificationStoreError::UnsafeEntry);
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

    fn acquire_lock(&self) -> Result<NotificationStoreLock, NotificationStoreError> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        options.mode(0o600);
        let file = options
            .open(self.root.join(NOTIFICATIONS_LOCK_FILE))
            .map_err(|error| io_error("open lock", error))?;
        #[cfg(unix)]
        {
            let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
            if result != 0 {
                return Err(io_error("lock", io::Error::last_os_error()));
            }
        }
        Ok(NotificationStoreLock { file })
    }

    fn read_document(&self) -> Result<NotificationDocument, NotificationStoreError> {
        let path = self.path();
        reject_unsafe_target(&path)?;
        let metadata = fs::metadata(&path).map_err(|error| io_error("metadata", error))?;
        if metadata.len() > MAX_NOTIFICATION_DOCUMENT_BYTES {
            return Err(NotificationStoreError::TooLarge);
        }
        let bytes = fs::read(&path).map_err(|error| io_error("read", error))?;
        let mut document: NotificationDocument =
            serde_json::from_slice(&bytes).map_err(|_| NotificationStoreError::Corrupt)?;
        if document.format_version > CURRENT_NOTIFICATION_FORMAT {
            return Err(NotificationStoreError::Newer {
                found: document.format_version,
                supported: CURRENT_NOTIFICATION_FORMAT,
            });
        }
        document.policy.normalize();
        document
            .ledger
            .validate()
            .map_err(|_| NotificationStoreError::Corrupt)?;
        Ok(document)
    }

    fn write_document(
        &self,
        document: &NotificationDocument,
    ) -> Result<Durability, NotificationStoreError> {
        let bytes =
            serde_json::to_vec_pretty(document).map_err(|_| NotificationStoreError::Corrupt)?;
        if bytes.len() as u64 > MAX_NOTIFICATION_DOCUMENT_BYTES {
            return Err(NotificationStoreError::TooLarge);
        }
        self.writer
            .write(&self.path(), &bytes)
            .map_err(|error| io_error("write", error))
    }
}

struct NotificationStoreLock {
    file: File,
}

impl Drop for NotificationStoreLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

fn reject_unsafe_target(path: &Path) -> Result<(), NotificationStoreError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| io_error("inspect", error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(NotificationStoreError::UnsafeEntry);
    }
    Ok(())
}

fn io_error(operation: &'static str, error: io::Error) -> NotificationStoreError {
    NotificationStoreError::Io {
        operation,
        kind: error.kind(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use termirust_domain::{
        ActivityState, HostSequence, HostedSessionId, NotificationClock, NotificationContext,
        NotificationEvent, NotificationMode, OccupantGeneration, SessionDeepLink,
        reduce_notification,
    };
    use uuid::Uuid;

    use super::*;

    #[derive(Default)]
    struct FailingWriter {
        calls: Mutex<usize>,
    }

    impl AtomicWriter for FailingWriter {
        fn write(&self, target: &Path, bytes: &[u8]) -> io::Result<Durability> {
            let mut calls = self.calls.lock().unwrap();
            *calls += 1;
            if *calls > 1 {
                return Err(io::Error::other("injected"));
            }
            SystemAtomicWriter.write(target, bytes)
        }
    }

    fn populated(snapshot: &NotificationSnapshot) -> NotificationLedger {
        let mut ledger = snapshot.ledger.clone();
        reduce_notification(
            &mut ledger,
            NotificationPolicy::default(),
            NotificationEvent {
                session_id: HostedSessionId::from_uuid(Uuid::from_u128(1)),
                generation: OccupantGeneration::new(1),
                activity: ActivityState::Done,
                activity_sequence: HostSequence::new(1),
            },
            "A session",
            SessionDeepLink::from_uuid(Uuid::from_u128(2)),
            NotificationContext {
                current_generation: OccupantGeneration::new(1),
                unread: true,
                visibly_focused: false,
            },
            NotificationClock {
                wall_millis: 10,
                monotonic_millis: 10,
                runtime_epoch_wall_millis: 0,
            },
        )
        .unwrap();
        ledger
    }

    #[test]
    fn notification_ledger_survives_restart_with_policy_and_dedupe_evidence() {
        let fixture = tempfile::tempdir().unwrap();
        let repository = NotificationRepository::open(fixture.path()).unwrap();
        let initial = repository.load().unwrap();
        let mut policy = initial.policy;
        policy.mode = NotificationMode::Os;
        policy.permission = PermissionState::Denied;
        let saved = repository
            .save(initial.revision, policy, populated(&initial))
            .unwrap();
        drop(repository);

        let reopened = NotificationRepository::open(fixture.path())
            .unwrap()
            .load()
            .unwrap();
        assert_eq!(reopened.revision, saved.revision);
        assert_eq!(reopened.policy, policy);
        assert_eq!(reopened.ledger.records.len(), 1);
    }

    #[test]
    fn notification_ledger_corruption_is_isolated_and_requires_explicit_reset() {
        let fixture = tempfile::tempdir().unwrap();
        let repository = NotificationRepository::open(fixture.path()).unwrap();
        fs::write(fixture.path().join(NOTIFICATIONS_FILE), b"not-json").unwrap();
        assert_eq!(
            repository.load().unwrap_err(),
            NotificationStoreError::Corrupt
        );
        let reset = repository.reset_after_corruption().unwrap();
        assert!(reset.ledger.records.is_empty());
        assert_eq!(reset.policy.permission, PermissionState::Unknown);
    }

    #[test]
    fn notification_ledger_failed_atomic_write_preserves_last_good_document() {
        let fixture = tempfile::tempdir().unwrap();
        let repository =
            NotificationRepository::open_with(fixture.path(), Arc::new(FailingWriter::default()))
                .unwrap();
        let initial = repository.load().unwrap();
        assert!(
            repository
                .save(initial.revision, initial.policy, populated(&initial))
                .is_err()
        );
        assert_eq!(repository.load().unwrap().revision, Revision::ZERO);
    }

    #[test]
    fn notification_ledger_serializes_writers_and_rejects_stale_revisions() {
        let fixture = tempfile::tempdir().unwrap();
        let first = NotificationRepository::open(fixture.path()).unwrap();
        let second = NotificationRepository::open(fixture.path()).unwrap();
        let first_snapshot = first.load().unwrap();
        let second_snapshot = second.load().unwrap();

        let saved = first
            .save(
                first_snapshot.revision,
                first_snapshot.policy,
                populated(&first_snapshot),
            )
            .unwrap();
        assert_eq!(
            second
                .save(
                    second_snapshot.revision,
                    second_snapshot.policy,
                    populated(&second_snapshot),
                )
                .unwrap_err(),
            NotificationStoreError::StaleRevision {
                expected: second_snapshot.revision,
                actual: saved.revision,
            }
        );
        assert_eq!(second.load().unwrap().ledger.records.len(), 1);
    }

    #[test]
    fn notification_ledger_rejects_unknown_fields_and_unsafe_targets() {
        let fixture = tempfile::tempdir().unwrap();
        let repository = NotificationRepository::open(fixture.path()).unwrap();
        let path = fixture.path().join(NOTIFICATIONS_FILE);
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value["unexpected"] = serde_json::json!(true);
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(
            repository.load().unwrap_err(),
            NotificationStoreError::Corrupt
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
                NotificationStoreError::UnsafeEntry
            );
            assert_eq!(fs::read(&outside).unwrap(), b"sentinel");
        }
    }
}
