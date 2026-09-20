//! A paired device, from the command line, for checking a running TermiRust against a real Mac.
//!
//! The mobile fixture is the computer; this is the phone. It pairs with the six-digit code
//! Devices shows, then watches this computer's screen and drives it. It answers questions the
//! automated tests cannot, such as which process macOS holds responsible for a Screen Recording
//! request, and it needs a person to grant the device screen access in Devices between pairing
//! and watching.
//!
//! ```text
//! cargo run -p multiplex-controller-listener --example controller_device_probe -- \
//!     pair --address 192.168.88.4:63322 --code 123456 --state /tmp/probe.json
//! cargo run -p multiplex-controller-listener --example controller_device_probe -- \
//!     watch --address 192.168.88.4:63322 --state /tmp/probe.json --seconds 15
//! ```
//!
//! `connect` is the same without a screen session, for checking the connection alone.

use std::time::Duration;

use multiplex_controller_listener::{
    ControllerClientChannel, ControllerCommand, ControllerConnectionPurpose, ControllerIncoming,
    ControllerResponse, ScreenFrameCapability, SystemHandshakeEntropy,
    pair_controller_with_code_client,
};
use multiplex_controller_security::{
    CapabilitySet, HostStaticPublicKey, PairingCode, StaticPrivateKey,
};
use multiplex_domain::ControllerDeviceId;
use multiplex_screen_protocol::{ControlHolder, FrameReader, InputKind, Profile, encode_frame};
use multiplex_screen_session::{InputEvent, ViewerEvent, ViewerSession};
use serde::{Deserialize, Serialize};

/// What pairing produced, so a later run can connect without pairing again.
#[derive(Serialize, Deserialize)]
struct ProbeState {
    device_private_hex: String,
    host_public_hex: String,
    identity_generation: u64,
    revocation_epoch: u64,
    session_generation: u64,
    capability_bits: u16,
}

fn main() {
    if let Err(message) = run() {
        eprintln!("probe failed: {message}");
        std::process::exit(1);
    }
}

#[tokio::main(flavor = "current_thread")]
async fn run() -> Result<(), String> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let mode = arguments.first().map(String::as_str).unwrap_or("");
    let address = value(&arguments, "--address").ok_or("--address is required")?;
    let state_path = value(&arguments, "--state").ok_or("--state is required")?;

    match mode {
        "pair" => {
            let code = value(&arguments, "--code").ok_or("--code is required")?;
            pair(&address, &code, &state_path).await
        }
        "watch" => {
            let seconds = value(&arguments, "--seconds")
                .and_then(|value| value.parse().ok())
                .unwrap_or(20);
            watch(&address, &state_path, seconds).await
        }
        "connect" => {
            let seconds = value(&arguments, "--seconds")
                .and_then(|value| value.parse().ok())
                .unwrap_or(20);
            connect(&address, &state_path, seconds).await
        }
        _ => Err(
            "usage: pair|watch|connect --address HOST:PORT --state FILE [--code NNNNNN]".to_owned(),
        ),
    }
}

async fn pair(address: &str, code: &str, state_path: &str) -> Result<(), String> {
    let mut stream = tokio::net::TcpStream::connect(address)
        .await
        .map_err(|error| format!("connect: {error}"))?;
    ControllerConnectionPurpose::PairCode
        .write_to(&mut stream)
        .await
        .map_err(|error| format!("preface: {:?}", error.code))?;
    let code = PairingCode::parse(code).map_err(|_| "the code must be six digits")?;
    let device_seed = random_bytes();
    let device_private = StaticPrivateKey::from_bytes(device_seed);
    let mut entropy = SystemHandshakeEntropy;
    let result = pair_controller_with_code_client(
        &mut stream,
        &code,
        device_private,
        StaticPrivateKey::from_bytes(random_bytes()),
        &mut entropy,
        ControllerDeviceId::new(),
        "Probe device".to_owned(),
        |_| Ok(()),
    )
    .await
    .map_err(|error| format!("pairing: {:?}", error.code))?;

    let state = ProbeState {
        device_private_hex: hex(&device_seed),
        host_public_hex: hex(&result.host_public_key.0),
        identity_generation: result.identity_generation,
        revocation_epoch: result.revocation_epoch,
        session_generation: result.session_generation,
        capability_bits: result.capability_bits,
    };
    std::fs::write(
        state_path,
        serde_json::to_vec_pretty(&state).map_err(|_| "state")?,
    )
    .map_err(|error| format!("state: {error}"))?;
    println!(
        "paired: capabilities={:#06x} identity_generation={} session_generation={}",
        state.capability_bits, state.identity_generation, state.session_generation
    );
    Ok(())
}

async fn connect(address: &str, state_path: &str, seconds: u64) -> Result<(), String> {
    let channel = open_channel(address, state_path).await?;
    println!(
        "connected: granted={:#06x}; holding for {seconds}s",
        channel.granted_capabilities().bits()
    );
    tokio::time::sleep(Duration::from_secs(seconds)).await;
    drop(channel);
    println!("closed");
    Ok(())
}

/// Watches this computer's first display, then asks for control and points and types at it.
async fn watch(address: &str, state_path: &str, seconds: u64) -> Result<(), String> {
    let mut channel = open_channel(address, state_path).await?;
    let granted = channel.granted_capabilities().bits();
    println!("connected: granted={granted:#06x}");

    channel
        .send(ControllerCommand::OpenScreen, deadline())
        .await
        .map_err(|error| format!("open_screen: {:?}", error.code))?;
    let ControllerResponse::ScreenOpened { ticket, .. } = channel
        .read_response()
        .await
        .map_err(|error| format!("open_screen: {:?}", error.code))?
    else {
        return Err("the computer did not open a screen session".to_owned());
    };
    println!("screen session opened");

    let mut session = ViewerSession::new(8 * 1024 * 1024);
    session.connect(
        ticket
            .as_slice()
            .try_into()
            .map_err(|_| "a ticket is 32 bytes")?,
    );
    let mut reader = FrameReader::new();
    let mut surface = None;
    let mut batches = 0usize;
    let mut asked_for_control = false;
    let mut drove = false;
    let deadline_at = tokio::time::Instant::now() + Duration::from_secs(seconds);

    loop {
        while let Some(message) = session.poll_outgoing() {
            let capability = match message.input_kind() {
                None => ScreenFrameCapability::Observe,
                Some(InputKind::Pointer) => ScreenFrameCapability::Pointer,
                Some(InputKind::Keyboard) => ScreenFrameCapability::Keyboard,
            };
            let bytes = encode_frame(&message).map_err(|_| "encoding a screen message")?;
            channel
                .send_screen(capability, &bytes)
                .await
                .map_err(|error| format!("sending: {:?}", error.code))?;
        }
        if tokio::time::Instant::now() >= deadline_at {
            break;
        }
        let incoming =
            match tokio::time::timeout(Duration::from_secs(5), channel.read_incoming()).await {
                Err(_) => break,
                Ok(incoming) => incoming.map_err(|error| format!("reading: {:?}", error.code))?,
            };
        let ControllerIncoming::Screen(bytes) = incoming else {
            continue;
        };
        reader.push(&bytes);
        while let Some(message) = reader
            .next_message()
            .map_err(|_| "the computer sent a malformed screen message")?
        {
            let is_batch = message.is_batch();
            for event in session
                .receive(message)
                .map_err(|error| format!("screen session: {error:?}"))?
            {
                match event {
                    ViewerEvent::Welcomed { surfaces, .. } => {
                        let first = surfaces.first().ok_or("this computer shares no display")?;
                        println!(
                            "welcomed: {} display(s); first is {} at {}x{}",
                            surfaces.len(),
                            first.name,
                            first.size.width(),
                            first.size.height()
                        );
                        surface = Some(first.id);
                        session.subscribe(first.id, Profile::Interactive);
                    }
                    ViewerEvent::Control(holder) => {
                        println!("control: {holder:?}");
                        if holder == ControlHolder::You
                            && !drove
                            && let Some(id) = surface
                        {
                            // A move well away from anything destructive, then one keystroke
                            // the person can see in a text field if they have one focused.
                            session.send_input(InputEvent::PointerMove {
                                surface: id,
                                x: 40,
                                y: 40,
                            });
                            session.send_input(InputEvent::Text {
                                surface: id,
                                text: "probe".to_owned(),
                            });
                            drove = true;
                            println!("sent a pointer move and a keystroke");
                        }
                    }
                    ViewerEvent::Closed { reason } => {
                        return Err(format!("the computer closed the session: {reason}"));
                    }
                    _ => {}
                }
            }
            if is_batch {
                batches += 1;
                if batches == 2 && !asked_for_control {
                    asked_for_control = true;
                    session.request_control();
                    println!("asked for control");
                }
            }
        }
    }

    let pixels = surface
        .and_then(|id| session.framebuffer(id))
        .map(|frame| (frame.size().width(), frame.size().height()));
    println!(
        "watched: {batches} batch(es); picture {:?}; drove={drove}",
        pixels
    );
    Ok(())
}

fn deadline() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or_default()
        .saturating_add(10_000)
}

async fn open_channel(
    address: &str,
    state_path: &str,
) -> Result<ControllerClientChannel<tokio::net::TcpStream>, String> {
    let state: ProbeState = serde_json::from_slice(
        &std::fs::read(state_path).map_err(|error| format!("state: {error}"))?,
    )
    .map_err(|_| "state")?;
    // `ControllerClientChannel::connect` writes the Authenticate purpose itself.
    let stream = tokio::net::TcpStream::connect(address)
        .await
        .map_err(|error| format!("connect: {error}"))?;
    let mut entropy = SystemHandshakeEntropy;
    ControllerClientChannel::connect(
        stream,
        state.identity_generation,
        state.revocation_epoch,
        state.session_generation,
        HostStaticPublicKey(unhex(&state.host_public_hex)?),
        StaticPrivateKey::from_bytes(unhex(&state.device_private_hex)?),
        // Ask for everything this build understands, so a capability granted since pairing
        // arrives without pairing again.
        CapabilitySet::from_bits(CapabilitySet::KNOWN_MASK).map_err(|_| "capabilities")?,
        &mut entropy,
    )
    .await
    .map_err(|error| format!("authenticating: {:?}", error.code))
}

fn value(arguments: &[String], name: &str) -> Option<String> {
    arguments
        .iter()
        .position(|argument| argument == name)
        .and_then(|index| arguments.get(index + 1))
        .cloned()
}

fn random_bytes() -> [u8; 32] {
    use rand::RngCore as _;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes
}

fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(value: &str) -> Result<[u8; 32], String> {
    if value.len() != 64 {
        return Err("a key must be 32 bytes".to_owned());
    }
    let mut bytes = [0u8; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| "a key must be hex".to_owned())?;
    }
    Ok(bytes)
}
