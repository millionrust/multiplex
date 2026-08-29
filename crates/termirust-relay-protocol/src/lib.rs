//! Canonical outer protocol for the optional TermiRust ciphertext relay.
//!
//! This crate owns no sockets, storage, terminal types, Controller plaintext, or user interface.
//! Relay operators are outside the Controller-v1 content trust boundary.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use std::fmt;
use zeroize::{Zeroize, ZeroizeOnDrop};

pub const RELAY_V1: RelayProtocolVersion = RelayProtocolVersion(1);
pub const RELAY_SUBPROTOCOL: &str = "termirust-relay-v1";
pub const RELAY_LOOPBACK_ORIGIN: &str = "termirust://relay-local";

pub const MAX_CIPHERTEXT_PAYLOAD_BYTES: usize = 1_048_576;
pub const RELAY_ENVELOPE_HEADER_BYTES: usize = 52;
pub const MAX_ENCODED_WEBSOCKET_MESSAGE_BYTES: usize = 1_048_640;
pub const MAX_REGISTERED_ROUTES: usize = 1_000;
pub const MAX_FORWARDING_PAIRS: usize = 100;
pub const MAX_UNAUTHENTICATED_HANDSHAKES: usize = 4;
pub const ADMISSION_LIFETIME_SECONDS: u64 = 30;
pub const IDLE_HEARTBEAT_SECONDS: u64 = 90;
pub const MAX_FAILED_ADMISSIONS_PER_SOURCE: u32 = 5;
pub const FAILED_ADMISSION_WINDOW_SECONDS: u64 = 600;
pub const MAX_QUEUE_MESSAGES: usize = 64;
pub const MAX_QUEUE_ENCODED_BYTES: usize = 4_194_304;
pub const RATE_BYTES_PER_SECOND: u64 = 4_194_304;
pub const RATE_BURST_BYTES: u64 = 8_388_608;

const ENVELOPE_MAGIC: [u8; 4] = *b"TRR1";
const HELLO_MAGIC: [u8; 4] = *b"TRH1";
const CHALLENGE_MAGIC: [u8; 4] = *b"TRC1";
const PROOF_MAGIC: [u8; 4] = *b"TRP1";
const RESULT_MAGIC: [u8; 4] = *b"TRA1";
const ADMISSION_DOMAIN: &[u8] = b"termirust-relay-admission-v1\0";

pub const CLIENT_HELLO_BYTES: usize = 40;
pub const ADMISSION_CHALLENGE_BYTES: usize = 128;
pub const ADMISSION_PROOF_BYTES: usize = 112;
pub const ADMISSION_RESULT_BYTES: usize = 16;

pub fn validate_websocket_message_len(len: usize) -> Result<(), RelayProtocolError> {
    if len == 0 || len > MAX_ENCODED_WEBSOCKET_MESSAGE_BYTES {
        Err(RelayProtocolError::FrameLimit)
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RelayProtocolVersion(pub u16);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RelayRouteId(pub [u8; 32]);

impl fmt::Debug for RelayRouteId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RelayRouteId([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum RelayEndpointRole {
    Host = 1,
    Controller = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum RelayDirection {
    HostToController = 1,
    ControllerToHost = 2,
}

impl RelayDirection {
    pub fn for_sender(role: RelayEndpointRole) -> Self {
        match role {
            RelayEndpointRole::Host => Self::HostToController,
            RelayEndpointRole::Controller => Self::ControllerToHost,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RelayConnectionSequence(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RelayRevocationEpoch(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RelayConnectionId(pub u64);

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RelayCredentialVerifier(pub [u8; 32]);

impl fmt::Debug for RelayCredentialVerifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RelayCredentialVerifier([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayQuota {
    pub queue_messages: usize,
    pub queue_encoded_bytes: usize,
    pub rate_bytes_per_second: u64,
    pub rate_burst_bytes: u64,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayRouteRegistration {
    pub route_id: RelayRouteId,
    pub host_verifier: RelayCredentialVerifier,
    pub controller_verifier: RelayCredentialVerifier,
    pub revocation_epoch: RelayRevocationEpoch,
    pub quota: RelayQuota,
    pub revoked: bool,
}

impl fmt::Debug for RelayRouteRegistration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RelayRouteRegistration")
            .field("route_id", &"[REDACTED]")
            .field("host_verifier", &"[REDACTED]")
            .field("controller_verifier", &"[REDACTED]")
            .field("revocation_epoch", &self.revocation_epoch)
            .field("quota", &self.quota)
            .field("revoked", &self.revoked)
            .finish()
    }
}

impl RelayRouteRegistration {
    pub fn new(
        route_id: RelayRouteId,
        host: &RelayAdmissionCredential,
        controller: &RelayAdmissionCredential,
    ) -> Self {
        Self {
            route_id,
            host_verifier: host.verifier(),
            controller_verifier: controller.verifier(),
            revocation_epoch: RelayRevocationEpoch(0),
            quota: RelayQuota::default(),
            revoked: false,
        }
    }

    pub fn verifier_for(&self, role: RelayEndpointRole) -> RelayCredentialVerifier {
        match role {
            RelayEndpointRole::Host => self.host_verifier,
            RelayEndpointRole::Controller => self.controller_verifier,
        }
    }

    pub fn validate(&self) -> Result<(), RelayProtocolError> {
        self.quota.validate()?;
        VerifyingKey::from_bytes(&self.host_verifier.0)
            .map_err(|_| RelayProtocolError::InvalidVerifier)?;
        VerifyingKey::from_bytes(&self.controller_verifier.0)
            .map_err(|_| RelayProtocolError::InvalidVerifier)?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayServerState {
    Stopped,
    Loading,
    ListeningLoopback,
    Draining,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayRouteState {
    Registered,
    HostWaiting,
    ControllerWaiting,
    Forwarding,
    Closed,
    Revoked,
}

impl Default for RelayQuota {
    fn default() -> Self {
        Self {
            queue_messages: MAX_QUEUE_MESSAGES,
            queue_encoded_bytes: MAX_QUEUE_ENCODED_BYTES,
            rate_bytes_per_second: RATE_BYTES_PER_SECOND,
            rate_burst_bytes: RATE_BURST_BYTES,
        }
    }
}

impl RelayQuota {
    pub fn validate(self) -> Result<Self, RelayProtocolError> {
        if self.queue_messages == 0
            || self.queue_messages > MAX_QUEUE_MESSAGES
            || self.queue_encoded_bytes == 0
            || self.queue_encoded_bytes > MAX_QUEUE_ENCODED_BYTES
            || self.rate_bytes_per_second == 0
            || self.rate_bytes_per_second > RATE_BYTES_PER_SECOND
            || self.rate_burst_bytes == 0
            || self.rate_burst_bytes > RATE_BURST_BYTES
            || self.rate_burst_bytes < self.rate_bytes_per_second
        {
            return Err(RelayProtocolError::InvalidQuota);
        }
        Ok(self)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct RelayEnvelopeV1 {
    route_id: RelayRouteId,
    direction: RelayDirection,
    sequence: RelayConnectionSequence,
    ciphertext: Vec<u8>,
}

impl fmt::Debug for RelayEnvelopeV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RelayEnvelopeV1")
            .field("route_id", &"[REDACTED]")
            .field("direction", &self.direction)
            .field("sequence", &self.sequence)
            .field("ciphertext_bytes", &self.ciphertext.len())
            .finish()
    }
}

impl RelayEnvelopeV1 {
    pub fn new(
        route_id: RelayRouteId,
        direction: RelayDirection,
        sequence: RelayConnectionSequence,
        ciphertext: Vec<u8>,
    ) -> Result<Self, RelayProtocolError> {
        if ciphertext.is_empty() || ciphertext.len() > MAX_CIPHERTEXT_PAYLOAD_BYTES {
            return Err(RelayProtocolError::FrameLimit);
        }
        Ok(Self {
            route_id,
            direction,
            sequence,
            ciphertext,
        })
    }

    pub fn route_id(&self) -> RelayRouteId {
        self.route_id
    }

    pub fn direction(&self) -> RelayDirection {
        self.direction
    }

    pub fn sequence(&self) -> RelayConnectionSequence {
        self.sequence
    }

    pub fn ciphertext(&self) -> &[u8] {
        &self.ciphertext
    }

    pub fn encoded_len(&self) -> usize {
        RELAY_ENVELOPE_HEADER_BYTES + self.ciphertext.len()
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.encoded_len());
        bytes.extend_from_slice(&ENVELOPE_MAGIC);
        bytes.extend_from_slice(&RELAY_V1.0.to_be_bytes());
        bytes.push(self.direction as u8);
        bytes.push(0);
        bytes.extend_from_slice(&self.route_id.0);
        bytes.extend_from_slice(&self.sequence.0.to_be_bytes());
        bytes.extend_from_slice(&(self.ciphertext.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&self.ciphertext);
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, RelayProtocolError> {
        if bytes.len() < RELAY_ENVELOPE_HEADER_BYTES || bytes[..4] != ENVELOPE_MAGIC {
            return Err(RelayProtocolError::InvalidEnvelope);
        }
        require_v1(u16::from_be_bytes(read_array(bytes, 4)?))?;
        if bytes[7] != 0 {
            return Err(RelayProtocolError::NonCanonical);
        }
        let direction = decode_direction(bytes[6])?;
        let route_id = RelayRouteId(read_array(bytes, 8)?);
        let sequence = RelayConnectionSequence(u64::from_be_bytes(read_array(bytes, 40)?));
        let ciphertext_len = u32::from_be_bytes(read_array(bytes, 48)?) as usize;
        if ciphertext_len == 0
            || ciphertext_len > MAX_CIPHERTEXT_PAYLOAD_BYTES
            || bytes.len() != RELAY_ENVELOPE_HEADER_BYTES + ciphertext_len
            || bytes.len() > MAX_ENCODED_WEBSOCKET_MESSAGE_BYTES
        {
            return Err(RelayProtocolError::FrameLimit);
        }
        Self::new(
            route_id,
            direction,
            sequence,
            bytes[RELAY_ENVELOPE_HEADER_BYTES..].to_vec(),
        )
    }
}

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct RelayAdmissionCredential {
    secret: [u8; 32],
}

impl fmt::Debug for RelayAdmissionCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RelayAdmissionCredential([REDACTED])")
    }
}

impl RelayAdmissionCredential {
    pub fn generate() -> Self {
        let mut secret = [0_u8; 32];
        OsRng.fill_bytes(&mut secret);
        Self { secret }
    }

    pub fn from_fixture_bytes(secret: [u8; 32]) -> Self {
        Self { secret }
    }

    /// Reconstructs a credential loaded from a protected credential store.
    ///
    /// The caller remains responsible for zeroizing the source bytes. This type deliberately
    /// provides no inverse operation and implements neither `Clone` nor serialization.
    pub fn from_secret_bytes(secret: [u8; 32]) -> Self {
        Self { secret }
    }

    pub fn verifier(&self) -> RelayCredentialVerifier {
        RelayCredentialVerifier(
            SigningKey::from_bytes(&self.secret)
                .verifying_key()
                .to_bytes(),
        )
    }

    pub fn prove(&self, challenge: &RelayAdmissionChallenge) -> RelayAdmissionProof {
        let signature = SigningKey::from_bytes(&self.secret)
            .sign(&challenge.signing_bytes())
            .to_bytes();
        RelayAdmissionProof {
            route_id: challenge.route_id,
            role: challenge.role,
            serial: challenge.serial,
            signature,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RelayClientHello {
    pub route_id: RelayRouteId,
    pub role: RelayEndpointRole,
}

impl RelayClientHello {
    pub fn encode(self) -> [u8; CLIENT_HELLO_BYTES] {
        let mut bytes = [0_u8; CLIENT_HELLO_BYTES];
        bytes[..4].copy_from_slice(&HELLO_MAGIC);
        bytes[4..6].copy_from_slice(&RELAY_V1.0.to_be_bytes());
        bytes[6] = self.role as u8;
        bytes[8..].copy_from_slice(&self.route_id.0);
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, RelayProtocolError> {
        if bytes.len() != CLIENT_HELLO_BYTES || bytes[..4] != HELLO_MAGIC || bytes[7] != 0 {
            return Err(RelayProtocolError::InvalidAdmissionMessage);
        }
        require_v1(u16::from_be_bytes(read_array(bytes, 4)?))?;
        Ok(Self {
            route_id: RelayRouteId(read_array(bytes, 8)?),
            role: decode_role(bytes[6])?,
        })
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct RelayAdmissionChallenge {
    pub route_id: RelayRouteId,
    pub role: RelayEndpointRole,
    pub verifier: RelayCredentialVerifier,
    pub revocation_epoch: RelayRevocationEpoch,
    pub serial: u64,
    pub expires_at_unix_seconds: u64,
    pub nonce: [u8; 32],
}

impl fmt::Debug for RelayAdmissionChallenge {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RelayAdmissionChallenge")
            .field("route_id", &"[REDACTED]")
            .field("role", &self.role)
            .field("verifier", &"[REDACTED]")
            .field("revocation_epoch", &self.revocation_epoch)
            .field("serial", &self.serial)
            .field("expires_at_unix_seconds", &self.expires_at_unix_seconds)
            .field("nonce", &"[REDACTED]")
            .finish()
    }
}

impl RelayAdmissionChallenge {
    pub fn encode(&self) -> [u8; ADMISSION_CHALLENGE_BYTES] {
        let mut bytes = [0_u8; ADMISSION_CHALLENGE_BYTES];
        bytes[..4].copy_from_slice(&CHALLENGE_MAGIC);
        bytes[4..6].copy_from_slice(&RELAY_V1.0.to_be_bytes());
        bytes[6] = self.role as u8;
        bytes[8..40].copy_from_slice(&self.route_id.0);
        bytes[40..72].copy_from_slice(&self.verifier.0);
        bytes[72..80].copy_from_slice(&self.revocation_epoch.0.to_be_bytes());
        bytes[80..88].copy_from_slice(&self.serial.to_be_bytes());
        bytes[88..96].copy_from_slice(&self.expires_at_unix_seconds.to_be_bytes());
        bytes[96..128].copy_from_slice(&self.nonce);
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, RelayProtocolError> {
        if bytes.len() != ADMISSION_CHALLENGE_BYTES
            || bytes[..4] != CHALLENGE_MAGIC
            || bytes[7] != 0
        {
            return Err(RelayProtocolError::InvalidAdmissionMessage);
        }
        require_v1(u16::from_be_bytes(read_array(bytes, 4)?))?;
        Ok(Self {
            route_id: RelayRouteId(read_array(bytes, 8)?),
            role: decode_role(bytes[6])?,
            verifier: RelayCredentialVerifier(read_array(bytes, 40)?),
            revocation_epoch: RelayRevocationEpoch(u64::from_be_bytes(read_array(bytes, 72)?)),
            serial: u64::from_be_bytes(read_array(bytes, 80)?),
            expires_at_unix_seconds: u64::from_be_bytes(read_array(bytes, 88)?),
            nonce: read_array(bytes, 96)?,
        })
    }

    pub fn signing_bytes(&self) -> Vec<u8> {
        let encoded = self.encode();
        let mut transcript = Vec::with_capacity(ADMISSION_DOMAIN.len() + encoded.len());
        transcript.extend_from_slice(ADMISSION_DOMAIN);
        transcript.extend_from_slice(&encoded);
        transcript
    }
}

pub struct RelayAdmissionProof {
    pub route_id: RelayRouteId,
    pub role: RelayEndpointRole,
    pub serial: u64,
    signature: [u8; 64],
}

impl fmt::Debug for RelayAdmissionProof {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RelayAdmissionProof([REDACTED])")
    }
}

impl RelayAdmissionProof {
    pub fn encode(&self) -> [u8; ADMISSION_PROOF_BYTES] {
        let mut bytes = [0_u8; ADMISSION_PROOF_BYTES];
        bytes[..4].copy_from_slice(&PROOF_MAGIC);
        bytes[4..6].copy_from_slice(&RELAY_V1.0.to_be_bytes());
        bytes[6] = self.role as u8;
        bytes[8..40].copy_from_slice(&self.route_id.0);
        bytes[40..48].copy_from_slice(&self.serial.to_be_bytes());
        bytes[48..].copy_from_slice(&self.signature);
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, RelayProtocolError> {
        if bytes.len() != ADMISSION_PROOF_BYTES || bytes[..4] != PROOF_MAGIC || bytes[7] != 0 {
            return Err(RelayProtocolError::InvalidAdmissionMessage);
        }
        require_v1(u16::from_be_bytes(read_array(bytes, 4)?))?;
        Ok(Self {
            route_id: RelayRouteId(read_array(bytes, 8)?),
            role: decode_role(bytes[6])?,
            serial: u64::from_be_bytes(read_array(bytes, 40)?),
            signature: read_array(bytes, 48)?,
        })
    }

    pub fn verify(&self, challenge: &RelayAdmissionChallenge) -> Result<(), RelayProtocolError> {
        if self.route_id != challenge.route_id
            || self.role != challenge.role
            || self.serial != challenge.serial
        {
            return Err(RelayProtocolError::InvalidProof);
        }
        let key = VerifyingKey::from_bytes(&challenge.verifier.0)
            .map_err(|_| RelayProtocolError::InvalidVerifier)?;
        key.verify(
            &challenge.signing_bytes(),
            &Signature::from_bytes(&self.signature),
        )
        .map_err(|_| RelayProtocolError::InvalidProof)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RelayAdmissionResult {
    pub diagnostic: RelayDiagnosticCode,
    pub connection_id: Option<RelayConnectionId>,
}

impl RelayAdmissionResult {
    pub fn accepted(connection_id: RelayConnectionId) -> Self {
        Self {
            diagnostic: RelayDiagnosticCode::Ready,
            connection_id: Some(connection_id),
        }
    }

    pub fn rejected(diagnostic: RelayDiagnosticCode) -> Self {
        Self {
            diagnostic,
            connection_id: None,
        }
    }

    pub fn encode(self) -> [u8; ADMISSION_RESULT_BYTES] {
        let mut bytes = [0_u8; ADMISSION_RESULT_BYTES];
        bytes[..4].copy_from_slice(&RESULT_MAGIC);
        bytes[4..6].copy_from_slice(&RELAY_V1.0.to_be_bytes());
        bytes[6..8].copy_from_slice(&self.diagnostic.as_u16().to_be_bytes());
        bytes[8..].copy_from_slice(
            &self
                .connection_id
                .unwrap_or(RelayConnectionId(0))
                .0
                .to_be_bytes(),
        );
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, RelayProtocolError> {
        if bytes.len() != ADMISSION_RESULT_BYTES || bytes[..4] != RESULT_MAGIC {
            return Err(RelayProtocolError::InvalidAdmissionMessage);
        }
        require_v1(u16::from_be_bytes(read_array(bytes, 4)?))?;
        let diagnostic = RelayDiagnosticCode::from_u16(u16::from_be_bytes(read_array(bytes, 6)?))?;
        let raw_id = u64::from_be_bytes(read_array(bytes, 8)?);
        let connection_id =
            (diagnostic == RelayDiagnosticCode::Ready).then_some(RelayConnectionId(raw_id));
        if connection_id.is_some_and(|id| id.0 == 0) || (connection_id.is_none() && raw_id != 0) {
            return Err(RelayProtocolError::NonCanonical);
        }
        Ok(Self {
            diagnostic,
            connection_id,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u16)]
pub enum RelayDiagnosticCode {
    Ready = 0,
    InvalidConfig = 1,
    LoopbackRequired = 2,
    StateLocked = 3,
    StateCorrupt = 4,
    StateVersionUnsupported = 5,
    StatePermissionDenied = 6,
    StateWriteFailed = 7,
    InvalidUpgrade = 8,
    OriginRejected = 9,
    HandshakeLimit = 10,
    AdmissionRateLimited = 11,
    UnknownRoute = 12,
    InvalidProof = 13,
    ReplayedProof = 14,
    ExpiredProof = 15,
    Revoked = 16,
    DuplicateRole = 17,
    PairLimit = 18,
    VersionMismatch = 19,
    MalformedEnvelope = 20,
    FrameLimit = 21,
    RouteMismatch = 22,
    DirectionMismatch = 23,
    SequenceReplay = 24,
    SequenceGap = 25,
    PeerOffline = 26,
    QueueLimit = 27,
    RateLimit = 28,
    IdleTimeout = 29,
    PeerDisconnected = 30,
    RevokedLive = 31,
    Shutdown = 32,
    TransportFailed = 33,
    Internal = 34,
}

impl RelayDiagnosticCode {
    pub const ALL: [Self; 35] = [
        Self::Ready,
        Self::InvalidConfig,
        Self::LoopbackRequired,
        Self::StateLocked,
        Self::StateCorrupt,
        Self::StateVersionUnsupported,
        Self::StatePermissionDenied,
        Self::StateWriteFailed,
        Self::InvalidUpgrade,
        Self::OriginRejected,
        Self::HandshakeLimit,
        Self::AdmissionRateLimited,
        Self::UnknownRoute,
        Self::InvalidProof,
        Self::ReplayedProof,
        Self::ExpiredProof,
        Self::Revoked,
        Self::DuplicateRole,
        Self::PairLimit,
        Self::VersionMismatch,
        Self::MalformedEnvelope,
        Self::FrameLimit,
        Self::RouteMismatch,
        Self::DirectionMismatch,
        Self::SequenceReplay,
        Self::SequenceGap,
        Self::PeerOffline,
        Self::QueueLimit,
        Self::RateLimit,
        Self::IdleTimeout,
        Self::PeerDisconnected,
        Self::RevokedLive,
        Self::Shutdown,
        Self::TransportFailed,
        Self::Internal,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "relay_ready",
            Self::InvalidConfig => "relay_invalid_config",
            Self::LoopbackRequired => "relay_loopback_required",
            Self::StateLocked => "relay_state_locked",
            Self::StateCorrupt => "relay_state_corrupt",
            Self::StateVersionUnsupported => "relay_state_version_unsupported",
            Self::StatePermissionDenied => "relay_state_permission_denied",
            Self::StateWriteFailed => "relay_state_write_failed",
            Self::InvalidUpgrade => "relay_invalid_upgrade",
            Self::OriginRejected => "relay_origin_rejected",
            Self::HandshakeLimit => "relay_handshake_limit",
            Self::AdmissionRateLimited => "relay_admission_rate_limited",
            Self::UnknownRoute => "relay_unknown_route",
            Self::InvalidProof => "relay_invalid_proof",
            Self::ReplayedProof => "relay_replayed_proof",
            Self::ExpiredProof => "relay_expired_proof",
            Self::Revoked => "relay_revoked",
            Self::DuplicateRole => "relay_duplicate_role",
            Self::PairLimit => "relay_pair_limit",
            Self::VersionMismatch => "relay_version_mismatch",
            Self::MalformedEnvelope => "relay_malformed_envelope",
            Self::FrameLimit => "relay_frame_limit",
            Self::RouteMismatch => "relay_route_mismatch",
            Self::DirectionMismatch => "relay_direction_mismatch",
            Self::SequenceReplay => "relay_sequence_replay",
            Self::SequenceGap => "relay_sequence_gap",
            Self::PeerOffline => "relay_peer_offline",
            Self::QueueLimit => "relay_queue_limit",
            Self::RateLimit => "relay_rate_limit",
            Self::IdleTimeout => "relay_idle_timeout",
            Self::PeerDisconnected => "relay_peer_disconnected",
            Self::RevokedLive => "relay_revoked_live",
            Self::Shutdown => "relay_shutdown",
            Self::TransportFailed => "relay_transport_failed",
            Self::Internal => "relay_internal",
        }
    }

    pub fn as_u16(self) -> u16 {
        self as u16
    }

    pub fn from_u16(value: u16) -> Result<Self, RelayProtocolError> {
        Self::ALL
            .into_iter()
            .find(|code| code.as_u16() == value)
            .ok_or(RelayProtocolError::UnknownDiagnostic)
    }
}

impl fmt::Display for RelayDiagnosticCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelayProtocolError {
    InvalidEnvelope,
    InvalidAdmissionMessage,
    VersionMismatch,
    NonCanonical,
    FrameLimit,
    InvalidQuota,
    InvalidVerifier,
    InvalidProof,
    UnknownDiagnostic,
}

impl fmt::Display for RelayProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidEnvelope => "invalid_envelope",
            Self::InvalidAdmissionMessage => "invalid_admission_message",
            Self::VersionMismatch => "version_mismatch",
            Self::NonCanonical => "non_canonical",
            Self::FrameLimit => "frame_limit",
            Self::InvalidQuota => "invalid_quota",
            Self::InvalidVerifier => "invalid_verifier",
            Self::InvalidProof => "invalid_proof",
            Self::UnknownDiagnostic => "unknown_diagnostic",
        })
    }
}

impl std::error::Error for RelayProtocolError {}

fn require_v1(version: u16) -> Result<(), RelayProtocolError> {
    if version == RELAY_V1.0 {
        Ok(())
    } else {
        Err(RelayProtocolError::VersionMismatch)
    }
}

fn decode_role(value: u8) -> Result<RelayEndpointRole, RelayProtocolError> {
    match value {
        1 => Ok(RelayEndpointRole::Host),
        2 => Ok(RelayEndpointRole::Controller),
        _ => Err(RelayProtocolError::InvalidAdmissionMessage),
    }
}

fn decode_direction(value: u8) -> Result<RelayDirection, RelayProtocolError> {
    match value {
        1 => Ok(RelayDirection::HostToController),
        2 => Ok(RelayDirection::ControllerToHost),
        _ => Err(RelayProtocolError::InvalidEnvelope),
    }
}

fn read_array<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N], RelayProtocolError> {
    bytes
        .get(offset..offset + N)
        .ok_or(RelayProtocolError::NonCanonical)?
        .try_into()
        .map_err(|_| RelayProtocolError::NonCanonical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn route(byte: u8) -> RelayRouteId {
        RelayRouteId([byte; 32])
    }

    #[test]
    fn envelope_boundaries_are_exact() {
        for size in [
            MAX_CIPHERTEXT_PAYLOAD_BYTES - 1,
            MAX_CIPHERTEXT_PAYLOAD_BYTES,
        ] {
            let frame = RelayEnvelopeV1::new(
                route(1),
                RelayDirection::HostToController,
                RelayConnectionSequence(0),
                vec![7; size],
            )
            .unwrap();
            assert_eq!(RelayEnvelopeV1::decode(&frame.encode()).unwrap(), frame);
        }
        assert_eq!(
            RelayEnvelopeV1::new(
                route(1),
                RelayDirection::HostToController,
                RelayConnectionSequence(0),
                vec![0; MAX_CIPHERTEXT_PAYLOAD_BYTES + 1],
            ),
            Err(RelayProtocolError::FrameLimit)
        );
    }

    #[test]
    fn websocket_message_boundaries_are_exact() {
        assert!(validate_websocket_message_len(MAX_ENCODED_WEBSOCKET_MESSAGE_BYTES - 1).is_ok());
        assert!(validate_websocket_message_len(MAX_ENCODED_WEBSOCKET_MESSAGE_BYTES).is_ok());
        assert_eq!(
            validate_websocket_message_len(MAX_ENCODED_WEBSOCKET_MESSAGE_BYTES + 1),
            Err(RelayProtocolError::FrameLimit)
        );
    }

    #[test]
    fn admission_transcript_binds_verifier_role_route_epoch_expiry_and_nonce() {
        let credential = RelayAdmissionCredential::from_fixture_bytes([0x11; 32]);
        let challenge = RelayAdmissionChallenge {
            route_id: route(0x22),
            role: RelayEndpointRole::Host,
            verifier: credential.verifier(),
            revocation_epoch: RelayRevocationEpoch(7),
            serial: 9,
            expires_at_unix_seconds: 1234,
            nonce: [0x33; 32],
        };
        let proof = credential.prove(&challenge);
        proof.verify(&challenge).unwrap();
        let mut mutated = challenge.clone();
        mutated.role = RelayEndpointRole::Controller;
        assert_eq!(
            proof.verify(&mutated),
            Err(RelayProtocolError::InvalidProof)
        );
        assert_eq!(
            RelayAdmissionChallenge::decode(&challenge.encode()).unwrap(),
            challenge
        );
        let decoded_proof = RelayAdmissionProof::decode(&proof.encode()).unwrap();
        decoded_proof.verify(&challenge).unwrap();
    }

    #[test]
    fn debug_and_diagnostics_are_stable_and_redacted() {
        let credential = RelayAdmissionCredential::from_fixture_bytes([0xAB; 32]);
        let route = route(0xCD);
        let challenge = RelayAdmissionChallenge {
            route_id: route,
            role: RelayEndpointRole::Host,
            verifier: credential.verifier(),
            revocation_epoch: RelayRevocationEpoch(0),
            serial: 1,
            expires_at_unix_seconds: 30,
            nonce: [0xEF; 32],
        };
        let text = format!(
            "{credential:?} {route:?} {challenge:?} {:?}",
            credential.prove(&challenge)
        );
        assert!(!text.contains(&hex::encode(route.0)));
        assert!(!text.contains(&hex::encode([0xAB; 32])));
        assert!(text.contains("[REDACTED]"));
        for code in RelayDiagnosticCode::ALL {
            assert_eq!(RelayDiagnosticCode::from_u16(code.as_u16()).unwrap(), code);
            assert!(code.as_str().starts_with("relay_"));
        }
    }

    proptest! {
        #[test]
        fn envelope_codec_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..4096)) {
            let _ = RelayEnvelopeV1::decode(&bytes);
        }

        #[test]
        fn admission_codecs_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
            let _ = RelayClientHello::decode(&bytes);
            let _ = RelayAdmissionChallenge::decode(&bytes);
            let _ = RelayAdmissionProof::decode(&bytes);
            let _ = RelayAdmissionResult::decode(&bytes);
        }
    }
}
