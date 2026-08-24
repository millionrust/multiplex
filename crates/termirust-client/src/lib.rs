//! Local, bounded client for the TermiRust Host protocol.

mod client;
mod error;
mod idempotency;
mod ipc;
mod sequence;
mod transport;

pub mod synthetic;

pub use client::{ConnectOptions, ConnectionState, HostClient, SequencedOutput};
pub use error::{ClientError, ClientErrorCode};
pub use idempotency::{IdempotencyCache, IdempotencyDecision};
pub use ipc::{
    FakePeerAuthorizer, LocalEndpoint, PeerAuthorizer, PeerIdentity, UserOnlyUnixListener,
    WindowsNamedPipeSecurityAdapter,
};
pub use sequence::{SequenceDecision, SequenceTracker};
pub use transport::AsyncEnvelopeStream;
