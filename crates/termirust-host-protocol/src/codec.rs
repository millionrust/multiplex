use std::fmt;

pub const FRAME_MAGIC: [u8; 4] = *b"TRH1";
pub const ENVELOPE_HEADER_BYTES: usize = 36;
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
pub const MAX_PAYLOAD_BYTES: usize = MAX_FRAME_BYTES - ENVELOPE_HEADER_BYTES;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum FrameKind {
    HandshakeRequest = 1,
    HandshakeResponse = 2,
    GetStateRequest = 3,
    StateEvent = 4,
    AttachRequest = 5,
    ReadyEvent = 6,
    OutputEvent = 7,
    ViewportSnapshotEvent = 8,
    InputRequest = 9,
    ResizeRequest = 10,
    StopRequest = 11,
    InterruptRequest = 12,
    ActivitySnapshotRequest = 13,
    DetachRequest = 14,
    CommandResult = 15,
    ExitEvent = 16,
    GapEvent = 17,
    ProtocolError = 18,
    LifecycleEvent = 19,
    ActivityEvent = 20,
    WarningEvent = 21,
}

impl TryFrom<u16> for FrameKind {
    type Error = CodecError;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::HandshakeRequest),
            2 => Ok(Self::HandshakeResponse),
            3 => Ok(Self::GetStateRequest),
            4 => Ok(Self::StateEvent),
            5 => Ok(Self::AttachRequest),
            6 => Ok(Self::ReadyEvent),
            7 => Ok(Self::OutputEvent),
            8 => Ok(Self::ViewportSnapshotEvent),
            9 => Ok(Self::InputRequest),
            10 => Ok(Self::ResizeRequest),
            11 => Ok(Self::StopRequest),
            12 => Ok(Self::InterruptRequest),
            13 => Ok(Self::ActivitySnapshotRequest),
            14 => Ok(Self::DetachRequest),
            15 => Ok(Self::CommandResult),
            16 => Ok(Self::ExitEvent),
            17 => Ok(Self::GapEvent),
            18 => Ok(Self::ProtocolError),
            19 => Ok(Self::LifecycleEvent),
            20 => Ok(Self::ActivityEvent),
            21 => Ok(Self::WarningEvent),
            _ => Err(CodecError::UnknownFrameKind),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WireEnvelope {
    pub protocol_major: u16,
    pub protocol_minor: u16,
    pub kind: FrameKind,
    pub flags: u16,
    pub request_id: [u8; 16],
    pub payload: Vec<u8>,
}

impl WireEnvelope {
    pub fn encode(&self) -> Result<Vec<u8>, CodecError> {
        if self.payload.len() > MAX_PAYLOAD_BYTES {
            return Err(CodecError::FrameTooLarge);
        }
        let payload_len =
            u32::try_from(self.payload.len()).map_err(|_| CodecError::FrameTooLarge)?;
        let mut bytes = Vec::with_capacity(ENVELOPE_HEADER_BYTES + self.payload.len());
        bytes.extend_from_slice(&FRAME_MAGIC);
        bytes.extend_from_slice(&self.protocol_major.to_be_bytes());
        bytes.extend_from_slice(&self.protocol_minor.to_be_bytes());
        bytes.extend_from_slice(&(self.kind as u16).to_be_bytes());
        bytes.extend_from_slice(&self.flags.to_be_bytes());
        bytes.extend_from_slice(&self.request_id);
        bytes.extend_from_slice(&payload_len.to_be_bytes());
        bytes.extend_from_slice(&0_u32.to_be_bytes());
        bytes.extend_from_slice(&self.payload);
        let checksum = checksum(&bytes[..32], &self.payload);
        bytes[32..36].copy_from_slice(&checksum.to_be_bytes());
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        if bytes.len() < ENVELOPE_HEADER_BYTES {
            return Err(CodecError::Truncated);
        }
        if bytes.len() > MAX_FRAME_BYTES {
            return Err(CodecError::FrameTooLarge);
        }
        if bytes[..4] != FRAME_MAGIC {
            return Err(CodecError::InvalidMagic);
        }
        let payload_len = u32::from_be_bytes(
            bytes[28..32]
                .try_into()
                .map_err(|_| CodecError::Truncated)?,
        ) as usize;
        if payload_len > MAX_PAYLOAD_BYTES {
            return Err(CodecError::FrameTooLarge);
        }
        let expected_len = ENVELOPE_HEADER_BYTES
            .checked_add(payload_len)
            .ok_or(CodecError::FrameTooLarge)?;
        if bytes.len() != expected_len {
            return Err(if bytes.len() < expected_len {
                CodecError::Truncated
            } else {
                CodecError::TrailingBytes
            });
        }
        let expected_checksum = u32::from_be_bytes(
            bytes[32..36]
                .try_into()
                .map_err(|_| CodecError::Truncated)?,
        );
        let actual_checksum = checksum(&bytes[..32], &bytes[36..]);
        if expected_checksum != actual_checksum {
            return Err(CodecError::ChecksumMismatch);
        }
        Ok(Self {
            protocol_major: u16::from_be_bytes(
                bytes[4..6].try_into().map_err(|_| CodecError::Truncated)?,
            ),
            protocol_minor: u16::from_be_bytes(
                bytes[6..8].try_into().map_err(|_| CodecError::Truncated)?,
            ),
            kind: FrameKind::try_from(u16::from_be_bytes(
                bytes[8..10].try_into().map_err(|_| CodecError::Truncated)?,
            ))?,
            flags: u16::from_be_bytes(
                bytes[10..12]
                    .try_into()
                    .map_err(|_| CodecError::Truncated)?,
            ),
            request_id: bytes[12..28]
                .try_into()
                .map_err(|_| CodecError::Truncated)?,
            payload: bytes[36..].to_vec(),
        })
    }
}

fn checksum(header_without_checksum: &[u8], payload: &[u8]) -> u32 {
    let mut material = Vec::with_capacity(header_without_checksum.len() + payload.len());
    material.extend_from_slice(header_without_checksum);
    material.extend_from_slice(payload);
    crc32c::crc32c(&material)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodecError {
    InvalidMagic,
    UnknownFrameKind,
    FrameTooLarge,
    ChecksumMismatch,
    Truncated,
    TrailingBytes,
    BufferLimit,
}

impl fmt::Display for CodecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidMagic => "host protocol frame has invalid magic",
            Self::UnknownFrameKind => "host protocol frame kind is unsupported",
            Self::FrameTooLarge => "host protocol frame exceeds the byte limit",
            Self::ChecksumMismatch => "host protocol frame checksum does not match",
            Self::Truncated => "host protocol frame is incomplete",
            Self::TrailingBytes => "host protocol frame has trailing bytes",
            Self::BufferLimit => "host protocol decoder buffer limit reached",
        })
    }
}

impl std::error::Error for CodecError {}

#[derive(Debug, Default)]
pub struct FrameDecoder {
    buffer: Vec<u8>,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn buffered_len(&self) -> usize {
        self.buffer.len()
    }

    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<WireEnvelope>, CodecError> {
        if self
            .buffer
            .len()
            .checked_add(bytes.len())
            .is_none_or(|length| length > MAX_FRAME_BYTES)
        {
            return Err(CodecError::BufferLimit);
        }
        self.buffer.extend_from_slice(bytes);
        let mut decoded = Vec::new();
        loop {
            if self.buffer.len() < ENVELOPE_HEADER_BYTES {
                break;
            }
            if self.buffer[..4] != FRAME_MAGIC {
                return Err(CodecError::InvalidMagic);
            }
            let payload_len = u32::from_be_bytes(
                self.buffer[28..32]
                    .try_into()
                    .map_err(|_| CodecError::Truncated)?,
            ) as usize;
            if payload_len > MAX_PAYLOAD_BYTES {
                return Err(CodecError::FrameTooLarge);
            }
            let frame_len = ENVELOPE_HEADER_BYTES
                .checked_add(payload_len)
                .ok_or(CodecError::FrameTooLarge)?;
            if self.buffer.len() < frame_len {
                break;
            }
            let frame = self.buffer.drain(..frame_len).collect::<Vec<_>>();
            decoded.push(WireEnvelope::decode(&frame)?);
        }
        Ok(decoded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn envelope(payload: Vec<u8>) -> WireEnvelope {
        WireEnvelope {
            protocol_major: 1,
            protocol_minor: 0,
            kind: FrameKind::OutputEvent,
            flags: 0,
            request_id: [7; 16],
            payload,
        }
    }

    #[test]
    fn fragmented_and_coalesced_frames_decode_exactly() {
        let first = envelope(b"first".to_vec()).encode().unwrap();
        let second = envelope(b"second".to_vec()).encode().unwrap();
        for split in 0..first.len() {
            let mut decoder = FrameDecoder::new();
            assert!(decoder.push(&first[..split]).unwrap().is_empty());
            let mut tail = first[split..].to_vec();
            tail.extend_from_slice(&second);
            assert_eq!(
                decoder.push(&tail).unwrap(),
                [envelope(b"first".to_vec()), envelope(b"second".to_vec())]
            );
        }
    }

    #[test]
    fn malformed_checksum_and_oversize_prefix_fail_closed() {
        let mut corrupted = envelope(b"content".to_vec()).encode().unwrap();
        *corrupted.last_mut().unwrap() ^= 1;
        assert_eq!(
            WireEnvelope::decode(&corrupted),
            Err(CodecError::ChecksumMismatch)
        );

        let mut prefix = vec![0_u8; ENVELOPE_HEADER_BYTES];
        prefix[..4].copy_from_slice(&FRAME_MAGIC);
        prefix[28..32].copy_from_slice(&(MAX_FRAME_BYTES as u32).to_be_bytes());
        assert_eq!(
            FrameDecoder::new().push(&prefix),
            Err(CodecError::FrameTooLarge)
        );
    }

    #[test]
    fn exact_maximum_frame_round_trips_and_one_byte_over_is_rejected() {
        let maximum = envelope(vec![0x5a; MAX_PAYLOAD_BYTES]);
        let encoded = maximum.encode().unwrap();
        assert_eq!(encoded.len(), MAX_FRAME_BYTES);
        assert_eq!(WireEnvelope::decode(&encoded).unwrap(), maximum);
        assert_eq!(
            envelope(vec![0; MAX_PAYLOAD_BYTES + 1]).encode(),
            Err(CodecError::FrameTooLarge)
        );
    }

    proptest! {
        #[test]
        fn arbitrary_payload_and_fragmentation_round_trip(
            payload in proptest::collection::vec(any::<u8>(), 0..8192),
            chunk_size in 1_usize..257,
        ) {
            let expected = envelope(payload);
            let encoded = expected.encode().unwrap();
            let mut decoder = FrameDecoder::new();
            let mut actual = Vec::new();
            for chunk in encoded.chunks(chunk_size) {
                actual.extend(decoder.push(chunk).unwrap());
            }
            prop_assert_eq!(actual, [expected]);
            prop_assert_eq!(decoder.buffered_len(), 0);
        }
    }

    #[test]
    fn codec_fuzz_smoke() {
        let mut value = 0x9e37_79b9_7f4a_7c15_u64;
        for length in 0..20_000_usize {
            let bounded_length = length % 2_048;
            let mut input = vec![0_u8; bounded_length];
            for byte in &mut input {
                value ^= value << 13;
                value ^= value >> 7;
                value ^= value << 17;
                *byte = value as u8;
            }
            let mut decoder = FrameDecoder::new();
            let _ = decoder.push(&input);
            assert!(decoder.buffered_len() <= MAX_FRAME_BYTES);
        }
    }
}
