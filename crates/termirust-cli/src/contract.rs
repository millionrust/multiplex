use std::fmt;

use serde::Serialize;
use termirust_domain::{
    HostedSession, HostedSessionState, LaunchPreset, PermissionPolicy, PresetRisk, ProjectStatus,
    ProjectSummary, Revision,
};

pub const CLI_JSON_SCHEMA_VERSION: u16 = 1;
pub const MAX_RESPONSE_RECORDS: usize = 1_000;
pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Usage,
    Validation,
    Unavailable,
    Incompatible,
    PermissionDenied,
    InteractionRequired,
    HostKeyUnknown,
    HostKeyChanged,
    AuthenticationDenied,
    BridgeUnavailable,
    UnknownCompletion,
    Conflict,
    ResourceLimit,
    Timeout,
    OperationFailed,
    Cancelled,
}

impl ErrorCode {
    pub const fn exit_code(self) -> i32 {
        match self {
            Self::Usage | Self::Validation => 2,
            Self::Unavailable | Self::Incompatible | Self::BridgeUnavailable => 3,
            Self::PermissionDenied
            | Self::InteractionRequired
            | Self::HostKeyUnknown
            | Self::HostKeyChanged
            | Self::AuthenticationDenied => 4,
            Self::Conflict => 5,
            Self::ResourceLimit => 6,
            Self::Timeout | Self::OperationFailed | Self::UnknownCompletion => 7,
            Self::Cancelled => 130,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Usage => "usage",
            Self::Validation => "validation",
            Self::Unavailable => "unavailable",
            Self::Incompatible => "incompatible",
            Self::PermissionDenied => "permission_denied",
            Self::InteractionRequired => "interaction_required",
            Self::HostKeyUnknown => "host_key_unknown",
            Self::HostKeyChanged => "host_key_changed",
            Self::AuthenticationDenied => "authentication_denied",
            Self::BridgeUnavailable => "bridge_unavailable",
            Self::UnknownCompletion => "unknown_completion",
            Self::Conflict => "conflict",
            Self::ResourceLimit => "resource_limit",
            Self::Timeout => "timeout",
            Self::OperationFailed => "operation_failed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Clone, Eq, PartialEq, Serialize)]
pub struct CliError {
    pub code: ErrorCode,
    pub message: String,
    pub hint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_revision: Option<u64>,
}

impl CliError {
    pub fn new(code: ErrorCode, message: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            code,
            message: bounded_text(message.into()),
            hint: bounded_text(hint.into()),
            current_revision: None,
        }
    }

    pub fn with_revision(mut self, revision: Revision) -> Self {
        self.current_revision = Some(revision.get());
        self
    }
}

impl fmt::Debug for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CliError")
            .field("code", &self.code)
            .field("message", &"<redacted>")
            .field("hint", &"<redacted>")
            .field("current_revision", &self.current_revision)
            .finish()
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CliError {}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StatusData {
    pub cli_version: String,
    pub json_schema_version: u16,
    pub protocol_minimum: String,
    pub protocol_maximum: String,
    pub store: String,
    pub host_control: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProjectView {
    pub id: String,
    pub name: String,
    pub status: String,
    pub revision: u64,
}

impl From<&ProjectSummary> for ProjectView {
    fn from(value: &ProjectSummary) -> Self {
        Self {
            id: value.project.id.to_string(),
            name: value.project.display_name.as_str().to_string(),
            status: match value.status {
                ProjectStatus::Available => "available",
                ProjectStatus::Unavailable => "unavailable",
                ProjectStatus::PermissionDenied => "permission_denied",
            }
            .to_string(),
            revision: value.project.revision.get(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PresetView {
    pub id: String,
    pub label: String,
    pub enabled: bool,
    pub favorite: bool,
    pub permission_policy: String,
    pub risk: String,
    pub revision: u64,
}

impl From<&LaunchPreset> for PresetView {
    fn from(value: &LaunchPreset) -> Self {
        Self {
            id: value.id.to_string(),
            label: value.label.as_str().to_string(),
            enabled: value.enabled,
            favorite: value.favorite,
            permission_policy: permission_policy(value.permission_policy).to_string(),
            risk: if matches!(value.risk, PresetRisk::Safe) {
                "safe"
            } else {
                "risky"
            }
            .to_string(),
            revision: value.revision.get(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SessionView {
    pub id: String,
    pub project_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset_id: Option<String>,
    pub title: String,
    pub state: String,
    pub activity: String,
    pub unread: bool,
    pub archived: bool,
    pub revision: u64,
}

impl From<&HostedSession> for SessionView {
    fn from(value: &HostedSession) -> Self {
        Self {
            id: value.id.to_string(),
            project_id: value.project_id.to_string(),
            group_id: value.group_id.map(|id| id.to_string()),
            preset_id: value.preset_id.map(|id| id.to_string()),
            title: value.title.as_str().to_string(),
            state: lifecycle_name(value.lifecycle).to_string(),
            activity: activity_name(value.activity.state).to_string(),
            unread: value.unread(),
            archived: value.archived_at.is_some(),
            revision: value.revision.get(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProjectListData {
    pub projects: Vec<ProjectView>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PresetListData {
    pub project_id: String,
    pub presets: Vec<PresetView>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SessionListData {
    pub sessions: Vec<SessionView>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SessionData {
    pub session: SessionView,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SessionMutationData {
    pub outcome: String,
    pub session: SessionView,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SessionRemovalPreviewData {
    pub session: SessionView,
    pub preview_token: String,
    pub repository_revision: u64,
    pub metadata_bytes: u64,
    pub journal_bytes: u64,
    pub transcript_bytes: u64,
    pub artifact_bytes: u64,
    pub total_bytes: u64,
    pub file_count: usize,
    pub confirmation: RemovalConfirmationKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemovalConfirmationKind {
    Remove,
    SessionTitle,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct HelpData {
    pub commands: Vec<String>,
    pub safety: String,
    pub exit_codes: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ControllerSshData {
    pub operation: String,
    pub route_state: String,
    pub target_label: String,
    pub ssh_host_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_fingerprint_suffix: Option<String>,
    pub capabilities: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_generation: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub writer_lease: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reconnect_attempt: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reconnect_deadline_millis: Option<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<ControllerRemoteSessionView>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ControllerRemoteSessionView {
    pub id: String,
    pub title: String,
    pub lifecycle: String,
    pub activity: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub occupant_generation: Option<u64>,
    pub last_output_sequence: u64,
    pub has_writer: bool,
    pub unread: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum CliData {
    Status(StatusData),
    Projects(ProjectListData),
    Presets(PresetListData),
    Sessions(SessionListData),
    Session(SessionData),
    Mutation(SessionMutationData),
    RemovalPreview(SessionRemovalPreviewData),
    ControllerSsh(ControllerSshData),
    Help(HelpData),
}

#[derive(Serialize)]
pub struct JsonSuccess<'a> {
    pub schema_version: u16,
    pub ok: bool,
    pub data: &'a CliData,
    pub warnings: &'a [String],
}

#[derive(Serialize)]
pub struct JsonFailure<'a> {
    pub schema_version: u16,
    pub ok: bool,
    pub error: &'a CliError,
    pub warnings: &'a [String],
}

pub fn lifecycle_name(state: HostedSessionState) -> &'static str {
    use HostedSessionState as State;
    match state {
        State::Draft => "draft",
        State::Validating => "validating",
        State::Starting => "starting",
        State::Provisioning => "provisioning",
        State::Attaching => "attaching",
        State::Replaying => "replaying",
        State::Live => "live",
        State::RecordingPaused => "recording_paused",
        State::Stopping => "stopping",
        State::Offline => "offline",
        State::Orphaned => "orphaned",
        State::Gap => "gap",
        State::PermissionDenied => "permission_denied",
        State::Incompatible => "incompatible",
        State::RunningAppAttached => "running_app_attached",
        State::Failed => "failed",
        State::Cancelled => "cancelled",
        State::Exited => "exited",
    }
}

fn permission_policy(policy: PermissionPolicy) -> &'static str {
    match policy {
        PermissionPolicy::AskAsNeeded => "ask_as_needed",
        PermissionPolicy::ReadOnly => "read_only",
        PermissionPolicy::WorkspaceWrite => "workspace_write",
    }
}

fn activity_name(activity: termirust_domain::ActivityState) -> &'static str {
    use termirust_domain::ActivityState as Activity;
    match activity {
        Activity::Unknown => "unknown",
        Activity::Idle => "idle",
        Activity::Busy => "busy",
        Activity::NeedsInput => "needs_input",
        Activity::Done => "done",
        Activity::Failed => "failed",
    }
}

fn bounded_text(value: String) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(512)
        .collect()
}
