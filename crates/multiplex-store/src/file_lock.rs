use std::fs::File;
use std::io;
use std::thread;
use std::time::{Duration, Instant};

pub(crate) fn exclusive(file: &File) -> io::Result<()> {
    fs2::FileExt::lock_exclusive(file)
}

pub(crate) fn shared(file: &File) -> io::Result<()> {
    fs2::FileExt::lock_shared(file)
}

pub(crate) fn exclusive_with_timeout(
    file: &File,
    timeout: Duration,
    retry_interval: Duration,
) -> io::Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        match fs2::FileExt::try_lock_exclusive(file) {
            Ok(()) => return Ok(()),
            Err(error) if contended(&error) && Instant::now() < deadline => {
                thread::sleep(retry_interval);
            }
            Err(error) => return Err(error),
        }
    }
}

/// Whether someone else holds this lock, which is worth waiting for rather than failing on. Locks
/// that queue report "would block"; Windows reports a lock violation, which has no kind of its
/// own, so fs2 names that error per platform.
fn contended(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::WouldBlock
        || (error.raw_os_error().is_some()
            && error.raw_os_error() == fs2::lock_contended_error().raw_os_error())
}

pub(crate) fn release(file: &File) {
    let _ = fs2::FileExt::unlock(file);
}
