#![allow(dead_code)]

use termirust_controller_security::{
    CONTROLLER_V1, CapabilitySet, ControllerCapability, PairingMachine, PairingNonce,
    PairingOfferCore, RevocationEpoch, StaticPrivateKey, host_public_key_from_private,
};

const NOW_MILLIS: u64 = 10_000;
const NOW_SECONDS: u64 = 1_000;

fn bytes(start: u8) -> [u8; 32] {
    core::array::from_fn(|index| start.wrapping_add(index as u8))
}

pub fn confirmed_pair() -> (
    termirust_controller_security::ConfirmedPairing,
    termirust_controller_security::ConfirmedPairing,
) {
    let host_static = StaticPrivateKey::from_fixture_bytes(bytes(0x00));
    let offer = PairingOfferCore {
        version: CONTROLLER_V1,
        expires_at_unix_seconds: NOW_SECONDS + 300,
        nonce: PairingNonce(bytes(0x80)),
        host_static_public_key: host_public_key_from_private(&host_static),
        capabilities: CapabilitySet::default()
            .with(ControllerCapability::ObserveSessions)
            .with(ControllerCapability::AttachOutput)
            .with(ControllerCapability::SendInput),
    };
    let mut device = PairingMachine::new_device_initiator(
        offer.clone(),
        StaticPrivateKey::from_fixture_bytes(bytes(0x20)),
        StaticPrivateKey::from_fixture_bytes(bytes(0x60)),
        NOW_MILLIS,
        NOW_SECONDS,
    )
    .unwrap();
    let mut host = PairingMachine::new_host_responder(
        offer,
        host_static,
        StaticPrivateKey::from_fixture_bytes(bytes(0x40)),
        NOW_MILLIS,
        NOW_SECONDS,
    )
    .unwrap();
    let message_1 = device.write_next(NOW_MILLIS + 1).unwrap();
    host.read_next(message_1.as_bytes(), NOW_MILLIS + 2)
        .unwrap();
    let message_2 = host.write_next(NOW_MILLIS + 3).unwrap();
    device
        .read_next(message_2.as_bytes(), NOW_MILLIS + 4)
        .unwrap();
    let message_3 = device.write_next(NOW_MILLIS + 5).unwrap();
    host.read_next(message_3.as_bytes(), NOW_MILLIS + 6)
        .unwrap();
    let sas = device.sas().cloned().unwrap();
    (
        device.confirm(&sas, RevocationEpoch(4)).unwrap(),
        host.confirm(&sas, RevocationEpoch(4)).unwrap(),
    )
}
