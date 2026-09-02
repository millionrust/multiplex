use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const MAX_PAIRED_DEVICES: usize = 16;
pub const MAX_PENDING_PAIRING_OFFERS: usize = 4;
pub const MAX_DEVICE_NAME_SCALARS: usize = 64;
pub const MAX_PAIRING_OFFER_LIFETIME_SECONDS: u64 = 5 * 60;
pub const PAIRING_ATTEMPT_LIMIT: usize = 5;
pub const PAIRING_ATTEMPT_WINDOW_SECONDS: u64 = 10 * 60;

const FINGERPRINT_DOMAIN: &[u8] = b"termirust-host-fingerprint-v1\0";
const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(transparent)]
pub struct HostIdentityGeneration(u64);

impl HostIdentityGeneration {
    pub const INITIAL: Self = Self(1);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub fn next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(transparent)]
pub struct DeviceStoreRevision(u64);

impl DeviceStoreRevision {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub fn next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

macro_rules! uuid_id {
    ($name:ident) => {
        #[derive(
            Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
        )]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            pub const fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

uuid_id!(ControllerDeviceId);
uuid_id!(PairingOfferId);

#[derive(Clone, Copy, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct HostPublicKey(pub [u8; 32]);

impl fmt::Debug for HostPublicKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HostPublicKey([REDACTED])")
    }
}

#[derive(Clone, Copy, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct DevicePublicKey(pub [u8; 32]);

impl fmt::Debug for DevicePublicKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DevicePublicKey([REDACTED])")
    }
}

#[derive(Clone, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct HostIdentitySecretRef(String);

impl HostIdentitySecretRef {
    pub fn new(value: impl Into<String>) -> Result<Self, ControllerDeviceError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 128
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_'))
        {
            return Err(ControllerDeviceError::InvalidSecretReference);
        }
        Ok(Self(value))
    }

    pub fn expose_reference(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for HostIdentitySecretRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HostIdentitySecretRef([OPAQUE])")
    }
}

#[derive(Clone, Copy, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct HostFingerprint([u8; 32]);

impl HostFingerprint {
    pub fn derive(public_key: HostPublicKey) -> Self {
        let mut digest = Sha256::new();
        digest.update(FINGERPRINT_DOMAIN);
        digest.update(public_key.0);
        Self(digest.finalize().into())
    }

    pub const fn from_digest(digest: [u8; 32]) -> Self {
        Self(digest)
    }

    pub const fn digest(self) -> [u8; 32] {
        self.0
    }

    pub fn canonical(self) -> String {
        let mut encoded = String::with_capacity(52);
        let mut accumulator = 0_u32;
        let mut bits = 0_u8;
        for byte in self.0 {
            accumulator = (accumulator << 8) | u32::from(byte);
            bits += 8;
            while bits >= 5 {
                bits -= 5;
                encoded.push(CROCKFORD[((accumulator >> bits) & 0x1f) as usize] as char);
            }
        }
        if bits > 0 {
            encoded.push(CROCKFORD[((accumulator << (5 - bits)) & 0x1f) as usize] as char);
        }
        encoded
            .as_bytes()
            .chunks(4)
            .map(|chunk| std::str::from_utf8(chunk).unwrap_or_default())
            .collect::<Vec<_>>()
            .join("-")
    }

    pub fn parse_canonical(value: &str) -> Result<Self, ControllerDeviceError> {
        let compact: String = value
            .chars()
            .filter(|character| *character != '-')
            .collect();
        if compact.len() != 52
            || value.split('-').any(|group| group.len() != 4)
            || value.split('-').count() != 13
        {
            return Err(ControllerDeviceError::InvalidFingerprint);
        }
        let mut output = [0_u8; 32];
        let mut accumulator = 0_u32;
        let mut bits = 0_u8;
        let mut output_index = 0;
        for (index, symbol) in compact.bytes().enumerate() {
            let value = crockford_value(symbol).ok_or(ControllerDeviceError::InvalidFingerprint)?;
            if index == 51 && value & 0x0f != 0 {
                return Err(ControllerDeviceError::InvalidFingerprintPadding);
            }
            accumulator = (accumulator << 5) | u32::from(value);
            bits += 5;
            if bits >= 8 && output_index < output.len() {
                bits -= 8;
                output[output_index] = ((accumulator >> bits) & 0xff) as u8;
                output_index += 1;
            }
        }
        if output_index != output.len() {
            return Err(ControllerDeviceError::InvalidFingerprint);
        }
        Ok(Self(output))
    }

    pub fn row_suffix(self) -> String {
        self.canonical()
            .chars()
            .filter(|character| *character != '-')
            .rev()
            .take(8)
            .collect::<String>()
            .chars()
            .rev()
            .collect()
    }
}

impl fmt::Debug for HostFingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HostFingerprint([REDACTED])")
    }
}

fn crockford_value(symbol: u8) -> Option<u8> {
    CROCKFORD
        .iter()
        .position(|candidate| *candidate == symbol)
        .map(|index| index as u8)
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HostIdentityPublic {
    pub generation: HostIdentityGeneration,
    pub public_key: HostPublicKey,
    pub fingerprint: HostFingerprint,
}

impl HostIdentityPublic {
    pub fn new(generation: HostIdentityGeneration, public_key: HostPublicKey) -> Self {
        Self {
            generation,
            public_key,
            fingerprint: HostFingerprint::derive(public_key),
        }
    }

    pub fn validate(&self) -> Result<(), ControllerDeviceError> {
        if self.generation.get() == 0
            || self.fingerprint != HostFingerprint::derive(self.public_key)
        {
            return Err(ControllerDeviceError::InvalidIdentity);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostIdentityState {
    Ready,
    Locked,
    PermissionDenied,
    Lost,
    ResetRequired,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerCapability {
    ObserveSessions,
    AttachOutput,
    SendInput,
    Resize,
    RespondToApproval,
}

impl ControllerCapability {
    const fn bit(self) -> u16 {
        1 << self as u8
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ControllerCapabilities(u16);

impl ControllerCapabilities {
    pub const KNOWN_MASK: u16 = 0x1f;

    pub fn from_bits(bits: u16) -> Result<Self, ControllerDeviceError> {
        if bits & !Self::KNOWN_MASK == 0 {
            Ok(Self(bits))
        } else {
            Err(ControllerDeviceError::UnknownCapability)
        }
    }

    pub const fn bits(self) -> u16 {
        self.0
    }

    pub const fn contains(self, capability: ControllerCapability) -> bool {
        self.0 & capability.bit() != 0
    }

    pub const fn with(self, capability: ControllerCapability) -> Self {
        Self(self.0 | capability.bit())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerProtocolRange {
    pub minimum: u16,
    pub maximum: u16,
}

impl ControllerProtocolRange {
    pub const V1: Self = Self {
        minimum: 1,
        maximum: 1,
    };

    pub const fn is_valid(self) -> bool {
        self.minimum != 0 && self.minimum <= self.maximum && self.maximum <= 1
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PairedDeviceStatus {
    Offline,
    Online,
    Revoked,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PairedDeviceRecord {
    pub device_id: ControllerDeviceId,
    pub public_key: DevicePublicKey,
    pub display_name: String,
    pub capabilities: ControllerCapabilities,
    pub protocol_range: ControllerProtocolRange,
    pub created_at: u64,
    pub last_seen_at: Option<u64>,
    pub revocation_epoch: u64,
    pub identity_generation: HostIdentityGeneration,
    pub status: PairedDeviceStatus,
    pub source_offer_id: PairingOfferId,
}

impl PairedDeviceRecord {
    pub fn validate(&self) -> Result<(), ControllerDeviceError> {
        validate_name(&self.display_name)?;
        ControllerCapabilities::from_bits(self.capabilities.bits())?;
        if !self.protocol_range.is_valid() || self.identity_generation.get() == 0 {
            return Err(ControllerDeviceError::InvalidDevice);
        }
        Ok(())
    }

    pub fn fingerprint_suffix(&self) -> String {
        HostFingerprint::derive(HostPublicKey(self.public_key.0)).row_suffix()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingOfferState {
    Offered,
    Handshaking,
    SasReady,
    HostConfirmed,
    Persisted,
    Acknowledged,
    Expired,
    Rejected,
    Failed,
    Uncertain,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PairingOfferRecord {
    pub offer_id: PairingOfferId,
    pub identity: HostIdentityPublic,
    pub nonce: [u8; 32],
    pub created_at: u64,
    pub expires_at: u64,
    pub protocol_range: ControllerProtocolRange,
    pub capabilities: ControllerCapabilities,
    pub route_candidates: Vec<String>,
    pub state: PairingOfferState,
    pub paired_device_key: Option<DevicePublicKey>,
}

impl fmt::Debug for PairingOfferRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PairingOfferRecord")
            .field("offer_id", &self.offer_id)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl PairingOfferRecord {
    pub fn validate(&self) -> Result<(), ControllerDeviceError> {
        self.identity.validate()?;
        ControllerCapabilities::from_bits(self.capabilities.bits())?;
        if !self.protocol_range.is_valid()
            || self.expires_at <= self.created_at
            || self.expires_at - self.created_at > MAX_PAIRING_OFFER_LIFETIME_SECONDS
            || self.route_candidates.iter().any(|route| route.len() > 512)
        {
            return Err(ControllerDeviceError::InvalidOffer);
        }
        Ok(())
    }

    pub const fn is_pending(&self) -> bool {
        matches!(
            self.state,
            PairingOfferState::Offered
                | PairingOfferState::Handshaking
                | PairingOfferState::SasReady
                | PairingOfferState::HostConfirmed
                | PairingOfferState::Persisted
                | PairingOfferState::Uncertain
        )
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PairingAttemptLedger {
    pub attempt_timestamps: Vec<u64>,
}

impl PairingAttemptLedger {
    pub fn record(&mut self, now: u64) -> Result<(), ControllerDeviceError> {
        self.attempt_timestamps
            .retain(|timestamp| now.saturating_sub(*timestamp) < PAIRING_ATTEMPT_WINDOW_SECONDS);
        if self.attempt_timestamps.len() >= PAIRING_ATTEMPT_LIMIT {
            return Err(ControllerDeviceError::RateLimited);
        }
        self.attempt_timestamps.push(now);
        Ok(())
    }

    fn validate(&self) -> Result<(), ControllerDeviceError> {
        if self.attempt_timestamps.len() > PAIRING_ATTEMPT_LIMIT {
            return Err(ControllerDeviceError::InvalidAttemptLedger);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerDeviceAuthority {
    pub identity: Option<HostIdentityPublic>,
    pub secret_ref: Option<HostIdentitySecretRef>,
    pub state: HostIdentityState,
    pub revocation_epoch: u64,
    pub session_generation: u64,
    pub devices: Vec<PairedDeviceRecord>,
    pub offers: Vec<PairingOfferRecord>,
    pub attempts: PairingAttemptLedger,
}

impl Default for ControllerDeviceAuthority {
    fn default() -> Self {
        Self {
            identity: None,
            secret_ref: None,
            state: HostIdentityState::Lost,
            revocation_epoch: 0,
            session_generation: 0,
            devices: Vec::new(),
            offers: Vec::new(),
            attempts: PairingAttemptLedger::default(),
        }
    }
}

impl ControllerDeviceAuthority {
    pub fn validate(&self) -> Result<(), ControllerDeviceError> {
        if self.devices.len() > MAX_PAIRED_DEVICES {
            return Err(ControllerDeviceError::DeviceLimit);
        }
        if self
            .offers
            .iter()
            .filter(|offer| offer.is_pending())
            .count()
            > MAX_PENDING_PAIRING_OFFERS
        {
            return Err(ControllerDeviceError::OfferLimit);
        }
        if self.identity.is_some() != self.secret_ref.is_some() {
            return Err(ControllerDeviceError::InvalidIdentity);
        }
        if let Some(identity) = &self.identity {
            identity.validate()?;
            if self
                .devices
                .iter()
                .any(|device| device.identity_generation.get() > identity.generation.get())
            {
                return Err(ControllerDeviceError::InvalidDevice);
            }
        } else if !self.devices.is_empty() || !self.offers.is_empty() {
            return Err(ControllerDeviceError::InvalidIdentity);
        }
        for device in &self.devices {
            device.validate()?;
        }
        for offer in &self.offers {
            offer.validate()?;
        }
        self.attempts.validate()?;
        Ok(())
    }

    pub fn create_offer(
        &mut self,
        offer_id: PairingOfferId,
        nonce: [u8; 32],
        now: u64,
        expires_at: u64,
        capabilities: ControllerCapabilities,
        route_candidates: Vec<String>,
    ) -> Result<PairingOfferRecord, ControllerDeviceError> {
        if self.state != HostIdentityState::Ready {
            return Err(ControllerDeviceError::IdentityUnavailable);
        }
        self.attempts.record(now)?;
        if self
            .offers
            .iter()
            .filter(|offer| offer.is_pending())
            .count()
            >= MAX_PENDING_PAIRING_OFFERS
        {
            return Err(ControllerDeviceError::OfferLimit);
        }
        let identity = self
            .identity
            .clone()
            .ok_or(ControllerDeviceError::IdentityUnavailable)?;
        let offer = PairingOfferRecord {
            offer_id,
            identity,
            nonce,
            created_at: now,
            expires_at,
            protocol_range: ControllerProtocolRange::V1,
            capabilities,
            route_candidates,
            state: PairingOfferState::Offered,
            paired_device_key: None,
        };
        offer.validate()?;
        self.offers.push(offer.clone());
        Ok(offer)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn persist_pairing(
        &mut self,
        offer_id: PairingOfferId,
        device_id: ControllerDeviceId,
        public_key: DevicePublicKey,
        display_name: String,
        now: u64,
    ) -> Result<PairedDeviceRecord, ControllerDeviceError> {
        if let Some(existing) = self
            .devices
            .iter()
            .find(|device| device.public_key == public_key || device.source_offer_id == offer_id)
        {
            if existing.device_id == device_id
                && existing.public_key == public_key
                && existing.source_offer_id == offer_id
            {
                return Ok(existing.clone());
            }
            if existing.device_id != device_id {
                return Err(ControllerDeviceError::PairingConflict);
            }
        }
        let previous_capability_bits = self
            .devices
            .iter()
            .filter(|device| device.device_id == device_id)
            .fold(0_u16, |bits, device| bits | device.capabilities.bits());
        let distinct_device_count = self
            .devices
            .iter()
            .filter(|device| device.device_id != device_id)
            .count();
        if distinct_device_count >= MAX_PAIRED_DEVICES {
            return Err(ControllerDeviceError::DeviceLimit);
        }
        let offer = self
            .offers
            .iter_mut()
            .find(|offer| offer.offer_id == offer_id)
            .ok_or(ControllerDeviceError::OfferNotFound)?;
        if now > offer.expires_at {
            offer.state = PairingOfferState::Expired;
            return Err(ControllerDeviceError::OfferExpired);
        }
        if !matches!(
            offer.state,
            PairingOfferState::SasReady | PairingOfferState::HostConfirmed
        ) {
            return Err(ControllerDeviceError::OfferConsumed);
        }
        let capabilities = ControllerCapabilities::from_bits(
            offer.capabilities.bits() | previous_capability_bits,
        )?;
        let record = PairedDeviceRecord {
            device_id,
            public_key,
            display_name,
            capabilities,
            protocol_range: offer.protocol_range,
            created_at: now,
            last_seen_at: None,
            revocation_epoch: self.revocation_epoch,
            identity_generation: offer.identity.generation,
            status: PairedDeviceStatus::Offline,
            source_offer_id: offer.offer_id,
        };
        record.validate()?;
        offer.state = PairingOfferState::Persisted;
        offer.paired_device_key = Some(public_key);
        self.devices.retain(|device| device.device_id != device_id);
        self.devices.push(record.clone());
        Ok(record)
    }

    pub fn acknowledge_pairing(
        &mut self,
        offer_id: PairingOfferId,
        public_key: DevicePublicKey,
    ) -> Result<ControllerDeviceId, ControllerDeviceError> {
        let device = self
            .devices
            .iter()
            .find(|device| device.source_offer_id == offer_id && device.public_key == public_key)
            .ok_or(ControllerDeviceError::PairingUncertain)?;
        if let Some(offer) = self
            .offers
            .iter_mut()
            .find(|offer| offer.offer_id == offer_id)
        {
            offer.state = PairingOfferState::Acknowledged;
        }
        Ok(device.device_id)
    }

    pub fn revoke_device(
        &mut self,
        device_id: ControllerDeviceId,
    ) -> Result<u64, ControllerDeviceError> {
        if !self
            .devices
            .iter()
            .any(|device| device.device_id == device_id)
        {
            return Err(ControllerDeviceError::DeviceNotFound);
        }
        self.revocation_epoch = self
            .revocation_epoch
            .checked_add(1)
            .ok_or(ControllerDeviceError::CounterOverflow)?;
        for device in &mut self.devices {
            device.revocation_epoch = self.revocation_epoch;
            if device.device_id == device_id {
                device.status = PairedDeviceStatus::Revoked;
            }
        }
        self.session_generation = self
            .session_generation
            .checked_add(1)
            .ok_or(ControllerDeviceError::CounterOverflow)?;
        Ok(self.revocation_epoch)
    }

    pub fn begin_identity_reset(
        &mut self,
    ) -> Result<HostIdentityGeneration, ControllerDeviceError> {
        let generation = self
            .identity
            .as_ref()
            .map(|identity| identity.generation)
            .unwrap_or_default()
            .next()
            .ok_or(ControllerDeviceError::CounterOverflow)?;
        self.revocation_epoch = self
            .revocation_epoch
            .checked_add(1)
            .ok_or(ControllerDeviceError::CounterOverflow)?;
        self.session_generation = self
            .session_generation
            .checked_add(1)
            .ok_or(ControllerDeviceError::CounterOverflow)?;
        for device in &mut self.devices {
            device.status = PairedDeviceStatus::Revoked;
            device.revocation_epoch = self.revocation_epoch;
        }
        for offer in &mut self.offers {
            if offer.is_pending() {
                offer.state = PairingOfferState::Rejected;
            }
        }
        self.state = HostIdentityState::ResetRequired;
        Ok(generation)
    }

    pub fn finish_identity_reset(
        &mut self,
        identity: HostIdentityPublic,
        secret_ref: HostIdentitySecretRef,
    ) -> Result<(), ControllerDeviceError> {
        identity.validate()?;
        let expected_generation = self
            .identity
            .as_ref()
            .map(|current| current.generation)
            .unwrap_or_default()
            .next()
            .ok_or(ControllerDeviceError::CounterOverflow)?;
        if identity.generation != expected_generation {
            return Err(ControllerDeviceError::InvalidIdentity);
        }
        self.identity = Some(identity);
        self.secret_ref = Some(secret_ref);
        self.state = HostIdentityState::Ready;
        self.offers.clear();
        Ok(())
    }

    pub fn authorize(&self, request: AuthorizationRequest) -> AuthorizationDecision {
        if self.state != HostIdentityState::Ready {
            return AuthorizationDecision::Deny(AuthorizationDenial::IdentityUnavailable);
        }
        let Some(identity) = &self.identity else {
            return AuthorizationDecision::Deny(AuthorizationDenial::IdentityUnavailable);
        };
        let Some(device) = self.devices.iter().find(|device| {
            device.device_id == request.device_id && device.public_key == request.public_key
        }) else {
            return AuthorizationDecision::Deny(AuthorizationDenial::UnknownDevice);
        };
        if device.status == PairedDeviceStatus::Revoked {
            return AuthorizationDecision::Deny(AuthorizationDenial::Revoked);
        }
        if request.identity_generation != identity.generation {
            return AuthorizationDecision::Deny(AuthorizationDenial::StaleIdentity);
        }
        if request.revocation_epoch != self.revocation_epoch
            || device.revocation_epoch != self.revocation_epoch
        {
            return AuthorizationDecision::Deny(AuthorizationDenial::StaleRevocationEpoch);
        }
        if request.session_generation != self.session_generation {
            return AuthorizationDecision::Deny(AuthorizationDenial::WrongSessionGeneration);
        }
        if request.now_millis > request.deadline_millis {
            return AuthorizationDecision::Deny(AuthorizationDenial::Expired);
        }
        if !device.capabilities.contains(request.capability) {
            return AuthorizationDecision::Deny(AuthorizationDenial::CapabilityDenied);
        }
        AuthorizationDecision::Allow
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AuthorizationRequest {
    pub device_id: ControllerDeviceId,
    pub public_key: DevicePublicKey,
    pub identity_generation: HostIdentityGeneration,
    pub capability: ControllerCapability,
    pub revocation_epoch: u64,
    pub session_generation: u64,
    pub now_millis: u64,
    pub deadline_millis: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthorizationDecision {
    Allow,
    Deny(AuthorizationDenial),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthorizationDenial {
    IdentityUnavailable,
    UnknownDevice,
    Revoked,
    StaleIdentity,
    StaleRevocationEpoch,
    WrongSessionGeneration,
    Expired,
    CapabilityDenied,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerDeviceError {
    InvalidSecretReference,
    InvalidFingerprint,
    InvalidFingerprintPadding,
    InvalidIdentity,
    UnknownCapability,
    InvalidDevice,
    InvalidOffer,
    InvalidAttemptLedger,
    IdentityUnavailable,
    DeviceLimit,
    OfferLimit,
    RateLimited,
    OfferNotFound,
    OfferExpired,
    OfferConsumed,
    DeviceNotFound,
    PairingConflict,
    PairingUncertain,
    CounterOverflow,
    InvalidDeviceName,
}

impl fmt::Display for ControllerDeviceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidSecretReference => "invalid Host identity secret reference",
            Self::InvalidFingerprint => "invalid Host identity fingerprint",
            Self::InvalidFingerprintPadding => "noncanonical Host fingerprint padding",
            Self::InvalidIdentity => "invalid Host identity metadata",
            Self::UnknownCapability => "unknown Controller capability",
            Self::InvalidDevice => "invalid paired-device record",
            Self::InvalidOffer => "invalid pairing offer",
            Self::InvalidAttemptLedger => "invalid pairing-attempt ledger",
            Self::IdentityUnavailable => "Host identity is unavailable",
            Self::DeviceLimit => "paired-device limit reached",
            Self::OfferLimit => "pending pairing-offer limit reached",
            Self::RateLimited => "pairing attempts are rate limited",
            Self::OfferNotFound => "pairing offer was not found",
            Self::OfferExpired => "pairing offer expired",
            Self::OfferConsumed => "pairing offer was already consumed",
            Self::DeviceNotFound => "paired device was not found",
            Self::PairingConflict => "pairing reconciliation conflicted",
            Self::PairingUncertain => "pairing acknowledgement is uncertain",
            Self::CounterOverflow => "controller authority counter overflow",
            Self::InvalidDeviceName => "invalid device name",
        })
    }
}

impl std::error::Error for ControllerDeviceError {}

fn validate_name(name: &str) -> Result<(), ControllerDeviceError> {
    let count = name.chars().count();
    if count == 0 || count > MAX_DEVICE_NAME_SCALARS || name.chars().any(char::is_control) {
        Err(ControllerDeviceError::InvalidDeviceName)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> HostIdentityPublic {
        HostIdentityPublic::new(HostIdentityGeneration::INITIAL, HostPublicKey([7; 32]))
    }

    fn authority() -> ControllerDeviceAuthority {
        ControllerDeviceAuthority {
            identity: Some(identity()),
            secret_ref: Some(HostIdentitySecretRef::new("identity:test").unwrap()),
            state: HostIdentityState::Ready,
            ..ControllerDeviceAuthority::default()
        }
    }

    #[test]
    fn controller_devices_fingerprint_matches_normative_vector() {
        let public_key = HostPublicKey(core::array::from_fn(|index| index as u8));
        let fingerprint = HostFingerprint::derive(public_key);
        assert_eq!(
            fingerprint.digest(),
            [
                0xbd, 0x0d, 0xa4, 0x54, 0x68, 0x73, 0xaf, 0x30, 0x9c, 0xa2, 0x59, 0xb3, 0x5c, 0x62,
                0x32, 0x01, 0xc8, 0x6c, 0x27, 0xd4, 0x22, 0xd2, 0x59, 0x8a, 0xa1, 0xd1, 0x2b, 0x39,
                0xe8, 0x94, 0x3e, 0xf4,
            ]
        );
        let display = "QM6T-8N38-EEQK-1752-B6SN-RRHJ-0746-R9YM-4B95-K2N1-T4NK-KT4M-7VT0";
        assert_eq!(fingerprint.canonical(), display);
        assert_eq!(fingerprint.row_suffix(), "KT4M7VT0");
        assert_eq!(HostFingerprint::parse_canonical(display), Ok(fingerprint));
        let mut noncanonical = display.to_string();
        noncanonical.pop();
        noncanonical.push('1');
        assert_eq!(
            HostFingerprint::parse_canonical(&noncanonical),
            Err(ControllerDeviceError::InvalidFingerprintPadding)
        );
    }

    #[test]
    fn controller_devices_enforces_offer_and_rate_limits() {
        let mut authority = authority();
        for index in 0..MAX_PENDING_PAIRING_OFFERS {
            authority
                .create_offer(
                    PairingOfferId::new(),
                    [index as u8; 32],
                    index as u64,
                    index as u64 + 60,
                    ControllerCapabilities::default(),
                    vec!["synthetic:test".into()],
                )
                .unwrap();
        }
        assert_eq!(
            authority.create_offer(
                PairingOfferId::new(),
                [9; 32],
                5,
                65,
                ControllerCapabilities::default(),
                vec![]
            ),
            Err(ControllerDeviceError::OfferLimit)
        );
    }

    #[test]
    fn controller_devices_reconciles_lost_ack_without_duplicates() {
        let mut authority = authority();
        let offer_id = PairingOfferId::new();
        authority
            .create_offer(
                offer_id,
                [3; 32],
                10,
                70,
                ControllerCapabilities::default().with(ControllerCapability::ObserveSessions),
                vec!["synthetic:test".into()],
            )
            .unwrap();
        authority.offers[0].state = PairingOfferState::SasReady;
        let device_id = ControllerDeviceId::new();
        let key = DevicePublicKey([8; 32]);
        let first = authority
            .persist_pairing(offer_id, device_id, key, "Phone".into(), 20)
            .unwrap();
        let reconciled = authority
            .persist_pairing(offer_id, device_id, key, "Phone".into(), 21)
            .unwrap();
        assert_eq!(first, reconciled);
        assert_eq!(authority.devices.len(), 1);
        assert_eq!(authority.acknowledge_pairing(offer_id, key), Ok(device_id));
    }

    #[test]
    fn controller_devices_repair_replaces_old_key_and_preserves_approved_capabilities() {
        let mut authority = authority();
        let device_id = ControllerDeviceId::new();
        let first_offer = PairingOfferId::new();
        let approved = ControllerCapabilities::default()
            .with(ControllerCapability::ObserveSessions)
            .with(ControllerCapability::AttachOutput)
            .with(ControllerCapability::SendInput)
            .with(ControllerCapability::Resize);
        authority
            .create_offer(first_offer, [3; 32], 10, 70, approved, vec![])
            .unwrap();
        authority.offers[0].state = PairingOfferState::SasReady;
        authority
            .persist_pairing(
                first_offer,
                device_id,
                DevicePublicKey([8; 32]),
                "Phone".into(),
                20,
            )
            .unwrap();

        let second_offer = PairingOfferId::new();
        let read_only = ControllerCapabilities::default()
            .with(ControllerCapability::ObserveSessions)
            .with(ControllerCapability::AttachOutput);
        authority
            .create_offer(second_offer, [4; 32], 21, 80, read_only, vec![])
            .unwrap();
        authority
            .offers
            .iter_mut()
            .find(|offer| offer.offer_id == second_offer)
            .unwrap()
            .state = PairingOfferState::SasReady;
        let repaired = authority
            .persist_pairing(
                second_offer,
                device_id,
                DevicePublicKey([9; 32]),
                "Phone".into(),
                30,
            )
            .unwrap();

        assert_eq!(authority.devices.len(), 1);
        assert_eq!(repaired.public_key, DevicePublicKey([9; 32]));
        assert!(
            repaired
                .capabilities
                .contains(ControllerCapability::SendInput)
        );
        assert!(repaired.capabilities.contains(ControllerCapability::Resize));
    }

    #[test]
    fn controller_devices_revoke_wins_authorization_race() {
        let mut authority = authority();
        let offer_id = PairingOfferId::new();
        authority
            .create_offer(
                offer_id,
                [3; 32],
                10,
                70,
                ControllerCapabilities::default().with(ControllerCapability::SendInput),
                vec!["synthetic:test".into()],
            )
            .unwrap();
        authority.offers[0].state = PairingOfferState::SasReady;
        let device_id = ControllerDeviceId::new();
        let key = DevicePublicKey([8; 32]);
        authority
            .persist_pairing(offer_id, device_id, key, "Phone".into(), 20)
            .unwrap();
        let request = AuthorizationRequest {
            device_id,
            public_key: key,
            identity_generation: HostIdentityGeneration::INITIAL,
            capability: ControllerCapability::SendInput,
            revocation_epoch: 0,
            session_generation: 0,
            now_millis: 20,
            deadline_millis: 21,
        };
        assert_eq!(authority.authorize(request), AuthorizationDecision::Allow);
        authority.revoke_device(device_id).unwrap();
        assert_eq!(
            authority.authorize(request),
            AuthorizationDecision::Deny(AuthorizationDenial::Revoked)
        );
    }

    #[test]
    fn controller_devices_revoke_rotates_other_devices_without_revoking_them() {
        let mut authority = authority();
        let first_id = ControllerDeviceId::new();
        let second_id = ControllerDeviceId::new();
        let capabilities =
            ControllerCapabilities::default().with(ControllerCapability::ObserveSessions);
        for (index, (device_id, key)) in [
            (first_id, DevicePublicKey([8; 32])),
            (second_id, DevicePublicKey([9; 32])),
        ]
        .into_iter()
        .enumerate()
        {
            authority.devices.push(PairedDeviceRecord {
                device_id,
                public_key: key,
                display_name: format!("Device {index}"),
                capabilities,
                protocol_range: ControllerProtocolRange::V1,
                created_at: 1,
                last_seen_at: None,
                revocation_epoch: 0,
                identity_generation: HostIdentityGeneration::INITIAL,
                status: PairedDeviceStatus::Offline,
                source_offer_id: PairingOfferId::new(),
            });
        }
        authority.revoke_device(first_id).unwrap();
        assert_eq!(authority.devices[0].status, PairedDeviceStatus::Revoked);
        assert_eq!(authority.devices[1].status, PairedDeviceStatus::Offline);
        assert_eq!(authority.devices[1].revocation_epoch, 1);
    }

    #[test]
    fn controller_devices_authorization_rechecks_every_boundary() {
        let mut authority = authority();
        let device_id = ControllerDeviceId::new();
        let public_key = DevicePublicKey([8; 32]);
        authority.devices.push(PairedDeviceRecord {
            device_id,
            public_key,
            display_name: "Phone".into(),
            capabilities: ControllerCapabilities::default()
                .with(ControllerCapability::ObserveSessions),
            protocol_range: ControllerProtocolRange::V1,
            created_at: 1,
            last_seen_at: None,
            revocation_epoch: 0,
            identity_generation: HostIdentityGeneration::INITIAL,
            status: PairedDeviceStatus::Offline,
            source_offer_id: PairingOfferId::new(),
        });
        let request = AuthorizationRequest {
            device_id,
            public_key,
            identity_generation: HostIdentityGeneration::INITIAL,
            capability: ControllerCapability::ObserveSessions,
            revocation_epoch: 0,
            session_generation: 0,
            now_millis: 10,
            deadline_millis: 20,
        };

        assert_eq!(authority.authorize(request), AuthorizationDecision::Allow);
        assert_eq!(
            authority.authorize(AuthorizationRequest {
                identity_generation: HostIdentityGeneration::new(2),
                ..request
            }),
            AuthorizationDecision::Deny(AuthorizationDenial::StaleIdentity)
        );
        assert_eq!(
            authority.authorize(AuthorizationRequest {
                session_generation: 1,
                ..request
            }),
            AuthorizationDecision::Deny(AuthorizationDenial::WrongSessionGeneration)
        );
        assert_eq!(
            authority.authorize(AuthorizationRequest {
                capability: ControllerCapability::SendInput,
                ..request
            }),
            AuthorizationDecision::Deny(AuthorizationDenial::CapabilityDenied)
        );
        assert_eq!(
            authority.authorize(AuthorizationRequest {
                now_millis: 21,
                ..request
            }),
            AuthorizationDecision::Deny(AuthorizationDenial::Expired)
        );
        assert_eq!(
            authority.authorize(AuthorizationRequest {
                public_key: DevicePublicKey([9; 32]),
                ..request
            }),
            AuthorizationDecision::Deny(AuthorizationDenial::UnknownDevice)
        );
    }

    #[test]
    fn controller_devices_rejects_oversized_names() {
        assert_eq!(validate_name(&"x".repeat(64)), Ok(()));
        assert_eq!(
            validate_name(&"x".repeat(65)),
            Err(ControllerDeviceError::InvalidDeviceName)
        );
        assert_eq!(
            validate_name("line\nbreak"),
            Err(ControllerDeviceError::InvalidDeviceName)
        );
    }

    #[test]
    fn controller_devices_reset_invalidates_all_prior_trust() {
        let mut authority = authority();
        let next = authority.begin_identity_reset().unwrap();
        assert_eq!(next, HostIdentityGeneration::new(2));
        assert_eq!(authority.state, HostIdentityState::ResetRequired);
        authority
            .finish_identity_reset(
                HostIdentityPublic::new(next, HostPublicKey([9; 32])),
                HostIdentitySecretRef::new("identity:next").unwrap(),
            )
            .unwrap();
        assert_eq!(authority.state, HostIdentityState::Ready);
        assert_eq!(authority.identity.unwrap().generation, next);
    }

    #[test]
    fn controller_devices_debug_redacts_sensitive_values() {
        let secret = HostIdentitySecretRef::new("identity:canary-secret").unwrap();
        assert!(!format!("{secret:?}").contains("canary"));
        let offer = PairingOfferRecord {
            offer_id: PairingOfferId::new(),
            identity: identity(),
            nonce: [42; 32],
            created_at: 0,
            expires_at: 60,
            protocol_range: ControllerProtocolRange::V1,
            capabilities: ControllerCapabilities::default(),
            route_candidates: vec!["private-route".into()],
            state: PairingOfferState::Offered,
            paired_device_key: None,
        };
        let debug = format!("{offer:?}");
        assert!(!debug.contains("private-route"));
        assert!(!debug.contains("nonce"));
        assert!(!debug.contains("route_candidates"));
    }
}
