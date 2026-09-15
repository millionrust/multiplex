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
    /// An encoded payload was malformed, truncated, or inconsistent with its tile size.
    CorruptPayload,
    /// A batch was truncated, had a bad header or operation, or had trailing bytes.
    MalformedBatch,
    /// A batch used a wire version this build does not speak.
    UnsupportedVersion,
    /// A batch exceeded [`crate::MAX_BATCH_BYTES`].
    BatchTooLarge,
    /// An operation named a tile outside the surface grid.
    TileOutOfRange,
    /// A move was empty, zero-distance, or read outside the surface.
    InvalidMove,
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
            Self::CorruptPayload => "corrupt_payload",
            Self::MalformedBatch => "malformed_batch",
            Self::UnsupportedVersion => "unsupported_version",
            Self::BatchTooLarge => "batch_too_large",
            Self::TileOutOfRange => "tile_out_of_range",
            Self::InvalidMove => "invalid_move",
        }
    }
}

impl fmt::Display for CodecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for CodecError {}
