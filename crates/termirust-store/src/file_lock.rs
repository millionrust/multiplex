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
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
            {
                thread::sleep(retry_interval);
            }
            Err(error) => return Err(error),
        }
    }
}

pub(crate) fn release(file: &File) {
    let _ = fs2::FileExt::unlock(file);
}
