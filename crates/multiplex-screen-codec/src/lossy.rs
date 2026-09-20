//! A cheap first pass for picture tiles. Exact pixels arrive later through refinement.
//!
//! The tile is averaged over 2×2 blocks, each channel keeps `bits` high bits, and the codes are
//! stored left-predicted and deflated. Before deflate a payload is `bits, codes…` with one BGR
//! code triple per block. Decoding restores each block's midpoint colour to all its pixels.

use miniz_oxide::deflate::compress_to_vec;
use miniz_oxide::inflate::decompress_to_vec_with_limit;

use crate::{BYTES_PER_PIXEL, CodecError, Frame, Rect, TILE_SIZE};

const DEFLATE_LEVEL: u8 = 6;

/// Bits kept per channel. More bits mean more detail and more bytes.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct LossyDetail(u8);

impl LossyDetail {
    /// Normal first pass.
    pub const STANDARD: Self = Self(6);
    /// The "lower detail on pictures" degradation step.
    pub const LOW: Self = Self(4);

    /// Accepts 3 to 7 bits per channel.
    pub const fn new(bits: u8) -> Option<Self> {
        if bits >= 3 && bits <= 7 {
            Some(Self(bits))
        } else {
            None
        }
    }

    pub const fn bits(self) -> u8 {
        self.0
    }
}

/// Encodes the pixels of `rect`, at most one tile, as a lossy first pass.
pub fn encode_lossy(frame: &Frame<'_>, rect: Rect, detail: LossyDetail) -> Vec<u8> {
    assert!(
        rect.width <= TILE_SIZE && rect.height <= TILE_SIZE,
        "rect larger than a tile"
    );
    let shift = 8 - detail.bits();
    let blocks_wide = rect.width.div_ceil(2);
    let blocks_high = rect.height.div_ceil(2);
    let mut raw = Vec::with_capacity(1 + (blocks_wide * blocks_high * 3) as usize);
    raw.push(detail.bits());
    for block_y in 0..blocks_high {
        let mut left = [0u8; 3];
        for block_x in 0..blocks_wide {
            let mut sum = [0u32; 3];
            let mut count = 0;
            for dy in 0..2 {
                let y = rect.y + block_y * 2 + dy;
                if y >= rect.bottom() {
                    continue;
                }
                let row = frame.row_span(y, rect);
                for dx in 0..2 {
                    let x = (block_x * 2 + dx) as usize;
                    if x >= rect.width as usize {
                        continue;
                    }
                    let pixel = &row[x * BYTES_PER_PIXEL..x * BYTES_PER_PIXEL + 3];
                    sum.iter_mut()
                        .zip(pixel)
                        .for_each(|(s, p)| *s += u32::from(*p));
                    count += 1;
                }
            }
            let code = sum.map(|channel| ((channel + count / 2) / count) as u8 >> shift);
            raw.extend([
                code[0].wrapping_sub(left[0]),
                code[1].wrapping_sub(left[1]),
                code[2].wrapping_sub(left[2]),
            ]);
            left = code;
        }
    }
    compress_to_vec(&raw, DEFLATE_LEVEL)
}

/// Decodes a lossy payload into tightly packed BGRA rows of `width` × `height`.
pub fn decode_lossy(payload: &[u8], width: u32, height: u32) -> Result<Vec<u8>, CodecError> {
    if width == 0 || height == 0 || width > TILE_SIZE || height > TILE_SIZE {
        return Err(CodecError::InvalidSize);
    }
    let blocks_wide = width.div_ceil(2) as usize;
    let blocks_high = height.div_ceil(2) as usize;
    let expected = 1 + blocks_wide * blocks_high * 3;
    let raw =
        decompress_to_vec_with_limit(payload, expected).map_err(|_| CodecError::CorruptPayload)?;
    if raw.len() != expected {
        return Err(CodecError::CorruptPayload);
    }
    let detail = LossyDetail::new(raw[0]).ok_or(CodecError::CorruptPayload)?;
    let shift = 8 - detail.bits();
    let midpoint = 1u8 << (shift - 1);
    let mut colors = Vec::with_capacity(blocks_wide * blocks_high);
    for row in raw[1..].chunks_exact(blocks_wide * 3) {
        let mut left = [0u8; 3];
        for residual in row.chunks_exact(3) {
            let code = [
                left[0].wrapping_add(residual[0]),
                left[1].wrapping_add(residual[1]),
                left[2].wrapping_add(residual[2]),
            ];
            colors.push(code.map(|c| (c << shift) | midpoint));
            left = code;
        }
    }
    let mut out = Vec::with_capacity(width as usize * height as usize * BYTES_PER_PIXEL);
    for y in 0..height as usize {
        for x in 0..width as usize {
            let [b, g, r] = colors[(y / 2) * blocks_wide + x / 2];
            out.extend([b, g, r, 0xFF]);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FrameBuffer, Size, encode_lossless};
    use proptest::prelude::*;

    fn picture(width: u32, height: u32) -> FrameBuffer {
        let size = Size::new(width, height).unwrap();
        let mut buffer = FrameBuffer::new(size);
        let pixels: Vec<u8> = (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .flat_map(|(x, y)| {
                let noise = ((x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663))
                    .wrapping_mul(2_654_435_761)
                    >> 27) as u8;
                [
                    (x * 3) as u8 + noise,
                    (y * 3) as u8 + noise,
                    ((x + y) * 2) as u8,
                    255,
                ]
            })
            .collect();
        buffer.write_rect(size.bounds(), &pixels).unwrap();
        buffer
    }

    fn psnr(a: &[u8], b: &[u8]) -> f64 {
        let (sum, n) = a
            .chunks_exact(4)
            .zip(b.chunks_exact(4))
            .flat_map(|(p, q)| (0..3).map(move |i| f64::from(p[i]) - f64::from(q[i])))
            .fold((0.0, 0.0), |(s, n), d| (s + d * d, n + 1.0));
        10.0 * (255.0 * 255.0 / (sum / n).max(1e-9)).log10()
    }

    #[test]
    fn first_pass_is_close_and_smaller_than_lossless() {
        let tile = picture(64, 64);
        let rect = Rect::new(0, 0, 64, 64);
        let lossy = encode_lossy(&tile.as_frame(), rect, LossyDetail::STANDARD);
        let lossless = encode_lossless(&tile.as_frame(), rect);
        let decoded = decode_lossy(&lossy, 64, 64).unwrap();
        let mut source = Vec::new();
        tile.as_frame().copy_rect_into(rect, &mut source);
        assert!(
            psnr(&source, &decoded) > 28.0,
            "psnr {}",
            psnr(&source, &decoded)
        );
        assert!(
            lossy.len() * 2 < lossless.len(),
            "lossy {} vs lossless {}",
            lossy.len(),
            lossless.len()
        );
    }

    #[test]
    fn lower_detail_uses_fewer_bytes() {
        let tile = picture(64, 64);
        let rect = Rect::new(0, 0, 64, 64);
        let standard = encode_lossy(&tile.as_frame(), rect, LossyDetail::STANDARD).len();
        let low = encode_lossy(&tile.as_frame(), rect, LossyDetail::LOW).len();
        assert!(low < standard);
    }

    #[test]
    fn detail_is_bounded() {
        assert_eq!(LossyDetail::new(2), None);
        assert_eq!(LossyDetail::new(8), None);
        assert_eq!(LossyDetail::new(5).map(LossyDetail::bits), Some(5));
    }

    #[test]
    fn corrupt_payloads_are_rejected() {
        let tile = picture(10, 10);
        let payload = encode_lossy(
            &tile.as_frame(),
            Rect::new(0, 0, 10, 10),
            LossyDetail::STANDARD,
        );
        assert_eq!(
            decode_lossy(&payload, 10, 12),
            Err(CodecError::CorruptPayload)
        );
        assert_eq!(
            decode_lossy(&compress_to_vec(&[9, 0, 0, 0], 6), 1, 1),
            Err(CodecError::CorruptPayload)
        );
        assert_eq!(decode_lossy(&payload, 0, 1), Err(CodecError::InvalidSize));
    }

    proptest! {
        #[test]
        fn odd_sizes_decode_to_the_right_length(width in 1u32..=64, height in 1u32..=64, bits in 3u8..=7) {
            let tile = picture(width, height);
            let payload = encode_lossy(&tile.as_frame(), Rect::new(0, 0, width, height), LossyDetail::new(bits).unwrap());
            let decoded = decode_lossy(&payload, width, height).unwrap();
            prop_assert_eq!(decoded.len(), (width * height * 4) as usize);
        }

        #[test]
        fn random_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..512), w in 1u32..=64, h in 1u32..=64) {
            let _ = decode_lossy(&bytes, w, h);
        }
    }
}
