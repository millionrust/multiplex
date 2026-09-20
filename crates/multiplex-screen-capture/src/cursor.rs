//! Drawing the mouse pointer into a captured frame.
//!
//! Some capture APIs composite the pointer for you and some hand it over separately. Desktop
//! Duplication is the second kind: the desktop image has no pointer in it at all, and the shape
//! and position arrive as metadata. A viewer with no pointer cannot tell where a click will land,
//! so something has to draw it.
//!
//! This module is deliberately free of platform types. The three shapes Windows can send are a
//! fiddly little format with two genuinely surprising rules in it, and keeping the blending here
//! means it can be tested on every platform rather than only on the one machine that can run it.

use multiplex_screen_codec::{BYTES_PER_PIXEL, Rect, Size};

/// How a pointer's pixels are meant to be combined with the screen under them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CursorKind {
    /// Two stacked 1-bit masks, AND then XOR, each `width` bits per row rounded up to whole bytes.
    /// The buffer is twice as tall as the pointer.
    ///
    /// AND 1 with XOR 0 leaves the screen alone; AND 1 with XOR 1 inverts it. That inversion is
    /// how an I-beam stays visible over both black and white text, and dropping it — treating the
    /// mask as simply opaque or transparent — is why a text cursor disappears against a matching
    /// background.
    Monochrome,
    /// Straight BGRA with an alpha channel.
    Color,
    /// BGRA where alpha is not a blend but a switch: 0 means take these colour bytes, and 0xFF
    /// means exclusive-or them with what is already there.
    MaskedColour,
}

/// One pointer shape, as the operating system described it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CursorShape {
    pub kind: CursorKind,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels of the pointer itself, not of the buffer: a monochrome buffer is twice
    /// this tall.
    pub height: u32,
    /// Bytes per row of `pixels`.
    pub pitch: usize,
    pub pixels: Vec<u8>,
}

impl CursorShape {
    /// Whether `pixels` is big enough for what the other fields claim.
    ///
    /// Worth checking rather than trusting: the shape buffer is sized by the operating system and
    /// read with arithmetic, so a mismatch is an out-of-bounds read rather than a wrong picture.
    pub fn is_consistent(&self) -> bool {
        if self.width == 0 || self.height == 0 || self.pitch == 0 {
            return false;
        }
        let rows = match self.kind {
            CursorKind::Monochrome => self.height as usize * 2,
            CursorKind::Color | CursorKind::MaskedColour => self.height as usize,
        };
        let needed = match self.kind {
            CursorKind::Monochrome => (self.width as usize).div_ceil(8),
            CursorKind::Color | CursorKind::MaskedColour => self.width as usize * BYTES_PER_PIXEL,
        };
        self.pitch >= needed && self.pixels.len() >= self.pitch * rows
    }

    /// Where this pointer covers, clipped to a frame of `size`, with its top-left at `at`.
    pub fn bounds(&self, at: (i32, i32), size: Size) -> Rect {
        let left = at.0.max(0) as u32;
        let top = at.1.max(0) as u32;
        let right =
            at.0.saturating_add(self.width as i32)
                .clamp(0, size.width() as i32) as u32;
        let bottom =
            at.1.saturating_add(self.height as i32)
                .clamp(0, size.height() as i32) as u32;
        if right <= left || bottom <= top {
            return Rect::default();
        }
        Rect::new(left, top, right - left, bottom - top)
    }
}

/// Draws `shape` into `frame` with its top-left at `at`, and reports what it touched.
///
/// `frame` is BGRA with `stride` bytes a row. A pointer partly off any edge is clipped, including
/// off the top and left, which is why the source offsets are computed rather than assumed to start
/// at zero.
pub fn composite(
    frame: &mut [u8],
    stride: usize,
    size: Size,
    shape: &CursorShape,
    at: (i32, i32),
) -> Rect {
    let area = shape.bounds(at, size);
    if area.is_empty() || !shape.is_consistent() {
        return Rect::default();
    }
    // How far into the pointer the visible part starts, for a pointer hanging off the top or left.
    let skip_x = at.0.min(0).unsigned_abs() as usize;
    let skip_y = at.1.min(0).unsigned_abs() as usize;

    for row in 0..area.height as usize {
        let source_y = skip_y + row;
        let target_y = area.y as usize + row;
        for column in 0..area.width as usize {
            let source_x = skip_x + column;
            let target_x = area.x as usize + column;
            let target = target_y * stride + target_x * BYTES_PER_PIXEL;
            if target + BYTES_PER_PIXEL > frame.len() {
                continue;
            }
            match shape.kind {
                CursorKind::Monochrome => {
                    let and = mask_bit(shape, source_x, source_y);
                    let xor = mask_bit(shape, source_x, source_y + shape.height as usize);
                    match (and, xor) {
                        // Transparent: the screen shows through untouched.
                        (true, false) => {}
                        // Inverted: this is what keeps an I-beam visible on any background.
                        (true, true) => {
                            for channel in 0..3 {
                                frame[target + channel] = !frame[target + channel];
                            }
                        }
                        // Opaque black, then opaque white.
                        (false, false) => paint(&mut frame[target..], [0, 0, 0]),
                        (false, true) => paint(&mut frame[target..], [255, 255, 255]),
                    }
                }
                CursorKind::Color => {
                    let source = source_y * shape.pitch + source_x * BYTES_PER_PIXEL;
                    let Some(pixel) = shape.pixels.get(source..source + BYTES_PER_PIXEL) else {
                        continue;
                    };
                    let alpha = u32::from(pixel[3]);
                    if alpha == 0 {
                        continue;
                    }
                    for channel in 0..3 {
                        let over = u32::from(pixel[channel]);
                        let under = u32::from(frame[target + channel]);
                        // Round to nearest rather than truncating, so a fully opaque pixel is
                        // exactly its own colour instead of one short of it.
                        frame[target + channel] =
                            ((over * alpha + under * (255 - alpha) + 127) / 255) as u8;
                    }
                }
                CursorKind::MaskedColour => {
                    let source = source_y * shape.pitch + source_x * BYTES_PER_PIXEL;
                    let Some(pixel) = shape.pixels.get(source..source + BYTES_PER_PIXEL) else {
                        continue;
                    };
                    if pixel[3] == 0 {
                        frame[target..target + 3].copy_from_slice(&pixel[..3]);
                    } else {
                        for channel in 0..3 {
                            frame[target + channel] ^= pixel[channel];
                        }
                    }
                }
            }
        }
    }
    area
}

fn paint(target: &mut [u8], colour: [u8; 3]) {
    target[..3].copy_from_slice(&colour);
}

/// One bit of a 1-bit-per-pixel mask, where the leftmost pixel is the *high* bit of its byte.
fn mask_bit(shape: &CursorShape, x: usize, y: usize) -> bool {
    let index = y * shape.pitch + x / 8;
    shape
        .pixels
        .get(index)
        .is_some_and(|byte| byte & (0x80 >> (x % 8)) != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(size: Size, fill: u8) -> Vec<u8> {
        vec![fill; (size.width() * size.height()) as usize * BYTES_PER_PIXEL]
    }

    fn pixel(frame: &[u8], size: Size, x: u32, y: u32) -> [u8; 4] {
        let stride = size.width() as usize * BYTES_PER_PIXEL;
        let at = y as usize * stride + x as usize * BYTES_PER_PIXEL;
        frame[at..at + 4].try_into().unwrap()
    }

    /// An 8x2 monochrome pointer: column 0 opaque black, column 1 inverting, the rest transparent.
    fn monochrome() -> CursorShape {
        // AND rows first, then XOR rows. Leftmost pixel is the high bit.
        let and = 0b0111_1111;
        let xor = 0b0100_0000;
        CursorShape {
            kind: CursorKind::Monochrome,
            width: 8,
            height: 2,
            pitch: 1,
            pixels: vec![and, and, xor, xor],
        }
    }

    #[test]
    fn a_monochrome_pointer_paints_inverts_and_lets_the_screen_through() {
        let size = Size::new(8, 4).unwrap();
        let mut pixels = frame(size, 200);
        let touched = composite(&mut pixels, 32, size, &monochrome(), (0, 0));
        assert_eq!(touched, Rect::new(0, 0, 8, 2));

        // AND 0, XOR 0: opaque black.
        assert_eq!(pixel(&pixels, size, 0, 0)[..3], [0, 0, 0]);
        // AND 1, XOR 1: inverted. This is the case a naive "mask means transparent" reading gets
        // wrong, and the reason an I-beam stays visible over text of any colour.
        assert_eq!(pixel(&pixels, size, 1, 0)[..3], [55, 55, 55]);
        // AND 1, XOR 0: untouched.
        assert_eq!(pixel(&pixels, size, 2, 0)[..3], [200, 200, 200]);
        // Below the pointer, untouched.
        assert_eq!(pixel(&pixels, size, 0, 2)[..3], [200, 200, 200]);
    }

    #[test]
    fn a_colour_pointer_blends_by_alpha() {
        let size = Size::new(2, 1).unwrap();
        let mut pixels = frame(size, 0);
        let shape = CursorShape {
            kind: CursorKind::Color,
            width: 2,
            height: 1,
            pitch: 8,
            // Fully opaque white, then half-transparent white.
            pixels: vec![255, 255, 255, 255, 255, 255, 255, 128],
        };
        composite(&mut pixels, 8, size, &shape, (0, 0));
        // Opaque must land exactly on its own colour, not one short of it.
        assert_eq!(pixel(&pixels, size, 0, 0)[..3], [255, 255, 255]);
        assert_eq!(pixel(&pixels, size, 1, 0)[..3], [128, 128, 128]);
    }

    #[test]
    fn a_masked_colour_pointer_copies_or_exclusive_ors() {
        let size = Size::new(2, 1).unwrap();
        let mut pixels = frame(size, 0xF0);
        let shape = CursorShape {
            kind: CursorKind::MaskedColour,
            width: 2,
            height: 1,
            pitch: 8,
            // Alpha 0: copy. Alpha 0xFF: exclusive-or.
            pixels: vec![10, 20, 30, 0, 0xFF, 0x0F, 0x00, 0xFF],
        };
        composite(&mut pixels, 8, size, &shape, (0, 0));
        assert_eq!(pixel(&pixels, size, 0, 0)[..3], [10, 20, 30]);
        assert_eq!(pixel(&pixels, size, 1, 0)[..3], [0x0F, 0xFF, 0xF0]);
    }

    #[test]
    fn a_pointer_hanging_off_an_edge_is_clipped_from_the_right_side() {
        let size = Size::new(8, 4).unwrap();
        let shape = monochrome();

        // Off the top-left: the visible part must be the pointer's bottom-right, not its start.
        // Getting this wrong draws the wrong corner rather than crashing, which is why it has its
        // own test rather than being left to the bounds check.
        let mut pixels = frame(size, 200);
        let touched = composite(&mut pixels, 32, size, &shape, (-1, -1));
        assert_eq!(touched, Rect::new(0, 0, 7, 1));
        // Source column 1 is the inverting one, now drawn at column 0.
        assert_eq!(pixel(&pixels, size, 0, 0)[..3], [55, 55, 55]);

        // Entirely off each edge: nothing touched, and no panic.
        for at in [(-8, 0), (8, 0), (0, -2), (0, 4)] {
            let mut pixels = frame(size, 200);
            assert_eq!(
                composite(&mut pixels, 32, size, &shape, at),
                Rect::default()
            );
            assert!(
                pixels.iter().all(|byte| *byte == 200),
                "{at:?} drew something"
            );
        }
    }

    #[test]
    fn a_shape_whose_buffer_is_too_small_is_refused_rather_than_read() {
        let size = Size::new(8, 4).unwrap();
        let mut pixels = frame(size, 200);
        let truncated = CursorShape {
            pixels: vec![0; 2],
            ..monochrome()
        };
        assert!(!truncated.is_consistent());
        assert_eq!(
            composite(&mut pixels, 32, size, &truncated, (0, 0)),
            Rect::default()
        );
        assert!(pixels.iter().all(|byte| *byte == 200));

        // A pitch narrower than one row of pixels is the same kind of lie.
        let narrow = CursorShape {
            pitch: 0,
            ..monochrome()
        };
        assert!(!narrow.is_consistent());
    }
}
