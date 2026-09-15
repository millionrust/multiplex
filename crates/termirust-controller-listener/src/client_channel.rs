use std::collections::HashMap;

use termirust_controller_security::{
    AuthenticatedConnection, CapabilitySet, ControllerCapability as SecurityCapability,
    ControllerFrameKind, HostStaticPublicKey, MAX_CONTROL_PAYLOAD_BYTES, MAX_TERMINAL_FRAME_BYTES,
    RevocationEpoch, StaticPrivateKey,
};
use termirust_domain::{CommandId, ControllerCapability as DomainCapability};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::{
    ControllerCommand, ControllerCommandEnvelope, ControllerResponse, HandshakeEntropy,
    ListenerError, ListenerErrorCode, decode_response, encode_command, initiate_controller,
    read_bounded_frame, write_bounded_frame,
};

pub struct ControllerClientChannel<S> {
    stream: S,
    connection: AuthenticatedConnection,
    session_generation: u64,
    revocation_epoch: u64,
    pending: HashMap<CommandId, SecurityCapability>,
}

impl<S> std::fmt::Debug for ControllerClientChannel<S> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ControllerClientChannel")
            .field("session_generation", &self.session_generation)
            .field("revocation_epoch", &self.revocation_epoch)
            .field("pending_commands", &self.pending.len())
            .field("stream", &"[REDACTED]")
            .field("connection", &"[REDACTED]")
            .finish()
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> ControllerClientChannel<S> {
    #[allow(clippy::too_many_arguments)]
    pub async fn connect(
        mut stream: S,
        identity_generation: u64,
        revocation_epoch: u64,
        session_generation: u64,
        host_key: HostStaticPublicKey,
        device_private: StaticPrivateKey,
        requested_capabilities: CapabilitySet,
        entropy: &mut impl HandshakeEntropy,
    ) -> Result<Self, ListenerError> {
        if session_generation == 0 {
            return Err(ListenerError::new(ListenerErrorCode::StaleGeneration));
        }
        let connection = initiate_controller(
            &mut stream,
            identity_generation,
            revocation_epoch,
            host_key,
            device_private,
            requested_capabilities,
            entropy,
        )
        .await?;
        Ok(Self {
            stream,
            connection,
            session_generation,
            revocation_epoch,
            pending: HashMap::new(),
        })
    }

    pub fn granted_capabilities(&self) -> CapabilitySet {
        self.connection.capabilities
    }

    pub async fn send(
        &mut self,
        command: ControllerCommand,
        deadline_millis: u64,
    ) -> Result<CommandId, ListenerError> {
        let command_id = CommandId::new();
        let capability = security_capability(command.kind().capability());
        if !self.connection.capabilities.contains(capability) {
            return Err(ListenerError::new(ListenerErrorCode::Unauthorized));
        }
        let envelope = ControllerCommandEnvelope::new(
            command_id,
            self.session_generation,
            deadline_millis,
            command,
        );
        let payload = encode_command(&envelope)?;
        let sealed = self
            .connection
            .transport
            .seal(
                ControllerFrameKind::Control,
                capability,
                RevocationEpoch(self.revocation_epoch),
                &payload,
            )
            .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
        write_bounded_frame(
            &mut self.stream,
            sealed.as_bytes(),
            MAX_TERMINAL_FRAME_BYTES,
        )
        .await?;
        self.pending.insert(command_id, capability);
        Ok(command_id)
    }

    pub async fn read_response(&mut self) -> Result<ControllerResponse, ListenerError> {
        let sealed = read_bounded_frame(&mut self.stream, MAX_TERMINAL_FRAME_BYTES).await?;
        let opened = self
            .connection
            .transport
            .open(&sealed)
            .map_err(|_| ListenerError::new(ListenerErrorCode::AuthenticationFailed))?;
        let maximum = match opened.kind {
            ControllerFrameKind::Control => MAX_CONTROL_PAYLOAD_BYTES,
            ControllerFrameKind::Terminal => MAX_TERMINAL_FRAME_BYTES,
        };
        let response = decode_response(&opened.payload, maximum)?;
        let (expected_kind, expected_capability, complete) = self.expected_response(&response)?;
        if opened.kind != expected_kind || opened.capability != expected_capability {
            return Err(ListenerError::new(ListenerErrorCode::Unauthorized));
        }
        if complete && let Some(command_id) = response_command_id(&response) {
            self.pending.remove(&command_id);
        }
        Ok(response)
    }

    fn expected_response(
        &self,
        response: &ControllerResponse,
    ) -> Result<(ControllerFrameKind, SecurityCapability, bool), ListenerError> {
        let value = match response {
            ControllerResponse::Sessions { command_id, .. } => (
                ControllerFrameKind::Control,
                self.require_pending(*command_id, SecurityCapability::ObserveSessions)?,
                true,
            ),
            ControllerResponse::Attached { command_id, .. } => (
                ControllerFrameKind::Control,
                self.require_pending(*command_id, SecurityCapability::AttachOutput)?,
                true,
            ),
            ControllerResponse::Snapshot { command_id, .. } => (
                ControllerFrameKind::Terminal,
                self.require_pending(*command_id, SecurityCapability::AttachOutput)?,
                false,
            ),
            ControllerResponse::Output { .. } => (
                ControllerFrameKind::Terminal,
                SecurityCapability::AttachOutput,
                false,
            ),
            ControllerResponse::Detached { command_id } => (
                ControllerFrameKind::Control,
                self.require_pending(*command_id, SecurityCapability::AttachOutput)?,
                true,
            ),
            ControllerResponse::Completed { command_id, .. }
            | ControllerResponse::Error { command_id, .. } => (
                ControllerFrameKind::Control,
                *self
                    .pending
                    .get(command_id)
                    .ok_or_else(|| ListenerError::new(ListenerErrorCode::Unauthorized))?,
                true,
            ),
        };
        Ok(value)
    }

    fn require_pending(
        &self,
        command_id: CommandId,
        capability: SecurityCapability,
    ) -> Result<SecurityCapability, ListenerError> {
        if self.pending.get(&command_id) == Some(&capability) {
            Ok(capability)
        } else {
            Err(ListenerError::new(ListenerErrorCode::Unauthorized))
        }
    }
}

fn response_command_id(response: &ControllerResponse) -> Option<CommandId> {
    match response {
        ControllerResponse::Sessions { command_id, .. }
        | ControllerResponse::Attached { command_id, .. }
        | ControllerResponse::Snapshot { command_id, .. }
        | ControllerResponse::Completed { command_id, .. }
        | ControllerResponse::Detached { command_id }
        | ControllerResponse::Error { command_id, .. } => Some(*command_id),
        ControllerResponse::Output { .. } => None,
    }
}

fn security_capability(capability: DomainCapability) -> SecurityCapability {
    match capability {
        DomainCapability::ObserveSessions => SecurityCapability::ObserveSessions,
        DomainCapability::AttachOutput => SecurityCapability::AttachOutput,
        DomainCapability::SendInput => SecurityCapability::SendInput,
        DomainCapability::Resize => SecurityCapability::Resize,
        DomainCapability::RespondToApproval => SecurityCapability::RespondToApproval,
    }
}
