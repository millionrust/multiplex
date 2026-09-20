//! Local-loopback server for forwarding authenticated, opaque Controller-v1 ciphertext.
//!
//! The server never decodes Controller frames and refuses cleartext non-loopback binds. Product
//! clients, public deployment, accounts, and offline queues are intentionally outside this crate.

mod config;
mod core;
mod error;
mod server;
mod store;

#[doc(hidden)]
pub mod harness;

pub use config::{RelayServerConfig, RelayServerLimits};
pub use core::{RelayCoreSnapshot, RelayDiagnosticSnapshot};
pub use error::RelayServerError;
pub use server::{RelayServer, RelayServerHandle, RelayTlsServerConfig};
pub use store::{RelayMetadataStore, RelayStoreFault};
