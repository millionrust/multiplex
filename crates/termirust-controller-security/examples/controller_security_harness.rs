use termirust_controller_security::{
    CONTROLLER_V1, CapabilitySet, ControllerCapability, ErrorCode, HANDSHAKE_TIMEOUT_MILLIS,
    PairingMachine, PairingNonce, PairingOfferCore, StaticPrivateKey, host_public_key_from_private,
};

const START_MILLIS: u64 = 10_000;
const NOW_SECONDS: u64 = 1_000;

fn main() {
    let scenario = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "exact".to_owned());
    let result = match scenario.as_str() {
        "exact" => exact(),
        "tamper" => tamper(),
        "replay" => replay(),
        "timeout" => timeout(),
        "cancel" => cancel(),
        _ => {
            eprintln!("usage: controller_security_harness [exact|tamper|replay|timeout|cancel]");
            std::process::exit(2);
        }
    };
    match result {
        Ok(message) => println!("controller-v1 scenario={scenario} {message}"),
        Err(code) => println!(
            "controller-v1 scenario={scenario} rejected={}",
            code.localization_id()
        ),
    }
}

fn exact() -> Result<&'static str, ErrorCode> {
    let (mut device, mut host) = machines()?;
    let message_1 = device.write_next(START_MILLIS + 1).map_err(|e| e.code())?;
    host.read_next(message_1.as_bytes(), START_MILLIS + 2)
        .map_err(|e| e.code())?;
    let message_2 = host.write_next(START_MILLIS + 3).map_err(|e| e.code())?;
    device
        .read_next(message_2.as_bytes(), START_MILLIS + 4)
        .map_err(|e| e.code())?;
    let message_3 = device.write_next(START_MILLIS + 5).map_err(|e| e.code())?;
    host.read_next(message_3.as_bytes(), START_MILLIS + 6)
        .map_err(|e| e.code())?;
    if device.sas() == host.sas() {
        Ok("handshake=ok sas=KR4A-QEYS")
    } else {
        Err(ErrorCode::SasMismatch)
    }
}

fn tamper() -> Result<&'static str, ErrorCode> {
    let (mut device, mut host) = machines()?;
    let message_1 = device.write_next(START_MILLIS + 1).map_err(|e| e.code())?;
    host.read_next(message_1.as_bytes(), START_MILLIS + 2)
        .map_err(|e| e.code())?;
    let message_2 = host.write_next(START_MILLIS + 3).map_err(|e| e.code())?;
    let mut tampered = message_2.as_bytes().to_vec();
    let last = tampered
        .len()
        .checked_sub(1)
        .ok_or(ErrorCode::InvalidEncoding)?;
    tampered[last] ^= 1;
    device
        .read_next(&tampered, START_MILLIS + 4)
        .map_err(|e| e.code())?;
    Err(ErrorCode::AuthenticationFailed)
}

fn replay() -> Result<&'static str, ErrorCode> {
    let (mut device, mut host) = machines()?;
    let message_1 = device.write_next(START_MILLIS + 1).map_err(|e| e.code())?;
    host.read_next(message_1.as_bytes(), START_MILLIS + 2)
        .map_err(|e| e.code())?;
    host.read_next(message_1.as_bytes(), START_MILLIS + 3)
        .map_err(|e| e.code())?;
    Err(ErrorCode::DuplicateFrame)
}

fn timeout() -> Result<&'static str, ErrorCode> {
    let (mut device, _) = machines()?;
    device
        .write_next(START_MILLIS + HANDSHAKE_TIMEOUT_MILLIS + 1)
        .map_err(|e| e.code())?;
    Err(ErrorCode::TimedOut)
}

fn cancel() -> Result<&'static str, ErrorCode> {
    let (mut device, _) = machines()?;
    Err(device.cancel().code())
}

fn machines() -> Result<(PairingMachine, PairingMachine), ErrorCode> {
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
    let device = PairingMachine::new_device_initiator(
        offer.clone(),
        StaticPrivateKey::from_fixture_bytes(bytes(0x20)),
        StaticPrivateKey::from_fixture_bytes(bytes(0x60)),
        START_MILLIS,
        NOW_SECONDS,
    )
    .map_err(|error| error.code())?;
    let host = PairingMachine::new_host_responder(
        offer,
        host_static,
        StaticPrivateKey::from_fixture_bytes(bytes(0x40)),
        START_MILLIS,
        NOW_SECONDS,
    )
    .map_err(|error| error.code())?;
    Ok((device, host))
}

fn bytes(start: u8) -> [u8; 32] {
    core::array::from_fn(|index| start.wrapping_add(index as u8))
}
