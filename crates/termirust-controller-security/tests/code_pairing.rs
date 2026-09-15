mod common;

use termirust_controller_security::{
    CodeBinding, CodeKeyExchange, ControllerCapability, ControllerFrameKind, ErrorCode,
    PairingCode, PairingMachine, PairingNonce, PairingRole, PairingState, RevocationEpoch,
};

fn bindings(device_code: &str, host_code: &str) -> (CodeBinding, CodeBinding) {
    let offer = common::offer();
    let device_nonce = PairingNonce(common::bytes(0xa0));
    let device = CodeKeyExchange::new(
        PairingRole::DeviceInitiator,
        &PairingCode::parse(device_code).unwrap(),
        &offer,
        &device_nonce,
        [0x11; 64],
    )
    .unwrap();
    let host = CodeKeyExchange::new(
        PairingRole::HostResponder,
        &PairingCode::parse(host_code).unwrap(),
        &offer,
        &device_nonce,
        [0x22; 64],
    )
    .unwrap();
    let (device_share, host_share) = (device.share(), host.share());
    (
        device.finish(&host_share).unwrap(),
        host.finish(&device_share).unwrap(),
    )
}

fn machines(device: &CodeBinding, host: &CodeBinding) -> (PairingMachine, PairingMachine) {
    (
        PairingMachine::new_device_initiator_with_code(
            common::offer(),
            device,
            common::device_static(),
            common::device_ephemeral(),
            common::NOW_MILLIS,
            common::NOW_SECONDS,
        )
        .unwrap(),
        PairingMachine::new_host_responder_with_code(
            common::offer(),
            host,
            common::host_static(),
            common::host_ephemeral(),
            common::NOW_MILLIS,
            common::NOW_SECONDS,
        )
        .unwrap(),
    )
}

/// Runs the three Noise messages, returning the first error with the step it happened at.
fn run(device: &mut PairingMachine, host: &mut PairingMachine) -> Result<(), (u8, ErrorCode)> {
    let now = common::NOW_MILLIS;
    let message_1 = device
        .write_next(now + 1)
        .map_err(|error| (1, error.code()))?;
    host.read_next(message_1.as_bytes(), now + 2)
        .map_err(|error| (1, error.code()))?;
    let message_2 = host
        .write_next(now + 3)
        .map_err(|error| (2, error.code()))?;
    device
        .read_next(message_2.as_bytes(), now + 4)
        .map_err(|error| (2, error.code()))?;
    let message_3 = device
        .write_next(now + 5)
        .map_err(|error| (3, error.code()))?;
    host.read_next(message_3.as_bytes(), now + 6)
        .map_err(|error| (3, error.code()))
}

#[test]
fn a_matching_code_pairs_both_keys_without_a_sas() {
    let (device_binding, host_binding) = bindings("305917", "305917");
    let (mut device, mut host) = machines(&device_binding, &host_binding);
    run(&mut device, &mut host).unwrap();
    assert_eq!(device.state(), PairingState::SasReady);
    assert_eq!(host.state(), PairingState::SasReady);

    let mut device = device
        .confirm_code_authenticated(RevocationEpoch(2))
        .unwrap();
    let mut host = host.confirm_code_authenticated(RevocationEpoch(2)).unwrap();
    assert_eq!(device.host_key, common::offer().host_static_public_key);
    assert_eq!(host.device_key, device.device_key);

    let sealed = device
        .transport
        .seal(
            ControllerFrameKind::Control,
            ControllerCapability::ObserveSessions,
            RevocationEpoch(2),
            b"registration",
        )
        .unwrap();
    assert_eq!(
        host.transport.open(sealed.as_bytes()).unwrap().payload,
        b"registration"
    );
}

#[test]
fn a_wrong_code_fails_at_the_host_proof_before_any_key_is_accepted() {
    let (device_binding, host_binding) = bindings("305917", "305918");
    let (mut device, mut host) = machines(&device_binding, &host_binding);
    assert_eq!(
        run(&mut device, &mut host),
        Err((2, ErrorCode::AuthenticationFailed))
    );
    assert_eq!(device.state(), PairingState::Failed);
    assert!(
        device
            .confirm_code_authenticated(RevocationEpoch(2))
            .is_err()
    );
}

#[test]
fn a_code_bound_handshake_does_not_interoperate_with_an_unbound_one() {
    let (device_binding, _) = bindings("305917", "305917");
    let mut device = PairingMachine::new_device_initiator_with_code(
        common::offer(),
        &device_binding,
        common::device_static(),
        common::device_ephemeral(),
        common::NOW_MILLIS,
        common::NOW_SECONDS,
    )
    .unwrap();
    let (_, mut host) = common::machines();
    assert_eq!(
        run(&mut device, &mut host),
        Err((2, ErrorCode::AuthenticationFailed))
    );
}

#[test]
fn only_a_code_bound_handshake_can_skip_the_sas() {
    let (device, _, _) = common::complete_handshake();
    assert_eq!(
        device
            .confirm_code_authenticated(RevocationEpoch(2))
            .map(|_| ())
            .unwrap_err()
            .code(),
        ErrorCode::WrongState
    );
}
