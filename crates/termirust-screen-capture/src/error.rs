use std::fmt;

/// Why capture could not start or continue. Messages never include pixels or window titles.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureError {
    /// The operating system refused capture, usually because Screen Recording is not allowed.
    NotPermitted,
    /// No display with the requested id exists.
    DisplayNotFound,
    /// The capture service failed to start or stopped.
    Unavailable,
    /// A delivered frame had a size, stride, or pixel format the codec cannot use.
    InvalidFrame,
    /// The source has no more frames.
    Ended,
}

impl CaptureError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::NotPermitted => "capture_not_permitted",
            Self::DisplayNotFound => "display_not_found",
            Self::Unavailable => "capture_unavailable",
            Self::InvalidFrame => "invalid_frame",
            Self::Ended => "capture_ended",
        }
    }
}

impl fmt::Display for CaptureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for CaptureError {}
