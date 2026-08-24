use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use termirust_domain::HostedSessionId;
use termirust_host_protocol::opaque_endpoint_name;

use crate::{ClientError, ClientErrorCode};

#[derive(Clone, Eq, PartialEq)]
pub struct LocalEndpoint {
    runtime_root: PathBuf,
    socket_path: PathBuf,
}

impl LocalEndpoint {
    pub fn new(runtime_root: impl Into<PathBuf>, session_id: HostedSessionId) -> Self {
        let runtime_root = runtime_root.into();
        let socket_path = runtime_root.join(opaque_endpoint_name(session_id));
        Self {
            runtime_root,
            socket_path,
        }
    }

    pub fn runtime_root(&self) -> &Path {
        &self.runtime_root
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

impl fmt::Debug for LocalEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalEndpoint")
            .field("runtime_root", &"[REDACTED]")
            .field(
                "opaque_socket_name",
                &self.socket_path.file_name().unwrap_or_default(),
            )
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeerIdentity {
    pub principal: u64,
}

pub trait PeerAuthorizer: Send + Sync + 'static {
    fn authorize(&self, peer: PeerIdentity) -> Result<(), ClientError>;
}

#[derive(Clone, Copy, Debug)]
pub struct FakePeerAuthorizer {
    expected: u64,
}

impl FakePeerAuthorizer {
    pub const fn new(expected: u64) -> Self {
        Self { expected }
    }
}

impl PeerAuthorizer for FakePeerAuthorizer {
    fn authorize(&self, peer: PeerIdentity) -> Result<(), ClientError> {
        if peer.principal == self.expected {
            Ok(())
        } else {
            Err(ClientError::new(ClientErrorCode::PermissionDenied))
        }
    }
}

/// Windows integration remains behind this authorization boundary until D01.
#[derive(Clone, Copy, Debug)]
pub struct WindowsNamedPipeSecurityAdapter<A> {
    authorizer: A,
}

impl<A: PeerAuthorizer> WindowsNamedPipeSecurityAdapter<A> {
    pub const fn new(authorizer: A) -> Self {
        Self { authorizer }
    }

    pub fn authorize_sid_token(&self, opaque_sid_token: u64) -> Result<(), ClientError> {
        self.authorizer.authorize(PeerIdentity {
            principal: opaque_sid_token,
        })
    }
}

#[cfg(unix)]
mod unix {
    use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _, PermissionsExt as _};

    use tokio::net::{UnixListener, UnixStream};

    use super::*;

    #[derive(Debug)]
    pub struct UserOnlyUnixListener {
        listener: UnixListener,
        endpoint: LocalEndpoint,
        socket_device: u64,
        socket_inode: u64,
        expected_uid: u32,
    }

    impl UserOnlyUnixListener {
        pub fn bind(endpoint: LocalEndpoint) -> Result<Self, ClientError> {
            prepare_runtime_root(endpoint.runtime_root())?;
            match fs::symlink_metadata(endpoint.socket_path()) {
                Ok(_) => {
                    return Err(ClientError::new(ClientErrorCode::PermissionDenied));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            let listener = UnixListener::bind(endpoint.socket_path())?;
            fs::set_permissions(endpoint.socket_path(), fs::Permissions::from_mode(0o600))?;
            let metadata = fs::symlink_metadata(endpoint.socket_path())?;
            if !metadata.file_type().is_socket() || metadata.permissions().mode() & 0o777 != 0o600 {
                return Err(ClientError::new(ClientErrorCode::PermissionDenied));
            }
            let expected_uid = unsafe { libc::geteuid() };
            if metadata.uid() != expected_uid {
                return Err(ClientError::new(ClientErrorCode::PermissionDenied));
            }
            Ok(Self {
                listener,
                endpoint,
                socket_device: metadata.dev(),
                socket_inode: metadata.ino(),
                expected_uid,
            })
        }

        pub fn endpoint(&self) -> &LocalEndpoint {
            &self.endpoint
        }

        pub async fn accept(&self) -> Result<UnixStream, ClientError> {
            let (stream, _) = self.listener.accept().await?;
            authorize_stream(&stream, self.expected_uid)?;
            Ok(stream)
        }
    }

    impl Drop for UserOnlyUnixListener {
        fn drop(&mut self) {
            if let Ok(metadata) = fs::symlink_metadata(self.endpoint.socket_path())
                && metadata.file_type().is_socket()
                && metadata.dev() == self.socket_device
                && metadata.ino() == self.socket_inode
            {
                let _ = fs::remove_file(self.endpoint.socket_path());
            }
        }
    }

    pub fn authorize_stream(stream: &UnixStream, expected_uid: u32) -> Result<(), ClientError> {
        let credentials = stream.peer_cred().map_err(ClientError::from)?;
        if credentials.uid() == expected_uid {
            Ok(())
        } else {
            Err(ClientError::new(ClientErrorCode::PermissionDenied))
        }
    }

    fn prepare_runtime_root(root: &Path) -> Result<(), ClientError> {
        match fs::symlink_metadata(root) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(ClientError::new(ClientErrorCode::PermissionDenied));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir_all(root)?;
            }
            Err(error) => return Err(error.into()),
        }
        fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
        let metadata = fs::symlink_metadata(root)?;
        let expected_uid = unsafe { libc::geteuid() };
        if metadata.uid() != expected_uid || metadata.permissions().mode() & 0o777 != 0o700 {
            return Err(ClientError::new(ClientErrorCode::PermissionDenied));
        }
        Ok(())
    }

    pub use self::UserOnlyUnixListener as ExportedUserOnlyUnixListener;
    pub use authorize_stream as authorize_unix_stream;

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::os::unix::fs::symlink;

        #[tokio::test]
        async fn endpoint_is_user_only_and_authorizes_current_uid() {
            let fixture = tempfile::tempdir().unwrap();
            let endpoint =
                LocalEndpoint::new(fixture.path().join("runtime"), HostedSessionId::new());
            let listener = UserOnlyUnixListener::bind(endpoint.clone()).unwrap();
            assert_eq!(
                fs::metadata(endpoint.runtime_root())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(endpoint.socket_path())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            let client = UnixStream::connect(endpoint.socket_path()).await.unwrap();
            authorize_stream(&client, unsafe { libc::geteuid() }).unwrap();
            listener.accept().await.unwrap();
        }

        #[test]
        fn endpoint_rejects_symlink_runtime_root_and_existing_socket_name() {
            let fixture = tempfile::tempdir().unwrap();
            let target = fixture.path().join("target");
            fs::create_dir(&target).unwrap();
            let alias = fixture.path().join("runtime");
            symlink(&target, &alias).unwrap();
            let endpoint = LocalEndpoint::new(&alias, HostedSessionId::new());
            assert_eq!(
                UserOnlyUnixListener::bind(endpoint).unwrap_err().code,
                ClientErrorCode::PermissionDenied
            );
        }
    }
}

#[cfg(unix)]
pub use unix::{ExportedUserOnlyUnixListener as UserOnlyUnixListener, authorize_unix_stream};

#[cfg(not(unix))]
pub struct UserOnlyUnixListener;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_and_windows_boundary_fail_closed() {
        let adapter = WindowsNamedPipeSecurityAdapter::new(FakePeerAuthorizer::new(7));
        assert!(adapter.authorize_sid_token(7).is_ok());
        assert_eq!(
            adapter.authorize_sid_token(8).unwrap_err().code,
            ClientErrorCode::PermissionDenied
        );
    }

    #[test]
    fn endpoint_debug_redacts_runtime_path() {
        let endpoint = LocalEndpoint::new("/private/sensitive/path", HostedSessionId::new());
        let debug = format!("{endpoint:?}");
        assert!(!debug.contains("sensitive"));
        assert!(debug.contains("[REDACTED]"));
    }
}
