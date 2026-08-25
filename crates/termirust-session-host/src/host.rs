use std::collections::{HashMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::fd::AsRawFd as _;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt as _;
#[cfg(unix)]
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _, PermissionsExt as _};

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use termirust_domain::{
    CommandId, DurabilityWatermark, HostLifecycle, OccupantGeneration, OccupantOwnership,
    OutputSequence, ProcessToken, RecognitionConfidence, RuntimeCapabilitySet, RuntimeOccupant,
    RuntimeRecognition,
};
use termirust_host_protocol::wire::{self, envelope_payload};
use termirust_host_protocol::{
    CURRENT_PROTOCOL, CapabilitySet, HANDSHAKE_NONCE_BYTES, MAX_IDEMPOTENCY_OUTCOMES,
    MAX_OUTPUT_BYTES, MAX_REPLAY_BYTES, MAX_REPLAY_RECORDS, NegotiatedLimits, PreservedPayload,
    ProtocolRange, ProtocolVersion, WireEnvelope, decode_command_id, decode_session_id,
    encode_command_id, encode_host_instance_id, encode_payload, encode_session_id, local_limits,
    negotiate_protocol, opaque_endpoint_name, payload_kind,
};
use termirust_store::{
    AppendOutcome, HostLease, HostMetadata, JournalFrame, JournalKind, JournalStore,
    TerminalSnapshot, load_snapshot,
};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Mutex, Notify, RwLock, Semaphore};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use crate::descriptor::LaunchDescriptor;
use crate::framing::HostWireStream;
use crate::process_observation::fingerprint_executable;
use crate::{HostError, HostErrorCode};

const MAX_CONNECTIONS: usize = 32;
pub const MAX_LIVE_HOSTS: usize = 32;
const PTY_CHANNEL_FRAMES: usize = 64;
const TASK_JOIN_DEADLINE: Duration = Duration::from_secs(2);
const EXIT_DRAIN_DEADLINE: Duration = Duration::from_millis(500);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);
const IDEMPOTENCY_TTL: Duration = Duration::from_secs(10 * 60);
const HOST_SLOT_PREFIX: &str = "host-slot-";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionHostStats {
    pub lifecycle: HostLifecycle,
    pub latest_sequence: OutputSequence,
    pub active_connections: usize,
    pub recording_paused: bool,
}

pub struct SessionHostHandle {
    runtime_root: PathBuf,
    session_id: termirust_domain::HostedSessionId,
    cancel: CancellationToken,
    task: Option<JoinHandle<Result<(), HostError>>>,
    state: Option<Arc<RuntimeState>>,
}

impl std::fmt::Debug for SessionHostHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SessionHostHandle")
            .field("runtime_root", &"[REDACTED]")
            .field("session_id", &self.session_id)
            .finish_non_exhaustive()
    }
}

impl SessionHostHandle {
    pub fn runtime_root(&self) -> &Path {
        &self.runtime_root
    }

    pub async fn stats(&self) -> SessionHostStats {
        match &self.state {
            Some(state) => state.stats().await,
            None => SessionHostStats {
                lifecycle: HostLifecycle::Exited,
                latest_sequence: OutputSequence::ZERO,
                active_connections: 0,
                recording_paused: false,
            },
        }
    }

    pub async fn wait(mut self) -> Result<(), HostError> {
        let task = self
            .task
            .take()
            .ok_or_else(|| HostError::new(HostErrorCode::JoinFailed))?;
        self.state.take();
        task.await
            .map_err(|_| HostError::new(HostErrorCode::JoinFailed))?
    }

    pub async fn shutdown(mut self) -> Result<(), HostError> {
        self.cancel.cancel();
        let task = self
            .task
            .take()
            .ok_or_else(|| HostError::new(HostErrorCode::JoinFailed))?;
        self.state.take();
        timeout(TASK_JOIN_DEADLINE + Duration::from_secs(5), task)
            .await
            .map_err(|_| HostError::new(HostErrorCode::JoinFailed).at_stage("host_task_timeout"))?
            .map_err(|_| HostError::new(HostErrorCode::JoinFailed).at_stage("host_task_panic"))?
    }
}

impl Drop for SessionHostHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

struct RuntimeState {
    descriptor: LaunchDescriptor,
    process_token: ProcessToken,
    runtime_recognition: Option<RuntimeRecognition>,
    process_group: i32,
    master: StdMutex<Box<dyn MasterPty + Send>>,
    writer: StdMutex<Box<dyn Write + Send>>,
    journal: Mutex<JournalStore>,
    parser: Mutex<vt100::Parser>,
    lifecycle: RwLock<HostLifecycle>,
    latest_sequence: AtomicU64,
    durable_sequence: AtomicU64,
    recording_paused: AtomicBool,
    exited: AtomicBool,
    exit_code: StdMutex<Option<i32>>,
    exit_notify: Notify,
    writer_lease: Mutex<Option<u64>>,
    active_connections: AtomicU64,
    next_connection: AtomicU64,
    idempotency: Mutex<MutationCache>,
    handshake_nonces: Mutex<NonceCache>,
}

impl RuntimeState {
    async fn stats(&self) -> SessionHostStats {
        SessionHostStats {
            lifecycle: *self.lifecycle.read().await,
            latest_sequence: OutputSequence::new(self.latest_sequence.load(Ordering::Acquire)),
            active_connections: usize::try_from(self.active_connections.load(Ordering::Acquire))
                .unwrap_or(usize::MAX),
            recording_paused: self.recording_paused.load(Ordering::Acquire),
        }
    }

    async fn claim_writer(&self, connection_id: u64) -> bool {
        let mut writer = self.writer_lease.lock().await;
        if writer.is_none() {
            *writer = Some(connection_id);
        }
        *writer == Some(connection_id)
    }

    async fn release_writer(&self, connection_id: u64) {
        let mut writer = self.writer_lease.lock().await;
        if *writer == Some(connection_id) {
            *writer = None;
        }
    }

    async fn has_writer(&self, connection_id: u64) -> bool {
        *self.writer_lease.lock().await == Some(connection_id)
    }

    fn signal_owned(&self, signal: i32) -> Result<(), HostError> {
        if !self
            .process_token
            .belongs_to(self.descriptor.host_instance_id)
            || self.process_token.platform_identity() != self.process_group as u64
        {
            return Err(HostError::new(HostErrorCode::ProcessIdentityUnavailable));
        }
        #[cfg(unix)]
        {
            let result = unsafe { libc::kill(-self.process_group, signal) };
            if result == 0 {
                Ok(())
            } else {
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::ESRCH) && self.exited.load(Ordering::Acquire)
                {
                    Ok(())
                } else {
                    Err(HostError::io(error))
                }
            }
        }
        #[cfg(not(unix))]
        {
            let _ = signal;
            Err(HostError::new(HostErrorCode::ProcessIdentityUnavailable))
        }
    }

    fn signal_owned_leader(&self, signal: i32) -> Result<(), HostError> {
        if !self
            .process_token
            .belongs_to(self.descriptor.host_instance_id)
            || self.process_token.platform_identity() != self.process_group as u64
        {
            return Err(HostError::new(HostErrorCode::ProcessIdentityUnavailable));
        }
        let result = unsafe { libc::kill(self.process_group, signal) };
        if result == 0 {
            Ok(())
        } else {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) && self.exited.load(Ordering::Acquire) {
                Ok(())
            } else {
                Err(HostError::io(error))
            }
        }
    }

    fn force_kill_owned(&self) -> Result<(), HostError> {
        self.signal_owned(libc::SIGKILL)?;
        self.signal_owned_leader(libc::SIGKILL)
    }

    async fn stop_owned(&self, force: bool) -> Result<(), HostError> {
        if self.exited.load(Ordering::Acquire) {
            return Ok(());
        }
        *self.lifecycle.write().await = HostLifecycle::Stopping;
        if force {
            self.force_kill_owned()?;
            return self
                .wait_for_exit(self.descriptor.stop_deadlines.total())
                .await;
        }
        self.signal_owned(libc::SIGINT)?;
        if self
            .wait_for_exit(self.descriptor.stop_deadlines.interrupt())
            .await
            .is_ok()
        {
            return Ok(());
        }
        self.signal_owned(libc::SIGTERM)?;
        let terminate_window = self
            .descriptor
            .stop_deadlines
            .terminate()
            .saturating_sub(self.descriptor.stop_deadlines.interrupt());
        if self.wait_for_exit(terminate_window).await.is_ok() {
            return Ok(());
        }
        self.force_kill_owned()?;
        let force_window = self
            .descriptor
            .stop_deadlines
            .total()
            .saturating_sub(self.descriptor.stop_deadlines.terminate());
        self.wait_for_exit(force_window).await
    }

    async fn wait_for_exit(&self, duration: Duration) -> Result<(), HostError> {
        if self.exited.load(Ordering::Acquire) {
            return Ok(());
        }
        timeout(
            duration.max(Duration::from_millis(1)),
            self.exit_notify.notified(),
        )
        .await
        .map_err(|_| HostError::new(HostErrorCode::Cancelled))?;
        Ok(())
    }
}

pub async fn start(descriptor: LaunchDescriptor) -> Result<SessionHostHandle, HostError> {
    start_with_cancel(descriptor, &CancellationToken::new()).await
}

pub async fn start_with_cancel(
    descriptor: LaunchDescriptor,
    launch_cancel: &CancellationToken,
) -> Result<SessionHostHandle, HostError> {
    if launch_cancel.is_cancelled() {
        return Err(HostError::new(HostErrorCode::Cancelled));
    }
    descriptor.validate()?;
    let host_permit = host_permits()
        .try_acquire_owned()
        .map_err(|_| HostError::new(HostErrorCode::ResourceLimit))?;
    let lease = Arc::new(HostLease::acquire(
        &descriptor.session_dir,
        descriptor.host_instance_id,
    )?);
    let listener = UserOnlyListener::bind(&descriptor.runtime_root, descriptor.session_id)?;
    let runtime_host_slot = RuntimeHostSlot::acquire(&descriptor.runtime_root)?;
    let host_capacity = HostCapacityGuards {
        _process: host_permit,
        _runtime: runtime_host_slot,
    };
    let journal = JournalStore::open(&lease, descriptor.journal_limits)?;
    let latest_sequence = journal.latest_sequence();
    let recording_paused = journal.recording_paused();

    if launch_cancel.is_cancelled() {
        return Err(HostError::new(HostErrorCode::Cancelled));
    }

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: descriptor.rows,
            cols: descriptor.columns,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|_| HostError::new(HostErrorCode::PtyUnavailable))?;
    let mut command = CommandBuilder::new(&descriptor.executable);
    command.args(&descriptor.arguments);
    command.env_clear();
    for (name, value) in &descriptor.environment {
        command.env(name, value);
    }
    command.env("TERM", "xterm-256color");
    if let Some(cwd) = &descriptor.cwd {
        command.cwd(cwd);
    }
    let mut child = pair
        .slave
        .spawn_command(command)
        .map_err(|_| HostError::new(HostErrorCode::ExecFailed))?;
    drop(pair.slave);
    if launch_cancel.is_cancelled() {
        let _ = child.kill();
        let _ = child.wait();
        return Err(HostError::new(HostErrorCode::Cancelled));
    }
    let process_id = child
        .process_id()
        .ok_or_else(|| HostError::new(HostErrorCode::ProcessIdentityUnavailable))?;
    let process_group = pair
        .master
        .process_group_leader()
        .ok_or_else(|| HostError::new(HostErrorCode::ProcessIdentityUnavailable))?;
    if process_group <= 0 || u32::try_from(process_group).ok() != Some(process_id) {
        let _ = child.kill();
        return Err(HostError::new(HostErrorCode::ProcessIdentityUnavailable));
    }
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|_| HostError::new(HostErrorCode::PtyUnavailable))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|_| HostError::new(HostErrorCode::PtyUnavailable))?;
    let process_token = ProcessToken::new(descriptor.host_instance_id, process_group as u64, 1);
    let runtime_recognition = recognition_for_launch(&descriptor, process_token);
    let state = Arc::new(RuntimeState {
        descriptor: descriptor.clone(),
        process_token,
        runtime_recognition,
        process_group,
        master: StdMutex::new(pair.master),
        writer: StdMutex::new(writer),
        journal: Mutex::new(journal),
        parser: Mutex::new(vt100::Parser::new(
            descriptor.rows,
            descriptor.columns,
            10_000,
        )),
        lifecycle: RwLock::new(HostLifecycle::Ready),
        latest_sequence: AtomicU64::new(latest_sequence.get()),
        durable_sequence: AtomicU64::new(latest_sequence.get()),
        recording_paused: AtomicBool::new(recording_paused),
        exited: AtomicBool::new(false),
        exit_code: StdMutex::new(None),
        exit_notify: Notify::new(),
        writer_lease: Mutex::new(None),
        active_connections: AtomicU64::new(0),
        next_connection: AtomicU64::new(1),
        idempotency: Mutex::new(MutationCache::default()),
        handshake_nonces: Mutex::new(NonceCache::default()),
    });
    write_metadata(&lease, &state, HostLifecycle::Ready, monotonic_nanos())?;

    let cancel = CancellationToken::new();
    let task = tokio::spawn(run_host(
        listener,
        lease,
        reader,
        child,
        state.clone(),
        cancel.clone(),
        host_capacity,
    ));
    Ok(SessionHostHandle {
        runtime_root: descriptor.runtime_root,
        session_id: descriptor.session_id,
        cancel,
        task: Some(task),
        state: Some(state),
    })
}

async fn run_host(
    listener: UserOnlyListener,
    lease: Arc<HostLease>,
    mut reader: Box<dyn Read + Send>,
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
    state: Arc<RuntimeState>,
    cancel: CancellationToken,
    _host_capacity: HostCapacityGuards,
) -> Result<(), HostError> {
    let child_cancel = cancel.child_token();
    let (output_tx, output_rx) = tokio::sync::mpsc::channel(PTY_CHANNEL_FRAMES);
    let reader_task = tokio::task::spawn_blocking(move || -> Result<(), HostError> {
        let mut bytes = vec![0_u8; MAX_OUTPUT_BYTES];
        loop {
            let count = reader.read(&mut bytes).map_err(HostError::io)?;
            if count == 0 {
                return Ok(());
            }
            if output_tx.blocking_send(bytes[..count].to_vec()).is_err() {
                return Ok(());
            }
        }
    });
    let output_task = tokio::spawn(output_loop(state.clone(), output_rx, child_cancel.clone()));
    let server_task = tokio::spawn(accept_loop(listener, state.clone(), child_cancel.clone()));
    let heartbeat_task = tokio::spawn(heartbeat_loop(
        lease.clone(),
        state.clone(),
        child_cancel.clone(),
    ));
    let natural_status = loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break None,
            _ = tokio::time::sleep(Duration::from_millis(10)) => {
                if let Some(status) = child.try_wait().map_err(HostError::io)? {
                    break Some(status);
                }
            }
        }
    };
    if let Some(status) = natural_status {
        mark_exited(
            &state,
            i32::try_from(status.exit_code()).unwrap_or(i32::MAX),
        )
        .await;
        tokio::time::sleep(EXIT_DRAIN_DEADLINE).await;
    } else {
        state.force_kill_owned()?;
        if child.try_wait().map_err(HostError::io)?.is_none() {
            child.kill().map_err(HostError::io)?;
        }
        let deadline = Instant::now() + TASK_JOIN_DEADLINE;
        let result = loop {
            if let Some(status) = child.try_wait().map_err(HostError::io)? {
                break status;
            }
            if Instant::now() >= deadline {
                let leader_alive = unsafe { libc::kill(state.process_group, 0) } == 0;
                let stage = if leader_alive {
                    "child_wait_timeout_leader_alive"
                } else {
                    "child_wait_timeout_unreaped"
                };
                return Err(HostError::new(HostErrorCode::JoinFailed).at_stage(stage));
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        mark_exited(
            &state,
            i32::try_from(result.exit_code()).unwrap_or(i32::MAX),
        )
        .await;
    }
    write_metadata(&lease, &state, HostLifecycle::Exited, monotonic_nanos())?;
    child_cancel.cancel();
    join_task(server_task).await?;
    join_task(output_task).await?;
    join_task(heartbeat_task).await?;
    drop(state);
    timeout(TASK_JOIN_DEADLINE, reader_task)
        .await
        .map_err(|_| HostError::new(HostErrorCode::JoinFailed).at_stage("reader_timeout"))?
        .map_err(|_| HostError::new(HostErrorCode::JoinFailed).at_stage("reader_panic"))??;
    Ok(())
}

fn host_permits() -> Arc<Semaphore> {
    static PERMITS: std::sync::OnceLock<Arc<Semaphore>> = std::sync::OnceLock::new();
    PERMITS
        .get_or_init(|| Arc::new(Semaphore::new(MAX_LIVE_HOSTS)))
        .clone()
}

struct HostCapacityGuards {
    _process: tokio::sync::OwnedSemaphorePermit,
    _runtime: RuntimeHostSlot,
}

struct RuntimeHostSlot {
    file: File,
}

impl RuntimeHostSlot {
    fn acquire(runtime_root: &Path) -> Result<Self, HostError> {
        for slot in 0..MAX_LIVE_HOSTS {
            let path = runtime_root.join(format!("{HOST_SLOT_PREFIX}{slot:02}.lock"));
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .mode(0o600)
                .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
                .open(path)
                .map_err(HostError::io)?;
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(HostError::io)?;
            let metadata = file.metadata().map_err(HostError::io)?;
            if !metadata.is_file()
                || metadata.uid() != unsafe { libc::geteuid() }
                || metadata.permissions().mode() & 0o777 != 0o600
            {
                return Err(HostError::new(HostErrorCode::PermissionDenied));
            }
            let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if result == 0 {
                return Ok(Self { file });
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::WouldBlock {
                return Err(HostError::io(error));
            }
        }
        Err(HostError::new(HostErrorCode::ResourceLimit))
    }
}

impl Drop for RuntimeHostSlot {
    fn drop(&mut self) {
        let _: i32 = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
    }
}

async fn join_task(task: JoinHandle<Result<(), HostError>>) -> Result<(), HostError> {
    timeout(TASK_JOIN_DEADLINE, task)
        .await
        .map_err(|_| HostError::new(HostErrorCode::JoinFailed).at_stage("background_task_timeout"))?
        .map_err(|_| HostError::new(HostErrorCode::JoinFailed).at_stage("background_task_panic"))?
}

async fn output_loop(
    state: Arc<RuntimeState>,
    mut output_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
    cancel: CancellationToken,
) -> Result<(), HostError> {
    let mut last_sync = Instant::now();
    loop {
        let bytes = tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            output = output_rx.recv() => match output {
                Some(bytes) => bytes,
                None => break,
            },
        };
        state.parser.lock().await.process(&bytes);
        let sequence = state
            .latest_sequence
            .fetch_add(1, Ordering::AcqRel)
            .checked_add(1)
            .map(OutputSequence::new)
            .ok_or_else(|| HostError::new(HostErrorCode::ResourceLimit))?;
        if !state.recording_paused.load(Ordering::Acquire) {
            let frame = JournalFrame {
                kind: JournalKind::Output,
                sequence,
                monotonic_nanos: monotonic_nanos(),
                flags: 0,
                payload: bytes,
            };
            let mut journal = state.journal.lock().await;
            if journal.append(&frame)? == AppendOutcome::RecordingPausedDiskLimit {
                state.recording_paused.store(true, Ordering::Release);
            } else if journal.compaction_due() {
                let parser = state.parser.lock().await;
                let (rows, columns) = parser.screen().size();
                let snapshot = TerminalSnapshot {
                    boundary: sequence,
                    columns: u32::from(columns),
                    rows: u32::from(rows),
                    terminal_bytes: parser.screen().contents_formatted(),
                };
                journal.compact(&snapshot)?;
            } else if last_sync.elapsed() >= HEARTBEAT_INTERVAL {
                journal.sync()?;
                last_sync = Instant::now();
            }
        }
    }
    Ok(())
}

async fn heartbeat_loop(
    lease: Arc<HostLease>,
    state: Arc<RuntimeState>,
    cancel: CancellationToken,
) -> Result<(), HostError> {
    let endpoint_name = opaque_endpoint_name(state.descriptor.session_id);
    let mut interval = tokio::time::interval(HEARTBEAT_INTERVAL);
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            _ = interval.tick() => {
                let latest = state.journal.lock().await.sync()?;
                state
                    .durable_sequence
                    .store(latest.get(), Ordering::Release);
                let metadata = HostMetadata {
                    format_version: HostMetadata::FORMAT_VERSION,
                    session_id: state.descriptor.session_id,
                    host_instance_id: state.descriptor.host_instance_id,
                    process_token: Some(state.process_token),
                    runtime_recognition: state.runtime_recognition.clone(),
                    lifecycle: *state.lifecycle.read().await,
                    endpoint_name: endpoint_name.clone(),
                    heartbeat_monotonic_nanos: monotonic_nanos(),
                    durability_watermark: Some(DurabilityWatermark {
                        sequence: latest,
                        monotonic_nanos: monotonic_nanos(),
                    }),
                };
                lease.write_metadata(&metadata)?;
            }
        }
    }
    Ok(())
}

async fn mark_exited(state: &RuntimeState, exit_code: i32) {
    if let Ok(mut code) = state.exit_code.lock() {
        *code = Some(exit_code);
    }
    state.exited.store(true, Ordering::Release);
    *state.lifecycle.write().await = HostLifecycle::Exited;
    state.exit_notify.notify_waiters();
}

fn write_metadata(
    lease: &HostLease,
    state: &RuntimeState,
    lifecycle: HostLifecycle,
    heartbeat: u64,
) -> Result<(), HostError> {
    let metadata = HostMetadata {
        format_version: HostMetadata::FORMAT_VERSION,
        session_id: state.descriptor.session_id,
        host_instance_id: state.descriptor.host_instance_id,
        process_token: Some(state.process_token),
        runtime_recognition: state.runtime_recognition.clone(),
        lifecycle,
        endpoint_name: opaque_endpoint_name(state.descriptor.session_id),
        heartbeat_monotonic_nanos: heartbeat,
        durability_watermark: Some(DurabilityWatermark {
            sequence: OutputSequence::new(state.latest_sequence.load(Ordering::Acquire)),
            monotonic_nanos: heartbeat,
        }),
    };
    lease.write_metadata(&metadata)?;
    Ok(())
}

fn recognition_for_launch(
    descriptor: &LaunchDescriptor,
    process_token: ProcessToken,
) -> Option<RuntimeRecognition> {
    let detection = descriptor.runtime_detection.as_ref()?;
    let fingerprint_matches = fingerprint_executable(&descriptor.executable)
        .ok()
        .is_some_and(|fingerprint| Some(fingerprint) == detection.fingerprint);
    let (ownership, confidence, capabilities) = if fingerprint_matches {
        (
            OccupantOwnership::Managed {
                host_instance: descriptor.host_instance_id,
                child_token: process_token,
            },
            RecognitionConfidence::Verified,
            detection.capabilities.clone(),
        )
    } else {
        (
            OccupantOwnership::Ambiguous,
            RecognitionConfidence::Uncertain,
            RuntimeCapabilitySet::default(),
        )
    };
    Some(RuntimeRecognition {
        occupant: Some(RuntimeOccupant {
            runtime_id: detection.runtime_id.clone(),
            descriptor_version: detection.descriptor_version,
            safe_version: detection.safe_version.clone(),
            generation: OccupantGeneration::new(process_token.generation()),
            ownership,
            capabilities,
            stale: false,
        }),
        confidence,
        observed_at_nanos: monotonic_nanos(),
    })
}

struct UserOnlyListener {
    listener: UnixListener,
    socket_path: PathBuf,
    socket_device: u64,
    socket_inode: u64,
    expected_uid: u32,
}

impl UserOnlyListener {
    fn bind(
        runtime_root: &Path,
        session_id: termirust_domain::HostedSessionId,
    ) -> Result<Self, HostError> {
        prepare_runtime_root(runtime_root)?;
        let endpoint_name = opaque_endpoint_name(session_id);
        let socket_path = runtime_root.join(&endpoint_name);
        match fs::symlink_metadata(&socket_path) {
            Ok(_) => return Err(HostError::new(HostErrorCode::PermissionDenied)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(HostError::io(error)),
        }
        let listener = UnixListener::bind(&socket_path).map_err(HostError::io)?;
        #[cfg(unix)]
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))
            .map_err(HostError::io)?;
        let metadata = fs::symlink_metadata(&socket_path).map_err(HostError::io)?;
        if !metadata.file_type().is_socket()
            || metadata.permissions().mode() & 0o777 != 0o600
            || metadata.uid() != unsafe { libc::geteuid() }
        {
            return Err(HostError::new(HostErrorCode::PermissionDenied));
        }
        Ok(Self {
            listener,
            socket_path,
            socket_device: metadata.dev(),
            socket_inode: metadata.ino(),
            expected_uid: unsafe { libc::geteuid() },
        })
    }

    async fn accept(&self) -> Result<UnixStream, HostError> {
        let (stream, _) = self.listener.accept().await.map_err(HostError::io)?;
        let credentials = stream.peer_cred().map_err(HostError::io)?;
        if credentials.uid() != self.expected_uid {
            return Err(HostError::new(HostErrorCode::PermissionDenied));
        }
        Ok(stream)
    }
}

impl Drop for UserOnlyListener {
    fn drop(&mut self) {
        if let Ok(metadata) = fs::symlink_metadata(&self.socket_path)
            && metadata.file_type().is_socket()
            && metadata.dev() == self.socket_device
            && metadata.ino() == self.socket_inode
        {
            let _ = fs::remove_file(&self.socket_path);
        }
    }
}

fn prepare_runtime_root(root: &Path) -> Result<(), HostError> {
    match fs::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(HostError::new(HostErrorCode::PermissionDenied));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(root).map_err(HostError::io)?;
        }
        Err(error) => return Err(HostError::io(error)),
    }
    fs::set_permissions(root, fs::Permissions::from_mode(0o700)).map_err(HostError::io)?;
    let metadata = fs::symlink_metadata(root).map_err(HostError::io)?;
    if metadata.uid() != unsafe { libc::geteuid() }
        || metadata.permissions().mode() & 0o777 != 0o700
    {
        return Err(HostError::new(HostErrorCode::PermissionDenied));
    }
    Ok(())
}

async fn accept_loop(
    listener: UserOnlyListener,
    state: Arc<RuntimeState>,
    cancel: CancellationToken,
) -> Result<(), HostError> {
    let permits = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    let mut handlers = JoinSet::new();
    loop {
        while let Some(result) = handlers.try_join_next() {
            result.map_err(|_| HostError::new(HostErrorCode::JoinFailed))??;
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            accepted = listener.accept() => {
                let stream = accepted?;
                let Ok(permit) = permits.clone().try_acquire_owned() else {
                    drop(stream);
                    continue;
                };
                let state = state.clone();
                let child_cancel = cancel.child_token();
                handlers.spawn(async move {
                    let connection_id = state.next_connection.fetch_add(1, Ordering::Relaxed);
                    state.active_connections.fetch_add(1, Ordering::AcqRel);
                    state.claim_writer(connection_id).await;
                    let result = serve_connection(stream, state.clone(), connection_id, child_cancel).await;
                    state.release_writer(connection_id).await;
                    state.active_connections.fetch_sub(1, Ordering::AcqRel);
                    drop(permit);
                    match result {
                        Err(error) if matches!(error.code, HostErrorCode::Cancelled | HostErrorCode::Protocol | HostErrorCode::ResourceLimit) => Ok(()),
                        other => other,
                    }
                });
            }
        }
    }
    cancel.cancel();
    while let Some(result) = handlers.join_next().await {
        result.map_err(|_| HostError::new(HostErrorCode::JoinFailed))??;
    }
    Ok(())
}

async fn serve_connection(
    stream: UnixStream,
    state: Arc<RuntimeState>,
    connection_id: u64,
    cancel: CancellationToken,
) -> Result<(), HostError> {
    let mut stream = HostWireStream::new(stream);
    let envelope = stream.read(&cancel).await?;
    let request_id = envelope.request_id;
    let payload = decode_checked(&envelope)?;
    let handshake = match payload.message {
        Some(envelope_payload::Message::HandshakeRequest(request)) => request,
        _ => {
            send_error(
                &mut stream,
                request_id,
                wire::ErrorCode::InvalidState,
                wire::RecoveryHint::Reconnect,
                0,
                &cancel,
            )
            .await?;
            return Ok(());
        }
    };
    if decode_session_id(&handshake.session_id)
        .map_err(|_| HostError::new(HostErrorCode::Protocol))?
        != state.descriptor.session_id
    {
        send_error(
            &mut stream,
            request_id,
            wire::ErrorCode::WrongSession,
            wire::RecoveryHint::Reauthorize,
            0,
            &cancel,
        )
        .await?;
        return Ok(());
    }
    let client_nonce: [u8; HANDSHAKE_NONCE_BYTES] = handshake
        .client_nonce
        .as_slice()
        .try_into()
        .map_err(|_| HostError::new(HostErrorCode::Protocol))?;
    if !state
        .handshake_nonces
        .lock()
        .await
        .accept(client_nonce, Instant::now())
    {
        send_error(
            &mut stream,
            request_id,
            wire::ErrorCode::HandshakeReplay,
            wire::RecoveryHint::Reconnect,
            0,
            &cancel,
        )
        .await?;
        return Ok(());
    }
    let peer_protocol = ProtocolRange::try_from(
        handshake
            .protocol
            .as_ref()
            .ok_or_else(|| HostError::new(HostErrorCode::Protocol))?,
    )
    .map_err(|_| HostError::new(HostErrorCode::Protocol))?;
    let Some(selected) = negotiate_protocol(CURRENT_PROTOCOL, peer_protocol) else {
        send_error(
            &mut stream,
            request_id,
            wire::ErrorCode::ProtocolIncompatible,
            wire::RecoveryHint::Upgrade,
            0,
            &cancel,
        )
        .await?;
        return Ok(());
    };
    let peer_limits = NegotiatedLimits::try_from(
        handshake
            .limits
            .as_ref()
            .ok_or_else(|| HostError::new(HostErrorCode::Protocol))?,
    )
    .map_err(|_| HostError::new(HostErrorCode::Protocol))?;
    let limits = local_limits().bounded_with(peer_limits);
    let capabilities =
        CapabilitySet::all_local().intersection(&CapabilitySet::from_wire(&handshake.capabilities));
    let host_nonce = state
        .descriptor
        .host_instance_id
        .as_uuid()
        .as_bytes()
        .repeat(2);
    send_message(
        &mut stream,
        request_id,
        selected,
        envelope_payload::Message::HandshakeResponse(wire::HandshakeResponse {
            host_instance_id: encode_host_instance_id(state.descriptor.host_instance_id),
            session_id: encode_session_id(state.descriptor.session_id),
            selected_version: Some(selected.into()),
            capabilities: capabilities.to_wire(),
            limits: Some(limits.into()),
            host_nonce,
            client_nonce_echo: handshake.client_nonce,
        }),
        &cancel,
    )
    .await?;

    loop {
        let envelope = stream.read(&cancel).await?;
        if envelope.protocol_major != selected.major || envelope.protocol_minor != selected.minor {
            return Err(HostError::new(HostErrorCode::Protocol));
        }
        let payload = decode_checked(&envelope)?;
        if required_capability(&payload)
            .is_some_and(|capability| !capabilities.contains(capability))
        {
            send_error(
                &mut stream,
                envelope.request_id,
                wire::ErrorCode::InvalidState,
                wire::RecoveryHint::Reauthorize,
                0,
                &cancel,
            )
            .await?;
            return Ok(());
        }
        match payload.message {
            Some(envelope_payload::Message::GetStateRequest(request)) => {
                require_session(&request.session_id, &state)?;
                send_state(
                    &mut stream,
                    envelope.request_id,
                    selected,
                    &state,
                    connection_id,
                    &cancel,
                )
                .await?;
            }
            Some(envelope_payload::Message::AttachRequest(request)) => {
                require_session(&request.session_id, &state)?;
                serve_attach(
                    &mut stream,
                    envelope.request_id,
                    selected,
                    &state,
                    connection_id,
                    request,
                    &cancel,
                )
                .await?;
            }
            Some(envelope_payload::Message::InputRequest(request)) => {
                require_session(&request.session_id, &state)?;
                if !state.has_writer(connection_id).await {
                    send_error(
                        &mut stream,
                        envelope.request_id,
                        wire::ErrorCode::PermissionDenied,
                        wire::RecoveryHint::Reauthorize,
                        0,
                        &cancel,
                    )
                    .await?;
                    return Ok(());
                }
                if request.bytes.len() > limits.maximum_output_bytes {
                    return Err(HostError::new(HostErrorCode::ResourceLimit));
                }
                let bytes = request.bytes;
                handle_mutation(
                    &mut stream,
                    envelope.request_id,
                    selected,
                    &state,
                    MutationRequest {
                        command_bytes: &request.command_id,
                        payload: &envelope.payload,
                        apply: || {
                            state
                                .writer
                                .lock()
                                .map_err(|_| HostError::new(HostErrorCode::Io))?
                                .write_all(&bytes)
                                .map_err(HostError::io)
                        },
                    },
                    &cancel,
                )
                .await?;
            }
            Some(envelope_payload::Message::ResizeRequest(request)) => {
                require_session(&request.session_id, &state)?;
                let viewport = request
                    .viewport
                    .ok_or_else(|| HostError::new(HostErrorCode::Protocol))?;
                let rows = u16::try_from(viewport.rows)
                    .map_err(|_| HostError::new(HostErrorCode::ResourceLimit))?;
                let cols = u16::try_from(viewport.columns)
                    .map_err(|_| HostError::new(HostErrorCode::ResourceLimit))?;
                if rows == 0 || cols == 0 {
                    return Err(HostError::new(HostErrorCode::Protocol));
                }
                handle_mutation(
                    &mut stream,
                    envelope.request_id,
                    selected,
                    &state,
                    MutationRequest {
                        command_bytes: &request.command_id,
                        payload: &envelope.payload,
                        apply: || {
                            state
                                .master
                                .lock()
                                .map_err(|_| HostError::new(HostErrorCode::Io))?
                                .resize(PtySize {
                                    rows,
                                    cols,
                                    pixel_width: 0,
                                    pixel_height: 0,
                                })
                                .map_err(|_| HostError::new(HostErrorCode::PtyUnavailable))?;
                            Ok(())
                        },
                    },
                    &cancel,
                )
                .await?;
                state.parser.lock().await.screen_mut().set_size(rows, cols);
            }
            Some(envelope_payload::Message::InterruptRequest(request)) => {
                require_session(&request.session_id, &state)?;
                handle_mutation(
                    &mut stream,
                    envelope.request_id,
                    selected,
                    &state,
                    MutationRequest {
                        command_bytes: &request.command_id,
                        payload: &envelope.payload,
                        apply: || state.signal_owned(libc::SIGINT),
                    },
                    &cancel,
                )
                .await?;
            }
            Some(envelope_payload::Message::StopRequest(request)) => {
                require_session(&request.session_id, &state)?;
                let command_id = decode_command_id(&request.command_id)
                    .map_err(|_| HostError::new(HostErrorCode::Protocol))?;
                validate_command_request(command_id, envelope.request_id)?;
                let payload_hash = crc32c::crc32c(&envelope.payload);
                let force = wire::StopMode::try_from(request.mode)
                    .unwrap_or(wire::StopMode::Unspecified)
                    == wire::StopMode::Force;
                let mut cache = state.idempotency.lock().await;
                match cache.inspect(command_id, payload_hash, Instant::now()) {
                    MutationDecision::Apply => {
                        state.stop_owned(force).await?;
                        cache.record(command_id, payload_hash, Instant::now());
                    }
                    MutationDecision::Replay => {}
                    MutationDecision::Conflict => {
                        send_error(
                            &mut stream,
                            envelope.request_id,
                            wire::ErrorCode::ConflictingDuplicate,
                            wire::RecoveryHint::RetryExplicitly,
                            0,
                            &cancel,
                        )
                        .await?;
                        return Ok(());
                    }
                }
                send_command_result(
                    &mut stream,
                    envelope.request_id,
                    selected,
                    command_id,
                    &cancel,
                )
                .await?;
            }
            Some(envelope_payload::Message::ActivitySnapshotRequest(request)) => {
                require_session(&request.session_id, &state)?;
                send_message(
                    &mut stream,
                    envelope.request_id,
                    selected,
                    envelope_payload::Message::ActivityEvent(wire::ActivityEvent {
                        session_id: encode_session_id(state.descriptor.session_id),
                        sequence: state.latest_sequence.load(Ordering::Acquire),
                        activity: i32::from(if state.exited.load(Ordering::Acquire) {
                            wire::Activity::Done
                        } else {
                            wire::Activity::Unknown
                        }),
                    }),
                    &cancel,
                )
                .await?;
            }
            Some(envelope_payload::Message::DetachRequest(request)) => {
                require_session(&request.session_id, &state)?;
                return Ok(());
            }
            _ => return Err(HostError::new(HostErrorCode::Protocol)),
        }
    }
}

async fn serve_attach(
    stream: &mut HostWireStream<UnixStream>,
    request_id: [u8; 16],
    version: ProtocolVersion,
    state: &RuntimeState,
    connection_id: u64,
    request: wire::AttachRequest,
    cancel: &CancellationToken,
) -> Result<(), HostError> {
    if request.maximum_replay_bytes == 0
        || request.maximum_replay_bytes > MAX_REPLAY_BYTES
        || request.maximum_replay_records == 0
        || request.maximum_replay_records > MAX_REPLAY_RECORDS
    {
        return Err(HostError::new(HostErrorCode::ResourceLimit));
    }
    let from = OutputSequence::new(request.from_sequence);
    let mut snapshot = None;
    let initial_read = {
        let journal = state.journal.lock().await;
        journal.read_from(from)
    };
    let read = match initial_read {
        Ok(read) => read,
        Err(error) if error.code == termirust_store::JournalErrorCode::HistoryUnavailable => {
            let Some(available) = load_snapshot(&state.descriptor.session_dir)? else {
                send_error(
                    stream,
                    request_id,
                    wire::ErrorCode::SequenceGap,
                    wire::RecoveryHint::Replay,
                    error
                        .expected_sequence
                        .map(OutputSequence::get)
                        .unwrap_or(0),
                    cancel,
                )
                .await?;
                return Ok(());
            };
            let read = state.journal.lock().await.read_from(available.boundary)?;
            snapshot = Some(available);
            read
        }
        Err(error) => return Err(error.into()),
    };
    send_message(
        stream,
        request_id,
        version,
        envelope_payload::Message::ReadyEvent(wire::ReadyEvent {
            session_id: encode_session_id(state.descriptor.session_id),
            latest_sequence: read.latest.unwrap_or(OutputSequence::ZERO).get(),
        }),
        cancel,
    )
    .await?;
    if let Some(TerminalSnapshot {
        boundary,
        columns,
        rows,
        terminal_bytes,
    }) = snapshot
    {
        send_message(
            stream,
            request_id,
            version,
            envelope_payload::Message::ViewportSnapshotEvent(wire::ViewportSnapshotEvent {
                session_id: encode_session_id(state.descriptor.session_id),
                boundary_sequence: boundary.get(),
                viewport: Some(wire::Viewport { columns, rows }),
                terminal_bytes,
            }),
            cancel,
        )
        .await?;
    }
    let mut bytes = 0_u64;
    let mut records = 0_u32;
    for frame in read.frames {
        if frame.kind != JournalKind::Output {
            continue;
        }
        bytes = bytes.saturating_add(frame.payload.len() as u64);
        records = records.saturating_add(1);
        if bytes > request.maximum_replay_bytes || records > request.maximum_replay_records {
            send_error(
                stream,
                request_id,
                wire::ErrorCode::SequenceGap,
                wire::RecoveryHint::Replay,
                frame.sequence.get(),
                cancel,
            )
            .await?;
            return Ok(());
        }
        send_message(
            stream,
            request_id,
            version,
            envelope_payload::Message::OutputEvent(wire::OutputEvent {
                session_id: encode_session_id(state.descriptor.session_id),
                sequence: frame.sequence.get(),
                bytes: frame.payload,
            }),
            cancel,
        )
        .await?;
    }
    send_state(stream, request_id, version, state, connection_id, cancel).await
}

async fn send_state(
    stream: &mut HostWireStream<UnixStream>,
    request_id: [u8; 16],
    version: ProtocolVersion,
    state: &RuntimeState,
    connection_id: u64,
    cancel: &CancellationToken,
) -> Result<(), HostError> {
    let read = state.journal.lock().await.read_from(OutputSequence::ZERO);
    let (earliest, latest) = match read {
        Ok(read) => (
            read.earliest.unwrap_or(OutputSequence::ZERO).get(),
            read.latest.unwrap_or(OutputSequence::ZERO).get(),
        ),
        Err(error) if error.code == termirust_store::JournalErrorCode::HistoryUnavailable => (
            error
                .expected_sequence
                .map(OutputSequence::get)
                .unwrap_or(0),
            state.latest_sequence.load(Ordering::Acquire),
        ),
        Err(error) => return Err(error.into()),
    };
    send_message(
        stream,
        request_id,
        version,
        envelope_payload::Message::StateEvent(wire::StateEvent {
            session_id: encode_session_id(state.descriptor.session_id),
            host_instance_id: encode_host_instance_id(state.descriptor.host_instance_id),
            lifecycle: lifecycle_wire(*state.lifecycle.read().await),
            earliest_sequence: earliest,
            latest_sequence: latest,
            has_writer_lease: state.has_writer(connection_id).await,
            recording_paused: state.recording_paused.load(Ordering::Acquire),
            durable_sequence: state.durable_sequence.load(Ordering::Acquire),
        }),
        cancel,
    )
    .await
}

struct MutationRequest<'a, F> {
    command_bytes: &'a [u8],
    payload: &'a [u8],
    apply: F,
}

async fn handle_mutation<F>(
    stream: &mut HostWireStream<UnixStream>,
    request_id: [u8; 16],
    version: ProtocolVersion,
    state: &RuntimeState,
    request: MutationRequest<'_, F>,
    cancel: &CancellationToken,
) -> Result<(), HostError>
where
    F: FnOnce() -> Result<(), HostError>,
{
    let command_id = decode_command_id(request.command_bytes)
        .map_err(|_| HostError::new(HostErrorCode::Protocol))?;
    validate_command_request(command_id, request_id)?;
    let payload_hash = crc32c::crc32c(request.payload);
    let mut cache = state.idempotency.lock().await;
    match cache.inspect(command_id, payload_hash, Instant::now()) {
        MutationDecision::Apply => {
            (request.apply)()?;
            cache.record(command_id, payload_hash, Instant::now());
        }
        MutationDecision::Replay => {}
        MutationDecision::Conflict => {
            send_error(
                stream,
                request_id,
                wire::ErrorCode::ConflictingDuplicate,
                wire::RecoveryHint::RetryExplicitly,
                0,
                cancel,
            )
            .await?;
            return Ok(());
        }
    }
    send_command_result(stream, request_id, version, command_id, cancel).await
}

fn validate_command_request(command_id: CommandId, request_id: [u8; 16]) -> Result<(), HostError> {
    if command_id.as_uuid().into_bytes() == request_id {
        Ok(())
    } else {
        Err(HostError::new(HostErrorCode::Protocol))
    }
}

async fn send_command_result(
    stream: &mut HostWireStream<UnixStream>,
    request_id: [u8; 16],
    version: ProtocolVersion,
    command_id: CommandId,
    cancel: &CancellationToken,
) -> Result<(), HostError> {
    send_message(
        stream,
        request_id,
        version,
        envelope_payload::Message::CommandResult(wire::CommandResult {
            command_id: encode_command_id(command_id),
            applied: true,
        }),
        cancel,
    )
    .await
}

fn require_session(bytes: &[u8], state: &RuntimeState) -> Result<(), HostError> {
    if decode_session_id(bytes).map_err(|_| HostError::new(HostErrorCode::Protocol))?
        == state.descriptor.session_id
    {
        Ok(())
    } else {
        Err(HostError::new(HostErrorCode::Protocol))
    }
}

fn required_capability(payload: &wire::EnvelopePayload) -> Option<wire::Capability> {
    match payload.message.as_ref()? {
        envelope_payload::Message::GetStateRequest(_) => Some(wire::Capability::State),
        envelope_payload::Message::AttachRequest(_) => Some(wire::Capability::AttachReplay),
        envelope_payload::Message::InputRequest(_) => Some(wire::Capability::Input),
        envelope_payload::Message::ResizeRequest(_) => Some(wire::Capability::Resize),
        envelope_payload::Message::StopRequest(_) => Some(wire::Capability::Stop),
        envelope_payload::Message::InterruptRequest(_) => Some(wire::Capability::Interrupt),
        envelope_payload::Message::ActivitySnapshotRequest(_) => {
            Some(wire::Capability::ActivitySnapshot)
        }
        _ => None,
    }
}

fn decode_checked(envelope: &WireEnvelope) -> Result<wire::EnvelopePayload, HostError> {
    let payload = PreservedPayload::decode(&envelope.payload)?.value;
    if payload_kind(&payload) == Some(envelope.kind) {
        Ok(payload)
    } else {
        Err(HostError::new(HostErrorCode::Protocol))
    }
}

async fn send_message(
    stream: &mut HostWireStream<UnixStream>,
    request_id: [u8; 16],
    version: ProtocolVersion,
    message: envelope_payload::Message,
    cancel: &CancellationToken,
) -> Result<(), HostError> {
    let payload = wire::EnvelopePayload {
        message: Some(message),
    };
    let kind = payload_kind(&payload).ok_or_else(|| HostError::new(HostErrorCode::Protocol))?;
    stream
        .write(
            &WireEnvelope {
                protocol_major: version.major,
                protocol_minor: version.minor,
                kind,
                flags: 0,
                request_id,
                payload: encode_payload(&payload),
            },
            cancel,
        )
        .await
}

async fn send_error(
    stream: &mut HostWireStream<UnixStream>,
    request_id: [u8; 16],
    code: wire::ErrorCode,
    recovery: wire::RecoveryHint,
    expected_sequence: u64,
    cancel: &CancellationToken,
) -> Result<(), HostError> {
    send_message(
        stream,
        request_id,
        CURRENT_PROTOCOL.maximum,
        envelope_payload::Message::ProtocolError(wire::ProtocolError {
            code: i32::from(code),
            recovery: i32::from(recovery),
            supported_protocol: Some(CURRENT_PROTOCOL.into()),
            expected_sequence,
            earliest_available_sequence: expected_sequence,
        }),
        cancel,
    )
    .await
}

fn lifecycle_wire(lifecycle: HostLifecycle) -> i32 {
    i32::from(match lifecycle {
        HostLifecycle::Starting => wire::Lifecycle::Starting,
        HostLifecycle::Ready => wire::Lifecycle::Ready,
        HostLifecycle::Stopping => wire::Lifecycle::Stopping,
        HostLifecycle::Exited => wire::Lifecycle::Exited,
        HostLifecycle::Failed | HostLifecycle::Orphaned => wire::Lifecycle::Failed,
    })
}

#[derive(Clone, Copy)]
struct MutationEntry {
    hash: u32,
    recorded_at: Instant,
}

#[derive(Default)]
struct MutationCache {
    entries: HashMap<CommandId, MutationEntry>,
    order: VecDeque<CommandId>,
}

enum MutationDecision {
    Apply,
    Replay,
    Conflict,
}

impl MutationCache {
    fn inspect(&mut self, id: CommandId, hash: u32, now: Instant) -> MutationDecision {
        self.prune(now);
        match self.entries.get(&id) {
            None => MutationDecision::Apply,
            Some(entry) if entry.hash == hash => MutationDecision::Replay,
            Some(_) => MutationDecision::Conflict,
        }
    }

    fn record(&mut self, id: CommandId, hash: u32, now: Instant) {
        self.prune(now);
        if !self.entries.contains_key(&id) {
            while self.entries.len() >= MAX_IDEMPOTENCY_OUTCOMES {
                if let Some(oldest) = self.order.pop_front() {
                    self.entries.remove(&oldest);
                }
            }
            self.order.push_back(id);
        }
        self.entries.insert(
            id,
            MutationEntry {
                hash,
                recorded_at: now,
            },
        );
    }

    fn prune(&mut self, now: Instant) {
        while let Some(id) = self.order.front().copied() {
            let expired = self.entries.get(&id).is_none_or(|entry| {
                now.saturating_duration_since(entry.recorded_at) >= IDEMPOTENCY_TTL
            });
            if !expired {
                break;
            }
            self.order.pop_front();
            self.entries.remove(&id);
        }
    }
}

#[derive(Default)]
struct NonceCache {
    entries: HashMap<[u8; HANDSHAKE_NONCE_BYTES], Instant>,
    order: VecDeque<[u8; HANDSHAKE_NONCE_BYTES]>,
}

impl NonceCache {
    fn accept(&mut self, nonce: [u8; HANDSHAKE_NONCE_BYTES], now: Instant) -> bool {
        while let Some(oldest) = self.order.front().copied() {
            let expired = self
                .entries
                .get(&oldest)
                .is_none_or(|recorded| now.saturating_duration_since(*recorded) >= IDEMPOTENCY_TTL);
            if !expired {
                break;
            }
            self.order.pop_front();
            self.entries.remove(&oldest);
        }
        if self.entries.contains_key(&nonce) {
            return false;
        }
        while self.entries.len() >= MAX_IDEMPOTENCY_OUTCOMES {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
        self.entries.insert(nonce, now);
        self.order.push_back(nonce);
        true
    }
}

fn monotonic_nanos() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    u64::try_from(START.get_or_init(Instant::now).elapsed().as_nanos()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use termirust_domain::{
        HostInstanceId, HostedSessionId, RuntimeCapability, RuntimeDetectionResult,
        RuntimeDetectionStatus, RuntimeId,
    };
    use termirust_store::JournalLimits;

    fn descriptor_with_detection(
        executable: PathBuf,
        fingerprint: termirust_domain::ExecutableFingerprint,
    ) -> LaunchDescriptor {
        let session_id = HostedSessionId::new();
        let fixture = std::env::temp_dir().join(format!("termirust-runtime-{session_id}"));
        let host_instance_id = HostInstanceId::new();
        LaunchDescriptor {
            format_version: LaunchDescriptor::FORMAT_VERSION,
            session_id,
            host_instance_id,
            runtime_root: fixture.join("runtime"),
            session_dir: fixture.join("session"),
            executable,
            runtime_detection: Some(RuntimeDetectionResult {
                runtime_id: RuntimeId::new("codex").unwrap(),
                descriptor_version: 1,
                status: RuntimeDetectionStatus::Available,
                fingerprint: Some(fingerprint),
                safe_version: Some("1.0.7".to_string()),
                capabilities: RuntimeCapabilitySet::new([
                    RuntimeCapability::InteractivePty,
                    RuntimeCapability::Cancellation,
                ]),
                diagnostic_code: None,
            }),
            arguments: Vec::new(),
            environment: BTreeMap::new(),
            cwd: None,
            columns: 80,
            rows: 24,
            journal_limits: JournalLimits::default(),
            stop_deadlines: crate::StopDeadlines::default(),
        }
    }

    #[test]
    fn process_observation_host_launch_requires_exact_executable_fingerprint_for_managed() {
        let executable = PathBuf::from("/bin/sh");
        let fingerprint = fingerprint_executable(&executable).unwrap();
        let descriptor = descriptor_with_detection(executable, fingerprint);
        let token = ProcessToken::new(descriptor.host_instance_id, 42, 1);

        let recognition = recognition_for_launch(&descriptor, token).unwrap();
        let occupant = recognition.occupant.unwrap();
        assert_eq!(recognition.confidence, RecognitionConfidence::Verified);
        assert!(matches!(
            occupant.ownership,
            OccupantOwnership::Managed {
                host_instance,
                child_token,
            } if host_instance == descriptor.host_instance_id && child_token == token
        ));
        assert!(!occupant.effective_capabilities().is_empty());
    }

    #[test]
    fn process_observation_host_launch_fails_closed_after_executable_change() {
        let fixture = tempfile::tempdir().unwrap();
        let changed = fixture.path().join("changed-runtime");
        fs::write(&changed, b"different executable").unwrap();
        let descriptor = descriptor_with_detection(
            PathBuf::from("/bin/sh"),
            fingerprint_executable(&changed).unwrap(),
        );
        let token = ProcessToken::new(descriptor.host_instance_id, 42, 1);

        let recognition = recognition_for_launch(&descriptor, token).unwrap();
        let occupant = recognition.occupant.unwrap();
        assert_eq!(recognition.confidence, RecognitionConfidence::Uncertain);
        assert_eq!(occupant.ownership, OccupantOwnership::Ambiguous);
        assert!(occupant.effective_capabilities().is_empty());
    }
}
