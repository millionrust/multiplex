mod common;

use multiplex_controller_security::{
    ControllerCapability, ControllerFrameKind, RevocationEpoch, decode_offer, encode_offer,
    pairing_prologue,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};

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
    screen_amendment: ScreenAmendment,
}

/// Amendment 1: the Remote Screens capabilities and the screen frame kind.
#[derive(Deserialize)]
struct ScreenAmendment {
    known_capability_mask: u16,
    capability_bits: ScreenCapabilityBits,
    offer_capability_bits: u16,
    offer_hex: String,
    prologue_hex: String,
    screen_frame_payload: String,
    screen_frame_hex: String,
    mutation_errors: ScreenMutationErrors,
}

#[derive(Deserialize)]
struct ScreenCapabilityBits {
    observe_screens: u8,
    control_pointer: u8,
    control_keyboard: u8,
}

#[derive(Deserialize)]
struct ScreenMutationErrors {
    capability_value_9: String,
    frame_kind_4: String,
    oversized_screen_frame: String,
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
    serde_json::from_str(include_str!("vectors/controller-v2.json"))
        .unwrap_or_else(|error| panic!("golden vector JSON failed: {error}"))
}

#[test]
fn exact_offer_handshake_sas_and_first_transport_frame_are_reproducible() {
    let expected = vector();
    assert_eq!(
        expected.noise_protocol,
        multiplex_controller_security::NOISE_PROTOCOL_NAME
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
            multiplex_controller_security::device_public_key_from_private(&common::device_static())
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
        let public = multiplex_controller_security::device_public_key_from_private(
            &multiplex_controller_security::StaticPrivateKey::from_fixture_bytes(private),
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
            b"controller-v2-first",
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
        multiplex_controller_security::MAX_SEQUENCE
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
    let mut salt_input = b"multiplex-controller-sas-v2\0".to_vec();
    salt_input.extend_from_slice(&nonce);
    let salt = Sha256::digest(&salt_input);
    assert_eq!(hex::encode(salt), anchor.salt_hex);

    let mut info = b"sas\0".to_vec();
    info.extend_from_slice(&2_u16.to_be_bytes());
    info.extend_from_slice(&0_u16.to_be_bytes());
    info.extend_from_slice(&host);
    info.extend_from_slice(&device);
    assert_eq!(hex::encode(&info), anchor.info_hex);
    let hkdf = Hkdf::<Sha256>::new(Some(&salt), &hash);
    let mut output = [0_u8; 5];
    hkdf.expand(&info, &mut output)
        .unwrap_or_else(|_| panic!("anchor HKDF expansion failed"));
    assert_eq!(hex::encode(output), anchor.hkdf_output_hex);
    let sas = multiplex_controller_security::derive_sas_v1(
        &multiplex_controller_security::PairingNonce(nonce),
        &multiplex_controller_security::HandshakeHash(hash),
        multiplex_controller_security::CONTROLLER_V2,
        multiplex_controller_security::HostStaticPublicKey(host),
        multiplex_controller_security::DeviceStaticPublicKey(device),
    )
    .unwrap_or_else(|error| panic!("anchor SAS failed: {error}"));
    assert_eq!(sas.as_str(), anchor.sas_display);
}

#[test]
fn amendment_one_locks_the_screen_capabilities_and_the_screen_frame() {
    let expected = vector().screen_amendment;
    assert_eq!(
        multiplex_controller_security::CapabilitySet::KNOWN_MASK,
        expected.known_capability_mask
    );
    for (capability, bit) in [
        (
            ControllerCapability::ObserveScreens,
            expected.capability_bits.observe_screens,
        ),
        (
            ControllerCapability::ControlPointer,
            expected.capability_bits.control_pointer,
        ),
        (
            ControllerCapability::ControlKeyboard,
            expected.capability_bits.control_keyboard,
        ),
    ] {
        assert_eq!(capability as u8, bit);
    }

    let offer = common::screen_offer();
    assert_eq!(offer.capabilities.bits(), expected.offer_capability_bits);
    assert_eq!(
        hex::encode(encode_offer(&offer).unwrap_or_else(|error| panic!("screen offer: {error}"))),
        expected.offer_hex
    );
    assert_eq!(
        hex::encode(
            pairing_prologue(&offer).unwrap_or_else(|error| panic!("screen prologue: {error}"))
        ),
        expected.prologue_hex
    );

    let payload = expected.screen_frame_payload.as_bytes();
    let (mut device, mut host) = confirmed_screen_pairing();
    let frame = device
        .transport
        .seal(
            ControllerFrameKind::Screen,
            ControllerCapability::ObserveScreens,
            RevocationEpoch(4),
            payload,
        )
        .unwrap_or_else(|error| panic!("screen seal: {error}"));
    assert_eq!(hex::encode(frame.as_bytes()), expected.screen_frame_hex);
    let opened = host
        .transport
        .open(frame.as_bytes())
        .unwrap_or_else(|error| panic!("screen open: {error}"));
    assert_eq!(opened.kind, ControllerFrameKind::Screen);
    assert_eq!(opened.capability, ControllerCapability::ObserveScreens);
    assert_eq!(opened.payload, payload);

    // The closed sets still fail on the first value each amendment did not define.
    for (offset, value, expected_error) in [
        (9, 9, &expected.mutation_errors.capability_value_9),
        (8, 4, &expected.mutation_errors.frame_kind_4),
    ] {
        let (mut device, mut host) = confirmed_screen_pairing();
        let mut bytes = device
            .transport
            .seal(
                ControllerFrameKind::Screen,
                ControllerCapability::ObserveScreens,
                RevocationEpoch(4),
                payload,
            )
            .unwrap_or_else(|error| panic!("screen seal: {error}"))
            .as_bytes()
            .to_vec();
        bytes[offset] = value;
        let error = host
            .transport
            .open(&bytes)
            .expect_err("a value outside the closed set must fail");
        assert_eq!(error.code().localization_id(), expected_error);
    }

    let (mut device, _) = confirmed_screen_pairing();
    let error = device
        .transport
        .seal(
            ControllerFrameKind::Screen,
            ControllerCapability::ObserveScreens,
            RevocationEpoch(4),
            &vec![0; multiplex_controller_security::MAX_SCREEN_FRAME_BYTES],
        )
        .expect_err("a screen frame past the limit must fail");
    assert_eq!(
        error.code().localization_id(),
        expected.mutation_errors.oversized_screen_frame
    );
}

/// A device and Host that completed the screen-capability pairing and confirmed at epoch 4.
fn confirmed_screen_pairing() -> (
    multiplex_controller_security::ConfirmedPairing,
    multiplex_controller_security::ConfirmedPairing,
) {
    let (device, host, _) = common::complete_handshake_for(common::screen_offer());
    let sas = device
        .sas()
        .cloned()
        .unwrap_or_else(|| panic!("SAS missing"));
    (
        device
            .confirm(&sas, RevocationEpoch(4))
            .unwrap_or_else(|error| panic!("device confirm: {error}")),
        host.confirm(&sas, RevocationEpoch(4))
            .unwrap_or_else(|error| panic!("host confirm: {error}")),
    )
}

fn array32(value: &str) -> [u8; 32] {
    hex::decode(value)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .unwrap_or_else(|| panic!("fixture field is not 32 bytes"))
}

/// The compatibility policy, demonstrated rather than asserted: the vectors this version
/// replaced are kept beside it, and every one of them is refused. A Controller-v1 offer is
/// rejected for its version before any key material is derived, and the prologue v1 bound is not
/// a prologue this version produces, so a v1 device cannot complete a handshake here.
#[test]
fn prior_vectors_are_refused() {
    #[derive(Deserialize)]
    struct PriorVector {
        protocol_version: String,
        offer_hex: String,
        prologue_hex: String,
    }

    let prior: PriorVector =
        serde_json::from_str(include_str!("vectors/legacy/controller-v1.json"))
            .unwrap_or_else(|error| panic!("prior vector JSON failed: {error}"));
    assert_eq!(prior.protocol_version, "1.0");

    let offer_bytes =
        hex::decode(&prior.offer_hex).unwrap_or_else(|error| panic!("prior offer hex: {error}"));
    assert_eq!(
        decode_offer(&offer_bytes).map_err(|error| error.code()),
        Err(multiplex_controller_security::ErrorCode::IncompatibleVersion)
    );

    let current =
        pairing_prologue(&common::offer()).unwrap_or_else(|error| panic!("prologue: {error}"));
    assert_ne!(hex::encode(current), prior.prologue_hex);
}
