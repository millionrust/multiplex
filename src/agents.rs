mod adapter;
mod codex;
mod context;
mod process;
mod protocol;
mod worktree;

pub use adapter::{AgentCapabilities, AgentProviderDescriptor, provider_descriptor};
pub use codex::{CodexSessionConfig, CodexSessionHandle, spawn_codex_session};
pub use context::{ContextHandoffPreview, build_context_handoff};
pub use process::{
    AgentExecutableStatus, AgentLaunchSpec, build_interactive_launch_spec,
    build_remote_interactive_arguments, detect_agent_executable,
};
pub use protocol::{
    AgentApprovalKind, AgentApprovalRequest, AgentEvent, AgentRole, AgentRunState,
    NormalizedToolCall, ToolOutcome,
};
pub use worktree::{
    ManagedWorktreeStatus, create_managed_worktree, managed_worktree_status,
    remove_managed_worktree,
};
