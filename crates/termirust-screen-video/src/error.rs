use std::fmt;

/// Why the motion path could not be used. Every one of these is survivable: the caller keeps
/// sending tiles.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VideoError {
    /// This build has no encoder for the platform it is running on.
    Unsupported,
    /// The Mac refused to open a low-latency hardware HEVC session.
    NoEncoder(i32),
    /// The encoder opened but would not offer long-term references, so loss would cost a
    /// keyframe. The motion path is not worth having on those terms.
    NoLongTermReferences,
    /// A frame was submitted and refused.
    Refused(i32),
    /// The region is outside what an encoder session accepts, or does not match the one this
    /// session was opened for.
    InvalidSize { width: u32, height: u32 },
    /// The pixels handed in are not the size their stride and height claim.
    ShortFrame,
}

impl fmt::Display for VideoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => write!(f, "this build has no hardware video encoder"),
            Self::NoEncoder(status) => {
                write!(f, "no low-latency HEVC encoder here (OSStatus {status})")
            }
            Self::NoLongTermReferences => write!(
                f,
                "the encoder does not offer long-term references, so loss would cost a keyframe"
            ),
            Self::Refused(status) => {
                write!(f, "the encoder refused a frame (OSStatus {status})")
            }
            Self::InvalidSize { width, height } => {
                write!(f, "{width}x{height} is not a region this encoder can take")
            }
            Self::ShortFrame => write!(f, "the frame is smaller than its stride and height claim"),
        }
    }
}

impl std::error::Error for VideoError {}
