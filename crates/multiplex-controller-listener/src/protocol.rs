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
/// The longest folder, shell or title a device may name when it starts a terminal.
pub const MAX_CREATE_SESSION_FIELD_BYTES: usize = 1_024;
/// The longest path, worktree or branch a device may name in a Git command.
pub const MAX_GIT_FIELD_BYTES: usize = 4_096;
/// The most commits one page of Git history carries.
pub const MAX_GIT_LOG_PAGE: u16 = 100;
/// What a Git answer is trimmed to, leaving room in a control frame for its envelope.
pub const MAX_GIT_RESPONSE_BYTES: usize = MAX_CONTROL_PAYLOAD_BYTES - 2 * 1024;
/// The most patch text read before trimming it to fit [`MAX_GIT_RESPONSE_BYTES`].
pub const MAX_GIT_DIFF_PATCH_BYTES: usize = 256 * 1024;
/// The most conversation items one `agent_chat` answer carries.
pub const MAX_AGENT_CHAT_PAGE: u16 = 100;
/// An agent chat answer is trimmed, oldest items first, until it fits in this.
pub const MAX_AGENT_CHAT_RESPONSE_BYTES: usize = MAX_CONTROL_PAYLOAD_BYTES - 2 * 1024;
/// Item ids and revisions are the log's own identifiers, bounded so a phone can echo them.
pub const MAX_AGENT_CHAT_ID_BYTES: usize = 256;

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
    /// Starts a terminal on the computer and attaches to it.
    ///
    /// The device chooses where it runs and what runs there, which is why this is its own
    /// capability rather than part of `SendInput`: typing into a terminal somebody opened is not
    /// the same as opening one. Every field is optional; the computer's own defaults answer for
    /// whatever is left out.
    CreateSession {
        /// Where the terminal starts. Absolute, on the computer's filesystem.
        folder: Option<String>,
        /// The program to run. The computer's default shell when absent.
        shell: Option<String>,
        /// What to call it in the session list.
        title: Option<String>,
        /// The size the device will draw it at.
        columns: u32,
        rows: u32,
    },
    /// The Git repository the terminal's folder is in: branch, changes, branches, worktrees.
    ///
    /// Reading a repository is reading what the terminal could show, so it needs what watching
    /// a terminal needs. `worktree` shows another worktree of the same repository instead.
    GitOverview {
        session_id: HostedSessionId,
        occupant_generation: OccupantGeneration,
        #[serde(default)]
        worktree: Option<String>,
    },
    /// One page of history from `HEAD`, newest first.
    GitLog {
        session_id: HostedSessionId,
        occupant_generation: OccupantGeneration,
        #[serde(default)]
        worktree: Option<String>,
        offset: u32,
        limit: u16,
    },
    /// A commit's changes, or the uncommitted ones when `commit` is absent; `path` narrows the
    /// patch to one file.
    GitDiff {
        session_id: HostedSessionId,
        occupant_generation: OccupantGeneration,
        #[serde(default)]
        worktree: Option<String>,
        #[serde(default)]
        commit: Option<String>,
        #[serde(default)]
        path: Option<String>,
    },
    /// Switches the worktree to a branch, as `git switch` would. Changing what the terminal's
    /// folder holds is as much as typing `git switch` into it, so it needs `SendInput`.
    GitSwitch {
        session_id: HostedSessionId,
        occupant_generation: OccupantGeneration,
        #[serde(default)]
        worktree: Option<String>,
        branch: String,
    },
    /// The conversation of the Claude Code or Codex session running in the terminal, read from
    /// that agent's own session log. Up to `limit` items, the newest unless `before` names the
    /// item to read back from. A `since_revision` equal to the log's current revision answers
    /// `unchanged` with no items, so a phone can ask often.
    AgentChat {
        session_id: HostedSessionId,
        occupant_generation: OccupantGeneration,
        #[serde(default)]
        before: Option<String>,
        #[serde(default)]
        since_revision: Option<String>,
        limit: u16,
    },
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
            Self::CreateSession { .. } => BridgeCommandKind::CreateSession,
            Self::GitOverview { .. } | Self::GitLog { .. } | Self::GitDiff { .. } => {
                BridgeCommandKind::GitRead
            }
            Self::GitSwitch { .. } => BridgeCommandKind::GitSwitch,
            Self::AgentChat { .. } => BridgeCommandKind::AgentChat,
            Self::Unsupported => BridgeCommandKind::Unsupported,
        }
    }

    pub const fn session_id(&self) -> Option<HostedSessionId> {
        match self {
            Self::ListSessions { .. }
            | Self::OpenScreen
            | Self::CloseScreen
            | Self::CreateSession { .. }
            | Self::Unsupported => None,
            Self::Attach { session_id, .. }
            | Self::AcquireWriter { session_id, .. }
            | Self::ReleaseWriter { session_id, .. }
            | Self::Input { session_id, .. }
            | Self::Resize { session_id, .. }
            | Self::Approval { session_id, .. }
            | Self::Detach { session_id, .. }
            | Self::GitOverview { session_id, .. }
            | Self::GitLog { session_id, .. }
            | Self::GitDiff { session_id, .. }
            | Self::GitSwitch { session_id, .. }
            | Self::AgentChat { session_id, .. } => Some(*session_id),
        }
    }

    pub const fn occupant_generation(&self) -> Option<OccupantGeneration> {
        match self {
            Self::ListSessions { .. }
            | Self::OpenScreen
            | Self::CloseScreen
            | Self::CreateSession { .. }
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
            }
            | Self::GitOverview {
                occupant_generation,
                ..
            }
            | Self::GitLog {
                occupant_generation,
                ..
            }
            | Self::GitDiff {
                occupant_generation,
                ..
            }
            | Self::GitSwitch {
                occupant_generation,
                ..
            }
            | Self::AgentChat {
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
            Self::CreateSession {
                folder,
                shell,
                title,
                columns,
                rows,
            } if *columns == 0
                || *rows == 0
                || *columns > 1_000
                || *rows > 1_000
                || folder.as_ref().is_some_and(|value| {
                    value.is_empty() || value.len() > MAX_CREATE_SESSION_FIELD_BYTES
                })
                || shell.as_ref().is_some_and(|value| {
                    value.is_empty() || value.len() > MAX_CREATE_SESSION_FIELD_BYTES
                })
                || title.as_ref().is_some_and(|value| {
                    value.is_empty() || value.len() > MAX_CREATE_SESSION_FIELD_BYTES
                }) =>
            {
                Err(ListenerError::new(ListenerErrorCode::MalformedFrame))
            }
            Self::AgentChat {
                before,
                since_revision,
                limit,
                ..
            } if *limit == 0
                || *limit > MAX_AGENT_CHAT_PAGE
                || !valid_agent_chat_id(before.as_deref())
                || !valid_agent_chat_id(since_revision.as_deref()) =>
            {
                Err(ListenerError::new(ListenerErrorCode::MalformedFrame))
            }
            Self::GitOverview { worktree, .. } if !valid_git_worktree(worktree.as_deref()) => {
                Err(ListenerError::new(ListenerErrorCode::MalformedFrame))
            }
            Self::GitLog {
                worktree,
                offset,
                limit,
                ..
            } if !valid_git_worktree(worktree.as_deref())
                || *limit == 0
                || *limit > MAX_GIT_LOG_PAGE
                || *offset > 1_000_000 =>
            {
                Err(ListenerError::new(ListenerErrorCode::MalformedFrame))
            }
            Self::GitDiff {
                worktree,
                commit,
                path,
                ..
            } if !valid_git_worktree(worktree.as_deref())
                || commit
                    .as_deref()
                    .is_some_and(|commit| !valid_git_commit(commit))
                || path.as_deref().is_some_and(|path| !valid_git_path(path)) =>
            {
                Err(ListenerError::new(ListenerErrorCode::MalformedFrame))
            }
            Self::GitSwitch {
                worktree, branch, ..
            } if !valid_git_worktree(worktree.as_deref()) || !valid_git_branch(branch) => {
                Err(ListenerError::new(ListenerErrorCode::MalformedFrame))
            }
            _ => Ok(()),
        }
    }
}

/// An absolute path of a worktree the overview listed.
fn valid_agent_chat_id(value: Option<&str>) -> bool {
    value.is_none_or(|value| {
        !value.is_empty()
            && value.len() <= MAX_AGENT_CHAT_ID_BYTES
            && !value.chars().any(char::is_control)
    })
}

fn valid_git_worktree(worktree: Option<&str>) -> bool {
    worktree.is_none_or(|worktree| {
        !worktree.is_empty()
            && worktree.len() <= MAX_GIT_FIELD_BYTES
            && !worktree.contains('\0')
            && std::path::Path::new(worktree).is_absolute()
    })
}

/// A full or abbreviated object id: never a ref name or an option.
fn valid_git_commit(commit: &str) -> bool {
    (4..=64).contains(&commit.len()) && commit.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// A path inside the worktree, relative to its top.
fn valid_git_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_GIT_FIELD_BYTES
        && !path.contains('\0')
        && !path.starts_with('/')
        && !path.split('/').any(|part| part == "..")
}

/// A branch as the overview named it. Git checks the rest; this keeps out what could be read
/// as an option or reach outside `refs/`.
fn valid_git_branch(branch: &str) -> bool {
    !branch.is_empty()
        && branch.len() <= 256
        && !branch.starts_with('-')
        && !branch.contains("..")
        && !branch
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
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
    /// A terminal the device asked for is running, and this is how to reach it.
    SessionCreated {
        command_id: CommandId,
        session_id: HostedSessionId,
        occupant_generation: OccupantGeneration,
    },
    /// The repository the terminal's folder is in, or `None` when it is in none.
    GitOverview {
        command_id: CommandId,
        session_id: HostedSessionId,
        repository: Option<GitRepository>,
    },
    GitLog {
        command_id: CommandId,
        session_id: HostedSessionId,
        commits: Vec<GitCommit>,
        next_offset: Option<u32>,
    },
    GitDiff {
        command_id: CommandId,
        session_id: HostedSessionId,
        diff: GitDiff,
    },
    /// Whether the switch happened, with what git said either way.
    GitSwitched {
        command_id: CommandId,
        session_id: HostedSessionId,
        switched: bool,
        branch: String,
        message: String,
    },
    /// The agent's conversation, or `None` when no Claude Code or Codex runs in the terminal.
    AgentChat {
        command_id: CommandId,
        session_id: HostedSessionId,
        chat: Option<AgentChat>,
    },
    /// A response from a newer computer than this build, which the reader skips.
    #[serde(other)]
    Unknown,
}

/// Which coding agent a terminal is running.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentProvider {
    ClaudeCode,
    Codex,
}

/// Whether the agent is working on a turn, as far as its log says.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentChatState {
    Working,
    Ready,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentChatItemKind {
    User,
    Assistant,
    Tool,
    /// Something that happened to the conversation rather than a message: an interrupted turn,
    /// a compacted history.
    Notice,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentToolState {
    Running,
    Succeeded,
    Failed,
}

/// One message, tool call or notice, with the log's own id so a phone can merge pages.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentChatItem {
    pub id: String,
    pub kind: AgentChatItemKind,
    /// A tool's name and what it was asked to act on.
    pub title: Option<String>,
    /// A message's text, or a tool's output.
    pub text: String,
    pub tool_state: Option<AgentToolState>,
    /// Whether `text` was cut short to fit.
    pub truncated: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentChat {
    pub provider: AgentProvider,
    /// The title the agent gave the conversation, when it gave one.
    pub title: Option<String>,
    pub state: AgentChatState,
    /// Changes whenever the log does; echoed back as `since_revision`.
    pub revision: String,
    /// Nothing changed since `since_revision`; `items` is empty.
    pub unchanged: bool,
    pub items: Vec<AgentChatItem>,
    /// More items come before the first one here.
    pub earlier: bool,
    /// The log is longer than the part read, so the oldest history is not available.
    pub history_truncated: bool,
}

/// A worktree's state as a phone shows it. Paths are on the computer's filesystem.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GitRepository {
    /// The terminal's folder.
    pub folder: String,
    /// The top of the worktree shown.
    pub root: String,
    /// `None` when `HEAD` is detached.
    pub branch: Option<String>,
    /// `None` before the first commit.
    pub head: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub changes: Vec<GitChange>,
    pub changes_truncated: bool,
    /// Local branches, then remote ones, most recently committed to first.
    pub branches: Vec<GitBranch>,
    pub branches_truncated: bool,
    pub worktrees: Vec<GitWorktree>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GitChangeStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Untracked,
    Conflicted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GitChange {
    pub path: String,
    pub original_path: Option<String>,
    pub status: GitChangeStatus,
    pub staged: bool,
    pub unstaged: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GitBranch {
    /// `main`, or `origin/main` for a remote one.
    pub name: String,
    pub remote: bool,
    /// Checked out in the worktree shown.
    pub current: bool,
    pub upstream: Option<String>,
    pub commit: String,
    pub subject: String,
    pub unix_time: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GitWorktree {
    pub path: String,
    pub branch: Option<String>,
    pub head: Option<String>,
    /// The worktree shown.
    pub current: bool,
    /// The worktree the terminal is in.
    pub terminal: bool,
    pub locked: bool,
    pub prunable: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GitCommit {
    pub oid: String,
    pub short: String,
    pub subject: String,
    pub author: String,
    pub unix_time: u64,
    pub parents: u32,
    /// Branches and tags pointing at it.
    pub refs: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GitCommitDetail {
    pub oid: String,
    pub short: String,
    pub author: String,
    pub email: String,
    pub unix_time: u64,
    pub parents: Vec<String>,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GitDiff {
    /// The commit shown, or `None` for uncommitted changes.
    pub commit: Option<GitCommitDetail>,
    pub files: Vec<GitDiffFile>,
    pub files_truncated: bool,
    /// Unified diff text, cut at a line when it is too long to send.
    pub patch: String,
    pub patch_truncated: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GitDiffFile {
    pub path: String,
    pub original_path: Option<String>,
    pub status: GitChangeStatus,
    /// `None` for a binary file or one not yet tracked.
    pub additions: Option<u32>,
    pub deletions: Option<u32>,
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
        ControllerResponse::AgentChat {
            chat: Some(chat), ..
        } if chat.items.len() > usize::from(MAX_AGENT_CHAT_PAGE)
            || (chat.unchanged && !chat.items.is_empty()) =>
        {
            Err(ListenerError::new(ListenerErrorCode::MalformedFrame))
        }
        ControllerResponse::GitLog {
            commits,
            next_offset,
            ..
        } if commits.len() > usize::from(MAX_GIT_LOG_PAGE) || next_offset == &Some(0) => {
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
        ControllerResponse::SessionCreated { .. } => "session_created",
        ControllerResponse::GitOverview { .. } => "git_overview",
        ControllerResponse::GitLog { .. } => "git_log",
        ControllerResponse::GitDiff { .. } => "git_diff",
        ControllerResponse::GitSwitched { .. } => "git_switched",
        ControllerResponse::AgentChat { .. } => "agent_chat",
        ControllerResponse::Unknown => "unknown",
        ControllerResponse::Error { .. } => "error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_chat_reads_with_attach_output_and_bounded_ids() {
        let chat = |limit: u16, before: Option<&str>| ControllerCommand::AgentChat {
            session_id: HostedSessionId::new(),
            occupant_generation: OccupantGeneration::new(1),
            before: before.map(str::to_owned),
            since_revision: None,
            limit,
        };
        assert!(chat(40, Some("a1b2-c3")).validate().is_ok());
        assert!(chat(0, None).validate().is_err());
        assert!(chat(MAX_AGENT_CHAT_PAGE + 1, None).validate().is_err());
        assert!(chat(40, Some("bad\nid")).validate().is_err());
        assert!(
            chat(40, Some(&"x".repeat(MAX_AGENT_CHAT_ID_BYTES + 1)))
                .validate()
                .is_err()
        );
        assert_eq!(
            chat(40, None).kind().capability(),
            multiplex_domain::ControllerCapability::AttachOutput
        );
        assert!(!chat(40, None).kind().requires_writer_lease());
        let encoded = encode_response(
            &ControllerResponse::AgentChat {
                command_id: CommandId::new(),
                session_id: HostedSessionId::new(),
                chat: None,
            },
            MAX_AGENT_CHAT_RESPONSE_BYTES,
        )
        .expect("encodes");
        assert!(String::from_utf8_lossy(&encoded).contains("\"kind\":\"agent_chat\""));
    }

    #[test]
    fn git_commands_carry_bounded_arguments_and_the_right_capability() {
        let session_id = HostedSessionId::new();
        let generation = OccupantGeneration::new(1);
        let diff = |commit: Option<&str>, path: Option<&str>| ControllerCommand::GitDiff {
            session_id,
            occupant_generation: generation,
            worktree: None,
            commit: commit.map(str::to_owned),
            path: path.map(str::to_owned),
        };
        assert!(
            diff(Some("1a2b3c4d"), Some("src/main.rs"))
                .validate()
                .is_ok()
        );
        assert!(diff(Some("HEAD"), None).validate().is_err());
        assert!(diff(Some("--output=x"), None).validate().is_err());
        assert!(diff(None, Some("../secret")).validate().is_err());
        assert!(diff(None, Some("/etc/passwd")).validate().is_err());
        let switch = |branch: &str| ControllerCommand::GitSwitch {
            session_id,
            occupant_generation: generation,
            worktree: None,
            branch: branch.to_owned(),
        };
        assert!(switch("feature/login").validate().is_ok());
        assert!(switch("-f").validate().is_err());
        assert!(switch("a b").validate().is_err());
        let overview = |worktree: &str| ControllerCommand::GitOverview {
            session_id,
            occupant_generation: generation,
            worktree: Some(worktree.to_owned()),
        };
        assert!(overview("relative/path").validate().is_err());
        let log = |limit: u16| ControllerCommand::GitLog {
            session_id,
            occupant_generation: generation,
            worktree: None,
            offset: 0,
            limit,
        };
        assert!(log(0).validate().is_err());
        assert!(log(MAX_GIT_LOG_PAGE + 1).validate().is_err());

        // Reading is watching; switching is typing.
        assert_eq!(
            log(10).kind().capability(),
            multiplex_domain::ControllerCapability::AttachOutput
        );
        assert_eq!(
            switch("main").kind().capability(),
            multiplex_domain::ControllerCapability::SendInput
        );
        assert!(switch("main").kind().addresses_a_session());
        assert!(!switch("main").kind().requires_writer_lease());
        assert_eq!(switch("main").session_id(), Some(session_id));
        assert_eq!(log(1).occupant_generation(), Some(generation));
    }

    #[test]
    fn git_responses_round_trip_and_older_readers_skip_them() {
        let response = ControllerResponse::GitSwitched {
            command_id: CommandId::new(),
            session_id: HostedSessionId::new(),
            switched: false,
            branch: "main".to_owned(),
            message: "error: Your local changes would be overwritten".to_owned(),
        };
        let bytes = encode_response(&response, MAX_CONTROL_PAYLOAD_BYTES).unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("\"kind\":\"git_switched\""));
        assert_eq!(
            decode_response(&bytes, MAX_CONTROL_PAYLOAD_BYTES).unwrap(),
            response
        );
        let unknown =
            br#"{"kind":"git_blame","command_id":"00000000-0000-0000-0000-000000000000"}"#;
        assert!(matches!(
            decode_response(unknown, MAX_CONTROL_PAYLOAD_BYTES),
            Ok(ControllerResponse::Unknown)
        ));
    }

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
            "command": { "kind": "conjure_a_pony", "colour": "roan" }
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
