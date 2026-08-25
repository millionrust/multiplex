mod descriptor;
mod error;
mod framing;
mod host;
pub mod process_observation;

pub use descriptor::{LaunchDescriptor, MAX_DESCRIPTOR_BYTES, StopDeadlines, stdin_is_pipe};
pub use error::{HostError, HostErrorCode};
pub use host::{MAX_LIVE_HOSTS, SessionHostHandle, SessionHostStats, start, start_with_cancel};
