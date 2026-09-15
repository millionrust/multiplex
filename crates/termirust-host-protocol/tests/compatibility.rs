use serde::Deserialize;
use termirust_domain::HostedSessionId;
use termirust_host_protocol::wire::{self, envelope_payload};
use termirust_host_protocol::{
    CURRENT_PROTOCOL, FrameKind, PreservedPayload, WireEnvelope, encode_payload, encode_session_id,
    local_limits,
};
use uuid::Uuid;

const GOLDEN_FRAME: &[u8] = include_bytes!("fixtures/handshake-request-v1.trh");
const GOLDEN_MANIFEST: &str = include_str!("fixtures/handshake-request-v1.json");
const SCHEMA: &str = include_str!("../proto/host.proto");
const POLICY: &str = include_str!("../COMPATIBILITY.md");

#[derive(Debug, Deserialize)]
struct Manifest {
    fixture: String,
    wire_format: String,
    protocol_major: u16,
    protocol_minor: u16,
    frame_kind: u16,
    frame_bytes: usize,
    request_id_hex: String,
    session_id: String,
    capabilities: Vec<i32>,
}

fn expected_frame() -> WireEnvelope {
    let session_id = HostedSessionId::from_uuid(Uuid::from_u128(1));
    let payload = wire::EnvelopePayload {
        message: Some(envelope_payload::Message::HandshakeRequest(
            wire::HandshakeRequest {
                session_id: encode_session_id(session_id),
                protocol: Some(CURRENT_PROTOCOL.into()),
                capabilities: golden_capabilities(),
                limits: Some(local_limits().into()),
                client_nonce: vec![0x22; 32],
                request_writer_lease: false,
            },
        )),
    };
    WireEnvelope {
        protocol_major: CURRENT_PROTOCOL.maximum.major,
        protocol_minor: CURRENT_PROTOCOL.maximum.minor,
        kind: FrameKind::HandshakeRequest,
        flags: 0,
        request_id: [0x11; 16],
        payload: encode_payload(&payload),
    }
}

fn golden_capabilities() -> Vec<i32> {
    (1..=7).collect()
}

#[test]
fn golden_v1_binary_decodes_and_reencodes_exactly() {
    let expected = expected_frame();
    let decoded = WireEnvelope::decode(GOLDEN_FRAME).unwrap();
    assert_eq!(decoded, expected);
    assert_eq!(decoded.encode().unwrap(), GOLDEN_FRAME);
    let payload = PreservedPayload::decode(&decoded.payload).unwrap();
    assert_eq!(payload.original_bytes(), decoded.payload);
    assert!(matches!(
        payload.value.message,
        Some(envelope_payload::Message::HandshakeRequest(_))
    ));
}

#[test]
fn golden_json_manifest_describes_binary_without_user_content() {
    let manifest: Manifest = serde_json::from_str(GOLDEN_MANIFEST).unwrap();
    assert_eq!(manifest.fixture, "handshake-request-v1");
    assert_eq!(manifest.wire_format, "TRH1+protobuf");
    assert_eq!(manifest.protocol_major, 1);
    assert_eq!(manifest.protocol_minor, 0);
    assert_eq!(manifest.frame_kind, FrameKind::HandshakeRequest as u16);
    assert_eq!(manifest.frame_bytes, GOLDEN_FRAME.len());
    assert_eq!(manifest.request_id_hex.len(), 32);
    assert_eq!(manifest.session_id, "00000000-0000-0000-0000-000000000001");
    assert_eq!(manifest.capabilities, golden_capabilities());
    assert!(!GOLDEN_MANIFEST.contains("path"));
    assert!(!GOLDEN_MANIFEST.contains("password"));
    assert!(!GOLDEN_MANIFEST.contains("token"));
}

#[test]
fn schema_and_policy_keep_additive_compatibility_guards() {
    assert!(SCHEMA.contains("package termirust.host.v1;"));
    assert!(SCHEMA.contains("reserved 40 to 49;"));
    assert!(!SCHEMA.contains("required "));
    assert!(POLICY.contains("Wire tags and enum\nnumbers are permanent"));
    assert!(POLICY.contains("original protobuf bytes"));
    assert!(POLICY.contains("must not apply a\nmutation"));
}
