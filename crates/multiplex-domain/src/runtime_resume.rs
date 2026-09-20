use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    CommandId, ExecutableFingerprint, HostedSession, HostedSessionId, HostedSessionState,
    OccupantGeneration, OccupantOwnership, PermissionPolicy, ProjectId, RecognitionConfidence,
    Revision, RuntimeCapability, RuntimeId, RuntimeRecognition, RuntimeVersion,
};

pub const CODEX_RESUME_VERSION: RuntimeVersion = RuntimeVersion::new(0, 150, 1);
pub const CODEX_RESUME_MAXIMUM_EXCLUSIVE: RuntimeVersion = RuntimeVersion::new(0, 150, 2);
pub const MAX_CONVERSATION_HANDLE_BYTES: usize = 128;
pub const MAX_RESUME_ARGUMENTS: usize = 16;
pub const MAX_RESUME_ARGUMENT_BYTES: usize = 4096;

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ConversationHandle(String);

impl ConversationHandle {
    pub fn codex(value: &str) -> Result<Self, ResumeError> {
        if value.is_empty() || value.len() > MAX_CONVERSATION_HANDLE_BYTES {
            return Err(ResumeError::ConversationMalformed);
        }
        let parsed = Uuid::parse_str(value).map_err(|_| ResumeError::ConversationMalformed)?;
        if parsed.hyphenated().to_string() != value.to_ascii_lowercase() {
            return Err(ResumeError::ConversationMalformed);
        }
        Ok(Self(value.to_ascii_lowercase()))
    }

    pub fn expose_to_provider(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ConversationHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ConversationHandle(<redacted>)")
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResumeRequest {
    pub command_id: CommandId,
    pub session_id: HostedSessionId,
    pub expected_generation: OccupantGeneration,
    pub expected_revision: Revision,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResumeCandidate {
    pub request: ResumeRequest,
    pub runtime_id: RuntimeId,
    pub runtime_version: RuntimeVersion,
    pub prior_generation: OccupantGeneration,
    pub expected_executable_fingerprint: ExecutableFingerprint,
    pub handle: ConversationHandle,
}

#[derive(Clone, Eq, PartialEq)]
pub struct ResumePlan {
    pub candidate: ResumeCandidate,
    pub replacement_session_id: HostedSessionId,
    pub canonical_project: ProjectId,
    pub working_directory: PathBuf,
    pub permission_policy: PermissionPolicy,
    pub executable: PathBuf,
    pub arguments: Vec<String>,
    pub safe_conversation_label: String,
}

impl ResumePlan {
    pub fn working_directory(&self) -> &Path {
        &self.working_directory
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }
}

impl fmt::Debug for ResumePlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResumePlan")
            .field("candidate", &self.candidate)
            .field("replacement_session_id", &self.replacement_session_id)
            .field("canonical_project", &self.canonical_project)
            .field("working_directory", &"<redacted>")
            .field("permission_policy", &self.permission_policy)
            .field("executable", &"<redacted>")
            .field(
                "arguments",
                &format_args!("{} entries", self.arguments.len()),
            )
            .field("safe_conversation_label", &self.safe_conversation_label)
            .finish()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ContinuityLink {
    pub command_id: CommandId,
    pub source_session_id: HostedSessionId,
    pub replacement_session_id: HostedSessionId,
    pub runtime_id: RuntimeId,
    pub prior_generation: OccupantGeneration,
    pub replacement_generation: OccupantGeneration,
    pub committed_at: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResumeEligibility {
    Eligible,
    Unavailable(ResumeError),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResumeError {
    StillRunning,
    OwnershipUnproven,
    StaleOccupant,
    StaleRevision,
    UnsupportedVersion,
    ConversationMissing,
    ConversationMalformed,
    PermissionDenied,
    ProviderUnavailable,
    ResourceLimit,
    Cancelled,
    ContinuityConflict,
}

impl fmt::Display for ResumeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::StillRunning => "session is still running",
            Self::OwnershipUnproven => "runtime ownership is not proven",
            Self::StaleOccupant => "runtime occurrence changed",
            Self::StaleRevision => "session metadata changed",
            Self::UnsupportedVersion => "runtime version cannot be resumed safely",
            Self::ConversationMissing => "provider conversation is unavailable",
            Self::ConversationMalformed => "provider conversation metadata is invalid",
            Self::PermissionDenied => "provider conversation access was denied",
            Self::ProviderUnavailable => "provider executable is unavailable",
            Self::ResourceLimit => "provider conversation exceeded a safety limit",
            Self::Cancelled => "resume was cancelled",
            Self::ContinuityConflict => "another replacement already won",
        })
    }
}

impl std::error::Error for ResumeError {}

pub fn codex_resume_contract_matches(version: RuntimeVersion) -> bool {
    version == CODEX_RESUME_VERSION
}

pub fn evaluate_resume(
    request: ResumeRequest,
    session: &HostedSession,
    recognition: Option<&RuntimeRecognition>,
    handle: Option<ConversationHandle>,
) -> Result<ResumeCandidate, ResumeError> {
    if request.session_id != session.id || request.expected_revision != session.revision {
        return Err(ResumeError::StaleRevision);
    }
    if !matches!(
        session.lifecycle,
        HostedSessionState::Exited | HostedSessionState::Failed
    ) {
        return Err(ResumeError::StillRunning);
    }
    let recognition = recognition.ok_or(ResumeError::OwnershipUnproven)?;
    if recognition.confidence != RecognitionConfidence::Verified {
        return Err(ResumeError::OwnershipUnproven);
    }
    let occupant = recognition
        .occupant
        .as_ref()
        .ok_or(ResumeError::OwnershipUnproven)?;
    if occupant.stale || occupant.generation != request.expected_generation {
        return Err(ResumeError::StaleOccupant);
    }
    if !matches!(occupant.ownership, OccupantOwnership::Managed { .. }) {
        return Err(ResumeError::OwnershipUnproven);
    }
    if occupant.runtime_id.as_str() != "codex"
        || occupant.descriptor_version == 0
        || !occupant
            .effective_capabilities()
            .contains(RuntimeCapability::Resume)
    {
        return Err(ResumeError::UnsupportedVersion);
    }
    let version = occupant
        .safe_version
        .as_deref()
        .and_then(crate::parse_runtime_version)
        .filter(|version| codex_resume_contract_matches(*version))
        .ok_or(ResumeError::UnsupportedVersion)?;
    let expected_executable_fingerprint = occupant
        .executable_fingerprint
        .ok_or(ResumeError::OwnershipUnproven)?;
    Ok(ResumeCandidate {
        request,
        runtime_id: occupant.runtime_id.clone(),
        runtime_version: version,
        prior_generation: occupant.generation,
        expected_executable_fingerprint,
        handle: handle.ok_or(ResumeError::ConversationMissing)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ActivityAggregate, HostInstanceId, OutputSequence, PositionKey, PresetId, ProcessToken,
        RuntimeCapabilitySet, RuntimeOccupant, SessionTitle, TitleSource,
    };

    fn session(lifecycle: HostedSessionState) -> HostedSession {
        HostedSession {
            id: HostedSessionId::new(),
            project_id: ProjectId::new(),
            group_id: None,
            preset_id: Some(PresetId::new()),
            title: SessionTitle::new("Resume fixture").unwrap(),
            title_source: TitleSource::Default,
            lifecycle,
            activity: ActivityAggregate::default(),
            pinned: false,
            position: PositionKey::FIRST,
            last_output_sequence: OutputSequence::ZERO,
            read_through_sequence: OutputSequence::ZERO,
            unread_sequence: None,
            archived_at: None,
            created_at: 1,
            updated_at: 1,
            revision: Revision::new(7),
        }
    }

    fn recognition(
        generation: u64,
        stale: bool,
        ownership: OccupantOwnership,
    ) -> RuntimeRecognition {
        RuntimeRecognition {
            occupant: Some(RuntimeOccupant {
                runtime_id: RuntimeId::new("codex").unwrap(),
                descriptor_version: 1,
                safe_version: Some("0.150.1".to_string()),
                executable_fingerprint: Some(crate::ExecutableFingerprint {
                    file_identity: 1,
                    size: 1,
                    modified_nanos: 1,
                    bounded_content_hash: 1,
                }),
                generation: OccupantGeneration::new(generation),
                ownership,
                capabilities: RuntimeCapabilitySet::new([
                    RuntimeCapability::InteractivePty,
                    RuntimeCapability::Resume,
                ]),
                stale,
            }),
            confidence: RecognitionConfidence::Verified,
            observed_at_nanos: 1,
        }
    }

    fn request(session: &HostedSession, generation: u64) -> ResumeRequest {
        ResumeRequest {
            command_id: CommandId::new(),
            session_id: session.id,
            expected_generation: OccupantGeneration::new(generation),
            expected_revision: session.revision,
        }
    }

    fn handle() -> ConversationHandle {
        ConversationHandle::codex("019cf76d-0493-77d1-8572-3fb4ac801ac8").unwrap()
    }

    #[test]
    fn exact_exited_managed_codex_occurrence_is_eligible() {
        let session = session(HostedSessionState::Exited);
        let host = HostInstanceId::new();
        let recognition = recognition(
            4,
            false,
            OccupantOwnership::Managed {
                host_instance: host,
                child_token: ProcessToken::new(host, 42, 4),
            },
        );
        let candidate = evaluate_resume(
            request(&session, 4),
            &session,
            Some(&recognition),
            Some(handle()),
        )
        .unwrap();
        assert_eq!(candidate.runtime_version, CODEX_RESUME_VERSION);
        assert!(!format!("{candidate:?}").contains(candidate.handle.expose_to_provider()));
    }

    #[test]
    fn running_observed_ambiguous_stale_and_wrong_version_fail_closed() {
        let running = session(HostedSessionState::Live);
        assert_eq!(
            evaluate_resume(request(&running, 4), &running, None, Some(handle())),
            Err(ResumeError::StillRunning)
        );

        let exited = session(HostedSessionState::Exited);
        let host = HostInstanceId::new();
        let fingerprint = crate::ExecutableFingerprint {
            file_identity: 1,
            size: 1,
            modified_nanos: 1,
            bounded_content_hash: 1,
        };
        for ownership in [
            OccupantOwnership::Observed {
                executable: fingerprint,
            },
            OccupantOwnership::Ambiguous,
        ] {
            assert_eq!(
                evaluate_resume(
                    request(&exited, 4),
                    &exited,
                    Some(&recognition(4, false, ownership)),
                    Some(handle()),
                ),
                Err(ResumeError::OwnershipUnproven)
            );
        }
        assert_eq!(
            evaluate_resume(
                request(&exited, 4),
                &exited,
                Some(&recognition(
                    4,
                    true,
                    OccupantOwnership::Managed {
                        host_instance: host,
                        child_token: ProcessToken::new(host, 42, 4),
                    },
                )),
                Some(handle()),
            ),
            Err(ResumeError::StaleOccupant)
        );
        let mut wrong = recognition(
            4,
            false,
            OccupantOwnership::Managed {
                host_instance: host,
                child_token: ProcessToken::new(host, 42, 4),
            },
        );
        wrong.occupant.as_mut().unwrap().safe_version = Some("0.150.2".to_string());
        assert_eq!(
            evaluate_resume(request(&exited, 4), &exited, Some(&wrong), Some(handle())),
            Err(ResumeError::UnsupportedVersion)
        );
    }

    #[test]
    fn conversation_handle_debug_and_validation_are_redacted_and_strict() {
        let handle = handle();
        assert_eq!(format!("{handle:?}"), "ConversationHandle(<redacted>)");
        assert_eq!(
            ConversationHandle::codex("../../conversation"),
            Err(ResumeError::ConversationMalformed)
        );
    }
}
