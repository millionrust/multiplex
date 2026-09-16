//! Serving Remote Screens over the Controller channel.
//!
//! [`ScreenHost`] is the [`ControllerScreenSession`] the listener carries: it reassembles the
//! screen protocol from screen frames, spends the ticket the device was issued, enforces what
//! each frame's capability allows against what the message would do, and answers with tile
//! batches. The application drives it through a [`ScreenHostHandle`]: captured frames in,
//! [`ScreenHostEvent`]s out.
//!
//! Injection lives with the caller, on whatever thread its platform needs, so this crate stays
//! free of platform code. See `docs/remote-screens-implementation-plan.md`, sections 4.5 to 4.7.

#![forbid(unsafe_code)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use termirust_controller_listener::{
    ControllerScreenSession, ListenerError, ListenerErrorCode, ScreenFrameCapability, ScreenGrants,
    ScreenOutgoing, ScreenTicketStore,
};
use termirust_screen_codec::{Frame, Rect};
use termirust_screen_protocol::{
    ControlHolder, FrameReader, InputKind, Message, PanePlacement, SurfaceInfo, encode_frame,
};
use termirust_screen_session::{
    Grants, HostConfig, HostEvent, HostSession, InputEvent, ResumeStore, TicketVerifier,
};

/// Something the application must act on while a device watches its screens.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ScreenHostEvent {
    /// A device started watching. Show the sharing indicator.
    Opened { device: u64 },
    /// Input to inject, already allowed by the device's capabilities and the writer lease.
    Input(InputEvent),
    /// The device asked for control; answer with [`ScreenHostHandle::set_control`].
    ControlRequested,
    /// The device gave control back, or lost it.
    ControlReleased,
    /// The session ended; release the lease and stop capturing for this device.
    Closed { reason: String },
}

/// Where the application receives [`ScreenHostEvent`]s. Called from the connection's task.
pub type ScreenHostObserver = Arc<dyn Fn(ScreenHostEvent) + Send + Sync>;

/// The ticket the listener issued is what the screen protocol's hello proves. The grants are held
/// only between spending the ticket and the session verifying the hello.
#[derive(Default)]
struct SpentTicket(Option<Grants>);

impl TicketVerifier for SpentTicket {
    fn verify(&mut self, _proof: &[u8; 32]) -> Option<Grants> {
        self.0.take()
    }
}

struct Inner {
    session: HostSession<SpentTicket>,
    reader: FrameReader,
    resume: ResumeStore,
    outgoing: ScreenOutgoing,
    observer: ScreenHostObserver,
    /// What the spent ticket allows, once the session is open.
    grants: Option<ScreenGrants>,
    open: bool,
}

impl Inner {
    /// Sends everything the session has queued as one chunk of the screen byte stream.
    fn flush(&mut self) -> Result<(), ListenerError> {
        let mut bytes = Vec::new();
        while let Some(message) = self.session.poll_outgoing() {
            let frame = encode_frame(&message)
                .map_err(|_| ListenerError::new(ListenerErrorCode::MalformedFrame))?;
            bytes.extend(frame);
        }
        if bytes.is_empty() {
            return Ok(());
        }
        self.outgoing
            .send(bytes)
            .map_err(|_| ListenerError::new(ListenerErrorCode::Io))
    }

    fn emit(&self, event: ScreenHostEvent) {
        (self.observer)(event);
    }

    fn handle(&mut self, events: Vec<HostEvent>) {
        for event in events {
            match event {
                HostEvent::Opened { device, .. } => self.emit(ScreenHostEvent::Opened { device }),
                HostEvent::Input(input) => self.emit(ScreenHostEvent::Input(input)),
                HostEvent::ControlRequested => self.emit(ScreenHostEvent::ControlRequested),
                HostEvent::ControlReleased => self.emit(ScreenHostEvent::ControlReleased),
                HostEvent::InputRefused
                | HostEvent::Subscribed { .. }
                | HostEvent::Unsubscribed { .. } => {}
            }
        }
    }

    fn close(&mut self, reason: &str) {
        if self.open {
            self.open = false;
            self.session.close(reason, &mut self.resume);
            let _ = self.flush();
            self.emit(ScreenHostEvent::Closed {
                reason: reason.to_owned(),
            });
        }
    }
}

/// The screen session for one Controller connection.
pub struct ScreenHost {
    inner: Arc<Mutex<Inner>>,
}

impl ScreenHost {
    /// Opens a session for the surfaces this computer shares. `outgoing` comes from the listener,
    /// and the handle is how the application feeds frames in.
    pub fn new(
        surfaces: Vec<SurfaceInfo>,
        config: HostConfig,
        outgoing: ScreenOutgoing,
        observer: ScreenHostObserver,
    ) -> (Self, ScreenHostHandle) {
        let inner = Arc::new(Mutex::new(Inner {
            session: HostSession::new(surfaces, SpentTicket::default(), config),
            reader: FrameReader::new(),
            resume: ResumeStore::default(),
            outgoing,
            observer,
            grants: None,
            open: true,
        }));
        (
            Self {
                inner: Arc::clone(&inner),
            },
            ScreenHostHandle { inner },
        )
    }

    fn handle_message(
        inner: &mut Inner,
        capability: ScreenFrameCapability,
        message: Message,
        tickets: &mut ScreenTicketStore,
    ) -> Result<(), ListenerError> {
        // What the frame claimed has to cover what the message does, whatever the device pairs
        // with: watching frames never carry input, and each kind of input has its own capability.
        let allowed = match (capability, message.input_kind()) {
            (ScreenFrameCapability::Observe, None) => true,
            (ScreenFrameCapability::Pointer, Some(InputKind::Pointer)) => inner
                .grants
                .is_some_and(|grants| grants.can_control_pointer),
            (ScreenFrameCapability::Keyboard, Some(InputKind::Keyboard)) => inner
                .grants
                .is_some_and(|grants| grants.can_control_keyboard),
            _ => false,
        };
        if !allowed {
            inner.close("capability_mismatch");
            return Err(ListenerError::new(ListenerErrorCode::Unauthorized));
        }
        if let Message::Hello(hello) = &message {
            let grants = tickets
                .spend(&hello.ticket_proof)
                .ok_or_else(|| ListenerError::new(ListenerErrorCode::Unauthorized))?;
            inner.grants = Some(grants);
            inner.session.verifier_mut().0 = Some(Grants {
                device: device_id(grants),
                can_view: grants.can_view,
                can_control: grants.can_control(),
            });
        }
        let events = inner.session.receive(message, &mut inner.resume);
        match events {
            Ok(events) => {
                inner.handle(events);
                inner.flush()
            }
            Err(error) => {
                inner.open = false;
                let _ = inner.flush();
                inner.emit(ScreenHostEvent::Closed {
                    reason: error.code().to_owned(),
                });
                Err(ListenerError::new(ListenerErrorCode::Unauthorized))
            }
        }
    }
}

/// The paired device's id, as the session's resume store keys it.
fn device_id(grants: ScreenGrants) -> u64 {
    let bytes = grants.device_id.as_uuid().as_u128().to_be_bytes();
    u64::from_be_bytes(bytes[..8].try_into().expect("eight bytes"))
}

#[async_trait]
impl ControllerScreenSession for ScreenHost {
    async fn receive(
        &mut self,
        capability: ScreenFrameCapability,
        bytes: &[u8],
        tickets: &mut ScreenTicketStore,
    ) -> Result<(), ListenerError> {
        let mut inner = self.inner.lock().expect("screen host mutex");
        if !inner.open {
            return Err(ListenerError::new(ListenerErrorCode::Unauthorized));
        }
        inner.reader.push(bytes);
        loop {
            let message = inner
                .reader
                .next_message()
                .map_err(|_| ListenerError::new(ListenerErrorCode::MalformedFrame))?;
            let Some(message) = message else {
                return Ok(());
            };
            Self::handle_message(&mut inner, capability, message, tickets)?;
        }
    }

    fn close(&mut self) {
        self.inner
            .lock()
            .expect("screen host mutex")
            .close("sharing_stopped");
    }
}

/// The application's side of a screen session: frames in, control decisions in.
#[derive(Clone)]
pub struct ScreenHostHandle {
    inner: Arc<Mutex<Inner>>,
}

impl ScreenHostHandle {
    /// Feeds one captured frame of `surface`. Encoding happens here, on the caller's thread, and
    /// only for what the device is watching.
    pub fn frame(
        &self,
        surface: u32,
        frame: &Frame<'_>,
        damage: Option<&[Rect]>,
        now_ms: u64,
    ) -> Result<(), ListenerError> {
        let mut inner = self.inner.lock().expect("screen host mutex");
        if !inner.open {
            return Ok(());
        }
        if inner.session.frame(surface, frame, damage, now_ms).is_err() {
            inner.close("encode_failed");
            return Err(ListenerError::new(ListenerErrorCode::MalformedFrame));
        }
        inner.flush()
    }

    /// Publishes where TermiRust terminal panes sit, so attached viewers draw them from text.
    pub fn set_panes(&self, surface: u32, panes: Vec<PanePlacement>) {
        let mut inner = self.inner.lock().expect("screen host mutex");
        inner.session.set_panes(surface, panes);
        let _ = inner.flush();
    }

    /// Tells the device who holds control; the application owns the writer lease.
    pub fn set_control(&self, holder: ControlHolder) {
        let mut inner = self.inner.lock().expect("screen host mutex");
        inner.session.set_control(holder);
        let _ = inner.flush();
    }

    /// Whether a device is still watching.
    pub fn is_open(&self) -> bool {
        self.inner.lock().expect("screen host mutex").open
    }

    /// Stops sharing with this device.
    pub fn stop(&self, reason: &str) {
        self.inner.lock().expect("screen host mutex").close(reason);
    }
}
