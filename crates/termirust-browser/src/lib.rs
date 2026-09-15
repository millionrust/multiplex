//! Isolated, ephemeral browser execution with deny-by-default network policy.

mod network;
mod process;
mod runtime;

pub use network::{ApprovedOrigin, NetworkPolicy};
pub use runtime::{
    BrowserArtifact, BrowserArtifactKind, BrowserCancellation, BrowserError, BrowserRequest,
    BrowserRuntime, BrowserRuntimeConfig, BrowserRuntimeStatus,
};
