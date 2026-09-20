//! ScreenCaptureKit backend. Requires macOS 13 and the Screen Recording permission.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::time::{Duration, Instant};

use multiplex_screen_codec::{BYTES_PER_PIXEL, Size};
use screencapturekit::cm::SCFrameStatus;
use screencapturekit::prelude::*;

use crate::source::pixel_rects;
use crate::{CaptureConfig, CaptureError, CapturedFrame, Damage, DisplayInfo, FrameSource};

/// Frames waiting for the consumer. A small queue keeps latency low; overflow drops frames and
/// marks the next one as fully damaged.
const QUEUE_DEPTH: usize = 2;

/// Displays that can be captured. Fails with [`CaptureError::NotPermitted`] when Screen
/// Recording has not been allowed.
pub fn displays() -> Result<Vec<DisplayInfo>, CaptureError> {
    let content = SCShareableContent::get().map_err(|_| CaptureError::NotPermitted)?;
    content
        .displays()
        .iter()
        .map(|display| {
            let frame = display.frame();
            Ok(DisplayInfo {
                id: display.display_id(),
                size_points: Size::new(display.width(), display.height())
                    .map_err(|_| CaptureError::InvalidFrame)?,
                origin_points: (frame.origin.x.round() as i32, frame.origin.y.round() as i32),
            })
        })
        .collect()
}

/// Captures one display with ScreenCaptureKit.
pub struct ScreenCaptureKitSource {
    stream: SCStream,
    frames: Receiver<CapturedFrame>,
}

impl ScreenCaptureKitSource {
    pub fn start(config: CaptureConfig) -> Result<Self, CaptureError> {
        let content = SCShareableContent::get().map_err(|_| CaptureError::NotPermitted)?;
        let displays = content.displays();
        let display = displays
            .iter()
            .find(|display| display.display_id() == config.display_id)
            .ok_or(CaptureError::DisplayNotFound)?;
        let points =
            Size::new(display.width(), display.height()).map_err(|_| CaptureError::InvalidFrame)?;
        let size = config.output_size(points)?;

        let filter = SCContentFilter::create()
            .with_display(display)
            .with_excluding_windows(&[])
            .build();
        let stream_config = SCStreamConfiguration::new()
            .with_width(size.width())
            .with_height(size.height())
            .with_pixel_format(PixelFormat::BGRA)
            .with_shows_cursor(config.show_cursor)
            .with_queue_depth(3)
            .with_minimum_frame_interval(&CMTime::new(1, config.max_fps.clamp(1, 120) as i32));

        let (sender, frames) = sync_channel(QUEUE_DEPTH);
        let handler = FrameHandler {
            sender,
            started: Instant::now(),
            dropped: Arc::new(AtomicBool::new(false)),
        };
        let mut stream = SCStream::new(&filter, &stream_config);
        stream.add_output_handler(handler, SCStreamOutputType::Screen);
        stream
            .start_capture()
            .map_err(|_| CaptureError::Unavailable)?;
        Ok(Self { stream, frames })
    }
}

impl FrameSource for ScreenCaptureKitSource {
    fn next_frame(&mut self, timeout: Duration) -> Result<Option<CapturedFrame>, CaptureError> {
        match self.frames.recv_timeout(timeout) {
            Ok(frame) => Ok(Some(frame)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(CaptureError::Ended),
        }
    }
}

impl Drop for ScreenCaptureKitSource {
    fn drop(&mut self) {
        let _ = self.stream.stop_capture();
    }
}

struct FrameHandler {
    sender: SyncSender<CapturedFrame>,
    started: Instant,
    dropped: Arc<AtomicBool>,
}

impl SCStreamOutputTrait for FrameHandler {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, of_type: SCStreamOutputType) {
        if !matches!(of_type, SCStreamOutputType::Screen) {
            return;
        }
        // Idle, blank, and suspended frames carry no new pixels.
        if !matches!(sample.frame_status(), Some(SCFrameStatus::Complete) | None) {
            return;
        }
        let Some(frame) = copy_frame(&sample, self.started) else {
            return;
        };
        let frame = if self.dropped.swap(false, Ordering::Relaxed) {
            CapturedFrame {
                damage: Damage::Unknown,
                ..frame
            }
        } else {
            frame
        };
        match self.sender.try_send(frame) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => self.dropped.store(true, Ordering::Relaxed),
            Err(TrySendError::Disconnected(_)) => {}
        }
    }
}

fn copy_frame(sample: &CMSampleBuffer, started: Instant) -> Option<CapturedFrame> {
    let buffer = sample.pixel_buffer()?;
    let guard = buffer.lock_read_only().ok()?;
    let width = u32::try_from(guard.width()).ok()?;
    let height = u32::try_from(guard.height()).ok()?;
    let size = Size::new(width, height).ok()?;
    let stride = guard.bytes_per_row();
    if stride < width as usize * BYTES_PER_PIXEL {
        return None;
    }
    let length = stride.checked_mul(height as usize)?.min(guard.data_size());
    let base = guard.base_address();
    if base.is_null() || length < stride * (height as usize - 1) + width as usize * BYTES_PER_PIXEL
    {
        return None;
    }
    // SAFETY: the pixel buffer stays locked for reading while `guard` lives, `base` is its
    // non-null base address, and `length` is bounded by the buffer's reported data size.
    #[allow(unsafe_code)]
    let bytes = unsafe { std::slice::from_raw_parts(base, length) };
    let mut pixels = bytes.to_vec();
    // Captured desktops are opaque; force alpha so tile hashes match decoded pixels.
    for row in pixels.chunks_mut(stride) {
        for pixel in row[..width as usize * BYTES_PER_PIXEL].chunks_exact_mut(BYTES_PER_PIXEL) {
            pixel[3] = 0xFF;
        }
    }
    let damage = match sample.dirty_rects() {
        Some(rects) => Damage::Rects(pixel_rects(
            rects.iter().map(|rect| {
                (
                    rect.origin.x,
                    rect.origin.y,
                    rect.size.width,
                    rect.size.height,
                )
            }),
            size,
        )),
        None => Damage::Unknown,
    };
    Some(CapturedFrame {
        size,
        stride,
        pixels,
        damage,
        timestamp_ms: started.elapsed().as_millis() as u64,
        scale: sample.scale_factor().map(|scale| scale as f32),
    })
}

/// Whether this process may capture the screen, without asking for it.
///
/// Worth checking before a session opens rather than after, because a process without the grant
/// does not fail loudly: ScreenCaptureKit hands back frames of a blank desktop, and the person
/// watching sees an empty screen with nothing to explain it.
///
/// macOS records the grant against the **responsible process**, which for the desktop app is the
/// app and for the LaunchAgent is the LaunchAgent itself — it has no responsible parent to
/// inherit from. So the two answer this differently even on a Mac where the app works, which is
/// the whole reason it has to be asked per process rather than assumed.
pub fn screen_capture_allowed() -> bool {
    core_graphics::access::ScreenCaptureAccess.preflight()
}

/// Asks for the grant, which shows the system prompt the first time and opens nothing after
/// that. Returns whether it is now held.
///
/// Only for a process a person is looking at. A background service that called this would
/// prompt from nowhere, so it reports instead and lets the app ask.
pub fn request_screen_capture() -> bool {
    core_graphics::access::ScreenCaptureAccess.request()
}
