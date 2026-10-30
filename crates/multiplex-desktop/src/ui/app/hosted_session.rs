use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Condvar, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use multiplex_client::{
    ClientError, ClientErrorCode, ConnectOptions, GpuiAttachModel, HostClient,
    HostReconciliationService, LocalEndpoint, OutputDisposition,
};
use multiplex_domain::{
    CommandId, ContinuityLink, HostInstanceId, HostedSessionId, OccupantGeneration, OutputSequence,
    Revision,
};
use multiplex_host_protocol::wire;
use multiplex_session_host::{LaunchDescriptor, StopDeadlines};
use multiplex_store::{
    ContinuityRepository, HostLease, HostLeaseState, JournalKind, JournalLimits, JournalStore,
    RecoveryResult, load_snapshot, read_host_metadata,
};
use rand::RngCore as _;
use tokio::runtime::Builder;
use tokio::sync::mpsc::{self, UnboundedReceiver};
use tokio_util::sync::CancellationToken;

use crate::ssh::{SessionCommand, SessionRuntimeHandle, SshEvent, SshEventSender};
use crate::ui::localization;

const HOST_READY_DEADLINE: Duration = Duration::from_secs(5);
const CONNECT_RETRY_INTERVAL: Duration = Duration::from_millis(
    multiplex_ui_contract::DesignTokens::new(multiplex_ui_contract::ThemeKind::System)
        .motion_hosted_connect_poll(false)
        .0 as u64,
);
const LIVE_POLL_INTERVAL: Duration = Duration::from_millis(
    multiplex_ui_contract::DesignTokens::new(multiplex_ui_contract::ThemeKind::System)
        .motion_hosted_live_poll(false)
        .0 as u64,
);
const MAX_STARTUP_HANDSHAKES: usize = 8;
/// How long after the last keystroke a viewer lets go of the writer lease, so the terminal
/// window a `multiplex-cli shell` runs in, or a paired device, can type into it again.
const VIEWER_LEASE_IDLE_MILLIS: u64 = 1_500;
const VIEWER_LEASE_IDLE: Duration = Duration::from_millis(VIEWER_LEASE_IDLE_MILLIS);

/// The writer lease as an app viewer holds it: taken to type and let go once the typing stops,
/// the way a `multiplex-cli shell` window holds it, so neither keeps the other out for good.
struct ViewerLease {
    held: bool,
    /// Another client held the lease when this viewer last asked for it.
    refused: bool,
    last_used: Instant,
}

impl ViewerLease {
    /// The handshake asks for the lease, so a viewer that does not hold it at first was refused.
    fn new() -> Self {
        Self {
            held: false,
            refused: true,
            last_used: Instant::now(),
        }
    }

    /// Whether typing here can reach the program: this viewer holds the lease, or nobody else
    /// was found holding it.
    fn writable(&self) -> bool {
        self.held || !self.refused
    }

    /// Takes the lease to type, unless another client holds it.
    async fn take(
        &mut self,
        client: &mut HostClient,
        cancel: &CancellationToken,
    ) -> Result<bool, ClientError> {
        if !self.held {
            self.held = client
                .set_writer_lease(CommandId::new(), true, cancel)
                .await?;
            self.refused = !self.held;
        }
        if self.held {
            self.last_used = Instant::now();
        }
        Ok(self.held)
    }

    /// Follows what the Host reports on each poll: lets go of a lease nobody has typed with for
    /// a while, and looks again whether a writer that refused this viewer has let go.
    async fn settle(
        &mut self,
        has_writer_lease: bool,
        client: &mut HostClient,
        cancel: &CancellationToken,
    ) -> Result<(), ClientError> {
        self.held = has_writer_lease;
        if self.held {
            self.refused = false;
            if self.last_used.elapsed() >= VIEWER_LEASE_IDLE {
                self.held = client
                    .set_writer_lease(CommandId::new(), false, cancel)
                    .await?;
            }
        } else if self.refused
            && client
                .set_writer_lease(CommandId::new(), true, cancel)
                .await?
        {
            client
                .set_writer_lease(CommandId::new(), false, cancel)
                .await?;
            self.refused = false;
        }
        Ok(())
    }
}

struct StartupPermit;

impl StartupPermit {
    fn acquire() -> Self {
        let (lock, ready) = startup_slots();
        let mut active = lock.lock().unwrap_or_else(|error| error.into_inner());
        while *active >= MAX_STARTUP_HANDSHAKES {
            active = ready
                .wait(active)
                .unwrap_or_else(|error| error.into_inner());
        }
        *active += 1;
        Self
    }
}

impl Drop for StartupPermit {
    fn drop(&mut self) {
        let (lock, ready) = startup_slots();
        let mut active = lock.lock().unwrap_or_else(|error| error.into_inner());
        *active = active.saturating_sub(1);
        ready.notify_one();
    }
}

fn startup_slots() -> &'static (Mutex<usize>, Condvar) {
    static SLOTS: OnceLock<(Mutex<usize>, Condvar)> = OnceLock::new();
    SLOTS.get_or_init(|| (Mutex::new(0), Condvar::new()))
}

#[derive(Clone, Debug)]
pub(super) struct DurableSessionPaths {
    pub runtime_root: PathBuf,
    pub session_dir: PathBuf,
}

impl DurableSessionPaths {
    pub fn create(app_dir: &Path, session_id: HostedSessionId) -> std::io::Result<Self> {
        let runtime_parent = durable_runtime_parent(app_dir);
        let sessions_parent = app_dir.join("durable-sessions");
        create_user_only_directory(&runtime_parent)?;
        create_user_only_directory(&sessions_parent)?;
        Ok(Self {
            runtime_root: runtime_parent.join(session_id.to_string()),
            session_dir: sessions_parent.join(session_id.to_string()),
        })
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
fn durable_runtime_parent(app_dir: &Path) -> PathBuf {
    app_dir.join("session-host-runtime")
}

#[cfg(unix)]
fn create_user_only_directory(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt as _;
    use std::os::unix::fs::PermissionsExt as _;

    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "durable session directory is not a trusted directory",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => fs::create_dir_all(path)?,
        Err(error) => return Err(error),
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    let metadata = fs::symlink_metadata(path)?;
    if metadata.uid() != unsafe { libc::geteuid() }
        || metadata.permissions().mode() & 0o777 != 0o700
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "durable session directory has unsafe ownership or permissions",
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn create_user_only_directory(path: &Path) -> std::io::Result<()> {
    fs::create_dir_all(path)
}

pub(super) struct DurableSessionSpec {
    pub pane_id: u64,
    pub session_id: HostedSessionId,
    pub paths: DurableSessionPaths,
    pub launch: Option<DurableLaunch>,
    pub from_sequence: OutputSequence,
    pub expected_occupant_generation: Option<OccupantGeneration>,
    pub continuity: Option<DurableContinuityCommit>,
}

pub(super) struct DurableContinuityCommit {
    pub store_root: PathBuf,
    pub expected_revision: Revision,
    pub link: ContinuityLink,
}

pub(super) struct DurableLaunch {
    pub executable: PathBuf,
    pub arguments: Vec<String>,
    pub cwd: PathBuf,
    pub runtime_detection: Option<multiplex_domain::RuntimeDetectionResult>,
}

pub(super) fn spawn_durable_session(
    spec: DurableSessionSpec,
    event_tx: SshEventSender,
) -> SessionRuntimeHandle {
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    let pane_id = spec.pane_id;
    let fallback = event_tx.clone();
    let thread_fallback = fallback.clone();
    let result = thread::Builder::new()
        .name(format!("durable-session-{pane_id}"))
        .spawn(move || {
            let result = run_durable_session(spec, command_rx, event_tx);
            if let Err(message) = result {
                let _ = thread_fallback.send(SshEvent::HostedStatus {
                    session_id: pane_id,
                    state: multiplex_domain::HostedSessionState::Offline,
                    last_sequence: 0,
                    durable_sequence: 0,
                    activity: multiplex_domain::ActivityAggregate::default(),
                    has_writer_lease: false,
                    detail: message.clone(),
                });
                let _ = thread_fallback.send(SshEvent::Disconnected {
                    session_id: pane_id,
                    message,
                });
            }
        });
    if let Err(error) = result {
        let _ = fallback.send(SshEvent::Error {
            session_id: pane_id,
            message: localization::hosted_session_worker_start_error(error.to_string()),
        });
    }
    SessionRuntimeHandle { command_tx }
}

fn run_durable_session(
    mut spec: DurableSessionSpec,
    mut command_rx: UnboundedReceiver<SessionCommand>,
    event_tx: SshEventSender,
) -> Result<(), String> {
    let startup_permit = StartupPermit::acquire();
    if let Some(launch) = spec.launch.take() {
        event_tx
            .send(status_event(
                spec.pane_id,
                multiplex_domain::HostedSessionState::Provisioning,
                spec.from_sequence,
                false,
                "Starting durable Host",
            ))
            .map_err(|_| "Application event channel closed".to_string())?;
        if !launch_host_process(&spec, launch, &mut command_rx, &event_tx)? {
            return Ok(());
        }
    }

    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("Unable to create durable client runtime: {error}"))?;
    runtime.block_on(attach_loop(spec, command_rx, event_tx, startup_permit))
}

fn launch_host_process(
    spec: &DurableSessionSpec,
    launch: DurableLaunch,
    command_rx: &mut UnboundedReceiver<SessionCommand>,
    event_tx: &SshEventSender,
) -> Result<bool, String> {
    let host_executable = fs::canonicalize(default_host_executable()?)
        .map_err(|error| format!("Unable to verify durable Host executable: {error}"))?;
    let mut environment = BTreeMap::new();
    for name in [
        "CODEX_HOME",
        "HOME",
        "LANG",
        "LC_ALL",
        "PATH",
        "SHELL",
        "TERM",
    ] {
        if let Ok(value) = std::env::var(name) {
            environment.insert(name.to_string(), value);
        }
    }
    environment
        .entry("TERM".to_string())
        .or_insert_with(|| "xterm-256color".to_string());
    environment.insert(
        "TERM_PROGRAM".to_string(),
        crate::local::TERMINAL_PROGRAM.to_string(),
    );
    let descriptor = LaunchDescriptor {
        format_version: LaunchDescriptor::FORMAT_VERSION,
        session_id: spec.session_id,
        host_instance_id: HostInstanceId::new(),
        expected_occupant_generation: spec.expected_occupant_generation,
        runtime_root: spec.paths.runtime_root.clone(),
        session_dir: spec.paths.session_dir.clone(),
        executable: launch.executable,
        runtime_detection: launch.runtime_detection,
        arguments: launch.arguments,
        environment,
        cwd: Some(launch.cwd),
        columns: 160,
        rows: 48,
        journal_limits: JournalLimits::default(),
        stop_deadlines: StopDeadlines::default(),
    };
    let mut command = Command::new(host_executable);
    // A Host a test starts stops when the test process does, even when its cleanup never runs.
    #[cfg(test)]
    command.env(
        multiplex_session_host::OWNER_PROCESS_VARIABLE,
        std::process::id().to_string(),
    );
    let mut process = command
        .arg("--session-host")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Unable to spawn durable Host: {error}"))?;
    serde_json::to_writer(
        process
            .stdin
            .as_mut()
            .ok_or_else(|| "Durable Host descriptor pipe is unavailable".to_string())?,
        &descriptor,
    )
    .map_err(|error| format!("Unable to encode durable Host descriptor: {error}"))?;
    process
        .stdin
        .take()
        .ok_or_else(|| "Durable Host descriptor pipe is unavailable".to_string())?
        .flush()
        .map_err(|error| format!("Unable to send durable Host descriptor: {error}"))?;

    let stdout = process
        .stdout
        .take()
        .ok_or_else(|| "Durable Host readiness pipe is unavailable".to_string())?;
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut line = String::new();
        let result = BufReader::new(stdout).read_line(&mut line).map(|_| line);
        let _ = ready_tx.send(result);
    });

    let deadline = Instant::now() + HOST_READY_DEADLINE;
    loop {
        if matches!(command_rx.try_recv(), Ok(SessionCommand::Disconnect)) {
            terminate_unready_host(&mut process);
            let _ = event_tx.send(SshEvent::Disconnected {
                session_id: spec.pane_id,
                message: localization::hosted_session_launch_cancelled(),
            });
            return Ok(false);
        }
        match ready_rx.recv_timeout(CONNECT_RETRY_INTERVAL) {
            Ok(Ok(line)) if line.contains("\"code\":\"host_ready\"") => break,
            Ok(Ok(line)) => {
                let failure = read_host_failure_code(&mut process);
                terminate_unready_host(&mut process);
                let code = if line.trim().is_empty() {
                    failure.as_deref().unwrap_or("empty response")
                } else {
                    "unexpected response"
                };
                return Err(format!(
                    "Durable Host returned an invalid readiness message ({code})"
                ));
            }
            Ok(Err(error)) => {
                terminate_unready_host(&mut process);
                return Err(format!("Unable to read durable Host readiness: {error}"));
            }
            Err(RecvTimeoutError::Disconnected) => {
                terminate_unready_host(&mut process);
                return Err("Durable Host exited before becoming ready".to_string());
            }
            Err(RecvTimeoutError::Timeout) if Instant::now() >= deadline => {
                terminate_unready_host(&mut process);
                return Err("Durable Host did not become ready within 5 seconds".to_string());
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
    if let Some(commit) = spec.continuity.as_ref() {
        let result = ContinuityRepository::open(&commit.store_root).and_then(|repository| {
            repository.record(commit.expected_revision, commit.link.clone())
        });
        if let Err(error) = result {
            stop_ready_host(spec, &mut process);
            return Err(format!(
                "Unable to commit durable session continuity: {error}"
            ));
        }
    }
    thread::spawn(move || {
        let _ = process.wait();
    });
    Ok(true)
}

fn stop_ready_host(spec: &DurableSessionSpec, process: &mut Child) {
    let result = Builder::new_current_thread()
        .enable_all()
        .build()
        .and_then(|runtime| {
            runtime.block_on(async {
                let cancel = CancellationToken::new();
                let mut nonce = [0_u8; 32];
                rand::rngs::OsRng.fill_bytes(&mut nonce);
                let endpoint = LocalEndpoint::new(&spec.paths.runtime_root, spec.session_id);
                let mut client = HostClient::connect(
                    endpoint,
                    ConnectOptions::local(spec.session_id, nonce),
                    &cancel,
                )
                .await
                .map_err(std::io::Error::other)?;
                client
                    .stop(CommandId::new(), wire::StopMode::Force, &cancel)
                    .await
                    .map_err(std::io::Error::other)?;
                Ok(())
            })
        });
    if result.is_err() {
        let _ = process.kill();
    }
    let deadline = Instant::now() + Duration::from_secs(6);
    while process.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(
            multiplex_ui_contract::DesignTokens::new(multiplex_ui_contract::ThemeKind::System)
                .motion_hosted_settle_poll(false)
                .0 as u64,
        ));
    }
    if process.try_wait().ok().flatten().is_none() {
        let _ = process.kill();
    }
    let _ = process.wait();
}

fn read_host_failure_code(process: &mut Child) -> Option<String> {
    process.try_wait().ok().flatten()?;
    let mut bytes = Vec::new();
    process
        .stderr
        .take()?
        .take(4 * 1024)
        .read_to_end(&mut bytes)
        .ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let marker = "\"code\":\"";
    let code = text.split_once(marker)?.1.split_once('"')?.0;
    let kind = text
        .split_once("\"io_kind\":\"")
        .and_then(|(_, value)| value.split_once('"'))
        .map(|(value, _)| value)
        .unwrap_or("None");
    (!code.is_empty()).then(|| format!("{code}/{kind}"))
}

fn default_host_executable() -> Result<PathBuf, String> {
    if let Some(path) = multiplex_env::var_os("MULTIPLEX_SESSION_HOST_BIN") {
        return Ok(PathBuf::from(path));
    }
    let current = std::env::current_exe().map_err(|error| error.to_string())?;
    // A unit test runs from the test harness in `deps/`, and handing the harness to a durable
    // Host launch starts another copy of the test suite instead of a Host: it either rejects the
    // Host's arguments and exits, leaving the caller to wait out the whole ready deadline, or
    // takes them as test filters and runs tests of its own in the background. The real binary
    // sits one level up once it has been built, and without it there is nothing correct to
    // launch, so say that rather than launching the wrong thing.
    #[cfg(test)]
    if current
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name == "deps")
        && let Some(debug_dir) = current.parent().and_then(Path::parent)
    {
        let packaged = debug_dir.join(format!("multiplex{}", std::env::consts::EXE_SUFFIX));
        return if packaged.is_file() {
            Ok(packaged)
        } else {
            Err(format!(
                "the durable Host binary {} has not been built, and a unit test launches it rather \
                 than itself; build it first with `cargo build -p multiplex`, or run the suite \
                 with --all-targets, which builds it",
                packaged.display()
            ))
        };
    }
    Ok(current)
}

fn terminate_unready_host(process: &mut Child) {
    let _ = process.kill();
    let _ = process.wait();
}

async fn attach_loop(
    spec: DurableSessionSpec,
    mut command_rx: UnboundedReceiver<SessionCommand>,
    event_tx: SshEventSender,
    startup_permit: StartupPermit,
) -> Result<(), String> {
    let cancel = CancellationToken::new();
    let endpoint = LocalEndpoint::new(&spec.paths.runtime_root, spec.session_id);
    let mut nonce = [0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let connect_deadline = Instant::now() + HOST_READY_DEADLINE;
    let mut client = loop {
        match HostClient::connect(
            endpoint.clone(),
            ConnectOptions::local(spec.session_id, nonce),
            &cancel,
        )
        .await
        {
            Ok(client) => break client,
            Err(_error)
                if Instant::now() < connect_deadline
                    && !multiplex_store::probe_host_lease(&spec.paths.session_dir)
                        .is_ok_and(|lease| lease == multiplex_store::HostLeaseState::Available) =>
            {
                if matches!(command_rx.try_recv(), Ok(SessionCommand::Disconnect)) {
                    return Ok(());
                }
                tokio::time::sleep(CONNECT_RETRY_INTERVAL).await;
                rand::rngs::OsRng.fill_bytes(&mut nonce);
            }
            Err(error) => {
                if replay_retained_output(&spec, &event_tx, &cancel).await? {
                    return Ok(());
                }
                report_client_failure(&spec, &event_tx, error, spec.from_sequence)?;
                return Ok(());
            }
        }
    };
    drop(startup_permit);
    let host_instance_id = client
        .host_instance_id()
        .ok_or_else(|| "Durable Host did not provide an instance identity".to_string())?;
    event_tx
        .send(SshEvent::HostedBound {
            session_id: spec.pane_id,
            host_instance_id,
        })
        .map_err(|_| "Application event channel closed".to_string())?;

    let mut model = GpuiAttachModel::new(spec.from_sequence);
    event_tx
        .send(status_event(
            spec.pane_id,
            multiplex_domain::HostedSessionState::Attaching,
            model.watermark(),
            false,
            "Attaching to durable Host",
        ))
        .map_err(|_| "Application event channel closed".to_string())?;
    let mut ticker = tokio::time::interval(LIVE_POLL_INTERVAL);
    let mut connected_sent = false;
    let mut lease = ViewerLease::new();

    loop {
        tokio::select! {
            command = command_rx.recv() => match command {
                Some(SessionCommand::Input(bytes)) => {
                    // The Host ends a connection that writes without the lease, so typing while
                    // another client holds it is dropped and the next poll shows it read-only.
                    let writable = lease.take(&mut client, &cancel).await
                        .map_err(|error| format!("Durable Host writer lease failed: {error}"))?;
                    if writable && !client.input_all(&bytes, &cancel).await
                        .map_err(|error| format!("Durable Host input failed: {error}"))?
                    {
                        lease.held = false;
                        lease.refused = true;
                    }
                }
                Some(SessionCommand::Resize(size)) => {
                    client.resize(CommandId::new(), u32::from(size.cols), u32::from(size.rows), &cancel).await
                        .map_err(|error| format!("Durable Host resize failed: {error}"))?;
                }
                Some(SessionCommand::StopDurable) => {
                    let stop_result = client
                        .stop(CommandId::new(), wire::StopMode::Graceful, &cancel)
                        .await;
                    client.disconnect();
                    let (state, status, disconnected) = match stop_result {
                        Ok(_) => (
                            multiplex_domain::HostedSessionState::Exited,
                            "Stopped; retained output is read-only",
                            "Durable session stopped",
                        ),
                        Err(error) => {
                            eprintln!("[durable-session] Host stop failed: {error}");
                            (
                                multiplex_domain::HostedSessionState::Orphaned,
                                "Stop was not confirmed; retained output and process evidence were preserved",
                                "Durable session stop was not confirmed",
                            )
                        }
                    };
                    let _ = event_tx.send(status_event(
                        spec.pane_id,
                        state,
                        model.watermark(),
                        false,
                        status,
                    ));
                    let _ = event_tx.send(SshEvent::Disconnected {
                        session_id: spec.pane_id,
                        message: disconnected.to_string(),
                    });
                    return Ok(());
                }
                Some(SessionCommand::Disconnect) | None => {
                    let _ = client.detach(&cancel).await;
                    let _ = event_tx.send(SshEvent::Disconnected {
                        session_id: spec.pane_id,
                        message: localization::hosted_session_detached(),
                    });
                    return Ok(());
                }
                Some(SessionCommand::KillTmuxSession { .. }) => {}
            },
            _ = ticker.tick() => {
                model.begin_replay();
                if !connected_sent {
                    event_tx.send(status_event(
                        spec.pane_id,
                        multiplex_domain::HostedSessionState::Replaying,
                        model.watermark(),
                        false,
                        "Replaying retained output",
                    )).map_err(|_| "Application event channel closed".to_string())?;
                }
                let outputs = match tokio::time::timeout(
                    Duration::from_secs(2),
                    client.attach(model.watermark(), 160, 48, &cancel),
                )
                .await
                {
                    Ok(Ok(outputs)) => outputs,
                    Ok(Err(error)) => {
                        report_client_failure(&spec, &event_tx, error, model.watermark())?;
                        return Ok(());
                    }
                    Err(_) => {
                        report_recovery_state(
                            &spec,
                            &event_tx,
                            multiplex_domain::HostedSessionState::Offline,
                            model.watermark(),
                            "Replay timed out; retry to reconnect to the durable Host",
                        )?;
                        return Ok(());
                    }
                };
                if let Some(snapshot) = client.take_last_snapshot() {
                    let boundary = OutputSequence::new(snapshot.boundary_sequence);
                    if model.apply_snapshot(boundary) {
                        event_tx.send(SshEvent::HostedSnapshot {
                            session_id: spec.pane_id,
                            host_instance_id,
                            data: snapshot.terminal_bytes,
                            boundary_sequence: boundary.get(),
                        }).map_err(|_| "Application event channel closed".to_string())?;
                    }
                }
                for output in outputs {
                    match model.observe_output(output.sequence, output.bytes.len()) {
                        OutputDisposition::Deliver => {
                            event_tx.send(SshEvent::HostedOutput {
                                session_id: spec.pane_id,
                                host_instance_id,
                                output_sequence: output.sequence,
                                data: output.bytes,
                            }).map_err(|_| "Application event channel closed".to_string())?;
                        }
                        OutputDisposition::Duplicate => {}
                        OutputDisposition::Gap { expected, received } => {
                            report_recovery_state(
                                &spec,
                                &event_tx,
                                multiplex_domain::HostedSessionState::Gap,
                                model.watermark(),
                                &format!(
                                    "Output history is incomplete: expected {expected}, received {received}"
                                ),
                            )?;
                            return Ok(());
                        }
                    }
                }
                let state = client.take_last_state().ok_or_else(|| {
                    "Durable Host did not return an attach state watermark".to_string()
                })?;
                let activity = client
                    .request_activity_snapshot(&cancel)
                    .await
                    .unwrap_or_default();
                let lifecycle = wire::Lifecycle::try_from(state.lifecycle)
                    .unwrap_or(wire::Lifecycle::Unspecified);
                if matches!(lifecycle, wire::Lifecycle::Exited | wire::Lifecycle::Failed) {
                    model.mark_dead();
                    let _ = event_tx.send(status_event_with_durability(
                        spec.pane_id,
                        multiplex_domain::HostedSessionState::Exited,
                        model.watermark(),
                        OutputSequence::new(state.durable_sequence),
                        false,
                        activity,
                        "Process exited; retained output is read-only",
                    ));
                    let _ = event_tx.send(SshEvent::Disconnected {
                        session_id: spec.pane_id,
                        message: localization::hosted_session_process_exited(),
                    });
                    return Ok(());
                }
                lease.settle(state.has_writer_lease, &mut client, &cancel).await
                    .map_err(|error| format!("Durable Host writer lease failed: {error}"))?;
                model.mark_live(lease.writable(), state.recording_paused);
                if !connected_sent {
                    event_tx.send(SshEvent::Connected {
                        session_id: spec.pane_id,
                        trusted_new_host: false,
                    }).map_err(|_| "Application event channel closed".to_string())?;
                    connected_sent = true;
                }
                event_tx.send(status_event_with_durability(
                    spec.pane_id,
                    if state.recording_paused {
                        multiplex_domain::HostedSessionState::RecordingPaused
                    } else {
                        multiplex_domain::HostedSessionState::Live
                    },
                    model.watermark(),
                    OutputSequence::new(state.durable_sequence),
                    lease.writable(),
                    activity,
                    if state.recording_paused {
                        "Live; output recording paused at the disk limit"
                    } else if lease.writable() {
                        "Live and writable"
                    } else {
                        "Live read-only; another client owns input"
                    },
                )).map_err(|_| "Application event channel closed".to_string())?;
            }
        }
    }
}

fn report_client_failure(
    spec: &DurableSessionSpec,
    event_tx: &SshEventSender,
    error: ClientError,
    sequence: OutputSequence,
) -> Result<(), String> {
    let (state, detail) = recovery_for_client_error(error);
    report_recovery_state(spec, event_tx, state, sequence, detail)
}

fn recovery_for_client_error(
    error: ClientError,
) -> (multiplex_domain::HostedSessionState, &'static str) {
    match error.code {
        ClientErrorCode::ProtocolIncompatible => (
            multiplex_domain::HostedSessionState::Incompatible,
            "Host protocol is incompatible; update Multiplex before retrying",
        ),
        ClientErrorCode::PermissionDenied
        | ClientErrorCode::WrongSession
        | ClientErrorCode::InvalidIdentity
        | ClientErrorCode::HandshakeReplay => (
            multiplex_domain::HostedSessionState::PermissionDenied,
            "Host identity was rejected; retained data was not modified",
        ),
        ClientErrorCode::SequenceGap
        | ClientErrorCode::ConflictingDuplicate
        | ClientErrorCode::ChecksumMismatch
        | ClientErrorCode::MalformedFrame
        | ClientErrorCode::FrameTooLarge => (
            multiplex_domain::HostedSessionState::Gap,
            "Output history could not be verified; retained output may be incomplete",
        ),
        ClientErrorCode::Io
        | ClientErrorCode::EndOfStream
        | ClientErrorCode::ResourceLimit
        | ClientErrorCode::Cancelled
        | ClientErrorCode::InvalidState => (
            multiplex_domain::HostedSessionState::Offline,
            "Durable Host is offline; retry to reconcile and reconnect",
        ),
    }
}

fn report_recovery_state(
    spec: &DurableSessionSpec,
    event_tx: &SshEventSender,
    state: multiplex_domain::HostedSessionState,
    sequence: OutputSequence,
    detail: &str,
) -> Result<(), String> {
    event_tx
        .send(status_event(spec.pane_id, state, sequence, false, detail))
        .map_err(|_| "Application event channel closed".to_string())?;
    let _ = event_tx.send(SshEvent::Disconnected {
        session_id: spec.pane_id,
        message: detail.to_string(),
    });
    Ok(())
}

async fn replay_retained_output(
    spec: &DurableSessionSpec,
    event_tx: &SshEventSender,
    cancel: &CancellationToken,
) -> Result<bool, String> {
    let recovery = HostReconciliationService::new(&spec.paths.runtime_root);
    let plan = match recovery.plan(&spec.paths.session_dir).await {
        Ok(plan) => plan,
        Err(_) => return Ok(false),
    };
    if plan.lease_state == HostLeaseState::Held || plan.preview_result == RecoveryResult::Ambiguous
    {
        return Ok(false);
    }
    let prior_lifecycle = plan.lifecycle;
    let reconciliation = recovery
        .reconcile(plan, cancel)
        .map_err(|error| format!("Unable to reconcile retained session output: {error}"))?;
    if !matches!(
        reconciliation.result,
        RecoveryResult::Reconciled | RecoveryResult::NoChange
    ) {
        return Ok(false);
    }
    let metadata = read_host_metadata(&spec.paths.session_dir).ok();
    let activity = metadata
        .as_ref()
        .map(|metadata| metadata.activity.clone())
        .unwrap_or_default();
    let lease = HostLease::acquire(&spec.paths.session_dir, HostInstanceId::new())
        .map_err(|error| format!("Unable to open retained session output: {error}"))?;
    let host_instance_id = metadata
        .map(|metadata| metadata.host_instance_id)
        .unwrap_or_else(|| lease.host_instance_id());
    event_tx
        .send(SshEvent::HostedBound {
            session_id: spec.pane_id,
            host_instance_id,
        })
        .map_err(|_| "Application event channel closed".to_string())?;
    let journal = JournalStore::open(&lease, JournalLimits::default())
        .map_err(|error| format!("Unable to read retained session output: {error}"))?;
    let from = match load_snapshot(&spec.paths.session_dir)
        .map_err(|error| format!("Unable to read retained terminal snapshot: {error}"))?
    {
        Some(snapshot) => {
            event_tx
                .send(SshEvent::HostedSnapshot {
                    session_id: spec.pane_id,
                    host_instance_id,
                    data: snapshot.terminal_bytes,
                    boundary_sequence: snapshot.boundary.get(),
                })
                .map_err(|_| "Application event channel closed".to_string())?;
            snapshot.boundary
        }
        None => OutputSequence::ZERO,
    };
    let read = journal
        .read_from(from)
        .map_err(|error| format!("Unable to replay retained session output: {error}"))?;
    for frame in read.frames {
        if frame.kind == JournalKind::Output {
            event_tx
                .send(SshEvent::HostedOutput {
                    session_id: spec.pane_id,
                    host_instance_id,
                    output_sequence: frame.sequence,
                    data: frame.payload,
                })
                .map_err(|_| "Application event channel closed".to_string())?;
        }
    }
    let state = if reconciliation.result == RecoveryResult::Reconciled
        || prior_lifecycle == multiplex_domain::HostLifecycle::Orphaned
    {
        multiplex_domain::HostedSessionState::Orphaned
    } else {
        multiplex_domain::HostedSessionState::Exited
    };
    let last_sequence = read.latest.unwrap_or(from);
    event_tx
        .send(status_event_with_durability(
            spec.pane_id,
            state,
            last_sequence,
            journal.latest_sequence(),
            false,
            activity,
            if state == multiplex_domain::HostedSessionState::Orphaned {
                "Host is gone; retained output is read-only and cannot be stopped"
            } else {
                "Process exited; retained output is read-only"
            },
        ))
        .map_err(|_| "Application event channel closed".to_string())?;
    let _ = event_tx.send(SshEvent::Disconnected {
        session_id: spec.pane_id,
        message: localization::hosted_session_retained_read_only(),
    });
    Ok(true)
}

fn status_event(
    pane_id: u64,
    state: multiplex_domain::HostedSessionState,
    sequence: OutputSequence,
    has_writer_lease: bool,
    detail: &str,
) -> SshEvent {
    status_event_with_durability(
        pane_id,
        state,
        sequence,
        OutputSequence::ZERO,
        has_writer_lease,
        multiplex_domain::ActivityAggregate::default(),
        detail,
    )
}

fn status_event_with_durability(
    pane_id: u64,
    state: multiplex_domain::HostedSessionState,
    sequence: OutputSequence,
    durable_sequence: OutputSequence,
    has_writer_lease: bool,
    activity: multiplex_domain::ActivityAggregate,
    detail: &str,
) -> SshEvent {
    SshEvent::HostedStatus {
        session_id: pane_id,
        state,
        last_sequence: sequence.get(),
        durable_sequence: durable_sequence.get(),
        activity,
        has_writer_lease,
        detail: detail.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durable_paths_are_stable_and_scoped_to_the_session() {
        let fixture = tempfile::tempdir().unwrap();
        let session_id = HostedSessionId::new();
        let paths = DurableSessionPaths::create(fixture.path(), session_id).unwrap();
        assert!(paths.runtime_root.ends_with(session_id.to_string()));
        assert!(paths.session_dir.ends_with(session_id.to_string()));
        assert_ne!(paths.runtime_root, paths.session_dir);
        #[cfg(unix)]
        assert!(paths.runtime_root.as_os_str().len() < 90);
    }

    #[test]
    fn attaching_to_a_dead_host_replays_output_without_the_startup_retry_delay() {
        use multiplex_domain::{ActivityAggregate, HostLifecycle};
        use multiplex_store::HostMetadata;
        let fixture = tempfile::tempdir().unwrap();
        let session_id = HostedSessionId::new();
        let paths = DurableSessionPaths {
            session_dir: fixture.path().join("session"),
            runtime_root: fixture.path().join("runtime"),
        };
        let host_id = HostInstanceId::new();
        let lease = HostLease::acquire(&paths.session_dir, host_id).unwrap();
        lease
            .write_metadata(&HostMetadata {
                format_version: HostMetadata::FORMAT_VERSION,
                session_id,
                host_instance_id: host_id,
                process_token: None,
                runtime_recognition: None,
                activity: ActivityAggregate::default(),
                lifecycle: HostLifecycle::Ready,
                endpoint_name: "host.sock".into(),
                heartbeat_monotonic_nanos: 1,
                durability_watermark: None,
            })
            .unwrap();
        drop(JournalStore::open(&lease, JournalLimits::default()).unwrap());
        drop(lease);
        let spec = DurableSessionSpec {
            pane_id: 42,
            session_id,
            paths,
            launch: None,
            from_sequence: OutputSequence::ZERO,
            expected_occupant_generation: None,
            continuity: None,
        };
        let (events, received, _) = SshEventSender::channel();
        let (_commands, command_rx) = mpsc::unbounded_channel();
        let started = Instant::now();
        run_durable_session(spec, command_rx, events).unwrap();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(received.try_iter().any(|event| matches!(
            event,
            SshEvent::HostedStatus {
                session_id: 42,
                state: multiplex_domain::HostedSessionState::Orphaned,
                ..
            }
        )));
    }

    #[test]
    fn status_events_keep_pane_and_durable_sequence_separate() {
        let event = status_event(
            42,
            multiplex_domain::HostedSessionState::Live,
            OutputSequence::new(9),
            true,
            "Live",
        );
        assert!(matches!(
            event,
            SshEvent::HostedStatus {
                session_id: 42,
                last_sequence: 9,
                has_writer_lease: true,
                ..
            }
        ));
    }

    #[test]
    fn recovery_errors_map_to_truthful_user_states() {
        assert_eq!(
            recovery_for_client_error(ClientError::new(ClientErrorCode::ProtocolIncompatible)).0,
            multiplex_domain::HostedSessionState::Incompatible
        );
        assert_eq!(
            recovery_for_client_error(ClientError::new(ClientErrorCode::PermissionDenied)).0,
            multiplex_domain::HostedSessionState::PermissionDenied
        );
        assert_eq!(
            recovery_for_client_error(ClientError::new(ClientErrorCode::SequenceGap)).0,
            multiplex_domain::HostedSessionState::Gap
        );
        assert_eq!(
            recovery_for_client_error(ClientError::new(ClientErrorCode::EndOfStream)).0,
            multiplex_domain::HostedSessionState::Offline
        );
    }

    #[test]
    fn unit_tests_resolve_the_packaged_binary_instead_of_the_test_harness() {
        let executable = default_host_executable().unwrap();
        assert!(executable.is_file());
        assert!(
            !executable
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .contains('-')
        );
    }

    /// A paste larger than one Host request reaches the program whole, in order, rather than
    /// being refused and ending the session.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_paste_larger_than_one_request_reaches_the_program_whole() {
        let fixture = tempfile::Builder::new()
            .prefix("multiplex-paste-")
            .tempdir_in("/tmp")
            .unwrap();
        let root = fs::canonicalize(fixture.path()).unwrap();
        let session_id = HostedSessionId::new();
        let session_dir = root.join("session");
        fs::create_dir_all(&session_dir).unwrap();
        let runtime_root = root.join("runtime");
        let received = root.join("received");
        let ready = root.join("ready");
        let paste: Vec<u8> = (0..multiplex_host_protocol::MAX_OUTPUT_BYTES * 3 + 123)
            .map(|index| b'a' + (index % 26) as u8)
            .collect();
        let host = multiplex_session_host::start(LaunchDescriptor {
            format_version: LaunchDescriptor::FORMAT_VERSION,
            session_id,
            host_instance_id: HostInstanceId::new(),
            expected_occupant_generation: None,
            runtime_root: runtime_root.clone(),
            session_dir,
            executable: fs::canonicalize("/bin/sh").unwrap(),
            runtime_detection: None,
            arguments: vec![
                "-c".into(),
                format!(
                    "stty raw -echo; : > '{}'; head -c {} > '{}'; sleep 30",
                    ready.display(),
                    paste.len(),
                    received.display()
                ),
            ],
            environment: BTreeMap::new(),
            cwd: Some(root),
            columns: 80,
            rows: 24,
            journal_limits: JournalLimits::default(),
            stop_deadlines: StopDeadlines::default(),
        })
        .await
        .unwrap();
        let cancel = CancellationToken::new();
        let mut client = HostClient::connect(
            LocalEndpoint::new(&runtime_root, session_id),
            ConnectOptions::local(session_id, [23; 32]),
            &cancel,
        )
        .await
        .unwrap();
        assert!(
            client
                .set_writer_lease(CommandId::new(), true, &cancel)
                .await
                .unwrap()
        );
        // `stty raw` has to be in place before the first byte, or the line discipline would
        // hold the paste to a line's length.
        let deadline = Instant::now() + Duration::from_secs(20);
        while !ready.exists() {
            assert!(Instant::now() < deadline, "the program never started");
            tokio::time::sleep(LIVE_POLL_INTERVAL).await;
        }
        assert!(client.input_all(&paste, &cancel).await.unwrap());
        loop {
            if fs::read(&received).is_ok_and(|bytes| bytes.len() == paste.len()) {
                break;
            }
            assert!(Instant::now() < deadline, "the paste never arrived whole");
            tokio::time::sleep(LIVE_POLL_INTERVAL).await;
        }
        assert_eq!(fs::read(&received).unwrap(), paste);
        client
            .stop(CommandId::new(), wire::StopMode::Force, &cancel)
            .await
            .unwrap();
        host.wait().await.unwrap();
    }

    /// The app viewer and the terminal window a `multiplex-cli shell` runs in take turns: the
    /// viewer lets go once it stops being typed in, and never keeps a lease it only looked at.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_viewer_lets_the_terminal_window_type_once_it_stops_typing() {
        let fixture = tempfile::Builder::new()
            .prefix("multiplex-lease-")
            .tempdir_in("/tmp")
            .unwrap();
        let root = fs::canonicalize(fixture.path()).unwrap();
        let session_id = HostedSessionId::new();
        let session_dir = root.join("session");
        fs::create_dir_all(&session_dir).unwrap();
        let runtime_root = root.join("runtime");
        let host = multiplex_session_host::start(LaunchDescriptor {
            format_version: LaunchDescriptor::FORMAT_VERSION,
            session_id,
            host_instance_id: HostInstanceId::new(),
            expected_occupant_generation: None,
            runtime_root: runtime_root.clone(),
            session_dir,
            executable: fs::canonicalize("/bin/sh").unwrap(),
            runtime_detection: None,
            arguments: vec!["-c".into(), "sleep 30".into()],
            environment: BTreeMap::new(),
            cwd: Some(root),
            columns: 80,
            rows: 24,
            journal_limits: JournalLimits::default(),
            stop_deadlines: StopDeadlines::default(),
        })
        .await
        .unwrap();
        let cancel = CancellationToken::new();
        let endpoint = LocalEndpoint::new(&runtime_root, session_id);
        let mut viewer = HostClient::connect(
            endpoint.clone(),
            ConnectOptions::local(session_id, [21; 32]),
            &cancel,
        )
        .await
        .unwrap();
        let mut window = HostClient::connect(
            endpoint,
            ConnectOptions::local_read_only(session_id, [22; 32]),
            &cancel,
        )
        .await
        .unwrap();
        let mut lease = ViewerLease::new();
        let held = viewer.get_state(&cancel).await.unwrap().has_writer_lease;
        lease.settle(held, &mut viewer, &cancel).await.unwrap();
        assert!(lease.held && lease.writable());
        assert!(
            !window
                .set_writer_lease(CommandId::new(), true, &cancel)
                .await
                .unwrap(),
            "the viewer holds the lease while it is being typed in"
        );

        lease.last_used = Instant::now() - VIEWER_LEASE_IDLE;
        lease.settle(true, &mut viewer, &cancel).await.unwrap();
        assert!(!lease.held && lease.writable());
        assert!(
            window
                .set_writer_lease(CommandId::new(), true, &cancel)
                .await
                .unwrap(),
            "the terminal window can type once the viewer has stopped"
        );

        assert!(!lease.take(&mut viewer, &cancel).await.unwrap());
        assert!(!lease.writable(), "typing in the viewer now waits");
        lease.settle(false, &mut viewer, &cancel).await.unwrap();
        assert!(!lease.writable());

        window
            .set_writer_lease(CommandId::new(), false, &cancel)
            .await
            .unwrap();
        lease.settle(false, &mut viewer, &cancel).await.unwrap();
        assert!(!lease.held && lease.writable());
        assert!(
            window
                .set_writer_lease(CommandId::new(), true, &cancel)
                .await
                .unwrap(),
            "looking again does not keep the lease from the window"
        );

        drop(window);
        assert!(lease.take(&mut viewer, &cancel).await.unwrap());
        assert!(
            viewer
                .input(CommandId::new(), b"x".to_vec(), &cancel)
                .await
                .unwrap()
        );
        viewer
            .stop(CommandId::new(), wire::StopMode::Force, &cancel)
            .await
            .unwrap();
        host.wait().await.unwrap();
    }
}
