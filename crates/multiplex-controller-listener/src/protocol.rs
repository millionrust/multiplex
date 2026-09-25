use std::fmt;

use multiplex_controller_security::MAX_CONTROL_PAYLOAD_BYTES;
use multiplex_domain::{
    CommandId, HostInstanceId, HostedSessionId, OccupantGeneration, OutputSequence,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{BridgeCommand, BridgeCommandKind, ListenerError, ListenerErrorCode};

const CONTROLLER_COMMAND_VERSION: u16 = 1;
const MAX_INPUT_BYTES: usize = 16 * 1024;
const MAX_ERROR_CODE_BYTES: usize = 64;
const MAX_SESSION_TITLE_SCALARS: usize = 256;
pub const MAX_SESSION_PAGE_RECORDS: u16 = 1_000;
pub const MAX_SESSION_PAGE_BYTES: usize = MAX_CONTROL_PAYLOAD_BYTES - 256;
pub const MAX_SNAPSHOT_CHUNK_BYTES: usize = 128 * 1024;
/// A screen ticket is exactly one 32-byte proof.
pub const SCREEN_TICKET_BYTES: usize = 32;

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

    /// A command is read when this build is at least as new as the device that sent it.
    ///
    /// The rule used to be equality, which meant a version could never be raised: the first
    /// device to send version 2 would be turned away by every computer already shipped. A
    /// command from an older device still reads, and one from a newer device is refused rather
    /// than guessed at. See `docs/decisions/controller-wire-growth.md`.
    pub fn validate(&self) -> Result<(), ListenerError> {
        if self.version == 0
            || self.version > CONTROLLER_COMMAND_VERSION
            || self.deadline_millis == 0
        {
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
    AcquireWriter {
        session_id: HostedSessionId,
        occupant_generation: OccupantGeneration,
    },
    ReleaseWriter {
        session_id: HostedSessionId,
        occupant_generation: OccupantGeneration,
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
    /// Asks for a ticket that opens a Remote Screens session on this connection.
    OpenScreen,
    /// Ends the screen session and invalidates its ticket.
    CloseScreen,
    /// A command from a newer device than this build.
    ///
    /// Decoding it rather than failing is what lets the wire grow: an unknown command used to
    /// end the connection, taking the terminal the person was reading with it, so a phone could
    /// never try something a computer might not have. It is refused, once, and the connection
    /// carries on. See `docs/decisions/controller-wire-growth.md`.
    #[serde(other)]
    Unsupported,
}

impl ControllerCommand {
    pub const fn kind(&self) -> BridgeCommandKind {
        match self {
            Self::ListSessions { .. } => BridgeCommandKind::ListSessions,
            Self::Attach { .. } => BridgeCommandKind::Attach,
            Self::AcquireWriter { .. } => BridgeCommandKind::AcquireWriter,
            Self::ReleaseWriter { .. } => BridgeCommandKind::ReleaseWriter,
            Self::Input { .. } => BridgeCommandKind::Input,
            Self::Resize { .. } => BridgeCommandKind::Resize,
            Self::Approval { .. } => BridgeCommandKind::Approval,
            Self::Detach { .. } => BridgeCommandKind::Detach,
            Self::OpenScreen => BridgeCommandKind::OpenScreen,
            Self::CloseScreen => BridgeCommandKind::CloseScreen,
            Self::Unsupported => BridgeCommandKind::Unsupported,
        }
    }

    pub const fn session_id(&self) -> Option<HostedSessionId> {
        match self {
            Self::ListSessions { .. }
            | Self::OpenScreen
            | Self::CloseScreen
            | Self::Unsupported => None,
            Self::Attach { session_id, .. }
            | Self::AcquireWriter { session_id, .. }
            | Self::ReleaseWriter { session_id, .. }
            | Self::Input { session_id, .. }
            | Self::Resize { session_id, .. }
            | Self::Approval { session_id, .. }
            | Self::Detach { session_id, .. } => Some(*session_id),
        }
    }

    pub const fn occupant_generation(&self) -> Option<OccupantGeneration> {
        match self {
            Self::ListSessions { .. }
            | Self::OpenScreen
            | Self::CloseScreen
            | Self::Unsupported => None,
            Self::Attach {
                occupant_generation,
                ..
            }
            | Self::AcquireWriter {
                occupant_generation,
                ..
            }
            | Self::ReleaseWriter {
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
            Self::ListSessions { .. }
            | Self::Detach { .. }
            | Self::Approval { .. }
            | Self::AcquireWriter { .. }
            | Self::ReleaseWriter { .. }
            | Self::OpenScreen
            | Self::CloseScreen
            | Self::Unsupported => Ok(()),
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
    Snapshot {
        command_id: CommandId,
        session_id: HostedSessionId,
        boundary_sequence: OutputSequence,
        columns: u32,
        rows: u32,
        chunk_index: u32,
        chunk_count: u32,
        bytes: Vec<u8>,
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
    /// The screen session may start. The ticket proves it to the screen protocol's hello, once.
    ScreenOpened {
        command_id: CommandId,
        ticket: Vec<u8>,
        can_control_pointer: bool,
        can_control_keyboard: bool,
    },
    Error {
        command_id: CommandId,
        code: String,
        completion_unknown: bool,
    },
    /// A response from a newer computer than this build, which the reader skips.
    #[serde(other)]
    Unknown,
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
    #[serde(default)]
    pub host_instance_id: Option<HostInstanceId>,
    #[serde(default)]
    pub origin: ControllerSessionOrigin,
    #[serde(default)]
    pub runtime: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<ControllerSessionCapability>,
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

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerSessionOrigin {
    Terminal,
    ManagedAgent,
    ObservedAgent,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerSessionCapability {
    ObserveSessions,
    AttachOutput,
    SendInput,
    Resize,
    RespondToApproval,
}

impl fmt::Debug for ControllerSessionSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ControllerSessionSummary")
            .field("session_id", &self.session_id)
            .field("host_instance_id", &self.host_instance_id)
            .field("origin", &self.origin)
            .field("runtime", &self.runtime)
            .field("capabilities", &self.capabilities)
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
                    || session.runtime.as_ref().is_some_and(|runtime| {
                        runtime.is_empty()
                            || runtime.len() > 128
                            || !runtime.bytes().all(|byte| {
                                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_')
                            })
                    })
                    || session.capabilities.len() > 5
                    || session
                        .capabilities
                        .windows(2)
                        .any(|pair| pair[0] >= pair[1])
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
        ControllerResponse::Snapshot {
            columns,
            rows,
            chunk_index,
            chunk_count,
            bytes,
            ..
        } if *columns == 0
            || *rows == 0
            || *columns > 400
            || *rows > 200
            || *chunk_count == 0
            || *chunk_index >= *chunk_count
            || bytes.len() > MAX_SNAPSHOT_CHUNK_BYTES =>
        {
            Err(ListenerError::new(ListenerErrorCode::MalformedFrame))
        }
        ControllerResponse::ScreenOpened { ticket, .. } if ticket.len() != SCREEN_TICKET_BYTES => {
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
        ControllerResponse::Snapshot { .. } => "snapshot",
        ControllerResponse::Output { .. } => "output",
        ControllerResponse::Completed { .. } => "completed",
        ControllerResponse::Detached { .. } => "detached",
        ControllerResponse::ScreenOpened { .. } => "screen_opened",
        ControllerResponse::Unknown => "unknown",
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

    /// A command this build has never heard of is read, refused, and survivable.
    ///
    /// `docs/decisions/controller-wire-growth.md`: it used to fail to decode, and a decode
    /// failure ends the connection, so a phone could never try something a computer might not
    /// have without risking the terminal the person was reading.
    #[test]
    fn a_command_from_a_newer_device_is_refused_rather_than_fatal() {
        let envelope = serde_json::json!({
            "version": 1,
            "command_id": CommandId::new(),
            "session_generation": 1,
            "deadline_millis": 1_000,
            "command": { "kind": "create_session", "folder": "/tmp" }
        });
        let decoded = decode_command(&serde_json::to_vec(&envelope).unwrap())
            .expect("an unknown command kind still decodes");
        assert!(decoded.command == ControllerCommand::Unsupported);
        // Garbage is still fatal: tolerance is for kinds, not for malformed frames.
        assert!(decode_command(br#"{"version":1,"command":"#).is_err());
    }

    /// A response this build has never heard of is skipped, not a broken stream.
    #[test]
    fn a_response_from_a_newer_computer_is_skipped() {
        let bytes = serde_json::to_vec(&serde_json::json!({
            "kind": "host_addresses",
            "addresses": ["192.168.1.10:7420"]
        }))
        .unwrap();
        assert!(
            decode_response(&bytes, MAX_CONTROL_PAYLOAD_BYTES).unwrap()
                == ControllerResponse::Unknown
        );
    }

    /// The version rule allows a raise: older devices still read, newer ones are refused.
    #[test]
    fn a_command_version_may_be_older_but_never_newer() {
        let mut envelope = ControllerCommandEnvelope::new(
            CommandId::new(),
            1,
            1_000,
            ControllerCommand::OpenScreen,
        );
        assert!(envelope.validate().is_ok());
        envelope.version = CONTROLLER_COMMAND_VERSION + 1;
        assert!(envelope.validate().is_err());
        envelope.version = 0;
        assert!(envelope.validate().is_err());
    }

    #[test]
    fn legacy_session_summaries_default_new_identity_and_capability_fields() {
        let session_id = HostedSessionId::new();
        let legacy = serde_json::json!({
            "session_id": session_id,
            "title": "Legacy",
            "project": null,
            "group": null,
            "lifecycle": "live",
            "activity": "idle",
            "occupant_generation": 1,
            "last_output_sequence": 2,
            "has_writer": false,
            "unread": false
        });
        let summary: ControllerSessionSummary = serde_json::from_value(legacy).unwrap();
        assert_eq!(summary.host_instance_id, None);
        assert_eq!(summary.origin, ControllerSessionOrigin::Unknown);
        assert_eq!(summary.runtime, None);
        assert!(summary.capabilities.is_empty());
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

        let acquire = ControllerCommandEnvelope::new(
            CommandId::new(),
            1,
            2,
            ControllerCommand::AcquireWriter {
                session_id: HostedSessionId::new(),
                occupant_generation: OccupantGeneration::new(1),
            },
        );
        assert_eq!(
            decode_command(&encode_command(&acquire).unwrap()).unwrap(),
            acquire
        );

        let oversized_input = ControllerCommandEnvelope::new(
            CommandId::new(),
            1,
            2,
            ControllerCommand::Input {
                session_id: HostedSessionId::new(),
                occupant_generation: OccupantGeneration::new(1),
                bytes: vec![0; MAX_INPUT_BYTES + 1],
            },
        );
        assert_eq!(
            encode_command(&oversized_input).unwrap_err().code,
            ListenerErrorCode::FrameTooLarge
        );
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
            host_instance_id: Some(HostInstanceId::new()),
            origin: ControllerSessionOrigin::ManagedAgent,
            runtime: Some("codex".into()),
            capabilities: vec![
                ControllerSessionCapability::ObserveSessions,
                ControllerSessionCapability::AttachOutput,
            ],
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

        let snapshot = ControllerResponse::Snapshot {
            command_id: CommandId::new(),
            session_id: HostedSessionId::new(),
            boundary_sequence: OutputSequence::new(11),
            columns: 120,
            rows: 40,
            chunk_index: 0,
            chunk_count: 1,
            bytes: b"retained screen".to_vec(),
        };
        let encoded = encode_response(
            &snapshot,
            multiplex_controller_security::MAX_TERMINAL_FRAME_BYTES,
        )
        .unwrap();
        assert_eq!(
            decode_response(
                &encoded,
                multiplex_controller_security::MAX_TERMINAL_FRAME_BYTES
            )
            .unwrap(),
            snapshot
        );
    }
}
