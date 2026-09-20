//! One LAN listener per user at a time.
//!
//! The desktop app and the background Controller service both serve the same saved route,
//! so they must never bind it together. Whichever listener worker is serving holds an
//! exclusive lock on `controller-listener.lock` in the Controller store. The operating
//! system releases the lock when the holder exits for any reason, including a crash, so a
//! waiting listener always gets its turn.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::{ListenerError, ListenerErrorCode};

const LOCK_FILE: &str = "controller-listener.lock";
const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Held while a listener serves the route. Dropping it lets the next listener bind.
#[derive(Debug)]
pub struct ListenerOwnership {
    _file: File,
}

impl ListenerOwnership {
    pub fn lock_path(controller_root: &Path) -> PathBuf {
        controller_root.join(LOCK_FILE)
    }

    /// Takes ownership if no other listener holds it.
    pub fn try_acquire(controller_root: &Path) -> Result<Option<Self>, ListenerError> {
        let file = open_lock_file(&Self::lock_path(controller_root))?;
        if lock_exclusive_nonblocking(&file)? {
            Ok(Some(Self { _file: file }))
        } else {
            Ok(None)
        }
    }

    /// Waits up to `timeout` for the current holder to let go. A background service asked
    /// to yield releases within a second or two; anything longer is a real conflict.
    pub fn acquire_within(
        controller_root: &Path,
        timeout: Duration,
    ) -> Result<Self, ListenerError> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(ownership) = Self::try_acquire(controller_root)? {
                return Ok(ownership);
            }
            if Instant::now() >= deadline {
                return Err(ListenerError::new(ListenerErrorCode::PortConflict));
            }
            std::thread::sleep(LOCK_POLL_INTERVAL);
        }
    }
}

#[cfg(unix)]
fn open_lock_file(path: &Path) -> Result<File, ListenerError> {
    use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};

    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(ListenerError::from)?;
    let metadata = file.metadata().map_err(ListenerError::from)?;
    // SAFETY: geteuid has no preconditions and cannot fail.
    if !metadata.is_file() || metadata.uid() != unsafe { libc::geteuid() } {
        return Err(ListenerError::new(ListenerErrorCode::PermissionDenied));
    }
    Ok(file)
}

#[cfg(not(unix))]
fn open_lock_file(path: &Path) -> Result<File, ListenerError> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(ListenerError::from)
}

#[cfg(unix)]
fn lock_exclusive_nonblocking(file: &File) -> Result<bool, ListenerError> {
    use std::os::fd::AsRawFd as _;

    // SAFETY: the descriptor is owned by `file` and stays open for the call.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    if error.kind() == std::io::ErrorKind::WouldBlock {
        Ok(false)
    } else {
        Err(ListenerError::from(error))
    }
}

// The background service is Unix-only, so there is nothing to coordinate with elsewhere.
#[cfg(not(unix))]
fn lock_exclusive_nonblocking(_: &File) -> Result<bool, ListenerError> {
    Ok(true)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn only_one_listener_owns_the_route_and_release_hands_it_over() {
        let fixture = tempfile::tempdir().unwrap();
        let first = ListenerOwnership::try_acquire(fixture.path())
            .unwrap()
            .expect("the first listener owns the route");
        assert!(
            ListenerOwnership::try_acquire(fixture.path())
                .unwrap()
                .is_none()
        );
        assert_eq!(
            ListenerOwnership::acquire_within(fixture.path(), Duration::from_millis(120))
                .unwrap_err()
                .code,
            ListenerErrorCode::PortConflict
        );
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            drop(first);
        });
        ListenerOwnership::acquire_within(fixture.path(), Duration::from_secs(5))
            .expect("the waiting listener takes over once the holder lets go");
        releaser.join().unwrap();
    }

    #[test]
    fn a_crashed_holder_releases_the_route() {
        let fixture = tempfile::tempdir().unwrap();
        let lock = ListenerOwnership::lock_path(fixture.path());
        // A child process takes the lock and is killed without cleaning up.
        let mut child = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg("exec /usr/bin/lockf -k \"$1\" /bin/sleep 30")
            .arg("sh")
            .arg(&lock)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while ListenerOwnership::try_acquire(fixture.path())
            .unwrap()
            .is_some()
        {
            if Instant::now() >= deadline || !Path::new("/usr/bin/lockf").exists() {
                let _ = child.kill();
                let _ = child.wait();
                eprintln!("skipping: lockf is unavailable to hold the lock from another process");
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        child.kill().unwrap();
        child.wait().unwrap();
        ListenerOwnership::acquire_within(fixture.path(), Duration::from_secs(5))
            .expect("the kernel releases a dead holder's lock");
    }

    #[test]
    fn a_symlinked_lock_file_is_refused() {
        let fixture = tempfile::tempdir().unwrap();
        let elsewhere = fixture.path().join("elsewhere");
        std::fs::write(&elsewhere, b"").unwrap();
        std::os::unix::fs::symlink(&elsewhere, ListenerOwnership::lock_path(fixture.path()))
            .unwrap();
        assert!(ListenerOwnership::try_acquire(fixture.path()).is_err());
    }
}
