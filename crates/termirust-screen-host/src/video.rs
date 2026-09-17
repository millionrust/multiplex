//! The macOS encoder, wrapped in this crate's vocabulary.
//!
//! The wrapping is here rather than in `termirust-screen-video` because the trait is this crate's:
//! the encoder crate knows about HEVC and long-term references, and nothing about sessions.

use termirust_screen_video::{Encoded, EncoderConfig, HevcEncoder, Request};

use crate::motion::{MotionEncoder, MotionEncoders, MotionFrame, MotionRequest};

/// Opens VideoToolbox sessions.
#[derive(Clone, Copy, Debug, Default)]
pub struct VideoToolbox;

impl MotionEncoders for VideoToolbox {
    fn open(&self, width: u32, height: u32, bitrate: u32) -> Option<Box<dyn MotionEncoder>> {
        // An encoder that will not offer long-term references fails to open, so a Mac that cannot
        // recover cheaply from loss keeps the tile path rather than paying a keyframe per loss.
        HevcEncoder::open(EncoderConfig::new(width, height).with_bitrate(bitrate))
            .ok()
            .map(|encoder| Box::new(Session { encoder }) as Box<dyn MotionEncoder>)
    }
}

struct Session {
    encoder: HevcEncoder,
}

impl MotionEncoder for Session {
    fn parameter_sets(&self) -> Vec<u8> {
        self.encoder.parameter_sets()
    }

    fn encode(&mut self, bgra: &[u8], request: MotionRequest<'_>) -> Vec<MotionFrame> {
        let stride = self.encoder.config().width as usize * 4;
        // A refused frame is one dropped frame, not a dead session: the next capture tries again,
        // and the tile path is still carrying the rest of the screen.
        self.encoder
            .encode(
                bgra,
                stride,
                Request {
                    acknowledged: request.acknowledged,
                    refresh: request.refresh,
                    keyframe: request.keyframe,
                },
            )
            .unwrap_or_default()
            .into_iter()
            .map(
                |Encoded {
                     payload,
                     keyframe,
                     token,
                 }| MotionFrame {
                    payload,
                    keyframe,
                    token,
                },
            )
            .collect()
    }
}
