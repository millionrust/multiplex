//! The Session Host's Unix platform: a user-only socket, `flock`ed slots, and the process group
//! a stop signals. See `docs/decisions/windows-session-host.md` for the Windows counterpart.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use multiplex_host_protocol::opaque_endpoint_name;
use portable_pty::MasterPty;
use tokio::net::{UnixListener, UnixStream};

use super::{HOST_SLOT_PREFIX, MAX_LIVE_HOSTS, StopSignal};
use crate::{HostError, HostErrorCode};

/// One connection from a client of this Host.
pub(super) type HostStream = UnixStream;

/// The process tree a Host owns: its child, which leads its own process group.
pub(super) struct OwnedProcess {
    process_group: i32,
}

impl OwnedProcess {
    /// The group the pseudo-terminal's child leads. A child that is not its group's leader could
    /// share the group with processes this Host does not own, so it is refused.
    pub(super) fn from_spawn(process_id: u32, master: &dyn MasterPty) -> Result<Self, HostError> {
        let process_group = master
            .process_group_leader()
            .ok_or_else(|| HostError::new(HostErrorCode::ProcessIdentityUnavailable))?;
        if process_group <= 0 || u32::try_from(process_group).ok() != Some(process_id) {
            return Err(HostError::new(HostErrorCode::ProcessIdentityUnavailable));
        }
        Ok(Self { process_group })
    }

    /// What a process token records as this tree's identity.
    pub(super) fn identity(&self) -> u64 {
        self.process_group as u64
    }

    pub(super) fn signal(&self, signal: StopSignal) -> Result<(), HostError> {
        let number = match signal {
            StopSignal::Interrupt => libc::SIGINT,
            StopSignal::Terminate => libc::SIGTERM,
            StopSignal::Kill => libc::SIGKILL,
        };
        let result = unsafe { libc::kill(-self.process_group, number) };
        if result == 0 {
            Ok(())
        } else {
            let error = io::Error::last_os_error();
            // No process left to signal means the process this stops is already gone. The
            // watcher may not have marked it exited yet, and failing on that timing reports a
            // stop that worked as an error to whoever asked for it.
            if error.raw_os_error() == Some(libc::ESRCH) {
                Ok(())
            } else {
                Err(HostError::io(error))
            }
        }
    }

    pub(super) fn leader_alive(&self) -> bool {
        (unsafe { libc::kill(self.process_group, 0) }) == 0
    }
}

pub(super) struct RuntimeHostSlot {
    file: File,
}

impl RuntimeHostSlot {
    pub(super) fn acquire(runtime_root: &Path) -> Result<Self, HostError> {
        for slot in 0..MAX_LIVE_HOSTS {
            let path = runtime_root.join(format!("{HOST_SLOT_PREFIX}{slot:02}.lock"));
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .mode(0o600)
                .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
                .open(path)
                .map_err(HostError::io)?;
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(HostError::io)?;
            let metadata = file.metadata().map_err(HostError::io)?;
            if !metadata.is_file()
                || metadata.uid() != unsafe { libc::geteuid() }
                || metadata.permissions().mode() & 0o777 != 0o600
            {
                return Err(HostError::new(HostErrorCode::PermissionDenied));
            }
            let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if result == 0 {
                return Ok(Self { file });
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::WouldBlock {
                return Err(HostError::io(error));
            }
        }
        Err(HostError::new(HostErrorCode::ResourceLimit))
    }
}

impl Drop for RuntimeHostSlot {
    fn drop(&mut self) {
        let _: i32 = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
    }
}

pub(super) struct UserOnlyListener {
    listener: UnixListener,
    socket_path: PathBuf,
    socket_device: u64,
    socket_inode: u64,
    expected_uid: u32,
}

impl UserOnlyListener {
    pub(super) fn bind(
        runtime_root: &Path,
        session_id: multiplex_domain::HostedSessionId,
    ) -> Result<Self, HostError> {
        prepare_runtime_root(runtime_root)?;
        let endpoint_name = opaque_endpoint_name(session_id);
        let socket_path = runtime_root.join(&endpoint_name);
        match fs::symlink_metadata(&socket_path) {
            Ok(_) => return Err(HostError::new(HostErrorCode::PermissionDenied)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(HostError::io(error)),
        }
        let listener = UnixListener::bind(&socket_path).map_err(HostError::io)?;
        #[cfg(unix)]
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))
            .map_err(HostError::io)?;
        let metadata = fs::symlink_metadata(&socket_path).map_err(HostError::io)?;
        if !metadata.file_type().is_socket()
            || metadata.permissions().mode() & 0o777 != 0o600
            || metadata.uid() != unsafe { libc::geteuid() }
        {
            return Err(HostError::new(HostErrorCode::PermissionDenied));
        }
        Ok(Self {
            listener,
            socket_path,
            socket_device: metadata.dev(),
            socket_inode: metadata.ino(),
            expected_uid: unsafe { libc::geteuid() },
        })
    }

    /// The next connection from this user, or `None` for one refused on its own: a peer that is not
    /// this user, or one already gone before it could be asked who it is. Neither says anything
    /// about the listener, and failing on them would turn every client away.
    pub(super) async fn accept(&self) -> Result<Option<UnixStream>, HostError> {
        let (stream, _) = match self.listener.accept().await {
            Ok(accepted) => accepted,
            Err(error) if error.kind() == io::ErrorKind::ConnectionAborted => return Ok(None),
            Err(error) => return Err(HostError::io(error)),
        };
        match stream.peer_cred() {
            Ok(credentials) if credentials.uid() == self.expected_uid => Ok(Some(stream)),
            _ => Ok(None),
        }
    }
}

impl Drop for UserOnlyListener {
    fn drop(&mut self) {
        if let Ok(metadata) = fs::symlink_metadata(&self.socket_path)
            && metadata.file_type().is_socket()
            && metadata.dev() == self.socket_device
            && metadata.ino() == self.socket_inode
        {
            let _ = fs::remove_file(&self.socket_path);
        }
    }
}

pub(super) fn prepare_runtime_root(root: &Path) -> Result<(), HostError> {
    match fs::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(HostError::new(HostErrorCode::PermissionDenied));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(root).map_err(HostError::io)?;
        }
        Err(error) => return Err(HostError::io(error)),
    }
    fs::set_permissions(root, fs::Permissions::from_mode(0o700)).map_err(HostError::io)?;
    let metadata = fs::symlink_metadata(root).map_err(HostError::io)?;
    if metadata.uid() != unsafe { libc::geteuid() }
        || metadata.permissions().mode() & 0o777 != 0o700
    {
        return Err(HostError::new(HostErrorCode::PermissionDenied));
    }
    Ok(())
}
