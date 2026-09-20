use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};

use crate::{HostedSessionId, OccupantGeneration, OutputSequence};

pub const MAX_ACTIVITY_SOURCE_ID_BYTES: usize = 64;
pub const HEURISTIC_IDLE_QUIET_NANOS: u64 = 2_000_000_000;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityState {
    #[default]
    Unknown,
    Idle,
    Busy,
    NeedsInput,
    Done,
    Failed,
}

impl ActivityState {
    const fn priority(self) -> u8 {
        match self {
            Self::Unknown => 0,
            Self::Idle => 1,
            Self::Done => 2,
            Self::Busy => 3,
            Self::NeedsInput => 4,
            Self::Failed => 5,
        }
    }

    pub const fn requires_attention(self) -> bool {
        matches!(self, Self::NeedsInput | Self::Done | Self::Failed)
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityConfidence {
    #[default]
    Estimated,
    Verified,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionReason {
    #[default]
    Input,
    Approval,
    Permission,
    Confirmation,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivitySourceKind {
    #[default]
    Unknown,
    Output,
    PromptObservation,
    ProcessObservation,
    StructuredAdapter,
    Approval,
    ProcessExit,
}

impl ActivitySourceKind {
    const fn priority(self) -> u8 {
        match self {
            Self::Unknown => 0,
            Self::Output => 1,
            Self::PromptObservation => 2,
            Self::ProcessObservation => 3,
            Self::StructuredAdapter => 4,
            Self::Approval => 5,
            Self::ProcessExit => 6,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct HostSequence(u64);

impl HostSequence {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ReadWatermark(pub OutputSequence);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ActivityEvidenceKind {
    Output,
    PromptQuiet {
        quiet_nanos: u64,
        prompt_recognized: bool,
        alternate_screen: bool,
    },
    ProcessBusy,
    StructuredIdle,
    StructuredBusy,
    ApprovalRequested {
        reason: AttentionReason,
    },
    ApprovalResolved,
    StructuredDone,
    StructuredFailed,
    ProcessExited {
        success: bool,
        output_drained: bool,
    },
    ObservationLost,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ActivityEvidence {
    pub session_id: HostedSessionId,
    pub generation: OccupantGeneration,
    pub host_sequence: HostSequence,
    pub output_sequence: OutputSequence,
    pub source_id: String,
    pub source_kind: ActivitySourceKind,
    pub confidence: ActivityConfidence,
    pub kind: ActivityEvidenceKind,
    pub expires_at: Option<u64>,
}

impl ActivityEvidence {
    pub fn validate(&self) -> Result<(), ActivityError> {
        if !valid_source_id(&self.source_id) {
            return Err(ActivityError::InvalidSource);
        }
        if self.host_sequence == HostSequence::ZERO {
            return Err(ActivityError::InvalidSequence);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ActivityAggregate {
    pub state: ActivityState,
    pub confidence: ActivityConfidence,
    pub effective_sequence: HostSequence,
    pub generation: OccupantGeneration,
    pub source_kind: ActivitySourceKind,
    pub source_id: String,
    pub expires_at: Option<u64>,
    pub stale: bool,
    pub attention_reason: Option<AttentionReason>,
    pub attention_sequence: Option<OutputSequence>,
}

impl Default for ActivityAggregate {
    fn default() -> Self {
        Self {
            state: ActivityState::Unknown,
            confidence: ActivityConfidence::Estimated,
            effective_sequence: HostSequence::ZERO,
            generation: OccupantGeneration::new(1),
            source_kind: ActivitySourceKind::Unknown,
            source_id: "unknown".to_string(),
            expires_at: None,
            stale: true,
            attention_reason: None,
            attention_sequence: None,
        }
    }
}

impl ActivityAggregate {
    pub fn validate(&self) -> Result<(), ActivityError> {
        if self.generation == OccupantGeneration::ZERO
            || !valid_source_id(&self.source_id)
            || (!self.state.requires_attention() && self.attention_sequence.is_some())
            || (self.state != ActivityState::NeedsInput && self.attention_reason.is_some())
        {
            return Err(ActivityError::InvalidAggregate);
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct ActivityAggregateFields {
    #[serde(default)]
    state: ActivityState,
    #[serde(default)]
    confidence: ActivityConfidence,
    #[serde(default)]
    effective_sequence: HostSequence,
    #[serde(default = "default_generation")]
    generation: OccupantGeneration,
    #[serde(default)]
    source_kind: ActivitySourceKind,
    #[serde(default = "default_source_id")]
    source_id: String,
    #[serde(default)]
    expires_at: Option<u64>,
    #[serde(default = "default_stale")]
    stale: bool,
    #[serde(default)]
    attention_reason: Option<AttentionReason>,
    #[serde(default)]
    attention_sequence: Option<OutputSequence>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ActivityAggregateRepr {
    Fields(ActivityAggregateFields),
    Legacy(ActivityState),
}

impl<'de> Deserialize<'de> for ActivityAggregate {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(match ActivityAggregateRepr::deserialize(deserializer)? {
            ActivityAggregateRepr::Fields(fields) => Self {
                state: fields.state,
                confidence: fields.confidence,
                effective_sequence: fields.effective_sequence,
                generation: fields.generation,
                source_kind: fields.source_kind,
                source_id: fields.source_id,
                expires_at: fields.expires_at,
                stale: fields.stale,
                attention_reason: fields.attention_reason,
                attention_sequence: fields.attention_sequence,
            },
            ActivityAggregateRepr::Legacy(state) => Self {
                state,
                ..Self::default()
            },
        })
    }
}

const fn default_generation() -> OccupantGeneration {
    OccupantGeneration::new(1)
}

fn default_source_id() -> String {
    "unknown".to_string()
}

const fn default_stale() -> bool {
    true
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActivityError {
    InvalidAggregate,
    InvalidSource,
    InvalidSequence,
    SessionMismatch,
}

impl fmt::Display for ActivityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidAggregate => "activity aggregate is invalid",
            Self::InvalidSource => "activity evidence source is invalid",
            Self::InvalidSequence => "activity evidence sequence is invalid",
            Self::SessionMismatch => "activity evidence belongs to another session",
        })
    }
}

fn valid_source_id(source_id: &str) -> bool {
    !source_id.is_empty()
        && source_id.len() <= MAX_ACTIVITY_SOURCE_ID_BYTES
        && source_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

impl std::error::Error for ActivityError {}

pub fn reduce_activity(
    session_id: HostedSessionId,
    aggregate: &mut ActivityAggregate,
    evidence: &ActivityEvidence,
) -> Result<bool, ActivityError> {
    evidence.validate()?;
    if evidence.session_id != session_id {
        return Err(ActivityError::SessionMismatch);
    }
    if evidence.generation < aggregate.generation {
        return Ok(false);
    }
    let generation_advanced = evidence.generation > aggregate.generation;
    if generation_advanced {
        *aggregate = ActivityAggregate {
            generation: evidence.generation,
            ..ActivityAggregate::default()
        };
    }

    let candidate = evidence_candidate(evidence);
    let Some((state, reason)) = candidate else {
        if matches!(evidence.kind, ActivityEvidenceKind::ObservationLost) && !aggregate.stale {
            aggregate.stale = true;
            return Ok(true);
        }
        return Ok(generation_advanced);
    };

    let order = evidence_order(evidence, state);
    let current_order = aggregate_order(aggregate);
    if order < current_order || (order == current_order && aggregate.state == state) {
        return Ok(false);
    }

    let verified_attention_locked = aggregate.state == ActivityState::NeedsInput
        && aggregate.confidence == ActivityConfidence::Verified;
    if verified_attention_locked
        && !matches!(
            evidence.kind,
            ActivityEvidenceKind::ApprovalResolved
                | ActivityEvidenceKind::StructuredDone
                | ActivityEvidenceKind::StructuredFailed
                | ActivityEvidenceKind::ProcessExited {
                    output_drained: true,
                    ..
                }
        )
    {
        return Ok(false);
    }

    aggregate.state = state;
    aggregate.confidence = evidence.confidence;
    aggregate.effective_sequence = evidence.host_sequence;
    aggregate.generation = evidence.generation;
    aggregate.source_kind = evidence.source_kind;
    aggregate.source_id.clone_from(&evidence.source_id);
    aggregate.expires_at = evidence.expires_at;
    aggregate.stale = false;
    aggregate.attention_reason = reason;
    aggregate.attention_sequence = state
        .requires_attention()
        .then_some(evidence.output_sequence)
        .filter(|sequence| *sequence > OutputSequence::ZERO);
    Ok(true)
}

pub fn refresh_activity_staleness(aggregate: &mut ActivityAggregate, now: u64) -> bool {
    if aggregate.stale || !aggregate.expires_at.is_some_and(|expires| now >= expires) {
        return false;
    }
    aggregate.stale = true;
    true
}

fn evidence_candidate(
    evidence: &ActivityEvidence,
) -> Option<(ActivityState, Option<AttentionReason>)> {
    match evidence.kind {
        ActivityEvidenceKind::Output
        | ActivityEvidenceKind::ProcessBusy
        | ActivityEvidenceKind::StructuredBusy => Some((ActivityState::Busy, None)),
        ActivityEvidenceKind::StructuredIdle | ActivityEvidenceKind::ApprovalResolved => {
            Some((ActivityState::Idle, None))
        }
        ActivityEvidenceKind::PromptQuiet {
            quiet_nanos,
            prompt_recognized: true,
            alternate_screen: false,
        } if quiet_nanos >= HEURISTIC_IDLE_QUIET_NANOS => Some((ActivityState::Idle, None)),
        ActivityEvidenceKind::ApprovalRequested { reason } => {
            Some((ActivityState::NeedsInput, Some(reason)))
        }
        ActivityEvidenceKind::StructuredDone => Some((ActivityState::Done, None)),
        ActivityEvidenceKind::StructuredFailed => Some((ActivityState::Failed, None)),
        ActivityEvidenceKind::ProcessExited {
            success,
            output_drained: true,
        } => Some((
            if success {
                ActivityState::Done
            } else {
                ActivityState::Failed
            },
            None,
        )),
        ActivityEvidenceKind::PromptQuiet { .. }
        | ActivityEvidenceKind::ProcessExited {
            output_drained: false,
            ..
        }
        | ActivityEvidenceKind::ObservationLost => None,
    }
}

fn evidence_order(evidence: &ActivityEvidence, state: ActivityState) -> (u64, u8, u8, &str) {
    (
        evidence.host_sequence.get(),
        state.priority(),
        evidence.source_kind.priority(),
        evidence.source_id.as_str(),
    )
}

fn aggregate_order(aggregate: &ActivityAggregate) -> (u64, u8, u8, &str) {
    (
        aggregate.effective_sequence.get(),
        aggregate.state.priority(),
        aggregate.source_kind.priority(),
        aggregate.source_id.as_str(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Scenario {
        name: String,
        expected: String,
    }

    fn evidence(
        session_id: HostedSessionId,
        sequence: u64,
        kind: ActivityEvidenceKind,
    ) -> ActivityEvidence {
        ActivityEvidence {
            session_id,
            generation: OccupantGeneration::new(1),
            host_sequence: HostSequence::new(sequence),
            output_sequence: OutputSequence::new(sequence),
            source_id: "test-adapter".to_string(),
            source_kind: ActivitySourceKind::StructuredAdapter,
            confidence: ActivityConfidence::Verified,
            kind,
            expires_at: None,
        }
    }

    #[test]
    fn activity_reducer_is_deterministic_for_delayed_and_duplicate_evidence() {
        let session_id = HostedSessionId::new();
        let mut aggregate = ActivityAggregate::default();
        let busy = evidence(session_id, 2, ActivityEvidenceKind::StructuredBusy);
        assert!(reduce_activity(session_id, &mut aggregate, &busy).unwrap());
        assert!(!reduce_activity(session_id, &mut aggregate, &busy).unwrap());
        assert!(
            !reduce_activity(
                session_id,
                &mut aggregate,
                &evidence(session_id, 1, ActivityEvidenceKind::StructuredFailed),
            )
            .unwrap()
        );
        assert_eq!(aggregate.state, ActivityState::Busy);
    }

    #[test]
    fn activity_reducer_fences_stale_generations_and_requires_drain_for_exit() {
        let session_id = HostedSessionId::new();
        let mut aggregate = ActivityAggregate::default();
        let mut next = evidence(
            session_id,
            1,
            ActivityEvidenceKind::ProcessExited {
                success: true,
                output_drained: false,
            },
        );
        next.generation = OccupantGeneration::new(2);
        assert!(reduce_activity(session_id, &mut aggregate, &next).unwrap());
        assert_eq!(aggregate.generation, OccupantGeneration::new(2));
        next.kind = ActivityEvidenceKind::ProcessExited {
            success: true,
            output_drained: true,
        };
        assert!(reduce_activity(session_id, &mut aggregate, &next).unwrap());
        let stale = evidence(session_id, 9, ActivityEvidenceKind::StructuredFailed);
        assert!(!reduce_activity(session_id, &mut aggregate, &stale).unwrap());
        assert_eq!(aggregate.state, ActivityState::Done);
    }

    #[test]
    fn activity_prompt_quiet_is_estimated_and_never_clears_verified_approval() {
        let session_id = HostedSessionId::new();
        let mut aggregate = ActivityAggregate::default();
        let approval = ActivityEvidence {
            source_kind: ActivitySourceKind::Approval,
            kind: ActivityEvidenceKind::ApprovalRequested {
                reason: AttentionReason::Approval,
            },
            ..evidence(session_id, 1, ActivityEvidenceKind::StructuredBusy)
        };
        reduce_activity(session_id, &mut aggregate, &approval).unwrap();
        let quiet = ActivityEvidence {
            host_sequence: HostSequence::new(2),
            source_kind: ActivitySourceKind::PromptObservation,
            confidence: ActivityConfidence::Estimated,
            kind: ActivityEvidenceKind::PromptQuiet {
                quiet_nanos: HEURISTIC_IDLE_QUIET_NANOS,
                prompt_recognized: true,
                alternate_screen: false,
            },
            ..approval
        };
        assert!(!reduce_activity(session_id, &mut aggregate, &quiet).unwrap());
        assert_eq!(aggregate.state, ActivityState::NeedsInput);
    }

    #[test]
    fn activity_staleness_is_monotonic_across_clock_reversal() {
        let mut aggregate = ActivityAggregate {
            stale: false,
            expires_at: Some(10),
            ..ActivityAggregate::default()
        };
        assert!(refresh_activity_staleness(&mut aggregate, 10));
        assert!(!refresh_activity_staleness(&mut aggregate, 2));
        assert!(aggregate.stale);
    }

    #[test]
    fn activity_legacy_state_deserializes_without_losing_compatibility() {
        let aggregate: ActivityAggregate = serde_json::from_str("\"idle\"").unwrap();
        assert_eq!(aggregate.state, ActivityState::Idle);
        assert!(aggregate.stale);
    }

    #[test]
    fn activity_fixture_manifest_covers_required_adversarial_scenarios() {
        let scenarios: Vec<Scenario> = serde_json::from_str(include_str!(
            "../../../tests/fixtures/activity/scenarios.json"
        ))
        .unwrap();
        let names = scenarios
            .iter()
            .map(|scenario| scenario.name.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        for required in [
            "delayed_duplicate",
            "stale_generation",
            "exit_drain",
            "prompt_quiet",
            "alternate_screen",
            "output_after_done",
            "clock_reversal",
            "view_read_race",
            "crash_boundary",
        ] {
            assert!(names.contains(required));
        }
        assert!(
            scenarios
                .iter()
                .all(|scenario| !scenario.expected.is_empty())
        );
        let malformed = include_str!("../../../tests/fixtures/activity/malformed.json");
        assert!(serde_json::from_str::<ActivityEvidence>(malformed).is_err());
    }

    #[test]
    fn activity_output_after_done_follows_newer_causal_evidence() {
        let session_id = HostedSessionId::new();
        let mut aggregate = ActivityAggregate::default();
        reduce_activity(
            session_id,
            &mut aggregate,
            &evidence(session_id, 1, ActivityEvidenceKind::StructuredDone),
        )
        .unwrap();
        let output = ActivityEvidence {
            source_kind: ActivitySourceKind::Output,
            confidence: ActivityConfidence::Estimated,
            ..evidence(session_id, 2, ActivityEvidenceKind::Output)
        };
        reduce_activity(session_id, &mut aggregate, &output).unwrap();
        assert_eq!(aggregate.state, ActivityState::Busy);
    }

    #[test]
    fn activity_alternate_screen_never_uses_prompt_heuristic() {
        let session_id = HostedSessionId::new();
        let mut aggregate = ActivityAggregate::default();
        reduce_activity(
            session_id,
            &mut aggregate,
            &evidence(session_id, 1, ActivityEvidenceKind::StructuredBusy),
        )
        .unwrap();
        let quiet = ActivityEvidence {
            host_sequence: HostSequence::new(2),
            source_kind: ActivitySourceKind::PromptObservation,
            confidence: ActivityConfidence::Estimated,
            kind: ActivityEvidenceKind::PromptQuiet {
                quiet_nanos: HEURISTIC_IDLE_QUIET_NANOS,
                prompt_recognized: true,
                alternate_screen: true,
            },
            ..evidence(session_id, 2, ActivityEvidenceKind::StructuredBusy)
        };
        assert!(!reduce_activity(session_id, &mut aggregate, &quiet).unwrap());
        assert_eq!(aggregate.state, ActivityState::Busy);
    }

    #[test]
    fn activity_recognized_prompt_after_quiet_can_estimate_idle() {
        let session_id = HostedSessionId::new();
        let mut aggregate = ActivityAggregate::default();
        reduce_activity(
            session_id,
            &mut aggregate,
            &evidence(session_id, 1, ActivityEvidenceKind::StructuredBusy),
        )
        .unwrap();
        let quiet = ActivityEvidence {
            host_sequence: HostSequence::new(2),
            source_kind: ActivitySourceKind::PromptObservation,
            confidence: ActivityConfidence::Estimated,
            kind: ActivityEvidenceKind::PromptQuiet {
                quiet_nanos: HEURISTIC_IDLE_QUIET_NANOS,
                prompt_recognized: true,
                alternate_screen: false,
            },
            ..evidence(session_id, 2, ActivityEvidenceKind::StructuredBusy)
        };
        assert!(reduce_activity(session_id, &mut aggregate, &quiet).unwrap());
        assert_eq!(aggregate.state, ActivityState::Idle);
        assert_eq!(aggregate.confidence, ActivityConfidence::Estimated);
    }
}
