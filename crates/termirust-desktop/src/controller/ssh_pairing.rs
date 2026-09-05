#[cfg(unix)]
mod unix {
    use std::fs;
    use std::io::{BufReader, ErrorKind};
    use std::os::fd::AsRawFd as _;
    use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _, PermissionsExt as _};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    use termirust_controller_listener::{
        SshHostPairingDecision, SshHostPairingDecisionValue, SshHostPairingPrompt,
    };
    use termirust_domain::PairingOfferId;

    const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(25);
    const IO_TIMEOUT: Duration = Duration::from_secs(2);

    struct IncomingPairing {
        prompt: SshHostPairingPrompt,
        response: mpsc::Sender<SshHostPairingDecisionValue>,
    }

    struct PendingPairing {
        offer_id: PairingOfferId,
        response: mpsc::Sender<SshHostPairingDecisionValue>,
    }

    pub struct SshPairingBroker {
        socket_path: PathBuf,
        socket_device: u64,
        socket_inode: u64,
        incoming: mpsc::Receiver<IncomingPairing>,
        pending: Option<PendingPairing>,
        stopping: Arc<AtomicBool>,
        worker: Option<JoinHandle<()>>,
    }

    impl SshPairingBroker {
        pub fn bind(socket_path: PathBuf) -> std::io::Result<Self> {
            let parent = socket_path.parent().ok_or_else(|| {
                std::io::Error::new(ErrorKind::InvalidInput, "pairing socket has no parent")
            })?;
            prepare_user_only_directory(parent)?;
            remove_owned_stale_socket(&socket_path)?;
            let listener = UnixListener::bind(&socket_path)?;
            fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))?;
            let metadata = fs::symlink_metadata(&socket_path)?;
            let expected_uid = unsafe { libc::geteuid() };
            if !metadata.file_type().is_socket()
                || metadata.uid() != expected_uid
                || metadata.permissions().mode() & 0o777 != 0o600
            {
                return Err(std::io::Error::new(
                    ErrorKind::PermissionDenied,
                    "pairing socket is not user-only",
                ));
            }
            listener.set_nonblocking(true)?;
            let (incoming_tx, incoming) = mpsc::channel();
            let stopping = Arc::new(AtomicBool::new(false));
            let worker_stopping = stopping.clone();
            let worker = thread::Builder::new()
                .name("controller-ssh-pairing".into())
                .spawn(move || broker_loop(listener, expected_uid, incoming_tx, worker_stopping))?;
            Ok(Self {
                socket_path,
                socket_device: metadata.dev(),
                socket_inode: metadata.ino(),
                incoming,
                pending: None,
                stopping,
                worker: Some(worker),
            })
        }

        pub fn poll(&mut self) -> Option<SshHostPairingPrompt> {
            if self.pending.is_some() {
                return None;
            }
            let incoming = self.incoming.try_recv().ok()?;
            self.pending = Some(PendingPairing {
                offer_id: incoming.prompt.offer_id,
                response: incoming.response,
            });
            Some(incoming.prompt)
        }

        pub fn decide(
            &mut self,
            offer_id: PairingOfferId,
            decision: SshHostPairingDecisionValue,
        ) -> std::io::Result<()> {
            let pending = self.pending.take().ok_or_else(|| {
                std::io::Error::new(ErrorKind::NotFound, "no SSH pairing decision is pending")
            })?;
            if pending.offer_id != offer_id {
                self.pending = Some(pending);
                return Err(std::io::Error::new(
                    ErrorKind::PermissionDenied,
                    "SSH pairing offer does not match",
                ));
            }
            pending.response.send(decision).map_err(|_| {
                std::io::Error::new(ErrorKind::BrokenPipe, "SSH pairing request ended")
            })
        }

        pub fn pending_offer_id(&self) -> Option<PairingOfferId> {
            self.pending.as_ref().map(|pending| pending.offer_id)
        }
    }

    impl Drop for SshPairingBroker {
        fn drop(&mut self) {
            self.pending.take();
            self.stopping.store(true, Ordering::Release);
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
            if let Ok(metadata) = fs::symlink_metadata(&self.socket_path)
                && metadata.file_type().is_socket()
                && metadata.dev() == self.socket_device
                && metadata.ino() == self.socket_inode
            {
                let _ = fs::remove_file(&self.socket_path);
            }
        }
    }

    fn broker_loop(
        listener: UnixListener,
        expected_uid: u32,
        incoming: mpsc::Sender<IncomingPairing>,
        stopping: Arc<AtomicBool>,
    ) {
        while !stopping.load(Ordering::Acquire) {
            let stream = match listener.accept() {
                Ok((stream, _)) => stream,
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    thread::sleep(ACCEPT_POLL_INTERVAL);
                    continue;
                }
                Err(_) => return,
            };
            if authorize_peer(&stream, expected_uid).is_err() {
                continue;
            }
            let _ = serve_pairing(stream, &incoming);
        }
    }

    fn serve_pairing(
        mut stream: UnixStream,
        incoming: &mpsc::Sender<IncomingPairing>,
    ) -> std::io::Result<()> {
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;
        let prompt = SshHostPairingPrompt::read(&mut BufReader::new(stream.try_clone()?))
            .map_err(protocol_error)?;
        let offer_id = prompt.offer_id;
        let (response, response_rx) = mpsc::channel();
        incoming
            .send(IncomingPairing { prompt, response })
            .map_err(|_| std::io::Error::new(ErrorKind::BrokenPipe, "pairing UI unavailable"))?;
        let decision = response_rx
            .recv()
            .map_err(|_| std::io::Error::new(ErrorKind::BrokenPipe, "pairing UI was closed"))?;
        SshHostPairingDecision::new(offer_id, decision)
            .write(&mut stream)
            .map_err(protocol_error)
    }

    fn protocol_error(error: termirust_controller_listener::ListenerError) -> std::io::Error {
        std::io::Error::new(ErrorKind::InvalidData, format!("{:?}", error.code))
    }

    fn prepare_user_only_directory(path: &Path) -> std::io::Result<()> {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(std::io::Error::new(
                    ErrorKind::PermissionDenied,
                    "pairing runtime root is not a trusted directory",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => fs::create_dir_all(path)?,
            Err(error) => return Err(error),
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        let metadata = fs::symlink_metadata(path)?;
        if metadata.uid() != unsafe { libc::geteuid() }
            || metadata.permissions().mode() & 0o777 != 0o700
        {
            return Err(std::io::Error::new(
                ErrorKind::PermissionDenied,
                "pairing runtime root is not user-only",
            ));
        }
        Ok(())
    }

    fn remove_owned_stale_socket(path: &Path) -> std::io::Result<()> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        if !metadata.file_type().is_socket() || metadata.uid() != unsafe { libc::geteuid() } {
            return Err(std::io::Error::new(
                ErrorKind::PermissionDenied,
                "refusing to replace untrusted pairing endpoint",
            ));
        }
        match UnixStream::connect(path) {
            Ok(_) => Err(std::io::Error::new(
                ErrorKind::AddrInUse,
                "another TermiRust instance owns the pairing endpoint",
            )),
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::ConnectionRefused | ErrorKind::NotFound
                ) =>
            {
                fs::remove_file(path)
            }
            Err(error) => Err(error),
        }
    }

    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
    fn authorize_peer(stream: &UnixStream, expected_uid: u32) -> std::io::Result<()> {
        let mut effective_uid = 0;
        let mut effective_gid = 0;
        let result =
            unsafe { libc::getpeereid(stream.as_raw_fd(), &mut effective_uid, &mut effective_gid) };
        if result == 0 && effective_uid == expected_uid {
            Ok(())
        } else {
            Err(std::io::Error::new(
                ErrorKind::PermissionDenied,
                "pairing peer is not the current user",
            ))
        }
    }

    #[cfg(target_os = "linux")]
    fn authorize_peer(stream: &UnixStream, expected_uid: u32) -> std::io::Result<()> {
        let mut credentials = libc::ucred {
            pid: 0,
            uid: 0,
            gid: 0,
        };
        let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        let result = unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut credentials as *mut libc::ucred as *mut libc::c_void,
                &mut length,
            )
        };
        if result == 0 && credentials.uid == expected_uid {
            Ok(())
        } else {
            Err(std::io::Error::new(
                ErrorKind::PermissionDenied,
                "pairing peer is not the current user",
            ))
        }
    }

    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "linux"
    )))]
    fn authorize_peer(_stream: &UnixStream, _expected_uid: u32) -> std::io::Result<()> {
        Err(std::io::Error::new(
            ErrorKind::Unsupported,
            "peer credential verification is unsupported",
        ))
    }
}

#[cfg(unix)]
pub use unix::SshPairingBroker;

#[cfg(all(test, unix))]
mod tests {
    use std::io::BufReader;
    use std::os::unix::net::UnixStream;
    use std::thread;
    use std::time::{Duration, Instant};

    use tempfile::tempdir;
    use termirust_controller_listener::{
        SshHostPairingDecision, SshHostPairingDecisionValue, SshHostPairingPrompt,
    };
    use termirust_domain::PairingOfferId;

    use super::SshPairingBroker;

    #[test]
    fn broker_delivers_one_matching_user_confirmed_decision() {
        let temp = tempdir().unwrap();
        let socket_path = temp.path().join("pairing.sock");
        let mut broker = SshPairingBroker::bind(socket_path.clone()).unwrap();
        let offer_id = PairingOfferId::new();
        let client = thread::spawn(move || {
            let mut stream = UnixStream::connect(socket_path).unwrap();
            SshHostPairingPrompt::new(offer_id, "ABCD-1234".into(), 500)
                .unwrap()
                .write(&mut stream)
                .unwrap();
            SshHostPairingDecision::read(&mut BufReader::new(stream)).unwrap()
        });

        let deadline = Instant::now() + Duration::from_secs(2);
        let prompt = loop {
            if let Some(prompt) = broker.poll() {
                break prompt;
            }
            assert!(
                Instant::now() < deadline,
                "pairing prompt was not delivered"
            );
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(prompt.offer_id, offer_id);
        assert_eq!(prompt.sas, "ABCD-1234");
        broker
            .decide(offer_id, SshHostPairingDecisionValue::Confirm)
            .unwrap();

        let decision = client.join().unwrap();
        assert_eq!(decision.offer_id, offer_id);
        assert_eq!(decision.decision, SshHostPairingDecisionValue::Confirm);
    }
}

#[cfg(not(unix))]
pub struct SshPairingBroker;

#[cfg(not(unix))]
impl SshPairingBroker {
    pub fn bind(_socket_path: std::path::PathBuf) -> std::io::Result<Self> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "SSH Host pairing is not available on this platform",
        ))
    }
}
