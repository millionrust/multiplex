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

// The VideoToolbox encoder lives in termirust-screen-video, which owns the one unsafe block in
// this path; nothing here touches a raw pointer.
#![forbid(unsafe_code)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use termirust_controller_listener::{
    ControllerScreenSession, ListenerError, ListenerErrorCode, ScreenFrameCapability, ScreenGrants,
    ScreenOutgoing, ScreenTicketStore,
};
use termirust_screen_codec::{Frame, Rect};
use termirust_screen_protocol::{
    ControlHolder, FeatureSet, FrameReader, InputKind, Message, PanePlacement, SurfaceInfo,
    encode_frame,
};
use termirust_screen_session::{
    Grants, HostConfig, HostEvent, HostSession, InputEvent, ResumeStore, TicketVerifier,
};

pub mod motion;
pub mod parity;
pub mod rate;
#[cfg(target_os = "macos")]
mod video;

pub use motion::{
    MotionEncoder, MotionEncoders, MotionFrame, MotionRequest, MotionSender, NoEncoders,
    platform_encoders,
};
pub use rate::RateEstimator;

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
    /// Sends the motion region as video, for viewers that negotiated it. Idle otherwise.
    motion: MotionSender,
    /// Names the burst each flush closes, so a report can be matched to what it measured.
    burst: u64,
    /// What the link has recently delivered, measured rather than assumed.
    rate: RateEstimator,
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
        // One flush is one burst. The mark closes it and says how much went before it, which is
        // the half of the measurement only this side knows; the viewer supplies the other half by
        // timing the arrival. Built here rather than through the session's outbox because the
        // count is of encoded bytes, which is a fact about this layer and not about the protocol.
        if self
            .session
            .agreed_features()
            .has(FeatureSet::BANDWIDTH_REPORTS)
        {
            let mark = Message::BurstMark {
                burst: self.burst,
                bytes: bytes.len() as u64,
            };
            let frame = encode_frame(&mark)
                .map_err(|_| ListenerError::new(ListenerErrorCode::MalformedFrame))?;
            bytes.extend(frame);
            self.burst = self.burst.wrapping_add(1);
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
                // Long-term reference feedback goes to the encoder, never to the application: it
                // is about the stream, not about what the device is allowed to do.
                HostEvent::VideoAcknowledged { tokens, .. } => self.motion.acknowledged(&tokens),
                HostEvent::VideoLost { .. } => self.motion.lost(),
                // The measurement of the link. It stays here rather than reaching the
                // application: what to do about it is 5.2, and nothing above this needs it.
                HostEvent::BurstMeasured {
                    bytes,
                    spread_micros,
                    ..
                } => self.rate.record(bytes, spread_micros),
                HostEvent::InputRefused
                | HostEvent::Subscribed { .. }
                | HostEvent::Unsubscribed { .. } => {}
            }
        }
    }

    fn close(&mut self, reason: &str) {
        if self.open {
            self.open = false;
            self.motion.stop();
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
        Self::with_encoders(surfaces, config, outgoing, observer, platform_encoders())
    }

    /// As [`Self::new`], with the video encoders named. Tests use it to drive the motion path
    /// with a fake encoder, or to turn it off with [`NoEncoders`].
    pub fn with_encoders(
        surfaces: Vec<SurfaceInfo>,
        config: HostConfig,
        outgoing: ScreenOutgoing,
        observer: ScreenHostObserver,
        encoders: Box<dyn MotionEncoders>,
    ) -> (Self, ScreenHostHandle) {
        let inner = Arc::new(Mutex::new(Inner {
            session: HostSession::new(surfaces, SpentTicket::default(), config),
            reader: FrameReader::new(),
            resume: ResumeStore::default(),
            outgoing,
            observer,
            grants: None,
            open: true,
            motion: MotionSender::new(encoders),
            burst: 0,
            rate: RateEstimator::new(),
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

/// A device that hangs up never sends `CloseScreen`, and the listener simply drops its session,
/// so closing here is what tells the application the watcher is gone. Without it the sharing
/// indicator keeps naming someone who left, and the injection thread keeps waiting for input.
/// [`Inner::close`] is idempotent, so a session closed properly first does not close twice.
impl Drop for ScreenHost {
    fn drop(&mut self) {
        self.inner
            .lock()
            .expect("screen host mutex")
            .close("device_left");
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
        // After the tile encoder, which is what decides whether this surface has a motion region
        // at all. On a viewer that did not negotiate video this does nothing.
        let Inner {
            session, motion, ..
        } = &mut *inner;
        motion.frame(session, surface, frame);
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

    /// Bytes per second this link has recently delivered, once enough bursts have been measured.
    ///
    /// `None` means nothing is known yet — a session that has only sent a few hundred bytes of
    /// typing has measured nothing worth acting on — and a caller should keep its defaults rather
    /// than degrade on an estimate it does not have. 5.2 is what acts on it.
    pub fn estimated_bytes_per_second(&self) -> Option<u64> {
        self.inner
            .lock()
            .expect("screen host mutex")
            .rate
            .estimate()
    }

    /// Stops sharing with this device.
    pub fn stop(&self, reason: &str) {
        self.inner.lock().expect("screen host mutex").close(reason);
    }
}
