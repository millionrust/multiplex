#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AgentRunState {
    #[default]
    Idle,
    Starting,
    Running,
    WaitingForApproval,
    Blocked,
    Succeeded,
    Failed,
    Cancelled,
    Disconnected,
}

impl AgentRunState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "Ready",
            Self::Starting => "Starting",
            Self::Running => "Running",
            Self::WaitingForApproval => "Needs approval",
            Self::Blocked => "Blocked",
            Self::Succeeded => "Completed",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
            Self::Disconnected => "Disconnected",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentRole {
    User,
    Assistant,
    System,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentApprovalKind {
    Command,
    FileChange,
    Permissions,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentApprovalRequest {
    pub request_id: String,
    pub kind: AgentApprovalKind,
    pub operation: String,
    pub working_directory: Option<String>,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NormalizedToolCall {
    pub call_id: String,
    pub name: String,
    pub summary: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolOutcome {
    Succeeded,
    Failed,
    Declined,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentEvent {
    StateChanged(AgentRunState),
    SessionReady {
        provider_session_id: String,
    },
    MessageDelta {
        role: AgentRole,
        text: String,
    },
    ToolStarted(NormalizedToolCall),
    ToolFinished {
        call_id: String,
        outcome: ToolOutcome,
    },
    ApprovalRequested(AgentApprovalRequest),
    Completed {
        summary: Option<String>,
    },
    Failed {
        error: String,
    },
    Diagnostic {
        message: String,
    },
}
