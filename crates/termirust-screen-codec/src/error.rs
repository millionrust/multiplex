use std::fmt;

/// Every way codec input can be rejected. Messages never include pixel or payload bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodecError {
    /// A width or height was zero or above [`crate::MAX_SURFACE_DIMENSION`].
    InvalidSize,
    /// A frame stride was shorter than one row of pixels.
    InvalidStride,
    /// A pixel buffer was shorter than its size and stride require.
    BufferTooShort,
    /// A frame did not match the size of the surface it was compared with.
    FrameSizeMismatch,
    /// A rectangle was empty or reached outside the surface.
    RectOutsideSurface,
    /// A pixel payload did not match the size of its rectangle.
    PayloadSizeMismatch,
}

impl CodecError {
    /// Stable machine-readable code for logs and diagnostics.
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidSize => "invalid_size",
            Self::InvalidStride => "invalid_stride",
            Self::BufferTooShort => "buffer_too_short",
            Self::FrameSizeMismatch => "frame_size_mismatch",
            Self::RectOutsideSurface => "rect_outside_surface",
            Self::PayloadSizeMismatch => "payload_size_mismatch",
        }
    }
}

impl fmt::Display for CodecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for CodecError {}
