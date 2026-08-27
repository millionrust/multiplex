#![allow(dead_code)] // Each integration-test crate compiles only the helpers it exercises.

use termirust_controller_security::{
    CONTROLLER_V1, CapabilitySet, ControllerCapability, PairingMachine, PairingNonce,
    PairingOfferCore, StaticPrivateKey, host_public_key_from_private,
};

pub const NOW_MILLIS: u64 = 10_000;
pub const NOW_SECONDS: u64 = 1_000;

pub fn bytes(start: u8) -> [u8; 32] {
    core::array::from_fn(|index| start.wrapping_add(index as u8))
}

pub fn host_static() -> StaticPrivateKey {
    StaticPrivateKey::from_fixture_bytes(bytes(0x00))
}

pub fn device_static() -> StaticPrivateKey {
    StaticPrivateKey::from_fixture_bytes(bytes(0x20))
}

pub fn host_ephemeral() -> StaticPrivateKey {
    StaticPrivateKey::from_fixture_bytes(bytes(0x40))
}

pub fn device_ephemeral() -> StaticPrivateKey {
    StaticPrivateKey::from_fixture_bytes(bytes(0x60))
}

pub fn offer() -> PairingOfferCore {
    PairingOfferCore {
        version: CONTROLLER_V1,
        expires_at_unix_seconds: NOW_SECONDS + 300,
        nonce: PairingNonce(bytes(0x80)),
        host_static_public_key: host_public_key_from_private(&host_static()),
        capabilities: CapabilitySet::default()
            .with(ControllerCapability::ObserveSessions)
            .with(ControllerCapability::AttachOutput)
            .with(ControllerCapability::SendInput),
    }
}

pub fn machines() -> (PairingMachine, PairingMachine) {
    let offer = offer();
    let device = PairingMachine::new_device_initiator(
        offer.clone(),
        device_static(),
        device_ephemeral(),
        NOW_MILLIS,
        NOW_SECONDS,
    )
    .unwrap_or_else(|error| panic!("device fixture failed: {error}"));
    let host = PairingMachine::new_host_responder(
        offer,
        host_static(),
        host_ephemeral(),
        NOW_MILLIS,
        NOW_SECONDS,
    )
    .unwrap_or_else(|error| panic!("host fixture failed: {error}"));
    (device, host)
}

pub fn complete_handshake() -> (PairingMachine, PairingMachine, Vec<Vec<u8>>) {
    let (mut device, mut host) = machines();
    let mut messages = Vec::new();

    let message_1 = device
        .write_next(NOW_MILLIS + 1)
        .unwrap_or_else(|error| panic!("message 1 write failed: {error}"));
    host.read_next(message_1.as_bytes(), NOW_MILLIS + 2)
        .unwrap_or_else(|error| panic!("message 1 read failed: {error}"));
    messages.push(message_1.as_bytes().to_vec());

    let message_2 = host
        .write_next(NOW_MILLIS + 3)
        .unwrap_or_else(|error| panic!("message 2 write failed: {error}"));
    device
        .read_next(message_2.as_bytes(), NOW_MILLIS + 4)
        .unwrap_or_else(|error| panic!("message 2 read failed: {error}"));
    messages.push(message_2.as_bytes().to_vec());

    let message_3 = device
        .write_next(NOW_MILLIS + 5)
        .unwrap_or_else(|error| panic!("message 3 write failed: {error}"));
    host.read_next(message_3.as_bytes(), NOW_MILLIS + 6)
        .unwrap_or_else(|error| panic!("message 3 read failed: {error}"));
    messages.push(message_3.as_bytes().to_vec());

    (device, host, messages)
}
