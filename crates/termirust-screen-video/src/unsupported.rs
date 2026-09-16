//! What the motion path looks like where there is no encoder: absent, and saying so.

use crate::{Encoded, EncoderConfig, Request, VideoError};

/// The stand-in on platforms without a hardware encoder. It cannot be opened, which is how the
/// caller learns to keep sending tiles. Windows and Linux encoders arrive in M6.
#[derive(Debug)]
pub struct HevcEncoder {
    _private: (),
}

impl HevcEncoder {
    pub fn open(_config: EncoderConfig) -> Result<Self, VideoError> {
        Err(VideoError::Unsupported)
    }

    pub fn parameter_sets(&self) -> &[u8] {
        &[]
    }

    pub fn encode(
        &mut self,
        _bgra: &[u8],
        _stride: usize,
        _request: Request<'_>,
    ) -> Result<Vec<Encoded>, VideoError> {
        Err(VideoError::Unsupported)
    }
}
