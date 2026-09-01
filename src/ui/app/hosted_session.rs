use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{RecvTimeoutError, Sender};
use std::sync::{Condvar, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use rand::RngCore as _;
use termirust_client::{
    ClientError, ClientErrorCode, ConnectOptions, GpuiAttachModel, HostClient,
    HostReconciliationService, LocalEndpoint, OutputDisposition,
};
use termirust_domain::{
    CommandId, ContinuityLink, HostInstanceId, HostedSessionId, OccupantGeneration, OutputSequence,
    Revision,
};
use termirust_host_protocol::wire;
use termirust_session_host::{LaunchDescriptor, StopDeadlines};
use termirust_store::{
    ContinuityRepository, HostLease, HostLeaseState, JournalKind, JournalLimits, JournalStore,
    RecoveryResult, load_snapshot, read_host_metadata,
};
use tokio::runtime::Builder;
use tokio::sync::mpsc::{self, UnboundedReceiver};
use tokio_util::sync::CancellationToken;

use crate::ssh::{SessionCommand, SessionRuntimeHandle, SshEvent};

const HOST_READY_DEADLINE: Duration = Duration::from_secs(5);
const CONNECT_RETRY_INTERVAL: Duration = Duration::from_millis(
    termirust_ui_contract::DesignTokens::new(termirust_ui_contract::ThemeKind::System)
        .motion_hosted_connect_poll(false)
        .0 as u64,
);
const LIVE_POLL_INTERVAL: Duration = Duration::from_millis(
    termirust_ui_contract::DesignTokens::new(termirust_ui_contract::ThemeKind::System)
        .motion_hosted_live_poll(false)
        .0 as u64,
);
const MAX_STARTUP_HANDSHAKES: usize = 8;

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
    pub runtime_detection: Option<termirust_domain::RuntimeDetectionResult>,
}

pub(super) fn spawn_durable_session(
    spec: DurableSessionSpec,
    event_tx: Sender<SshEvent>,
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
                    state: termirust_domain::HostedSessionState::Offline,
                    last_sequence: 0,
                    durable_sequence: 0,
                    activity: termirust_domain::ActivityAggregate::default(),
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
            message: format!("Unable to start durable session worker: {error}"),
        });
    }
    SessionRuntimeHandle { command_tx }
}

fn run_durable_session(
    mut spec: DurableSessionSpec,
    mut command_rx: UnboundedReceiver<SessionCommand>,
    event_tx: Sender<SshEvent>,
) -> Result<(), String> {
    let startup_permit = StartupPermit::acquire();
    if let Some(launch) = spec.launch.take() {
        event_tx
            .send(status_event(
                spec.pane_id,
                termirust_domain::HostedSessionState::Provisioning,
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
    event_tx: &Sender<SshEvent>,
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
    let mut process = Command::new(host_executable)
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
                message: "Launch cancelled before the durable Host became ready".to_string(),
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
            termirust_ui_contract::DesignTokens::new(termirust_ui_contract::ThemeKind::System)
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
    if let Some(path) = std::env::var_os("TERMIRUST_SESSION_HOST_BIN") {
        return Ok(PathBuf::from(path));
    }
    let current = std::env::current_exe().map_err(|error| error.to_string())?;
    #[cfg(test)]
    if current
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name == "deps")
        && let Some(debug_dir) = current.parent().and_then(Path::parent)
    {
        let packaged = debug_dir.join(format!("termirust{}", std::env::consts::EXE_SUFFIX));
        if packaged.is_file() {
            return Ok(packaged);
        }
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
    event_tx: Sender<SshEvent>,
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
            Err(_error) if Instant::now() < connect_deadline => {
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
            termirust_domain::HostedSessionState::Attaching,
            model.watermark(),
            false,
            "Attaching to durable Host",
        ))
        .map_err(|_| "Application event channel closed".to_string())?;
    let mut ticker = tokio::time::interval(LIVE_POLL_INTERVAL);
    let mut connected_sent = false;

    loop {
        tokio::select! {
            command = command_rx.recv() => match command {
                Some(SessionCommand::Input(bytes)) => {
                    client.input(CommandId::new(), bytes, &cancel).await
                        .map_err(|error| format!("Durable Host input failed: {error}"))?;
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
                            termirust_domain::HostedSessionState::Exited,
                            "Stopped; retained output is read-only",
                            "Durable session stopped",
                        ),
                        Err(error) => {
                            eprintln!("[durable-session] Host stop failed: {error}");
                            (
                                termirust_domain::HostedSessionState::Orphaned,
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
                        message: "Detached; the durable session continues in the background".to_string(),
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
                        termirust_domain::HostedSessionState::Replaying,
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
                            termirust_domain::HostedSessionState::Offline,
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
                                termirust_domain::HostedSessionState::Gap,
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
                        termirust_domain::HostedSessionState::Exited,
                        model.watermark(),
                        OutputSequence::new(state.durable_sequence),
                        false,
                        activity,
                        "Process exited; retained output is read-only",
                    ));
                    let _ = event_tx.send(SshEvent::Disconnected {
                        session_id: spec.pane_id,
                        message: "Durable process exited; retained output is read-only".to_string(),
                    });
                    return Ok(());
                }
                model.mark_live(state.has_writer_lease, state.recording_paused);
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
                        termirust_domain::HostedSessionState::RecordingPaused
                    } else {
                        termirust_domain::HostedSessionState::Live
                    },
                    model.watermark(),
                    OutputSequence::new(state.durable_sequence),
                    state.has_writer_lease,
                    activity,
                    if state.recording_paused {
                        "Live; output recording paused at the disk limit"
                    } else if state.has_writer_lease {
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
    event_tx: &Sender<SshEvent>,
    error: ClientError,
    sequence: OutputSequence,
) -> Result<(), String> {
    let (state, detail) = recovery_for_client_error(error);
    report_recovery_state(spec, event_tx, state, sequence, detail)
}

fn recovery_for_client_error(
    error: ClientError,
) -> (termirust_domain::HostedSessionState, &'static str) {
    match error.code {
        ClientErrorCode::ProtocolIncompatible => (
            termirust_domain::HostedSessionState::Incompatible,
            "Host protocol is incompatible; update TermiRust before retrying",
        ),
        ClientErrorCode::PermissionDenied
        | ClientErrorCode::WrongSession
        | ClientErrorCode::InvalidIdentity
        | ClientErrorCode::HandshakeReplay => (
            termirust_domain::HostedSessionState::PermissionDenied,
            "Host identity was rejected; retained data was not modified",
        ),
        ClientErrorCode::SequenceGap
        | ClientErrorCode::ConflictingDuplicate
        | ClientErrorCode::ChecksumMismatch
        | ClientErrorCode::MalformedFrame
        | ClientErrorCode::FrameTooLarge => (
            termirust_domain::HostedSessionState::Gap,
            "Output history could not be verified; retained output may be incomplete",
        ),
        ClientErrorCode::Io
        | ClientErrorCode::EndOfStream
        | ClientErrorCode::ResourceLimit
        | ClientErrorCode::Cancelled
        | ClientErrorCode::InvalidState => (
            termirust_domain::HostedSessionState::Offline,
            "Durable Host is offline; retry to reconcile and reconnect",
        ),
    }
}

fn report_recovery_state(
    spec: &DurableSessionSpec,
    event_tx: &Sender<SshEvent>,
    state: termirust_domain::HostedSessionState,
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
    event_tx: &Sender<SshEvent>,
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
        || prior_lifecycle == termirust_domain::HostLifecycle::Orphaned
    {
        termirust_domain::HostedSessionState::Orphaned
    } else {
        termirust_domain::HostedSessionState::Exited
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
            if state == termirust_domain::HostedSessionState::Orphaned {
                "Host is gone; retained output is read-only and cannot be stopped"
            } else {
                "Process exited; retained output is read-only"
            },
        ))
        .map_err(|_| "Application event channel closed".to_string())?;
    let _ = event_tx.send(SshEvent::Disconnected {
        session_id: spec.pane_id,
        message: "Retained durable output is read-only".to_string(),
    });
    Ok(true)
}

fn status_event(
    pane_id: u64,
    state: termirust_domain::HostedSessionState,
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
        termirust_domain::ActivityAggregate::default(),
        detail,
    )
}

fn status_event_with_durability(
    pane_id: u64,
    state: termirust_domain::HostedSessionState,
    sequence: OutputSequence,
    durable_sequence: OutputSequence,
    has_writer_lease: bool,
    activity: termirust_domain::ActivityAggregate,
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
    fn status_events_keep_pane_and_durable_sequence_separate() {
        let event = status_event(
            42,
            termirust_domain::HostedSessionState::Live,
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
            termirust_domain::HostedSessionState::Incompatible
        );
        assert_eq!(
            recovery_for_client_error(ClientError::new(ClientErrorCode::PermissionDenied)).0,
            termirust_domain::HostedSessionState::PermissionDenied
        );
        assert_eq!(
            recovery_for_client_error(ClientError::new(ClientErrorCode::SequenceGap)).0,
            termirust_domain::HostedSessionState::Gap
        );
        assert_eq!(
            recovery_for_client_error(ClientError::new(ClientErrorCode::EndOfStream)).0,
            termirust_domain::HostedSessionState::Offline
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
}
