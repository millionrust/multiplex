//! Exact tile pixels for text and UI.
//!
//! Before deflate a payload is either:
//! - palette: `1, n, n × BGR, (index, run)…` with 1 ≤ n ≤ 64 and runs of 1–255 covering every pixel;
//! - delta: `2, (ΔB, ΔG, ΔR)…` where each pixel is predicted by its left neighbour, or by the pixel
//!   above for the first column. Alpha is always opaque on decode.

use miniz_oxide::deflate::compress_to_vec;
use miniz_oxide::inflate::decompress_to_vec_with_limit;

use crate::{BYTES_PER_PIXEL, CodecError, Frame, MAX_PALETTE_COLORS, Rect, TILE_SIZE};

const MODE_PALETTE: u8 = 1;
const MODE_DELTA: u8 = 2;
const DEFLATE_LEVEL: u8 = 6;

/// Encodes the pixels of `rect`, which must be at most one tile in size.
pub fn encode_lossless(frame: &Frame<'_>, rect: Rect) -> Vec<u8> {
    assert!(
        rect.width <= TILE_SIZE && rect.height <= TILE_SIZE,
        "rect larger than a tile"
    );
    let raw = palette_form(frame, rect).unwrap_or_else(|| delta_form(frame, rect));
    compress_to_vec(&raw, DEFLATE_LEVEL)
}

/// Decodes a payload into tightly packed BGRA rows of `width` × `height`.
pub fn decode_lossless(payload: &[u8], width: u32, height: u32) -> Result<Vec<u8>, CodecError> {
    if width == 0 || height == 0 || width > TILE_SIZE || height > TILE_SIZE {
        return Err(CodecError::InvalidSize);
    }
    let pixels = width as usize * height as usize;
    let limit = 2 + MAX_PALETTE_COLORS * 3 + (pixels * 2).max(pixels * 3);
    let raw =
        decompress_to_vec_with_limit(payload, limit).map_err(|_| CodecError::CorruptPayload)?;
    let (&mode, body) = raw.split_first().ok_or(CodecError::CorruptPayload)?;
    let mut out = Vec::with_capacity(pixels * BYTES_PER_PIXEL);
    match mode {
        MODE_PALETTE => decode_palette(body, pixels, &mut out)?,
        MODE_DELTA => decode_delta(body, width as usize, pixels, &mut out)?,
        _ => return Err(CodecError::CorruptPayload),
    }
    Ok(out)
}

fn palette_form(frame: &Frame<'_>, rect: Rect) -> Option<Vec<u8>> {
    let mut palette: Vec<[u8; 3]> = Vec::with_capacity(MAX_PALETTE_COLORS);
    let mut runs: Vec<u8> = Vec::new();
    let mut current: Option<(u8, u8)> = None;
    for y in rect.y..rect.bottom() {
        for pixel in frame.row_span(y, rect).chunks_exact(BYTES_PER_PIXEL) {
            let color = [pixel[0], pixel[1], pixel[2]];
            let index = match palette.iter().position(|entry| *entry == color) {
                Some(index) => index as u8,
                None if palette.len() < MAX_PALETTE_COLORS => {
                    palette.push(color);
                    (palette.len() - 1) as u8
                }
                None => return None,
            };
            current = match current {
                Some((run_index, run)) if run_index == index && run < u8::MAX => {
                    Some((index, run + 1))
                }
                Some((run_index, run)) => {
                    runs.extend([run_index, run]);
                    Some((index, 1))
                }
                None => Some((index, 1)),
            };
        }
    }
    if let Some((index, run)) = current {
        runs.extend([index, run]);
    }
    let mut raw = Vec::with_capacity(2 + palette.len() * 3 + runs.len());
    raw.extend([MODE_PALETTE, palette.len() as u8]);
    palette.iter().for_each(|color| raw.extend(color));
    raw.extend(runs);
    Some(raw)
}

fn delta_form(frame: &Frame<'_>, rect: Rect) -> Vec<u8> {
    let mut raw = Vec::with_capacity(1 + rect.width as usize * rect.height as usize * 3);
    raw.push(MODE_DELTA);
    let mut above = [0u8; 3];
    for y in rect.y..rect.bottom() {
        let row = frame.row_span(y, rect);
        let first = [row[0], row[1], row[2]];
        let mut left = above;
        for pixel in row.chunks_exact(BYTES_PER_PIXEL) {
            let color = [pixel[0], pixel[1], pixel[2]];
            raw.extend([
                color[0].wrapping_sub(left[0]),
                color[1].wrapping_sub(left[1]),
                color[2].wrapping_sub(left[2]),
            ]);
            left = color;
        }
        above = first;
    }
    raw
}

fn decode_palette(body: &[u8], pixels: usize, out: &mut Vec<u8>) -> Result<(), CodecError> {
    let (&count, rest) = body.split_first().ok_or(CodecError::CorruptPayload)?;
    let count = count as usize;
    if count == 0 || count > MAX_PALETTE_COLORS || rest.len() < count * 3 {
        return Err(CodecError::CorruptPayload);
    }
    let (colors, runs) = rest.split_at(count * 3);
    if runs.len() % 2 != 0 {
        return Err(CodecError::CorruptPayload);
    }
    for run in runs.chunks_exact(2) {
        let (index, length) = (run[0] as usize, run[1] as usize);
        if index >= count
            || length == 0
            || out.len() + length * BYTES_PER_PIXEL > pixels * BYTES_PER_PIXEL
        {
            return Err(CodecError::CorruptPayload);
        }
        let color = &colors[index * 3..index * 3 + 3];
        for _ in 0..length {
            out.extend([color[0], color[1], color[2], 0xFF]);
        }
    }
    if out.len() != pixels * BYTES_PER_PIXEL {
        return Err(CodecError::CorruptPayload);
    }
    Ok(())
}

fn decode_delta(
    body: &[u8],
    width: usize,
    pixels: usize,
    out: &mut Vec<u8>,
) -> Result<(), CodecError> {
    if body.len() != pixels * 3 {
        return Err(CodecError::CorruptPayload);
    }
    let mut above = [0u8; 3];
    for row in body.chunks_exact(width * 3) {
        let mut left = above;
        for (column, residual) in row.chunks_exact(3).enumerate() {
            let color = [
                left[0].wrapping_add(residual[0]),
                left[1].wrapping_add(residual[1]),
                left[2].wrapping_add(residual[2]),
            ];
            if column == 0 {
                above = color;
            }
            out.extend([color[0], color[1], color[2], 0xFF]);
            left = color;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FrameBuffer, Size};
    use proptest::prelude::*;

    fn buffer(width: u32, height: u32, fill: impl Fn(u32, u32) -> [u8; 3]) -> FrameBuffer {
        let size = Size::new(width, height).unwrap();
        let mut buffer = FrameBuffer::new(size);
        let pixels: Vec<u8> = (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .flat_map(|(x, y)| {
                let [b, g, r] = fill(x, y);
                [b, g, r, 255]
            })
            .collect();
        buffer.write_rect(size.bounds(), &pixels).unwrap();
        buffer
    }

    fn roundtrip(buffer: &FrameBuffer, rect: Rect) -> (usize, bool) {
        let payload = encode_lossless(&buffer.as_frame(), rect);
        let decoded = decode_lossless(&payload, rect.width, rect.height).unwrap();
        let mut expected = Vec::new();
        buffer.as_frame().copy_rect_into(rect, &mut expected);
        (payload.len(), decoded == expected)
    }

    #[test]
    fn text_tiles_roundtrip_small() {
        let glyphs = buffer(64, 64, |x, y| {
            if (x / 3 + y / 5) % 4 == 0 {
                [230, 232, 235]
            } else {
                [29, 25, 23]
            }
        });
        let (bytes, exact) = roundtrip(&glyphs, Rect::new(0, 0, 64, 64));
        assert!(exact);
        assert!(bytes < 400, "glyph tile took {bytes} bytes");
    }

    #[test]
    fn many_coloured_tiles_roundtrip_exactly() {
        let gradient = buffer(64, 64, |x, y| {
            [(x * 4) as u8, (y * 4) as u8, ((x ^ y) * 3) as u8]
        });
        assert!(roundtrip(&gradient, Rect::new(0, 0, 64, 64)).1);
        let edge = buffer(70, 70, |x, y| {
            [(x * 7) as u8, (y * 11) as u8, (x * y) as u8]
        });
        assert!(roundtrip(&edge, Rect::new(64, 64, 6, 6)).1);
    }

    #[test]
    fn long_runs_split_at_255() {
        let solid = buffer(64, 64, |_, _| [1, 2, 3]);
        assert!(roundtrip(&solid, Rect::new(0, 0, 64, 64)).1);
    }

    #[test]
    fn corrupt_payloads_are_rejected() {
        let glyphs = buffer(16, 16, |x, _| {
            if x % 2 == 0 {
                [0, 0, 0]
            } else {
                [255, 255, 255]
            }
        });
        let payload = encode_lossless(&glyphs.as_frame(), Rect::new(0, 0, 16, 16));
        assert_eq!(
            decode_lossless(&payload, 16, 15),
            Err(CodecError::CorruptPayload)
        );
        assert_eq!(
            decode_lossless(&payload, 0, 16),
            Err(CodecError::InvalidSize)
        );
        assert_eq!(
            decode_lossless(&payload, 65, 16),
            Err(CodecError::InvalidSize)
        );
        assert_eq!(
            decode_lossless(&compress_to_vec(&[3, 0, 0], 6), 1, 1),
            Err(CodecError::CorruptPayload)
        );
        assert_eq!(
            decode_lossless(&compress_to_vec(&[1, 1, 0, 0, 0, 1, 2], 6), 3, 1),
            Err(CodecError::CorruptPayload)
        );
        assert_eq!(
            decode_lossless(&[0xFF; 10], 4, 4),
            Err(CodecError::CorruptPayload)
        );
    }

    proptest! {
        #[test]
        fn any_tile_roundtrips(width in 1u32..=64, height in 1u32..=64, seed in any::<u32>(), colors in 1u32..300) {
            let tile = buffer(width, height, |x, y| {
                let v = (x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(40_503) ^ seed) % colors;
                [v as u8, (v >> 3) as u8, (v * 7) as u8]
            });
            prop_assert!(roundtrip(&tile, Rect::new(0, 0, width, height)).1);
        }

        #[test]
        fn random_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..512), w in 1u32..=64, h in 1u32..=64) {
            let _ = decode_lossless(&bytes, w, h);
        }
    }
}
