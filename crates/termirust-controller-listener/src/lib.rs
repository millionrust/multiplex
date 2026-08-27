//! Opt-in exact-interface Controller bridge for user-controlled LAN and VPN routes.
//!
//! The listener is disabled by default, does not advertise itself, never mutates a firewall,
//! and rejects wildcard or public bind addresses before reaching the socket adapter.

mod authorization;
mod bind;
mod error;
mod framing;
mod handshake;
mod host_backend;
mod interfaces;
mod launch;
mod protocol;
mod queue;
mod rate_limit;
mod runtime;

pub use authorization::{BridgeAuthorization, BridgeCommand, BridgeCommandKind};
pub use bind::{
    BoundControllerListener, BoundRoute, ControllerBinder, GeneratedPortSource, SystemBinder,
    SystemGeneratedPortSource, bind_selected_route,
};
pub use error::{ListenerError, ListenerErrorCode};
pub use framing::{read_bounded_frame, write_bounded_frame};
pub use handshake::{
    AuthenticatedControllerConnection, HandshakeEntropy, SystemHandshakeEntropy,
    authenticate_controller,
};
pub use host_backend::HostBackendFactory;
pub use interfaces::{InterfaceProvider, SystemInterfaceProvider, resolve_selected_interface};
pub use launch::{ListenerLaunchDescriptor, run_listener_worker};
pub use protocol::{
    ApprovalDecision, ControllerCommand, ControllerCommandEnvelope, ControllerResponse,
    ControllerSessionSummary, decode_command, decode_response, encode_command, encode_response,
};
pub use queue::{BoundedFrameQueue, QueueClass};
pub use rate_limit::{AuthRateLimiter, SourceBucket, SourceBucketKey};
pub use runtime::{
    AuthoritySnapshot, ControllerAuthorityProvider, ControllerBackendFactory,
    ControllerConnectionBackend, HostCommandContext, ListenerRuntime, ListenerRuntimeReport,
};
