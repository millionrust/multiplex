use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Seek as _, Write as _};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::fd::AsRawFd as _;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _};

use serde::{Deserialize, Serialize};
use termirust_domain::{
    DurabilityWatermark, HostInstanceId, HostLifecycle, HostedSessionId, ProcessToken,
};

use crate::{AtomicWriter, Durability, SystemAtomicWriter};

const LOCK_FILE: &str = "host.lock";
const HOST_FILE: &str = "host.json";
const MAX_HOST_METADATA_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeaseErrorCode {
    Busy,
    PermissionDenied,
    UnsafeEntry,
    InvalidMetadata,
    Io,
}

#[derive(Debug)]
pub struct LeaseError {
    pub code: LeaseErrorCode,
    pub io_kind: Option<io::ErrorKind>,
}

impl LeaseError {
    fn new(code: LeaseErrorCode) -> Self {
        Self {
            code,
            io_kind: None,
        }
    }

    fn io(error: io::Error) -> Self {
        let code = match error.kind() {
            io::ErrorKind::PermissionDenied => LeaseErrorCode::PermissionDenied,
            io::ErrorKind::WouldBlock => LeaseErrorCode::Busy,
            _ => LeaseErrorCode::Io,
        };
        Self {
            code,
            io_kind: Some(error.kind()),
        }
    }
}

impl fmt::Display for LeaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.code {
            LeaseErrorCode::Busy => "session Host lease is already held",
            LeaseErrorCode::PermissionDenied => "session Host lease permission was denied",
            LeaseErrorCode::UnsafeEntry => "session Host storage contains an unsafe entry",
            LeaseErrorCode::InvalidMetadata => "session Host metadata is invalid",
            LeaseErrorCode::Io => "session Host lease I/O failed",
        })
    }
}

impl std::error::Error for LeaseError {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostMetadata {
    pub format_version: u16,
    pub session_id: HostedSessionId,
    pub host_instance_id: HostInstanceId,
    pub process_token: Option<ProcessToken>,
    pub lifecycle: HostLifecycle,
    pub endpoint_name: String,
    pub heartbeat_monotonic_nanos: u64,
    pub durability_watermark: Option<DurabilityWatermark>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReconciliationResult {
    Active,
    Orphaned,
    Exited,
}

impl HostMetadata {
    pub const FORMAT_VERSION: u16 = 1;

    pub fn validate(&self, expected_host: HostInstanceId) -> Result<(), LeaseError> {
        if self.format_version != Self::FORMAT_VERSION
            || self.host_instance_id != expected_host
            || self.endpoint_name.is_empty()
            || self.endpoint_name.len() > 128
            || self.endpoint_name.contains('/')
            || self
                .process_token
                .is_some_and(|token| !token.belongs_to(expected_host))
        {
            return Err(LeaseError::new(LeaseErrorCode::InvalidMetadata));
        }
        Ok(())
    }
}

pub struct HostLease {
    file: File,
    session_dir: PathBuf,
    host_instance_id: HostInstanceId,
}

impl fmt::Debug for HostLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HostLease")
            .field("session_dir", &"[REDACTED]")
            .field("host_instance_id", &self.host_instance_id)
            .finish()
    }
}

impl HostLease {
    pub fn acquire(
        session_dir: impl Into<PathBuf>,
        host_instance_id: HostInstanceId,
    ) -> Result<Self, LeaseError> {
        let session_dir = session_dir.into();
        prepare_user_only_directory(&session_dir)?;
        let lock_path = session_dir.join(LOCK_FILE);
        reject_unsafe_file_if_present(&lock_path)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&lock_path).map_err(LeaseError::io)?;
        lock_exclusive_nonblocking(&file)?;
        file.set_len(0).map_err(LeaseError::io)?;
        file.rewind().map_err(LeaseError::io)?;
        file.write_all(host_instance_id.to_string().as_bytes())
            .map_err(LeaseError::io)?;
        file.sync_data().map_err(LeaseError::io)?;
        Ok(Self {
            file,
            session_dir,
            host_instance_id,
        })
    }

    pub fn host_instance_id(&self) -> HostInstanceId {
        self.host_instance_id
    }

    pub fn session_dir(&self) -> &Path {
        &self.session_dir
    }

    pub fn write_metadata(&self, metadata: &HostMetadata) -> Result<Durability, LeaseError> {
        metadata.validate(self.host_instance_id)?;
        let bytes = serde_json::to_vec_pretty(metadata)
            .map_err(|_| LeaseError::new(LeaseErrorCode::InvalidMetadata))?;
        if bytes.len() > MAX_HOST_METADATA_BYTES {
            return Err(LeaseError::new(LeaseErrorCode::InvalidMetadata));
        }
        SystemAtomicWriter
            .write(&self.session_dir.join(HOST_FILE), &bytes)
            .map_err(LeaseError::io)
    }
}

pub fn read_host_metadata(session_dir: &Path) -> Result<HostMetadata, LeaseError> {
    let path = session_dir.join(HOST_FILE);
    let metadata = fs::symlink_metadata(&path).map_err(LeaseError::io)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_HOST_METADATA_BYTES as u64
    {
        return Err(LeaseError::new(LeaseErrorCode::UnsafeEntry));
    }
    let bytes = fs::read(path).map_err(LeaseError::io)?;
    let host: HostMetadata = serde_json::from_slice(&bytes)
        .map_err(|_| LeaseError::new(LeaseErrorCode::InvalidMetadata))?;
    host.validate(host.host_instance_id)?;
    Ok(host)
}

pub fn reconcile_host(session_dir: &Path) -> Result<ReconciliationResult, LeaseError> {
    let mut metadata = read_host_metadata(session_dir)?;
    let lease = match HostLease::acquire(session_dir, metadata.host_instance_id) {
        Ok(lease) => lease,
        Err(error) if error.code == LeaseErrorCode::Busy => {
            return Ok(ReconciliationResult::Active);
        }
        Err(error) => return Err(error),
    };
    if metadata.lifecycle == HostLifecycle::Exited {
        return Ok(ReconciliationResult::Exited);
    }
    metadata.lifecycle = HostLifecycle::Orphaned;
    lease.write_metadata(&metadata)?;
    Ok(ReconciliationResult::Orphaned)
}

impl Drop for HostLease {
    fn drop(&mut self) {
        unlock(&self.file);
    }
}

fn prepare_user_only_directory(path: &Path) -> Result<(), LeaseError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(LeaseError::new(LeaseErrorCode::UnsafeEntry));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(path).map_err(LeaseError::io)?;
        }
        Err(error) => return Err(LeaseError::io(error)),
    }
    #[cfg(unix)]
    {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(LeaseError::io)?;
        let metadata = fs::symlink_metadata(path).map_err(LeaseError::io)?;
        if metadata.uid() != unsafe { libc::geteuid() }
            || metadata.permissions().mode() & 0o777 != 0o700
        {
            return Err(LeaseError::new(LeaseErrorCode::PermissionDenied));
        }
    }
    Ok(())
}

fn reject_unsafe_file_if_present(path: &Path) -> Result<(), LeaseError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(LeaseError::new(LeaseErrorCode::UnsafeEntry))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(LeaseError::io(error)),
    }
}

#[cfg(unix)]
fn lock_exclusive_nonblocking(file: &File) -> Result<(), LeaseError> {
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        Ok(())
    } else {
        Err(LeaseError::io(io::Error::last_os_error()))
    }
}

#[cfg(not(unix))]
fn lock_exclusive_nonblocking(_: &File) -> Result<(), LeaseError> {
    Err(LeaseError::new(LeaseErrorCode::PermissionDenied))
}

#[cfg(unix)]
fn unlock(file: &File) {
    let _: i32 = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
}

#[cfg(not(unix))]
fn unlock(_: &File) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn lease_is_exclusive_user_only_and_metadata_is_bounded() {
        let fixture = tempfile::tempdir().unwrap();
        let session_dir = fixture.path().join("session");
        let host = HostInstanceId::new();
        let lease = HostLease::acquire(&session_dir, host).unwrap();
        assert_eq!(
            fs::metadata(&session_dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            HostLease::acquire(&session_dir, HostInstanceId::new())
                .unwrap_err()
                .code,
            LeaseErrorCode::Busy
        );
        let metadata = HostMetadata {
            format_version: HostMetadata::FORMAT_VERSION,
            session_id: HostedSessionId::new(),
            host_instance_id: host,
            process_token: None,
            lifecycle: HostLifecycle::Starting,
            endpoint_name: "opaque".to_string(),
            heartbeat_monotonic_nanos: 1,
            durability_watermark: None,
        };
        lease.write_metadata(&metadata).unwrap();
        assert_eq!(
            fs::metadata(session_dir.join(HOST_FILE))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn lease_rejects_symlink_session_directory() {
        let fixture = tempfile::tempdir().unwrap();
        let target = fixture.path().join("target");
        fs::create_dir(&target).unwrap();
        let alias = fixture.path().join("alias");
        symlink(target, &alias).unwrap();
        assert_eq!(
            HostLease::acquire(alias, HostInstanceId::new())
                .unwrap_err()
                .code,
            LeaseErrorCode::UnsafeEntry
        );
    }

    #[test]
    fn reconciliation_marks_lost_live_lease_orphaned_without_process_action() {
        let fixture = tempfile::tempdir().unwrap();
        let session_dir = fixture.path().join("session");
        let host = HostInstanceId::new();
        let lease = HostLease::acquire(&session_dir, host).unwrap();
        lease
            .write_metadata(&HostMetadata {
                format_version: HostMetadata::FORMAT_VERSION,
                session_id: HostedSessionId::new(),
                host_instance_id: host,
                process_token: Some(ProcessToken::new(host, 999_999, 1)),
                lifecycle: HostLifecycle::Ready,
                endpoint_name: "opaque".to_string(),
                heartbeat_monotonic_nanos: 1,
                durability_watermark: None,
            })
            .unwrap();
        assert_eq!(
            reconcile_host(&session_dir).unwrap(),
            ReconciliationResult::Active
        );
        drop(lease);
        assert_eq!(
            reconcile_host(&session_dir).unwrap(),
            ReconciliationResult::Orphaned
        );
        assert_eq!(
            read_host_metadata(&session_dir).unwrap().lifecycle,
            HostLifecycle::Orphaned
        );
    }
}
