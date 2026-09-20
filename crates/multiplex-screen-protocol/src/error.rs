use std::fmt;

/// Why a frame or message was rejected. Messages never include payload bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolError {
    /// A frame's declared length exceeded the bound for its kind.
    FrameTooLarge,
    /// A frame was empty, truncated, had an unknown kind, a bad field, or trailing bytes.
    Malformed,
    /// The peer speaks another protocol version.
    UnsupportedVersion,
    /// A contained tile batch failed to parse.
    InvalidBatch,
}

impl ProtocolError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::FrameTooLarge => "frame_too_large",
            Self::Malformed => "malformed_message",
            Self::UnsupportedVersion => "unsupported_version",
            Self::InvalidBatch => "invalid_batch",
        }
    }
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for ProtocolError {}
