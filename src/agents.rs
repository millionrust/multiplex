mod adapter;
mod process;
mod worktree;

pub use adapter::{AgentCapabilities, AgentProviderDescriptor, provider_descriptor};
pub use process::{
    AgentExecutableStatus, AgentLaunchSpec, build_interactive_launch_spec,
    build_remote_interactive_arguments, detect_agent_executable,
};
pub use worktree::{
    ManagedWorktreeStatus, create_managed_worktree, managed_worktree_status,
    remove_managed_worktree,
};
