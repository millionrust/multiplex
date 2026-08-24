use std::fs;
use std::path::PathBuf;

use serde::Serialize;
use termirust_domain::HostedSessionId;
use termirust_host_protocol::wire::{self, envelope_payload};
use termirust_host_protocol::{
    CURRENT_PROTOCOL, CapabilitySet, FrameKind, WireEnvelope, encode_payload, encode_session_id,
    local_limits,
};
use uuid::Uuid;

#[derive(Serialize)]
struct Manifest {
    fixture: &'static str,
    wire_format: &'static str,
    protocol_major: u16,
    protocol_minor: u16,
    frame_kind: u16,
    frame_bytes: usize,
    request_id_hex: &'static str,
    session_id: String,
    capabilities: Vec<i32>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let session_id = HostedSessionId::from_uuid(Uuid::from_u128(1));
    let request_id = [0x11; 16];
    let capabilities = CapabilitySet::all_local().to_wire();
    let payload = wire::EnvelopePayload {
        message: Some(envelope_payload::Message::HandshakeRequest(
            wire::HandshakeRequest {
                session_id: encode_session_id(session_id),
                protocol: Some(CURRENT_PROTOCOL.into()),
                capabilities: capabilities.clone(),
                limits: Some(local_limits().into()),
                client_nonce: vec![0x22; 32],
            },
        )),
    };
    let frame = WireEnvelope {
        protocol_major: CURRENT_PROTOCOL.maximum.major,
        protocol_minor: CURRENT_PROTOCOL.maximum.minor,
        kind: FrameKind::HandshakeRequest,
        flags: 0,
        request_id,
        payload: encode_payload(&payload),
    }
    .encode()?;
    let output = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    fs::create_dir_all(&output)?;
    fs::write(output.join("handshake-request-v1.trh"), &frame)?;
    let manifest = Manifest {
        fixture: "handshake-request-v1",
        wire_format: "TRH1+protobuf",
        protocol_major: CURRENT_PROTOCOL.maximum.major,
        protocol_minor: CURRENT_PROTOCOL.maximum.minor,
        frame_kind: FrameKind::HandshakeRequest as u16,
        frame_bytes: frame.len(),
        request_id_hex: "11111111111111111111111111111111",
        session_id: session_id.to_string(),
        capabilities,
    };
    fs::write(
        output.join("handshake-request-v1.json"),
        format!("{}\n", serde_json::to_string_pretty(&manifest)?),
    )?;
    Ok(())
}
