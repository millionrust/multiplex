mod descriptor;
mod error;
mod framing;
#[cfg(unix)]
mod host;
#[cfg(not(unix))]
#[path = "host_unsupported.rs"]
mod host;
pub mod process_observation;
mod resume;

pub use descriptor::{LaunchDescriptor, MAX_DESCRIPTOR_BYTES, StopDeadlines, stdin_is_pipe};
pub use error::{HostError, HostErrorCode};
pub use host::{MAX_LIVE_HOSTS, SessionHostHandle, SessionHostStats, start, start_with_cancel};
pub use resume::{
    CodexResumePlanInput, ResumeValidationCancellation, build_codex_resume_plan,
    discover_codex_conversation_handle,
};
