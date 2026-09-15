use termirust_screen_codec::{BYTES_PER_PIXEL, Frame, FrameBuffer, Size};

/// Averages `factor` × `factor` blocks, for live previews on the Devices list. Partial blocks on
/// the right and bottom edges average only the pixels they contain.
pub fn downscale(frame: &Frame<'_>, factor: u32) -> FrameBuffer {
    let factor = factor.max(1);
    let source = frame.size();
    let size = Size::new(
        source.width().div_ceil(factor),
        source.height().div_ceil(factor),
    )
    .expect("a downscaled size is never larger than the source");
    let mut pixels =
        Vec::with_capacity(size.width() as usize * size.height() as usize * BYTES_PER_PIXEL);
    for block_y in 0..size.height() {
        let top = block_y * factor;
        let bottom = (top + factor).min(source.height());
        for block_x in 0..size.width() {
            let left = (block_x * factor) as usize;
            let right = ((block_x + 1) * factor).min(source.width()) as usize;
            let mut sum = [0u32; 3];
            let mut count = 0;
            for y in top..bottom {
                let row = frame.row(y);
                for pixel in row[left * BYTES_PER_PIXEL..right * BYTES_PER_PIXEL]
                    .chunks_exact(BYTES_PER_PIXEL)
                {
                    sum.iter_mut()
                        .zip(pixel)
                        .for_each(|(s, p)| *s += u32::from(*p));
                    count += 1;
                }
            }
            let [b, g, r] = sum.map(|channel| ((channel + count / 2) / count) as u8);
            pixels.extend([b, g, r, 0xFF]);
        }
    }
    let mut buffer = FrameBuffer::new(size);
    buffer
        .write_rect(size.bounds(), &pixels)
        .expect("pixels match the downscaled size");
    buffer
}

#[cfg(test)]
mod tests {
    use super::*;
    use termirust_screen_codec::Rect;

    #[test]
    fn averages_blocks_including_partial_edges() {
        let size = Size::new(5, 3).unwrap();
        let mut buffer = FrameBuffer::new(size);
        buffer
            .fill_rect(Rect::new(0, 0, 2, 2), [200, 100, 0, 255])
            .unwrap();
        buffer
            .fill_rect(Rect::new(0, 0, 1, 1), [0, 0, 0, 255])
            .unwrap();
        buffer
            .fill_rect(Rect::new(4, 0, 1, 3), [9, 9, 9, 255])
            .unwrap();
        let small = downscale(&buffer.as_frame(), 2);
        assert_eq!(small.size(), Size::new(3, 2).unwrap());
        let row0 = small.as_frame().row(0);
        assert_eq!(&row0[0..4], &[150, 75, 0, 255]);
        assert_eq!(&row0[8..12], &[9, 9, 9, 255]);
        assert_eq!(&small.as_frame().row(1)[8..12], &[9, 9, 9, 255]);
    }

    #[test]
    fn factor_one_is_a_copy() {
        let size = Size::new(3, 2).unwrap();
        let mut buffer = FrameBuffer::new(size);
        buffer
            .fill_rect(Rect::new(1, 1, 1, 1), [1, 2, 3, 255])
            .unwrap();
        assert_eq!(downscale(&buffer.as_frame(), 1), buffer);
        assert_eq!(downscale(&buffer.as_frame(), 0), buffer);
    }
}
