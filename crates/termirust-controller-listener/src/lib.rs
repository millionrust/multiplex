//! Opt-in Controller bridge for private LAN and VPN networks.
//!
//! The listener is disabled by default. When enabled it binds each private address the
//! computer has, never mutates a firewall, and rejects wildcard, loopback, and public bind
//! addresses before reaching the socket adapter.

mod authorization;
mod bind;
mod client_channel;
mod desktop_pane_bridge;
mod devices;
mod discovery;
mod error;
mod firewall;
mod framing;
mod handshake;
mod host_backend;
mod interfaces;
mod launch;
mod ownership;
mod pairing;
mod pairing_protocol;
mod process_protocol;
mod protocol;
mod queue;
mod rate_limit;
mod runtime;
mod screen_session;
mod screen_tickets;
mod ssh_pairing_broker;
mod tmux_sessions;

pub use authorization::{BridgeAuthorization, BridgeCommand, BridgeCommandKind};
pub use bind::{
    BoundAddress, BoundControllerListeners, ControllerBinder, GeneratedPortSource, SystemBinder,
    SystemGeneratedPortSource, bind_address, bind_private_addresses,
};
pub use client_channel::{ControllerClientChannel, ControllerIncoming};
pub use desktop_pane_bridge::{
    DesktopPaneBridgeEndpoint, DesktopPaneBridgeServer, DesktopPaneRegistration,
    DesktopPaneRegistry, DesktopPaneTransport,
};
pub use devices::{
    ControllerChannelCloser, ControllerDeviceService, ControllerDeviceServiceError,
    NoControllerChannels,
};
pub use discovery::{
    BONJOUR_SERVICE_TYPE, BonjourAdvertisement, BonjourAnnouncement, bonjour_advertisement,
    discovery_id,
};
pub use error::{ListenerError, ListenerErrorCode};
pub use firewall::{FirewallObservation, FirewallObserver, SystemFirewallObserver};
pub use framing::{read_bounded_frame, write_bounded_frame};
pub use handshake::{
    AuthenticatedControllerConnection, HandshakeEntropy, SystemHandshakeEntropy,
    authenticate_controller, initiate_controller,
};
pub use host_backend::HostBackendFactory;
pub use interfaces::{InterfaceProvider, SystemInterfaceProvider};
pub use launch::{
    LISTENER_OWNERSHIP_WAIT, ListenerLaunchDescriptor, RepositoryBridgeSources,
    run_listener_worker, serve_repository_stdio_bridge,
};
pub use ownership::ListenerOwnership;
pub use pairing::{
    CodePairingAttempt, ControllerClientPairingResult, ControllerPairingAuthority,
    HostPairingDecision, MAX_CODE_PAIRING_ATTEMPTS, PairingAuthoritySnapshot, pair_controller,
    pair_controller_client, pair_controller_with_code, pair_controller_with_code_client,
};
pub use pairing_protocol::{
    CodePairingChallenge, CodePairingHello, ControllerConnectionPurpose, ControllerPairingOffer,
    MAX_PAIRING_ROUTES, PairingConnectRequest, PairingDeviceRegistration, PairingHostAck,
    PairingRoute, SshControllerPairingOffer,
};
pub use process_protocol::{
    ListenerControlCommand, ListenerProcessEvent, ProcessFirewallObservation,
    ProcessPairingDecision,
};
pub use protocol::{
    ApprovalDecision, ControllerCommand, ControllerCommandEnvelope, ControllerResponse,
    ControllerSessionCapability, ControllerSessionOrigin, ControllerSessionSummary,
    MAX_SESSION_PAGE_BYTES, MAX_SNAPSHOT_CHUNK_BYTES, decode_command, decode_response,
    encode_command, encode_response,
};
pub use queue::{BoundedFrameQueue, QueueClass};
pub use rate_limit::{AuthRateLimiter, SourceBucket, SourceBucketKey};
pub use runtime::{
    AuthoritySnapshot, ControllerAuthorityProvider, ControllerBackendFactory,
    ControllerConnectionBackend, HostCommandContext, ListenerRuntime, ListenerRuntimeReport,
    ListenerServices, ListeningAddressObserver, serve_authenticated_stdio_stream,
};
pub use screen_session::{
    ControllerScreenSession, MAX_SCREEN_PAYLOAD_BYTES, SCREEN_OUTGOING_DEPTH,
    ScreenFrameCapability, ScreenOutgoing,
};
pub use screen_tickets::{ScreenGrants, ScreenTicketStore};
pub use ssh_pairing_broker::{
    SshHostPairingDecision, SshHostPairingDecisionValue, SshHostPairingPrompt,
    request_ssh_host_pairing_decision,
};
pub use tmux_sessions::{TMUX_RUNTIME_ID, TmuxSessionSource};
