//! Watching and driving a TermiRust computer's screen from a phone.
//!
//! The phone already owns the Controller connection through `termirust-controller-bindings`;
//! this boundary owns what rides inside its screen frames. Feed it the bytes of every screen
//! frame that arrives, take the bytes it wants sent back, and read pixels for the parts that
//! changed.
//!
//! It performs no I/O, keeps no thread, and copies pixels only for the rectangles asked for,
//! because a phone should not copy a 24 MB framebuffer to draw a blinking cursor.

#![forbid(unsafe_code)]

use std::sync::Mutex;

use termirust_screen_codec::Rect;
use termirust_screen_protocol::{
    ControlHolder, FeatureSet, FrameReader, InputKind, KeyEvent, Modifiers, PointerButton, Profile,
    ResumeOutcome, Viewport, encode_frame,
};
use termirust_screen_session::{InputEvent, ViewerEvent, ViewerSession};

uniffi::setup_scaffolding!();

/// What a device may do, and therefore which capability a screen frame must claim.
#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum ScreenCapability {
    /// Everything that is not input.
    Observe,
    Pointer,
    Keyboard,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum ScreenControlHolder {
    Nobody,
    You,
    AnotherDevice,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum ScreenResume {
    NotRequested,
    Partial,
    Full,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum ScreenPointerButton {
    Primary,
    Secondary,
    Middle,
}

/// One screen this computer shares.
#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct ScreenSurface {
    pub id: u32,
    pub width: u32,
    pub height: u32,
    /// Pixels per point, in thousandths.
    pub scale_milli: u16,
    pub name: String,
}

/// A rectangle of a surface, in surface pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Record)]
pub struct ScreenRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Where a TermiRust terminal pane sits, for clients that draw it from its text instead.
#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct ScreenPane {
    /// The hosted session id, as 16 bytes.
    pub session: Vec<u8>,
    pub rect: ScreenRect,
    pub cell_width: u16,
    pub cell_height: u16,
}

/// Bytes to send to the computer, under the capability they need.
#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct ScreenOutgoing {
    pub capability: ScreenCapability,
    pub bytes: Vec<u8>,
}

/// Pixels of one rectangle, in BGRA order, row by row with no padding.
#[derive(Clone, Debug, Eq, PartialEq, uniffi::Record)]
pub struct ScreenPixels {
    pub rect: ScreenRect,
    pub bgra: Vec<u8>,
}

/// Something the phone's interface should act on.
#[derive(Clone, Debug, Eq, PartialEq, uniffi::Enum)]
pub enum ScreenEvent {
    /// The computer accepted the ticket and listed what it shares.
    Welcomed {
        surfaces: Vec<ScreenSurface>,
        resume: ScreenResume,
    },
    /// Part of a surface changed. Read the rectangles and draw them.
    Updated {
        surface: u32,
        preview: bool,
        damaged: Vec<ScreenRect>,
        /// The whole surface was replaced; anything drawn before is stale.
        reset: bool,
    },
    Control {
        holder: ScreenControlHolder,
    },
    /// A region is changing fast enough that the computer will stream it (Stage B).
    MotionRegion {
        surface: u32,
        rect: Option<ScreenRect>,
    },
    Panes {
        surface: u32,
        panes: Vec<ScreenPane>,
    },
    /// The decoder configuration for a surface's motion region, which has to reach the decoder
    /// before the first frame does.
    ///
    /// Only sent on platforms this library cannot decode for itself. On Apple platforms the
    /// region is decoded here and arrives as [`ScreenEvent::Updated`] like everything else, so a
    /// client that never handles this case still shows video there.
    VideoConfig {
        surface: u32,
        /// Where on the surface to draw the decoded picture.
        rect: ScreenRect,
        /// Parameter sets in Annex B, which is what `MediaCodec` wants as `csd-0`.
        parameter_sets: Vec<u8>,
    },
    /// One encoded frame of the motion region, in Annex B. See [`ScreenEvent::VideoConfig`].
    VideoFrame {
        surface: u32,
        sequence: u64,
        keyframe: bool,
        /// The long-term reference this frame carries, if any. Once it has decoded, name it in
        /// [`ScreenViewer::report_video`] so the computer may predict from it.
        token: Option<u32>,
        payload: Vec<u8>,
    },
    Closed {
        reason: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, uniffi::Error)]
pub enum ScreenBindingError {
    /// The computer sent something this session cannot accept; the session is over. Named for
    /// what it is rather than "protocol", which Swift will not accept as a case name.
    InvalidMessage,
    /// A ticket must be exactly 32 bytes.
    InvalidTicket,
    /// A rectangle was empty or outside the surface.
    InvalidRect,
    /// Modifier bits outside the four defined ones.
    InvalidModifiers,
    /// The session is not open.
    Closed,
    Unexpected,
}

impl std::fmt::Display for ScreenBindingError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidMessage => "invalid_message",
            Self::InvalidTicket => "invalid_ticket",
            Self::InvalidRect => "invalid_rect",
            Self::InvalidModifiers => "invalid_modifiers",
            Self::Closed => "closed",
            Self::Unexpected => "unexpected",
        })
    }
}

impl std::error::Error for ScreenBindingError {}

/// One screen session: the phone's side of watching one computer.
#[derive(uniffi::Object)]
pub struct ScreenViewer {
    inner: Mutex<Inner>,
}

struct Inner {
    session: ViewerSession,
    reader: FrameReader,
}

#[uniffi::export]
impl ScreenViewer {
    /// `cache_bytes` is the tile cache this phone can spare; the computer models the same budget.
    ///
    /// A viewer built this way watches the tile path only. Use [`Self::with_motion`] to ask for
    /// the motion region as video as well.
    #[uniffi::constructor]
    pub fn new(cache_bytes: u64) -> std::sync::Arc<Self> {
        Self::with_motion(cache_bytes, false)
    }

    /// A viewer that also asks for the motion region as encoded video.
    ///
    /// `decodes_video` is the caller's promise that it will draw what arrives. On a platform this
    /// library decodes for itself — every Apple one — it is ignored and video is always asked
    /// for, because there is nothing for the caller to do. Everywhere else, saying yes without
    /// handling [`ScreenEvent::VideoConfig`] and [`ScreenEvent::VideoFrame`] leaves the moving
    /// part of the screen frozen, so the default is no.
    #[uniffi::constructor]
    pub fn with_motion(cache_bytes: u64, decodes_video: bool) -> std::sync::Arc<Self> {
        let decodes_here = cfg!(any(target_os = "macos", target_os = "ios"));
        let features = if decodes_here || decodes_video {
            FeatureSet::from_bits(FeatureSet::KNOWN)
        } else {
            // Parity and reference acknowledgement are only about video, so a viewer that will
            // not show video asks for none of them.
            FeatureSet::none()
        };
        std::sync::Arc::new(Self {
            inner: Mutex::new(Inner {
                session: ViewerSession::with_features(cache_bytes as usize, features),
                reader: FrameReader::new(),
            }),
        })
    }

    /// Tells the computer which long-term references this client's decoder holds, and which frame
    /// it could not rebuild.
    ///
    /// Only for clients decoding video themselves. A reference must be named **only once the
    /// frame carrying it has actually decoded**: the computer predicts from what this says it
    /// holds, so naming one it does not have produces a stream it cannot decode.
    ///
    /// Does nothing where this library decodes for itself, because it reports on its own.
    pub fn report_video(&self, surface: u32, held: Vec<u32>, lost: Option<u64>) {
        let _ = self.with(|inner| {
            let _ = inner.session.report_video(surface, &held, lost);
            Ok(())
        });
    }

    /// Starts a session with the ticket the computer issued over the Controller channel. When a
    /// view was open before, this asks to resume it instead of resending the whole screen.
    pub fn connect(&self, ticket: Vec<u8>) -> Result<(), ScreenBindingError> {
        let proof: [u8; 32] = ticket
            .try_into()
            .map_err(|_| ScreenBindingError::InvalidTicket)?;
        self.with(|inner| {
            inner.session.connect(proof);
            Ok(())
        })
    }

    /// The Controller connection dropped. Pixels and caches are kept for the next connect.
    pub fn disconnected(&self) {
        let _ = self.with(|inner| {
            inner.session.disconnected();
            inner.reader = FrameReader::new();
            Ok(())
        });
    }

    pub fn subscribe(&self, surface: u32, preview: bool) {
        let profile = if preview {
            Profile::Thumbnail
        } else {
            Profile::Interactive
        };
        let _ = self.with(|inner| {
            inner.session.subscribe(surface, profile);
            Ok(())
        });
    }

    pub fn unsubscribe(&self, surface: u32) {
        let _ = self.with(|inner| {
            inner.session.unsubscribe(surface);
            Ok(())
        });
    }

    /// Tells the computer what part of the surface is on screen, so it sends that part first.
    pub fn set_viewport(
        &self,
        surface: u32,
        rect: ScreenRect,
        scale_milli: u16,
    ) -> Result<(), ScreenBindingError> {
        let rect = codec_rect(rect)?;
        if scale_milli == 0 {
            return Err(ScreenBindingError::InvalidRect);
        }
        self.with(|inner| {
            inner.session.set_viewport(Viewport {
                surface,
                rect,
                scale_milli,
            });
            Ok(())
        })
    }

    /// Names the terminal sessions this phone draws from their text, so the computer stops
    /// sending pixels where those panes sit.
    pub fn attach_panes(&self, sessions: Vec<Vec<u8>>) -> Result<(), ScreenBindingError> {
        let sessions = sessions
            .into_iter()
            .map(|session| {
                session
                    .try_into()
                    .map_err(|_| ScreenBindingError::InvalidMessage)
            })
            .collect::<Result<Vec<[u8; 16]>, _>>()?;
        self.with(|inner| {
            inner.session.attach_panes(sessions);
            Ok(())
        })
    }

    pub fn request_control(&self) {
        let _ = self.with(|inner| {
            inner.session.request_control();
            Ok(())
        });
    }

    pub fn release_control(&self) {
        let _ = self.with(|inner| {
            inner.session.release_control();
            Ok(())
        });
    }

    pub fn control(&self) -> ScreenControlHolder {
        self.with(|inner| Ok(holder(inner.session.control())))
            .unwrap_or(ScreenControlHolder::Nobody)
    }

    pub fn send_pointer_move(&self, surface: u32, x: u32, y: u32) {
        self.send(InputEvent::PointerMove { surface, x, y });
    }

    pub fn send_pointer_button(
        &self,
        surface: u32,
        x: u32,
        y: u32,
        button: ScreenPointerButton,
        pressed: bool,
    ) {
        self.send(InputEvent::PointerButton {
            surface,
            x,
            y,
            button: match button {
                ScreenPointerButton::Primary => PointerButton::Primary,
                ScreenPointerButton::Secondary => PointerButton::Secondary,
                ScreenPointerButton::Middle => PointerButton::Middle,
            },
            pressed,
        });
    }

    /// Positive `dy` shows content above, as a wheel turned away from the reader does.
    pub fn send_scroll(&self, surface: u32, x: u32, y: u32, dx: i32, dy: i32) {
        self.send(InputEvent::Scroll {
            surface,
            x,
            y,
            dx,
            dy,
        });
    }

    /// `usage` is a USB HID usage on the keyboard page, so layouts stay the computer's business.
    pub fn send_key(
        &self,
        usage: u16,
        modifiers: u8,
        pressed: bool,
    ) -> Result<(), ScreenBindingError> {
        let modifiers = Modifiers::new(modifiers).ok_or(ScreenBindingError::InvalidModifiers)?;
        self.send(InputEvent::Key(KeyEvent {
            usage,
            modifiers,
            pressed,
        }));
        Ok(())
    }

    pub fn send_text(&self, surface: u32, text: String) {
        if text.is_empty() {
            return;
        }
        self.send(InputEvent::Text { surface, text });
    }

    /// Feeds the payload of one screen frame. Returns what the interface should act on.
    pub fn receive(&self, bytes: Vec<u8>) -> Result<Vec<ScreenEvent>, ScreenBindingError> {
        self.with(|inner| {
            inner.reader.push(&bytes);
            let mut events = Vec::new();
            loop {
                let message = inner
                    .reader
                    .next_message()
                    .map_err(|_| ScreenBindingError::InvalidMessage)?;
                let Some(message) = message else {
                    return Ok(events);
                };
                let applied = inner
                    .session
                    .receive(message)
                    .map_err(|_| ScreenBindingError::InvalidMessage)?;
                events.extend(applied.into_iter().map(event));
            }
        })
    }

    /// The next bytes to send inside a screen frame, with the capability that frame must claim.
    pub fn poll_outgoing(&self) -> Option<ScreenOutgoing> {
        self.with(|inner| {
            Ok(inner.session.poll_outgoing().and_then(|message| {
                let capability = match message.input_kind() {
                    None => ScreenCapability::Observe,
                    Some(InputKind::Pointer) => ScreenCapability::Pointer,
                    Some(InputKind::Keyboard) => ScreenCapability::Keyboard,
                };
                encode_frame(&message)
                    .ok()
                    .map(|bytes| ScreenOutgoing { capability, bytes })
            }))
        })
        .unwrap_or_default()
    }

    /// The size of a surface's picture, once anything arrived for it.
    pub fn surface_size(&self, surface: u32, preview: bool) -> Option<ScreenRect> {
        self.with(|inner| {
            let frame = if preview {
                inner.session.preview(surface)
            } else {
                inner.session.framebuffer(surface)
            };
            Ok(frame.map(|frame| ScreenRect {
                x: 0,
                y: 0,
                width: frame.size().width(),
                height: frame.size().height(),
            }))
        })
        .unwrap_or_default()
    }

    /// Copies one rectangle of a surface, for drawing what an update reported as damaged.
    pub fn copy_pixels(
        &self,
        surface: u32,
        preview: bool,
        rect: ScreenRect,
    ) -> Result<ScreenPixels, ScreenBindingError> {
        let wanted = codec_rect(rect)?;
        self.with(|inner| {
            let frame = if preview {
                inner.session.preview(surface)
            } else {
                inner.session.framebuffer(surface)
            }
            .ok_or(ScreenBindingError::Closed)?;
            let clipped = wanted.intersect(frame.size().bounds());
            if clipped.is_empty() {
                return Err(ScreenBindingError::InvalidRect);
            }
            let view = frame.as_frame();
            let mut bgra = Vec::with_capacity(clipped.width as usize * clipped.height as usize * 4);
            for y in clipped.y..clipped.y + clipped.height {
                let row = view.row(y);
                let start = clipped.x as usize * 4;
                let end = start + clipped.width as usize * 4;
                bgra.extend_from_slice(&row[start..end]);
            }
            Ok(ScreenPixels {
                rect: wire_rect(clipped),
                bgra,
            })
        })
    }
}

impl ScreenViewer {
    fn send(&self, input: InputEvent) {
        let _ = self.with(|inner| {
            inner.session.send_input(input);
            Ok(())
        });
    }

    fn with<T>(
        &self,
        action: impl FnOnce(&mut Inner) -> Result<T, ScreenBindingError>,
    ) -> Result<T, ScreenBindingError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| ScreenBindingError::Unexpected)?;
        action(&mut inner)
    }
}

fn holder(value: ControlHolder) -> ScreenControlHolder {
    match value {
        ControlHolder::Nobody => ScreenControlHolder::Nobody,
        ControlHolder::You => ScreenControlHolder::You,
        ControlHolder::AnotherDevice => ScreenControlHolder::AnotherDevice,
    }
}

fn codec_rect(rect: ScreenRect) -> Result<Rect, ScreenBindingError> {
    let value = Rect::new(rect.x, rect.y, rect.width, rect.height);
    if value.is_empty() {
        return Err(ScreenBindingError::InvalidRect);
    }
    Ok(value)
}

const fn wire_rect(rect: Rect) -> ScreenRect {
    ScreenRect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}

/// Turns a session event into one the phone can act on.
fn event(value: ViewerEvent) -> ScreenEvent {
    match value {
        ViewerEvent::Welcomed { surfaces, resume } => ScreenEvent::Welcomed {
            surfaces: surfaces
                .into_iter()
                .map(|surface| ScreenSurface {
                    id: surface.id,
                    width: surface.size.width(),
                    height: surface.size.height(),
                    scale_milli: surface.scale_milli,
                    name: surface.name,
                })
                .collect(),
            resume: match resume {
                ResumeOutcome::NotRequested => ScreenResume::NotRequested,
                ResumeOutcome::Partial => ScreenResume::Partial,
                ResumeOutcome::Full => ScreenResume::Full,
            },
        },
        ViewerEvent::Updated {
            surface,
            preview,
            damaged,
            reset,
        } => ScreenEvent::Updated {
            surface,
            preview,
            damaged: damaged.into_iter().map(wire_rect).collect(),
            reset,
        },
        ViewerEvent::Control(value) => ScreenEvent::Control {
            holder: holder(value),
        },
        ViewerEvent::MotionRegion { surface, rect } => ScreenEvent::MotionRegion {
            surface,
            rect: rect.map(wire_rect),
        },
        ViewerEvent::Panes { surface, panes } => ScreenEvent::Panes {
            surface,
            panes: panes
                .into_iter()
                .map(|pane| ScreenPane {
                    session: pane.session.to_vec(),
                    rect: wire_rect(pane.rect),
                    cell_width: pane.cell_width,
                    cell_height: pane.cell_height,
                })
                .collect(),
        },
        ViewerEvent::Closed { reason } => ScreenEvent::Closed { reason },
        // The motion path, on a platform this library has no decoder for. Where it does have one
        // the session decodes and draws the region itself, and these never reach here.
        ViewerEvent::VideoConfig(config) => ScreenEvent::VideoConfig {
            surface: config.surface,
            rect: wire_rect(config.rect),
            parameter_sets: config.payload,
        },
        ViewerEvent::VideoFrame(frame) => ScreenEvent::VideoFrame {
            surface: frame.surface,
            sequence: frame.sequence,
            keyframe: frame.keyframe,
            token: frame.token,
            payload: frame.payload,
        },
    }
}

/// The messages this boundary refuses to build, so a caller cannot make the computer close the
/// session by accident.
#[cfg(test)]
mod tests {
    use super::*;

    fn viewer() -> std::sync::Arc<ScreenViewer> {
        ScreenViewer::new(64 << 20)
    }

    #[test]
    fn a_ticket_is_exactly_thirty_two_bytes() {
        assert_eq!(
            viewer().connect(vec![7; 31]),
            Err(ScreenBindingError::InvalidTicket)
        );
        assert_eq!(viewer().connect(vec![7; 32]), Ok(()));
    }

    #[test]
    fn the_hello_and_input_go_out_under_the_right_capability() {
        let viewer = viewer();
        viewer.connect(vec![7; 32]).unwrap();
        let hello = viewer.poll_outgoing().expect("a hello is queued");
        assert_eq!(hello.capability, ScreenCapability::Observe);
        assert!(!hello.bytes.is_empty());

        viewer.subscribe(1, false);
        assert_eq!(
            viewer.poll_outgoing().map(|out| out.capability),
            Some(ScreenCapability::Observe)
        );

        viewer.send_pointer_move(1, 10, 20);
        assert_eq!(
            viewer.poll_outgoing().map(|out| out.capability),
            Some(ScreenCapability::Pointer)
        );
        viewer.send_key(0x04, 0, true).unwrap();
        assert_eq!(
            viewer.poll_outgoing().map(|out| out.capability),
            Some(ScreenCapability::Keyboard)
        );
        viewer.send_text(1, "ls\n".to_owned());
        assert_eq!(
            viewer.poll_outgoing().map(|out| out.capability),
            Some(ScreenCapability::Keyboard)
        );
        assert_eq!(viewer.poll_outgoing(), None);
    }

    #[test]
    fn nonsense_from_the_caller_is_refused_before_it_reaches_the_computer() {
        let viewer = viewer();
        assert_eq!(
            viewer.send_key(0x04, 0xF0, true),
            Err(ScreenBindingError::InvalidModifiers)
        );
        let empty = ScreenRect {
            x: 0,
            y: 0,
            width: 0,
            height: 10,
        };
        assert_eq!(
            viewer.set_viewport(1, empty, 1000),
            Err(ScreenBindingError::InvalidRect)
        );
        assert_eq!(
            viewer.attach_panes(vec![vec![1; 15]]),
            Err(ScreenBindingError::InvalidMessage)
        );

        viewer.send_text(1, String::new());
        viewer.connect(vec![7; 32]).unwrap();
        assert!(viewer.poll_outgoing().is_some(), "only the hello is queued");
        assert_eq!(viewer.poll_outgoing(), None, "empty text is not a message");
    }

    #[test]
    fn bytes_that_are_not_a_screen_session_end_it() {
        let viewer = viewer();
        viewer.connect(vec![7; 32]).unwrap();
        assert_eq!(
            viewer.receive(vec![0xFF; 64]),
            Err(ScreenBindingError::InvalidMessage)
        );
    }
}
