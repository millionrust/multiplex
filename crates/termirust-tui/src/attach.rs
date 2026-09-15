use std::sync::Arc;
use std::thread;
use std::time::Duration;

use rand::RngCore as _;
use termirust_client::{
    ClientError, ClientErrorCode, ConnectOptions, GpuiAttachModel, HostClient, LocalEndpoint,
    OutputDisposition, SequencedOutput,
};
use termirust_domain::{CommandId, HostInstanceId, HostedSessionId, OutputSequence};
use termirust_host_protocol::wire;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::input::TerminalInputModel;
use crate::terminal_view::TerminalView;

const COMMAND_QUEUE_CAPACITY: usize = 64;
const LIVE_POLL_INTERVAL: Duration = Duration::from_millis(50);
const ATTACH_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Viewport {
    pub columns: u16,
    pub rows: u16,
}

impl Viewport {
    pub const fn new(columns: u16, rows: u16) -> Self {
        Self { columns, rows }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TuiAttachState {
    Detached,
    Attaching,
    Replaying,
    LiveReadOnly,
    LiveInteractive,
    Gap,
    Exited,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostLifecycle {
    Starting,
    Ready,
    Stopping,
    Exited,
    Failed,
    Unknown,
}

impl HostLifecycle {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Ready => "ready",
            Self::Stopping => "stopping",
            Self::Exited => "exited",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }

    const fn terminal(self) -> bool {
        matches!(self, Self::Exited | Self::Failed)
    }
}

#[derive(Clone, Debug)]
pub struct HostAttachState {
    pub lifecycle: HostLifecycle,
    pub earliest_sequence: OutputSequence,
    pub latest_sequence: OutputSequence,
    pub durable_sequence: OutputSequence,
    pub has_writer_lease: bool,
    pub recording_paused: bool,
}

#[derive(Clone, Debug)]
pub struct AttachSnapshot {
    pub boundary: OutputSequence,
    pub terminal_bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct AttachBatch {
    pub host_instance_id: HostInstanceId,
    pub snapshot: Option<AttachSnapshot>,
    pub outputs: Vec<SequencedOutput>,
    pub state: HostAttachState,
}

#[derive(Clone, Debug)]
pub enum AttachEvent {
    Attaching {
        generation: u64,
    },
    Batch {
        generation: u64,
        batch: AttachBatch,
    },
    LeaseChanged {
        generation: u64,
        held: bool,
    },
    InputRejected {
        generation: u64,
    },
    Failed {
        generation: u64,
        failure: AttachFailure,
    },
    Detached {
        generation: u64,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttachFailure {
    Gap,
    PermissionDenied,
    Incompatible,
    ResourceLimit,
    ProtocolViolation,
    Unavailable,
}

impl AttachFailure {
    pub const fn message(self) -> &'static str {
        match self {
            Self::Gap => "Output history has a sequence gap; retry the attachment.",
            Self::PermissionDenied => "The local Host rejected this client identity.",
            Self::Incompatible => "The local Host protocol is incompatible; update TermiRust.",
            Self::ResourceLimit => "The Host response exceeded a bounded terminal limit.",
            Self::ProtocolViolation => "The local Host returned inconsistent terminal state.",
            Self::Unavailable => "The durable Host is unavailable; press r to retry.",
        }
    }
}

#[derive(Debug)]
pub struct AttachedTerminal {
    generation: u64,
    session_id: HostedSessionId,
    title: String,
    state: TuiAttachState,
    lifecycle: HostLifecycle,
    host_instance_id: Option<HostInstanceId>,
    watermark: OutputSequence,
    durable_sequence: OutputSequence,
    latest_sequence: OutputSequence,
    replay_records: u64,
    replay_bytes: u64,
    recording_paused: bool,
    failure: Option<AttachFailure>,
    diagnostic: Option<&'static str>,
    viewport: Viewport,
    terminal: TerminalView,
    input: TerminalInputModel,
}

impl AttachedTerminal {
    pub fn new(
        generation: u64,
        session_id: HostedSessionId,
        title: String,
        viewport: Viewport,
    ) -> Self {
        Self {
            generation,
            session_id,
            title,
            state: TuiAttachState::Attaching,
            lifecycle: HostLifecycle::Unknown,
            host_instance_id: None,
            watermark: OutputSequence::ZERO,
            durable_sequence: OutputSequence::ZERO,
            latest_sequence: OutputSequence::ZERO,
            replay_records: 0,
            replay_bytes: 0,
            recording_paused: false,
            failure: None,
            diagnostic: None,
            viewport,
            terminal: TerminalView::new(viewport.columns, viewport.rows),
            input: TerminalInputModel::default(),
        }
    }

    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub const fn session_id(&self) -> HostedSessionId {
        self.session_id
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub const fn state(&self) -> TuiAttachState {
        self.state
    }

    pub const fn lifecycle(&self) -> HostLifecycle {
        self.lifecycle
    }

    pub const fn watermark(&self) -> OutputSequence {
        self.watermark
    }

    pub const fn durable_sequence(&self) -> OutputSequence {
        self.durable_sequence
    }

    pub const fn latest_sequence(&self) -> OutputSequence {
        self.latest_sequence
    }

    pub const fn replay_records(&self) -> u64 {
        self.replay_records
    }

    pub const fn replay_bytes(&self) -> u64 {
        self.replay_bytes
    }

    pub const fn recording_paused(&self) -> bool {
        self.recording_paused
    }

    pub const fn failure(&self) -> Option<AttachFailure> {
        self.failure
    }

    pub const fn diagnostic(&self) -> Option<&'static str> {
        self.diagnostic
    }

    pub const fn viewport(&self) -> Viewport {
        self.viewport
    }

    pub fn terminal(&self) -> &TerminalView {
        &self.terminal
    }

    pub fn input(&self) -> &TerminalInputModel {
        &self.input
    }

    pub fn input_mut(&mut self) -> &mut TerminalInputModel {
        &mut self.input
    }

    pub fn apply(&mut self, event: AttachEvent) -> bool {
        let event_generation = match &event {
            AttachEvent::Attaching { generation }
            | AttachEvent::Batch { generation, .. }
            | AttachEvent::LeaseChanged { generation, .. }
            | AttachEvent::InputRejected { generation }
            | AttachEvent::Failed { generation, .. }
            | AttachEvent::Detached { generation } => *generation,
        };
        if event_generation != self.generation {
            return false;
        }
        match event {
            AttachEvent::Attaching { .. } => {
                self.state = TuiAttachState::Attaching;
                self.failure = None;
                self.diagnostic = None;
            }
            AttachEvent::Batch { batch, .. } => self.apply_batch(batch),
            AttachEvent::LeaseChanged { held, .. } => {
                self.input.set_lease(held);
                self.state = if held {
                    TuiAttachState::LiveInteractive
                } else {
                    TuiAttachState::LiveReadOnly
                };
            }
            AttachEvent::InputRejected { .. } => {
                self.input.set_lease(false);
                self.state = TuiAttachState::LiveReadOnly;
                self.diagnostic = Some("Input lease was lost; no offline input was retained.");
            }
            AttachEvent::Failed { failure, .. } => {
                self.input.set_lease(false);
                self.state = if failure == AttachFailure::Gap {
                    TuiAttachState::Gap
                } else {
                    TuiAttachState::Unavailable
                };
                self.failure = Some(failure);
                self.diagnostic = Some(failure.message());
            }
            AttachEvent::Detached { .. } => {
                self.input.set_lease(false);
                self.state = TuiAttachState::Detached;
            }
        }
        true
    }

    pub fn resize(&mut self, viewport: Viewport) -> Viewport {
        let (columns, rows) = self.terminal.resize(viewport.columns, viewport.rows);
        self.viewport = Viewport::new(columns, rows);
        self.viewport
    }

    fn apply_batch(&mut self, batch: AttachBatch) {
        if self
            .host_instance_id
            .is_some_and(|expected| expected != batch.host_instance_id)
        {
            self.input.set_lease(false);
            self.state = TuiAttachState::Unavailable;
            self.failure = Some(AttachFailure::ProtocolViolation);
            self.diagnostic = Some("The durable Host generation changed; retry from the fleet.");
            return;
        }
        self.host_instance_id = Some(batch.host_instance_id);
        self.state = TuiAttachState::Replaying;
        if let Some(snapshot) = batch.snapshot {
            if snapshot.boundary < self.watermark {
                self.fail_gap();
                return;
            }
            self.terminal.reset(
                self.viewport.columns,
                self.viewport.rows,
                &snapshot.terminal_bytes,
            );
            self.watermark = snapshot.boundary;
            self.replay_bytes = self
                .replay_bytes
                .saturating_add(snapshot.terminal_bytes.len() as u64);
        }
        for output in batch.outputs {
            if output.sequence <= self.watermark {
                continue;
            }
            if self.watermark.checked_next() != Some(output.sequence) {
                self.fail_gap();
                return;
            }
            self.terminal.process(&output.bytes);
            self.watermark = output.sequence;
            self.replay_records = self.replay_records.saturating_add(1);
            self.replay_bytes = self.replay_bytes.saturating_add(output.bytes.len() as u64);
        }
        self.lifecycle = batch.state.lifecycle;
        if batch.state.latest_sequence < self.watermark
            || batch.state.durable_sequence > batch.state.latest_sequence
        {
            self.input.set_lease(false);
            self.state = TuiAttachState::Unavailable;
            self.failure = Some(AttachFailure::ProtocolViolation);
            self.diagnostic = Some(AttachFailure::ProtocolViolation.message());
            return;
        }
        self.latest_sequence = batch.state.latest_sequence;
        self.durable_sequence = batch.state.durable_sequence;
        self.recording_paused = batch.state.recording_paused;
        self.input.set_lease(batch.state.has_writer_lease);
        self.failure = None;
        self.diagnostic = None;
        self.state = if batch.state.lifecycle.terminal() {
            self.input.set_lease(false);
            TuiAttachState::Exited
        } else if batch.state.has_writer_lease {
            TuiAttachState::LiveInteractive
        } else {
            TuiAttachState::LiveReadOnly
        };
    }

    fn fail_gap(&mut self) {
        self.input.set_lease(false);
        self.state = TuiAttachState::Gap;
        self.failure = Some(AttachFailure::Gap);
        self.diagnostic = Some(AttachFailure::Gap.message());
    }
}

#[derive(Clone, Debug)]
pub enum AttachCommand {
    Input(Vec<u8>),
    Resize(Viewport),
    RequestLease,
    Detach,
}

pub struct AttachWorker {
    command_tx: mpsc::Sender<AttachCommand>,
    cancellation: CancellationToken,
    join: Option<thread::JoinHandle<()>>,
}

impl AttachWorker {
    pub fn try_send(&self, command: AttachCommand) -> bool {
        self.command_tx.try_send(command).is_ok()
    }

    pub fn cancel(&self) {
        self.cancellation.cancel();
    }
}

impl Drop for AttachWorker {
    fn drop(&mut self) {
        self.cancellation.cancel();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

pub type AttachEventSink = Arc<dyn Fn(AttachEvent) -> bool + Send + Sync>;

pub fn spawn_attach_worker(
    generation: u64,
    endpoint: LocalEndpoint,
    session_id: HostedSessionId,
    viewport: Viewport,
    sink: AttachEventSink,
) -> std::io::Result<AttachWorker> {
    let (command_tx, command_rx) = mpsc::channel(COMMAND_QUEUE_CAPACITY);
    let cancellation = CancellationToken::new();
    let worker_cancel = cancellation.clone();
    let join = thread::Builder::new()
        .name("termirust-tui-attach".into())
        .spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| AttachFailure::Unavailable)
                .and_then(|runtime| {
                    runtime.block_on(run_worker(
                        generation,
                        endpoint,
                        session_id,
                        viewport,
                        command_rx,
                        &worker_cancel,
                        &sink,
                    ))
                });
            if let Err(failure) = result
                && !worker_cancel.is_cancelled()
            {
                let _ = sink(AttachEvent::Failed {
                    generation,
                    failure,
                });
            }
        })?;
    Ok(AttachWorker {
        command_tx,
        cancellation,
        join: Some(join),
    })
}

async fn run_worker(
    generation: u64,
    endpoint: LocalEndpoint,
    session_id: HostedSessionId,
    mut viewport: Viewport,
    mut command_rx: mpsc::Receiver<AttachCommand>,
    cancel: &CancellationToken,
    sink: &AttachEventSink,
) -> Result<(), AttachFailure> {
    emit(sink, AttachEvent::Attaching { generation })?;
    let mut nonce = [0_u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let mut client =
        HostClient::connect(endpoint, ConnectOptions::local(session_id, nonce), cancel)
            .await
            .map_err(map_client_error)?;
    let host_instance_id = client
        .host_instance_id()
        .ok_or(AttachFailure::Unavailable)?;
    let mut attach = GpuiAttachModel::new(OutputSequence::ZERO);
    let mut ticker = tokio::time::interval(LIVE_POLL_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                client.disconnect();
                return Ok(());
            }
            command = command_rx.recv() => match command {
                Some(AttachCommand::Input(bytes)) => {
                    if !attach.has_writer_lease() {
                        emit(sink, AttachEvent::InputRejected { generation })?;
                        continue;
                    }
                    if client.input(CommandId::new(), bytes, cancel).await.is_err() {
                        attach.mark_live(false, false);
                        emit(sink, AttachEvent::InputRejected { generation })?;
                    }
                }
                Some(AttachCommand::Resize(next)) => {
                    viewport = next;
                    if attach.has_writer_lease()
                        && client.resize(
                            CommandId::new(),
                            u32::from(next.columns),
                            u32::from(next.rows),
                            cancel,
                        ).await.is_err()
                    {
                        attach.mark_live(false, false);
                        emit(sink, AttachEvent::InputRejected { generation })?;
                    }
                }
                Some(AttachCommand::RequestLease) => {
                    match client.set_writer_lease(CommandId::new(), true, cancel).await {
                        Ok(held) => {
                            attach.mark_live(held, false);
                            emit(sink, AttachEvent::LeaseChanged { generation, held })?;
                        }
                        Err(_) => emit(sink, AttachEvent::LeaseChanged { generation, held: false })?,
                    }
                }
                Some(AttachCommand::Detach) | None => {
                    let _ = client.detach(cancel).await;
                    let _ = sink(AttachEvent::Detached { generation });
                    return Ok(());
                }
            },
            _ = ticker.tick() => {
                attach.begin_replay();
                let outputs = tokio::time::timeout(
                    ATTACH_TIMEOUT,
                    client.attach(
                        attach.watermark(),
                        u32::from(viewport.columns),
                        u32::from(viewport.rows),
                        cancel,
                    ),
                )
                .await
                .map_err(|_| AttachFailure::Unavailable)?
                .map_err(map_client_error)?;
                let snapshot = client.take_last_snapshot().map(|snapshot| AttachSnapshot {
                    boundary: OutputSequence::new(snapshot.boundary_sequence),
                    terminal_bytes: snapshot.terminal_bytes,
                });
                if let Some(snapshot) = &snapshot
                    && !attach.apply_snapshot(snapshot.boundary)
                {
                    return Err(AttachFailure::Gap);
                }
                for output in &outputs {
                    if matches!(
                        attach.observe_output(output.sequence, output.bytes.len()),
                        OutputDisposition::Gap { .. }
                    ) {
                        return Err(AttachFailure::Gap);
                    }
                }
                let state = client
                    .take_last_state()
                    .ok_or(AttachFailure::Unavailable)?;
                let host_state = map_host_state(state);
                attach.mark_live(host_state.has_writer_lease, host_state.recording_paused);
                let terminal = host_state.lifecycle.terminal();
                emit(sink, AttachEvent::Batch {
                    generation,
                    batch: AttachBatch {
                        host_instance_id,
                        snapshot,
                        outputs,
                        state: host_state,
                    },
                })?;
                if terminal {
                    client.disconnect();
                    return Ok(());
                }
            }
        }
    }
}

fn map_host_state(state: wire::StateEvent) -> HostAttachState {
    let lifecycle =
        match wire::Lifecycle::try_from(state.lifecycle).unwrap_or(wire::Lifecycle::Unspecified) {
            wire::Lifecycle::Starting => HostLifecycle::Starting,
            wire::Lifecycle::Ready => HostLifecycle::Ready,
            wire::Lifecycle::Stopping => HostLifecycle::Stopping,
            wire::Lifecycle::Exited => HostLifecycle::Exited,
            wire::Lifecycle::Failed => HostLifecycle::Failed,
            wire::Lifecycle::Unspecified => HostLifecycle::Unknown,
        };
    HostAttachState {
        lifecycle,
        earliest_sequence: OutputSequence::new(state.earliest_sequence),
        latest_sequence: OutputSequence::new(state.latest_sequence),
        durable_sequence: OutputSequence::new(state.durable_sequence),
        has_writer_lease: state.has_writer_lease,
        recording_paused: state.recording_paused,
    }
}

fn map_client_error(error: ClientError) -> AttachFailure {
    match error.code {
        ClientErrorCode::SequenceGap => AttachFailure::Gap,
        ClientErrorCode::PermissionDenied
        | ClientErrorCode::WrongSession
        | ClientErrorCode::InvalidIdentity
        | ClientErrorCode::HandshakeReplay => AttachFailure::PermissionDenied,
        ClientErrorCode::ProtocolIncompatible => AttachFailure::Incompatible,
        ClientErrorCode::FrameTooLarge | ClientErrorCode::ResourceLimit => {
            AttachFailure::ResourceLimit
        }
        ClientErrorCode::MalformedFrame | ClientErrorCode::ChecksumMismatch => {
            AttachFailure::ProtocolViolation
        }
        ClientErrorCode::Io
        | ClientErrorCode::EndOfStream
        | ClientErrorCode::ConflictingDuplicate
        | ClientErrorCode::Cancelled
        | ClientErrorCode::InvalidState => AttachFailure::Unavailable,
    }
}

fn emit(sink: &AttachEventSink, event: AttachEvent) -> Result<(), AttachFailure> {
    if sink(event) {
        Ok(())
    } else {
        Err(AttachFailure::Unavailable)
    }
}

pub fn endpoint_for_source(
    source: &dyn crate::source::FleetSource,
    session_id: HostedSessionId,
) -> Result<LocalEndpoint, AttachFailure> {
    source
        .local_endpoint(session_id)
        .ok_or(AttachFailure::Unavailable)
}

#[cfg(test)]
mod tests {
    use termirust_domain::HostInstanceId;

    use super::*;
    use crate::input::InteractiveLease;

    fn batch(host: HostInstanceId, sequence: u64, held: bool) -> AttachEvent {
        AttachEvent::Batch {
            generation: 2,
            batch: AttachBatch {
                host_instance_id: host,
                snapshot: None,
                outputs: vec![SequencedOutput {
                    sequence: OutputSequence::new(sequence),
                    bytes: b"ok".to_vec(),
                }],
                state: HostAttachState {
                    lifecycle: HostLifecycle::Ready,
                    earliest_sequence: OutputSequence::new(1),
                    latest_sequence: OutputSequence::new(sequence),
                    durable_sequence: OutputSequence::new(sequence),
                    has_writer_lease: held,
                    recording_paused: false,
                },
            },
        }
    }

    #[test]
    fn attach_model_orders_output_and_rejects_stale_generation_and_host() {
        let session = HostedSessionId::new();
        let host = HostInstanceId::new();
        let mut model = AttachedTerminal::new(2, session, "Build".into(), Viewport::new(80, 20));
        assert!(!model.apply(AttachEvent::Attaching { generation: 1 }));
        assert!(model.apply(batch(host, 1, true)));
        assert_eq!(model.state(), TuiAttachState::LiveInteractive);
        assert_eq!(model.watermark(), OutputSequence::new(1));
        assert_eq!(model.terminal().contents(), "ok");

        assert!(model.apply(batch(HostInstanceId::new(), 2, true)));
        assert_eq!(model.state(), TuiAttachState::Unavailable);
        assert_eq!(model.input().lease(), InteractiveLease::ViewOnly);
    }

    #[test]
    fn gap_state_disables_writes() {
        let session = HostedSessionId::new();
        let host = HostInstanceId::new();
        let mut model = AttachedTerminal::new(2, session, "Build".into(), Viewport::new(80, 20));
        assert!(model.apply(batch(host, 2, true)));
        assert_eq!(model.state(), TuiAttachState::Gap);
        assert_eq!(model.input().lease(), InteractiveLease::ViewOnly);
    }

    #[test]
    fn exited_state_disables_writes_without_discarding_screen() {
        let session = HostedSessionId::new();
        let host = HostInstanceId::new();
        let mut model = AttachedTerminal::new(2, session, "Build".into(), Viewport::new(80, 20));
        assert!(model.apply(batch(host, 1, true)));
        let contents = model.terminal().contents();
        assert!(model.apply(AttachEvent::Batch {
            generation: 2,
            batch: AttachBatch {
                host_instance_id: host,
                snapshot: None,
                outputs: Vec::new(),
                state: HostAttachState {
                    lifecycle: HostLifecycle::Exited,
                    earliest_sequence: OutputSequence::new(1),
                    latest_sequence: OutputSequence::new(1),
                    durable_sequence: OutputSequence::new(1),
                    has_writer_lease: false,
                    recording_paused: false,
                },
            },
        }));
        assert_eq!(model.state(), TuiAttachState::Exited);
        assert_eq!(model.input().lease(), InteractiveLease::ViewOnly);
        assert_eq!(model.terminal().contents(), contents);
    }
}
