//! Local, capability-scoped, read-only MCP access to TermiRust.

mod actions;
mod backend;
mod protocol;
mod transport;

pub use actions::{ActionPolicy, ActionPolicyStore, ApprovedAction};
pub use backend::{ActionRequest, InspectionSource, LocalInspectionSource, SourceError};
pub use protocol::{
    Capability, CapabilitySet, MCP_PROTOCOL_VERSION, McpServer, ServerConfiguration,
};
pub use transport::{MAX_REQUEST_BYTES, run_stdio};
