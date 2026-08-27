#![no_main]

use libfuzzer_sys::fuzz_target;
use termirust_controller_security::{
    CapabilitySet, ControllerCapability, PairingMachine, PairingNonce, PairingOfferCore,
    RevocationEpoch, StaticPrivateKey, CONTROLLER_V1, decode_offer,
    host_public_key_from_private,
};

fuzz_target!(|data: &[u8]| {
    let _ = decode_offer(data);

    let host_static = StaticPrivateKey::from_fixture_bytes([1; 32]);
    let device_static = StaticPrivateKey::from_fixture_bytes([4; 32]);
    let offer = PairingOfferCore {
        version: CONTROLLER_V1,
        expires_at_unix_seconds: 300,
        nonce: PairingNonce([2; 32]),
        host_static_public_key: host_public_key_from_private(&host_static),
        capabilities: CapabilitySet::default().with(ControllerCapability::ObserveSessions),
    };
    let Ok(mut device) = PairingMachine::new_device_initiator(
        offer.clone(),
        device_static,
        StaticPrivateKey::from_fixture_bytes([5; 32]),
        0,
        0,
    ) else {
        return;
    };
    let Ok(mut host) = PairingMachine::new_host_responder(
        offer,
        host_static,
        StaticPrivateKey::from_fixture_bytes([3; 32]),
        0,
        0,
    ) else {
        return;
    };

    let Ok(message_1) = device.write_next(1) else {
        return;
    };
    if host.read_next(message_1.as_bytes(), 2).is_err() {
        return;
    }
    let Ok(message_2) = host.write_next(3) else {
        return;
    };
    if device.read_next(message_2.as_bytes(), 4).is_err() {
        return;
    }
    let Ok(message_3) = device.write_next(5) else {
        return;
    };
    if host.read_next(message_3.as_bytes(), 6).is_err() {
        return;
    }
    let Some(sas) = host.sas().cloned() else {
        return;
    };
    let Ok(mut confirmed) = host.confirm(&sas, RevocationEpoch(0)) else {
        return;
    };
    let _ = confirmed.transport.open(data);
});
