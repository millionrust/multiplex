mod common;

use termirust_controller_security::{
    ControllerCapability, ControllerFrameKind, ErrorCode, HANDSHAKE_TIMEOUT_MILLIS, PairingMachine,
    PairingState, RevocationEpoch,
};

#[test]
fn both_roles_reach_the_same_sas_and_exchange_scoped_frames() {
    let (device, host, _) = common::complete_handshake();
    assert_eq!(device.state(), PairingState::SasReady);
    assert_eq!(host.state(), PairingState::SasReady);
    assert_eq!(device.sas(), host.sas());
    assert_eq!(device.handshake_hash(), host.handshake_hash());

    let sas = device
        .sas()
        .cloned()
        .unwrap_or_else(|| panic!("device SAS missing"));
    let mut device = device
        .confirm(&sas, RevocationEpoch(4))
        .unwrap_or_else(|error| panic!("device confirmation failed: {error}"));
    let mut host = host
        .confirm(&sas, RevocationEpoch(4))
        .unwrap_or_else(|error| panic!("host confirmation failed: {error}"));
    let sealed = device
        .transport
        .seal(
            ControllerFrameKind::Control,
            ControllerCapability::SendInput,
            RevocationEpoch(4),
            b"fixture input",
        )
        .unwrap_or_else(|error| panic!("frame seal failed: {error}"));
    let opened = host
        .transport
        .open(sealed.as_bytes())
        .unwrap_or_else(|error| panic!("frame open failed: {error}"));
    assert_eq!(opened.payload, b"fixture input");
}

#[test]
fn duplicate_out_of_order_timeout_cancel_reject_and_sas_mismatch_fail_closed() {
    let (_, mut host) = common::machines();
    assert_eq!(
        host.write_next(common::NOW_MILLIS)
            .map_err(|error| error.code()),
        Err(ErrorCode::WrongState)
    );

    let (mut device, mut host) = common::machines();
    let first = device
        .write_next(common::NOW_MILLIS + 1)
        .unwrap_or_else(|error| panic!("message 1 failed: {error}"));
    host.read_next(first.as_bytes(), common::NOW_MILLIS + 2)
        .unwrap_or_else(|error| panic!("message 1 read failed: {error}"));
    assert_eq!(
        host.read_next(first.as_bytes(), common::NOW_MILLIS + 3)
            .map_err(|error| error.code()),
        Err(ErrorCode::WrongState)
    );

    let (mut device, _) = common::machines();
    assert_eq!(
        device
            .write_next(common::NOW_MILLIS + HANDSHAKE_TIMEOUT_MILLIS + 1)
            .map_err(|error| error.code()),
        Err(ErrorCode::TimedOut)
    );
    assert_eq!(device.state(), PairingState::Expired);

    let (mut device, _) = common::machines();
    assert_eq!(device.cancel().code(), ErrorCode::Cancelled);
    assert_eq!(device.state(), PairingState::Rejected);
    assert!(device.sas().is_none());

    let (device, _, _) = common::complete_handshake();
    assert_eq!(device.reject().code(), ErrorCode::Rejected);

    let (device, _, _) = common::complete_handshake();
    let (_, other_host, _) = common::complete_handshake();
    let mut wrong = other_host
        .sas()
        .cloned()
        .unwrap_or_else(|| panic!("fixture SAS missing"));
    if wrong == *device.sas().unwrap_or_else(|| panic!("device SAS missing")) {
        let different_offer = termirust_controller_security::derive_sas_v1(
            &termirust_controller_security::PairingNonce([1; 32]),
            &termirust_controller_security::HandshakeHash([2; 32]),
            termirust_controller_security::CONTROLLER_V1,
            termirust_controller_security::HostStaticPublicKey([3; 32]),
            termirust_controller_security::DeviceStaticPublicKey([4; 32]),
        )
        .unwrap_or_else(|error| panic!("different SAS failed: {error}"));
        wrong = different_offer;
    }
    let error = device.confirm(&wrong, RevocationEpoch(4)).unwrap_err();
    assert_eq!(error.code(), ErrorCode::SasMismatch);
}

#[test]
fn cancellation_clears_every_live_pairing_state() {
    let (mut created, _) = common::machines();
    assert_eq!(created.cancel().code(), ErrorCode::Cancelled);
    assert_eq!(created.state(), PairingState::Rejected);
    assert!(created.sas().is_none());
    assert!(created.handshake_hash().is_none());

    let (mut device, mut host) = common::machines();
    let message_1 = device
        .write_next(common::NOW_MILLIS + 1)
        .unwrap_or_else(|error| panic!("message 1 failed: {error}"));
    host.read_next(message_1.as_bytes(), common::NOW_MILLIS + 2)
        .unwrap_or_else(|error| panic!("message 1 read failed: {error}"));
    assert_eq!(host.cancel().code(), ErrorCode::Cancelled);
    assert_eq!(host.state(), PairingState::Rejected);

    let message_2 = {
        let (_, mut fresh_host) = common::machines();
        fresh_host
            .read_next(message_1.as_bytes(), common::NOW_MILLIS + 2)
            .unwrap_or_else(|error| panic!("fresh message 1 read failed: {error}"));
        fresh_host
            .write_next(common::NOW_MILLIS + 3)
            .unwrap_or_else(|error| panic!("message 2 failed: {error}"))
    };
    device
        .read_next(message_2.as_bytes(), common::NOW_MILLIS + 4)
        .unwrap_or_else(|error| panic!("message 2 read failed: {error}"));
    assert_eq!(device.cancel().code(), ErrorCode::Cancelled);
    assert_eq!(device.state(), PairingState::Rejected);

    let (mut sas_ready, _, _) = common::complete_handshake();
    assert_eq!(sas_ready.cancel().code(), ErrorCode::Cancelled);
    assert_eq!(sas_ready.state(), PairingState::Rejected);
    assert!(sas_ready.sas().is_none());
    assert!(sas_ready.handshake_hash().is_none());
}

#[test]
fn expired_offer_and_wrong_host_key_are_rejected_before_handshake() {
    let mut expired = common::offer();
    expired.expires_at_unix_seconds = common::NOW_SECONDS - 1;
    let error = PairingMachine::new_device_initiator(
        expired,
        common::device_static(),
        common::device_ephemeral(),
        common::NOW_MILLIS,
        common::NOW_SECONDS,
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::Expired);

    let mut wrong_key = common::offer();
    wrong_key.host_static_public_key.0[0] ^= 1;
    let error = PairingMachine::new_host_responder(
        wrong_key,
        common::host_static(),
        common::host_ephemeral(),
        common::NOW_MILLIS,
        common::NOW_SECONDS,
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::WrongKey);
}

#[test]
fn offer_lifetime_and_monotonic_deadline_boundaries_are_exact() {
    let exact = common::offer();
    PairingMachine::new_device_initiator(
        exact,
        common::device_static(),
        common::device_ephemeral(),
        common::NOW_MILLIS,
        common::NOW_SECONDS,
    )
    .unwrap_or_else(|error| panic!("300-second offer was rejected: {error}"));

    let mut too_long = common::offer();
    too_long.expires_at_unix_seconds += 1;
    let error = PairingMachine::new_device_initiator(
        too_long,
        common::device_static(),
        common::device_ephemeral(),
        common::NOW_MILLIS,
        common::NOW_SECONDS,
    )
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::InvalidEncoding);

    let mut short_offer = common::offer();
    short_offer.expires_at_unix_seconds = common::NOW_SECONDS + 2;
    let mut at_deadline = PairingMachine::new_device_initiator(
        short_offer.clone(),
        common::device_static(),
        common::device_ephemeral(),
        common::NOW_MILLIS,
        common::NOW_SECONDS,
    )
    .unwrap_or_else(|error| panic!("short offer setup failed: {error}"));
    assert!(at_deadline.write_next(common::NOW_MILLIS + 2_000).is_ok());

    let mut after_deadline = PairingMachine::new_device_initiator(
        short_offer,
        common::device_static(),
        common::device_ephemeral(),
        common::NOW_MILLIS,
        common::NOW_SECONDS,
    )
    .unwrap_or_else(|error| panic!("short offer setup failed: {error}"));
    assert_eq!(
        after_deadline
            .write_next(common::NOW_MILLIS + 2_001)
            .map_err(|error| error.code()),
        Err(ErrorCode::TimedOut)
    );
    assert_eq!(after_deadline.state(), PairingState::Expired);
}

#[test]
fn every_single_bit_handshake_message_mutation_fails_before_confirmation() {
    let (_, _, messages) = common::complete_handshake();
    for (message_index, original) in messages.iter().enumerate() {
        for bit in 0..(original.len() * 8) {
            let mut mutation = original.clone();
            mutation[bit / 8] ^= 1 << (bit % 8);
            assert!(
                mutated_handshake_fails(message_index, &mutation),
                "message={}, bit={bit}",
                message_index + 1
            );
        }
    }
}

fn mutated_handshake_fails(message_index: usize, mutation: &[u8]) -> bool {
    let (mut device, mut host) = common::machines();
    let message_1 = match device.write_next(common::NOW_MILLIS + 1) {
        Ok(message) => message,
        Err(_) => return true,
    };
    let incoming_1 = if message_index == 0 {
        mutation
    } else {
        message_1.as_bytes()
    };
    if host.read_next(incoming_1, common::NOW_MILLIS + 2).is_err() {
        return true;
    }
    let message_2 = match host.write_next(common::NOW_MILLIS + 3) {
        Ok(message) => message,
        Err(_) => return true,
    };
    let incoming_2 = if message_index == 1 {
        mutation
    } else {
        message_2.as_bytes()
    };
    if device
        .read_next(incoming_2, common::NOW_MILLIS + 4)
        .is_err()
    {
        return true;
    }
    let message_3 = match device.write_next(common::NOW_MILLIS + 5) {
        Ok(message) => message,
        Err(_) => return true,
    };
    let incoming_3 = if message_index == 2 {
        mutation
    } else {
        message_3.as_bytes()
    };
    if host.read_next(incoming_3, common::NOW_MILLIS + 6).is_err() {
        return true;
    }
    device.sas() != host.sas()
}
