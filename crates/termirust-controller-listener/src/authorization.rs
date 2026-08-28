use termirust_domain::{
    ControllerAuthorizationDecision, ControllerAuthorizationRequest, ControllerCapability,
    ControllerDeviceAuthority, HostedSessionId, OccupantGeneration,
};

use crate::{ListenerError, ListenerErrorCode};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BridgeCommandKind {
    ListSessions,
    Attach,
    AcquireWriter,
    ReleaseWriter,
    Input,
    Resize,
    Approval,
    Detach,
}

impl BridgeCommandKind {
    pub const fn capability(self) -> ControllerCapability {
        match self {
            Self::ListSessions => ControllerCapability::ObserveSessions,
            Self::Attach | Self::Detach => ControllerCapability::AttachOutput,
            Self::AcquireWriter | Self::ReleaseWriter | Self::Input => {
                ControllerCapability::SendInput
            }
            Self::Resize => ControllerCapability::Resize,
            Self::Approval => ControllerCapability::RespondToApproval,
        }
    }

    pub const fn requires_writer_lease(self) -> bool {
        matches!(
            self,
            Self::ReleaseWriter | Self::Input | Self::Resize | Self::Approval
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BridgeCommand {
    pub kind: BridgeCommandKind,
    pub session_id: Option<HostedSessionId>,
    pub occupant_generation: Option<OccupantGeneration>,
    pub session_generation: u64,
    pub deadline_millis: u64,
}

pub struct BridgeAuthorization<'a> {
    authority: &'a ControllerDeviceAuthority,
}

impl<'a> BridgeAuthorization<'a> {
    pub const fn new(authority: &'a ControllerDeviceAuthority) -> Self {
        Self { authority }
    }

    pub fn authorize(
        &self,
        mut request: ControllerAuthorizationRequest,
        command: BridgeCommand,
        current_occupant_generation: Option<OccupantGeneration>,
        has_writer_lease: bool,
    ) -> Result<(), ListenerError> {
        request.capability = command.kind.capability();
        request.session_generation = command.session_generation;
        request.deadline_millis = command.deadline_millis;
        match self.authority.authorize(request) {
            ControllerAuthorizationDecision::Allow => {}
            ControllerAuthorizationDecision::Deny(denial) => {
                return Err(ListenerError::authorization(denial));
            }
        }
        if command.kind != BridgeCommandKind::ListSessions {
            let presented = command
                .occupant_generation
                .ok_or_else(|| ListenerError::new(ListenerErrorCode::StaleGeneration))?;
            if current_occupant_generation != Some(presented) {
                return Err(ListenerError::new(ListenerErrorCode::StaleGeneration));
            }
        }
        if command.kind.requires_writer_lease() && !has_writer_lease {
            return Err(ListenerError::new(ListenerErrorCode::WriterLeaseRequired));
        }
        Ok(())
    }
}
