//! Desktop Duplication backend. Requires Windows 8 or later.
//!
//! Windows offers two capture APIs and they are not interchangeable here.
//! Windows.Graphics.Capture is newer, composites the cursor, and can capture a single window — but
//! it hands back a whole texture every frame and never says what changed. Desktop Duplication is
//! older and whole-display only, but it reports dirty and move rectangles, and this codec is built
//! around damage: a frame that cannot say what changed makes the encoder compare every tile on the
//! screen. That is the entire cost model, so damage wins.
//!
//! Two consequences worth knowing before reading the code.
//!
//! **The cursor is not in the picture.** Desktop Duplication excludes it from the desktop image
//! and reports its shape and position separately. This backend does not yet composite it, so
//! `CaptureConfig::show_cursor` has no effect; a viewer sees the screen without a pointer.
//!
//! **Capture is at native pixels and this backend cannot resample.** A `CaptureConfig` asking for
//! any other size is refused when the source starts, rather than quietly delivering a different
//! resolution than the caller asked for. Callers wanting a preview downscale the frame themselves,
//! which is what the host already does for thumbnails.

#![allow(unsafe_code)]

use std::time::{Duration, Instant};

use termirust_screen_codec::{BYTES_PER_PIXEL, Rect, Size};
use windows::Win32::Foundation::{HMODULE, RECT, WAIT_TIMEOUT};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_NOT_FOUND, DXGI_ERROR_WAIT_TIMEOUT,
    DXGI_OUTDUPL_FRAME_INFO, DXGI_OUTDUPL_MOVE_RECT, DXGI_OUTDUPL_POINTER_SHAPE_INFO,
    DXGI_OUTDUPL_POINTER_SHAPE_TYPE_COLOR, DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MASKED_COLOR,
    DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME, DXGI_OUTPUT_DESC, IDXGIAdapter1, IDXGIFactory1,
    IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource,
};
use windows::core::Interface;

use crate::cursor::{CursorKind, CursorShape, composite};
use crate::source::pixel_rects;
use crate::{CaptureConfig, CaptureError, CapturedFrame, Damage, DisplayInfo, FrameSource};

/// A stable id for a display, from the device name Windows gives it (`\\.\DISPLAY1`).
///
/// Windows has no small stable display number to borrow. An index into the enumeration would
/// change the moment a monitor is unplugged, which would silently repoint a saved session at a
/// different screen, so the name is hashed instead. FNV-1a, because this only has to be stable and
/// spread, not unguessable.
fn display_id(device_name: &[u16; 32]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for unit in device_name.iter().take_while(|unit| **unit != 0) {
        for byte in unit.to_le_bytes() {
            hash ^= u32::from(byte);
            hash = hash.wrapping_mul(0x0100_0193);
        }
    }
    // Zero is reserved so a caller can tell "no display" from a real one.
    hash.max(1)
}

fn rect_size(desc: &DXGI_OUTPUT_DESC) -> Result<Size, CaptureError> {
    let bounds = desc.DesktopCoordinates;
    let width = bounds.right.saturating_sub(bounds.left);
    let height = bounds.bottom.saturating_sub(bounds.top);
    Size::new(width.max(0) as u32, height.max(0) as u32).map_err(|_| CaptureError::InvalidFrame)
}

/// Every adapter's every output, paired, because a duplication only works on a device created on
/// the adapter that owns the output. Getting this wrong is how Desktop Duplication fails on
/// laptops with two graphics chips.
fn outputs() -> Result<Vec<(IDXGIAdapter1, IDXGIOutput1, DXGI_OUTPUT_DESC)>, CaptureError> {
    let factory: IDXGIFactory1 =
        unsafe { CreateDXGIFactory1() }.map_err(|_| CaptureError::Unavailable)?;
    let mut found = Vec::new();
    for adapter_index in 0.. {
        let adapter = match unsafe { factory.EnumAdapters1(adapter_index) } {
            Ok(adapter) => adapter,
            Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(_) => return Err(CaptureError::Unavailable),
        };
        for output_index in 0.. {
            let output = match unsafe { adapter.EnumOutputs(output_index) } {
                Ok(output) => output,
                Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(_) => return Err(CaptureError::Unavailable),
            };
            let desc = unsafe { output.GetDesc() }.map_err(|_| CaptureError::Unavailable)?;
            // Not attached to the desktop: there is nothing to duplicate.
            if !desc.AttachedToDesktop.as_bool() {
                continue;
            }
            let output1: IDXGIOutput1 = output.cast().map_err(|_| CaptureError::Unavailable)?;
            found.push((adapter.clone(), output1, desc));
        }
    }
    Ok(found)
}

/// Displays that can be captured.
pub fn displays() -> Result<Vec<DisplayInfo>, CaptureError> {
    outputs()?
        .iter()
        .map(|(_, _, desc)| {
            Ok(DisplayInfo {
                id: display_id(&desc.DeviceName),
                size_points: rect_size(desc)?,
                origin_points: (desc.DesktopCoordinates.left, desc.DesktopCoordinates.top),
            })
        })
        .collect()
}

/// Whether capture is possible at all.
///
/// Desktop Duplication has no permission prompt — unlike macOS there is nothing for a user to
/// grant — so this only reports whether an adapter and an attached output exist.
pub fn screen_capture_allowed() -> bool {
    outputs().is_ok_and(|outputs| !outputs.is_empty())
}

/// Captures one display with Desktop Duplication.
pub struct DesktopDuplicationSource {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    output: IDXGIOutput1,
    duplication: IDXGIOutputDuplication,
    staging: ID3D11Texture2D,
    size: Size,
    started: Instant,
    /// Reused between frames so a 4K desktop is not two allocations per frame.
    ///
    /// Held as `u32` rather than `u8` for its alignment. Windows writes `DXGI_OUTDUPL_MOVE_RECT`
    /// and `RECT` into this buffer, both of which are four-byte aligned, and a `Vec<u8>` only
    /// promises one. In practice an allocator hands back something well aligned and the
    /// difference never shows, which is exactly what makes it worth not relying on.
    metadata: Vec<u32>,
    /// The desktop as Windows drew it, with no pointer on it. Kept between frames so a pointer
    /// that moves over a still screen can be redrawn without waiting for the desktop to change.
    clean: Vec<u8>,
    /// Set when the duplication had to be reopened, because everything on screen may have changed
    /// while it was gone and the rectangles from before it went say nothing about now.
    resynchronised: bool,
    /// Whether the caller asked for a pointer at all.
    show_cursor: bool,
    /// The pointer, which Desktop Duplication sends separately from the desktop image and only
    /// when it changes, so it has to be remembered.
    cursor: Option<CursorShape>,
    cursor_at: (i32, i32),
    cursor_visible: bool,
    /// Where the pointer was drawn last time, so moving it can damage the place it left.
    cursor_was: Rect,
    /// Set when the pointer moved but the desktop did not, so the next frame is worth sending
    /// even though Windows had no new desktop image for us.
    cursor_moved: bool,
}

impl DesktopDuplicationSource {
    pub fn start(config: CaptureConfig) -> Result<Self, CaptureError> {
        let (adapter, output, desc) = outputs()?
            .into_iter()
            .find(|(_, _, desc)| display_id(&desc.DeviceName) == config.display_id)
            .ok_or(CaptureError::DisplayNotFound)?;
        let size = rect_size(&desc)?;
        if config.output_size(size)? != size {
            // Refused rather than quietly delivered at the wrong resolution. See the module note.
            return Err(CaptureError::Unavailable);
        }

        let mut device = None;
        let mut context = None;
        unsafe {
            D3D11CreateDevice(
                &adapter,
                // UNKNOWN is required when an adapter is named, and naming it is what keeps the
                // device on the same chip as the output.
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        }
        .map_err(|_| CaptureError::Unavailable)?;
        let device = device.ok_or(CaptureError::Unavailable)?;
        let context = context.ok_or(CaptureError::Unavailable)?;

        let duplication = unsafe { output.DuplicateOutput(&device) }.map_err(duplication_error)?;
        let staging = staging_texture(&device, size)?;

        Ok(Self {
            device,
            context,
            output,
            duplication,
            staging,
            size,
            started: Instant::now(),
            metadata: Vec::new(),
            clean: Vec::new(),
            resynchronised: false,
            show_cursor: config.show_cursor,
            cursor: None,
            cursor_at: (0, 0),
            cursor_visible: false,
            cursor_was: Rect::default(),
            cursor_moved: false,
        })
    }

    /// The display this source is capturing, in pixels.
    pub const fn size(&self) -> Size {
        self.size
    }

    /// Opens the duplication again after Windows took it away.
    ///
    /// It does that for ordinary reasons — a resolution change, a full-screen game starting, or
    /// the secure desktop coming up for a UAC prompt — so losing it is not an error, it is a thing
    /// that happens during a normal session and has to be recovered from silently. A display that
    /// came back a different size is not recoverable here: the caller has a session built around
    /// the old size and has to start again.
    fn reopen(&mut self) -> Result<(), CaptureError> {
        let desc = unsafe { self.output.GetDesc() }.map_err(|_| CaptureError::Unavailable)?;
        if rect_size(&desc)? != self.size {
            return Err(CaptureError::Unavailable);
        }
        self.duplication =
            unsafe { self.output.DuplicateOutput(&self.device) }.map_err(duplication_error)?;
        self.resynchronised = true;
        Ok(())
    }

    /// One attempt at a frame. `Ok(None)` means nothing usable arrived before `timeout`.
    fn acquire(&mut self, timeout: Duration) -> Result<Option<CapturedFrame>, CaptureError> {
        let millis = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        match unsafe {
            self.duplication
                .AcquireNextFrame(millis, &mut info, &mut resource)
        } {
            Ok(()) => {}
            Err(error) if is_timeout(error.code()) => return Ok(None),
            Err(error) if error.code() == DXGI_ERROR_ACCESS_LOST => {
                self.reopen()?;
                return Ok(None);
            }
            Err(_) => return Err(CaptureError::Unavailable),
        }

        // From here on the frame is held and must be released on every path out, including the
        // error ones, or the next AcquireNextFrame fails and the session stalls for good.
        let taken = self.take_frame(&info, resource.as_ref());
        let released = unsafe { self.duplication.ReleaseFrame() };
        if released.is_err() {
            return Err(CaptureError::Unavailable);
        }
        taken
    }

    fn take_frame(
        &mut self,
        info: &DXGI_OUTDUPL_FRAME_INFO,
        resource: Option<&IDXGIResource>,
    ) -> Result<Option<CapturedFrame>, CaptureError> {
        self.take_pointer(info)?;

        // A frame whose last present time is zero carries no new desktop image: the pointer moved
        // and nothing else. That is worth sending only because the pointer is part of the picture
        // here -- without this the cursor would freeze on a still screen until something else
        // happened to change. The damage is just where it was and where it now is, so a pointer
        // crossing a still desktop costs two small rectangles rather than a screen.
        let desktop_changed = info.LastPresentTime != 0 && resource.is_some();
        if !desktop_changed && !(self.cursor_moved && self.show_cursor) {
            return Ok(None);
        }

        let mut damage = if desktop_changed {
            let resource = resource.expect("checked by desktop_changed");
            let texture: ID3D11Texture2D =
                resource.cast().map_err(|_| CaptureError::InvalidFrame)?;
            let damage = self.damage(info)?;
            unsafe { self.context.CopyResource(&self.staging, &texture) };
            self.read_staging()?;
            damage
        } else {
            // Reusing the desktop we already have, so only the pointer is damaged.
            Damage::Rects(Vec::new())
        };
        if self.clean.is_empty() {
            return Ok(None);
        }

        let mut pixels = self.clean.clone();
        let stride = self.size.width() as usize * BYTES_PER_PIXEL;
        let drawn = self.draw_pointer(&mut pixels, stride);
        // Both ends of the move: the pointer's new home, and the hole it left behind. Reporting
        // only the new one leaves a trail of cursors down the screen.
        if let Damage::Rects(rects) = &mut damage {
            for rect in [self.cursor_was, drawn] {
                if !rect.is_empty() {
                    rects.push(rect);
                }
            }
        }
        self.cursor_was = drawn;
        self.cursor_moved = false;

        Ok(Some(CapturedFrame {
            size: self.size,
            stride,
            pixels,
            damage,
            timestamp_ms: self.started.elapsed().as_millis() as u64,
            scale: None,
        }))
    }

    /// Draws the remembered pointer, and reports where it landed.
    fn draw_pointer(&self, pixels: &mut [u8], stride: usize) -> Rect {
        if !self.show_cursor || !self.cursor_visible {
            return Rect::default();
        }
        let Some(shape) = self.cursor.as_ref() else {
            return Rect::default();
        };
        composite(pixels, stride, self.size, shape, self.cursor_at)
    }

    /// Reads whatever the frame said about the pointer.
    ///
    /// Position and shape arrive independently and only when they change, so both are remembered:
    /// a frame that says nothing about the pointer means it is still where it was, not that it has
    /// gone. `LastMouseUpdateTime` of zero is how Windows says "nothing about the pointer here",
    /// and treating that as "the pointer is at 0,0 and hidden" makes it flicker into the corner.
    fn take_pointer(&mut self, info: &DXGI_OUTDUPL_FRAME_INFO) -> Result<(), CaptureError> {
        if info.PointerShapeBufferSize > 0 {
            let mut buffer = vec![0u8; info.PointerShapeBufferSize as usize];
            let mut shape_info = DXGI_OUTDUPL_POINTER_SHAPE_INFO::default();
            let mut required = 0;
            let read = unsafe {
                self.duplication.GetFramePointerShape(
                    buffer.len() as u32,
                    buffer.as_mut_ptr().cast(),
                    &mut required,
                    &mut shape_info,
                )
            };
            if read.is_ok() {
                buffer.truncate(required as usize);
                let kind = match shape_info.Type {
                    t if t == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME.0 as u32 => {
                        Some(CursorKind::Monochrome)
                    }
                    t if t == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_COLOR.0 as u32 => {
                        Some(CursorKind::Color)
                    }
                    t if t == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MASKED_COLOR.0 as u32 => {
                        Some(CursorKind::MaskedColour)
                    }
                    // An unknown shape kind is dropped rather than guessed at: drawing a pointer
                    // wrong is worse than drawing none.
                    _ => None,
                };
                self.cursor = kind.and_then(|kind| {
                    let height = if kind == CursorKind::Monochrome {
                        shape_info.Height / 2
                    } else {
                        shape_info.Height
                    };
                    let shape = CursorShape {
                        kind,
                        width: shape_info.Width,
                        height,
                        pitch: shape_info.Pitch as usize,
                        pixels: buffer,
                    };
                    shape.is_consistent().then_some(shape)
                });
                self.cursor_moved = true;
            }
        }
        if info.LastMouseUpdateTime != 0 {
            let visible = info.PointerPosition.Visible.as_bool();
            let at = (
                info.PointerPosition.Position.x,
                info.PointerPosition.Position.y,
            );
            if visible != self.cursor_visible || at != self.cursor_at {
                self.cursor_moved = true;
            }
            self.cursor_visible = visible;
            self.cursor_at = at;
        }
        Ok(())
    }

    /// What changed, from the move and dirty rectangles Windows attached to the frame.
    fn damage(&mut self, info: &DXGI_OUTDUPL_FRAME_INFO) -> Result<Damage, CaptureError> {
        if std::mem::take(&mut self.resynchronised) || info.TotalMetadataBufferSize == 0 {
            return Ok(Damage::Unknown);
        }
        let total_bytes = info.TotalMetadataBufferSize as usize;
        self.metadata.clear();
        self.metadata.resize(total_bytes.div_ceil(4), 0);
        let base = self.metadata.as_mut_ptr().cast::<u8>();

        // Move rectangles first: the documented order, and the call reports how many bytes it
        // used so the dirty rectangles can be read into what is left.
        let mut move_bytes = 0;
        unsafe {
            self.duplication.GetFrameMoveRects(
                total_bytes as u32,
                base.cast::<DXGI_OUTDUPL_MOVE_RECT>(),
                &mut move_bytes,
            )
        }
        .map_err(|_| CaptureError::Unavailable)?;
        let move_bytes = (move_bytes as usize).min(total_bytes);

        let mut dirty_bytes = 0;
        unsafe {
            self.duplication.GetFrameDirtyRects(
                (total_bytes - move_bytes) as u32,
                base.add(move_bytes).cast::<RECT>(),
                &mut dirty_bytes,
            )
        }
        .map_err(|_| CaptureError::Unavailable)?;
        let dirty_bytes = (dirty_bytes as usize).min(total_bytes - move_bytes);

        // Both offsets stay four-byte aligned: the buffer is `u32`-aligned and a move rectangle is
        // twenty-four bytes, so the dirty rectangles begin on a multiple of four however many
        // moves came first.
        let moves = unsafe {
            std::slice::from_raw_parts(
                base.cast::<DXGI_OUTDUPL_MOVE_RECT>(),
                move_bytes / size_of::<DXGI_OUTDUPL_MOVE_RECT>(),
            )
        };
        let dirty = unsafe {
            std::slice::from_raw_parts(
                base.add(move_bytes).cast::<RECT>(),
                dirty_bytes / size_of::<RECT>(),
            )
        };
        Ok(Damage::Rects(collect_rects(moves, dirty, self.size)))
    }

    /// Copies the staging texture into `self.clean`, tightly packed.
    ///
    /// The mapped rows are padded to the driver's pitch, which is not the codec's stride, so the
    /// rows are copied one at a time rather than in one block.
    fn read_staging(&mut self) -> Result<(), CaptureError> {
        let row_bytes = self.size.width() as usize * BYTES_PER_PIXEL;
        let height = self.size.height() as usize;
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.context
                .Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
        }
        .map_err(|_| CaptureError::Unavailable)?;

        let pitch = mapped.RowPitch as usize;
        let read = (|| {
            if mapped.pData.is_null() || pitch < row_bytes {
                return Err(CaptureError::InvalidFrame);
            }
            self.clean.clear();
            self.clean.reserve(row_bytes * height);
            for row in 0..height {
                let start = unsafe { mapped.pData.cast::<u8>().add(row * pitch) };
                self.clean
                    .extend_from_slice(unsafe { std::slice::from_raw_parts(start, row_bytes) });
            }
            Ok(())
        })();
        unsafe { self.context.Unmap(&self.staging, 0) };
        read
    }
}

impl FrameSource for DesktopDuplicationSource {
    fn next_frame(&mut self, timeout: Duration) -> Result<Option<CapturedFrame>, CaptureError> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if let Some(frame) = self.acquire(remaining)? {
                return Ok(Some(frame));
            }
            // Pointer-only frames and a reopened duplication both come back empty without having
            // waited, so the deadline is what ends this rather than the number of attempts.
            if Instant::now() >= deadline {
                return Ok(None);
            }
        }
    }
}

/// Move and dirty rectangles as one damage list, clipped to the frame.
///
/// A move names where a block of pixels came from and where it went. The codec has no notion of a
/// move, so both ends are reported as damaged: the destination because it holds new pixels, and
/// the source because whatever is there now arrived some other way. Over-reporting damage costs
/// tile comparisons; under-reporting leaves the viewer looking at pixels that are wrong until
/// something else happens to touch them, so the conservative direction is the only safe one.
fn collect_rects(moves: &[DXGI_OUTDUPL_MOVE_RECT], dirty: &[RECT], size: Size) -> Vec<Rect> {
    let spans = moves
        .iter()
        .flat_map(|moved| {
            let width = moved.DestinationRect.right - moved.DestinationRect.left;
            let height = moved.DestinationRect.bottom - moved.DestinationRect.top;
            [
                (
                    f64::from(moved.DestinationRect.left),
                    f64::from(moved.DestinationRect.top),
                    f64::from(width),
                    f64::from(height),
                ),
                (
                    f64::from(moved.SourcePoint.x),
                    f64::from(moved.SourcePoint.y),
                    f64::from(width),
                    f64::from(height),
                ),
            ]
        })
        .chain(dirty.iter().map(|rect| {
            (
                f64::from(rect.left),
                f64::from(rect.top),
                f64::from(rect.right - rect.left),
                f64::from(rect.bottom - rect.top),
            )
        }));
    pixel_rects(spans, size)
}

fn staging_texture(device: &ID3D11Device, size: Size) -> Result<ID3D11Texture2D, CaptureError> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: size.width(),
        Height: size.height(),
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut texture = None;
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut texture)) }
        .map_err(|_| CaptureError::Unavailable)?;
    texture.ok_or(CaptureError::Unavailable)
}

/// Why a duplication could not be opened.
///
/// `E_ACCESSDENIED` here means the secure desktop is in front — a UAC prompt or the lock screen —
/// which a session recovers from by trying again, not a permission the user can grant. Everything
/// else is reported as unavailable.
fn duplication_error(error: windows::core::Error) -> CaptureError {
    if error.code() == windows::Win32::Foundation::E_ACCESSDENIED {
        CaptureError::NotPermitted
    } else {
        CaptureError::Unavailable
    }
}

fn is_timeout(code: windows::core::HRESULT) -> bool {
    code == DXGI_ERROR_WAIT_TIMEOUT || code == windows::core::HRESULT::from_win32(WAIT_TIMEOUT.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(text: &str) -> [u16; 32] {
        let mut units = [0u16; 32];
        for (slot, unit) in units.iter_mut().zip(text.encode_utf16()) {
            *slot = unit;
        }
        units
    }

    #[test]
    fn display_ids_follow_the_device_name_not_the_order() {
        let first = display_id(&name(r"\\.\DISPLAY1"));
        let second = display_id(&name(r"\\.\DISPLAY2"));
        assert_ne!(first, second);
        assert_eq!(first, display_id(&name(r"\\.\DISPLAY1")));
        // Never zero: a caller has to be able to tell a real display from none.
        assert_ne!(display_id(&name("")), 0);
    }

    #[test]
    fn a_move_damages_both_ends_and_clips_to_the_frame() {
        let size = Size::new(100, 80).unwrap();
        let moves = [DXGI_OUTDUPL_MOVE_RECT {
            SourcePoint: windows::Win32::Foundation::POINT { x: 10, y: 10 },
            DestinationRect: RECT {
                left: 10,
                top: 30,
                right: 40,
                bottom: 50,
            },
        }];
        let dirty = [
            RECT {
                left: 60,
                top: 0,
                right: 200,
                bottom: 10,
            },
            RECT {
                left: 5,
                top: 5,
                right: 5,
                bottom: 9,
            },
        ];
        assert_eq!(
            collect_rects(&moves, &dirty, size),
            vec![
                // The destination, then where those pixels came from.
                Rect::new(10, 30, 30, 20),
                Rect::new(10, 10, 30, 20),
                // Clipped to the frame, and the empty one dropped.
                Rect::new(60, 0, 40, 10),
            ]
        );
    }

    #[test]
    fn no_metadata_at_all_is_not_the_same_as_nothing_changed() {
        // An empty damage list means "nothing changed"; a frame Windows gave no metadata for
        // means "compare everything". Conflating them leaves the screen stale.
        assert_eq!(
            collect_rects(&[], &[], Size::new(8, 8).unwrap()),
            Vec::new()
        );
    }
}
