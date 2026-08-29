use serde_json::json;
use std::env;
use std::fs;
use std::path::Path;
use termirust_relay_protocol::{
    ADMISSION_CHALLENGE_BYTES, ADMISSION_LIFETIME_SECONDS, ADMISSION_PROOF_BYTES,
    ADMISSION_RESULT_BYTES, CLIENT_HELLO_BYTES, MAX_CIPHERTEXT_PAYLOAD_BYTES,
    MAX_ENCODED_WEBSOCKET_MESSAGE_BYTES, MAX_FAILED_ADMISSIONS_PER_SOURCE, MAX_FORWARDING_PAIRS,
    MAX_QUEUE_ENCODED_BYTES, MAX_QUEUE_MESSAGES, MAX_REGISTERED_ROUTES,
    MAX_UNAUTHENTICATED_HANDSHAKES, RATE_BURST_BYTES, RATE_BYTES_PER_SECOND,
    RELAY_ENVELOPE_HEADER_BYTES, RELAY_V1, RelayAdmissionChallenge, RelayAdmissionCredential,
    RelayAdmissionResult, RelayClientHello, RelayConnectionId, RelayConnectionSequence,
    RelayDiagnosticCode, RelayDirection, RelayEndpointRole, RelayEnvelopeV1, RelayRevocationEpoch,
    RelayRouteId,
};

fn main() {
    let arguments: Vec<String> = env::args().skip(1).collect();
    if arguments.len() != 2 || !matches!(arguments[0].as_str(), "--check" | "--write") {
        eprintln!("usage: relay_vectors {{--check|--write}} <path>");
        std::process::exit(2);
    }
    let expected = render();
    let path = Path::new(&arguments[1]);
    match arguments[0].as_str() {
        "--write" => {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create vector directory");
            }
            fs::write(path, expected).expect("write vectors");
        }
        "--check" => {
            let actual = fs::read_to_string(path).expect("read vectors");
            if actual != expected {
                eprintln!("relay-v1 vectors are stale; regenerate with --write");
                std::process::exit(1);
            }
        }
        _ => unreachable!(),
    }
}

fn render() -> String {
    let route_id = RelayRouteId([0x22; 32]);
    let credential = RelayAdmissionCredential::from_fixture_bytes([0x11; 32]);
    let challenge = RelayAdmissionChallenge {
        route_id,
        role: RelayEndpointRole::Host,
        verifier: credential.verifier(),
        revocation_epoch: RelayRevocationEpoch(7),
        serial: 9,
        expires_at_unix_seconds: 1_234,
        nonce: [0x33; 32],
    };
    let proof = credential.prove(&challenge);
    let envelope = RelayEnvelopeV1::new(
        route_id,
        RelayDirection::HostToController,
        RelayConnectionSequence(5),
        vec![0x44, 0x55, 0x66],
    )
    .expect("fixture envelope");
    let diagnostics: Vec<_> = RelayDiagnosticCode::ALL
        .into_iter()
        .map(|code| json!({"number": code.as_u16(), "code": code.as_str()}))
        .collect();
    let value = json!({
        "schema": "termirust-relay-v1-vectors",
        "schema_version": 1,
        "protocol_version": RELAY_V1.0,
        "limits": {
            "ciphertext_payload_bytes": MAX_CIPHERTEXT_PAYLOAD_BYTES,
            "encoded_websocket_message_bytes": MAX_ENCODED_WEBSOCKET_MESSAGE_BYTES,
            "envelope_header_bytes": RELAY_ENVELOPE_HEADER_BYTES,
            "registered_routes": MAX_REGISTERED_ROUTES,
            "forwarding_pairs": MAX_FORWARDING_PAIRS,
            "unauthenticated_handshakes": MAX_UNAUTHENTICATED_HANDSHAKES,
            "failed_admissions_per_source": MAX_FAILED_ADMISSIONS_PER_SOURCE,
            "admission_lifetime_seconds": ADMISSION_LIFETIME_SECONDS,
            "queue_messages": MAX_QUEUE_MESSAGES,
            "queue_encoded_bytes": MAX_QUEUE_ENCODED_BYTES,
            "rate_bytes_per_second": RATE_BYTES_PER_SECOND,
            "rate_burst_bytes": RATE_BURST_BYTES
        },
        "wire_sizes": {
            "client_hello": CLIENT_HELLO_BYTES,
            "admission_challenge": ADMISSION_CHALLENGE_BYTES,
            "admission_proof": ADMISSION_PROOF_BYTES,
            "admission_result": ADMISSION_RESULT_BYTES
        },
        "vectors": {
            "client_hello_hex": hex(&RelayClientHello { route_id, role: RelayEndpointRole::Host }.encode()),
            "challenge_hex": hex(&challenge.encode()),
            "proof_hex": hex(&proof.encode()),
            "accepted_hex": hex(&RelayAdmissionResult::accepted(RelayConnectionId(12)).encode()),
            "rejected_hex": hex(&RelayAdmissionResult::rejected(RelayDiagnosticCode::InvalidProof).encode()),
            "envelope_hex": hex(&envelope.encode()),
            "host_verifier_hex": hex(&credential.verifier().0)
        },
        "diagnostics": diagnostics
    });
    format!(
        "{}\n",
        serde_json::to_string_pretty(&value).expect("serialize vectors")
    )
}

fn hex(bytes: &[u8]) -> String {
    hex::encode(bytes)
}
