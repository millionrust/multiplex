//! The xdg-desktop-portal ScreenCast backend, over PipeWire.
//!
//! On Wayland an application cannot enumerate screens, choose one, or start capturing them. The
//! compositor does all three, behind a portal the user answers. That is not a limitation to work
//! around; it is the security model, and it shapes this backend in three ways that the macOS and
//! Windows ones do not share.
//!
//! **There is no `displays()`.** Nothing can be listed before the user has picked, so this module
//! deliberately exports no enumeration rather than inventing a placeholder screen. A caller starts
//! a source and then asks it what it was given, with [`PortalScreenCastSource::display`].
//!
//! **`CaptureConfig::display_id` is ignored**, for the same reason: the picker chooses, not the
//! caller. Passing a token from a previous session is how a second run skips the dialog, and that
//! is what [`PortalScreenCastSource::restore_token`] is for — store it and hand it back next time.
//!
//! **Frames arrive with [`Damage::Unknown`].** PipeWire does carry damage regions, in a
//! `SPA_META_VideoDamage` on each buffer, but the safe `pipewire` 0.8 buffer type exposes only the
//! data planes and no metadata, so reaching them means hand-rolling the whole stream against
//! `pw_sys`. It is worth being exact about what that costs, because it is easy to overstate:
//! damage saves the encoder hashing every tile every frame, not bytes on the wire. Without it the
//! encoder compares everything and still sends only what changed. So this costs CPU and the
//! latency that follows from it, and nothing else, which is a fair price for a first backend.

#![allow(unsafe_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ashpd::desktop::screencast::{
    CursorMode, OpenPipeWireRemoteOptions, Screencast, SelectSourcesOptions, SourceType,
    StartCastOptions,
};
use ashpd::desktop::{CreateSessionOptions, PersistMode};
use ashpd::enumflags2::BitFlags;
use multiplex_screen_codec::{BYTES_PER_PIXEL, Size};
use pipewire as pw;
use pw::spa;
use pw::spa::pod::Pod;
use spa::param::format::{FormatProperties, MediaSubtype, MediaType};
use spa::param::video::{VideoFormat, VideoInfoRaw};
use spa::utils::{Fraction, Rectangle};

use crate::{CaptureConfig, CaptureError, CapturedFrame, Damage, DisplayInfo, FrameSource};

/// Frames waiting for the consumer. A small queue keeps latency low; overflow drops frames, which
/// costs nothing here because every frame already reports unknown damage.
const QUEUE_DEPTH: usize = 2;

/// What the portal handed back, which is all this backend knows about the screen.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Negotiated {
    size: Size,
    stride: usize,
}

/// Captures the screen the user chose in the portal dialog.
pub struct PortalScreenCastSource {
    frames: Receiver<CapturedFrame>,
    /// Filled by the stream thread once PipeWire has settled on a format. Until then there is no
    /// honest answer to "how big is the screen", because the compositor has not said.
    negotiated: Arc<Mutex<Option<Negotiated>>>,
    display: DisplayInfo,
    restore_token: Option<String>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl PortalScreenCastSource {
    /// Asks the portal for a screen and starts streaming it.
    ///
    /// Blocks while the user answers the dialog, which may be a long time or never. A
    /// `restore_token` from a previous session skips the dialog when the compositor supports it.
    pub fn start(config: CaptureConfig, restore_token: Option<&str>) -> Result<Self, CaptureError> {
        let session = async_io::block_on(open_session(config, restore_token))?;
        let stop = Arc::new(AtomicBool::new(false));
        let negotiated = Arc::new(Mutex::new(None));
        let (sender, frames) = sync_channel(QUEUE_DEPTH);

        let thread = {
            let stop = Arc::clone(&stop);
            let negotiated = Arc::clone(&negotiated);
            let node = session.node_id;
            let fd = session.fd;
            std::thread::Builder::new()
                .name("screen-capture-pipewire".to_owned())
                .spawn(move || run_stream(fd, node, &sender, &negotiated, &stop))
                .map_err(|_| CaptureError::Unavailable)?
        };

        Ok(Self {
            frames,
            negotiated,
            display: session.display,
            restore_token: session.restore_token,
            stop,
            thread: Some(thread),
        })
    }

    /// What the portal actually gave, which the caller did not get to choose.
    ///
    /// The size here is what the portal reported when the session started. The frames may settle
    /// on a different one, because PipeWire negotiates the format separately; once a frame has
    /// arrived, its own size is the authority.
    pub const fn display(&self) -> DisplayInfo {
        self.display
    }

    /// The token to hand back to [`PortalScreenCastSource::start`] next time, so the user is not
    /// asked again. `None` when the compositor does not support restoring.
    pub fn restore_token(&self) -> Option<&str> {
        self.restore_token.as_deref()
    }

    /// The format PipeWire settled on, once it has.
    pub fn negotiated_size(&self) -> Option<Size> {
        self.negotiated
            .lock()
            .ok()
            .and_then(|state| state.map(|state| state.size))
    }
}

impl FrameSource for PortalScreenCastSource {
    fn next_frame(&mut self, timeout: Duration) -> Result<Option<CapturedFrame>, CaptureError> {
        match self.frames.recv_timeout(timeout) {
            Ok(frame) => Ok(Some(frame)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            // The stream thread is gone, which on this path means the compositor ended the
            // session — the user revoked it, or the screen it was sharing disappeared.
            Err(RecvTimeoutError::Disconnected) => Err(CaptureError::Ended),
        }
    }
}

impl Drop for PortalScreenCastSource {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // The loop checks the flag on its own timer, so this joins rather than detaching: leaving
        // a PipeWire main loop running against a dropped channel is how a portal session stays
        // open and the user keeps seeing a "screen is being shared" indicator for nothing.
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct PortalSession {
    fd: std::os::fd::OwnedFd,
    node_id: u32,
    display: DisplayInfo,
    restore_token: Option<String>,
}

async fn open_session(
    config: CaptureConfig,
    restore_token: Option<&str>,
) -> Result<PortalSession, CaptureError> {
    let proxy = Screencast::new().await.map_err(portal_error)?;
    let session = proxy
        .create_session(CreateSessionOptions::default())
        .await
        .map_err(portal_error)?;
    let sources = SelectSourcesOptions::default()
        .set_cursor_mode(if config.show_cursor {
            CursorMode::Embedded
        } else {
            CursorMode::Hidden
        })
        .set_sources(BitFlags::from(SourceType::Monitor))
        .set_multiple(false)
        .set_restore_token(restore_token)
        // The point of asking is not to have to ask again: the grant lasts until the user takes
        // it back, and the token in the response is what skips the dialog next time.
        .set_persist_mode(PersistMode::ExplicitlyRevoked);
    proxy
        .select_sources(&session, sources)
        .await
        .map_err(portal_error)?;

    let response = proxy
        .start(&session, None, StartCastOptions::default())
        .await
        .map_err(portal_error)?
        .response()
        .map_err(portal_error)?;
    let stream = response
        .streams()
        .first()
        .ok_or(CaptureError::NotPermitted)?;
    let (width, height) = stream.size().unwrap_or((0, 0));
    let display = DisplayInfo {
        id: stream.pipe_wire_node_id(),
        size_points: Size::new(width.max(0) as u32, height.max(0) as u32)
            .map_err(|_| CaptureError::InvalidFrame)?,
        origin_points: stream.position().unwrap_or((0, 0)),
    };

    let fd = proxy
        .open_pipe_wire_remote(&session, OpenPipeWireRemoteOptions::default())
        .await
        .map_err(portal_error)?;
    Ok(PortalSession {
        fd,
        node_id: stream.pipe_wire_node_id(),
        display,
        restore_token: response.restore_token().map(str::to_owned),
    })
}

/// A portal refusal is the user saying no, which is not the same as the portal being broken.
fn portal_error(error: ashpd::Error) -> CaptureError {
    match error {
        ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled)
        | ashpd::Error::Response(ashpd::desktop::ResponseError::Other) => {
            CaptureError::NotPermitted
        }
        _ => CaptureError::Unavailable,
    }
}

/// Runs the PipeWire main loop until the source is dropped.
fn run_stream(
    fd: std::os::fd::OwnedFd,
    node_id: u32,
    sender: &SyncSender<CapturedFrame>,
    negotiated: &Arc<Mutex<Option<Negotiated>>>,
    stop: &Arc<AtomicBool>,
) {
    // Errors here end the thread, which closes the channel, which `next_frame` reports as the
    // session having ended. There is nobody else to tell.
    let Ok(main_loop) = pw::main_loop::MainLoop::new(None) else {
        return;
    };
    let Ok(context) = pw::context::Context::new(&main_loop) else {
        return;
    };
    let Ok(core) = context.connect_fd(fd, None) else {
        return;
    };
    let Ok(stream) = pw::stream::Stream::new(
        &core,
        "termirust-screen",
        pw::properties::properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    ) else {
        return;
    };

    let started = Instant::now();
    let state = StreamState {
        sender: sender.clone(),
        negotiated: Arc::clone(negotiated),
        started,
        info: VideoInfoRaw::new(),
    };

    let listener = stream
        .add_local_listener_with_user_data(state)
        .param_changed(|_, state, id, param| {
            let Some(param) = param else { return };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let Ok((media_type, media_subtype)) = spa::param::format_utils::parse_format(param)
            else {
                return;
            };
            if media_type != MediaType::Video || media_subtype != MediaSubtype::Raw {
                return;
            }
            if state.info.parse(param).is_err() {
                return;
            }
            let Ok(size) = Size::new(state.info.size().width, state.info.size().height) else {
                return;
            };
            if let Ok(mut slot) = state.negotiated.lock() {
                *slot = Some(Negotiated {
                    size,
                    stride: size.width() as usize * BYTES_PER_PIXEL,
                });
            }
        })
        .process(|stream, state| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let Some(negotiated) = state.negotiated.lock().ok().and_then(|slot| *slot) else {
                return;
            };
            let datas = buffer.datas_mut();
            let Some(data) = datas.first_mut() else {
                return;
            };
            // A stride PipeWire reports as zero or negative means it is not telling us, so the
            // negotiated width is the only thing left to believe.
            let stride = usize::try_from(data.chunk().stride()).unwrap_or(negotiated.stride);
            let stride = if stride == 0 {
                negotiated.stride
            } else {
                stride
            };
            let Some(pixels) = data.data() else {
                // No mapped pointer: this buffer is a DMA-BUF, which needs a GPU import this
                // backend does not do. Dropping it is better than sending a black screen.
                return;
            };
            let Some(frame) = copy_frame(pixels, stride, negotiated, state.started.elapsed())
            else {
                return;
            };
            match state.sender.try_send(frame) {
                Ok(()) | Err(TrySendError::Full(_)) => {}
                Err(TrySendError::Disconnected(_)) => {}
            }
        })
        .register();
    let Ok(_listener) = listener else { return };

    let object = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: spa::param::ParamType::EnumFormat.as_raw(),
        properties: format_properties(),
    };
    let Ok(values) = spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(object),
    ) else {
        return;
    };
    let values = values.0.into_inner();
    let Some(pod) = Pod::from_bytes(&values) else {
        return;
    };
    let mut params = [pod];

    if stream
        .connect(
            spa::utils::Direction::Input,
            Some(node_id),
            pw::stream::StreamFlags::AUTOCONNECT
                | pw::stream::StreamFlags::MAP_BUFFERS
                | pw::stream::StreamFlags::RT_PROCESS,
            &mut params,
        )
        .is_err()
    {
        return;
    }

    // The main loop owns the thread, so the stop flag is checked from a timer inside it rather
    // than from outside.
    let quit = main_loop.clone();
    let stop = Arc::clone(stop);
    let timer = main_loop.loop_().add_timer(move |_| {
        if stop.load(Ordering::Relaxed) {
            quit.quit();
        }
    });
    let _ = timer.update_timer(
        Some(Duration::from_millis(100)),
        Some(Duration::from_millis(100)),
    );
    main_loop.run();
}

struct StreamState {
    sender: SyncSender<CapturedFrame>,
    negotiated: Arc<Mutex<Option<Negotiated>>>,
    started: Instant,
    info: VideoInfoRaw,
}

/// What this backend will accept, as the properties of an `EnumFormat` object.
///
/// Written out by hand because libspa converts an `AudioInfoRaw` into properties for you and a
/// `VideoInfoRaw` not at all — and because what goes in here is a negotiation rather than a
/// statement. Size and framerate are ranges, not numbers: the compositor owns both, and asking for
/// one exact size is how a stream fails to start on a screen that is not that size.
///
/// Two pixel layouts are offered. Both put blue first in four bytes, which is what the codec
/// reads; the difference is only whether the fourth byte means anything, and captured screens are
/// opaque either way. Offering both is the difference between working on most compositors and
/// working on some.
fn format_properties() -> Vec<spa::pod::Property> {
    use spa::pod::{ChoiceValue, Property, Value};
    use spa::utils::{Choice, ChoiceEnum, ChoiceFlags, Id};

    vec![
        Property::new(
            FormatProperties::MediaType.as_raw(),
            Value::Id(Id(spa::sys::SPA_MEDIA_TYPE_video)),
        ),
        Property::new(
            FormatProperties::MediaSubtype.as_raw(),
            Value::Id(Id(spa::sys::SPA_MEDIA_SUBTYPE_raw)),
        ),
        Property::new(
            FormatProperties::VideoFormat.as_raw(),
            Value::Choice(ChoiceValue::Id(Choice(
                ChoiceFlags::empty(),
                ChoiceEnum::Enum {
                    default: Id(VideoFormat::BGRx.as_raw()),
                    alternatives: vec![
                        Id(VideoFormat::BGRx.as_raw()),
                        Id(VideoFormat::BGRA.as_raw()),
                    ],
                },
            ))),
        ),
        Property::new(
            FormatProperties::VideoSize.as_raw(),
            Value::Choice(ChoiceValue::Rectangle(Choice(
                ChoiceFlags::empty(),
                ChoiceEnum::Range {
                    default: Rectangle {
                        width: 1920,
                        height: 1080,
                    },
                    min: Rectangle {
                        width: 1,
                        height: 1,
                    },
                    max: Rectangle {
                        width: 16384,
                        height: 16384,
                    },
                },
            ))),
        ),
        Property::new(
            FormatProperties::VideoFramerate.as_raw(),
            Value::Choice(ChoiceValue::Fraction(Choice(
                ChoiceFlags::empty(),
                ChoiceEnum::Range {
                    default: Fraction { num: 60, denom: 1 },
                    min: Fraction { num: 0, denom: 1 },
                    max: Fraction { num: 240, denom: 1 },
                },
            ))),
        ),
    ]
}

/// Copies one mapped PipeWire buffer into a tightly packed frame.
///
/// Returns `None` rather than a short frame when the buffer is smaller than the negotiated size
/// says it should be: a truncated screen decodes into garbage, and a dropped frame does not.
fn copy_frame(
    pixels: &[u8],
    stride: usize,
    negotiated: Negotiated,
    elapsed: Duration,
) -> Option<CapturedFrame> {
    let row_bytes = negotiated.size.width() as usize * BYTES_PER_PIXEL;
    let height = negotiated.size.height() as usize;
    if stride < row_bytes || pixels.len() < stride.checked_mul(height)? {
        return None;
    }
    let mut tight = Vec::with_capacity(row_bytes * height);
    for row in 0..height {
        let start = row * stride;
        tight.extend_from_slice(&pixels[start..start + row_bytes]);
    }
    Some(CapturedFrame::tight(
        negotiated.size,
        tight,
        // See the module note: PipeWire has the damage, the safe buffer type does not expose it.
        Damage::Unknown,
        elapsed.as_millis() as u64,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn negotiated(width: u32, height: u32) -> Negotiated {
        let size = Size::new(width, height).unwrap();
        Negotiated {
            size,
            stride: size.width() as usize * BYTES_PER_PIXEL,
        }
    }

    #[test]
    fn padded_rows_are_copied_out_without_their_padding() {
        let state = negotiated(2, 2);
        // Four bytes of padding after each two-pixel row, which is what a compositor's stride
        // usually looks like and what a straight memcpy would smear across the picture.
        let pixels = vec![
            1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 0, //
            9, 10, 11, 12, 13, 14, 15, 16, 0, 0, 0, 0,
        ];
        let frame = copy_frame(&pixels, 12, state, Duration::from_millis(7)).unwrap();
        assert_eq!(
            frame.pixels,
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
        );
        assert_eq!(frame.stride, 8);
        assert_eq!(frame.timestamp_ms, 7);
        assert_eq!(frame.damage, Damage::Unknown);
    }

    #[test]
    fn a_short_buffer_is_dropped_rather_than_delivered_truncated() {
        let state = negotiated(4, 4);
        assert!(copy_frame(&[0; 16], 16, state, Duration::ZERO).is_none());
        // A stride narrower than the row is a buffer that cannot hold the frame it claims.
        assert!(copy_frame(&[0; 256], 8, state, Duration::ZERO).is_none());
        // Exactly enough is enough.
        assert!(copy_frame(&[0; 64], 16, state, Duration::ZERO).is_some());
    }

    #[test]
    fn the_offered_format_negotiates_rather_than_demands() {
        use spa::pod::{ChoiceValue, Value};
        use spa::utils::ChoiceEnum;

        let properties = format_properties();
        let of = |key: FormatProperties| {
            properties
                .iter()
                .find(|property| property.key == key.as_raw())
                .map(|property| &property.value)
        };

        // Size and framerate must be ranges. Naming one exact size is how a stream fails to start
        // on a screen that is not that size, which is most of them.
        assert!(matches!(
            of(FormatProperties::VideoSize),
            Some(Value::Choice(ChoiceValue::Rectangle(_)))
        ));
        assert!(matches!(
            of(FormatProperties::VideoFramerate),
            Some(Value::Choice(ChoiceValue::Fraction(_)))
        ));

        // Both four-byte blue-first layouts, or this works on some compositors and not others.
        let Some(Value::Choice(ChoiceValue::Id(choice))) = of(FormatProperties::VideoFormat) else {
            panic!("the pixel format must be a choice");
        };
        let ChoiceEnum::Enum { alternatives, .. } = &choice.1 else {
            panic!("the pixel format must offer alternatives");
        };
        assert!(
            alternatives
                .iter()
                .any(|id| id.0 == VideoFormat::BGRx.as_raw())
        );
        assert!(
            alternatives
                .iter()
                .any(|id| id.0 == VideoFormat::BGRA.as_raw())
        );
    }
}
