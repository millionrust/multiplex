use core::fmt;
use core::num::NonZeroU32;

use clatter::KeyPair;
use clatter::NqHandshakeCore;
use clatter::bytearray::{ByteArray, SensitiveByteArray};
use clatter::crypto::cipher::ChaChaPoly;
use clatter::crypto::dh::X25519;
use clatter::crypto::hash::Blake2s;
use clatter::handshakepattern::noise_xx;
use clatter::traits::{Dh, Handshaker};
use rand_core::{CryptoRng, Error as RngError, RngCore};
use subtle::ConstantTimeEq;

use crate::authorization::AuthorizationPolicy;
use crate::codec::{
    PAIRING_PAYLOAD_BYTES, PairingPayload, decode_pairing_payload, encode_pairing_payload,
    pairing_prologue,
};
use crate::error::{ControllerSecurityError, ErrorCode, Result};
use crate::sas::derive_sas_v1;
use crate::transport::ControllerTransport;
use crate::types::{
    DeviceStaticPublicKey, HANDSHAKE_TIMEOUT_MILLIS, HandshakeHash, HandshakeMessage,
    HostStaticPublicKey, MAX_PAIRING_OFFER_LIFETIME_SECONDS, PairingOfferCore, PairingRole,
    PairingState, PairingStep, RevocationEpoch, SasCode, StaticPrivateKey,
};

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

    fn fill_bytes(&mut self, dest: &mut [u8]) {
        dest.fill(0);
    }

    fn try_fill_bytes(&mut self, _dest: &mut [u8]) -> core::result::Result<(), RngError> {
        Err(RngError::from(
            NonZeroU32::new(u32::MAX).unwrap_or(NonZeroU32::MIN),
        ))
    }
}

impl CryptoRng for NoFallbackRng {}

pub struct PairingMachine {
    role: PairingRole,
    state: PairingState,
    offer: PairingOfferCore,
    device_key: DeviceStaticPublicKey,
    messages_completed: u8,
    deadline_millis: u64,
    noise: Option<NoiseHandshake>,
    handshake_hash: Option<HandshakeHash>,
    sas: Option<SasCode>,
}

impl fmt::Debug for PairingMachine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PairingMachine")
            .field("role", &self.role)
            .field("state", &self.state)
            .field("messages_completed", &self.messages_completed)
            .finish()
    }
}

impl PairingMachine {
    #[allow(clippy::too_many_arguments)]
    pub fn new_device_initiator(
        offer: PairingOfferCore,
        device_static_private: StaticPrivateKey,
        device_ephemeral_private: StaticPrivateKey,
        now_millis: u64,
        now_unix_seconds: u64,
    ) -> Result<Self> {
        validate_offer_time(&offer, now_unix_seconds)?;
        let static_pair = keypair(&device_static_private);
        let device_key = DeviceStaticPublicKey(static_pair.public);
        let ephemeral_pair = keypair(&device_ephemeral_private);
        let prologue = pairing_prologue(&offer)?;
        let noise = NoiseHandshake::new(
            noise_xx(),
            &prologue,
            true,
            Some(static_pair),
            Some(ephemeral_pair),
            None,
            None,
        )
        .map_err(|_| ErrorCode::CryptoFailure)?;
        let deadline_millis = pairing_deadline(&offer, now_millis, now_unix_seconds);
        Ok(Self {
            role: PairingRole::DeviceInitiator,
            state: PairingState::Created,
            offer,
            device_key,
            messages_completed: 0,
            deadline_millis,
            noise: Some(noise),
            handshake_hash: None,
            sas: None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_host_responder(
        offer: PairingOfferCore,
        host_static_private: StaticPrivateKey,
        host_ephemeral_private: StaticPrivateKey,
        now_millis: u64,
        now_unix_seconds: u64,
    ) -> Result<Self> {
        validate_offer_time(&offer, now_unix_seconds)?;
        let static_pair = keypair(&host_static_private);
        if static_pair.public != offer.host_static_public_key.0 {
            return Err(ErrorCode::WrongKey.into());
        }
        let ephemeral_pair = keypair(&host_ephemeral_private);
        let prologue = pairing_prologue(&offer)?;
        let noise = NoiseHandshake::new(
            noise_xx(),
            &prologue,
            false,
            Some(static_pair),
            Some(ephemeral_pair),
            None,
            None,
        )
        .map_err(|_| ErrorCode::CryptoFailure)?;
        let deadline_millis = pairing_deadline(&offer, now_millis, now_unix_seconds);
        Ok(Self {
            role: PairingRole::HostResponder,
            state: PairingState::Created,
            offer,
            device_key: DeviceStaticPublicKey([0; 32]),
            messages_completed: 0,
            deadline_millis,
            noise: Some(noise),
            handshake_hash: None,
            sas: None,
        })
    }

    #[must_use]
    pub const fn state(&self) -> PairingState {
        self.state
    }

    #[must_use]
    pub fn sas(&self) -> Option<&SasCode> {
        self.sas.as_ref()
    }

    #[must_use]
    pub fn handshake_hash(&self) -> Option<&HandshakeHash> {
        self.handshake_hash.as_ref()
    }

    pub fn write_next(&mut self, now_millis: u64) -> Result<HandshakeMessage> {
        self.check_deadline(now_millis)?;
        let (step, expected_role) = match (self.role, self.messages_completed) {
            (PairingRole::DeviceInitiator, 0) => {
                (PairingStep::DeviceHello, PairingRole::DeviceInitiator)
            }
            (PairingRole::HostResponder, 1) => (PairingStep::HostProof, PairingRole::HostResponder),
            (PairingRole::DeviceInitiator, 2) => {
                (PairingStep::DeviceProof, PairingRole::DeviceInitiator)
            }
            _ => return self.fail(ErrorCode::WrongState.into()),
        };
        let payload = self.payload(step, expected_role);
        let encoded = encode_pairing_payload(&payload)?;
        let mut message = vec![0_u8; PAIRING_PAYLOAD_BYTES + 96];
        let Some(noise) = self.noise.as_mut() else {
            return self.fail(ErrorCode::WrongState.into());
        };
        let result = noise
            .write_message(&encoded, &mut message)
            .map_err(|_| ControllerSecurityError::from(ErrorCode::CryptoFailure));
        let length = match result {
            Ok(length) => length,
            Err(error) => return self.fail(error),
        };
        message.truncate(length);
        self.messages_completed += 1;
        self.state = PairingState::Handshaking;
        if self.messages_completed == 3
            && let Err(error) = self.finish_sas()
        {
            return self.fail(error);
        }
        Ok(HandshakeMessage::new(message))
    }

    pub fn read_next(&mut self, message: &[u8], now_millis: u64) -> Result<()> {
        self.check_deadline(now_millis)?;
        let (expected_step, expected_role) = match (self.role, self.messages_completed) {
            (PairingRole::HostResponder, 0) => {
                (PairingStep::DeviceHello, PairingRole::DeviceInitiator)
            }
            (PairingRole::DeviceInitiator, 1) => {
                (PairingStep::HostProof, PairingRole::HostResponder)
            }
            (PairingRole::HostResponder, 2) => {
                (PairingStep::DeviceProof, PairingRole::DeviceInitiator)
            }
            _ => return self.fail(ErrorCode::WrongState.into()),
        };
        if message.len() > PAIRING_PAYLOAD_BYTES + 96 {
            return self.fail(ErrorCode::FrameTooLarge.into());
        }
        let mut payload_bytes = [0_u8; PAIRING_PAYLOAD_BYTES];
        let Some(noise) = self.noise.as_mut() else {
            return self.fail(ErrorCode::WrongState.into());
        };
        let result = noise
            .read_message(message, &mut payload_bytes)
            .map_err(|_| ControllerSecurityError::from(ErrorCode::AuthenticationFailed));
        let payload_len = match result {
            Ok(length) => length,
            Err(error) => return self.fail(error),
        };
        if payload_len != PAIRING_PAYLOAD_BYTES {
            return self.fail(ErrorCode::InvalidEncoding.into());
        }
        let payload = match decode_pairing_payload(&payload_bytes) {
            Ok(payload) => payload,
            Err(error) => return self.fail(error),
        };
        if let Err(error) = self.validate_payload(&payload, expected_step, expected_role) {
            return self.fail(error);
        }
        if expected_step == PairingStep::DeviceHello {
            self.device_key = payload.device_key;
        }
        self.messages_completed += 1;
        self.state = PairingState::Handshaking;
        if expected_step == PairingStep::HostProof {
            let Some(remote) = self.noise.as_ref().and_then(Handshaker::get_remote_static) else {
                return self.fail(ErrorCode::WrongKey.into());
            };
            if remote != self.offer.host_static_public_key.0 {
                return self.fail(ErrorCode::WrongKey.into());
            }
        }
        if self.messages_completed == 3 {
            let Some(remote) = self.noise.as_ref().and_then(Handshaker::get_remote_static) else {
                return self.fail(ErrorCode::WrongKey.into());
            };
            if remote != self.device_key.0 {
                return self.fail(ErrorCode::WrongKey.into());
            }
            if let Err(error) = self.finish_sas() {
                return self.fail(error);
            }
        }
        Ok(())
    }

    pub fn confirm(
        mut self,
        compared_sas: &SasCode,
        revocation_epoch: RevocationEpoch,
    ) -> Result<ConfirmedPairing> {
        if self.state != PairingState::SasReady {
            return Err(ErrorCode::WrongState.into());
        }
        let sas = self.sas.as_ref().ok_or(ErrorCode::WrongState)?;
        if !bool::from(sas.bytes().ct_eq(compared_sas.bytes())) {
            self.state = PairingState::Rejected;
            self.noise = None;
            return Err(ErrorCode::SasMismatch.into());
        }
        let noise = self.noise.take().ok_or(ErrorCode::WrongState)?;
        let cipherstates = noise
            .finalize()
            .map_err(|_| ErrorCode::CryptoFailure)?
            .take();
        let policy = AuthorizationPolicy::new(self.offer.capabilities, revocation_epoch);
        let (send, receive) = match self.role {
            PairingRole::DeviceInitiator => (
                cipherstates.initiator_to_responder,
                cipherstates.responder_to_initiator,
            ),
            PairingRole::HostResponder => (
                cipherstates.responder_to_initiator,
                cipherstates.initiator_to_responder,
            ),
        };
        self.state = PairingState::Confirmed;
        Ok(ConfirmedPairing {
            role: self.role,
            host_key: self.offer.host_static_public_key,
            device_key: self.device_key,
            capabilities: self.offer.capabilities,
            transport: ControllerTransport::new(send, receive, policy),
        })
    }

    pub fn reject(mut self) -> ControllerSecurityError {
        self.state = PairingState::Rejected;
        self.noise = None;
        ErrorCode::Rejected.into()
    }

    pub fn cancel(&mut self) -> ControllerSecurityError {
        self.state = PairingState::Rejected;
        self.noise = None;
        self.handshake_hash = None;
        self.sas = None;
        ErrorCode::Cancelled.into()
    }

    fn payload(&self, step: PairingStep, role: PairingRole) -> PairingPayload {
        PairingPayload {
            step,
            role,
            version: self.offer.version,
            nonce: self.offer.nonce.clone(),
            host_key: self.offer.host_static_public_key,
            device_key: self.device_key,
            capabilities: self.offer.capabilities,
        }
    }

    fn validate_payload(
        &self,
        payload: &PairingPayload,
        step: PairingStep,
        role: PairingRole,
    ) -> Result<()> {
        if payload.step != step {
            return Err(ErrorCode::InvalidStep.into());
        }
        if payload.role != role {
            return Err(ErrorCode::InvalidRole.into());
        }
        if payload.version != self.offer.version {
            return Err(ErrorCode::IncompatibleVersion.into());
        }
        if payload.nonce != self.offer.nonce {
            return Err(ErrorCode::WrongNonce.into());
        }
        if payload.host_key != self.offer.host_static_public_key {
            return Err(ErrorCode::WrongKey.into());
        }
        if self.device_key.0 != [0; 32] && payload.device_key != self.device_key {
            return Err(ErrorCode::WrongKey.into());
        }
        if payload.capabilities != self.offer.capabilities {
            return Err(ErrorCode::CapabilityDenied.into());
        }
        Ok(())
    }

    fn finish_sas(&mut self) -> Result<()> {
        let Some(noise) = self.noise.as_ref() else {
            return self.fail(ErrorCode::WrongState.into());
        };
        if !noise.is_finished() {
            return self.fail(ErrorCode::WrongState.into());
        }
        let hash_bytes = noise.get_state().get_hash();
        let mut hash = [0_u8; 32];
        hash.copy_from_slice(hash_bytes.as_slice());
        let handshake_hash = HandshakeHash(hash);
        let sas = derive_sas_v1(
            &self.offer.nonce,
            &handshake_hash,
            self.offer.version,
            self.offer.host_static_public_key,
            self.device_key,
        )?;
        self.handshake_hash = Some(handshake_hash);
        self.sas = Some(sas);
        self.state = PairingState::SasReady;
        Ok(())
    }

    fn check_deadline(&mut self, now_millis: u64) -> Result<()> {
        if !matches!(
            self.state,
            PairingState::Created | PairingState::Handshaking
        ) {
            return self.fail(ErrorCode::WrongState.into());
        }
        if now_millis > self.deadline_millis {
            self.state = PairingState::Expired;
            self.noise = None;
            self.handshake_hash = None;
            self.sas = None;
            return Err(ErrorCode::TimedOut.into());
        }
        Ok(())
    }

    fn fail<T>(&mut self, error: ControllerSecurityError) -> Result<T> {
        self.state = PairingState::Failed;
        self.noise = None;
        self.handshake_hash = None;
        self.sas = None;
        Err(error)
    }
}

pub struct ConfirmedPairing {
    pub role: PairingRole,
    pub host_key: HostStaticPublicKey,
    pub device_key: DeviceStaticPublicKey,
    pub capabilities: crate::types::CapabilitySet,
    pub transport: ControllerTransport,
}

impl fmt::Debug for ConfirmedPairing {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConfirmedPairing")
            .field("role", &self.role)
            .field("host_key", &self.host_key)
            .field("device_key", &self.device_key)
            .field("capabilities", &self.capabilities)
            .field("transport", &"[REDACTED]")
            .finish()
    }
}

fn keypair(private: &StaticPrivateKey) -> KeyPair<[u8; 32], SensitiveByteArray<[u8; 32]>> {
    let secret = SensitiveByteArray::from_slice(private.as_bytes());
    let public = X25519::pubkey(&secret);
    KeyPair { public, secret }
}

#[must_use]
pub fn host_public_key_from_private(private: &StaticPrivateKey) -> HostStaticPublicKey {
    HostStaticPublicKey(keypair(private).public)
}

#[must_use]
pub fn device_public_key_from_private(private: &StaticPrivateKey) -> DeviceStaticPublicKey {
    DeviceStaticPublicKey(keypair(private).public)
}

fn validate_offer_time(offer: &PairingOfferCore, now_unix_seconds: u64) -> Result<()> {
    offer.version.require_v1()?;
    if now_unix_seconds > offer.expires_at_unix_seconds {
        Err(ErrorCode::Expired.into())
    } else if offer
        .expires_at_unix_seconds
        .saturating_sub(now_unix_seconds)
        > MAX_PAIRING_OFFER_LIFETIME_SECONDS
    {
        Err(ErrorCode::InvalidEncoding.into())
    } else {
        Ok(())
    }
}

fn pairing_deadline(offer: &PairingOfferCore, now_millis: u64, now_unix_seconds: u64) -> u64 {
    let offer_lifetime_millis = offer
        .expires_at_unix_seconds
        .saturating_sub(now_unix_seconds)
        .saturating_mul(1_000);
    now_millis.saturating_add(offer_lifetime_millis.min(HANDSHAKE_TIMEOUT_MILLIS))
}
