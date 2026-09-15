use std::fmt;

/// Every way codec input can be rejected. Messages never include pixel or payload bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodecError {
    /// A width or height was zero or above [`crate::MAX_SURFACE_DIMENSION`].
    InvalidSize,
}

impl CodecError {
    /// Stable machine-readable code for logs and diagnostics.
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidSize => "invalid_size",
        }
    }
}

impl fmt::Display for CodecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for CodecError {}
