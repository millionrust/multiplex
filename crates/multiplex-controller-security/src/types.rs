use core::fmt;

use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::error::{ErrorCode, Result};

pub const NOISE_PROTOCOL_NAME: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";
pub const HANDSHAKE_TIMEOUT_MILLIS: u64 = 30_000;
pub const MAX_PAIRING_OFFER_LIFETIME_SECONDS: u64 = 300;
pub const MAX_CONTROL_PAYLOAD_BYTES: usize = 64 * 1024;
pub const MAX_TERMINAL_FRAME_BYTES: usize = 1024 * 1024;
/// A complete screen frame, header and tag included. The tile codec's largest batch.
pub const MAX_SCREEN_FRAME_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ControllerProtocolVersion {
    pub major: u16,
    pub minor: u16,
}

pub const CONTROLLER_V2: ControllerProtocolVersion =
    ControllerProtocolVersion { major: 2, minor: 0 };

impl ControllerProtocolVersion {
    /// Exact-version compatibility, as the ADR requires. A Controller-v1 peer is refused here,
    /// before any key material is derived, rather than failing later as an authentication error
    /// that a user could not tell from tampering: its separators derive different keys.
    pub(crate) fn require_supported(self) -> Result<()> {
        if self == CONTROLLER_V2 {
            Ok(())
        } else {
            Err(ErrorCode::IncompatibleVersion.into())
        }
    }
}

macro_rules! public_key_type {
    ($name:ident) => {
        #[derive(Clone, Copy, Eq, Hash, PartialEq)]
        pub struct $name(pub [u8; 32]);

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!(stringify!($name), "(..)"))
            }
        }
    };
}

public_key_type!(HostStaticPublicKey);
public_key_type!(DeviceStaticPublicKey);

#[derive(Clone, Eq, PartialEq, Zeroize, ZeroizeOnDrop)]
pub struct StaticPrivateKey([u8; 32]);

impl StaticPrivateKey {
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub fn from_fixture_bytes(bytes: [u8; 32]) -> Self {
        Self::from_bytes(bytes)
    }

    pub(crate) fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Copies the key solely for an inherited pipe handoff to an owned listener process.
    /// Callers must zeroize the returned bytes after the handoff.
    #[must_use]
    pub fn copy_for_process_handoff(&self) -> [u8; 32] {
        self.0
    }

    /// Copies the key solely for immediate encoding into an operating-system secret store.
    /// The caller must zeroize the returned bytes and any encoded representation.
    #[must_use]
    pub fn copy_for_secret_storage(&self) -> [u8; 32] {
        self.0
    }
}

impl fmt::Debug for StaticPrivateKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("StaticPrivateKey([REDACTED])")
    }
}

#[derive(Clone, Eq, PartialEq, Zeroize, ZeroizeOnDrop)]
pub struct PairingNonce(pub [u8; 32]);

impl fmt::Debug for PairingNonce {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PairingNonce([REDACTED])")
    }
}

#[derive(Clone, Eq, PartialEq, Zeroize, ZeroizeOnDrop)]
pub struct HandshakeHash(pub [u8; 32]);

impl fmt::Debug for HandshakeHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HandshakeHash([REDACTED])")
    }
}

#[derive(Clone, Eq, PartialEq, Zeroize, ZeroizeOnDrop)]
pub struct SasCode([u8; 9]);

impl SasCode {
    pub(crate) fn new(bytes: [u8; 9]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.0).unwrap_or("REDACTED")
    }

    #[must_use]
    pub fn accessibility_symbols(&self) -> Vec<&'static str> {
        self.0
            .iter()
            .filter(|byte| **byte != b'-')
            .map(|byte| {
                if byte.is_ascii_digit() {
                    "digit"
                } else {
                    "letter"
                }
            })
            .collect()
    }

    pub(crate) fn bytes(&self) -> &[u8; 9] {
        &self.0
    }
}

impl fmt::Debug for SasCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SasCode([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ControllerCapability {
    ObserveSessions = 0,
    AttachOutput = 1,
    SendInput = 2,
    Resize = 3,
    RespondToApproval = 4,
    ObserveScreens = 5,
    ControlPointer = 6,
    ControlKeyboard = 7,
}

impl ControllerCapability {
    pub(crate) const fn bit(self) -> u16 {
        1 << (self as u8)
    }

    pub(crate) fn from_wire(value: u8) -> Result<Self> {
        match value {
            0 => Ok(Self::ObserveSessions),
            1 => Ok(Self::AttachOutput),
            2 => Ok(Self::SendInput),
            3 => Ok(Self::Resize),
            4 => Ok(Self::RespondToApproval),
            5 => Ok(Self::ObserveScreens),
            6 => Ok(Self::ControlPointer),
            7 => Ok(Self::ControlKeyboard),
            _ => Err(ErrorCode::UnknownCapability.into()),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CapabilitySet(u16);

impl CapabilitySet {
    /// Bits 0 to 4 from Controller-v1, bits 5 to 7 from the Remote Screens amendment.
    pub const KNOWN_MASK: u16 = 0x00ff;

    /// A set this build put together itself, where an unknown bit is a mistake rather than news.
    pub fn from_bits(bits: u16) -> Result<Self> {
        if bits & !Self::KNOWN_MASK == 0 {
            Ok(Self(bits))
        } else {
            Err(ErrorCode::UnknownCapability.into())
        }
    }

    /// A set read off the wire, where a bit this build does not know means the peer is newer.
    ///
    /// Those bits are dropped rather than refused. A device cannot gain anything by asking for
    /// one — what it is granted is decided by the computer from its own record — and refusing
    /// the whole connection over a bit nobody here understands is how a protocol stops growing.
    /// `multiplex-screen-protocol`'s `FeatureSet` has always read its bits this way.
    #[must_use]
    pub const fn from_wire(bits: u16) -> Self {
        Self(bits & Self::KNOWN_MASK)
    }

    #[must_use]
    pub const fn bits(self) -> u16 {
        self.0
    }

    #[must_use]
    pub const fn contains(self, capability: ControllerCapability) -> bool {
        self.0 & capability.bit() != 0
    }

    #[must_use]
    pub const fn with(self, capability: ControllerCapability) -> Self {
        Self(self.0 | capability.bit())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum PairingRole {
    DeviceInitiator = 1,
    HostResponder = 2,
}

impl PairingRole {
    pub(crate) fn from_wire(value: u8) -> Result<Self> {
        match value {
            1 => Ok(Self::DeviceInitiator),
            2 => Ok(Self::HostResponder),
            _ => Err(ErrorCode::InvalidRole.into()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum PairingStep {
    DeviceHello = 1,
    HostProof = 2,
    DeviceProof = 3,
}

impl PairingStep {
    pub(crate) fn from_wire(value: u8) -> Result<Self> {
        match value {
            1 => Ok(Self::DeviceHello),
            2 => Ok(Self::HostProof),
            3 => Ok(Self::DeviceProof),
            _ => Err(ErrorCode::InvalidStep.into()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PairingState {
    Created,
    Handshaking,
    SasReady,
    Confirmed,
    Rejected,
    Expired,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PairingOfferCore {
    pub version: ControllerProtocolVersion,
    pub expires_at_unix_seconds: u64,
    pub nonce: PairingNonce,
    pub host_static_public_key: HostStaticPublicKey,
    pub capabilities: CapabilitySet,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct RevocationEpoch(pub u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ControllerFrameKind {
    Control = 1,
    Terminal = 2,
    /// Remote Screens: one framed `multiplex-screen-protocol` message.
    Screen = 3,
}

impl ControllerFrameKind {
    pub(crate) fn from_wire(value: u8) -> Result<Self> {
        match value {
            1 => Ok(Self::Control),
            2 => Ok(Self::Terminal),
            3 => Ok(Self::Screen),
            _ => Err(ErrorCode::InvalidEncoding.into()),
        }
    }
}

#[derive(Clone, Eq, PartialEq, Zeroize, ZeroizeOnDrop)]
pub struct ControllerFrame {
    #[zeroize(skip)]
    pub kind: ControllerFrameKind,
    #[zeroize(skip)]
    pub capability: ControllerCapability,
    #[zeroize(skip)]
    pub revocation_epoch: RevocationEpoch,
    #[zeroize(skip)]
    pub sequence: u64,
    pub payload: Vec<u8>,
}

#[derive(Clone, Eq, PartialEq, Zeroize, ZeroizeOnDrop)]
pub struct HandshakeMessage(Vec<u8>);

impl HandshakeMessage {
    pub(crate) fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Clone, Eq, PartialEq, Zeroize, ZeroizeOnDrop)]
pub struct SealedControllerFrame(Vec<u8>);

impl SealedControllerFrame {
    pub(crate) fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for SealedControllerFrame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SealedControllerFrame([REDACTED])")
    }
}

impl fmt::Debug for HandshakeMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HandshakeMessage([REDACTED])")
    }
}

impl fmt::Debug for ControllerFrame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ControllerFrame")
            .field("kind", &self.kind)
            .field("capability", &self.capability)
            .field("revocation_epoch", &self.revocation_epoch)
            .field("sequence", &self.sequence)
            .field("payload", &"[REDACTED]")
            .finish()
    }
}
