//! Local, bounded client for the TermiRust Host protocol.

mod attach_model;
mod client;
mod dev_url_projection;
mod error;
mod host_recovery;
mod idempotency;
mod ipc;
mod sequence;
mod ssh_controller;
mod transport;

pub mod synthetic;

pub use attach_model::{AttachPhase, GpuiAttachModel, OutputDisposition};
pub use client::{ConnectOptions, ConnectionState, HostClient, SequencedOutput};
pub use dev_url_projection::{DevUrlProjection, DevUrlProjectionUpdate};
pub use error::{ClientError, ClientErrorCode};
pub use host_recovery::{
    AuthenticatedHostPeer, AuthenticatedIpcProbe, HostPeerProbe, HostProbeRequest,
    HostReconciliationError, HostReconciliationErrorCode, HostReconciliationPlan,
    HostReconciliationReceipt, HostReconciliationService, HostRecoveryFaultPoint,
};
pub use idempotency::{IdempotencyCache, IdempotencyDecision};
pub use ipc::{
    FakePeerAuthorizer, LocalEndpoint, PeerAuthorizer, PeerIdentity, UserOnlyUnixListener,
    WindowsNamedPipeSecurityAdapter,
};
pub use sequence::{SequenceDecision, SequenceTracker};
pub use ssh_controller::{
    AsyncSshControllerProcess, ControllerClientIdentityRef, KnownHostPolicy,
    RemoteControllerSession, SshControllerError, SshControllerErrorCode, SshControllerProcess,
    SshControllerTarget, SshControllerTargetId, SshOperationClass, SshReconnectDecision,
    SshReconnectPolicy, SshRouteState, ValidatedDnsOrIp, ValidatedUser, resolve_system_ssh,
    strict_ssh_command_argv,
};
pub use transport::AsyncEnvelopeStream;
