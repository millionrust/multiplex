mod adapter;
mod process;

pub use adapter::{AgentCapabilities, AgentProviderDescriptor, provider_descriptor};
pub use process::{
    AgentExecutableStatus, AgentLaunchSpec, build_interactive_launch_spec, detect_agent_executable,
};
