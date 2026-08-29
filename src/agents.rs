pub mod activity;
mod adapter;
mod codex;
mod context;
mod discovery;
mod headless;
mod orchestrator;
mod process;
mod protocol;
mod resume;
mod stream;
mod transcript;
mod worktree;

pub use activity::activity_projection_for_agent_event;
pub use adapter::provider_descriptor;
pub use codex::{
    CodexSessionConfig, CodexSessionHandle, RemoteCodexSessionConfig, spawn_codex_session,
    spawn_remote_codex_session,
};
pub use context::{build_agent_context_handoff, build_context_handoff};
pub use discovery::{
    CliDiscovery, DiscoveryCancellation, RuntimeDiscoveryEntry, RuntimeDiscoveryReport,
    discovery_path_snapshot, known_runtime_descriptors,
};
pub use headless::{
    HeadlessSessionConfig, HeadlessSessionHandle, RemoteHeadlessSessionConfig,
    RemoteHeadlessSessionHandle, spawn_headless_session, spawn_remote_headless_session,
};
pub use orchestrator::{SchedulableAgent, schedule_dependency_dag};
pub use process::{
    AgentExecutableStatus, build_app_attached_launch_config, build_interactive_launch_spec,
    build_remote_interactive_arguments, detect_agent_executable,
};
pub use protocol::{AgentApprovalRequest, AgentEvent, AgentRole, AgentRunState};
pub use resume::{CodexResumeLimits, ResumeValidationCancellation, build_codex_resume_plan};
pub(crate) use transcript::sanitized_candidate_transcript_contract;
pub use worktree::{create_managed_worktree, managed_worktree_status, remove_managed_worktree};
