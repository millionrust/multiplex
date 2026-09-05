use termirust_domain::{
    ActivityConfidence, ActivityEvidenceKind, ActivitySourceKind, AttentionReason,
};

use super::protocol::{AgentApprovalKind, AgentEvent, AgentRunState};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentActivityProjection {
    pub kind: ActivityEvidenceKind,
    pub confidence: ActivityConfidence,
    pub source_kind: ActivitySourceKind,
}

impl AgentActivityProjection {
    pub fn requires_unread_attention(&self) -> bool {
        matches!(
            self.kind,
            ActivityEvidenceKind::ApprovalRequested { .. }
                | ActivityEvidenceKind::StructuredDone
                | ActivityEvidenceKind::StructuredFailed
        )
    }
}

pub fn activity_projection_for_agent_event(event: &AgentEvent) -> Option<AgentActivityProjection> {
    let (kind, confidence, source_kind) = match event {
        AgentEvent::StateChanged(state) => match state {
            AgentRunState::Idle => (
                ActivityEvidenceKind::StructuredIdle,
                ActivityConfidence::Verified,
                ActivitySourceKind::StructuredAdapter,
            ),
            AgentRunState::Starting | AgentRunState::Running => (
                ActivityEvidenceKind::StructuredBusy,
                ActivityConfidence::Verified,
                ActivitySourceKind::StructuredAdapter,
            ),
            AgentRunState::WaitingForApproval | AgentRunState::Blocked => (
                ActivityEvidenceKind::ApprovalRequested {
                    reason: AttentionReason::Approval,
                },
                ActivityConfidence::Verified,
                ActivitySourceKind::Approval,
            ),
            AgentRunState::Succeeded => (
                ActivityEvidenceKind::StructuredDone,
                ActivityConfidence::Verified,
                ActivitySourceKind::StructuredAdapter,
            ),
            AgentRunState::Failed => (
                ActivityEvidenceKind::StructuredFailed,
                ActivityConfidence::Verified,
                ActivitySourceKind::StructuredAdapter,
            ),
            AgentRunState::Cancelled | AgentRunState::Disconnected => (
                ActivityEvidenceKind::ObservationLost,
                ActivityConfidence::Estimated,
                ActivitySourceKind::StructuredAdapter,
            ),
        },
        AgentEvent::MessageDelta { .. }
        | AgentEvent::ToolStarted(_)
        | AgentEvent::ToolFinished { .. } => (
            ActivityEvidenceKind::StructuredBusy,
            ActivityConfidence::Verified,
            ActivitySourceKind::StructuredAdapter,
        ),
        AgentEvent::ApprovalRequested(request) => (
            ActivityEvidenceKind::ApprovalRequested {
                reason: match request.kind {
                    AgentApprovalKind::Permissions => AttentionReason::Permission,
                    AgentApprovalKind::Command | AgentApprovalKind::FileChange => {
                        AttentionReason::Approval
                    }
                },
            },
            ActivityConfidence::Verified,
            ActivitySourceKind::Approval,
        ),
        AgentEvent::Completed { .. } => (
            ActivityEvidenceKind::StructuredDone,
            ActivityConfidence::Verified,
            ActivitySourceKind::StructuredAdapter,
        ),
        AgentEvent::Failed { .. } => (
            ActivityEvidenceKind::StructuredFailed,
            ActivityConfidence::Verified,
            ActivitySourceKind::StructuredAdapter,
        ),
        AgentEvent::SessionReady { .. } | AgentEvent::Diagnostic { .. } => return None,
    };
    Some(AgentActivityProjection {
        kind,
        confidence,
        source_kind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::protocol::{AgentApprovalRequest, AgentEvent};

    #[test]
    fn activity_agent_adapter_never_copies_provider_content_into_projection() {
        let secret = "provider-content-must-not-persist";
        let event = AgentEvent::ApprovalRequested(AgentApprovalRequest {
            request_id: secret.to_string(),
            kind: AgentApprovalKind::Permissions,
            operation: secret.to_string(),
            working_directory: Some(secret.to_string()),
            reason: Some(secret.to_string()),
        });
        let projection = activity_projection_for_agent_event(&event).unwrap();
        let serialized = serde_json::to_string(&projection.kind).unwrap();
        assert!(!serialized.contains(secret));
        assert!(projection.requires_unread_attention());
        assert!(matches!(
            projection.kind,
            ActivityEvidenceKind::ApprovalRequested {
                reason: AttentionReason::Permission
            }
        ));
    }
}
