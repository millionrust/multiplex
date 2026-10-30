//! A bounded raw-input reader whose worker exits even if stdin stays open.

use std::io::{self, Read};
use std::os::fd::{AsRawFd, BorrowedFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

use tokio::sync::mpsc;

pub(super) struct RawStdin {
    receiver: mpsc::Receiver<Vec<u8>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl RawStdin {
    pub(super) fn stdin() -> io::Result<Self> {
        // Duplicate ownership, without changing stdin's flags or closing the original fd.
        // SAFETY: fd 0 is the process's standard input; cloning reports an invalid fd.
        let input = unsafe { BorrowedFd::borrow_raw(libc::STDIN_FILENO) }.try_clone_to_owned()?;
        Self::start(std::fs::File::from(input))
    }

    fn start(mut input: impl Read + AsRawFd + Send + 'static) -> io::Result<Self> {
        let (sender, receiver) = mpsc::channel(8);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = thread::Builder::new()
            .name("terminal-stdin".into())
            .spawn(move || {
                let mut bytes = [0; 16 * 1024];
                while !worker_stop.load(Ordering::Acquire) {
                    let mut fd = libc::pollfd {
                        fd: input.as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    };
                    // A finite poll lets shutdown join an idle reader without another key.
                    // SAFETY: fd points to one initialized pollfd for this call's lifetime.
                    let ready = unsafe { libc::poll(&mut fd, 1, 25) };
                    if ready == 0 {
                        continue;
                    }
                    if ready < 0 {
                        if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                            continue;
                        }
                        break;
                    }
                    match input.read(&mut bytes) {
                        Ok(0) => break,
                        Ok(count) => {
                            if sender.blocking_send(bytes[..count].to_vec()).is_err() {
                                break;
                            }
                        }
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }
            })?;
        Ok(Self {
            receiver,
            stop,
            worker: Some(worker),
        })
    }

    pub(super) async fn recv(&mut self) -> Option<Vec<u8>> {
        self.receiver.recv().await
    }
}

impl Drop for RawStdin {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // Wake a worker blocked on a full queue before joining it.
        self.receiver.close();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant};

    #[tokio::test]
    async fn repeated_idle_readers_release_their_workers_and_file_descriptors() {
        let started = Instant::now();
        for _ in 0..100 {
            let (reader, mut writer) = UnixStream::pair().unwrap();
            writer
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            let mut input = RawStdin::start(reader).unwrap();
            writer.write_all(b"ready").unwrap();
            assert_eq!(input.recv().await.unwrap(), b"ready");
            let worker_lifetime = Arc::downgrade(&input.stop);
            drop(input);
            assert!(worker_lifetime.upgrade().is_none(), "reader worker leaked");
            assert_eq!(writer.read(&mut [0]).unwrap(), 0, "reader fd leaked");
        }
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn dropping_a_full_input_queue_joins_the_blocked_worker() {
        let (reader, mut writer) = UnixStream::pair().unwrap();
        let input = RawStdin::start(reader).unwrap();
        let lifetime = Arc::downgrade(&input.stop);
        let writing = thread::spawn(move || writer.write_all(&vec![b'x'; 1024 * 1024]));
        let deadline = Instant::now() + Duration::from_secs(2);
        while input.receiver.len() < 8 {
            assert!(
                Instant::now() < deadline,
                "reader did not fill its bounded queue"
            );
            thread::sleep(Duration::from_millis(5));
        }
        drop(input);
        let _ = writing.join().unwrap();
        assert!(lifetime.upgrade().is_none(), "blocked reader worker leaked");
    }
}
