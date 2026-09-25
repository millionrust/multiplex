//! Writes the golden vectors from the live construction, for the review the ADR's change-control
//! section requires when a deliberate protocol change lands.
//!
//! A conformance run never regenerates anything: `tests/golden_vectors.rs` only consumes the
//! stored bytes, and this file is compiled only under `cfg(test)` and only runs when asked for by
//! name. Regenerating is a reviewed act, so it is explicit:
//!
//! ```text
//! cargo test -p multiplex-controller-security --lib -- --ignored write_golden_vectors
//! ```
//!
//! The fixtures below are the ones `tests/common/mod.rs` uses. If they ever drift apart, the
//! conformance tests fail against the file this writes, which is the check that keeps them
//! honest.

use sha2::{Digest, Sha256};

use crate::{
    CapabilitySet, ControllerCapability, ControllerFrameKind, HandshakeHash, PairingMachine,
    PairingNonce, PairingOfferCore, RevocationEpoch, StaticPrivateKey,
    device_public_key_from_private, encode_offer, host_public_key_from_private, pairing_prologue,
    types::CONTROLLER_V2,
};

const NOW_MILLIS: u64 = 10_000;
const NOW_SECONDS: u64 = 1_000;

fn bytes(start: u8) -> [u8; 32] {
    core::array::from_fn(|index| start.wrapping_add(index as u8))
}

fn host_static() -> StaticPrivateKey {
    StaticPrivateKey::from_fixture_bytes(bytes(0x00))
}

fn device_static() -> StaticPrivateKey {
    StaticPrivateKey::from_fixture_bytes(bytes(0x20))
}

fn host_ephemeral() -> StaticPrivateKey {
    StaticPrivateKey::from_fixture_bytes(bytes(0x40))
}

fn device_ephemeral() -> StaticPrivateKey {
    StaticPrivateKey::from_fixture_bytes(bytes(0x60))
}

fn offer() -> PairingOfferCore {
    PairingOfferCore {
        version: CONTROLLER_V2,
        expires_at_unix_seconds: NOW_SECONDS + 300,
        nonce: PairingNonce(bytes(0x80)),
        host_static_public_key: host_public_key_from_private(&host_static()),
        capabilities: CapabilitySet::default()
            .with(ControllerCapability::ObserveSessions)
            .with(ControllerCapability::AttachOutput)
            .with(ControllerCapability::SendInput),
    }
}

fn screen_offer() -> PairingOfferCore {
    PairingOfferCore {
        capabilities: offer()
            .capabilities
            .with(ControllerCapability::ObserveScreens)
            .with(ControllerCapability::ControlPointer)
            .with(ControllerCapability::ControlKeyboard),
        ..offer()
    }
}

fn machines_for(offer: PairingOfferCore) -> (PairingMachine, PairingMachine) {
    let device = PairingMachine::new_device_initiator(
        offer.clone(),
        device_static(),
        device_ephemeral(),
        NOW_MILLIS,
        NOW_SECONDS,
    )
    .expect("the device fixture starts");
    let host = PairingMachine::new_host_responder(
        offer,
        host_static(),
        host_ephemeral(),
        NOW_MILLIS,
        NOW_SECONDS,
    )
    .expect("the host fixture starts");
    (device, host)
}

fn complete_handshake_for(
    offer: PairingOfferCore,
) -> (PairingMachine, PairingMachine, Vec<Vec<u8>>) {
    let (mut device, mut host) = machines_for(offer);
    let mut messages = Vec::new();
    let message_1 = device.write_next(NOW_MILLIS + 1).expect("message 1");
    host.read_next(message_1.as_bytes(), NOW_MILLIS + 2)
        .expect("message 1 is read");
    messages.push(message_1.as_bytes().to_vec());
    let message_2 = host.write_next(NOW_MILLIS + 3).expect("message 2");
    device
        .read_next(message_2.as_bytes(), NOW_MILLIS + 4)
        .expect("message 2 is read");
    messages.push(message_2.as_bytes().to_vec());
    let message_3 = device.write_next(NOW_MILLIS + 5).expect("message 3");
    host.read_next(message_3.as_bytes(), NOW_MILLIS + 6)
        .expect("message 3 is read");
    messages.push(message_3.as_bytes().to_vec());
    (device, host, messages)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The SAS anchor of the ADR: fixed inputs, and every step of the derivation spelled out so a
/// reviewer can follow it without running this code.
fn sas_anchor() -> String {
    use hkdf::Hkdf;

    let nonce = bytes(0x00);
    let hash = bytes(0x20);
    let host = bytes(0x40);
    let device = bytes(0x60);
    let mut salt_input = b"multiplex-controller-sas-v2\0".to_vec();
    salt_input.extend_from_slice(&nonce);
    let salt = Sha256::digest(&salt_input);
    let mut info = b"sas\0".to_vec();
    info.extend_from_slice(&CONTROLLER_V2.major.to_be_bytes());
    info.extend_from_slice(&CONTROLLER_V2.minor.to_be_bytes());
    info.extend_from_slice(&host);
    info.extend_from_slice(&device);
    let hkdf = Hkdf::<Sha256>::new(Some(&salt), &hash);
    let mut output = [0_u8; 5];
    hkdf.expand(&info, &mut output).expect("the anchor expands");
    let sas = crate::derive_sas_v1(
        &PairingNonce(nonce),
        &HandshakeHash(hash),
        CONTROLLER_V2,
        crate::HostStaticPublicKey(host),
        crate::DeviceStaticPublicKey(device),
    )
    .expect("the anchor derives a SAS");
    format!(
        "  \"normative_sas_anchor\": {{\n    \"pairing_nonce_hex\": \"{}\",\n    \"handshake_hash_hex\": \"{}\",\n    \"host_static_public_hex\": \"{}\",\n    \"device_static_public_hex\": \"{}\",\n    \"salt_hex\": \"{}\",\n    \"info_hex\": \"{}\",\n    \"hkdf_output_hex\": \"{}\",\n    \"sas_display\": \"{}\"\n  }}",
        hex(&nonce),
        hex(&hash),
        hex(&host),
        hex(&device),
        hex(&salt),
        hex(&info),
        hex(&output),
        sas.as_str()
    )
}

/// One sealed frame, from a transport starting at `sequence` with the keys this handshake split
/// into. The last frame of the vector is the one a channel would send at the highest legal
/// sequence, which no handshake reaches in a test.
fn frame_at(
    sequence: u64,
    keys: ([u8; 32], [u8; 32]),
    kind: ControllerFrameKind,
    capability: ControllerCapability,
    capabilities: CapabilitySet,
    payload: &[u8],
) -> Vec<u8> {
    let policy = crate::authorization::AuthorizationPolicy::new(capabilities, RevocationEpoch(4));
    let mut transport =
        crate::transport::ControllerTransport::from_test_keys(&keys.0, &keys.1, policy, sequence);
    transport
        .seal(kind, capability, RevocationEpoch(4), payload)
        .expect("the frame seals")
        .as_bytes()
        .to_vec()
}

#[test]
#[ignore = "writes the golden vectors; run deliberately, and review what it wrote"]
fn write_golden_vectors() {
    const FIRST_PAYLOAD: &[u8] = b"controller-v2-first";
    const SCREEN_PAYLOAD: &str = "controller-v2-screen";

    let offer = offer();
    let offer_bytes = encode_offer(&offer).expect("the offer encodes");
    let prologue = pairing_prologue(&offer).expect("the prologue builds");
    let (device, host, messages) = complete_handshake_for(offer.clone());
    let handshake_hash = device.handshake_hash().expect("a handshake hash").0;
    let sas = device.sas().expect("a SAS").clone();
    assert_eq!(host.sas(), device.sas(), "both sides read the same SAS");

    let keys = {
        let (device, _, _) = complete_handshake_for(offer.clone());
        device
            .split_keys_for_vectors()
            .expect("the handshake splits into transport keys")
    };

    let first_frame = frame_at(
        0,
        keys,
        ControllerFrameKind::Control,
        ControllerCapability::ObserveSessions,
        offer.capabilities,
        FIRST_PAYLOAD,
    );
    let last_frame = frame_at(
        crate::MAX_SEQUENCE,
        keys,
        ControllerFrameKind::Control,
        ControllerCapability::ObserveSessions,
        offer.capabilities,
        FIRST_PAYLOAD,
    );

    let screen_offer = screen_offer();
    let screen_offer_bytes = encode_offer(&screen_offer).expect("the screen offer encodes");
    let screen_prologue = pairing_prologue(&screen_offer).expect("the screen prologue builds");
    let screen_keys = {
        let (device, _, _) = complete_handshake_for(screen_offer.clone());
        device
            .split_keys_for_vectors()
            .expect("the screen handshake splits into transport keys")
    };
    let screen_frame = frame_at(
        0,
        screen_keys,
        ControllerFrameKind::Screen,
        ControllerCapability::ObserveScreens,
        screen_offer.capabilities,
        SCREEN_PAYLOAD.as_bytes(),
    );

    let adr = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/decisions/controller-security-v1.md"),
    )
    .expect("the ADR is readable");
    let lockfile =
        std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.lock"))
            .expect("the lockfile is readable");

    let document = format!(
        r#"{{
  "fixture_warning": "FIXTURE-ONLY PRIVATE KEYS; NEVER USE IN PRODUCTION",
  "protocol_version": "{major}.{minor}",
  "noise_protocol": "{noise}",
  "implementation": "clatter=2.2.0",
  "offer_hex": "{offer_hex}",
  "prologue_hex": "{prologue_hex}",
  "pairing_nonce_hex": "{nonce}",
  "expires_at_unix_seconds": {expiry},
  "capability_bits": {capabilities},
  "host_static_private_hex": "{host_private}",
  "host_static_public_hex": "{host_public}",
  "host_ephemeral_private_hex": "{host_ephemeral_private}",
  "host_ephemeral_public_hex": "{host_ephemeral_public}",
  "device_static_private_hex": "{device_private}",
  "device_static_public_hex": "{device_public}",
  "device_ephemeral_private_hex": "{device_ephemeral_private}",
  "device_ephemeral_public_hex": "{device_ephemeral_public}",
  "message_1_hex": "{message_1}",
  "message_2_hex": "{message_2}",
  "message_3_hex": "{message_3}",
  "handshake_hash_hex": "{handshake_hash}",
  "sas_display": "{sas}",
  "initiator_to_responder_key_hex": "{initiator_key}",
  "responder_to_initiator_key_hex": "{responder_key}",
  "first_sequence": 0,
  "first_frame_payload": "{first_payload}",
  "first_frame_hex": "{first_frame}",
  "last_sequence": {last_sequence},
  "last_frame_hex": "{last_frame}",
  "mutation_errors": {{
    "message_1_bound_field": "authentication_failed_or_bound_field_error",
    "message_2_bit": "authentication_failed",
    "message_3_bit": "authentication_failed",
    "replay": "wrong_state_or_duplicate_frame"
  }},
{anchor},
  "screen_amendment": {{
    "known_capability_mask": {known_mask},
    "capability_bits": {{
      "observe_screens": {observe_screens},
      "control_pointer": {control_pointer},
      "control_keyboard": {control_keyboard}
    }},
    "offer_capability_bits": {screen_capabilities},
    "offer_hex": "{screen_offer_hex}",
    "prologue_hex": "{screen_prologue_hex}",
    "screen_frame_payload": "{screen_payload}",
    "screen_frame_hex": "{screen_frame_hex}",
    "mutation_errors": {{
      "capability_value_9": "controller.security.unknown_capability",
      "frame_kind_4": "controller.security.invalid_encoding",
      "oversized_screen_frame": "controller.security.frame_too_large"
    }}
  }},
  "adr_sha256": "{adr_sha256}",
  "cargo_lock_sha256": "{lock_sha256}"
}}
"#,
        major = CONTROLLER_V2.major,
        minor = CONTROLLER_V2.minor,
        noise = crate::NOISE_PROTOCOL_NAME,
        offer_hex = hex(&offer_bytes),
        prologue_hex = hex(&prologue),
        nonce = hex(&offer.nonce.0),
        expiry = offer.expires_at_unix_seconds,
        capabilities = offer.capabilities.bits(),
        host_private = hex(&bytes(0x00)),
        host_public = hex(&offer.host_static_public_key.0),
        host_ephemeral_private = hex(&bytes(0x40)),
        host_ephemeral_public = hex(&host_public_key_from_private(&host_ephemeral()).0),
        device_private = hex(&bytes(0x20)),
        device_public = hex(&device_public_key_from_private(&device_static()).0),
        device_ephemeral_private = hex(&bytes(0x60)),
        device_ephemeral_public = hex(&device_public_key_from_private(&device_ephemeral()).0),
        message_1 = hex(&messages[0]),
        message_2 = hex(&messages[1]),
        message_3 = hex(&messages[2]),
        handshake_hash = hex(&handshake_hash),
        sas = sas.as_str(),
        initiator_key = hex(&keys.0),
        responder_key = hex(&keys.1),
        first_payload = String::from_utf8_lossy(FIRST_PAYLOAD),
        first_frame = hex(&first_frame),
        last_sequence = crate::MAX_SEQUENCE,
        last_frame = hex(&last_frame),
        anchor = sas_anchor(),
        known_mask = CapabilitySet::KNOWN_MASK,
        observe_screens = ControllerCapability::ObserveScreens as u8,
        control_pointer = ControllerCapability::ControlPointer as u8,
        control_keyboard = ControllerCapability::ControlKeyboard as u8,
        screen_capabilities = screen_offer.capabilities.bits(),
        screen_offer_hex = hex(&screen_offer_bytes),
        screen_prologue_hex = hex(&screen_prologue),
        screen_payload = SCREEN_PAYLOAD,
        screen_frame_hex = hex(&screen_frame),
        adr_sha256 = hex(&Sha256::digest(&adr)),
        lock_sha256 = hex(&Sha256::digest(&lockfile)),
    );

    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/controller-v2.json");
    std::fs::write(&path, document).expect("the regenerated vectors are written");
    println!("wrote {}", path.display());
}
