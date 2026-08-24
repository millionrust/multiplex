mod codec;
mod model;

pub mod wire {
    include!(concat!(env!("OUT_DIR"), "/termirust.host.v1.rs"));
}

pub use codec::{
    CodecError, ENVELOPE_HEADER_BYTES, FRAME_MAGIC, FrameDecoder, FrameKind, MAX_FRAME_BYTES,
    MAX_PAYLOAD_BYTES, WireEnvelope,
};
pub use model::{
    CapabilitySet, IdDecodeError, NegotiatedLimits, PreservedPayload, ProtocolRange,
    ProtocolVersion, decode_command_id, decode_host_instance_id, decode_session_id,
    encode_command_id, encode_host_instance_id, encode_payload, encode_session_id,
    negotiate_protocol, payload_kind,
};

pub const CURRENT_PROTOCOL: ProtocolRange = ProtocolRange {
    minimum: ProtocolVersion { major: 1, minor: 0 },
    maximum: ProtocolVersion { major: 1, minor: 0 },
};
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024;
pub const MAX_OUTBOUND_FRAMES: usize = 1_024;
pub const MAX_IDEMPOTENCY_OUTCOMES: usize = 1_024;
pub const IDEMPOTENCY_TTL_SECONDS: u64 = 10 * 60;
pub const MAX_REPLAY_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_REPLAY_RECORDS: u32 = 50_000;
pub const HANDSHAKE_NONCE_BYTES: usize = 32;

pub fn local_limits() -> NegotiatedLimits {
    NegotiatedLimits {
        maximum_frame_bytes: MAX_FRAME_BYTES,
        maximum_output_bytes: MAX_OUTPUT_BYTES,
        maximum_outbound_frames: MAX_OUTBOUND_FRAMES,
        maximum_idempotency_outcomes: MAX_IDEMPOTENCY_OUTCOMES,
        idempotency_ttl_seconds: IDEMPOTENCY_TTL_SECONDS,
    }
}
