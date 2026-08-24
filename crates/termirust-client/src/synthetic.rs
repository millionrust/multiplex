use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use termirust_domain::{HostInstanceId, HostedSessionId, OutputSequence};
use termirust_host_protocol::wire::{self, envelope_payload};
use termirust_host_protocol::{
    CURRENT_PROTOCOL, CapabilitySet, HANDSHAKE_NONCE_BYTES, MAX_OUTBOUND_FRAMES, MAX_OUTPUT_BYTES,
    MAX_REPLAY_BYTES, MAX_REPLAY_RECORDS, NegotiatedLimits, PreservedPayload, ProtocolRange,
    ProtocolVersion, WireEnvelope, decode_command_id, decode_session_id, encode_command_id,
    encode_host_instance_id, encode_payload, encode_session_id, local_limits, negotiate_protocol,
    payload_kind,
};
use tokio::sync::{Mutex, Semaphore};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{Duration, timeout};
use tokio_util::sync::CancellationToken;

#[cfg(unix)]
use tokio::net::UnixStream;

use crate::{
    AsyncEnvelopeStream, ClientError, ClientErrorCode, IdempotencyCache, IdempotencyDecision,
    LocalEndpoint, UserOnlyUnixListener,
};

const MAX_CONNECTIONS: usize = 32;
const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(2);

#[derive(Clone, Debug)]
pub struct SyntheticHostConfig {
    pub session_id: HostedSessionId,
    pub host_instance_id: HostInstanceId,
    pub protocol: ProtocolRange,
    pub capabilities: CapabilitySet,
    pub limits: NegotiatedLimits,
    pub host_nonce: [u8; HANDSHAKE_NONCE_BYTES],
    pub first_output_sequence: OutputSequence,
    pub output: Vec<Vec<u8>>,
}

impl SyntheticHostConfig {
    pub fn local(
        session_id: HostedSessionId,
        host_instance_id: HostInstanceId,
        host_nonce: [u8; HANDSHAKE_NONCE_BYTES],
    ) -> Self {
        Self {
            session_id,
            host_instance_id,
            protocol: CURRENT_PROTOCOL,
            capabilities: CapabilitySet::all_local(),
            limits: local_limits(),
            host_nonce,
            first_output_sequence: OutputSequence::new(1),
            output: Vec::new(),
        }
    }

    fn validate(&self) -> Result<(), ClientError> {
        if !self.protocol.is_valid()
            || !self.limits.is_valid()
            || self.first_output_sequence == OutputSequence::ZERO
            || self.output.len() > MAX_REPLAY_RECORDS as usize
            || self
                .output
                .iter()
                .any(|bytes| bytes.len() > MAX_OUTPUT_BYTES)
            || self
                .output
                .iter()
                .try_fold(0_u64, |total, bytes| total.checked_add(bytes.len() as u64))
                .is_none_or(|total| total > MAX_REPLAY_BYTES)
        {
            return Err(ClientError::new(ClientErrorCode::ResourceLimit));
        }
        self.first_output_sequence
            .checked_add(self.output.len() as u64)
            .ok_or_else(|| ClientError::new(ClientErrorCode::ResourceLimit))?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SyntheticHostStats {
    pub applied_mutations: u64,
    pub cached_outcomes: usize,
    pub active_connections: usize,
    pub recent_bad_peers: usize,
}

struct SyntheticState {
    idempotency: IdempotencyCache,
    applied_mutations: u64,
    active_connections: usize,
    bad_peers: BadPeerLimiter,
    handshake_nonces: HandshakeNonceCache,
}

impl Default for SyntheticState {
    fn default() -> Self {
        Self {
            idempotency: IdempotencyCache::default(),
            applied_mutations: 0,
            active_connections: 0,
            bad_peers: BadPeerLimiter::new(32, Duration::from_secs(60)),
            handshake_nonces: HandshakeNonceCache::default(),
        }
    }
}

pub struct SyntheticHostHandle {
    endpoint: LocalEndpoint,
    cancel: CancellationToken,
    task: Option<JoinHandle<Result<(), ClientError>>>,
    state: Arc<Mutex<SyntheticState>>,
}

impl SyntheticHostHandle {
    pub fn endpoint(&self) -> &LocalEndpoint {
        &self.endpoint
    }

    pub async fn stats(&self) -> SyntheticHostStats {
        let mut state = self.state.lock().await;
        SyntheticHostStats {
            applied_mutations: state.applied_mutations,
            cached_outcomes: state.idempotency.len(),
            active_connections: state.active_connections,
            recent_bad_peers: state.bad_peers.len_at(Instant::now()),
        }
    }

    pub async fn shutdown(mut self) -> Result<(), ClientError> {
        self.cancel.cancel();
        let Some(task) = self.task.take() else {
            return Ok(());
        };
        timeout(SHUTDOWN_DEADLINE, task)
            .await
            .map_err(|_| ClientError::new(ClientErrorCode::Cancelled))?
            .map_err(|_| ClientError::new(ClientErrorCode::InvalidState))?
    }
}

impl Drop for SyntheticHostHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

#[cfg(unix)]
pub async fn start(
    endpoint: LocalEndpoint,
    config: SyntheticHostConfig,
) -> Result<SyntheticHostHandle, ClientError> {
    config.validate()?;
    let listener = UserOnlyUnixListener::bind(endpoint.clone())?;
    let cancel = CancellationToken::new();
    let state = Arc::new(Mutex::new(SyntheticState::default()));
    let task = tokio::spawn(run_server(listener, config, state.clone(), cancel.clone()));
    Ok(SyntheticHostHandle {
        endpoint,
        cancel,
        task: Some(task),
        state,
    })
}

#[cfg(not(unix))]
pub async fn start(
    _: LocalEndpoint,
    _: SyntheticHostConfig,
) -> Result<SyntheticHostHandle, ClientError> {
    Err(ClientError::new(ClientErrorCode::PermissionDenied))
}

#[cfg(unix)]
async fn run_server(
    listener: UserOnlyUnixListener,
    config: SyntheticHostConfig,
    state: Arc<Mutex<SyntheticState>>,
    cancel: CancellationToken,
) -> Result<(), ClientError> {
    let permits = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    let mut handlers = JoinSet::new();
    loop {
        while let Some(result) = handlers.try_join_next() {
            result.map_err(|_| ClientError::new(ClientErrorCode::InvalidState))??;
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            accepted = listener.accept() => {
                let stream = accepted?;
                if !state.lock().await.bad_peers.allows(Instant::now()) {
                    drop(stream);
                    continue;
                }
                let Ok(permit) = permits.clone().try_acquire_owned() else {
                    drop(stream);
                    continue;
                };
                let config = config.clone();
                let state = state.clone();
                let child_cancel = cancel.child_token();
                handlers.spawn(async move {
                    {
                        let mut shared = state.lock().await;
                        shared.active_connections += 1;
                    }
                    let result = serve_connection(stream, config, state.clone(), child_cancel).await;
                    {
                        let mut shared = state.lock().await;
                        shared.active_connections = shared.active_connections.saturating_sub(1);
                    }
                    drop(permit);
                    if let Err(error) = result {
                        if matches!(error.code, ClientErrorCode::MalformedFrame | ClientErrorCode::ChecksumMismatch | ClientErrorCode::FrameTooLarge | ClientErrorCode::WrongSession | ClientErrorCode::InvalidIdentity) {
                            state.lock().await.bad_peers.record(Instant::now());
                        }
                        match error.code {
                            ClientErrorCode::EndOfStream | ClientErrorCode::Cancelled | ClientErrorCode::MalformedFrame | ClientErrorCode::ChecksumMismatch | ClientErrorCode::FrameTooLarge | ClientErrorCode::WrongSession | ClientErrorCode::InvalidIdentity => Ok(()),
                            _ => Err(error),
                        }
                    } else {
                        Ok(())
                    }
                });
            }
        }
    }
    cancel.cancel();
    while let Some(result) = handlers.join_next().await {
        result.map_err(|_| ClientError::new(ClientErrorCode::InvalidState))??;
    }
    Ok(())
}

#[cfg(unix)]
async fn serve_connection(
    stream: UnixStream,
    config: SyntheticHostConfig,
    state: Arc<Mutex<SyntheticState>>,
    cancel: CancellationToken,
) -> Result<(), ClientError> {
    let mut stream = AsyncEnvelopeStream::new(stream);
    let handshake_envelope = match stream.read(&cancel).await {
        Ok(envelope) => envelope,
        Err(error) => {
            state.lock().await.bad_peers.record(Instant::now());
            send_transport_error(&mut stream, &config, error, &cancel).await?;
            return Ok(());
        }
    };
    let request_id = handshake_envelope.request_id;
    let handshake = match decode_checked(&handshake_envelope)?.message {
        Some(envelope_payload::Message::HandshakeRequest(handshake)) => handshake,
        _ => {
            send_error(
                &mut stream,
                request_id,
                wire::ErrorCode::InvalidState,
                wire::RecoveryHint::Reconnect,
                config.protocol,
                0,
                &cancel,
            )
            .await?;
            return Ok(());
        }
    };
    if decode_session_id(&handshake.session_id)? != config.session_id {
        state.lock().await.bad_peers.record(Instant::now());
        send_error(
            &mut stream,
            request_id,
            wire::ErrorCode::WrongSession,
            wire::RecoveryHint::Reauthorize,
            config.protocol,
            0,
            &cancel,
        )
        .await?;
        return Ok(());
    }
    if handshake.client_nonce.len() != HANDSHAKE_NONCE_BYTES {
        state.lock().await.bad_peers.record(Instant::now());
        send_error(
            &mut stream,
            request_id,
            wire::ErrorCode::MalformedFrame,
            wire::RecoveryHint::Reconnect,
            config.protocol,
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
            .ok_or_else(|| ClientError::new(ClientErrorCode::MalformedFrame))?,
    )?;
    let Some(selected) = negotiate_protocol(config.protocol, peer_protocol) else {
        send_error(
            &mut stream,
            request_id,
            wire::ErrorCode::ProtocolIncompatible,
            wire::RecoveryHint::Upgrade,
            config.protocol,
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
            .ok_or_else(|| ClientError::new(ClientErrorCode::MalformedFrame))?,
    )?;
    let limits = config.limits.bounded_with(peer_limits);
    let client_nonce: [u8; HANDSHAKE_NONCE_BYTES] = handshake
        .client_nonce
        .as_slice()
        .try_into()
        .map_err(|_| ClientError::new(ClientErrorCode::MalformedFrame))?;
    if !state
        .lock()
        .await
        .handshake_nonces
        .accept(client_nonce, Instant::now())
    {
        state.lock().await.bad_peers.record(Instant::now());
        send_error(
            &mut stream,
            request_id,
            wire::ErrorCode::HandshakeReplay,
            wire::RecoveryHint::Reconnect,
            config.protocol,
            0,
            &cancel,
        )
        .await?;
        return Ok(());
    }
    let capabilities = config
        .capabilities
        .intersection(&CapabilitySet::from_wire(&handshake.capabilities));
    send_message(
        &mut stream,
        request_id,
        selected,
        envelope_payload::Message::HandshakeResponse(wire::HandshakeResponse {
            host_instance_id: encode_host_instance_id(config.host_instance_id),
            session_id: encode_session_id(config.session_id),
            selected_version: Some(selected.into()),
            capabilities: capabilities.to_wire(),
            limits: Some(limits.into()),
            host_nonce: config.host_nonce.to_vec(),
            client_nonce_echo: handshake.client_nonce,
        }),
        &cancel,
    )
    .await?;

    loop {
        let envelope = match stream.read(&cancel).await {
            Ok(envelope) => envelope,
            Err(error) => {
                state.lock().await.bad_peers.record(Instant::now());
                send_transport_error(&mut stream, &config, error, &cancel).await?;
                return Ok(());
            }
        };
        if envelope.protocol_major != selected.major || envelope.protocol_minor != selected.minor {
            send_error(
                &mut stream,
                envelope.request_id,
                wire::ErrorCode::ProtocolIncompatible,
                wire::RecoveryHint::Reconnect,
                config.protocol,
                0,
                &cancel,
            )
            .await?;
            return Ok(());
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
                config.protocol,
                0,
                &cancel,
            )
            .await?;
            return Ok(());
        }
        match payload.message {
            Some(envelope_payload::Message::GetStateRequest(request)) => {
                require_session(&request.session_id, config.session_id)?;
                send_state(&mut stream, envelope.request_id, selected, &config, &cancel).await?;
            }
            Some(envelope_payload::Message::AttachRequest(request)) => {
                require_session(&request.session_id, config.session_id)?;
                serve_attach(
                    &mut stream,
                    envelope.request_id,
                    selected,
                    &config,
                    request,
                    &cancel,
                )
                .await?;
            }
            Some(envelope_payload::Message::InputRequest(request)) => {
                require_session(&request.session_id, config.session_id)?;
                if request.bytes.len() > limits.maximum_output_bytes {
                    send_error(
                        &mut stream,
                        envelope.request_id,
                        wire::ErrorCode::ResourceLimit,
                        wire::RecoveryHint::RetryExplicitly,
                        config.protocol,
                        0,
                        &cancel,
                    )
                    .await?;
                    return Ok(());
                }
                if handle_mutation(
                    &mut stream,
                    envelope.request_id,
                    selected,
                    &state,
                    &request.command_id,
                    &envelope.payload,
                    &cancel,
                )
                .await?
                {
                    return Ok(());
                }
            }
            Some(envelope_payload::Message::ResizeRequest(request)) => {
                require_session(&request.session_id, config.session_id)?;
                if request
                    .viewport
                    .as_ref()
                    .is_none_or(|viewport| viewport.columns == 0 || viewport.rows == 0)
                {
                    send_error(
                        &mut stream,
                        envelope.request_id,
                        wire::ErrorCode::MalformedFrame,
                        wire::RecoveryHint::RetryExplicitly,
                        config.protocol,
                        0,
                        &cancel,
                    )
                    .await?;
                    return Ok(());
                }
                if handle_mutation(
                    &mut stream,
                    envelope.request_id,
                    selected,
                    &state,
                    &request.command_id,
                    &envelope.payload,
                    &cancel,
                )
                .await?
                {
                    return Ok(());
                }
            }
            Some(envelope_payload::Message::StopRequest(request)) => {
                require_session(&request.session_id, config.session_id)?;
                if wire::StopMode::try_from(request.mode).unwrap_or(wire::StopMode::Unspecified)
                    == wire::StopMode::Unspecified
                {
                    send_error(
                        &mut stream,
                        envelope.request_id,
                        wire::ErrorCode::MalformedFrame,
                        wire::RecoveryHint::RetryExplicitly,
                        config.protocol,
                        0,
                        &cancel,
                    )
                    .await?;
                    return Ok(());
                }
                if handle_mutation(
                    &mut stream,
                    envelope.request_id,
                    selected,
                    &state,
                    &request.command_id,
                    &envelope.payload,
                    &cancel,
                )
                .await?
                {
                    return Ok(());
                }
            }
            Some(envelope_payload::Message::InterruptRequest(request)) => {
                require_session(&request.session_id, config.session_id)?;
                if handle_mutation(
                    &mut stream,
                    envelope.request_id,
                    selected,
                    &state,
                    &request.command_id,
                    &envelope.payload,
                    &cancel,
                )
                .await?
                {
                    return Ok(());
                }
            }
            Some(envelope_payload::Message::ActivitySnapshotRequest(request)) => {
                require_session(&request.session_id, config.session_id)?;
                send_message(
                    &mut stream,
                    envelope.request_id,
                    selected,
                    envelope_payload::Message::ActivityEvent(wire::ActivityEvent {
                        session_id: encode_session_id(config.session_id),
                        sequence: latest_sequence(&config)?.get(),
                        activity: i32::from(wire::Activity::Unknown),
                    }),
                    &cancel,
                )
                .await?;
            }
            Some(envelope_payload::Message::DetachRequest(request)) => {
                require_session(&request.session_id, config.session_id)?;
                return Ok(());
            }
            _ => {
                send_error(
                    &mut stream,
                    envelope.request_id,
                    wire::ErrorCode::InvalidState,
                    wire::RecoveryHint::Reconnect,
                    config.protocol,
                    0,
                    &cancel,
                )
                .await?;
                return Ok(());
            }
        }
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

#[cfg(unix)]
async fn serve_attach(
    stream: &mut AsyncEnvelopeStream<UnixStream>,
    request_id: [u8; 16],
    version: ProtocolVersion,
    config: &SyntheticHostConfig,
    request: wire::AttachRequest,
    cancel: &CancellationToken,
) -> Result<(), ClientError> {
    if request
        .viewport
        .as_ref()
        .is_none_or(|viewport| viewport.columns == 0 || viewport.rows == 0)
        || request.maximum_replay_bytes == 0
        || request.maximum_replay_bytes > MAX_REPLAY_BYTES
        || request.maximum_replay_records == 0
        || request.maximum_replay_records > MAX_REPLAY_RECORDS
    {
        return Err(ClientError::new(ClientErrorCode::ResourceLimit));
    }
    let earliest = config.first_output_sequence;
    let from_sequence = OutputSequence::new(request.from_sequence);
    if from_sequence
        .checked_next()
        .is_some_and(|expected| expected < earliest)
    {
        send_message(
            stream,
            request_id,
            version,
            envelope_payload::Message::GapEvent(wire::GapEvent {
                session_id: encode_session_id(config.session_id),
                expected_sequence: request.from_sequence.saturating_add(1),
                actual_sequence: earliest.get(),
                earliest_available_sequence: earliest.get(),
            }),
            cancel,
        )
        .await?;
        return Ok(());
    }
    let latest = latest_sequence(config)?;
    send_message(
        stream,
        request_id,
        version,
        envelope_payload::Message::ReadyEvent(wire::ReadyEvent {
            session_id: encode_session_id(config.session_id),
            latest_sequence: latest.get(),
        }),
        cancel,
    )
    .await?;
    let mut sent_bytes = 0_u64;
    let mut sent_records = 0_u32;
    for (index, bytes) in config.output.iter().enumerate() {
        let sequence = config
            .first_output_sequence
            .checked_add(index as u64)
            .ok_or_else(|| ClientError::new(ClientErrorCode::ResourceLimit))?;
        if sequence <= from_sequence {
            continue;
        }
        sent_bytes = sent_bytes
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| ClientError::new(ClientErrorCode::ResourceLimit))?;
        sent_records = sent_records.saturating_add(1);
        if sent_bytes > request.maximum_replay_bytes
            || sent_records > request.maximum_replay_records
        {
            send_message(
                stream,
                request_id,
                version,
                envelope_payload::Message::GapEvent(wire::GapEvent {
                    session_id: encode_session_id(config.session_id),
                    expected_sequence: sequence.get(),
                    actual_sequence: sequence.get(),
                    earliest_available_sequence: earliest.get(),
                }),
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
                session_id: encode_session_id(config.session_id),
                sequence: sequence.get(),
                bytes: bytes.clone(),
            }),
            cancel,
        )
        .await?;
    }
    send_state(stream, request_id, version, config, cancel).await
}

#[cfg(unix)]
async fn handle_mutation(
    stream: &mut AsyncEnvelopeStream<UnixStream>,
    request_id: [u8; 16],
    version: ProtocolVersion,
    state: &Arc<Mutex<SyntheticState>>,
    command_bytes: &[u8],
    payload: &[u8],
    cancel: &CancellationToken,
) -> Result<bool, ClientError> {
    let command_id = decode_command_id(command_bytes)?;
    if command_id.as_uuid().into_bytes() != request_id {
        return Err(ClientError::new(ClientErrorCode::MalformedFrame));
    }
    let payload_hash = crc32c::crc32c(payload);
    let decision = {
        let mut shared = state.lock().await;
        match shared
            .idempotency
            .inspect(command_id, payload_hash, Instant::now())
        {
            IdempotencyDecision::Apply => {
                shared.applied_mutations = shared.applied_mutations.saturating_add(1);
                shared
                    .idempotency
                    .record(command_id, payload_hash, true, Instant::now());
                IdempotencyDecision::Apply
            }
            IdempotencyDecision::Conflict => {
                shared.bad_peers.record(Instant::now());
                IdempotencyDecision::Conflict
            }
            other => other,
        }
    };
    match decision {
        IdempotencyDecision::Conflict => {
            send_error(
                stream,
                request_id,
                wire::ErrorCode::ConflictingDuplicate,
                wire::RecoveryHint::RetryExplicitly,
                CURRENT_PROTOCOL,
                0,
                cancel,
            )
            .await?;
            Ok(true)
        }
        IdempotencyDecision::Apply | IdempotencyDecision::Replay { applied: true } => {
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
            .await?;
            Ok(false)
        }
        IdempotencyDecision::Replay { applied: false } => {
            send_message(
                stream,
                request_id,
                version,
                envelope_payload::Message::CommandResult(wire::CommandResult {
                    command_id: encode_command_id(command_id),
                    applied: false,
                }),
                cancel,
            )
            .await?;
            Ok(false)
        }
    }
}

#[cfg(unix)]
async fn send_transport_error(
    stream: &mut AsyncEnvelopeStream<UnixStream>,
    config: &SyntheticHostConfig,
    error: ClientError,
    cancel: &CancellationToken,
) -> Result<(), ClientError> {
    let code = match error.code {
        ClientErrorCode::FrameTooLarge => wire::ErrorCode::FrameTooLarge,
        ClientErrorCode::ChecksumMismatch => wire::ErrorCode::ChecksumMismatch,
        ClientErrorCode::Cancelled | ClientErrorCode::EndOfStream => return Ok(()),
        _ => wire::ErrorCode::MalformedFrame,
    };
    send_error(
        stream,
        [0; 16],
        code,
        wire::RecoveryHint::Reconnect,
        config.protocol,
        0,
        cancel,
    )
    .await
}

#[cfg(unix)]
async fn send_state(
    stream: &mut AsyncEnvelopeStream<UnixStream>,
    request_id: [u8; 16],
    version: ProtocolVersion,
    config: &SyntheticHostConfig,
    cancel: &CancellationToken,
) -> Result<(), ClientError> {
    send_message(
        stream,
        request_id,
        version,
        envelope_payload::Message::StateEvent(wire::StateEvent {
            session_id: encode_session_id(config.session_id),
            host_instance_id: encode_host_instance_id(config.host_instance_id),
            lifecycle: i32::from(wire::Lifecycle::Ready),
            earliest_sequence: config.first_output_sequence.get(),
            latest_sequence: latest_sequence(config)?.get(),
            has_writer_lease: true,
        }),
        cancel,
    )
    .await
}

fn latest_sequence(config: &SyntheticHostConfig) -> Result<OutputSequence, ClientError> {
    if config.output.is_empty() {
        Ok(OutputSequence::new(
            config.first_output_sequence.get().saturating_sub(1),
        ))
    } else {
        config
            .first_output_sequence
            .checked_add(config.output.len() as u64 - 1)
            .ok_or_else(|| ClientError::new(ClientErrorCode::ResourceLimit))
    }
}

#[cfg(unix)]
async fn send_error(
    stream: &mut AsyncEnvelopeStream<UnixStream>,
    request_id: [u8; 16],
    code: wire::ErrorCode,
    recovery: wire::RecoveryHint,
    protocol: ProtocolRange,
    expected_sequence: u64,
    cancel: &CancellationToken,
) -> Result<(), ClientError> {
    send_message(
        stream,
        request_id,
        protocol.maximum,
        envelope_payload::Message::ProtocolError(wire::ProtocolError {
            code: i32::from(code),
            recovery: i32::from(recovery),
            supported_protocol: Some(protocol.into()),
            expected_sequence,
            earliest_available_sequence: 0,
        }),
        cancel,
    )
    .await
}

#[cfg(unix)]
async fn send_message(
    stream: &mut AsyncEnvelopeStream<UnixStream>,
    request_id: [u8; 16],
    version: ProtocolVersion,
    message: envelope_payload::Message,
    cancel: &CancellationToken,
) -> Result<(), ClientError> {
    let payload = wire::EnvelopePayload {
        message: Some(message),
    };
    let kind =
        payload_kind(&payload).ok_or_else(|| ClientError::new(ClientErrorCode::MalformedFrame))?;
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

fn decode_checked(envelope: &WireEnvelope) -> Result<wire::EnvelopePayload, ClientError> {
    let payload = PreservedPayload::decode(&envelope.payload)?.value;
    if payload_kind(&payload) != Some(envelope.kind) {
        return Err(ClientError::new(ClientErrorCode::MalformedFrame));
    }
    Ok(payload)
}

fn require_session(bytes: &[u8], expected: HostedSessionId) -> Result<(), ClientError> {
    if decode_session_id(bytes)? == expected {
        Ok(())
    } else {
        Err(ClientError::new(ClientErrorCode::WrongSession))
    }
}

#[derive(Debug)]
pub struct BoundedOutboundQueue {
    frames: VecDeque<WireEnvelope>,
    capacity: usize,
    gap_pending: bool,
}

#[derive(Debug)]
pub struct BadPeerLimiter {
    attempts: VecDeque<Instant>,
    maximum: usize,
    window: Duration,
}

#[derive(Debug)]
struct HandshakeNonceCache {
    entries: VecDeque<([u8; HANDSHAKE_NONCE_BYTES], Instant)>,
}

impl Default for HandshakeNonceCache {
    fn default() -> Self {
        Self {
            entries: VecDeque::with_capacity(1_024),
        }
    }
}

impl HandshakeNonceCache {
    fn accept(&mut self, nonce: [u8; HANDSHAKE_NONCE_BYTES], now: Instant) -> bool {
        let ttl = Duration::from_secs(10 * 60);
        while self
            .entries
            .front()
            .is_some_and(|(_, inserted)| now.saturating_duration_since(*inserted) >= ttl)
        {
            self.entries.pop_front();
        }
        if self.entries.iter().any(|(existing, _)| *existing == nonce) {
            return false;
        }
        if self.entries.len() == 1_024 {
            self.entries.pop_front();
        }
        self.entries.push_back((nonce, now));
        true
    }
}

impl BadPeerLimiter {
    pub fn new(maximum: usize, window: Duration) -> Self {
        Self {
            attempts: VecDeque::with_capacity(maximum),
            maximum: maximum.max(1),
            window,
        }
    }

    pub fn allows(&mut self, now: Instant) -> bool {
        self.expire(now);
        self.attempts.len() < self.maximum
    }

    pub fn record(&mut self, now: Instant) {
        self.expire(now);
        if self.attempts.len() == self.maximum {
            self.attempts.pop_front();
        }
        self.attempts.push_back(now);
    }

    pub fn len_at(&mut self, now: Instant) -> usize {
        self.expire(now);
        self.attempts.len()
    }

    fn expire(&mut self, now: Instant) {
        while self
            .attempts
            .front()
            .is_some_and(|attempt| now.saturating_duration_since(*attempt) >= self.window)
        {
            self.attempts.pop_front();
        }
    }
}

impl BoundedOutboundQueue {
    pub fn new(capacity: usize) -> Self {
        Self {
            frames: VecDeque::with_capacity(capacity.min(MAX_OUTBOUND_FRAMES)),
            capacity: capacity.clamp(1, MAX_OUTBOUND_FRAMES),
            gap_pending: false,
        }
    }

    pub fn push(&mut self, frame: WireEnvelope) -> bool {
        if self.frames.len() == self.capacity {
            self.gap_pending = true;
            false
        } else {
            self.frames.push_back(frame);
            true
        }
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn gap_pending(&self) -> bool {
        self.gap_pending
    }

    pub fn pop_front(&mut self) -> Option<WireEnvelope> {
        self.frames.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use termirust_host_protocol::FrameKind;

    #[test]
    fn outbound_queue_is_bounded_and_coalesces_gap_state() {
        let mut queue = BoundedOutboundQueue::new(2);
        let frame = WireEnvelope {
            protocol_major: 1,
            protocol_minor: 0,
            kind: FrameKind::OutputEvent,
            flags: 0,
            request_id: [0; 16],
            payload: Vec::new(),
        };
        assert!(queue.push(frame.clone()));
        assert!(queue.push(frame.clone()));
        assert!(!queue.push(frame));
        assert_eq!(queue.len(), 2);
        assert!(queue.gap_pending);
    }

    #[test]
    fn outbound_queue_accepts_exact_protocol_maximum() {
        let mut queue = BoundedOutboundQueue::new(MAX_OUTBOUND_FRAMES);
        for value in 0..MAX_OUTBOUND_FRAMES {
            assert!(queue.push(WireEnvelope {
                protocol_major: 1,
                protocol_minor: 0,
                kind: FrameKind::OutputEvent,
                flags: 0,
                request_id: [value as u8; 16],
                payload: Vec::new(),
            }));
        }
        assert_eq!(queue.len(), MAX_OUTBOUND_FRAMES);
        assert!(!queue.push(WireEnvelope {
            protocol_major: 1,
            protocol_minor: 0,
            kind: FrameKind::OutputEvent,
            flags: 0,
            request_id: [0; 16],
            payload: Vec::new(),
        }));
        assert!(queue.gap_pending());
    }

    #[test]
    fn config_rejects_output_over_any_bound() {
        let mut config = SyntheticHostConfig::local(
            HostedSessionId::new(),
            HostInstanceId::new(),
            [1; HANDSHAKE_NONCE_BYTES],
        );
        config.output = vec![vec![0; MAX_OUTPUT_BYTES + 1]];
        assert_eq!(
            config.validate().unwrap_err().code,
            ClientErrorCode::ResourceLimit
        );
    }

    #[test]
    fn config_accepts_exact_output_chunk_limit() {
        let mut config = SyntheticHostConfig::local(
            HostedSessionId::new(),
            HostInstanceId::new(),
            [1; HANDSHAKE_NONCE_BYTES],
        );
        config.output = vec![vec![0; MAX_OUTPUT_BYTES]];
        assert!(config.validate().is_ok());
    }

    #[test]
    fn bad_peer_limiter_is_bounded_and_recovers_after_window() {
        let start = Instant::now();
        let mut limiter = BadPeerLimiter::new(2, Duration::from_secs(10));
        assert!(limiter.allows(start));
        limiter.record(start);
        limiter.record(start);
        assert!(!limiter.allows(start));
        assert!(limiter.allows(start + Duration::from_secs(10)));
        assert_eq!(limiter.len_at(start + Duration::from_secs(10)), 0);
    }

    #[test]
    fn handshake_nonce_cache_rejects_replay_and_stays_bounded() {
        let start = Instant::now();
        let mut cache = HandshakeNonceCache::default();
        assert!(cache.accept([1; HANDSHAKE_NONCE_BYTES], start));
        assert!(!cache.accept([1; HANDSHAKE_NONCE_BYTES], start));
        for value in 0..1_100_u16 {
            let mut nonce = [0_u8; HANDSHAKE_NONCE_BYTES];
            nonce[..2].copy_from_slice(&value.to_be_bytes());
            cache.accept(nonce, start);
        }
        assert_eq!(cache.entries.len(), 1_024);
        assert!(cache.accept(
            [1; HANDSHAKE_NONCE_BYTES],
            start + Duration::from_secs(10 * 60)
        ));
    }
}
