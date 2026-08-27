//! Controller-v1's in-memory security reference.
//!
//! This crate owns no sockets, storage, user interface, or platform bindings. It has not been
//! independently audited and must not be described as an audited security product.

mod authorization;
mod codec;
mod connection;
mod error;
mod pairing;
mod sas;
mod transport;
mod types;

pub use authorization::{AuthorizationDecision, AuthorizationPolicy};
pub use codec::{PAIRING_OFFER_BYTES, decode_offer, encode_offer, pairing_prologue};
pub use connection::{
    AuthenticatedConnection, AuthenticatedPeerClaim, ConnectionChallenge, ConnectionInitiator,
    ConnectionPrelude, ConnectionResponder, NOISE_CONNECTION_PROTOCOL_NAME,
};
pub use error::{ControllerSecurityError, ErrorCode};
pub use pairing::{
    ConfirmedPairing, PairingMachine, device_public_key_from_private, host_public_key_from_private,
};
pub use sas::derive_sas_v1;
pub use transport::{ControllerTransport, MAX_SEQUENCE};
pub use types::{
    CONTROLLER_V1, CapabilitySet, ControllerCapability, ControllerFrame, ControllerFrameKind,
    ControllerProtocolVersion, DeviceStaticPublicKey, HANDSHAKE_TIMEOUT_MILLIS, HandshakeHash,
    HandshakeMessage, HostStaticPublicKey, MAX_CONTROL_PAYLOAD_BYTES,
    MAX_PAIRING_OFFER_LIFETIME_SECONDS, MAX_TERMINAL_FRAME_BYTES, NOISE_PROTOCOL_NAME,
    PairingNonce, PairingOfferCore, PairingRole, PairingState, PairingStep, RevocationEpoch,
    SasCode, SealedControllerFrame, StaticPrivateKey,
};
