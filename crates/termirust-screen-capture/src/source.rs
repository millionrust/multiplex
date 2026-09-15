use std::time::Duration;

use termirust_screen_codec::{BYTES_PER_PIXEL, Frame, Rect, Size};

use crate::CaptureError;

/// A display that can be captured.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DisplayInfo {
    pub id: u32,
    /// Size in points; multiply by the scale reported on frames for pixels.
    pub size_points: Size,
    /// Top-left corner in the global display arrangement, in points.
    pub origin_points: (i32, i32),
}

/// What changed since the previous delivered frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Damage {
    /// The source cannot say; compare every tile.
    Unknown,
    /// Only these rectangles, in frame pixels, changed.
    Rects(Vec<Rect>),
}

/// One captured frame: opaque BGRA rows, `stride` bytes each.
#[derive(Clone, Debug, PartialEq)]
pub struct CapturedFrame {
    pub size: Size,
    pub stride: usize,
    pub pixels: Vec<u8>,
    pub damage: Damage,
    /// Milliseconds since the source started.
    pub timestamp_ms: u64,
    /// Pixels per point, when the source reports it.
    pub scale: Option<f32>,
}

impl CapturedFrame {
    /// An opaque frame with no row padding.
    pub fn tight(size: Size, pixels: Vec<u8>, damage: Damage, timestamp_ms: u64) -> Self {
        Self {
            size,
            stride: size.width() as usize * BYTES_PER_PIXEL,
            pixels,
            damage,
            timestamp_ms,
            scale: None,
        }
    }

    /// A borrowed view for the codec.
    pub fn frame(&self) -> Result<Frame<'_>, CaptureError> {
        Frame::new(self.size, self.stride, &self.pixels).map_err(|_| CaptureError::InvalidFrame)
    }

    /// Damage rectangles for the encoder; `None` means compare every tile.
    pub fn damage_rects(&self) -> Option<&[Rect]> {
        match &self.damage {
            Damage::Unknown => None,
            Damage::Rects(rects) => Some(rects),
        }
    }
}

/// What to capture and how often.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaptureConfig {
    pub display_id: u32,
    /// Output pixels per point. Use the display's backing scale for full detail, or less for
    /// previews.
    pub scale: f32,
    /// Most frames delivered per second. Unchanged frames are never delivered.
    pub max_fps: u32,
    pub show_cursor: bool,
}

impl CaptureConfig {
    pub const fn display(display_id: u32) -> Self {
        Self {
            display_id,
            scale: 1.0,
            max_fps: 60,
            show_cursor: true,
        }
    }

    /// The output size for a display of `points`, at least one pixel on each side.
    pub fn output_size(&self, points: Size) -> Result<Size, CaptureError> {
        let scaled = |side: u32| ((side as f32 * self.scale).round() as u32).max(1);
        Size::new(scaled(points.width()), scaled(points.height()))
            .map_err(|_| CaptureError::InvalidFrame)
    }
}

/// A stream of captured frames.
pub trait FrameSource {
    /// Waits up to `timeout` for the next changed frame. `Ok(None)` means nothing changed in time.
    fn next_frame(&mut self, timeout: Duration) -> Result<Option<CapturedFrame>, CaptureError>;
}

/// Converts operating-system rectangles in `f64` pixels to whole-pixel rectangles inside `size`,
/// rounding outward so no changed pixel is missed. Empty results are dropped.
pub fn pixel_rects(rects: impl IntoIterator<Item = (f64, f64, f64, f64)>, size: Size) -> Vec<Rect> {
    rects
        .into_iter()
        .filter_map(|(x, y, width, height)| {
            if !(x.is_finite() && y.is_finite() && width.is_finite() && height.is_finite()) {
                return None;
            }
            let left = x.floor().max(0.0);
            let top = y.floor().max(0.0);
            let right = (x + width).ceil().min(f64::from(size.width()));
            let bottom = (y + height).ceil().min(f64::from(size.height()));
            (right > left && bottom > top).then(|| {
                Rect::new(
                    left as u32,
                    top as u32,
                    (right - left) as u32,
                    (bottom - top) as u32,
                )
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operating_system_rects_round_outward_and_clip() {
        let size = Size::new(100, 50).unwrap();
        let rects = pixel_rects(
            [
                (10.4, 5.6, 9.2, 1.1),
                (-5.0, -5.0, 10.0, 10.0),
                (95.0, 40.0, 20.0, 20.0),
                (200.0, 0.0, 5.0, 5.0),
                (f64::NAN, 0.0, 1.0, 1.0),
                (3.0, 3.0, 0.0, 4.0),
            ],
            size,
        );
        assert_eq!(
            rects,
            vec![
                Rect::new(10, 5, 10, 2),
                Rect::new(0, 0, 5, 5),
                Rect::new(95, 40, 5, 10)
            ]
        );
    }

    #[test]
    fn output_size_scales_points() {
        let points = Size::new(1512, 982).unwrap();
        let mut config = CaptureConfig::display(1);
        assert_eq!(config.output_size(points).unwrap(), points);
        config.scale = 2.0;
        assert_eq!(
            config.output_size(points).unwrap(),
            Size::new(3024, 1964).unwrap()
        );
        config.scale = 0.2;
        assert_eq!(
            config.output_size(points).unwrap(),
            Size::new(302, 196).unwrap()
        );
    }

    #[test]
    fn frames_expose_codec_views_and_damage() {
        let size = Size::new(2, 1).unwrap();
        let frame = CapturedFrame::tight(
            size,
            vec![0; 8],
            Damage::Rects(vec![Rect::new(0, 0, 1, 1)]),
            5,
        );
        assert_eq!(frame.frame().unwrap().size(), size);
        assert_eq!(frame.damage_rects(), Some(&[Rect::new(0, 0, 1, 1)][..]));
        let broken = CapturedFrame::tight(size, vec![0; 7], Damage::Unknown, 5);
        assert_eq!(broken.frame().unwrap_err(), CaptureError::InvalidFrame);
        assert_eq!(broken.damage_rects(), None);
    }
}
