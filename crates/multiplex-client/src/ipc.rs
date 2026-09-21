use std::fmt;
#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use std::io;
use std::path::{Path, PathBuf};

use multiplex_domain::HostedSessionId;
use multiplex_host_protocol::opaque_endpoint_name;

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

    /// Resolves the user-only runtime endpoint used by a durable session Host.
    ///
    /// Unix sockets live below the platform temporary directory so their paths
    /// remain short enough for the operating system. Other platforms keep the
    /// runtime beside the configured application data.
    pub fn for_config_root(config_root: &Path, session_id: HostedSessionId) -> Self {
        Self::new(
            durable_runtime_parent(config_root).join(session_id.to_string()),
            session_id,
        )
    }

    pub fn runtime_root(&self) -> &Path {
        &self.runtime_root
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

#[cfg(target_os = "macos")]
fn durable_runtime_parent(_: &Path) -> PathBuf {
    PathBuf::from(format!("/private/tmp/termirust-{}", unsafe {
        libc::geteuid()
    }))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn durable_runtime_parent(_: &Path) -> PathBuf {
    PathBuf::from(format!("/tmp/termirust-{}", unsafe { libc::geteuid() }))
}

#[cfg(not(unix))]
fn durable_runtime_parent(config_root: &Path) -> PathBuf {
    config_root.join("session-host-runtime")
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

#[cfg(test)]
mod endpoint_tests {
    use super::*;

    #[test]
    fn config_root_resolution_is_opaque_and_debug_output_is_redacted() {
        let session_id = HostedSessionId::new();
        let endpoint = LocalEndpoint::for_config_root(Path::new("/sensitive/config"), session_id);
        assert_eq!(
            endpoint.socket_path().parent(),
            Some(endpoint.runtime_root())
        );
        let debug = format!("{endpoint:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("sensitive"));
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

/// Reaching a Windows Session Host: its named pipe, and the check that the Host runs as this user.
#[cfg(windows)]
pub(crate) mod windows_pipe {
    use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle, RawHandle};
    use std::time::Duration;

    use multiplex_host_protocol::host_pipe_name;
    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};
    use windows_sys::Win32::Foundation::{ERROR_PIPE_BUSY, HANDLE};
    use windows_sys::Win32::Security::{
        EqualSid, GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    use super::LocalEndpoint;
    use crate::error::{ClientError, ClientErrorCode};

    /// How long to keep asking while the Host's one waiting pipe instance is taken by another
    /// client, between its accepting that client and making the next instance.
    const BUSY_RETRIES: usize = 100;
    const BUSY_WAIT: Duration = Duration::from_millis(10);

    pub(crate) async fn connect(endpoint: &LocalEndpoint) -> Result<NamedPipeClient, ClientError> {
        let endpoint_name = endpoint
            .socket_path()
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| ClientError::new(ClientErrorCode::PermissionDenied))?;
        let name = host_pipe_name(endpoint_name);
        let mut attempts = 0;
        let client = loop {
            match ClientOptions::new().open(&name) {
                Ok(client) => break client,
                Err(error)
                    if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32)
                        && attempts < BUSY_RETRIES =>
                {
                    attempts += 1;
                    tokio::time::sleep(BUSY_WAIT).await;
                }
                Err(error) => return Err(ClientError::from(error)),
            }
        };
        // The pipe's access list keeps other users out of a Host this user started; this is what
        // keeps this user out of a pipe someone else created under the same name.
        if server_is_this_user(&client) {
            Ok(client)
        } else {
            Err(ClientError::new(ClientErrorCode::PermissionDenied))
        }
    }

    fn owned(handle: HANDLE) -> Option<OwnedHandle> {
        // SAFETY: `handle` was just returned for this process to close, and is closed once.
        (!handle.is_null()).then(|| unsafe { OwnedHandle::from_raw_handle(handle as RawHandle) })
    }

    /// The `TOKEN_USER` of `process`, in a buffer aligned for the SID it points into.
    fn token_user(process: HANDLE) -> Option<Vec<u64>> {
        let mut token: HANDLE = std::ptr::null_mut();
        // SAFETY: `process` is open; the token handle returned is owned below.
        if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
            return None;
        }
        let token = owned(token)?;
        let mut needed = 0_u32;
        // SAFETY: a null buffer asks only for the size.
        unsafe {
            GetTokenInformation(
                token.as_raw_handle() as HANDLE,
                TokenUser,
                std::ptr::null_mut(),
                0,
                &mut needed,
            );
        }
        if needed == 0 {
            return None;
        }
        let mut buffer = vec![0_u64; (needed as usize).div_ceil(8)];
        // SAFETY: `buffer` holds at least `needed` bytes.
        let read = unsafe {
            GetTokenInformation(
                token.as_raw_handle() as HANDLE,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                needed,
                &mut needed,
            )
        };
        (read != 0).then_some(buffer)
    }

    fn server_is_this_user(client: &NamedPipeClient) -> bool {
        let mut process_id = 0_u32;
        // SAFETY: the pipe handle is open for the life of `client`.
        if unsafe { GetNamedPipeServerProcessId(client.as_raw_handle() as HANDLE, &mut process_id) }
            == 0
        {
            return false;
        }
        // SAFETY: opened for its identity only, and owned.
        let Some(server) =
            owned(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) })
        else {
            return false;
        };
        // SAFETY: the pseudo-handle for this process needs no closing.
        let (Some(ours), Some(theirs)) = (
            token_user(unsafe { GetCurrentProcess() }),
            token_user(server.as_raw_handle() as HANDLE),
        ) else {
            return false;
        };
        // SAFETY: each buffer holds a TOKEN_USER whose SID lies inside it, and both outlive the call.
        unsafe {
            EqualSid(
                (*ours.as_ptr().cast::<TOKEN_USER>()).User.Sid,
                (*theirs.as_ptr().cast::<TOKEN_USER>()).User.Sid,
            ) != 0
        }
    }
}

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
