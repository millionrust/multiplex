//! Local, bounded client for the TermiRust Host protocol.

mod attach_model;
mod client;
mod dev_url_projection;
mod error;
mod idempotency;
mod ipc;
mod sequence;
mod transport;

pub mod synthetic;

pub use attach_model::{AttachPhase, GpuiAttachModel, OutputDisposition};
pub use client::{ConnectOptions, ConnectionState, HostClient, SequencedOutput};
pub use dev_url_projection::{DevUrlProjection, DevUrlProjectionUpdate};
pub use error::{ClientError, ClientErrorCode};
pub use idempotency::{IdempotencyCache, IdempotencyDecision};
pub use ipc::{
    FakePeerAuthorizer, LocalEndpoint, PeerAuthorizer, PeerIdentity, UserOnlyUnixListener,
    WindowsNamedPipeSecurityAdapter,
};
pub use sequence::{SequenceDecision, SequenceTracker};
pub use transport::AsyncEnvelopeStream;
