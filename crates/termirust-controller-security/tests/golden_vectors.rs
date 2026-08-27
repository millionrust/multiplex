mod common;

use serde::Deserialize;
use sha2::{Digest, Sha256};
use termirust_controller_security::{
    ControllerCapability, ControllerFrameKind, RevocationEpoch, decode_offer, encode_offer,
    pairing_prologue,
};

#[derive(Deserialize)]
struct Vector {
    noise_protocol: String,
    implementation: String,
    offer_hex: String,
    prologue_hex: String,
    host_static_private_hex: String,
    host_static_public_hex: String,
    host_ephemeral_private_hex: String,
    host_ephemeral_public_hex: String,
    device_static_private_hex: String,
    device_static_public_hex: String,
    device_ephemeral_private_hex: String,
    device_ephemeral_public_hex: String,
    message_1_hex: String,
    message_2_hex: String,
    message_3_hex: String,
    handshake_hash_hex: String,
    sas_display: String,
    initiator_to_responder_key_hex: String,
    responder_to_initiator_key_hex: String,
    first_frame_hex: String,
    last_frame_hex: String,
    last_sequence: u64,
    adr_sha256: String,
    cargo_lock_sha256: String,
    normative_sas_anchor: SasAnchor,
}

#[derive(Deserialize)]
struct SasAnchor {
    pairing_nonce_hex: String,
    handshake_hash_hex: String,
    host_static_public_hex: String,
    device_static_public_hex: String,
    salt_hex: String,
    info_hex: String,
    hkdf_output_hex: String,
    sas_display: String,
}

fn vector() -> Vector {
    serde_json::from_str(include_str!("vectors/controller-v1.json"))
        .unwrap_or_else(|error| panic!("golden vector JSON failed: {error}"))
}

#[test]
fn exact_offer_handshake_sas_and_first_transport_frame_are_reproducible() {
    let expected = vector();
    assert_eq!(
        expected.noise_protocol,
        termirust_controller_security::NOISE_PROTOCOL_NAME
    );
    assert_eq!(expected.implementation, "clatter=2.2.0");
    let offer = common::offer();
    assert_eq!(
        hex::encode(encode_offer(&offer).unwrap_or_else(|error| panic!("offer: {error}"))),
        expected.offer_hex
    );
    assert_eq!(
        hex::encode(pairing_prologue(&offer).unwrap_or_else(|error| panic!("prologue: {error}"))),
        expected.prologue_hex
    );
    let decoded = decode_offer(
        &hex::decode(&expected.offer_hex).unwrap_or_else(|error| panic!("offer hex: {error}")),
    )
    .unwrap_or_else(|error| panic!("offer decode: {error}"));
    assert_eq!(decoded, offer);

    let (device, host, messages) = common::complete_handshake();
    assert_eq!(hex::encode(&messages[0]), expected.message_1_hex);
    assert_eq!(hex::encode(&messages[1]), expected.message_2_hex);
    assert_eq!(hex::encode(&messages[2]), expected.message_3_hex);
    assert_eq!(
        hex::encode(
            device
                .handshake_hash()
                .unwrap_or_else(|| panic!("hash missing"))
                .0
        ),
        expected.handshake_hash_hex
    );
    assert_eq!(
        device
            .sas()
            .unwrap_or_else(|| panic!("SAS missing"))
            .as_str(),
        expected.sas_display
    );
    assert_eq!(host.sas(), device.sas());
    assert_eq!(
        hex::encode(offer.host_static_public_key.0),
        expected.host_static_public_hex
    );
    assert_eq!(
        hex::encode(
            termirust_controller_security::device_public_key_from_private(&common::device_static())
                .0
        ),
        expected.device_static_public_hex
    );
    for (private_hex, public_hex) in [
        (
            &expected.host_static_private_hex,
            &expected.host_static_public_hex,
        ),
        (
            &expected.host_ephemeral_private_hex,
            &expected.host_ephemeral_public_hex,
        ),
        (
            &expected.device_static_private_hex,
            &expected.device_static_public_hex,
        ),
        (
            &expected.device_ephemeral_private_hex,
            &expected.device_ephemeral_public_hex,
        ),
    ] {
        let private = array32(private_hex);
        let public = termirust_controller_security::device_public_key_from_private(
            &termirust_controller_security::StaticPrivateKey::from_fixture_bytes(private),
        );
        assert_eq!(hex::encode(public.0), *public_hex);
    }

    let sas = device
        .sas()
        .cloned()
        .unwrap_or_else(|| panic!("SAS missing"));
    let mut confirmed = device
        .confirm(&sas, RevocationEpoch(4))
        .unwrap_or_else(|error| panic!("confirm: {error}"));
    let frame = confirmed
        .transport
        .seal(
            ControllerFrameKind::Control,
            ControllerCapability::ObserveSessions,
            RevocationEpoch(4),
            b"controller-v1-first",
        )
        .unwrap_or_else(|error| panic!("seal: {error}"));
    assert_eq!(hex::encode(frame.as_bytes()), expected.first_frame_hex);
}

#[test]
fn fixture_locks_transport_keys_last_sequence_and_document_checksums() {
    let expected = vector();
    assert_eq!(expected.initiator_to_responder_key_hex.len(), 64);
    assert_eq!(expected.responder_to_initiator_key_hex.len(), 64);
    assert_ne!(
        expected.initiator_to_responder_key_hex,
        expected.responder_to_initiator_key_hex
    );
    assert_eq!(
        expected.last_sequence,
        termirust_controller_security::MAX_SEQUENCE
    );
    assert_eq!(&expected.last_frame_hex[40..56], "fffffffffffffffd");
    assert_eq!(
        hex::encode(Sha256::digest(include_bytes!(
            "../../../docs/decisions/controller-security-v1.md"
        ))),
        expected.adr_sha256
    );
    assert_eq!(
        hex::encode(Sha256::digest(include_bytes!("../../../Cargo.lock"))),
        expected.cargo_lock_sha256
    );
}

#[test]
fn normative_anchor_locks_salt_info_hkdf_and_display() {
    use hkdf::Hkdf;

    let anchor = vector().normative_sas_anchor;
    let nonce = array32(&anchor.pairing_nonce_hex);
    let hash = array32(&anchor.handshake_hash_hex);
    let host = array32(&anchor.host_static_public_hex);
    let device = array32(&anchor.device_static_public_hex);
    let mut salt_input = b"termirust-controller-sas-v1\0".to_vec();
    salt_input.extend_from_slice(&nonce);
    let salt = Sha256::digest(&salt_input);
    assert_eq!(hex::encode(salt), anchor.salt_hex);

    let mut info = b"sas\0".to_vec();
    info.extend_from_slice(&1_u16.to_be_bytes());
    info.extend_from_slice(&0_u16.to_be_bytes());
    info.extend_from_slice(&host);
    info.extend_from_slice(&device);
    assert_eq!(hex::encode(&info), anchor.info_hex);
    let hkdf = Hkdf::<Sha256>::new(Some(&salt), &hash);
    let mut output = [0_u8; 5];
    hkdf.expand(&info, &mut output)
        .unwrap_or_else(|_| panic!("anchor HKDF expansion failed"));
    assert_eq!(hex::encode(output), anchor.hkdf_output_hex);
    let sas = termirust_controller_security::derive_sas_v1(
        &termirust_controller_security::PairingNonce(nonce),
        &termirust_controller_security::HandshakeHash(hash),
        termirust_controller_security::CONTROLLER_V1,
        termirust_controller_security::HostStaticPublicKey(host),
        termirust_controller_security::DeviceStaticPublicKey(device),
    )
    .unwrap_or_else(|error| panic!("anchor SAS failed: {error}"));
    assert_eq!(sas.as_str(), anchor.sas_display);
}

fn array32(value: &str) -> [u8; 32] {
    hex::decode(value)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .unwrap_or_else(|| panic!("fixture field is not 32 bytes"))
}
