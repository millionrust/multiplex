mod adapter;
mod codex;
mod context;
mod headless;
mod orchestrator;
mod process;
mod protocol;
mod stream;
mod worktree;

pub use adapter::provider_descriptor;
pub use codex::{CodexSessionConfig, CodexSessionHandle, spawn_codex_session};
pub use context::{build_agent_context_handoff, build_context_handoff};
pub use headless::{HeadlessSessionConfig, HeadlessSessionHandle, spawn_headless_session};
pub use orchestrator::{SchedulableAgent, schedule_dependency_dag};
pub use process::{
    AgentExecutableStatus, build_interactive_launch_spec, build_remote_interactive_arguments,
    detect_agent_executable,
};
pub use protocol::{AgentApprovalRequest, AgentEvent, AgentRole, AgentRunState};
pub use worktree::{create_managed_worktree, managed_worktree_status, remove_managed_worktree};
