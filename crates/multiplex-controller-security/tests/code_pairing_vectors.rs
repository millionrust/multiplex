mod common;

use multiplex_controller_security::{
    CodeKeyExchange, PairingCode, PairingMachine, PairingNonce, PairingRole, encode_offer,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct Fixture {
    code: String,
    device_nonce_hex: String,
    device_scalar_entropy_byte: u8,
    host_scalar_entropy_byte: u8,
    offer_hex: String,
    device_share_hex: String,
    host_share_hex: String,
    message_1_hex: String,
    message_2_hex: String,
    message_3_hex: String,
    handshake_hash_hex: String,
}

fn fixture() -> Fixture {
    serde_json::from_str(include_str!("vectors/controller-code-v2.json")).unwrap()
}

#[test]
fn code_pairing_shares_messages_and_transcript_are_reproducible() {
    let fixture = fixture();
    let offer = common::offer();
    assert_eq!(
        hex::encode(encode_offer(&offer).unwrap()),
        fixture.offer_hex
    );
    let code = PairingCode::parse(&fixture.code).unwrap();
    let nonce = PairingNonce(
        hex::decode(&fixture.device_nonce_hex)
            .unwrap()
            .try_into()
            .unwrap(),
    );
    let device = CodeKeyExchange::new(
        PairingRole::DeviceInitiator,
        &code,
        &offer,
        &nonce,
        [fixture.device_scalar_entropy_byte; 64],
    )
    .unwrap();
    let host = CodeKeyExchange::new(
        PairingRole::HostResponder,
        &code,
        &offer,
        &nonce,
        [fixture.host_scalar_entropy_byte; 64],
    )
    .unwrap();
    let (device_share, host_share) = (device.share(), host.share());
    assert_eq!(hex::encode(device_share), fixture.device_share_hex);
    assert_eq!(hex::encode(host_share), fixture.host_share_hex);
    let device_binding = device.finish(&host_share).unwrap();
    let host_binding = host.finish(&device_share).unwrap();

    let mut device = PairingMachine::new_device_initiator_with_code(
        offer.clone(),
        &device_binding,
        common::device_static(),
        common::device_ephemeral(),
        common::NOW_MILLIS,
        common::NOW_SECONDS,
    )
    .unwrap();
    let mut host = PairingMachine::new_host_responder_with_code(
        offer,
        &host_binding,
        common::host_static(),
        common::host_ephemeral(),
        common::NOW_MILLIS,
        common::NOW_SECONDS,
    )
    .unwrap();
    let message_1 = device.write_next(common::NOW_MILLIS + 1).unwrap();
    assert_eq!(hex::encode(message_1.as_bytes()), fixture.message_1_hex);
    host.read_next(message_1.as_bytes(), common::NOW_MILLIS + 2)
        .unwrap();
    let message_2 = host.write_next(common::NOW_MILLIS + 3).unwrap();
    assert_eq!(hex::encode(message_2.as_bytes()), fixture.message_2_hex);
    device
        .read_next(message_2.as_bytes(), common::NOW_MILLIS + 4)
        .unwrap();
    let message_3 = device.write_next(common::NOW_MILLIS + 5).unwrap();
    assert_eq!(hex::encode(message_3.as_bytes()), fixture.message_3_hex);
    host.read_next(message_3.as_bytes(), common::NOW_MILLIS + 6)
        .unwrap();
    assert_eq!(
        hex::encode(host.handshake_hash().unwrap().0),
        fixture.handshake_hash_hex
    );
    assert_eq!(device.handshake_hash(), host.handshake_hash());
}

#[test]
fn the_recorded_host_proof_fails_under_any_other_code() {
    let fixture = fixture();
    let offer = common::offer();
    let nonce = PairingNonce(
        hex::decode(&fixture.device_nonce_hex)
            .unwrap()
            .try_into()
            .unwrap(),
    );
    for other in ["305918", "000000"] {
        let device = CodeKeyExchange::new(
            PairingRole::DeviceInitiator,
            &PairingCode::parse(other).unwrap(),
            &offer,
            &nonce,
            [fixture.device_scalar_entropy_byte; 64],
        )
        .unwrap();
        let binding = device
            .finish(&hex::decode(&fixture.host_share_hex).unwrap())
            .unwrap();
        let mut device = PairingMachine::new_device_initiator_with_code(
            offer.clone(),
            &binding,
            common::device_static(),
            common::device_ephemeral(),
            common::NOW_MILLIS,
            common::NOW_SECONDS,
        )
        .unwrap();
        device.write_next(common::NOW_MILLIS + 1).unwrap();
        assert!(
            device
                .read_next(
                    &hex::decode(&fixture.message_2_hex).unwrap(),
                    common::NOW_MILLIS + 2
                )
                .is_err()
        );
    }
}

/// Writes this fixture from the live construction, for the review the ADR's change-control
/// section requires. Deliberate, like the pairing vector's own regeneration:
///
/// ```text
/// cargo test -p multiplex-controller-security --test code_pairing_vectors -- \
///     --ignored write_code_pairing_vectors
/// ```
#[test]
#[ignore = "writes the code-pairing vectors; run deliberately, and review what it wrote"]
fn write_code_pairing_vectors() {
    const CODE: &str = "305917";
    const DEVICE_NONCE: [u8; 32] = [
        0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xab, 0xac, 0xad, 0xae,
        0xaf, 0xb0, 0xb1, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xbb, 0xbc, 0xbd,
        0xbe, 0xbf,
    ];
    const DEVICE_ENTROPY: u8 = 17;
    const HOST_ENTROPY: u8 = 34;

    let offer = common::offer();
    let code = PairingCode::parse(CODE).expect("the code parses");
    let nonce = PairingNonce(DEVICE_NONCE);
    let device_exchange = CodeKeyExchange::new(
        PairingRole::DeviceInitiator,
        &code,
        &offer,
        &nonce,
        [DEVICE_ENTROPY; 64],
    )
    .expect("the device share");
    let host_exchange = CodeKeyExchange::new(
        PairingRole::HostResponder,
        &code,
        &offer,
        &nonce,
        [HOST_ENTROPY; 64],
    )
    .expect("the host share");
    let (device_share, host_share) = (device_exchange.share(), host_exchange.share());
    let device_binding = device_exchange
        .finish(&host_share)
        .expect("the device binding");
    let host_binding = host_exchange
        .finish(&device_share)
        .expect("the host binding");

    let mut device = PairingMachine::new_device_initiator_with_code(
        offer.clone(),
        &device_binding,
        common::device_static(),
        common::device_ephemeral(),
        common::NOW_MILLIS,
        common::NOW_SECONDS,
    )
    .expect("the device machine starts");
    let mut host = PairingMachine::new_host_responder_with_code(
        offer.clone(),
        &host_binding,
        common::host_static(),
        common::host_ephemeral(),
        common::NOW_MILLIS,
        common::NOW_SECONDS,
    )
    .expect("the host machine starts");
    let message_1 = device
        .write_next(common::NOW_MILLIS + 1)
        .expect("message 1");
    host.read_next(message_1.as_bytes(), common::NOW_MILLIS + 2)
        .expect("message 1 is read");
    let message_2 = host.write_next(common::NOW_MILLIS + 3).expect("message 2");
    device
        .read_next(message_2.as_bytes(), common::NOW_MILLIS + 4)
        .expect("message 2 is read");
    let message_3 = device
        .write_next(common::NOW_MILLIS + 5)
        .expect("message 3");
    host.read_next(message_3.as_bytes(), common::NOW_MILLIS + 6)
        .expect("message 3 is read");

    let document = format!(
        r#"{{
  "description": "Code pairing fixture: CPace over Ristretto255/SHA-512 keyed by a six-digit code, bound into the Controller-v2 XX prologue. FIXTURE-ONLY; NEVER USE IN PRODUCTION.",
  "code": "{code}",
  "device_nonce_hex": "{nonce}",
  "device_scalar_entropy_byte": {device_entropy},
  "host_scalar_entropy_byte": {host_entropy},
  "offer_hex": "{offer}",
  "device_share_hex": "{device_share}",
  "host_share_hex": "{host_share}",
  "message_1_hex": "{message_1}",
  "message_2_hex": "{message_2}",
  "message_3_hex": "{message_3}",
  "handshake_hash_hex": "{handshake_hash}"
}}
"#,
        code = CODE,
        nonce = hex::encode(DEVICE_NONCE),
        device_entropy = DEVICE_ENTROPY,
        host_entropy = HOST_ENTROPY,
        offer = hex::encode(encode_offer(&offer).expect("the offer encodes")),
        device_share = hex::encode(device_share),
        host_share = hex::encode(host_share),
        message_1 = hex::encode(message_1.as_bytes()),
        message_2 = hex::encode(message_2.as_bytes()),
        message_3 = hex::encode(message_3.as_bytes()),
        handshake_hash = hex::encode(host.handshake_hash().expect("a handshake hash").0),
    );
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/vectors/controller-code-v2.json");
    std::fs::write(&path, document).expect("the regenerated vectors are written");
    println!("wrote {}", path.display());
}
