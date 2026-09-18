//! A viewer watching and driving a computer over a real Controller connection.

use std::sync::{Arc, Mutex};

use termirust_controller_listener::{
    AuthoritySnapshot, ControllerAuthorityProvider, ControllerBackendFactory,
    ControllerClientChannel, ControllerCommand, ControllerConnectionBackend, ControllerIncoming,
    ControllerResponse, ControllerScreenSession, HostCommandContext, ListenerError,
    ScreenFrameCapability, ScreenOutgoing, SystemHandshakeEntropy,
    serve_authenticated_stdio_stream,
};
use termirust_controller_security::{
    CapabilitySet, ControllerCapability as SecurityCapability, HostStaticPublicKey,
    StaticPrivateKey, device_public_key_from_private, host_public_key_from_private,
};
use termirust_domain::{
    AuthenticatedPeer, ControllerCapabilities, ControllerCapability as DomainCapability,
    ControllerDeviceAuthority, ControllerDeviceId, ControllerProtocolRange, DevicePublicKey,
    HostIdentityGeneration, HostIdentityPublic, HostIdentitySecretRef, HostIdentityState,
    HostPublicKey, PairedDeviceRecord, PairedDeviceStatus, PairingAttemptLedger, PairingOfferId,
};
use termirust_screen_codec::{FrameBuffer, Rect, Size};
use termirust_screen_host::{ScreenHost, ScreenHostEvent, ScreenHostHandle};
use termirust_screen_protocol::{
    ControlHolder, FrameReader, KeyEvent, Modifiers, PointerButton, Profile, SurfaceInfo,
    encode_frame,
};
use termirust_screen_session::{HostConfig, InputEvent, ViewerSession};
use tokio_util::sync::CancellationToken;

fn size() -> Size {
    Size::new(640, 400).unwrap()
}

fn surfaces() -> Vec<SurfaceInfo> {
    vec![SurfaceInfo {
        id: 1,
        size: size(),
        scale_milli: 2000,
        name: "Built-in Display".to_owned(),
    }]
}

/// A desktop with a window whose content grows with `step`.
fn desktop(step: u32) -> FrameBuffer {
    let mut buffer = FrameBuffer::new(size());
    buffer
        .fill_rect(size().bounds(), [240, 236, 232, 255])
        .unwrap();
    buffer
        .fill_rect(Rect::new(80, 60, 460, 280), [35, 30, 28, 255])
        .unwrap();
    for row in 0..12 {
        let length = (row * 31 + step * 17) % 400 + 20;
        buffer
            .fill_rect(
                Rect::new(96, 76 + row * 20, length, 10),
                [212, 205, 201, 255],
            )
            .unwrap();
    }
    buffer
}

struct Authority {
    value: Mutex<ControllerDeviceAuthority>,
    host_private: StaticPrivateKey,
}

impl ControllerAuthorityProvider for Authority {
    fn snapshot(&self) -> Result<AuthoritySnapshot, ListenerError> {
        Ok(AuthoritySnapshot {
            authority: self.value.lock().unwrap().clone(),
            host_private: self.host_private.clone(),
        })
    }
}

fn authority(host: &StaticPrivateKey, device: &StaticPrivateKey) -> ControllerDeviceAuthority {
    ControllerDeviceAuthority {
        identity: Some(HostIdentityPublic::new(
            HostIdentityGeneration::INITIAL,
            HostPublicKey(host_public_key_from_private(host).0),
        )),
        secret_ref: Some(HostIdentitySecretRef::new("identity:screens").unwrap()),
        state: HostIdentityState::Ready,
        revocation_epoch: 2,
        session_generation: 9,
        devices: vec![PairedDeviceRecord {
            device_id: ControllerDeviceId::new(),
            public_key: DevicePublicKey(device_public_key_from_private(device).0),
            display_name: "Laptop".to_owned(),
            capabilities: ControllerCapabilities::default()
                .with(DomainCapability::ObserveScreens)
                .with(DomainCapability::ControlPointer),
            protocol_range: ControllerProtocolRange::V1,
            created_at: 1,
            last_seen_at: None,
            revocation_epoch: 2,
            identity_generation: HostIdentityGeneration::INITIAL,
            status: PairedDeviceStatus::Online,
            source_offer_id: PairingOfferId::new(),
        }],
        offers: Vec::new(),
        attempts: PairingAttemptLedger::default(),
    }
}

/// Serves no terminal sessions, only screens.
#[derive(Default)]
struct Screens {
    handle: Arc<Mutex<Option<ScreenHostHandle>>>,
    events: Arc<Mutex<Vec<ScreenHostEvent>>>,
}

struct NoTerminals;

#[async_trait::async_trait]
impl ControllerConnectionBackend for NoTerminals {
    async fn command_context(
        &mut self,
        _: &termirust_controller_listener::ControllerCommandEnvelope,
        _: &CancellationToken,
    ) -> Result<HostCommandContext, ListenerError> {
        Ok(HostCommandContext::default())
    }

    async fn execute(
        &mut self,
        command: termirust_controller_listener::ControllerCommandEnvelope,
        _: &CancellationToken,
    ) -> Result<Vec<ControllerResponse>, ListenerError> {
        Ok(vec![ControllerResponse::Error {
            command_id: command.command_id,
            code: "no_terminals".to_owned(),
            completion_unknown: false,
        }])
    }
}

impl ControllerBackendFactory for Screens {
    fn open(
        &self,
        _: &AuthenticatedPeer,
    ) -> Result<Box<dyn ControllerConnectionBackend>, ListenerError> {
        Ok(Box::new(NoTerminals))
    }

    fn open_screens(
        &self,
        _: &AuthenticatedPeer,
        outgoing: ScreenOutgoing,
    ) -> Option<Box<dyn ControllerScreenSession>> {
        let events = Arc::clone(&self.events);
        let (host, handle) = ScreenHost::new(
            surfaces(),
            HostConfig::default(),
            outgoing,
            Arc::new(move |event| events.lock().unwrap().push(event)),
        );
        *self.handle.lock().unwrap() = Some(handle);
        Some(Box::new(host))
    }
}

/// The viewer half: a session, and the Controller channel it rides.
struct Viewer {
    session: ViewerSession,
    channel: ControllerClientChannel<tokio::io::DuplexStream>,
    reader: FrameReader,
}

impl Viewer {
    /// Sends everything the viewer session has queued, under the capability each message needs.
    async fn send(&mut self) {
        while let Some(message) = self.session.poll_outgoing() {
            let capability = match message.input_kind() {
                None => ScreenFrameCapability::Observe,
                Some(termirust_screen_protocol::InputKind::Pointer) => {
                    ScreenFrameCapability::Pointer
                }
                Some(termirust_screen_protocol::InputKind::Keyboard) => {
                    ScreenFrameCapability::Keyboard
                }
            };
            let frame = encode_frame(&message).unwrap();
            self.channel.send_screen(capability, &frame).await.unwrap();
        }
    }

    /// Reads screen frames until the viewer has applied `count` batches.
    async fn receive(&mut self, count: usize) {
        let mut applied = 0;
        while applied < count {
            let incoming = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                self.channel.read_incoming(),
            )
            .await
            .expect("the host answered")
            .unwrap();
            let ControllerIncoming::Screen(bytes) = incoming else {
                panic!("expected screen bytes");
            };
            self.reader.push(&bytes);
            while let Some(message) = self.reader.next_message().unwrap() {
                let batch = message.is_batch();
                self.session.receive(message).unwrap();
                if batch {
                    applied += 1;
                }
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_viewer_watches_and_drives_a_computer_over_the_controller_channel() {
    let host_private = StaticPrivateKey::from_fixture_bytes([51; 32]);
    let device_private = StaticPrivateKey::from_fixture_bytes([52; 32]);
    let provider: Arc<dyn ControllerAuthorityProvider> = Arc::new(Authority {
        value: Mutex::new(authority(&host_private, &device_private)),
        host_private: host_private.clone(),
    });
    let backends = Arc::new(Screens::default());
    let handle = Arc::clone(&backends.handle);
    let events = Arc::clone(&backends.events);
    let (client, mut server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(async move {
        serve_authenticated_stdio_stream(&mut server, provider, backends, CancellationToken::new())
            .await
    });

    let mut channel = ControllerClientChannel::connect(
        client,
        1,
        2,
        9,
        HostStaticPublicKey(host_public_key_from_private(&host_private).0),
        device_private,
        CapabilitySet::default()
            .with(SecurityCapability::ObserveScreens)
            .with(SecurityCapability::ControlPointer),
        &mut SystemHandshakeEntropy,
    )
    .await
    .unwrap();

    let deadline = 10_000 + now_ms();
    channel
        .send(ControllerCommand::OpenScreen, deadline)
        .await
        .unwrap();
    let ControllerResponse::ScreenOpened {
        ticket,
        can_control_pointer,
        can_control_keyboard,
        ..
    } = channel.read_response().await.unwrap()
    else {
        panic!("a screen session was not opened");
    };
    assert!(can_control_pointer && !can_control_keyboard);

    let mut viewer = Viewer {
        session: ViewerSession::new(64 << 20),
        channel,
        reader: FrameReader::new(),
    };
    viewer
        .session
        .connect(ticket.try_into().expect("a 32-byte ticket"));
    viewer.send().await;
    viewer.session.subscribe(1, Profile::Interactive);
    viewer.send().await;

    // The host captures; the viewer sees exactly those pixels.
    let handle = wait_for_handle(&handle).await;
    for step in 0..3 {
        let frame = desktop(step);
        handle
            .frame(1, &frame.as_frame(), None, u64::from(step) * 100)
            .unwrap();
        viewer.receive(1).await;
        viewer.send().await;
        assert_eq!(
            viewer.session.framebuffer(1).unwrap(),
            &frame,
            "step {step}"
        );
    }

    // Control is the host's to give.
    viewer.session.request_control();
    viewer.send().await;
    wait_for(&events, |events| {
        events.contains(&ScreenHostEvent::ControlRequested)
    })
    .await;
    handle.set_control(ControlHolder::You);
    let click = InputEvent::PointerButton {
        surface: 1,
        x: 120,
        y: 90,
        button: PointerButton::Primary,
        pressed: true,
    };
    viewer.session.send_input(click.clone());
    viewer.send().await;
    wait_for(&events, |events| {
        events.contains(&ScreenHostEvent::Input(click.clone()))
    })
    .await;

    // The keyboard was never granted, so typing ends the screen session.
    viewer.session.send_input(InputEvent::Key(KeyEvent {
        usage: 0x04,
        modifiers: Modifiers::default(),
        pressed: true,
    }));
    assert!(
        viewer.send_keyboard_refused().await,
        "the channel refuses a capability the device never had"
    );

    drop(viewer);
    assert!(server_task.await.unwrap().is_err(), "the client hung up");
    let events = events.lock().unwrap();
    assert!(matches!(
        events.first(),
        Some(ScreenHostEvent::Opened { .. })
    ));
}

impl Viewer {
    /// Tries to send the queued keyboard input; true when the channel refused it.
    async fn send_keyboard_refused(&mut self) -> bool {
        let mut refused = false;
        while let Some(message) = self.session.poll_outgoing() {
            let frame = encode_frame(&message).unwrap();
            refused |= self
                .channel
                .send_screen(ScreenFrameCapability::Keyboard, &frame)
                .await
                .is_err();
        }
        refused
    }
}

async fn wait_for_handle(handle: &Arc<Mutex<Option<ScreenHostHandle>>>) -> ScreenHostHandle {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Some(handle) = handle.lock().unwrap().clone() {
                return handle;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the screen session opened")
}

async fn wait_for(
    events: &Arc<Mutex<Vec<ScreenHostEvent>>>,
    ready: impl Fn(&[ScreenHostEvent]) -> bool,
) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if ready(&events.lock().unwrap()) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the host reported the event");
}

fn now_ms() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}

/// A device that hangs up never sends `CloseScreen`; the listener just drops the session. The
/// application still has to hear that the watcher left, or the sharing indicator keeps naming
/// someone who is gone.
#[test]
fn dropping_a_session_tells_the_application_the_device_left() {
    let events: Arc<Mutex<Vec<ScreenHostEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&events);
    let (outgoing, _incoming) = tokio::sync::mpsc::unbounded_channel();
    let (host, handle) = ScreenHost::new(
        surfaces(),
        HostConfig::default(),
        outgoing,
        Arc::new(move |event| recorded.lock().unwrap().push(event)),
    );
    assert!(handle.is_open());

    drop(host);

    assert!(!handle.is_open(), "the session is over once it is dropped");
    let events = events.lock().unwrap();
    assert!(
        matches!(events.last(), Some(ScreenHostEvent::Closed { .. })),
        "the application was told, got {events:?}"
    );
}
