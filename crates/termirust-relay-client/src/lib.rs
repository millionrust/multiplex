//! Outbound, route-pinned relay transport for TermiRust Controller-v1 streams.
//!
//! Relay admission is only an outer transport gate. Callers must still run the complete
//! Controller-v1 authentication and authorization protocol over [`RelayByteStream`].

mod config;
mod credential;
mod error;
mod reconnect;
mod socket;
mod stream;

pub use config::{
    RelayAttemptId, RelayClientRole, RelayClientState, RelayCredentialRef, RelayDeviceId,
    RelayEndpointConfig, RelayEndpointId, RelayRouteBinding, RelaySpkiPin, RelayWssUrl,
};
pub use credential::{
    MemoryRelaySecretStore, RelayCredentialSecret, RelaySecretStore, RelaySecretStoreError,
};
pub use error::{RelayRouteError, RelayRouteErrorCode};
pub use reconnect::{
    MutationDisposition, MutationReconciliation, RelayOperationClass, RelayReconnectDecision,
    RelayReconnectPolicy,
};
pub use socket::{RelaySocket, RelayTlsClientConfig, spki_pin_from_certificate};
pub use stream::{RelayByteStream, RelayConnectionHandle};
