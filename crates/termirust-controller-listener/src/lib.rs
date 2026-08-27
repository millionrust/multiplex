//! Opt-in exact-interface Controller bridge for user-controlled LAN and VPN routes.
//!
//! The listener is disabled by default, does not advertise itself, never mutates a firewall,
//! and rejects wildcard or public bind addresses before reaching the socket adapter.

mod authorization;
mod bind;
mod error;
mod framing;
mod interfaces;
mod queue;
mod rate_limit;

pub use authorization::{BridgeAuthorization, BridgeCommand, BridgeCommandKind};
pub use bind::{
    BoundControllerListener, BoundRoute, ControllerBinder, GeneratedPortSource, SystemBinder,
    SystemGeneratedPortSource, bind_selected_route,
};
pub use error::{ListenerError, ListenerErrorCode};
pub use framing::{read_bounded_frame, write_bounded_frame};
pub use interfaces::{InterfaceProvider, SystemInterfaceProvider, resolve_selected_interface};
pub use queue::{BoundedFrameQueue, QueueClass};
pub use rate_limit::{AuthRateLimiter, SourceBucket, SourceBucketKey};
