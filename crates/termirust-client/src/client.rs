use std::time::Duration;

use termirust_domain::{CommandId, HostInstanceId, HostedSessionId, OutputSequence};
use termirust_host_protocol::wire::{self, envelope_payload};
use termirust_host_protocol::{
    CURRENT_PROTOCOL, CapabilitySet, FrameKind, HANDSHAKE_NONCE_BYTES, MAX_OUTPUT_BYTES,
    MAX_REPLAY_BYTES, MAX_REPLAY_RECORDS, NegotiatedLimits, PreservedPayload, ProtocolRange,
    ProtocolVersion, WireEnvelope, decode_host_instance_id, decode_session_id, encode_command_id,
    encode_payload, encode_session_id, local_limits, negotiate_protocol, payload_kind,
};
use tokio_util::sync::CancellationToken;

#[cfg(unix)]
use tokio::net::UnixStream;

use crate::ipc::LocalEndpoint;
#[cfg(unix)]
use crate::ipc::authorize_unix_stream;
use crate::sequence::SequenceError;
use crate::{AsyncEnvelopeStream, ClientError, ClientErrorCode, SequenceDecision, SequenceTracker};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionState {
    Disconnected,
    Handshaking,
    Ready,
    Closing,
}

#[derive(Clone, Debug)]
pub struct ConnectOptions {
    pub session_id: HostedSessionId,
    pub protocol: ProtocolRange,
    pub capabilities: CapabilitySet,
    pub limits: NegotiatedLimits,
    pub client_nonce: [u8; HANDSHAKE_NONCE_BYTES],
    pub handshake_timeout: Duration,
}

impl ConnectOptions {
    pub fn local(session_id: HostedSessionId, client_nonce: [u8; HANDSHAKE_NONCE_BYTES]) -> Self {
        Self {
            session_id,
            protocol: CURRENT_PROTOCOL,
            capabilities: CapabilitySet::all_local(),
            limits: local_limits(),
            client_nonce,
            handshake_timeout: Duration::from_secs(2),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SequencedOutput {
    pub sequence: OutputSequence,
    pub bytes: Vec<u8>,
}

#[cfg(unix)]
type PlatformStream = UnixStream;

pub struct HostClient {
    endpoint: LocalEndpoint,
    options: ConnectOptions,
    state: ConnectionState,
    #[cfg(unix)]
    stream: Option<AsyncEnvelopeStream<PlatformStream>>,
    host_instance_id: Option<HostInstanceId>,
    selected_version: Option<ProtocolVersion>,
    capabilities: CapabilitySet,
    limits: NegotiatedLimits,
    next_request: u64,
}

impl HostClient {
    pub fn disconnected(endpoint: LocalEndpoint, options: ConnectOptions) -> Self {
        Self {
            endpoint,
            capabilities: CapabilitySet::from_wire(&[]),
            limits: options.limits,
            options,
            state: ConnectionState::Disconnected,
            #[cfg(unix)]
            stream: None,
            host_instance_id: None,
            selected_version: None,
            next_request: 1,
        }
    }

    pub async fn connect(
        endpoint: LocalEndpoint,
        options: ConnectOptions,
        cancel: &CancellationToken,
    ) -> Result<Self, ClientError> {
        let mut client = Self::disconnected(endpoint, options);
        client.establish(cancel).await?;
        Ok(client)
    }

    pub const fn state(&self) -> ConnectionState {
        self.state
    }

    pub const fn host_instance_id(&self) -> Option<HostInstanceId> {
        self.host_instance_id
    }

    pub fn capabilities(&self) -> &CapabilitySet {
        &self.capabilities
    }

    pub const fn limits(&self) -> NegotiatedLimits {
        self.limits
    }

    #[cfg(unix)]
    async fn establish(&mut self, cancel: &CancellationToken) -> Result<(), ClientError> {
        if self.state != ConnectionState::Disconnected {
            return Err(ClientError::new(ClientErrorCode::InvalidState));
        }
        self.state = ConnectionState::Handshaking;
        let result = tokio::time::timeout(self.options.handshake_timeout, async {
            let stream = tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    return Err(ClientError::new(ClientErrorCode::Cancelled));
                }
                result = UnixStream::connect(self.endpoint.socket_path()) => {
                    result.map_err(ClientError::from)?
                }
            };
            authorize_unix_stream(&stream, unsafe { libc::geteuid() })?;
            self.stream = Some(AsyncEnvelopeStream::new(stream));
            self.perform_handshake(cancel).await
        })
        .await;
        match result {
            Ok(Ok(())) => {
                self.state = ConnectionState::Ready;
                Ok(())
            }
            Ok(Err(error)) => {
                self.stream = None;
                self.state = ConnectionState::Disconnected;
                Err(error)
            }
            Err(_) => {
                self.stream = None;
                self.state = ConnectionState::Disconnected;
                Err(ClientError::new(ClientErrorCode::Cancelled))
            }
        }
    }

    #[cfg(not(unix))]
    async fn establish(&mut self, _: &CancellationToken) -> Result<(), ClientError> {
        Err(ClientError::new(ClientErrorCode::PermissionDenied))
    }

    async fn perform_handshake(&mut self, cancel: &CancellationToken) -> Result<(), ClientError> {
        let request = wire::HandshakeRequest {
            session_id: encode_session_id(self.options.session_id),
            protocol: Some(self.options.protocol.into()),
            capabilities: self.options.capabilities.to_wire(),
            limits: Some(self.options.limits.into()),
            client_nonce: self.options.client_nonce.to_vec(),
        };
        let request_id = self.next_request_id();
        let response = self
            .exchange(
                FrameKind::HandshakeRequest,
                request_id,
                envelope_payload::Message::HandshakeRequest(request),
                cancel,
            )
            .await?;
        let response = match response.message {
            Some(envelope_payload::Message::HandshakeResponse(response)) => response,
            Some(envelope_payload::Message::ProtocolError(error)) => {
                return Err(ClientError::protocol(&error));
            }
            _ => return Err(ClientError::new(ClientErrorCode::MalformedFrame)),
        };
        if decode_session_id(&response.session_id)? != self.options.session_id
            || response.client_nonce_echo != self.options.client_nonce
            || response.host_nonce.len() != HANDSHAKE_NONCE_BYTES
        {
            return Err(ClientError::new(ClientErrorCode::WrongSession));
        }
        let selected = ProtocolVersion::try_from(
            response
                .selected_version
                .as_ref()
                .ok_or_else(|| ClientError::new(ClientErrorCode::MalformedFrame))?,
        )?;
        if negotiate_protocol(
            self.options.protocol,
            ProtocolRange {
                minimum: selected,
                maximum: selected,
            },
        ) != Some(selected)
        {
            return Err(ClientError::new(ClientErrorCode::ProtocolIncompatible));
        }
        let capabilities = CapabilitySet::from_wire(&response.capabilities);
        if capabilities.intersection(&self.options.capabilities) != capabilities {
            return Err(ClientError::new(ClientErrorCode::MalformedFrame));
        }
        let limits = NegotiatedLimits::try_from(
            response
                .limits
                .as_ref()
                .ok_or_else(|| ClientError::new(ClientErrorCode::MalformedFrame))?,
        )?;
        if self.options.limits.bounded_with(limits) != limits {
            return Err(ClientError::new(ClientErrorCode::MalformedFrame));
        }
        self.host_instance_id = Some(decode_host_instance_id(&response.host_instance_id)?);
        self.selected_version = Some(selected);
        self.capabilities = capabilities;
        self.limits = limits;
        Ok(())
    }

    pub async fn reconnect(
        &mut self,
        client_nonce: [u8; HANDSHAKE_NONCE_BYTES],
        cancel: &CancellationToken,
    ) -> Result<(), ClientError> {
        self.disconnect();
        self.options.client_nonce = client_nonce;
        self.establish(cancel).await
    }

    pub fn disconnect(&mut self) {
        self.state = ConnectionState::Closing;
        #[cfg(unix)]
        {
            self.stream = None;
        }
        self.host_instance_id = None;
        self.selected_version = None;
        self.state = ConnectionState::Disconnected;
    }

    pub async fn get_state(
        &mut self,
        cancel: &CancellationToken,
    ) -> Result<wire::StateEvent, ClientError> {
        self.require_ready()?;
        self.require_capability(wire::Capability::State)?;
        let request_id = self.next_request_id();
        let response = self
            .exchange(
                FrameKind::GetStateRequest,
                request_id,
                envelope_payload::Message::GetStateRequest(wire::GetStateRequest {
                    session_id: encode_session_id(self.options.session_id),
                }),
                cancel,
            )
            .await?;
        match response.message {
            Some(envelope_payload::Message::StateEvent(state)) => {
                self.require_session(&state.session_id)?;
                Ok(state)
            }
            Some(envelope_payload::Message::ProtocolError(error)) => {
                Err(ClientError::protocol(&error))
            }
            _ => Err(ClientError::new(ClientErrorCode::MalformedFrame)),
        }
    }

    pub async fn attach(
        &mut self,
        from_sequence: OutputSequence,
        columns: u32,
        rows: u32,
        cancel: &CancellationToken,
    ) -> Result<Vec<SequencedOutput>, ClientError> {
        self.require_ready()?;
        self.require_capability(wire::Capability::AttachReplay)?;
        let request_id = self.next_request_id();
        self.write_message(
            FrameKind::AttachRequest,
            request_id,
            envelope_payload::Message::AttachRequest(wire::AttachRequest {
                session_id: encode_session_id(self.options.session_id),
                from_sequence: from_sequence.get(),
                viewport: Some(wire::Viewport { columns, rows }),
                maximum_replay_bytes: MAX_REPLAY_BYTES,
                maximum_replay_records: MAX_REPLAY_RECORDS,
            }),
            cancel,
        )
        .await?;
        let mut tracker = SequenceTracker::new(from_sequence);
        let mut outputs = Vec::new();
        let mut bytes = 0_u64;
        let mut saw_ready = false;
        loop {
            let envelope = self.read_envelope(cancel).await?;
            if envelope.request_id != request_id {
                return Err(ClientError::new(ClientErrorCode::MalformedFrame));
            }
            let payload = decode_checked_payload(&envelope)?;
            match payload.message {
                Some(envelope_payload::Message::ReadyEvent(ready)) => {
                    self.require_session(&ready.session_id)?;
                    saw_ready = true;
                }
                Some(envelope_payload::Message::OutputEvent(output)) if saw_ready => {
                    self.require_session(&output.session_id)?;
                    if output.bytes.len() > self.limits.maximum_output_bytes.min(MAX_OUTPUT_BYTES) {
                        return Err(ClientError::new(ClientErrorCode::ResourceLimit));
                    }
                    let sequence = OutputSequence::new(output.sequence);
                    match tracker.observe(sequence, &output.bytes) {
                        Ok(SequenceDecision::Accept) => {
                            bytes = bytes
                                .checked_add(output.bytes.len() as u64)
                                .ok_or_else(|| ClientError::new(ClientErrorCode::ResourceLimit))?;
                            if bytes > MAX_REPLAY_BYTES
                                || outputs.len() >= MAX_REPLAY_RECORDS as usize
                            {
                                return Err(ClientError::new(ClientErrorCode::ResourceLimit));
                            }
                            outputs.push(SequencedOutput {
                                sequence,
                                bytes: output.bytes,
                            });
                        }
                        Ok(SequenceDecision::IdenticalDuplicate) => {}
                        Err(error) => return Err(sequence_error(error)),
                    }
                }
                Some(envelope_payload::Message::StateEvent(state)) if saw_ready => {
                    self.require_session(&state.session_id)?;
                    return Ok(outputs);
                }
                Some(envelope_payload::Message::GapEvent(gap)) => {
                    return Err(ClientError {
                        code: ClientErrorCode::SequenceGap,
                        io_kind: None,
                        recovery: Some(wire::RecoveryHint::Replay),
                        expected_sequence: Some(OutputSequence::new(gap.expected_sequence)),
                    });
                }
                Some(envelope_payload::Message::ProtocolError(error)) => {
                    return Err(ClientError::protocol(&error));
                }
                _ => return Err(ClientError::new(ClientErrorCode::MalformedFrame)),
            }
        }
    }

    pub async fn input(
        &mut self,
        command_id: CommandId,
        bytes: Vec<u8>,
        cancel: &CancellationToken,
    ) -> Result<bool, ClientError> {
        self.require_capability(wire::Capability::Input)?;
        if bytes.len() > self.limits.maximum_output_bytes {
            return Err(ClientError::new(ClientErrorCode::ResourceLimit));
        }
        self.mutation(
            command_id,
            envelope_payload::Message::InputRequest(wire::InputRequest {
                session_id: encode_session_id(self.options.session_id),
                command_id: encode_command_id(command_id),
                bytes,
            }),
            FrameKind::InputRequest,
            cancel,
        )
        .await
    }

    pub async fn resize(
        &mut self,
        command_id: CommandId,
        columns: u32,
        rows: u32,
        cancel: &CancellationToken,
    ) -> Result<bool, ClientError> {
        self.require_capability(wire::Capability::Resize)?;
        self.mutation(
            command_id,
            envelope_payload::Message::ResizeRequest(wire::ResizeRequest {
                session_id: encode_session_id(self.options.session_id),
                command_id: encode_command_id(command_id),
                viewport: Some(wire::Viewport { columns, rows }),
            }),
            FrameKind::ResizeRequest,
            cancel,
        )
        .await
    }

    pub async fn stop(
        &mut self,
        command_id: CommandId,
        mode: wire::StopMode,
        cancel: &CancellationToken,
    ) -> Result<bool, ClientError> {
        self.require_capability(wire::Capability::Stop)?;
        self.mutation(
            command_id,
            envelope_payload::Message::StopRequest(wire::StopRequest {
                session_id: encode_session_id(self.options.session_id),
                command_id: encode_command_id(command_id),
                mode: i32::from(mode),
            }),
            FrameKind::StopRequest,
            cancel,
        )
        .await
    }

    pub async fn interrupt(
        &mut self,
        command_id: CommandId,
        cancel: &CancellationToken,
    ) -> Result<bool, ClientError> {
        self.require_capability(wire::Capability::Interrupt)?;
        self.mutation(
            command_id,
            envelope_payload::Message::InterruptRequest(wire::InterruptRequest {
                session_id: encode_session_id(self.options.session_id),
                command_id: encode_command_id(command_id),
            }),
            FrameKind::InterruptRequest,
            cancel,
        )
        .await
    }

    pub async fn request_activity_snapshot(
        &mut self,
        cancel: &CancellationToken,
    ) -> Result<wire::ActivityEvent, ClientError> {
        self.require_ready()?;
        self.require_capability(wire::Capability::ActivitySnapshot)?;
        let request_id = self.next_request_id();
        let response = self
            .exchange(
                FrameKind::ActivitySnapshotRequest,
                request_id,
                envelope_payload::Message::ActivitySnapshotRequest(wire::ActivitySnapshotRequest {
                    session_id: encode_session_id(self.options.session_id),
                }),
                cancel,
            )
            .await?;
        match response.message {
            Some(envelope_payload::Message::ActivityEvent(event)) => {
                self.require_session(&event.session_id)?;
                Ok(event)
            }
            Some(envelope_payload::Message::ProtocolError(error)) => {
                Err(ClientError::protocol(&error))
            }
            _ => Err(ClientError::new(ClientErrorCode::MalformedFrame)),
        }
    }

    pub async fn detach(&mut self, cancel: &CancellationToken) -> Result<(), ClientError> {
        self.require_ready()?;
        let request_id = self.next_request_id();
        self.write_message(
            FrameKind::DetachRequest,
            request_id,
            envelope_payload::Message::DetachRequest(wire::DetachRequest {
                session_id: encode_session_id(self.options.session_id),
            }),
            cancel,
        )
        .await?;
        self.disconnect();
        Ok(())
    }

    async fn mutation(
        &mut self,
        command_id: CommandId,
        message: envelope_payload::Message,
        kind: FrameKind,
        cancel: &CancellationToken,
    ) -> Result<bool, ClientError> {
        self.require_ready()?;
        let request_id = command_id.as_uuid().into_bytes();
        let response = self.exchange(kind, request_id, message, cancel).await?;
        match response.message {
            Some(envelope_payload::Message::CommandResult(result)) => {
                if result.command_id != encode_command_id(command_id) {
                    return Err(ClientError::new(ClientErrorCode::MalformedFrame));
                }
                Ok(result.applied)
            }
            Some(envelope_payload::Message::ProtocolError(error)) => {
                Err(ClientError::protocol(&error))
            }
            _ => Err(ClientError::new(ClientErrorCode::MalformedFrame)),
        }
    }

    async fn exchange(
        &mut self,
        kind: FrameKind,
        request_id: [u8; 16],
        message: envelope_payload::Message,
        cancel: &CancellationToken,
    ) -> Result<wire::EnvelopePayload, ClientError> {
        self.write_message(kind, request_id, message, cancel)
            .await?;
        let envelope = self.read_envelope(cancel).await?;
        if envelope.request_id != request_id {
            return Err(ClientError::new(ClientErrorCode::MalformedFrame));
        }
        decode_checked_payload(&envelope)
    }

    async fn write_message(
        &mut self,
        kind: FrameKind,
        request_id: [u8; 16],
        message: envelope_payload::Message,
        cancel: &CancellationToken,
    ) -> Result<(), ClientError> {
        let version = self
            .selected_version
            .unwrap_or(self.options.protocol.maximum);
        let payload = wire::EnvelopePayload {
            message: Some(message),
        };
        let envelope = WireEnvelope {
            protocol_major: version.major,
            protocol_minor: version.minor,
            kind,
            flags: 0,
            request_id,
            payload: encode_payload(&payload),
        };
        #[cfg(unix)]
        {
            self.stream
                .as_mut()
                .ok_or_else(|| ClientError::new(ClientErrorCode::InvalidState))?
                .write(&envelope, cancel)
                .await
        }
        #[cfg(not(unix))]
        {
            let _ = (envelope, cancel);
            Err(ClientError::new(ClientErrorCode::PermissionDenied))
        }
    }

    async fn read_envelope(
        &mut self,
        cancel: &CancellationToken,
    ) -> Result<WireEnvelope, ClientError> {
        #[cfg(unix)]
        {
            self.stream
                .as_mut()
                .ok_or_else(|| ClientError::new(ClientErrorCode::InvalidState))?
                .read(cancel)
                .await
        }
        #[cfg(not(unix))]
        {
            let _ = cancel;
            Err(ClientError::new(ClientErrorCode::PermissionDenied))
        }
    }

    fn require_ready(&self) -> Result<(), ClientError> {
        if self.state == ConnectionState::Ready {
            Ok(())
        } else {
            Err(ClientError::new(ClientErrorCode::InvalidState))
        }
    }

    fn require_capability(&self, capability: wire::Capability) -> Result<(), ClientError> {
        if self.capabilities.contains(capability) {
            Ok(())
        } else {
            Err(ClientError::new(ClientErrorCode::InvalidState))
        }
    }

    fn require_session(&self, bytes: &[u8]) -> Result<(), ClientError> {
        if decode_session_id(bytes)? == self.options.session_id {
            Ok(())
        } else {
            Err(ClientError::new(ClientErrorCode::WrongSession))
        }
    }

    fn next_request_id(&mut self) -> [u8; 16] {
        let mut value = [0_u8; 16];
        value[8..].copy_from_slice(&self.next_request.to_be_bytes());
        self.next_request = self.next_request.saturating_add(1);
        value
    }
}

fn decode_checked_payload(envelope: &WireEnvelope) -> Result<wire::EnvelopePayload, ClientError> {
    let payload = PreservedPayload::decode(&envelope.payload)?.value;
    if payload_kind(&payload) != Some(envelope.kind) {
        return Err(ClientError::new(ClientErrorCode::MalformedFrame));
    }
    Ok(payload)
}

fn sequence_error(error: SequenceError) -> ClientError {
    match error {
        SequenceError::Gap { expected, .. } => ClientError {
            code: ClientErrorCode::SequenceGap,
            io_kind: None,
            recovery: Some(wire::RecoveryHint::Replay),
            expected_sequence: Some(expected),
        },
        SequenceError::ConflictingDuplicate { .. } => {
            ClientError::new(ClientErrorCode::ConflictingDuplicate)
        }
        SequenceError::Overflow => ClientError::new(ClientErrorCode::ResourceLimit),
    }
}
