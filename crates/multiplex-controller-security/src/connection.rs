use core::fmt;
use core::num::NonZeroU32;

use clatter::KeyPair;
use clatter::NqHandshakeCore;
use clatter::bytearray::{ByteArray, SensitiveByteArray};
use clatter::crypto::cipher::ChaChaPoly;
use clatter::crypto::dh::X25519;
use clatter::crypto::hash::Blake2s;
use clatter::handshakepattern::noise_ik;
use clatter::traits::{Dh, Handshaker};
use rand_core::{CryptoRng, Error as RngError, RngCore};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::authorization::AuthorizationPolicy;
use crate::error::{ErrorCode, Result};
use crate::transport::ControllerTransport;
use crate::types::{
    CapabilitySet, ControllerProtocolVersion, DeviceStaticPublicKey, HANDSHAKE_TIMEOUT_MILLIS,
    HandshakeMessage, HostStaticPublicKey, RevocationEpoch, StaticPrivateKey,
};

pub const NOISE_CONNECTION_PROTOCOL_NAME: &str = "Noise_IK_25519_ChaChaPoly_BLAKE2s";

const CONNECTION_PROLOGUE_DOMAIN: &[u8] = b"termirust-controller-connection-v1\0";
const CONNECTION_PAYLOAD_MAGIC: [u8; 4] = *b"TRC1";
const CONNECTION_PAYLOAD_BYTES: usize = 27;
const CONNECTION_MESSAGE_MAX_BYTES: usize = CONNECTION_PAYLOAD_BYTES + 96;
const DEVICE_ROLE: u8 = 1;
const HOST_ROLE: u8 = 2;

type NoiseHandshake = NqHandshakeCore<X25519, ChaChaPoly, Blake2s, NoFallbackRng>;

#[derive(Clone, Default)]
struct NoFallbackRng;

impl RngCore for NoFallbackRng {
    fn next_u32(&mut self) -> u32 {
        0
    }

    fn next_u64(&mut self) -> u64 {
        0
    }

    fn fill_bytes(&mut self, destination: &mut [u8]) {
        destination.fill(0);
    }

    fn try_fill_bytes(&mut self, _: &mut [u8]) -> core::result::Result<(), RngError> {
        Err(RngError::from(
            NonZeroU32::new(u32::MAX).unwrap_or(NonZeroU32::MIN),
        ))
    }
}

impl CryptoRng for NoFallbackRng {}

#[derive(Clone, Eq, PartialEq, Zeroize, ZeroizeOnDrop)]
pub struct ConnectionPrelude {
    #[zeroize(skip)]
    pub version: ControllerProtocolVersion,
    #[zeroize(skip)]
    pub identity_generation: u64,
    #[zeroize(skip)]
    pub revocation_epoch: RevocationEpoch,
    pub client_nonce: [u8; 32],
}

impl fmt::Debug for ConnectionPrelude {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConnectionPrelude")
            .field("version", &self.version)
            .field("identity_generation", &self.identity_generation)
            .field("revocation_epoch", &self.revocation_epoch)
            .field("client_nonce", &"[REDACTED]")
            .finish()
    }
}

impl ConnectionPrelude {
    pub const ENCODED_BYTES: usize = 56;

    pub fn encode(&self) -> Result<[u8; Self::ENCODED_BYTES]> {
        self.validate()?;
        let mut bytes = [0_u8; Self::ENCODED_BYTES];
        bytes[..4].copy_from_slice(b"TCP1");
        bytes[4..6].copy_from_slice(&self.version.major.to_be_bytes());
        bytes[6..8].copy_from_slice(&self.version.minor.to_be_bytes());
        bytes[8..16].copy_from_slice(&self.identity_generation.to_be_bytes());
        bytes[16..24].copy_from_slice(&self.revocation_epoch.0.to_be_bytes());
        bytes[24..].copy_from_slice(&self.client_nonce);
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != Self::ENCODED_BYTES || bytes[..4] != *b"TCP1" {
            return Err(ErrorCode::InvalidEncoding.into());
        }
        let prelude = Self {
            version: ControllerProtocolVersion {
                major: u16::from_be_bytes([bytes[4], bytes[5]]),
                minor: u16::from_be_bytes([bytes[6], bytes[7]]),
            },
            identity_generation: u64::from_be_bytes(
                bytes[8..16]
                    .try_into()
                    .map_err(|_| ErrorCode::InvalidEncoding)?,
            ),
            revocation_epoch: RevocationEpoch(u64::from_be_bytes(
                bytes[16..24]
                    .try_into()
                    .map_err(|_| ErrorCode::InvalidEncoding)?,
            )),
            client_nonce: bytes[24..]
                .try_into()
                .map_err(|_| ErrorCode::InvalidEncoding)?,
        };
        prelude.validate()?;
        Ok(prelude)
    }

    fn validate(&self) -> Result<()> {
        self.version.require_v1()?;
        if self.identity_generation == 0 || self.client_nonce == [0; 32] {
            return Err(ErrorCode::InvalidEncoding.into());
        }
        Ok(())
    }
}

#[derive(Clone, Eq, PartialEq, Zeroize, ZeroizeOnDrop)]
pub struct ConnectionChallenge {
    pub server_nonce: [u8; 32],
}

impl fmt::Debug for ConnectionChallenge {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConnectionChallenge")
            .field("server_nonce", &"[REDACTED]")
            .finish()
    }
}

impl ConnectionChallenge {
    pub const ENCODED_BYTES: usize = 36;

    pub fn encode(&self) -> Result<[u8; Self::ENCODED_BYTES]> {
        if self.server_nonce == [0; 32] {
            return Err(ErrorCode::InvalidEncoding.into());
        }
        let mut bytes = [0_u8; Self::ENCODED_BYTES];
        bytes[..4].copy_from_slice(b"TCC1");
        bytes[4..].copy_from_slice(&self.server_nonce);
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != Self::ENCODED_BYTES || bytes[..4] != *b"TCC1" {
            return Err(ErrorCode::InvalidEncoding.into());
        }
        let challenge = Self {
            server_nonce: bytes[4..]
                .try_into()
                .map_err(|_| ErrorCode::InvalidEncoding)?,
        };
        challenge.encode()?;
        Ok(challenge)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AuthenticatedPeerClaim {
    pub device_key: DeviceStaticPublicKey,
    pub requested_capabilities: CapabilitySet,
    pub identity_generation: u64,
    pub revocation_epoch: RevocationEpoch,
}

pub struct AuthenticatedConnection {
    pub host_key: HostStaticPublicKey,
    pub device_key: DeviceStaticPublicKey,
    pub identity_generation: u64,
    pub revocation_epoch: RevocationEpoch,
    pub capabilities: CapabilitySet,
    pub transport: ControllerTransport,
}

impl fmt::Debug for AuthenticatedConnection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthenticatedConnection")
            .field("host_key", &self.host_key)
            .field("device_key", &self.device_key)
            .field("identity_generation", &self.identity_generation)
            .field("revocation_epoch", &self.revocation_epoch)
            .field("capabilities", &self.capabilities)
            .field("transport", &"[REDACTED]")
            .finish()
    }
}

pub struct ConnectionInitiator {
    prelude: ConnectionPrelude,
    host_key: HostStaticPublicKey,
    device_key: DeviceStaticPublicKey,
    requested_capabilities: CapabilitySet,
    deadline_millis: u64,
    noise: Option<NoiseHandshake>,
    hello_written: bool,
}

impl fmt::Debug for ConnectionInitiator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConnectionInitiator")
            .field("hello_written", &self.hello_written)
            .finish_non_exhaustive()
    }
}

impl ConnectionInitiator {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        prelude: ConnectionPrelude,
        challenge: &ConnectionChallenge,
        host_key: HostStaticPublicKey,
        device_static_private: StaticPrivateKey,
        device_ephemeral_private: StaticPrivateKey,
        requested_capabilities: CapabilitySet,
        now_millis: u64,
    ) -> Result<Self> {
        prelude.validate()?;
        challenge.encode()?;
        let static_pair = keypair(&device_static_private);
        let device_key = DeviceStaticPublicKey(static_pair.public);
        let noise = NoiseHandshake::new(
            noise_ik(),
            &connection_prologue(&prelude, challenge, host_key),
            true,
            Some(static_pair),
            Some(keypair(&device_ephemeral_private)),
            Some(host_key.0),
            None,
        )
        .map_err(|_| ErrorCode::CryptoFailure)?;
        Ok(Self {
            prelude,
            host_key,
            device_key,
            requested_capabilities,
            deadline_millis: now_millis.saturating_add(HANDSHAKE_TIMEOUT_MILLIS),
            noise: Some(noise),
            hello_written: false,
        })
    }

    pub fn write_hello(&mut self, now_millis: u64) -> Result<HandshakeMessage> {
        self.check_deadline(now_millis)?;
        if self.hello_written {
            return Err(ErrorCode::WrongState.into());
        }
        let payload =
            encode_connection_payload(DEVICE_ROLE, &self.prelude, self.requested_capabilities);
        let noise = self.noise.as_mut().ok_or(ErrorCode::WrongState)?;
        let mut message = vec![0_u8; CONNECTION_MESSAGE_MAX_BYTES];
        let length = noise
            .write_message(&payload, &mut message)
            .map_err(|_| ErrorCode::CryptoFailure)?;
        message.truncate(length);
        self.hello_written = true;
        Ok(HandshakeMessage::new(message))
    }

    pub fn read_accept(
        mut self,
        message: &[u8],
        now_millis: u64,
    ) -> Result<AuthenticatedConnection> {
        self.check_deadline(now_millis)?;
        if !self.hello_written || message.len() > CONNECTION_MESSAGE_MAX_BYTES {
            return Err(ErrorCode::WrongState.into());
        }
        let mut payload = [0_u8; CONNECTION_PAYLOAD_BYTES];
        let mut noise = self.noise.take().ok_or(ErrorCode::WrongState)?;
        let length = noise
            .read_message(message, &mut payload)
            .map_err(|_| ErrorCode::AuthenticationFailed)?;
        if length != CONNECTION_PAYLOAD_BYTES || !noise.is_finished() {
            return Err(ErrorCode::InvalidEncoding.into());
        }
        let granted = decode_connection_payload(HOST_ROLE, &self.prelude, &payload)?;
        if granted.bits() & !self.requested_capabilities.bits() != 0 {
            return Err(ErrorCode::CapabilityDenied.into());
        }
        if noise.get_remote_static() != Some(self.host_key.0) {
            return Err(ErrorCode::WrongKey.into());
        }
        finalize_connection(
            noise,
            true,
            self.host_key,
            self.device_key,
            self.prelude.identity_generation,
            self.prelude.revocation_epoch,
            granted,
        )
    }

    fn check_deadline(&self, now_millis: u64) -> Result<()> {
        if now_millis > self.deadline_millis {
            Err(ErrorCode::TimedOut.into())
        } else {
            Ok(())
        }
    }
}

pub struct ConnectionResponder {
    prelude: ConnectionPrelude,
    host_key: HostStaticPublicKey,
    deadline_millis: u64,
    noise: Option<NoiseHandshake>,
    claim: Option<AuthenticatedPeerClaim>,
}

impl fmt::Debug for ConnectionResponder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConnectionResponder")
            .field("has_claim", &self.claim.is_some())
            .finish_non_exhaustive()
    }
}

impl ConnectionResponder {
    pub fn new(
        prelude: ConnectionPrelude,
        challenge: &ConnectionChallenge,
        host_static_private: StaticPrivateKey,
        host_ephemeral_private: StaticPrivateKey,
        now_millis: u64,
    ) -> Result<Self> {
        prelude.validate()?;
        challenge.encode()?;
        let static_pair = keypair(&host_static_private);
        let host_key = HostStaticPublicKey(static_pair.public);
        let noise = NoiseHandshake::new(
            noise_ik(),
            &connection_prologue(&prelude, challenge, host_key),
            false,
            Some(static_pair),
            Some(keypair(&host_ephemeral_private)),
            None,
            None,
        )
        .map_err(|_| ErrorCode::CryptoFailure)?;
        Ok(Self {
            prelude,
            host_key,
            deadline_millis: now_millis.saturating_add(HANDSHAKE_TIMEOUT_MILLIS),
            noise: Some(noise),
            claim: None,
        })
    }

    pub fn read_hello(
        &mut self,
        message: &[u8],
        now_millis: u64,
    ) -> Result<AuthenticatedPeerClaim> {
        self.check_deadline(now_millis)?;
        if self.claim.is_some() || message.len() > CONNECTION_MESSAGE_MAX_BYTES {
            return Err(ErrorCode::WrongState.into());
        }
        let mut payload = [0_u8; CONNECTION_PAYLOAD_BYTES];
        let noise = self.noise.as_mut().ok_or(ErrorCode::WrongState)?;
        let length = noise
            .read_message(message, &mut payload)
            .map_err(|_| ErrorCode::AuthenticationFailed)?;
        if length != CONNECTION_PAYLOAD_BYTES {
            return Err(ErrorCode::InvalidEncoding.into());
        }
        let requested = decode_connection_payload(DEVICE_ROLE, &self.prelude, &payload)?;
        let device_key = DeviceStaticPublicKey(
            noise
                .get_remote_static()
                .ok_or(ErrorCode::AuthenticationFailed)?,
        );
        let claim = AuthenticatedPeerClaim {
            device_key,
            requested_capabilities: requested,
            identity_generation: self.prelude.identity_generation,
            revocation_epoch: self.prelude.revocation_epoch,
        };
        self.claim = Some(claim);
        Ok(claim)
    }

    pub fn write_accept(
        mut self,
        granted_capabilities: CapabilitySet,
        now_millis: u64,
    ) -> Result<(HandshakeMessage, AuthenticatedConnection)> {
        self.check_deadline(now_millis)?;
        let claim = self.claim.ok_or(ErrorCode::WrongState)?;
        if granted_capabilities.bits() & !claim.requested_capabilities.bits() != 0 {
            return Err(ErrorCode::CapabilityDenied.into());
        }
        let payload = encode_connection_payload(HOST_ROLE, &self.prelude, granted_capabilities);
        let mut noise = self.noise.take().ok_or(ErrorCode::WrongState)?;
        let mut message = vec![0_u8; CONNECTION_MESSAGE_MAX_BYTES];
        let length = noise
            .write_message(&payload, &mut message)
            .map_err(|_| ErrorCode::CryptoFailure)?;
        message.truncate(length);
        if !noise.is_finished() {
            return Err(ErrorCode::WrongState.into());
        }
        let connection = finalize_connection(
            noise,
            false,
            self.host_key,
            claim.device_key,
            self.prelude.identity_generation,
            self.prelude.revocation_epoch,
            granted_capabilities,
        )?;
        Ok((HandshakeMessage::new(message), connection))
    }

    fn check_deadline(&self, now_millis: u64) -> Result<()> {
        if now_millis > self.deadline_millis {
            Err(ErrorCode::TimedOut.into())
        } else {
            Ok(())
        }
    }
}

fn connection_prologue(
    prelude: &ConnectionPrelude,
    challenge: &ConnectionChallenge,
    host_key: HostStaticPublicKey,
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(CONNECTION_PROLOGUE_DOMAIN.len() + 88);
    bytes.extend_from_slice(CONNECTION_PROLOGUE_DOMAIN);
    bytes.extend_from_slice(&prelude.version.major.to_be_bytes());
    bytes.extend_from_slice(&prelude.version.minor.to_be_bytes());
    bytes.extend_from_slice(&prelude.identity_generation.to_be_bytes());
    bytes.extend_from_slice(&prelude.revocation_epoch.0.to_be_bytes());
    bytes.extend_from_slice(&prelude.client_nonce);
    bytes.extend_from_slice(&challenge.server_nonce);
    bytes.extend_from_slice(&host_key.0);
    bytes
}

fn encode_connection_payload(
    role: u8,
    prelude: &ConnectionPrelude,
    capabilities: CapabilitySet,
) -> [u8; CONNECTION_PAYLOAD_BYTES] {
    let mut bytes = [0_u8; CONNECTION_PAYLOAD_BYTES];
    bytes[..4].copy_from_slice(&CONNECTION_PAYLOAD_MAGIC);
    bytes[4] = role;
    bytes[5..7].copy_from_slice(&prelude.version.major.to_be_bytes());
    bytes[7..9].copy_from_slice(&prelude.version.minor.to_be_bytes());
    bytes[9..17].copy_from_slice(&prelude.identity_generation.to_be_bytes());
    bytes[17..25].copy_from_slice(&prelude.revocation_epoch.0.to_be_bytes());
    bytes[25..27].copy_from_slice(&capabilities.bits().to_be_bytes());
    bytes
}

fn decode_connection_payload(
    expected_role: u8,
    prelude: &ConnectionPrelude,
    bytes: &[u8; CONNECTION_PAYLOAD_BYTES],
) -> Result<CapabilitySet> {
    if bytes[..4] != CONNECTION_PAYLOAD_MAGIC || bytes[4] != expected_role {
        return Err(ErrorCode::InvalidEncoding.into());
    }
    let version = ControllerProtocolVersion {
        major: u16::from_be_bytes([bytes[5], bytes[6]]),
        minor: u16::from_be_bytes([bytes[7], bytes[8]]),
    };
    let generation = u64::from_be_bytes(
        bytes[9..17]
            .try_into()
            .map_err(|_| ErrorCode::InvalidEncoding)?,
    );
    let epoch = RevocationEpoch(u64::from_be_bytes(
        bytes[17..25]
            .try_into()
            .map_err(|_| ErrorCode::InvalidEncoding)?,
    ));
    if version != prelude.version
        || generation != prelude.identity_generation
        || epoch != prelude.revocation_epoch
    {
        return Err(ErrorCode::AuthenticationFailed.into());
    }
    CapabilitySet::from_bits(u16::from_be_bytes([bytes[25], bytes[26]]))
}

#[allow(clippy::too_many_arguments)]
fn finalize_connection(
    noise: NoiseHandshake,
    initiator: bool,
    host_key: HostStaticPublicKey,
    device_key: DeviceStaticPublicKey,
    identity_generation: u64,
    revocation_epoch: RevocationEpoch,
    capabilities: CapabilitySet,
) -> Result<AuthenticatedConnection> {
    let cipherstates = noise
        .finalize()
        .map_err(|_| ErrorCode::CryptoFailure)?
        .take();
    let (send, receive) = if initiator {
        (
            cipherstates.initiator_to_responder,
            cipherstates.responder_to_initiator,
        )
    } else {
        (
            cipherstates.responder_to_initiator,
            cipherstates.initiator_to_responder,
        )
    };
    Ok(AuthenticatedConnection {
        host_key,
        device_key,
        identity_generation,
        revocation_epoch,
        capabilities,
        transport: ControllerTransport::new(
            send,
            receive,
            AuthorizationPolicy::new(capabilities, revocation_epoch),
        ),
    })
}

fn keypair(private: &StaticPrivateKey) -> KeyPair<[u8; 32], SensitiveByteArray<[u8; 32]>> {
    let secret = SensitiveByteArray::from_slice(private.as_bytes());
    let public = X25519::pubkey(&secret);
    KeyPair { public, secret }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ControllerCapability, ControllerFrameKind};

    fn prelude() -> ConnectionPrelude {
        ConnectionPrelude {
            version: crate::CONTROLLER_V1,
            identity_generation: 7,
            revocation_epoch: RevocationEpoch(11),
            client_nonce: [5; 32],
        }
    }

    #[test]
    fn ik_connection_authenticates_both_keys_and_transports_exact_bytes() {
        let host = StaticPrivateKey::from_fixture_bytes([1; 32]);
        let device = StaticPrivateKey::from_fixture_bytes([2; 32]);
        let host_key = crate::host_public_key_from_private(&host);
        let challenge = ConnectionChallenge {
            server_nonce: [6; 32],
        };
        let requested = CapabilitySet::default()
            .with(ControllerCapability::ObserveSessions)
            .with(ControllerCapability::AttachOutput)
            .with(ControllerCapability::SendInput);
        let granted = CapabilitySet::default()
            .with(ControllerCapability::ObserveSessions)
            .with(ControllerCapability::AttachOutput);
        let mut initiator = ConnectionInitiator::new(
            prelude(),
            &challenge,
            host_key,
            device,
            StaticPrivateKey::from_fixture_bytes([3; 32]),
            requested,
            100,
        )
        .unwrap();
        let mut responder = ConnectionResponder::new(
            prelude(),
            &challenge,
            host,
            StaticPrivateKey::from_fixture_bytes([4; 32]),
            100,
        )
        .unwrap();
        let hello = initiator.write_hello(101).unwrap();
        let claim = responder.read_hello(hello.as_bytes(), 102).unwrap();
        assert_eq!(claim.requested_capabilities, requested);
        let (accept, mut host_connection) = responder.write_accept(granted, 103).unwrap();
        assert_eq!(claim.device_key, host_connection.device_key);
        let mut device_connection = initiator.read_accept(accept.as_bytes(), 104).unwrap();
        assert_eq!(device_connection.capabilities, granted);
        let sealed = device_connection
            .transport
            .seal(
                ControllerFrameKind::Control,
                ControllerCapability::ObserveSessions,
                RevocationEpoch(11),
                b"list",
            )
            .unwrap();
        let opened = host_connection.transport.open(sealed.as_bytes()).unwrap();
        assert_eq!(opened.payload, b"list");
    }

    #[test]
    fn ik_connection_rejects_wrong_host_epoch_capability_and_replay() {
        let challenge = ConnectionChallenge {
            server_nonce: [9; 32],
        };
        let requested = CapabilitySet::default().with(ControllerCapability::ObserveSessions);
        let mut initiator = ConnectionInitiator::new(
            prelude(),
            &challenge,
            HostStaticPublicKey([8; 32]),
            StaticPrivateKey::from_fixture_bytes([2; 32]),
            StaticPrivateKey::from_fixture_bytes([3; 32]),
            requested,
            0,
        )
        .unwrap();
        let hello = initiator.write_hello(1).unwrap();
        assert_eq!(
            initiator.write_hello(2).unwrap_err().code(),
            ErrorCode::WrongState
        );
        let mut wrong_epoch = prelude();
        wrong_epoch.revocation_epoch = RevocationEpoch(12);
        let mut responder = ConnectionResponder::new(
            wrong_epoch,
            &challenge,
            StaticPrivateKey::from_fixture_bytes([1; 32]),
            StaticPrivateKey::from_fixture_bytes([4; 32]),
            0,
        )
        .unwrap();
        assert_eq!(
            responder
                .read_hello(hello.as_bytes(), 2)
                .unwrap_err()
                .code(),
            ErrorCode::AuthenticationFailed
        );
    }

    #[test]
    fn connection_prelude_and_challenge_are_exact_and_bounded() {
        let encoded = prelude().encode().unwrap();
        assert_eq!(ConnectionPrelude::decode(&encoded).unwrap(), prelude());
        let challenge = ConnectionChallenge {
            server_nonce: [7; 32],
        };
        assert_eq!(
            ConnectionChallenge::decode(&challenge.encode().unwrap()).unwrap(),
            challenge
        );
        assert!(ConnectionPrelude::decode(&encoded[..55]).is_err());
        assert!(ConnectionChallenge::decode(&[0; 36]).is_err());
    }
}
