use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use serde::{Deserialize, Serialize};
use termirust_client::{LocalEndpoint, UserOnlyUnixListener};
use termirust_domain::{HostedSessionId, OccupantGeneration, OutputSequence};
use tokio::net::UnixStream;
use tokio_util::sync::CancellationToken;

use crate::{
    ControllerCommand, ControllerResponse, ControllerSessionCapability, ControllerSessionOrigin,
    ControllerSessionSummary, HostCommandContext, ListenerError, ListenerErrorCode,
    MAX_SNAPSHOT_CHUNK_BYTES, read_bounded_frame, write_bounded_frame,
};

const MAX_BRIDGE_FRAME_BYTES: usize = 4 * 1024 * 1024;
const MAX_JOURNAL_BYTES: usize = 2 * 1024 * 1024;
const MAX_OUTPUT_CHUNK_BYTES: usize = 64 * 1024;

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopPaneBridgeEndpoint {
    runtime_root: PathBuf,
    endpoint_id: HostedSessionId,
}

impl DesktopPaneBridgeEndpoint {
    pub fn new(runtime_root: PathBuf, endpoint_id: HostedSessionId) -> Result<Self, ListenerError> {
        let endpoint = Self {
            runtime_root,
            endpoint_id,
        };
        endpoint.validate()?;
        Ok(endpoint)
    }

    pub fn validate(&self) -> Result<(), ListenerError> {
        if !self.runtime_root.is_absolute()
            || self.runtime_root.as_os_str().is_empty()
            || self.runtime_root.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            })
        {
            return Err(ListenerError::new(ListenerErrorCode::InvalidPolicy));
        }
        Ok(())
    }

    fn local_endpoint(&self) -> LocalEndpoint {
        LocalEndpoint::new(&self.runtime_root, self.endpoint_id)
    }
}

impl fmt::Debug for DesktopPaneBridgeEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DesktopPaneBridgeEndpoint")
            .field("runtime_root", &"[REDACTED]")
            .field("endpoint_id", &self.endpoint_id)
            .finish()
    }
}

#[derive(Clone)]
pub struct DesktopPaneTransport {
    input: Arc<dyn Fn(Vec<u8>) -> bool + Send + Sync>,
    resize: Option<Arc<dyn Fn(u32, u32) -> bool + Send + Sync>>,
}

impl DesktopPaneTransport {
    pub fn new(input: impl Fn(Vec<u8>) -> bool + Send + Sync + 'static) -> Self {
        Self {
            input: Arc::new(input),
            resize: None,
        }
    }

    pub fn with_resize(
        input: impl Fn(Vec<u8>) -> bool + Send + Sync + 'static,
        resize: impl Fn(u32, u32) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            input: Arc::new(input),
            resize: Some(Arc::new(resize)),
        }
    }

    fn send_input(&self, bytes: Vec<u8>) -> bool {
        (self.input)(bytes)
    }

    fn send_resize(&self, columns: u32, rows: u32) -> bool {
        self.resize
            .as_ref()
            .is_some_and(|resize| resize(columns, rows))
    }

    fn supports_resize(&self) -> bool {
        self.resize.is_some()
    }
}

impl fmt::Debug for DesktopPaneTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DesktopPaneTransport([OPAQUE])")
    }
}

#[derive(Clone, Debug)]
pub struct DesktopPaneRegistration {
    pub session_id: HostedSessionId,
    pub title: String,
    pub runtime: String,
    pub columns: u32,
    pub rows: u32,
    pub transport: DesktopPaneTransport,
}

#[derive(Clone, Default)]
pub struct DesktopPaneRegistry {
    inner: Arc<Mutex<RegistryState>>,
}

impl fmt::Debug for DesktopPaneRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DesktopPaneRegistry([REDACTED])")
    }
}

#[derive(Default)]
struct RegistryState {
    revision: u64,
    panes: HashMap<HostedSessionId, DesktopPaneRecord>,
}

struct DesktopPaneRecord {
    title: String,
    runtime: String,
    generation: OccupantGeneration,
    columns: u32,
    rows: u32,
    transport: DesktopPaneTransport,
    latest_sequence: OutputSequence,
    snapshot_sequence: OutputSequence,
    snapshot: Vec<u8>,
    output_bytes: usize,
    outputs: VecDeque<DesktopPaneOutput>,
    writer_connection: Option<u64>,
    controller_viewport: Option<(u32, u32)>,
}

#[derive(Clone)]
struct DesktopPaneOutput {
    sequence: OutputSequence,
    bytes: Vec<u8>,
}

impl DesktopPaneRegistry {
    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .map(|state| state.panes.len())
            .unwrap_or_default()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn register(&self, registration: DesktopPaneRegistration) {
        let Ok(mut state) = self.inner.lock() else {
            return;
        };
        if let Some(existing) = state.panes.get_mut(&registration.session_id) {
            existing.title = bounded_title(registration.title);
            existing.runtime = bounded_runtime(registration.runtime);
            existing.columns = registration.columns.clamp(1, 1_000);
            existing.rows = registration.rows.clamp(1, 1_000);
            existing.transport = registration.transport;
            return;
        }
        state.panes.insert(
            registration.session_id,
            DesktopPaneRecord {
                title: bounded_title(registration.title),
                runtime: bounded_runtime(registration.runtime),
                generation: OccupantGeneration::new(1),
                columns: registration.columns.clamp(1, 1_000),
                rows: registration.rows.clamp(1, 1_000),
                transport: registration.transport,
                latest_sequence: OutputSequence::ZERO,
                snapshot_sequence: OutputSequence::ZERO,
                snapshot: Vec::new(),
                output_bytes: 0,
                outputs: VecDeque::new(),
                writer_connection: None,
                controller_viewport: None,
            },
        );
        state.revision = next_revision(state.revision);
    }

    pub fn update_title(&self, session_id: HostedSessionId, title: String) {
        let Ok(mut state) = self.inner.lock() else {
            return;
        };
        let title = bounded_title(title);
        let changed = state.panes.get_mut(&session_id).is_some_and(|pane| {
            if pane.title == title {
                false
            } else {
                pane.title = title;
                true
            }
        });
        if changed {
            state.revision = next_revision(state.revision);
        }
    }

    pub fn update_size(&self, session_id: HostedSessionId, columns: u32, rows: u32) {
        if let Ok(mut state) = self.inner.lock()
            && let Some(pane) = state.panes.get_mut(&session_id)
        {
            pane.columns = columns.clamp(1, 1_000);
            pane.rows = rows.clamp(1, 1_000);
        }
    }

    pub fn controller_viewport(&self, session_id: HostedSessionId) -> Option<(u32, u32)> {
        self.inner
            .lock()
            .ok()
            .and_then(|state| state.panes.get(&session_id)?.controller_viewport)
    }

    pub fn remove(&self, session_id: HostedSessionId) {
        let Ok(mut state) = self.inner.lock() else {
            return;
        };
        if state.panes.remove(&session_id).is_some() {
            state.revision = next_revision(state.revision);
        }
    }

    pub fn append_output<F>(&self, session_id: HostedSessionId, bytes: &[u8], snapshot: F)
    where
        F: FnOnce() -> Vec<u8>,
    {
        if bytes.is_empty() {
            return;
        }
        let Ok(mut state) = self.inner.lock() else {
            return;
        };
        let Some(pane) = state.panes.get_mut(&session_id) else {
            return;
        };
        let chunks = bytes.chunks(MAX_OUTPUT_CHUNK_BYTES).collect::<Vec<_>>();
        let incoming_bytes = chunks.iter().map(|chunk| chunk.len()).sum::<usize>();
        for chunk in chunks {
            let Some(sequence) = pane.latest_sequence.checked_next() else {
                return;
            };
            pane.latest_sequence = sequence;
            pane.output_bytes = pane.output_bytes.saturating_add(chunk.len());
            pane.outputs.push_back(DesktopPaneOutput {
                sequence,
                bytes: chunk.to_vec(),
            });
        }
        if pane.output_bytes > MAX_JOURNAL_BYTES || incoming_bytes > MAX_JOURNAL_BYTES {
            pane.snapshot = snapshot();
            pane.snapshot_sequence = pane.latest_sequence;
            pane.outputs.clear();
            pane.output_bytes = 0;
        }
    }

    fn snapshot(&self) -> Result<RegistrySnapshot, ListenerError> {
        let state = self
            .inner
            .lock()
            .map_err(|_| ListenerError::new(ListenerErrorCode::HostUnavailable))?;
        let mut sessions = state
            .panes
            .iter()
            .map(|(session_id, pane)| {
                let mut capabilities = vec![
                    ControllerSessionCapability::ObserveSessions,
                    ControllerSessionCapability::AttachOutput,
                    ControllerSessionCapability::SendInput,
                ];
                if pane.transport.supports_resize() {
                    capabilities.push(ControllerSessionCapability::Resize);
                }
                ControllerSessionSummary {
                    session_id: *session_id,
                    host_instance_id: None,
                    origin: ControllerSessionOrigin::Terminal,
                    runtime: Some(pane.runtime.clone()),
                    capabilities,
                    title: pane.title.clone(),
                    project: None,
                    group: None,
                    lifecycle: "live".to_owned(),
                    activity: "unknown".to_owned(),
                    occupant_generation: Some(pane.generation),
                    last_output_sequence: pane.latest_sequence,
                    has_writer: pane.writer_connection.is_some(),
                    unread: false,
                }
            })
            .collect::<Vec<_>>();
        sessions.sort_by(|a, b| a.title.cmp(&b.title).then(a.session_id.cmp(&b.session_id)));
        Ok(RegistrySnapshot {
            revision: state.revision.max(1),
            sessions,
        })
    }

    fn context(
        &self,
        connection_id: u64,
        session_id: HostedSessionId,
    ) -> Result<Option<HostCommandContext>, ListenerError> {
        let state = self
            .inner
            .lock()
            .map_err(|_| ListenerError::new(ListenerErrorCode::HostUnavailable))?;
        Ok(state.panes.get(&session_id).map(|pane| HostCommandContext {
            occupant_generation: Some(pane.generation),
            has_writer_lease: pane.writer_connection == Some(connection_id),
        }))
    }

    fn release_connection(&self, connection_id: u64) {
        if let Ok(mut state) = self.inner.lock() {
            for pane in state.panes.values_mut() {
                if pane.writer_connection == Some(connection_id) {
                    pane.writer_connection = None;
                    pane.controller_viewport = None;
                    let _ = pane.transport.send_resize(pane.columns, pane.rows);
                }
            }
        }
    }
}

struct RegistrySnapshot {
    revision: u64,
    sessions: Vec<ControllerSessionSummary>,
}

fn bounded_title(value: String) -> String {
    value.chars().take(256).collect()
}

fn bounded_runtime(value: String) -> String {
    value.chars().take(64).collect()
}

fn next_revision(current: u64) -> u64 {
    current.saturating_add(1).max(1)
}

pub struct DesktopPaneBridgeServer {
    endpoint: DesktopPaneBridgeEndpoint,
    cancel: CancellationToken,
    thread: Option<thread::JoinHandle<()>>,
}

impl fmt::Debug for DesktopPaneBridgeServer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DesktopPaneBridgeServer")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl DesktopPaneBridgeServer {
    pub fn start(
        runtime_root: impl AsRef<Path>,
        registry: DesktopPaneRegistry,
    ) -> Result<Self, ListenerError> {
        let endpoint = DesktopPaneBridgeEndpoint::new(
            runtime_root.as_ref().to_path_buf(),
            HostedSessionId::new(),
        )?;
        let thread_endpoint = endpoint.clone();
        let cancel = CancellationToken::new();
        let thread_cancel = cancel.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("termirust-desktop-pane-bridge".to_owned())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                let Ok(runtime) = runtime else {
                    let _ = ready_tx.send(Err(ListenerErrorCode::HostUnavailable));
                    return;
                };
                runtime.block_on(async move {
                    let listener = match UserOnlyUnixListener::bind(thread_endpoint.local_endpoint()) {
                        Ok(listener) => listener,
                        Err(_) => {
                            let _ = ready_tx.send(Err(ListenerErrorCode::HostUnavailable));
                            return;
                        }
                    };
                    if ready_tx.send(Ok(())).is_err() {
                        return;
                    }
                    let next_connection = Arc::new(AtomicU64::new(1));
                    loop {
                        tokio::select! {
                            _ = thread_cancel.cancelled() => break,
                            accepted = listener.accept() => {
                                let Ok(stream) = accepted else { continue; };
                                let connection_id = next_connection.fetch_add(1, Ordering::Relaxed).max(1);
                                let connection_registry = registry.clone();
                                tokio::spawn(async move {
                                    let _ = serve_connection(stream, connection_id, connection_registry.clone()).await;
                                    connection_registry.release_connection(connection_id);
                                });
                            }
                        }
                    }
                });
            })
            .map_err(|_| ListenerError::new(ListenerErrorCode::HostUnavailable))?;
        match ready_rx.recv_timeout(std::time::Duration::from_secs(2)) {
            Ok(Ok(())) => Ok(Self {
                endpoint,
                cancel,
                thread: Some(thread),
            }),
            _ => {
                cancel.cancel();
                let _ = thread.join();
                Err(ListenerError::new(ListenerErrorCode::HostUnavailable))
            }
        }
    }

    pub fn endpoint(&self) -> DesktopPaneBridgeEndpoint {
        self.endpoint.clone()
    }
}

impl Drop for DesktopPaneBridgeServer {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum BridgeRequest {
    List,
    Context {
        session_id: HostedSessionId,
    },
    Execute {
        command_id: termirust_domain::CommandId,
        command: ControllerCommand,
    },
    Next,
    Reset,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum BridgeReply {
    Sessions {
        revision: u64,
        sessions: Vec<ControllerSessionSummary>,
    },
    Context {
        context: Option<HostCommandContextWire>,
    },
    Responses {
        responses: Vec<ControllerResponse>,
    },
    Next {
        response: Option<ControllerResponse>,
    },
    Reset,
    Error,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct HostCommandContextWire {
    occupant_generation: Option<OccupantGeneration>,
    has_writer_lease: bool,
}

impl From<HostCommandContext> for HostCommandContextWire {
    fn from(value: HostCommandContext) -> Self {
        Self {
            occupant_generation: value.occupant_generation,
            has_writer_lease: value.has_writer_lease,
        }
    }
}

impl From<HostCommandContextWire> for HostCommandContext {
    fn from(value: HostCommandContextWire) -> Self {
        Self {
            occupant_generation: value.occupant_generation,
            has_writer_lease: value.has_writer_lease,
        }
    }
}

struct BridgeConnectionState {
    active: Option<ActiveDesktopAttach>,
}

#[derive(Clone, Copy)]
struct ActiveDesktopAttach {
    session_id: HostedSessionId,
    generation: OccupantGeneration,
    cursor: OutputSequence,
}

async fn serve_connection(
    mut stream: UnixStream,
    connection_id: u64,
    registry: DesktopPaneRegistry,
) -> Result<(), ListenerError> {
    let mut connection = BridgeConnectionState { active: None };
    loop {
        let bytes = read_bounded_frame(&mut stream, MAX_BRIDGE_FRAME_BYTES).await?;
        let request = serde_json::from_slice::<BridgeRequest>(&bytes)
            .map_err(|_| ListenerError::new(ListenerErrorCode::MalformedFrame))?;
        let reply = handle_request(&registry, connection_id, &mut connection, request);
        let bytes = serde_json::to_vec(&reply)
            .map_err(|_| ListenerError::new(ListenerErrorCode::MalformedFrame))?;
        if bytes.len() > MAX_BRIDGE_FRAME_BYTES {
            return Err(ListenerError::new(ListenerErrorCode::FrameTooLarge));
        }
        write_bounded_frame(&mut stream, &bytes, MAX_BRIDGE_FRAME_BYTES).await?;
    }
}

fn handle_request(
    registry: &DesktopPaneRegistry,
    connection_id: u64,
    connection: &mut BridgeConnectionState,
    request: BridgeRequest,
) -> BridgeReply {
    match request {
        BridgeRequest::List => match registry.snapshot() {
            Ok(snapshot) => BridgeReply::Sessions {
                revision: snapshot.revision,
                sessions: snapshot.sessions,
            },
            Err(_) => BridgeReply::Error,
        },
        BridgeRequest::Context { session_id } => BridgeReply::Context {
            context: registry
                .context(connection_id, session_id)
                .ok()
                .flatten()
                .map(Into::into),
        },
        BridgeRequest::Execute {
            command_id,
            command,
        } => BridgeReply::Responses {
            responses: execute_desktop_command(
                registry,
                connection_id,
                connection,
                command_id,
                command,
            ),
        },
        BridgeRequest::Next => BridgeReply::Next {
            response: match next_desktop_output(registry, connection) {
                Ok(response) => response,
                Err(()) => return BridgeReply::Error,
            },
        },
        BridgeRequest::Reset => {
            registry.release_connection(connection_id);
            connection.active = None;
            BridgeReply::Reset
        }
    }
}

fn execute_desktop_command(
    registry: &DesktopPaneRegistry,
    connection_id: u64,
    connection: &mut BridgeConnectionState,
    command_id: termirust_domain::CommandId,
    command: ControllerCommand,
) -> Vec<ControllerResponse> {
    match command {
        ControllerCommand::ListSessions { .. } => vec![error_response(command_id)],
        ControllerCommand::Attach {
            session_id,
            occupant_generation,
            from_sequence,
            columns: _,
            rows: _,
        } => {
            if connection
                .active
                .is_some_and(|active| active.session_id != session_id)
            {
                registry.release_connection(connection_id);
            }
            let Ok(mut state) = registry.inner.lock() else {
                return vec![error_response(command_id)];
            };
            let Some(pane) = state.panes.get_mut(&session_id) else {
                return vec![error_response(command_id)];
            };
            if pane.generation != occupant_generation {
                return vec![error_response(command_id)];
            }
            let earliest = pane.outputs.front().map(|output| output.sequence);
            let needs_snapshot = from_sequence < pane.snapshot_sequence
                || earliest.is_some_and(|first| {
                    from_sequence
                        .checked_next()
                        .is_some_and(|expected| expected < first)
                });
            let replay_from = if needs_snapshot {
                pane.snapshot_sequence
            } else {
                from_sequence
            };
            let outputs = pane
                .outputs
                .iter()
                .filter(|output| output.sequence > replay_from)
                .cloned()
                .collect::<Vec<_>>();
            let replay_through = outputs
                .last()
                .map(|output| output.sequence)
                .unwrap_or(replay_from);
            connection.active = Some(ActiveDesktopAttach {
                session_id,
                generation: occupant_generation,
                cursor: replay_through,
            });
            let mut responses = Vec::new();
            if needs_snapshot {
                append_snapshot_responses(
                    &mut responses,
                    command_id,
                    session_id,
                    pane.snapshot_sequence,
                    pane.columns,
                    pane.rows,
                    &pane.snapshot,
                );
            } else if from_sequence == OutputSequence::ZERO {
                append_snapshot_responses(
                    &mut responses,
                    command_id,
                    session_id,
                    OutputSequence::ZERO,
                    pane.columns,
                    pane.rows,
                    &[],
                );
            }
            responses.push(ControllerResponse::Attached {
                command_id,
                session_id,
                occupant_generation,
                replay_through_sequence: replay_through,
                has_writer_lease: pane.writer_connection == Some(connection_id),
            });
            responses.extend(
                outputs
                    .into_iter()
                    .map(|output| ControllerResponse::Output {
                        session_id,
                        sequence: output.sequence,
                        bytes: output.bytes,
                    }),
            );
            responses
        }
        ControllerCommand::AcquireWriter {
            session_id,
            occupant_generation,
        } => {
            let applied = registry.inner.lock().ok().and_then(|mut state| {
                let pane = state.panes.get_mut(&session_id)?;
                if pane.generation != occupant_generation
                    || pane
                        .writer_connection
                        .is_some_and(|owner| owner != connection_id)
                {
                    return Some(false);
                }
                pane.writer_connection = Some(connection_id);
                Some(true)
            });
            completion(command_id, applied.unwrap_or(false))
        }
        ControllerCommand::ReleaseWriter {
            session_id,
            occupant_generation,
        } => {
            let applied = registry.inner.lock().ok().and_then(|mut state| {
                let pane = state.panes.get_mut(&session_id)?;
                if pane.generation != occupant_generation {
                    return Some(false);
                }
                if pane.writer_connection == Some(connection_id) {
                    pane.writer_connection = None;
                    pane.controller_viewport = None;
                    let _ = pane.transport.send_resize(pane.columns, pane.rows);
                }
                Some(true)
            });
            completion(command_id, applied.unwrap_or(false))
        }
        ControllerCommand::Input {
            session_id,
            occupant_generation,
            bytes,
        } => {
            let transport = registry.inner.lock().ok().and_then(|state| {
                let pane = state.panes.get(&session_id)?;
                (pane.generation == occupant_generation
                    && pane.writer_connection == Some(connection_id))
                .then(|| pane.transport.clone())
            });
            completion(
                command_id,
                transport.is_some_and(|transport| transport.send_input(bytes)),
            )
        }
        ControllerCommand::Resize {
            session_id,
            occupant_generation,
            columns,
            rows,
        } => {
            let applied = registry.inner.lock().ok().and_then(|mut state| {
                let pane = state.panes.get_mut(&session_id)?;
                if pane.generation != occupant_generation
                    || pane.writer_connection != Some(connection_id)
                {
                    return Some(false);
                }
                let columns = columns.clamp(1, 1_000);
                let rows = rows.clamp(1, 1_000);
                if !pane.transport.send_resize(columns, rows) {
                    return Some(false);
                }
                pane.controller_viewport = Some((columns, rows));
                Some(true)
            });
            completion(command_id, applied.unwrap_or(false))
        }
        ControllerCommand::Approval { .. } => {
            vec![ControllerResponse::Error {
                command_id,
                code: "unsupported_for_desktop_pane".to_owned(),
                completion_unknown: false,
            }]
        }
        ControllerCommand::Detach { session_id, .. } => {
            if connection
                .active
                .is_some_and(|active| active.session_id == session_id)
            {
                registry.release_connection(connection_id);
                connection.active = None;
            }
            vec![ControllerResponse::Detached { command_id }]
        }
    }
}

fn next_desktop_output(
    registry: &DesktopPaneRegistry,
    connection: &mut BridgeConnectionState,
) -> Result<Option<ControllerResponse>, ()> {
    let Some(active) = connection.active else {
        return Ok(None);
    };
    let state = registry.inner.lock().map_err(|_| ())?;
    let pane = state.panes.get(&active.session_id).ok_or(())?;
    if pane.generation != active.generation {
        return Err(());
    }
    if active.cursor < pane.snapshot_sequence {
        return Err(());
    }
    let Some(output) = pane
        .outputs
        .iter()
        .find(|output| output.sequence > active.cursor)
        .cloned()
    else {
        return Ok(None);
    };
    connection.active = Some(ActiveDesktopAttach {
        cursor: output.sequence,
        ..active
    });
    Ok(Some(ControllerResponse::Output {
        session_id: active.session_id,
        sequence: output.sequence,
        bytes: output.bytes,
    }))
}

fn append_snapshot_responses(
    responses: &mut Vec<ControllerResponse>,
    command_id: termirust_domain::CommandId,
    session_id: HostedSessionId,
    boundary_sequence: OutputSequence,
    columns: u32,
    rows: u32,
    bytes: &[u8],
) {
    let chunk_count = bytes.len().max(1).div_ceil(MAX_SNAPSHOT_CHUNK_BYTES);
    if bytes.is_empty() {
        responses.push(ControllerResponse::Snapshot {
            command_id,
            session_id,
            boundary_sequence,
            columns,
            rows,
            chunk_index: 0,
            chunk_count: 1,
            bytes: Vec::new(),
        });
        return;
    }
    for (index, chunk) in bytes.chunks(MAX_SNAPSHOT_CHUNK_BYTES).enumerate() {
        responses.push(ControllerResponse::Snapshot {
            command_id,
            session_id,
            boundary_sequence,
            columns,
            rows,
            chunk_index: u32::try_from(index).unwrap_or(u32::MAX),
            chunk_count: u32::try_from(chunk_count).unwrap_or(u32::MAX),
            bytes: chunk.to_vec(),
        });
    }
}

fn completion(command_id: termirust_domain::CommandId, applied: bool) -> Vec<ControllerResponse> {
    vec![ControllerResponse::Completed {
        command_id,
        applied,
    }]
}

fn error_response(command_id: termirust_domain::CommandId) -> ControllerResponse {
    ControllerResponse::Error {
        command_id,
        code: "host_unavailable".to_owned(),
        completion_unknown: false,
    }
}

pub(crate) struct DesktopPaneBridgeClient {
    stream: UnixStream,
}

impl DesktopPaneBridgeClient {
    pub(crate) async fn connect(
        endpoint: &DesktopPaneBridgeEndpoint,
    ) -> Result<Self, ListenerError> {
        endpoint.validate()?;
        let stream = UnixStream::connect(endpoint.local_endpoint().socket_path())
            .await
            .map_err(|_| ListenerError::new(ListenerErrorCode::HostUnavailable))?;
        Ok(Self { stream })
    }

    async fn request(&mut self, request: BridgeRequest) -> Result<BridgeReply, ListenerError> {
        let bytes = serde_json::to_vec(&request)
            .map_err(|_| ListenerError::new(ListenerErrorCode::MalformedFrame))?;
        write_bounded_frame(&mut self.stream, &bytes, MAX_BRIDGE_FRAME_BYTES).await?;
        let bytes = read_bounded_frame(&mut self.stream, MAX_BRIDGE_FRAME_BYTES).await?;
        serde_json::from_slice(&bytes)
            .map_err(|_| ListenerError::new(ListenerErrorCode::MalformedFrame))
    }

    pub(crate) async fn list(
        &mut self,
    ) -> Result<(u64, Vec<ControllerSessionSummary>), ListenerError> {
        match self.request(BridgeRequest::List).await? {
            BridgeReply::Sessions { revision, sessions } => Ok((revision, sessions)),
            _ => Err(ListenerError::new(ListenerErrorCode::HostUnavailable)),
        }
    }

    pub(crate) async fn context(
        &mut self,
        session_id: HostedSessionId,
    ) -> Result<Option<HostCommandContext>, ListenerError> {
        match self.request(BridgeRequest::Context { session_id }).await? {
            BridgeReply::Context { context } => Ok(context.map(Into::into)),
            _ => Err(ListenerError::new(ListenerErrorCode::HostUnavailable)),
        }
    }

    pub(crate) async fn execute(
        &mut self,
        command_id: termirust_domain::CommandId,
        command: ControllerCommand,
    ) -> Result<Vec<ControllerResponse>, ListenerError> {
        match self
            .request(BridgeRequest::Execute {
                command_id,
                command,
            })
            .await?
        {
            BridgeReply::Responses { responses } => Ok(responses),
            _ => Err(ListenerError::new(ListenerErrorCode::HostUnavailable)),
        }
    }

    pub(crate) async fn next(&mut self) -> Result<Option<ControllerResponse>, ListenerError> {
        match self.request(BridgeRequest::Next).await? {
            BridgeReply::Next { response } => Ok(response),
            _ => Err(ListenerError::new(ListenerErrorCode::HostUnavailable)),
        }
    }

    pub(crate) async fn reset(&mut self) -> Result<(), ListenerError> {
        match self.request(BridgeRequest::Reset).await? {
            BridgeReply::Reset => Ok(()),
            _ => Err(ListenerError::new(ListenerErrorCode::HostUnavailable)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use termirust_domain::CommandId;

    fn registration(
        session_id: HostedSessionId,
        writes: Arc<AtomicUsize>,
    ) -> DesktopPaneRegistration {
        DesktopPaneRegistration {
            session_id,
            title: "Local Terminal".to_owned(),
            runtime: "local_shell".to_owned(),
            columns: 80,
            rows: 24,
            transport: DesktopPaneTransport::with_resize(
                move |bytes| {
                    writes.fetch_add(bytes.len(), Ordering::Relaxed);
                    true
                },
                |_, _| true,
            ),
        }
    }

    #[test]
    fn registry_lists_replays_controls_and_removes_live_panes() {
        let registry = DesktopPaneRegistry::default();
        let session_id = HostedSessionId::new();
        let writes = Arc::new(AtomicUsize::new(0));
        registry.register(registration(session_id, writes.clone()));
        registry.append_output(session_id, b"hello\r\n", Vec::new);

        let listed = registry.snapshot().unwrap();
        assert_eq!(listed.sessions.len(), 1);
        assert_eq!(listed.sessions[0].title, "Local Terminal");
        assert_eq!(listed.sessions[0].origin, ControllerSessionOrigin::Terminal);
        assert!(
            listed.sessions[0]
                .capabilities
                .contains(&ControllerSessionCapability::Resize)
        );

        let mut connection = BridgeConnectionState { active: None };
        let attached = execute_desktop_command(
            &registry,
            7,
            &mut connection,
            CommandId::new(),
            ControllerCommand::Attach {
                session_id,
                occupant_generation: OccupantGeneration::new(1),
                from_sequence: OutputSequence::ZERO,
                columns: 40,
                rows: 12,
            },
        );
        assert!(attached.iter().any(|response| matches!(
            response,
            ControllerResponse::Snapshot {
                boundary_sequence: OutputSequence::ZERO,
                columns: 80,
                rows: 24,
                bytes,
                ..
            } if bytes.is_empty()
        )));
        assert!(attached.iter().any(|response| matches!(response, ControllerResponse::Output { bytes, .. } if bytes == b"hello\r\n")));

        let acquired = execute_desktop_command(
            &registry,
            7,
            &mut connection,
            CommandId::new(),
            ControllerCommand::AcquireWriter {
                session_id,
                occupant_generation: OccupantGeneration::new(1),
            },
        );
        assert!(matches!(
            acquired.as_slice(),
            [ControllerResponse::Completed { applied: true, .. }]
        ));
        let resized = execute_desktop_command(
            &registry,
            7,
            &mut connection,
            CommandId::new(),
            ControllerCommand::Resize {
                session_id,
                occupant_generation: OccupantGeneration::new(1),
                columns: 40,
                rows: 12,
            },
        );
        assert!(matches!(
            resized.as_slice(),
            [ControllerResponse::Completed { applied: true, .. }]
        ));
        assert_eq!(registry.controller_viewport(session_id), Some((40, 12)));
        let input = execute_desktop_command(
            &registry,
            7,
            &mut connection,
            CommandId::new(),
            ControllerCommand::Input {
                session_id,
                occupant_generation: OccupantGeneration::new(1),
                bytes: b"pwd\r".to_vec(),
            },
        );
        assert!(matches!(
            input.as_slice(),
            [ControllerResponse::Completed { applied: true, .. }]
        ));
        assert_eq!(writes.load(Ordering::Relaxed), 4);

        let released = execute_desktop_command(
            &registry,
            7,
            &mut connection,
            CommandId::new(),
            ControllerCommand::ReleaseWriter {
                session_id,
                occupant_generation: OccupantGeneration::new(1),
            },
        );
        assert!(matches!(
            released.as_slice(),
            [ControllerResponse::Completed { applied: true, .. }]
        ));
        assert_eq!(registry.controller_viewport(session_id), None);

        registry.remove(session_id);
        assert!(registry.snapshot().unwrap().sessions.is_empty());
        assert!(registry.context(7, session_id).unwrap().is_none());
    }

    #[test]
    fn journal_checkpoints_to_a_bounded_snapshot_after_overflow() {
        let registry = DesktopPaneRegistry::default();
        let session_id = HostedSessionId::new();
        registry.register(registration(session_id, Arc::new(AtomicUsize::new(0))));
        let output = vec![b'x'; MAX_JOURNAL_BYTES + 1];
        registry.append_output(session_id, &output, || b"current-screen".to_vec());

        let state = registry.inner.lock().unwrap();
        let pane = state.panes.get(&session_id).unwrap();
        assert!(pane.outputs.is_empty());
        assert_eq!(pane.output_bytes, 0);
        assert_eq!(pane.snapshot, b"current-screen");
        assert_eq!(pane.snapshot_sequence, pane.latest_sequence);
    }

    #[tokio::test]
    async fn same_user_socket_round_trip_lists_attaches_and_forwards_input() {
        let fixture = tempfile::tempdir().unwrap();
        let registry = DesktopPaneRegistry::default();
        let session_id = HostedSessionId::new();
        let writes = Arc::new(AtomicUsize::new(0));
        registry.register(registration(session_id, writes.clone()));
        registry.append_output(session_id, b"ready\r\n", Vec::new);
        let server = DesktopPaneBridgeServer::start(fixture.path(), registry).unwrap();
        let mut client = DesktopPaneBridgeClient::connect(&server.endpoint())
            .await
            .unwrap();

        let (_, sessions) = client.list().await.unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, session_id);
        assert_eq!(
            client
                .context(session_id)
                .await
                .unwrap()
                .unwrap()
                .occupant_generation,
            Some(OccupantGeneration::new(1))
        );

        let attached = client
            .execute(
                CommandId::new(),
                ControllerCommand::Attach {
                    session_id,
                    occupant_generation: OccupantGeneration::new(1),
                    from_sequence: OutputSequence::ZERO,
                    columns: 80,
                    rows: 24,
                },
            )
            .await
            .unwrap();
        assert!(attached.iter().any(|response| matches!(
            response,
            ControllerResponse::Output { bytes, .. } if bytes == b"ready\r\n"
        )));
        client
            .execute(
                CommandId::new(),
                ControllerCommand::AcquireWriter {
                    session_id,
                    occupant_generation: OccupantGeneration::new(1),
                },
            )
            .await
            .unwrap();
        client
            .execute(
                CommandId::new(),
                ControllerCommand::Input {
                    session_id,
                    occupant_generation: OccupantGeneration::new(1),
                    bytes: b"whoami\r".to_vec(),
                },
            )
            .await
            .unwrap();
        assert_eq!(writes.load(Ordering::Relaxed), 7);

        drop(client);
        drop(server);
    }
}
