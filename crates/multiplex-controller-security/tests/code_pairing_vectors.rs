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
    serde_json::from_str(include_str!("vectors/controller-code-v1.json")).unwrap()
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
