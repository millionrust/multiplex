use std::fmt;

use termirust_screen_codec::CodecError;

/// Why a session rejected a message. After any error except [`SessionError::Codec`] on a single
/// batch, the session is closed and the transport should be dropped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionError {
    /// The ticket proof was not accepted, or the device may not view this computer.
    NotAuthorized,
    /// A message arrived that this side never receives, or before the session opened.
    ProtocolViolation,
    /// A message named a surface the host does not share.
    UnknownSurface,
    /// The session was already closed.
    Closed,
    /// A tile batch could not be applied.
    Codec(CodecError),
}

impl SessionError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::NotAuthorized => "not_authorized",
            Self::ProtocolViolation => "protocol_violation",
            Self::UnknownSurface => "unknown_surface",
            Self::Closed => "session_closed",
            Self::Codec(error) => error.code(),
        }
    }
}

impl fmt::Display for SessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for SessionError {}

impl From<CodecError> for SessionError {
    fn from(error: CodecError) -> Self {
        Self::Codec(error)
    }
}
