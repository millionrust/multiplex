use std::fmt;

use serde::{Deserialize, Serialize};
use termirust_controller_security::MAX_CONTROL_PAYLOAD_BYTES;
use termirust_domain::{CommandId, HostedSessionId, OccupantGeneration, OutputSequence};
use uuid::Uuid;

use crate::{BridgeCommand, BridgeCommandKind, ListenerError, ListenerErrorCode};

const CONTROLLER_COMMAND_VERSION: u16 = 1;
const MAX_INPUT_BYTES: usize = 60 * 1024;
const MAX_ERROR_CODE_BYTES: usize = 64;
const MAX_SESSION_TITLE_SCALARS: usize = 256;
pub const MAX_SESSION_PAGE_RECORDS: u16 = 1_000;
pub const MAX_SESSION_PAGE_BYTES: usize = MAX_CONTROL_PAYLOAD_BYTES - 256;

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerCommandEnvelope {
    pub version: u16,
    pub command_id: CommandId,
    pub session_generation: u64,
    pub deadline_millis: u64,
    pub command: ControllerCommand,
}

impl ControllerCommandEnvelope {
    pub fn new(
        command_id: CommandId,
        session_generation: u64,
        deadline_millis: u64,
        command: ControllerCommand,
    ) -> Self {
        Self {
            version: CONTROLLER_COMMAND_VERSION,
            command_id,
            session_generation,
            deadline_millis,
            command,
        }
    }

    pub fn validate(&self) -> Result<(), ListenerError> {
        if self.version != CONTROLLER_COMMAND_VERSION || self.deadline_millis == 0 {
            return Err(ListenerError::new(ListenerErrorCode::MalformedFrame));
        }
        self.command.validate()
    }

    pub fn bridge_command(&self) -> BridgeCommand {
        BridgeCommand {
            kind: self.command.kind(),
            session_id: self.command.session_id(),
            occupant_generation: self.command.occupant_generation(),
            session_generation: self.session_generation,
            deadline_millis: self.deadline_millis,
        }
    }
}

impl fmt::Debug for ControllerCommandEnvelope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ControllerCommandEnvelope")
            .field("version", &self.version)
            .field("command_id", &self.command_id)
            .field("command", &self.command.kind())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControllerCommand {
    ListSessions {
        offset: u32,
        limit: u16,
        expected_revision: Option<u64>,
    },
    Attach {
        session_id: HostedSessionId,
        occupant_generation: OccupantGeneration,
        from_sequence: OutputSequence,
        columns: u32,
        rows: u32,
    },
    Input {
        session_id: HostedSessionId,
        occupant_generation: OccupantGeneration,
        bytes: Vec<u8>,
    },
    Resize {
        session_id: HostedSessionId,
        occupant_generation: OccupantGeneration,
        columns: u32,
        rows: u32,
    },
    Approval {
        session_id: HostedSessionId,
        occupant_generation: OccupantGeneration,
        approval_id: Uuid,
        decision: ApprovalDecision,
    },
    Detach {
        session_id: HostedSessionId,
        occupant_generation: OccupantGeneration,
    },
}

impl ControllerCommand {
    pub const fn kind(&self) -> BridgeCommandKind {
        match self {
            Self::ListSessions { .. } => BridgeCommandKind::ListSessions,
            Self::Attach { .. } => BridgeCommandKind::Attach,
            Self::Input { .. } => BridgeCommandKind::Input,
            Self::Resize { .. } => BridgeCommandKind::Resize,
            Self::Approval { .. } => BridgeCommandKind::Approval,
            Self::Detach { .. } => BridgeCommandKind::Detach,
        }
    }

    pub const fn session_id(&self) -> Option<HostedSessionId> {
        match self {
            Self::ListSessions { .. } => None,
            Self::Attach { session_id, .. }
            | Self::Input { session_id, .. }
            | Self::Resize { session_id, .. }
            | Self::Approval { session_id, .. }
            | Self::Detach { session_id, .. } => Some(*session_id),
        }
    }

    pub const fn occupant_generation(&self) -> Option<OccupantGeneration> {
        match self {
            Self::ListSessions { .. } => None,
            Self::Attach {
                occupant_generation,
                ..
            }
            | Self::Input {
                occupant_generation,
                ..
            }
            | Self::Resize {
                occupant_generation,
                ..
            }
            | Self::Approval {
                occupant_generation,
                ..
            }
            | Self::Detach {
                occupant_generation,
                ..
            } => Some(*occupant_generation),
        }
    }

    fn validate(&self) -> Result<(), ListenerError> {
        match self {
            Self::ListSessions {
                offset,
                limit,
                expected_revision,
            } if *limit == 0
                || *limit > MAX_SESSION_PAGE_RECORDS
                || usize::try_from(*offset).unwrap_or(usize::MAX) > 10_000
                || expected_revision == &Some(0) =>
            {
                Err(ListenerError::new(ListenerErrorCode::MalformedFrame))
            }
            Self::ListSessions { .. } | Self::Detach { .. } | Self::Approval { .. } => Ok(()),
            Self::Attach { columns, rows, .. } | Self::Resize { columns, rows, .. }
                if *columns == 0 || *rows == 0 || *columns > 1_000 || *rows > 1_000 =>
            {
                Err(ListenerError::new(ListenerErrorCode::MalformedFrame))
            }
            Self::Input { bytes, .. } if bytes.is_empty() || bytes.len() > MAX_INPUT_BYTES => {
                Err(ListenerError::new(ListenerErrorCode::FrameTooLarge))
            }
            _ => Ok(()),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Approve,
    Deny,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControllerResponse {
    Sessions {
        command_id: CommandId,
        revision: u64,
        update_sequence: u64,
        sessions: Vec<ControllerSessionSummary>,
        next_offset: Option<u32>,
    },
    Attached {
        command_id: CommandId,
        session_id: HostedSessionId,
        occupant_generation: OccupantGeneration,
        replay_through_sequence: OutputSequence,
        has_writer_lease: bool,
    },
    Output {
        session_id: HostedSessionId,
        sequence: OutputSequence,
        bytes: Vec<u8>,
    },
    Completed {
        command_id: CommandId,
        applied: bool,
    },
    Detached {
        command_id: CommandId,
    },
    Error {
        command_id: CommandId,
        code: String,
        completion_unknown: bool,
    },
}

impl fmt::Debug for ControllerResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ControllerResponse")
            .field("kind", &response_kind(self))
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerSessionSummary {
    pub session_id: HostedSessionId,
    pub title: String,
    pub project: Option<String>,
    pub group: Option<String>,
    pub lifecycle: String,
    pub activity: String,
    pub occupant_generation: Option<OccupantGeneration>,
    pub last_output_sequence: OutputSequence,
    pub has_writer: bool,
    pub unread: bool,
}

impl fmt::Debug for ControllerSessionSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ControllerSessionSummary")
            .field("session_id", &self.session_id)
            .field("title", &"[REDACTED]")
            .field("project", &self.project.as_ref().map(|_| "[REDACTED]"))
            .field("group", &self.group.as_ref().map(|_| "[REDACTED]"))
            .field("lifecycle", &self.lifecycle)
            .field("activity", &self.activity)
            .field("occupant_generation", &self.occupant_generation)
            .field("last_output_sequence", &self.last_output_sequence)
            .field("has_writer", &self.has_writer)
            .field("unread", &self.unread)
            .finish()
    }
}

pub fn decode_command(bytes: &[u8]) -> Result<ControllerCommandEnvelope, ListenerError> {
    if bytes.is_empty() || bytes.len() > MAX_CONTROL_PAYLOAD_BYTES {
        return Err(ListenerError::new(ListenerErrorCode::FrameTooLarge));
    }
    let command: ControllerCommandEnvelope = serde_json::from_slice(bytes)
        .map_err(|_| ListenerError::new(ListenerErrorCode::MalformedFrame))?;
    command.validate()?;
    Ok(command)
}

pub fn encode_command(command: &ControllerCommandEnvelope) -> Result<Vec<u8>, ListenerError> {
    command.validate()?;
    encode_bounded(command, MAX_CONTROL_PAYLOAD_BYTES)
}

pub fn decode_response(
    bytes: &[u8],
    maximum_bytes: usize,
) -> Result<ControllerResponse, ListenerError> {
    if bytes.is_empty() || bytes.len() > maximum_bytes {
        return Err(ListenerError::new(ListenerErrorCode::FrameTooLarge));
    }
    let response: ControllerResponse = serde_json::from_slice(bytes)
        .map_err(|_| ListenerError::new(ListenerErrorCode::MalformedFrame))?;
    validate_response(&response)?;
    Ok(response)
}

pub fn encode_response(
    response: &ControllerResponse,
    maximum_bytes: usize,
) -> Result<Vec<u8>, ListenerError> {
    validate_response(response)?;
    encode_bounded(response, maximum_bytes)
}

fn validate_response(response: &ControllerResponse) -> Result<(), ListenerError> {
    match response {
        ControllerResponse::Sessions {
            revision,
            update_sequence,
            sessions,
            next_offset,
            ..
        } if *revision == 0
            || *update_sequence == 0
            || sessions.len() > usize::from(MAX_SESSION_PAGE_RECORDS)
            || next_offset == &Some(0)
            || sessions.iter().any(|session| {
                session.title.chars().count() > MAX_SESSION_TITLE_SCALARS
                    || session
                        .project
                        .as_ref()
                        .is_some_and(|name| name.chars().count() > MAX_SESSION_TITLE_SCALARS)
                    || session
                        .group
                        .as_ref()
                        .is_some_and(|name| name.chars().count() > MAX_SESSION_TITLE_SCALARS)
                    || session.lifecycle.len() > MAX_ERROR_CODE_BYTES
                    || session.activity.len() > MAX_ERROR_CODE_BYTES
            }) =>
        {
            Err(ListenerError::new(ListenerErrorCode::FrameTooLarge))
        }
        ControllerResponse::Output { bytes, .. } if bytes.is_empty() => {
            Err(ListenerError::new(ListenerErrorCode::MalformedFrame))
        }
        ControllerResponse::Error { code, .. }
            if code.is_empty()
                || code.len() > MAX_ERROR_CODE_BYTES
                || !code
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'_') =>
        {
            Err(ListenerError::new(ListenerErrorCode::MalformedFrame))
        }
        _ => Ok(()),
    }
}

fn encode_bounded(value: &impl Serialize, maximum_bytes: usize) -> Result<Vec<u8>, ListenerError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|_| ListenerError::new(ListenerErrorCode::MalformedFrame))?;
    if bytes.len() > maximum_bytes {
        return Err(ListenerError::new(ListenerErrorCode::FrameTooLarge));
    }
    Ok(bytes)
}

fn response_kind(response: &ControllerResponse) -> &'static str {
    match response {
        ControllerResponse::Sessions { .. } => "sessions",
        ControllerResponse::Attached { .. } => "attached",
        ControllerResponse::Output { .. } => "output",
        ControllerResponse::Completed { .. } => "completed",
        ControllerResponse::Detached { .. } => "detached",
        ControllerResponse::Error { .. } => "error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_round_trip_and_debug_never_contains_input_bytes() {
        let command = ControllerCommandEnvelope::new(
            CommandId::new(),
            2,
            4_000,
            ControllerCommand::Input {
                session_id: HostedSessionId::new(),
                occupant_generation: OccupantGeneration::new(3),
                bytes: b"TOP-SECRET-CONTENT".to_vec(),
            },
        );
        let encoded = encode_command(&command).unwrap();
        assert_eq!(decode_command(&encoded).unwrap(), command);
        assert!(!format!("{command:?}").contains("TOP-SECRET"));
    }

    #[test]
    fn malformed_versions_dimensions_input_and_unknown_fields_fail_closed() {
        let mut command = ControllerCommandEnvelope::new(
            CommandId::new(),
            1,
            2,
            ControllerCommand::Resize {
                session_id: HostedSessionId::new(),
                occupant_generation: OccupantGeneration::new(1),
                columns: 0,
                rows: 24,
            },
        );
        assert_eq!(
            encode_command(&command).unwrap_err().code,
            ListenerErrorCode::MalformedFrame
        );
        command.version = 2;
        assert!(encode_command(&command).is_err());
        assert!(decode_command(br#"{"version":1,"unknown":true}"#).is_err());
    }

    #[test]
    fn session_pages_are_bounded_and_require_stable_nonzero_revisions() {
        let command = ControllerCommandEnvelope::new(
            CommandId::new(),
            0,
            2,
            ControllerCommand::ListSessions {
                offset: 0,
                limit: MAX_SESSION_PAGE_RECORDS,
                expected_revision: None,
            },
        );
        assert_eq!(
            decode_command(&encode_command(&command).unwrap()).unwrap(),
            command
        );

        let invalid = ControllerCommandEnvelope::new(
            CommandId::new(),
            0,
            2,
            ControllerCommand::ListSessions {
                offset: 0,
                limit: MAX_SESSION_PAGE_RECORDS + 1,
                expected_revision: None,
            },
        );
        assert!(encode_command(&invalid).is_err());
        assert!(
            encode_response(
                &ControllerResponse::Sessions {
                    command_id: CommandId::new(),
                    revision: 0,
                    update_sequence: 1,
                    sessions: Vec::new(),
                    next_offset: None,
                },
                MAX_CONTROL_PAYLOAD_BYTES,
            )
            .is_err()
        );

        let summary = ControllerSessionSummary {
            session_id: HostedSessionId::new(),
            title: "Deploy".into(),
            project: Some("Console".into()),
            group: Some("Release".into()),
            lifecycle: "live".into(),
            activity: "needs_input".into(),
            occupant_generation: Some(OccupantGeneration::new(2)),
            last_output_sequence: OutputSequence::new(9),
            has_writer: false,
            unread: true,
        };
        let response = ControllerResponse::Sessions {
            command_id: CommandId::new(),
            revision: 4,
            update_sequence: 4,
            sessions: vec![summary],
            next_offset: None,
        };
        let encoded = encode_response(&response, MAX_SESSION_PAGE_BYTES).unwrap();
        assert_eq!(
            decode_response(&encoded, MAX_SESSION_PAGE_BYTES).unwrap(),
            response
        );

        let attached = ControllerResponse::Attached {
            command_id: CommandId::new(),
            session_id: HostedSessionId::new(),
            occupant_generation: OccupantGeneration::new(3),
            replay_through_sequence: OutputSequence::new(12),
            has_writer_lease: false,
        };
        let encoded = encode_response(&attached, MAX_CONTROL_PAYLOAD_BYTES).unwrap();
        assert_eq!(
            decode_response(&encoded, MAX_CONTROL_PAYLOAD_BYTES).unwrap(),
            attached
        );
    }
}
