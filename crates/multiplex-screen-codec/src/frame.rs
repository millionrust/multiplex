use crate::{CodecError, Rect, Size};

/// Bytes per pixel. Pixels are BGRA with 8 bits per channel, the layout ScreenCaptureKit and
/// Desktop Duplication deliver.
pub const BYTES_PER_PIXEL: usize = 4;

/// Borrowed BGRA pixels, rows top to bottom, each row `stride` bytes long.
#[derive(Clone, Copy, Debug)]
pub struct Frame<'a> {
    size: Size,
    stride: usize,
    pixels: &'a [u8],
}

impl<'a> Frame<'a> {
    /// Checks that `stride` covers a row and that `pixels` holds every row.
    pub fn new(size: Size, stride: usize, pixels: &'a [u8]) -> Result<Self, CodecError> {
        let row_bytes = size.width() as usize * BYTES_PER_PIXEL;
        if stride < row_bytes {
            return Err(CodecError::InvalidStride);
        }
        let needed = stride
            .checked_mul(size.height() as usize - 1)
            .and_then(|bytes| bytes.checked_add(row_bytes))
            .ok_or(CodecError::BufferTooShort)?;
        if pixels.len() < needed {
            return Err(CodecError::BufferTooShort);
        }
        Ok(Self {
            size,
            stride,
            pixels,
        })
    }

    pub const fn size(&self) -> Size {
        self.size
    }

    pub const fn stride(&self) -> usize {
        self.stride
    }

    /// The pixels of row `y` without stride padding. Panics when `y` is outside the frame.
    pub fn row(&self, y: u32) -> &'a [u8] {
        assert!(y < self.size.height(), "row outside frame");
        let start = y as usize * self.stride;
        &self.pixels[start..start + self.size.width() as usize * BYTES_PER_PIXEL]
    }

    /// The pixels of `rect` within row `y`. Panics when either is outside the frame.
    pub fn row_span(&self, y: u32, rect: Rect) -> &'a [u8] {
        assert!(self.size.bounds().contains_rect(rect), "rect outside frame");
        let row = self.row(y);
        &row[rect.x as usize * BYTES_PER_PIXEL..rect.right() as usize * BYTES_PER_PIXEL]
    }

    /// Appends the pixels of `rect` to `out` as tightly packed BGRA rows.
    pub fn copy_rect_into(&self, rect: Rect, out: &mut Vec<u8>) {
        for y in rect.y..rect.bottom() {
            out.extend_from_slice(self.row_span(y, rect));
        }
    }
}

/// Owned BGRA pixels with no row padding. The viewer draws from one; the encoder keeps one as
/// its record of the last source frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameBuffer {
    size: Size,
    pixels: Vec<u8>,
}

impl FrameBuffer {
    /// An opaque black surface.
    pub fn new(size: Size) -> Self {
        let mut pixels = vec![0; size.width() as usize * size.height() as usize * BYTES_PER_PIXEL];
        pixels
            .as_chunks_mut::<BYTES_PER_PIXEL>()
            .0
            .iter_mut()
            .for_each(|pixel| pixel[3] = 0xFF);
        Self { size, pixels }
    }

    /// Copies a frame, dropping any stride padding.
    pub fn from_frame(frame: &Frame<'_>) -> Self {
        let mut pixels = Vec::with_capacity(
            frame.size().width() as usize * frame.size().height() as usize * BYTES_PER_PIXEL,
        );
        frame.copy_rect_into(frame.size().bounds(), &mut pixels);
        Self {
            size: frame.size(),
            pixels,
        }
    }

    pub const fn size(&self) -> Size {
        self.size
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub fn as_frame(&self) -> Frame<'_> {
        Frame {
            size: self.size,
            stride: self.size.width() as usize * BYTES_PER_PIXEL,
            pixels: &self.pixels,
        }
    }

    /// Writes tightly packed BGRA rows into `rect`.
    pub fn write_rect(&mut self, rect: Rect, bgra: &[u8]) -> Result<(), CodecError> {
        if rect.is_empty() || !self.size.bounds().contains_rect(rect) {
            return Err(CodecError::RectOutsideSurface);
        }
        let span = rect.width as usize * BYTES_PER_PIXEL;
        if bgra.len() != span * rect.height as usize {
            return Err(CodecError::PayloadSizeMismatch);
        }
        let stride = self.size.width() as usize * BYTES_PER_PIXEL;
        for (row, source) in bgra.chunks_exact(span).enumerate() {
            let start = (rect.y as usize + row) * stride + rect.x as usize * BYTES_PER_PIXEL;
            self.pixels[start..start + span].copy_from_slice(source);
        }
        Ok(())
    }

    /// Fills `rect` with one colour.
    pub fn fill_rect(&mut self, rect: Rect, bgra: [u8; 4]) -> Result<(), CodecError> {
        if rect.is_empty() || !self.size.bounds().contains_rect(rect) {
            return Err(CodecError::RectOutsideSurface);
        }
        let stride = self.size.width() as usize * BYTES_PER_PIXEL;
        for y in rect.y..rect.bottom() {
            let start = y as usize * stride + rect.x as usize * BYTES_PER_PIXEL;
            self.pixels[start..start + rect.width as usize * BYTES_PER_PIXEL]
                .as_chunks_mut::<BYTES_PER_PIXEL>()
                .0
                .iter_mut()
                .for_each(|pixel| *pixel = bgra);
        }
        Ok(())
    }

    /// Makes each row `y` of `rect` a copy of row `y + dy` as it was before the call. Overlapping
    /// source and destination rows are handled.
    pub fn apply_move(&mut self, rect: Rect, dy: i32) -> Result<(), CodecError> {
        let bounds = self.size.bounds();
        let source_top = i64::from(rect.y) + i64::from(dy);
        let source_bottom = i64::from(rect.bottom()) + i64::from(dy);
        if rect.is_empty()
            || dy == 0
            || !bounds.contains_rect(rect)
            || source_top < 0
            || source_bottom > i64::from(bounds.height)
        {
            return Err(CodecError::InvalidMove);
        }
        let stride = self.size.width() as usize * BYTES_PER_PIXEL;
        let left = rect.x as usize * BYTES_PER_PIXEL;
        let span = rect.width as usize * BYTES_PER_PIXEL;
        let copy_row = |pixels: &mut Vec<u8>, y: u32| {
            let destination = y as usize * stride + left;
            let source = (i64::from(y) + i64::from(dy)) as usize * stride + left;
            pixels.copy_within(source..source + span, destination);
        };
        if dy > 0 {
            (rect.y..rect.bottom()).for_each(|y| copy_row(&mut self.pixels, y));
        } else {
            (rect.y..rect.bottom())
                .rev()
                .for_each(|y| copy_row(&mut self.pixels, y));
        }
        Ok(())
    }

    /// Copies a whole frame of the same size into this buffer.
    pub fn copy_from_frame(&mut self, frame: &Frame<'_>) -> Result<(), CodecError> {
        if frame.size() != self.size {
            return Err(CodecError::FrameSizeMismatch);
        }
        let stride = self.size.width() as usize * BYTES_PER_PIXEL;
        for y in 0..self.size.height() {
            let start = y as usize * stride;
            self.pixels[start..start + stride].copy_from_slice(frame.row(y));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_validate_stride_and_length() {
        let size = Size::new(3, 2).unwrap();
        assert_eq!(
            Frame::new(size, 11, &[0; 24]).unwrap_err(),
            CodecError::InvalidStride
        );
        assert_eq!(
            Frame::new(size, 16, &[0; 27]).unwrap_err(),
            CodecError::BufferTooShort
        );
        let padded: Vec<u8> = (0..28).collect();
        let frame = Frame::new(size, 16, &padded).unwrap();
        assert_eq!(frame.row(1), &padded[16..28]);
        let buffer = FrameBuffer::from_frame(&frame);
        assert_eq!(&buffer.pixels()[12..], &padded[16..28]);
        assert_eq!(buffer.as_frame().stride(), 12);
    }

    #[test]
    fn framebuffer_writes_and_fills_inside_bounds_only() {
        let mut buffer = FrameBuffer::new(Size::new(4, 4).unwrap());
        assert!(
            buffer
                .pixels()
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| *p == [0, 0, 0, 255])
        );
        buffer
            .fill_rect(Rect::new(1, 1, 2, 2), [1, 2, 3, 255])
            .unwrap();
        assert_eq!(
            buffer.as_frame().row_span(1, Rect::new(0, 1, 4, 1)),
            &[0, 0, 0, 255, 1, 2, 3, 255, 1, 2, 3, 255, 0, 0, 0, 255]
        );
        assert_eq!(
            buffer.write_rect(Rect::new(3, 3, 2, 1), &[0; 8]),
            Err(CodecError::RectOutsideSurface)
        );
        assert_eq!(
            buffer.write_rect(Rect::new(0, 0, 2, 1), &[0; 4]),
            Err(CodecError::PayloadSizeMismatch)
        );
        buffer
            .write_rect(Rect::new(0, 3, 1, 1), &[9, 9, 9, 255])
            .unwrap();
        assert_eq!(&buffer.pixels()[48..52], &[9, 9, 9, 255]);
    }
}
