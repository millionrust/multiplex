//! The Session Host's Windows platform: a named pipe only this user can open, lock files taken
//! with the standard library, and a job object holding the process tree a stop ends.
//!
//! Every choice here keeps one of the Unix Host's guarantees with the platform's own mechanism;
//! `docs/decisions/windows-session-host.md` lists them side by side.

use std::ffi::c_void;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::windows::fs::MetadataExt as _;
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle, RawHandle};
use std::path::{Path, PathBuf};

use multiplex_host_protocol::{host_pipe_name as pipe_name, opaque_endpoint_name};
use portable_pty::MasterPty;
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use tokio::sync::Mutex;
use windows_sys::Win32::Foundation::{
    ERROR_PIPE_CONNECTED, HANDLE, HLOCAL, LocalFree, STILL_ACTIVE,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    EqualSid, GetTokenInformation, PSID, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetExitCodeProcess, OpenProcess, OpenProcessToken,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
};

use super::{HOST_SLOT_PREFIX, MAX_LIVE_HOSTS, StopSignal};
use crate::{HostError, HostErrorCode};

/// One connection from a client of this Host.
pub(super) type HostStream = NamedPipeServer;

/// The job exit code a Host's stop leaves, distinct from anything the program itself returns.
const STOPPED_EXIT_CODE: u32 = 0xC000_013A;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

fn owned(handle: HANDLE) -> Option<OwnedHandle> {
    // SAFETY: `handle` was just returned by the system for this process to close, and is closed
    // exactly once, by the `OwnedHandle`.
    (!handle.is_null()).then(|| unsafe { OwnedHandle::from_raw_handle(handle as RawHandle) })
}

/// The process tree a Host owns: its child, and the job object everything it starts joins.
pub(super) struct OwnedProcess {
    process_id: u32,
    process: OwnedHandle,
    job: OwnedHandle,
}

impl OwnedProcess {
    /// Puts the pseudo-console's child in a new job. The job ends the tree if this Host itself
    /// ends, as the Unix Host's hangup would, and is what a stop terminates.
    pub(super) fn from_spawn(process_id: u32, _master: &dyn MasterPty) -> Result<Self, HostError> {
        let unavailable = || HostError::new(HostErrorCode::ProcessIdentityUnavailable);
        // SAFETY: each call gets arguments the system documents as valid for it; the handles it
        // returns are owned immediately, and the limit structure lives for the call reading it.
        unsafe {
            let job = owned(CreateJobObjectW(std::ptr::null(), std::ptr::null()))
                .ok_or_else(unavailable)?;
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job.as_raw_handle() as HANDLE,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
                    .map_err(|_| unavailable())?,
            ) == 0
            {
                return Err(unavailable());
            }
            let process = owned(OpenProcess(
                PROCESS_SET_QUOTA | PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                process_id,
            ))
            .ok_or_else(unavailable)?;
            if AssignProcessToJobObject(
                job.as_raw_handle() as HANDLE,
                process.as_raw_handle() as HANDLE,
            ) == 0
            {
                return Err(unavailable());
            }
            Ok(Self {
                process_id,
                process,
                job,
            })
        }
    }

    /// What a process token records as this tree's identity.
    pub(super) fn identity(&self) -> u64 {
        u64::from(self.process_id)
    }

    /// Ends the tree. The interrupt, a Ctrl+C typed into the pseudo-console, is sent by the Host
    /// before anything reaches here; there is no gentler Windows step between it and this.
    pub(super) fn signal(&self, signal: StopSignal) -> Result<(), HostError> {
        debug_assert_ne!(signal, StopSignal::Interrupt);
        if !self.leader_alive() {
            return Ok(());
        }
        // SAFETY: the job handle is owned by this struct and open.
        let ended =
            unsafe { TerminateJobObject(self.job.as_raw_handle() as HANDLE, STOPPED_EXIT_CODE) };
        if ended != 0 || !self.leader_alive() {
            Ok(())
        } else {
            Err(HostError::io(io::Error::last_os_error()))
        }
    }

    pub(super) fn leader_alive(&self) -> bool {
        let mut code = 0_u32;
        // SAFETY: the process handle is owned by this struct and open; `code` outlives the call.
        let read = unsafe { GetExitCodeProcess(self.process.as_raw_handle() as HANDLE, &mut code) };
        read != 0 && code == STILL_ACTIVE as u32
    }
}

/// One of the `MAX_LIVE_HOSTS` slots, held while this Host runs. The lock is released when the
/// file closes, however the process ends.
pub(super) struct RuntimeHostSlot {
    _file: File,
}

impl RuntimeHostSlot {
    pub(super) fn acquire(runtime_root: &Path) -> Result<Self, HostError> {
        for slot in 0..MAX_LIVE_HOSTS {
            let path = runtime_root.join(format!("{HOST_SLOT_PREFIX}{slot:02}.lock"));
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(path)
                .map_err(HostError::io)?;
            match file.try_lock() {
                Ok(()) => return Ok(Self { _file: file }),
                Err(fs::TryLockError::WouldBlock) => {}
                Err(fs::TryLockError::Error(error)) => return Err(HostError::io(error)),
            }
        }
        Err(HostError::new(HostErrorCode::ResourceLimit))
    }
}

/// The runtime root lives under the user's own profile, whose inherited access list already keeps
/// other standard users out. What is checked is that it is a real directory: a junction or link
/// could send the Host's files somewhere else.
pub(super) fn prepare_runtime_root(root: &Path) -> Result<(), HostError> {
    match fs::symlink_metadata(root) {
        Ok(metadata)
            if !metadata.is_dir()
                || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 =>
        {
            Err(HostError::new(HostErrorCode::PermissionDenied))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(root).map_err(HostError::io)
        }
        Err(error) => Err(HostError::io(error)),
    }
}

/// This process's user, as the security identifier the pipe's access list and every connection
/// check compare against.
struct UserSid {
    /// A `TOKEN_USER` and the SID it points into, as `GetTokenInformation` wrote them.
    buffer: Vec<u64>,
}

// SAFETY: the buffer is written once, when read from the token, and only read afterwards.
unsafe impl Send for UserSid {}
// SAFETY: as above; nothing mutates it after construction.
unsafe impl Sync for UserSid {}

impl UserSid {
    fn of_token(token: HANDLE) -> Option<Self> {
        let mut needed = 0_u32;
        // SAFETY: a null buffer with length zero asks only for the size, written to `needed`.
        unsafe {
            GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed);
        }
        if needed == 0 {
            return None;
        }
        // u64 elements keep the SID pointer inside it suitably aligned.
        let mut buffer = vec![0_u64; (needed as usize).div_ceil(8)];
        // SAFETY: `buffer` holds at least `needed` bytes.
        let read = unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                needed,
                &mut needed,
            )
        };
        (read != 0).then_some(Self { buffer })
    }

    fn of_process(process: HANDLE) -> Option<Self> {
        let mut token: HANDLE = std::ptr::null_mut();
        // SAFETY: `process` is an open process handle; the token handle returned is owned below.
        if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
            return None;
        }
        let token = owned(token)?;
        Self::of_token(token.as_raw_handle() as HANDLE)
    }

    fn current() -> Result<Self, HostError> {
        // SAFETY: the pseudo-handle for this process needs no closing.
        Self::of_process(unsafe { GetCurrentProcess() })
            .ok_or_else(|| HostError::new(HostErrorCode::PermissionDenied))
    }

    fn sid(&self) -> PSID {
        // SAFETY: the buffer holds a TOKEN_USER, whose first field names the SID stored after it.
        unsafe { (*self.buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }

    fn same_as(&self, other: &Self) -> bool {
        // SAFETY: both SIDs live inside buffers that outlive the call.
        unsafe { EqualSid(self.sid(), other.sid()) != 0 }
    }

    fn string(&self) -> Result<String, HostError> {
        let mut text: *mut u16 = std::ptr::null_mut();
        // SAFETY: the SID is valid; the string the system allocates is freed below.
        if unsafe { ConvertSidToStringSidW(self.sid(), &mut text) } == 0 || text.is_null() {
            return Err(HostError::new(HostErrorCode::PermissionDenied));
        }
        // SAFETY: `text` is a NUL-terminated string the system allocated.
        let value = unsafe {
            let length = (0..).take_while(|&index| *text.add(index) != 0).count();
            let value = String::from_utf16_lossy(std::slice::from_raw_parts(text, length));
            LocalFree(text as HLOCAL);
            value
        };
        Ok(value)
    }
}

/// A security descriptor granting this user alone every access to the pipe.
struct PipeSecurity {
    descriptor: *mut c_void,
}

// SAFETY: the descriptor is built once and only read by pipe creation afterwards.
unsafe impl Send for PipeSecurity {}
// SAFETY: as above.
unsafe impl Sync for PipeSecurity {}

impl PipeSecurity {
    fn for_user(user: &UserSid) -> Result<Self, HostError> {
        // A protected DACL (no inheritance) with one entry: generic-all for this user's SID.
        let sddl: Vec<u16> = format!("D:P(A;;GA;;;{})", user.string()?)
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut descriptor: *mut c_void = std::ptr::null_mut();
        // SAFETY: `sddl` is NUL-terminated and outlives the call; the descriptor it allocates is
        // freed when this struct drops.
        let built = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        };
        if built == 0 || descriptor.is_null() {
            return Err(HostError::new(HostErrorCode::PermissionDenied));
        }
        Ok(Self { descriptor })
    }

    fn create(&self, name: &str, first: bool) -> Result<NamedPipeServer, HostError> {
        let mut attributes = SECURITY_ATTRIBUTES {
            nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>())
                .map_err(|_| HostError::new(HostErrorCode::Io))?,
            lpSecurityDescriptor: self.descriptor,
            bInheritHandle: 0,
        };
        // SAFETY: `attributes` points at a live descriptor and lives for the call. Requiring the
        // first instance is what stops a pipe someone else created under this name from being
        // joined; refusing remote clients keeps the pipe to this machine.
        unsafe {
            ServerOptions::new()
                .first_pipe_instance(first)
                .reject_remote_clients(true)
                .create_with_security_attributes_raw(name, (&raw mut attributes).cast())
        }
        .map_err(|error| {
            if error.kind() == io::ErrorKind::PermissionDenied {
                HostError::new(HostErrorCode::PermissionDenied)
            } else {
                HostError::io(error)
            }
        })
    }
}

impl Drop for PipeSecurity {
    fn drop(&mut self) {
        // SAFETY: the descriptor was allocated by the system for this struct to free, once.
        unsafe {
            LocalFree(self.descriptor as HLOCAL);
        }
    }
}

/// The Host's pipe, and the endpoint file clients find it by.
pub(super) struct UserOnlyListener {
    pipe_name: String,
    security: PipeSecurity,
    user: UserSid,
    waiting: Mutex<Option<NamedPipeServer>>,
    marker_path: PathBuf,
    /// Held open, without sharing delete, so the endpoint cannot be replaced while this Host runs.
    marker: Option<File>,
}

impl UserOnlyListener {
    pub(super) fn bind(
        runtime_root: &Path,
        session_id: multiplex_domain::HostedSessionId,
    ) -> Result<Self, HostError> {
        prepare_runtime_root(runtime_root)?;
        let endpoint_name = opaque_endpoint_name(session_id);
        let marker_path = runtime_root.join(&endpoint_name);
        // Anything already at the endpoint, as on Unix, is refused rather than replaced.
        let marker = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&marker_path)
            .map_err(|error| {
                if error.kind() == io::ErrorKind::AlreadyExists {
                    HostError::new(HostErrorCode::PermissionDenied)
                } else {
                    HostError::io(error)
                }
            })?;
        let user = UserSid::current()?;
        let security = PipeSecurity::for_user(&user)?;
        let pipe_name = pipe_name(&endpoint_name);
        let first = security.create(&pipe_name, true)?;
        Ok(Self {
            pipe_name,
            security,
            user,
            waiting: Mutex::new(Some(first)),
            marker_path,
            marker: Some(marker),
        })
    }

    /// The next connection from this user, or `None` for one refused on its own: a peer that is
    /// not this user, or one gone before it could be asked. Neither says anything about the pipe.
    pub(super) async fn accept(&self) -> Result<Option<HostStream>, HostError> {
        let mut waiting = self.waiting.lock().await;
        let server = match waiting.as_ref() {
            Some(server) => server,
            None => {
                *waiting = Some(self.security.create(&self.pipe_name, false)?);
                waiting.as_ref().expect("just created")
            }
        };
        if let Err(error) = server.connect().await
            && error.raw_os_error() != Some(ERROR_PIPE_CONNECTED as i32)
        {
            // This instance is spent; the next accept makes a fresh one.
            waiting.take();
            return if error.kind() == io::ErrorKind::BrokenPipe {
                Ok(None)
            } else {
                Err(HostError::io(error))
            };
        }
        // The next client needs an instance waiting before this one is handed over.
        let next = self.security.create(&self.pipe_name, false)?;
        let connected = waiting.replace(next).expect("an instance was waiting");
        drop(waiting);
        if client_is(&connected, &self.user) {
            Ok(Some(connected))
        } else {
            Ok(None)
        }
    }
}

/// Whether the process at the other end of `pipe` runs as `user`. The access list already keeps
/// everyone else from opening the pipe; this is the same check the Unix Host makes with the
/// socket's peer credentials, kept for the same reason.
fn client_is(pipe: &NamedPipeServer, user: &UserSid) -> bool {
    let mut process_id = 0_u32;
    // SAFETY: the pipe handle is open for the life of `pipe`; `process_id` outlives the call.
    let known =
        unsafe { GetNamedPipeClientProcessId(pipe.as_raw_handle() as HANDLE, &mut process_id) };
    if known == 0 {
        return false;
    }
    // SAFETY: opening a process for its identity only; the handle is owned and closed.
    let Some(process) =
        owned(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) })
    else {
        return false;
    };
    UserSid::of_process(process.as_raw_handle() as HANDLE).is_some_and(|peer| peer.same_as(user))
}

impl Drop for UserOnlyListener {
    fn drop(&mut self) {
        // Closed first: Windows will not delete a file this process still has open.
        self.marker.take();
        let _ = fs::remove_file(&self.marker_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_knows_its_own_user() {
        let user = UserSid::current().unwrap();
        assert!(user.string().unwrap().starts_with("S-1-"));
        assert!(user.same_as(&UserSid::current().unwrap()));
    }

    #[test]
    fn a_runtime_root_is_created_and_a_file_in_its_place_refused() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runtime");
        prepare_runtime_root(&root).unwrap();
        assert!(root.is_dir());
        let file = temp.path().join("file");
        fs::write(&file, b"").unwrap();
        assert_eq!(
            prepare_runtime_root(&file).unwrap_err().code,
            HostErrorCode::PermissionDenied
        );
    }

    #[test]
    fn slots_are_exclusive_until_released() {
        let temp = tempfile::tempdir().unwrap();
        let held: Vec<_> = (0..MAX_LIVE_HOSTS)
            .map(|_| RuntimeHostSlot::acquire(temp.path()).unwrap())
            .collect();
        assert_eq!(
            RuntimeHostSlot::acquire(temp.path())
                .err()
                .map(|error| error.code),
            Some(HostErrorCode::ResourceLimit)
        );
        drop(held);
        assert!(RuntimeHostSlot::acquire(temp.path()).is_ok());
    }

    #[tokio::test]
    async fn this_user_connects_and_is_accepted() {
        use tokio::net::windows::named_pipe::ClientOptions;

        let temp = tempfile::tempdir().unwrap();
        let session = multiplex_domain::HostedSessionId::new();
        let listener = UserOnlyListener::bind(temp.path(), session).unwrap();
        let name = pipe_name(&opaque_endpoint_name(session));
        let client = tokio::spawn(async move { ClientOptions::new().open(&name) });
        let accepted = listener.accept().await.unwrap();
        assert!(accepted.is_some());
        assert!(client.await.unwrap().is_ok());
        // A second Host for the same endpoint is refused rather than sharing it.
        assert_eq!(
            UserOnlyListener::bind(temp.path(), session)
                .err()
                .map(|error| error.code),
            Some(HostErrorCode::PermissionDenied)
        );
    }
}
