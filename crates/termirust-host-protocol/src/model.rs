use std::collections::BTreeSet;
use std::fmt;

use prost::Message as _;
use termirust_domain::{CommandId, HostInstanceId, HostedSessionId};
use uuid::Uuid;

use crate::wire::{self, envelope_payload};
use crate::{
    FrameKind, IDEMPOTENCY_TTL_SECONDS, MAX_FRAME_BYTES, MAX_IDEMPOTENCY_OUTCOMES,
    MAX_OUTBOUND_FRAMES, MAX_OUTPUT_BYTES,
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ProtocolVersion {
    pub major: u16,
    pub minor: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProtocolRange {
    pub minimum: ProtocolVersion,
    pub maximum: ProtocolVersion,
}

impl ProtocolRange {
    pub fn is_valid(self) -> bool {
        self.minimum <= self.maximum && self.minimum.major == self.maximum.major
    }
}

pub fn negotiate_protocol(local: ProtocolRange, peer: ProtocolRange) -> Option<ProtocolVersion> {
    if !local.is_valid() || !peer.is_valid() || local.minimum.major != peer.minimum.major {
        return None;
    }
    let minimum = local.minimum.max(peer.minimum);
    let maximum = local.maximum.min(peer.maximum);
    (minimum <= maximum).then_some(maximum)
}

impl TryFrom<&wire::ProtocolRange> for ProtocolRange {
    type Error = IdDecodeError;

    fn try_from(value: &wire::ProtocolRange) -> Result<Self, Self::Error> {
        let minimum = value.minimum.as_ref().ok_or(IdDecodeError::MissingField)?;
        let maximum = value.maximum.as_ref().ok_or(IdDecodeError::MissingField)?;
        let range = Self {
            minimum: ProtocolVersion::try_from(minimum)?,
            maximum: ProtocolVersion::try_from(maximum)?,
        };
        range
            .is_valid()
            .then_some(range)
            .ok_or(IdDecodeError::InvalidRange)
    }
}

impl TryFrom<&wire::ProtocolVersion> for ProtocolVersion {
    type Error = IdDecodeError;

    fn try_from(value: &wire::ProtocolVersion) -> Result<Self, Self::Error> {
        Ok(Self {
            major: u16::try_from(value.major).map_err(|_| IdDecodeError::IntegerRange)?,
            minor: u16::try_from(value.minor).map_err(|_| IdDecodeError::IntegerRange)?,
        })
    }
}

impl From<ProtocolVersion> for wire::ProtocolVersion {
    fn from(value: ProtocolVersion) -> Self {
        Self {
            major: u32::from(value.major),
            minor: u32::from(value.minor),
        }
    }
}

impl From<ProtocolRange> for wire::ProtocolRange {
    fn from(value: ProtocolRange) -> Self {
        Self {
            minimum: Some(value.minimum.into()),
            maximum: Some(value.maximum.into()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilitySet(BTreeSet<i32>);

impl CapabilitySet {
    pub fn all_local() -> Self {
        Self(
            [
                wire::Capability::State,
                wire::Capability::AttachReplay,
                wire::Capability::Input,
                wire::Capability::Resize,
                wire::Capability::Stop,
                wire::Capability::Interrupt,
                wire::Capability::ActivitySnapshot,
            ]
            .into_iter()
            .map(i32::from)
            .collect(),
        )
    }

    pub fn from_wire(values: &[i32]) -> Self {
        Self(values.iter().copied().filter(|value| *value != 0).collect())
    }

    pub fn intersection(&self, other: &Self) -> Self {
        Self(self.0.intersection(&other.0).copied().collect())
    }

    pub fn to_wire(&self) -> Vec<i32> {
        self.0.iter().copied().collect()
    }

    pub fn contains(&self, capability: wire::Capability) -> bool {
        self.0.contains(&i32::from(capability))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NegotiatedLimits {
    pub maximum_frame_bytes: usize,
    pub maximum_output_bytes: usize,
    pub maximum_outbound_frames: usize,
    pub maximum_idempotency_outcomes: usize,
    pub idempotency_ttl_seconds: u64,
}

impl NegotiatedLimits {
    pub fn bounded_with(self, peer: Self) -> Self {
        Self {
            maximum_frame_bytes: self.maximum_frame_bytes.min(peer.maximum_frame_bytes),
            maximum_output_bytes: self.maximum_output_bytes.min(peer.maximum_output_bytes),
            maximum_outbound_frames: self
                .maximum_outbound_frames
                .min(peer.maximum_outbound_frames),
            maximum_idempotency_outcomes: self
                .maximum_idempotency_outcomes
                .min(peer.maximum_idempotency_outcomes),
            idempotency_ttl_seconds: self
                .idempotency_ttl_seconds
                .min(peer.idempotency_ttl_seconds),
        }
    }

    pub fn is_valid(self) -> bool {
        (crate::ENVELOPE_HEADER_BYTES..=MAX_FRAME_BYTES).contains(&self.maximum_frame_bytes)
            && (1..=MAX_OUTPUT_BYTES).contains(&self.maximum_output_bytes)
            && (1..=MAX_OUTBOUND_FRAMES).contains(&self.maximum_outbound_frames)
            && (1..=MAX_IDEMPOTENCY_OUTCOMES).contains(&self.maximum_idempotency_outcomes)
            && (1..=IDEMPOTENCY_TTL_SECONDS).contains(&self.idempotency_ttl_seconds)
    }
}

impl TryFrom<&wire::Limits> for NegotiatedLimits {
    type Error = IdDecodeError;

    fn try_from(value: &wire::Limits) -> Result<Self, Self::Error> {
        let limits = Self {
            maximum_frame_bytes: usize::try_from(value.maximum_frame_bytes)
                .map_err(|_| IdDecodeError::IntegerRange)?,
            maximum_output_bytes: usize::try_from(value.maximum_output_bytes)
                .map_err(|_| IdDecodeError::IntegerRange)?,
            maximum_outbound_frames: usize::try_from(value.maximum_outbound_frames)
                .map_err(|_| IdDecodeError::IntegerRange)?,
            maximum_idempotency_outcomes: usize::try_from(value.maximum_idempotency_outcomes)
                .map_err(|_| IdDecodeError::IntegerRange)?,
            idempotency_ttl_seconds: u64::from(value.idempotency_ttl_seconds),
        };
        limits
            .is_valid()
            .then_some(limits)
            .ok_or(IdDecodeError::InvalidLimits)
    }
}

impl From<NegotiatedLimits> for wire::Limits {
    fn from(value: NegotiatedLimits) -> Self {
        Self {
            maximum_frame_bytes: u32::try_from(value.maximum_frame_bytes).unwrap_or(u32::MAX),
            maximum_output_bytes: u32::try_from(value.maximum_output_bytes).unwrap_or(u32::MAX),
            maximum_outbound_frames: u32::try_from(value.maximum_outbound_frames)
                .unwrap_or(u32::MAX),
            maximum_idempotency_outcomes: u32::try_from(value.maximum_idempotency_outcomes)
                .unwrap_or(u32::MAX),
            idempotency_ttl_seconds: u32::try_from(value.idempotency_ttl_seconds)
                .unwrap_or(u32::MAX),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdDecodeError {
    InvalidLength,
    InvalidUuid,
    MissingField,
    IntegerRange,
    InvalidRange,
    InvalidLimits,
}

impl fmt::Display for IdDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidLength => "host protocol opaque value has an invalid length",
            Self::InvalidUuid => "host protocol opaque identifier is invalid",
            Self::MissingField => "host protocol required field is absent",
            Self::IntegerRange => "host protocol integer is outside the supported range",
            Self::InvalidRange => "host protocol version range is invalid",
            Self::InvalidLimits => "host protocol limits are invalid",
        })
    }
}

impl std::error::Error for IdDecodeError {}

fn decode_uuid(bytes: &[u8]) -> Result<Uuid, IdDecodeError> {
    if bytes.len() != 16 {
        return Err(IdDecodeError::InvalidLength);
    }
    Uuid::from_slice(bytes).map_err(|_| IdDecodeError::InvalidUuid)
}

pub fn encode_session_id(value: HostedSessionId) -> Vec<u8> {
    value.as_uuid().as_bytes().to_vec()
}

pub fn decode_session_id(bytes: &[u8]) -> Result<HostedSessionId, IdDecodeError> {
    decode_uuid(bytes).map(HostedSessionId::from_uuid)
}

pub fn encode_host_instance_id(value: HostInstanceId) -> Vec<u8> {
    value.as_uuid().as_bytes().to_vec()
}

pub fn decode_host_instance_id(bytes: &[u8]) -> Result<HostInstanceId, IdDecodeError> {
    decode_uuid(bytes).map(HostInstanceId::from_uuid)
}

pub fn encode_command_id(value: CommandId) -> Vec<u8> {
    value.as_uuid().as_bytes().to_vec()
}

pub fn decode_command_id(bytes: &[u8]) -> Result<CommandId, IdDecodeError> {
    decode_uuid(bytes).map(CommandId::from_uuid)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreservedPayload {
    pub value: wire::EnvelopePayload,
    original: Vec<u8>,
}

impl PreservedPayload {
    pub fn decode(bytes: &[u8]) -> Result<Self, prost::DecodeError> {
        Ok(Self {
            value: wire::EnvelopePayload::decode(bytes)?,
            original: bytes.to_vec(),
        })
    }

    pub fn original_bytes(&self) -> &[u8] {
        &self.original
    }

    pub fn into_original_bytes(self) -> Vec<u8> {
        self.original
    }
}

pub fn encode_payload(value: &wire::EnvelopePayload) -> Vec<u8> {
    value.encode_to_vec()
}

pub fn payload_kind(value: &wire::EnvelopePayload) -> Option<FrameKind> {
    Some(match value.message.as_ref()? {
        envelope_payload::Message::HandshakeRequest(_) => FrameKind::HandshakeRequest,
        envelope_payload::Message::HandshakeResponse(_) => FrameKind::HandshakeResponse,
        envelope_payload::Message::GetStateRequest(_) => FrameKind::GetStateRequest,
        envelope_payload::Message::StateEvent(_) => FrameKind::StateEvent,
        envelope_payload::Message::AttachRequest(_) => FrameKind::AttachRequest,
        envelope_payload::Message::ReadyEvent(_) => FrameKind::ReadyEvent,
        envelope_payload::Message::OutputEvent(_) => FrameKind::OutputEvent,
        envelope_payload::Message::ViewportSnapshotEvent(_) => FrameKind::ViewportSnapshotEvent,
        envelope_payload::Message::InputRequest(_) => FrameKind::InputRequest,
        envelope_payload::Message::ResizeRequest(_) => FrameKind::ResizeRequest,
        envelope_payload::Message::StopRequest(_) => FrameKind::StopRequest,
        envelope_payload::Message::InterruptRequest(_) => FrameKind::InterruptRequest,
        envelope_payload::Message::ActivitySnapshotRequest(_) => FrameKind::ActivitySnapshotRequest,
        envelope_payload::Message::DetachRequest(_) => FrameKind::DetachRequest,
        envelope_payload::Message::CommandResult(_) => FrameKind::CommandResult,
        envelope_payload::Message::ExitEvent(_) => FrameKind::ExitEvent,
        envelope_payload::Message::GapEvent(_) => FrameKind::GapEvent,
        envelope_payload::Message::ProtocolError(_) => FrameKind::ProtocolError,
        envelope_payload::Message::LifecycleEvent(_) => FrameKind::LifecycleEvent,
        envelope_payload::Message::ActivityEvent(_) => FrameKind::ActivityEvent,
        envelope_payload::Message::WarningEvent(_) => FrameKind::WarningEvent,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CURRENT_PROTOCOL;

    #[test]
    fn protocol_negotiation_chooses_highest_common_version() {
        let peer = ProtocolRange {
            minimum: ProtocolVersion { major: 1, minor: 0 },
            maximum: ProtocolVersion { major: 1, minor: 4 },
        };
        assert_eq!(
            negotiate_protocol(CURRENT_PROTOCOL, peer),
            Some(CURRENT_PROTOCOL.maximum)
        );
        assert_eq!(
            negotiate_protocol(
                CURRENT_PROTOCOL,
                ProtocolRange {
                    minimum: ProtocolVersion { major: 2, minor: 0 },
                    maximum: ProtocolVersion { major: 2, minor: 0 },
                }
            ),
            None
        );
    }

    #[test]
    fn unknown_minor_fields_can_be_forwarded_byte_for_byte() {
        let payload = wire::EnvelopePayload {
            message: Some(envelope_payload::Message::GetStateRequest(
                wire::GetStateRequest {
                    session_id: vec![1; 16],
                },
            )),
        };
        let mut future = payload.encode_to_vec();
        future.extend_from_slice(&[0xf8, 0x07, 0x01]);
        let preserved = PreservedPayload::decode(&future).unwrap();
        assert_eq!(preserved.original_bytes(), future);
    }
}
