//! Keeps the LAN Controller listener reachable while the desktop app is closed.
//!
//! `termirust controller-service run` supervises one headless listener worker for the saved
//! route. The desktop app and the service never serve the route together: a listener worker
//! holds [`ListenerOwnership`] while it serves, and a starting desktop app asks the service to
//! yield over a user-only socket before starting its own listener. When the app quits, its
//! worker exits, the lock frees, and the service takes the route back. Pairing new devices
//! still needs the app; the service only serves devices that are already paired.
//!
//! On macOS the service is installed as a per-user LaunchAgent that runs at login. On Windows it is
//! a value under the current user's `Run` key, which needs no administrator rights, and the app
//! asks it to yield through two small files in the user's own runtime directory instead of a
//! socket; a lock file the running service holds is how anything tells that it is running.

use std::io::{self, BufReader};
#[cfg(unix)]
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use multiplex_controller_listener::{ListenerError, ListenerOwnership};

pub const SERVICE_COMMAND: &str = "controller-service";
/// The LaunchAgent label, derived from the app bundle identifier.
pub const LAUNCH_AGENT_LABEL: &str = "com.millionrust.multiplex.controller-service";
/// Labels installed copies registered under earlier bundle identifiers: TermiRust's, then
/// Multiplex's before it moved to com.millionrust. An old plist can name an application path that
/// no longer exists, so launchd keeps trying to start something that is gone, or two agents serve
/// the same route; both installing and removing the service take every one of them out.
#[cfg(target_os = "macos")]
const LEGACY_LAUNCH_AGENT_LABELS: [&str; 2] = [
    "com.termirust.desktop.controller-service",
    "com.multiplex.desktop.controller-service",
];

const YIELD_SOCKET: &str = "controller-service.sock";
/// Windows: the app writes this to ask for the route, and the service answers with the next one.
#[cfg(windows)]
const YIELD_REQUEST_FILE: &str = "controller-service.yield";
#[cfg(windows)]
const YIELD_RELEASED_FILE: &str = "controller-service.released";
/// Windows: held exclusively for as long as a service runs.
#[cfg(windows)]
const SERVICE_LOCK_FILE: &str = "controller-service.lock";
/// Windows: written by `remove` (or a reinstall) to ask the running service to exit.
#[cfg(windows)]
const STOP_REQUEST_FILE: &str = "controller-service.stop";
#[cfg(windows)]
const HANDSHAKE_POLL: Duration = Duration::from_millis(20);
const YIELD_REQUEST: &[u8] = b"yield\n";
const YIELD_ACK: &[u8] = b"released\n";
const YIELD_TIMEOUT: Duration = Duration::from_secs(3);
const SUPERVISOR_POLL: Duration = Duration::from_millis(100);
const ROUTE_RETRY: Duration = Duration::from_secs(2);
/// After yielding, the app gets this long to take the route before the service reclaims it.
const HANDOVER_GRACE: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServiceError(&'static str);

impl ServiceError {
    pub const UNSUPPORTED: Self = Self("service.unsupported");

    pub const fn code(self) -> &'static str {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceStatus {
    NotInstalled,
    Installed,
    Running,
    Unsupported,
}

/// Where the desktop app and the service meet to hand over the route.
pub fn yield_socket_path(runtime_parent: &Path) -> PathBuf {
    runtime_parent.join(YIELD_SOCKET)
}

/// Asks a running service to stop serving the route, and waits until it has. Returns `false`
/// when no service answered, which is the normal case when none is installed.
#[cfg(unix)]
pub fn request_yield(runtime_parent: &Path) -> bool {
    use std::os::unix::net::UnixStream;

    let Ok(mut stream) = UnixStream::connect(yield_socket_path(runtime_parent)) else {
        return false;
    };
    if stream.set_read_timeout(Some(YIELD_TIMEOUT)).is_err()
        || stream.set_write_timeout(Some(YIELD_TIMEOUT)).is_err()
        || stream.write_all(YIELD_REQUEST).is_err()
    {
        return false;
    }
    let mut reply = [0_u8; YIELD_ACK.len()];
    stream.read_exact(&mut reply).is_ok() && reply == YIELD_ACK
}

#[cfg(windows)]
pub fn request_yield(runtime_parent: &Path) -> bool {
    if !service_running(runtime_parent) {
        return false;
    }
    let released = runtime_parent.join(YIELD_RELEASED_FILE);
    let request = runtime_parent.join(YIELD_REQUEST_FILE);
    // An answer left from an exchange that timed out must not be read as this one's.
    let _ = std::fs::remove_file(&released);
    if std::fs::write(&request, YIELD_REQUEST).is_err() {
        return false;
    }
    let deadline = Instant::now() + YIELD_TIMEOUT;
    while Instant::now() < deadline {
        if released.is_file() {
            let _ = std::fs::remove_file(&released);
            return true;
        }
        thread::sleep(HANDSHAKE_POLL);
    }
    let _ = std::fs::remove_file(&request);
    false
}

#[cfg(not(any(unix, windows)))]
pub fn request_yield(_: &Path) -> bool {
    false
}

/// Whether a service holds the lock in `runtime_parent`: a lock nobody holds is left over from a
/// service that exited, and taking it here releases it again straight away.
#[cfg(windows)]
fn service_running(runtime_parent: &Path) -> bool {
    let Ok(file) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(runtime_parent.join(SERVICE_LOCK_FILE))
    else {
        return false;
    };
    matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock))
}

/// A serving listener, as the supervisor sees it.
pub trait ServingListener {
    fn is_finished(&self) -> bool;
    /// Stops serving and returns once the route is released.
    fn stop(self: Box<Self>);
}

/// Runs listeners for the saved route until `should_stop` returns true. `start` is called
/// whenever the route is free; it returns `Err` when there is nothing to serve yet, such as
/// a disabled route, and the supervisor tries again later.
#[cfg(any(unix, windows))]
pub fn supervise(
    controller_root: &Path,
    runtime_parent: &Path,
    mut start: impl FnMut() -> Result<Box<dyn ServingListener>, ServiceError>,
    mut should_stop: impl FnMut() -> bool,
) -> Result<(), ServiceError> {
    let mut channel = YieldChannel::open(runtime_parent)?;
    let mut serving: Option<Box<dyn ServingListener>> = None;
    let mut next_attempt = Instant::now();
    while !should_stop() {
        if channel.requested() {
            if let Some(listener) = serving.take() {
                listener.stop();
            }
            channel.acknowledge();
            wait_for_handover(controller_root, &mut should_stop);
            next_attempt = Instant::now();
            continue;
        }
        if serving
            .as_ref()
            .is_some_and(|listener| listener.is_finished())
        {
            if let Some(listener) = serving.take() {
                listener.stop();
            }
            next_attempt = Instant::now() + ROUTE_RETRY;
        }
        if serving.is_none() && Instant::now() >= next_attempt {
            // An unreadable lock, such as a store the app has not created yet, is retried.
            let route_free = ListenerOwnership::try_acquire(controller_root)
                .is_ok_and(|ownership| ownership.is_some());
            if route_free {
                match start() {
                    Ok(listener) => serving = Some(listener),
                    Err(_) => next_attempt = Instant::now() + ROUTE_RETRY,
                }
            } else {
                next_attempt = Instant::now() + ROUTE_RETRY;
            }
        }
        thread::sleep(SUPERVISOR_POLL);
    }
    if let Some(listener) = serving.take() {
        listener.stop();
    }
    channel.close();
    Ok(())
}

/// Where a starting app's request to take the route arrives: a user-only socket on Unix.
#[cfg(unix)]
struct YieldChannel {
    socket: std::os::unix::net::UnixListener,
    path: PathBuf,
    pending: Option<std::os::unix::net::UnixStream>,
}

#[cfg(unix)]
impl YieldChannel {
    fn open(runtime_parent: &Path) -> Result<Self, ServiceError> {
        Ok(Self {
            socket: bind_yield_socket(runtime_parent)?,
            path: yield_socket_path(runtime_parent),
            pending: None,
        })
    }

    fn requested(&mut self) -> bool {
        self.pending = accept_yield_request(&self.socket);
        self.pending.is_some()
    }

    fn acknowledge(&mut self) {
        if let Some(stream) = self.pending.take() {
            acknowledge_yield(stream);
        }
    }

    fn close(self) {
        let _ = std::fs::remove_file(self.path);
    }
}

/// Where a starting app's request to take the route arrives: a file in the user's own runtime
/// directory on Windows, answered with another. The lock held here is what `service_running`
/// and a second service see.
#[cfg(windows)]
struct YieldChannel {
    runtime_parent: PathBuf,
    _lock: std::fs::File,
}

#[cfg(windows)]
impl YieldChannel {
    fn open(runtime_parent: &Path) -> Result<Self, ServiceError> {
        std::fs::create_dir_all(runtime_parent)
            .map_err(|_| ServiceError("service.runtime_unavailable"))?;
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(runtime_parent.join(SERVICE_LOCK_FILE))
            .map_err(|_| ServiceError("service.socket_failed"))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(ServiceError("service.already_running"));
            }
            Err(_) => return Err(ServiceError("service.socket_failed")),
        }
        // Whatever a previous service left unanswered is stale now.
        for name in [YIELD_REQUEST_FILE, YIELD_RELEASED_FILE, STOP_REQUEST_FILE] {
            let _ = std::fs::remove_file(runtime_parent.join(name));
        }
        Ok(Self {
            runtime_parent: runtime_parent.to_path_buf(),
            _lock: lock,
        })
    }

    fn requested(&mut self) -> bool {
        std::fs::remove_file(self.runtime_parent.join(YIELD_REQUEST_FILE)).is_ok()
    }

    fn acknowledge(&mut self) {
        let _ = std::fs::write(self.runtime_parent.join(YIELD_RELEASED_FILE), YIELD_ACK);
    }

    fn close(self) {
        let _ = std::fs::remove_file(self.runtime_parent.join(YIELD_RELEASED_FILE));
    }
}

#[cfg(unix)]
fn bind_yield_socket(
    runtime_parent: &Path,
) -> Result<std::os::unix::net::UnixListener, ServiceError> {
    use std::os::unix::fs::PermissionsExt as _;
    use std::os::unix::net::{UnixListener, UnixStream};

    let path = yield_socket_path(runtime_parent);
    std::fs::create_dir_all(runtime_parent)
        .map_err(|_| ServiceError("service.runtime_unavailable"))?;
    std::fs::set_permissions(runtime_parent, std::fs::Permissions::from_mode(0o700))
        .map_err(|_| ServiceError("service.runtime_unavailable"))?;
    if UnixStream::connect(&path).is_ok() {
        return Err(ServiceError("service.already_running"));
    }
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).map_err(|_| ServiceError("service.socket_failed"))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .map_err(|_| ServiceError("service.socket_failed"))?;
    listener
        .set_nonblocking(true)
        .map_err(|_| ServiceError("service.socket_failed"))?;
    Ok(listener)
}

#[cfg(unix)]
fn accept_yield_request(
    socket: &std::os::unix::net::UnixListener,
) -> Option<std::os::unix::net::UnixStream> {
    let (mut stream, _) = socket.accept().ok()?;
    stream.set_nonblocking(false).ok()?;
    stream.set_read_timeout(Some(YIELD_TIMEOUT)).ok()?;
    let mut request = [0_u8; YIELD_REQUEST.len()];
    stream.read_exact(&mut request).ok()?;
    (request == YIELD_REQUEST).then_some(stream)
}

#[cfg(unix)]
fn acknowledge_yield(mut stream: std::os::unix::net::UnixStream) {
    let _ = stream.set_write_timeout(Some(YIELD_TIMEOUT));
    let _ = stream.write_all(YIELD_ACK);
}

/// Waits until the app that asked has taken the route, or the grace period passes.
#[cfg(any(unix, windows))]
fn wait_for_handover(controller_root: &Path, should_stop: &mut impl FnMut() -> bool) {
    let deadline = Instant::now() + HANDOVER_GRACE;
    while Instant::now() < deadline && !should_stop() {
        match ListenerOwnership::try_acquire(controller_root) {
            Ok(None) | Err(_) => return,
            Ok(Some(probe)) => {
                // Release the probe before waiting, or the app could never take the route.
                drop(probe);
                thread::sleep(SUPERVISOR_POLL);
            }
        }
    }
}

/// A listener worker thread fed its launch descriptor through a pipe. Closing the pipe is the
/// worker's stop signal.
#[cfg(any(unix, windows))]
struct WorkerListener {
    control: Option<io::PipeWriter>,
    thread: Option<JoinHandle<Result<(), ListenerError>>>,
}

#[cfg(any(unix, windows))]
impl ServingListener for WorkerListener {
    fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }

    fn stop(mut self: Box<Self>) {
        self.control.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(any(unix, windows))]
fn start_worker(
    screens: super::screen_sharing::ScreenSharing,
) -> Result<Box<dyn ServingListener>, ServiceError> {
    use multiplex_controller_listener::{
        ListenerLaunchDescriptor, run_listener_worker_with_screens,
    };
    use multiplex_store::{ControllerDeviceRepository, ControllerNetworkRepository};

    let unavailable = ServiceError("service.storage_unavailable");
    let app_root = crate::storage::app_dir().map_err(|_| unavailable)?;
    let controller_root = crate::storage::controller_store_dir().map_err(|_| unavailable)?;
    let network = ControllerNetworkRepository::open(&controller_root)
        .and_then(|repository| repository.load())
        .map_err(|_| unavailable)?;
    if !network.policy.enabled {
        return Err(ServiceError("service.route_disabled"));
    }
    let repository =
        ControllerDeviceRepository::open(controller_root.clone()).map_err(|_| unavailable)?;
    let identity = super::host_identity::HostIdentityService::new(
        repository,
        super::host_identity::OsSecretStore,
        super::host_identity::OsIdentityEntropy,
    )
    .load_or_create()
    .map_err(|_| ServiceError("service.identity_unavailable"))?;
    let host_private = identity
        .static_private_key()
        .ok_or(ServiceError("service.identity_unavailable"))?;
    let runtime_parent = crate::controller_runtime_parent(&app_root);
    let sources = super::remote_bridge_sources(&runtime_parent);
    let descriptor = ListenerLaunchDescriptor::new(
        controller_root,
        crate::storage::project_store_dir().map_err(|_| unavailable)?,
        app_root.join("durable-sessions"),
        runtime_parent,
        network.revision,
        network.policy,
        &host_private,
    )
    .and_then(|descriptor| descriptor.with_desktop_pane_bridge(sources.desktop_pane_bridge))
    .map(|descriptor| descriptor.with_tmux_sessions(sources.tmux_sessions.is_some()))
    .map_err(|_| ServiceError("service.descriptor_invalid"))?;
    let (reader, mut control) = io::pipe().map_err(|_| ServiceError("service.pipe_failed"))?;
    descriptor
        .write(&mut control)
        .map_err(|_| ServiceError("service.pipe_failed"))?;
    let thread = thread::Builder::new()
        .name("termirust-controller-service".to_owned())
        .spawn(move || {
            // The same screen provider the app's own worker gets. Without it a paired phone
            // could watch this computer only while the app happened to be open, which is exactly
            // the thing the background service exists to stop being true.
            //
            // Capture and injection then run inside this service process, and macOS records its
            // permission grants against it rather than against the app: a LaunchAgent has no
            // responsible parent to inherit from. `screen_permission` is what notices.
            run_listener_worker_with_screens(
                BufReader::new(reader),
                io::sink(),
                Some(std::sync::Arc::new(screens)),
            )
        })
        .map_err(|_| ServiceError("service.worker_failed"))?;
    Ok(Box::new(WorkerListener {
        control: Some(control),
        thread: Some(thread),
    }))
}

/// `termirust controller-service run|install|remove|status`.
pub fn run_command(arguments: &[String]) -> Result<(), ServiceError> {
    match arguments.first().map(String::as_str) {
        Some("run") if arguments.len() == 1 => run_foreground(),
        Some("install") if arguments.len() == 1 => {
            install()?;
            println!("Multiplex will keep paired devices connected after you quit the app.");
            Ok(())
        }
        Some("remove") if arguments.len() == 1 => {
            remove()?;
            println!("The background Controller listener was removed.");
            Ok(())
        }
        Some("status") if arguments.len() == 1 => {
            let state = status();
            println!(
                "{}",
                match state {
                    ServiceStatus::NotInstalled => "not installed",
                    ServiceStatus::Installed => "installed, not running",
                    ServiceStatus::Running => "running",
                    ServiceStatus::Unsupported => "unsupported on this platform",
                }
            );
            // The grant the service needs and cannot ask for. Said here because the alternative
            // is a phone showing a blank screen with nothing anywhere to explain why.
            if matches!(state, ServiceStatus::Installed | ServiceStatus::Running)
                && let Some(line) = screen_capture_line()
            {
                println!("{line}");
            }
            Ok(())
        }
        _ => Err(ServiceError("service.usage")),
    }
}

#[cfg(unix)]
fn run_foreground() -> Result<(), ServiceError> {
    let app_root =
        crate::storage::app_dir().map_err(|_| ServiceError("service.storage_unavailable"))?;
    let controller_root = crate::storage::controller_store_dir()
        .map_err(|_| ServiceError("service.storage_unavailable"))?;
    let runtime_parent = crate::controller_runtime_parent(&app_root);
    let screens = super::screen_sharing::ScreenSharing::enabled();
    supervise(
        &controller_root,
        &runtime_parent,
        || start_worker(screens.clone()),
        || false,
    )
}

/// Windows has no launchd to stop the service, so `remove` asks it to exit with a file.
#[cfg(windows)]
fn run_foreground() -> Result<(), ServiceError> {
    let app_root =
        crate::storage::app_dir().map_err(|_| ServiceError("service.storage_unavailable"))?;
    let controller_root = crate::storage::controller_store_dir()
        .map_err(|_| ServiceError("service.storage_unavailable"))?;
    let runtime_parent = crate::controller_runtime_parent(&app_root);
    let stop = runtime_parent.join(STOP_REQUEST_FILE);
    // Shared with the tray, so its icon always knows who is watching.
    let screens = super::screen_sharing::ScreenSharing::enabled();
    let tray = super::windows_tray::spawn(screens.clone());
    supervise(
        &controller_root,
        &runtime_parent,
        || start_worker(screens.clone()),
        || tray.stop_requested() || std::fs::remove_file(&stop).is_ok(),
    )
}

#[cfg(not(any(unix, windows)))]
fn run_foreground() -> Result<(), ServiceError> {
    Err(ServiceError("service.unsupported"))
}

#[cfg(target_os = "macos")]
pub fn status() -> ServiceStatus {
    let Some(plist) = launch_agent_path() else {
        return ServiceStatus::Unsupported;
    };
    if !plist.is_file() {
        return ServiceStatus::NotInstalled;
    }
    let running = std::process::Command::new("/bin/launchctl")
        .args(["print", &launchctl_service_target()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if running {
        ServiceStatus::Running
    } else {
        ServiceStatus::Installed
    }
}

#[cfg(windows)]
pub fn status() -> ServiceStatus {
    if windows_run::read().is_none() {
        return ServiceStatus::NotInstalled;
    }
    match crate::storage::app_dir() {
        Ok(app_root) if service_running(&crate::controller_runtime_parent(&app_root)) => {
            ServiceStatus::Running
        }
        _ => ServiceStatus::Installed,
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn status() -> ServiceStatus {
    ServiceStatus::Unsupported
}

#[cfg(target_os = "macos")]
pub fn install() -> Result<(), ServiceError> {
    let plist_path = launch_agent_path().ok_or(ServiceError("service.home_unavailable"))?;
    let executable = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .map_err(|_| ServiceError("service.executable_unavailable"))?;
    let log = crate::storage::app_dir()
        .map_err(|_| ServiceError("service.storage_unavailable"))?
        .join("controller-service.log");
    let contents = launch_agent_plist(&executable, &log);
    if let Some(parent) = plist_path.parent() {
        std::fs::create_dir_all(parent).map_err(|_| ServiceError("service.write_failed"))?;
    }
    multiplex_store::AtomicWriter::write(
        &multiplex_store::SystemAtomicWriter,
        &plist_path,
        contents.as_bytes(),
    )
    .map_err(|_| ServiceError("service.write_failed"))?;
    // Replace a previous registration so a moved app takes effect.
    let _ = launchctl(&["bootout", &launchctl_service_target()]);
    remove_legacy_launch_agent();
    launchctl(&[
        "bootstrap",
        &launchctl_domain(),
        &plist_path.to_string_lossy(),
    ])
    .map_err(|_| ServiceError("service.launchctl_failed"))
}

/// Registers the service to start at logon, and starts it now so it serves before the next one.
#[cfg(windows)]
pub fn install() -> Result<(), ServiceError> {
    use std::os::windows::process::CommandExt as _;
    use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS};

    let executable =
        std::env::current_exe().map_err(|_| ServiceError("service.executable_unavailable"))?;
    let app_root =
        crate::storage::app_dir().map_err(|_| ServiceError("service.storage_unavailable"))?;
    windows_run::write(&format!(
        "\"{}\" {SERVICE_COMMAND} run",
        executable.display()
    ))?;
    // A service already running may be an older copy of the app; replace it, as launchd does.
    let runtime_parent = crate::controller_runtime_parent(&app_root);
    stop_running_service(&runtime_parent);
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(app_root.join("controller-service.log"))
        .map_err(|_| ServiceError("service.write_failed"))?;
    let log_err = log
        .try_clone()
        .map_err(|_| ServiceError("service.write_failed"))?;
    std::process::Command::new(executable)
        .args([SERVICE_COMMAND, "run"])
        .stdin(std::process::Stdio::null())
        .stdout(log)
        .stderr(log_err)
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .spawn()
        .map(drop)
        .map_err(|_| ServiceError("service.start_failed"))
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn install() -> Result<(), ServiceError> {
    Err(ServiceError("service.unsupported"))
}

#[cfg(target_os = "macos")]
pub fn remove() -> Result<(), ServiceError> {
    let plist_path = launch_agent_path().ok_or(ServiceError("service.home_unavailable"))?;
    let _ = launchctl(&["bootout", &launchctl_service_target()]);
    remove_legacy_launch_agent();
    match std::fs::remove_file(&plist_path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(ServiceError("service.write_failed")),
    }
}

#[cfg(windows)]
pub fn remove() -> Result<(), ServiceError> {
    windows_run::delete()?;
    if let Ok(app_root) = crate::storage::app_dir() {
        stop_running_service(&crate::controller_runtime_parent(&app_root));
    }
    Ok(())
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn remove() -> Result<(), ServiceError> {
    Err(ServiceError("service.unsupported"))
}

/// Asks a running service to exit, and waits until its lock is free or the wait runs out.
#[cfg(windows)]
fn stop_running_service(runtime_parent: &Path) {
    if !service_running(runtime_parent) {
        return;
    }
    if std::fs::write(runtime_parent.join(STOP_REQUEST_FILE), b"stop\n").is_err() {
        return;
    }
    // The supervisor stops its worker before it exits, which can take the worker's own timeout.
    let deadline = Instant::now() + YIELD_TIMEOUT + HANDOVER_GRACE;
    while Instant::now() < deadline && service_running(runtime_parent) {
        thread::sleep(HANDSHAKE_POLL);
    }
}

/// The service's value under `HKEY_CURRENT_USER\...\Run`: what Windows starts at this user's
/// logon, and which the user can see and switch off under Startup apps.
#[cfg(windows)]
mod windows_run {
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
    };

    use super::{LAUNCH_AGENT_LABEL, ServiceError};

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }

    pub(super) fn read() -> Option<String> {
        let key = wide(RUN_KEY);
        let name = wide(LAUNCH_AGENT_LABEL);
        let mut bytes: u32 = 0;
        // SAFETY: both strings are NUL-terminated and outlive the call; a null data pointer asks
        // only for the size, which is written to `bytes`.
        let found = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut bytes,
            )
        };
        if found != ERROR_SUCCESS || bytes == 0 {
            return None;
        }
        let mut buffer = vec![0_u16; (bytes as usize).div_ceil(2)];
        // SAFETY: `buffer` holds at least `bytes` bytes, the size the previous call reported.
        let read = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut bytes,
            )
        };
        if read != ERROR_SUCCESS {
            return None;
        }
        let end = buffer
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(buffer.len());
        Some(String::from_utf16_lossy(&buffer[..end]))
    }

    pub(super) fn write(command: &str) -> Result<(), ServiceError> {
        let key = wide(RUN_KEY);
        let name = wide(LAUNCH_AGENT_LABEL);
        let data = wide(command);
        let length =
            u32::try_from(data.len() * 2).map_err(|_| ServiceError("service.write_failed"))?;
        // SAFETY: all three strings are NUL-terminated and outlive the call, and `length` is the
        // size of `data` in bytes, terminator included, as REG_SZ requires.
        let written = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                REG_SZ,
                data.as_ptr().cast(),
                length,
            )
        };
        if written == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(ServiceError("service.write_failed"))
        }
    }

    pub(super) fn delete() -> Result<(), ServiceError> {
        let key = wide(RUN_KEY);
        let name = wide(LAUNCH_AGENT_LABEL);
        // SAFETY: both strings are NUL-terminated and outlive the call.
        let deleted = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) };
        if deleted == ERROR_SUCCESS || deleted == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(ServiceError("service.write_failed"))
        }
    }
}

/// Takes out the agent an installed copy registered before the rename. Nothing here is worth
/// failing over: the agent may never have existed, and what matters is that this app's own
/// registration is the one launchd has.
#[cfg(target_os = "macos")]
fn remove_legacy_launch_agent() {
    for label in LEGACY_LAUNCH_AGENT_LABELS {
        let _ = launchctl(&["bootout", &format!("{}/{label}", launchctl_domain())]);
        if let Some(home) = dirs::home_dir() {
            let _ = std::fs::remove_file(
                home.join("Library/LaunchAgents")
                    .join(format!("{label}.plist")),
            );
        }
    }
}

#[cfg(target_os = "macos")]
fn launch_agent_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| {
        home.join("Library/LaunchAgents")
            .join(format!("{LAUNCH_AGENT_LABEL}.plist"))
    })
}

#[cfg(target_os = "macos")]
fn launchctl_domain() -> String {
    // SAFETY: getuid has no preconditions and cannot fail.
    format!("gui/{}", unsafe { libc::getuid() })
}

#[cfg(target_os = "macos")]
fn launchctl_service_target() -> String {
    format!("{}/{LAUNCH_AGENT_LABEL}", launchctl_domain())
}

#[cfg(target_os = "macos")]
fn launchctl(arguments: &[&str]) -> Result<(), ()> {
    std::process::Command::new("/bin/launchctl")
        .args(arguments)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|_| ())
        .and_then(|status| if status.success() { Ok(()) } else { Err(()) })
}

/// A user LaunchAgent that starts the service at login and restarts it if it exits.
pub fn launch_agent_plist(executable: &Path, log: &Path) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{label}</string>
	<key>ProgramArguments</key>
	<array>
		<string>{executable}</string>
		<string>{SERVICE_COMMAND}</string>
		<string>run</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<true/>
	<key>ProcessType</key>
	<string>Background</string>
	<key>ThrottleInterval</key>
	<integer>10</integer>
	<key>StandardOutPath</key>
	<string>{log}</string>
	<key>StandardErrorPath</key>
	<string>{log}</string>
</dict>
</plist>
"#,
        label = xml_escape(LAUNCH_AGENT_LABEL),
        executable = xml_escape(&executable.to_string_lossy()),
        log = xml_escape(&log.to_string_lossy()),
    )
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    #[test]
    fn one_service_holds_the_route_and_a_second_is_refused() {
        let runtime = tempfile::tempdir().unwrap();
        assert!(!service_running(runtime.path()));
        let channel = YieldChannel::open(runtime.path()).unwrap();
        assert!(service_running(runtime.path()));
        assert_eq!(
            YieldChannel::open(runtime.path()).err(),
            Some(ServiceError("service.already_running"))
        );
        channel.close();
        assert!(!service_running(runtime.path()));
    }

    #[test]
    fn a_starting_app_is_answered_once_the_service_lets_go() {
        let runtime = tempfile::tempdir().unwrap();
        let service_dir = runtime.path().to_path_buf();
        let service = thread::spawn(move || {
            let mut channel = YieldChannel::open(&service_dir).unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            while Instant::now() < deadline {
                if channel.requested() {
                    channel.acknowledge();
                    return true;
                }
                thread::sleep(HANDSHAKE_POLL);
            }
            false
        });
        while !service_running(runtime.path()) {
            thread::sleep(HANDSHAKE_POLL);
        }
        assert!(request_yield(runtime.path()));
        assert!(service.join().unwrap());
    }

    #[test]
    fn nothing_answers_when_no_service_runs() {
        let runtime = tempfile::tempdir().unwrap();
        assert!(!request_yield(runtime.path()));
        assert!(!runtime.path().join(YIELD_REQUEST_FILE).exists());
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use super::*;

    /// The service's screen recording grant is its own, not the app's.
    ///
    /// This does not assert which way the answer goes — that depends on what this machine has
    /// been granted — but on what is said about it. A person who granted Multiplex Screen
    /// Recording has every reason to believe that covered the background service too, so the
    /// message has to name the service and say why the app's grant is not enough.
    #[test]
    #[cfg(target_os = "macos")]
    fn a_missing_grant_says_whose_grant_is_missing() {
        let Some(line) = screen_capture_line() else {
            // This machine has granted it, which is the other correct answer.
            assert!(multiplex_screen_capture::screen_capture_allowed());
            return;
        };
        assert!(
            line.contains(LAUNCH_AGENT_LABEL),
            "the message must name the process to allow: {line}"
        );
        assert!(
            line.contains("Screen Recording") || line.contains("Screen & System Audio Recording"),
            "and where to allow it: {line}"
        );
        assert!(
            line.contains("does not cover the service"),
            "and why granting the app was not enough: {line}"
        );
    }

    /// Stands in for a listener worker: holds the route lock while serving, like the real one.
    struct FakeListener {
        _ownership: ListenerOwnership,
        stopped: Arc<AtomicUsize>,
    }

    impl ServingListener for FakeListener {
        fn is_finished(&self) -> bool {
            false
        }

        fn stop(self: Box<Self>) {
            self.stopped.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn short_fixture() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("tr-svc-")
            .tempdir_in("/tmp")
            .unwrap()
    }

    fn wait_until(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !condition() {
            assert!(Instant::now() < deadline, "condition not met in time");
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn service_serves_yields_to_the_app_and_takes_the_route_back() {
        let fixture = short_fixture();
        let controller_root = fixture.path().join("controller");
        std::fs::create_dir_all(&controller_root).unwrap();
        let runtime_parent = fixture.path().join("runtime");
        let started = Arc::new(AtomicUsize::new(0));
        let stopped = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));

        let supervisor = {
            let controller_root = controller_root.clone();
            let runtime_parent = runtime_parent.clone();
            let started = started.clone();
            let stopped = stopped.clone();
            let stop = stop.clone();
            thread::spawn(move || {
                supervise(
                    &controller_root,
                    &runtime_parent,
                    || {
                        let ownership = ListenerOwnership::try_acquire(&controller_root)
                            .unwrap()
                            .ok_or(ServiceError("test.route_busy"))?;
                        started.fetch_add(1, Ordering::SeqCst);
                        Ok(Box::new(FakeListener {
                            _ownership: ownership,
                            stopped: stopped.clone(),
                        }) as Box<dyn ServingListener>)
                    },
                    || stop.load(Ordering::SeqCst),
                )
            })
        };

        wait_until(|| started.load(Ordering::SeqCst) == 1);
        assert!(
            ListenerOwnership::try_acquire(&controller_root)
                .unwrap()
                .is_none(),
            "the service owns the route while the app is closed"
        );

        // The app opens: it asks the service to yield, then its own listener takes the lock.
        assert!(request_yield(&runtime_parent));
        assert_eq!(stopped.load(Ordering::SeqCst), 1);
        let app_listener =
            ListenerOwnership::acquire_within(&controller_root, Duration::from_secs(5))
                .expect("the app's listener gets the route right after the service yields");
        thread::sleep(ROUTE_RETRY + Duration::from_millis(500));
        assert_eq!(
            started.load(Ordering::SeqCst),
            1,
            "the service must not restart while the app serves"
        );

        // The app quits: its listener exits and the service takes the route back.
        drop(app_listener);
        wait_until(|| started.load(Ordering::SeqCst) == 2);

        stop.store(true, Ordering::SeqCst);
        supervisor.join().unwrap().unwrap();
        assert_eq!(stopped.load(Ordering::SeqCst), 2);
        assert!(!yield_socket_path(&runtime_parent).exists());
    }

    #[test]
    fn a_disabled_route_is_retried_without_serving() {
        let fixture = short_fixture();
        let controller_root = fixture.path().join("controller");
        std::fs::create_dir_all(&controller_root).unwrap();
        let runtime_parent = fixture.path().join("runtime");
        let attempts = Arc::new(Mutex::new(Vec::new()));
        let stop_after = Instant::now() + ROUTE_RETRY * 2 + Duration::from_millis(500);
        let recorded = attempts.clone();
        supervise(
            &controller_root,
            &runtime_parent,
            || {
                recorded.lock().unwrap().push(Instant::now());
                Err(ServiceError("service.route_disabled"))
            },
            || Instant::now() >= stop_after,
        )
        .unwrap();
        let attempts = attempts.lock().unwrap();
        assert!(
            (2..=4).contains(&attempts.len()),
            "retries are paced, got {}",
            attempts.len()
        );
        assert!(
            attempts
                .windows(2)
                .all(|pair| pair[1] - pair[0] >= ROUTE_RETRY - Duration::from_millis(150))
        );
    }

    #[test]
    fn a_missing_controller_store_is_waited_for_not_fatal() {
        let fixture = short_fixture();
        let controller_root = fixture.path().join("not-created-yet");
        let runtime_parent = fixture.path().join("runtime");
        let stop_after = Instant::now() + Duration::from_millis(600);
        let mut starts = 0;
        supervise(
            &controller_root,
            &runtime_parent,
            || {
                starts += 1;
                Err(ServiceError("test.unexpected"))
            },
            || Instant::now() >= stop_after,
        )
        .expect("the service keeps running until the store exists");
        assert_eq!(starts, 0);
        assert!(!yield_socket_path(&runtime_parent).exists());
    }

    #[test]
    fn no_service_means_no_yield_and_no_delay() {
        let fixture = short_fixture();
        let started = Instant::now();
        assert!(!request_yield(fixture.path()));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn a_second_service_refuses_to_start() {
        let fixture = short_fixture();
        let runtime_parent = fixture.path().join("runtime");
        let _first = bind_yield_socket(&runtime_parent).unwrap();
        assert_eq!(
            bind_yield_socket(&runtime_parent).unwrap_err(),
            ServiceError("service.already_running")
        );
    }

    #[test]
    fn launch_agent_plist_is_escaped_and_valid() {
        let fixture = tempfile::tempdir().unwrap();
        let plist = launch_agent_plist(
            Path::new("/Applications/Terminal & Co <beta>.app/Contents/MacOS/multiplex"),
            Path::new("/Users/me/Library/Application Support/multiplex/controller-service.log"),
        );
        assert!(plist.contains("<string>com.millionrust.multiplex.controller-service</string>"));
        assert!(plist.contains("Terminal &amp; Co &lt;beta&gt;.app"));
        assert!(plist.contains("<string>controller-service</string>\n\t\t<string>run</string>"));
        let path = fixture.path().join("agent.plist");
        std::fs::write(&path, &plist).unwrap();
        if Path::new("/usr/bin/plutil").exists() {
            let lint = std::process::Command::new("/usr/bin/plutil")
                .arg("-lint")
                .arg(&path)
                .output()
                .unwrap();
            assert!(lint.status.success(), "{lint:?}");
        }
    }
}

/// What to say about the service's own screen recording grant, if anything.
///
/// macOS records a grant against the responsible process. The desktop app is responsible for its
/// own listener worker, so that one inherits the app's grant; the LaunchAgent has no responsible
/// parent, so it is asked for separately and under its own name. A person who granted the app
/// Screen Recording has every reason to think that settled it, which is why this says otherwise
/// explicitly rather than leaving a blank screen to be puzzled over.
#[cfg(target_os = "macos")]
fn screen_capture_line() -> Option<String> {
    if multiplex_screen_capture::screen_capture_allowed() {
        return None;
    }
    Some(format!(
        "Screens are not shared yet: macOS has not granted this service Screen Recording. \
         Open System Settings > Privacy & Security > Screen & System Audio Recording and allow \
         {LAUNCH_AGENT_LABEL}. Granting it to Multiplex itself does not cover the service, \
         because macOS records the grant against whichever process asked."
    ))
}

#[cfg(not(target_os = "macos"))]
fn screen_capture_line() -> Option<String> {
    None
}
