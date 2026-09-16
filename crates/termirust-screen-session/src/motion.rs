//! Turning the motion stream back into pixels on the viewer.
//!
//! A decoded motion region is drawn straight into the surface's framebuffer, on top of the tiles.
//! That is safe because the host stops claiming to know those pixels the moment it promotes the
//! region, and re-sends every tile in it when it demotes. It also means nothing above this layer
//! has to know the motion path exists: `framebuffer()` returns the whole screen, video included,
//! for the desktop viewer and both phones alike.
//!
//! Where there is no decoder — any platform this build has no hardware decoder for — the frames
//! are handed up instead, so the application can decode them itself. Android does exactly that
//! with MediaCodec.
//!
//! The other half of the job is the acknowledgement loop. A frame that decodes and carries a
//! reference token means the decoder now holds that reference, and the host may predict from it.
//! Nothing else may be acknowledged: a token for a frame that never decoded would tell the host
//! to predict from a picture this viewer does not have.

use std::collections::HashMap;

use termirust_screen_codec::Rect;
use termirust_screen_protocol::{MAX_VIDEO_TOKENS, VideoConfig, VideoFrame};
use termirust_screen_video::{HevcDecoder, Picture, VideoError};

/// What decoding a frame produced.
#[derive(Debug, Default)]
pub(crate) struct Decoded {
    /// Pixels to draw, and where.
    pub picture: Option<(Rect, Picture)>,
    /// References the decoder now holds, when that changed since the last frame.
    pub holding: Option<Vec<u32>>,
    /// This viewer has no decoder, so the frame is the application's to deal with.
    pub undecoded: bool,
}

struct Region {
    rect: Rect,
    decoder: Option<HevcDecoder>,
    /// References the decoder holds, newest last. Bounded by what one message can carry.
    held: Vec<u32>,
}

/// Decoders for the motion regions this viewer is watching.
#[derive(Default)]
pub(crate) struct MotionView {
    regions: HashMap<u32, Region>,
}

impl MotionView {
    /// Opens a decoder for a surface's motion region.
    ///
    /// Replaces any decoder already there: a new configuration is a new encoder, and the old
    /// decoder's references describe a stream that no longer exists.
    pub fn configure(&mut self, config: &VideoConfig) {
        let decoder = match HevcDecoder::open(&config.payload) {
            Ok(decoder) => Some(decoder),
            Err(VideoError::Unsupported) => None,
            Err(_) => {
                // Parameter sets this machine cannot use. The tile path still has the region, so
                // the screen stays correct; it just does not get the cheaper stream.
                None
            }
        };
        self.regions.insert(
            config.surface,
            Region {
                rect: config.rect,
                decoder,
                held: Vec::new(),
            },
        );
    }

    /// Decodes one frame into the region it belongs to.
    pub fn decode(&mut self, frame: &VideoFrame) -> Decoded {
        let Some(region) = self.regions.get_mut(&frame.surface) else {
            // A frame before any configuration. Nothing can decode it, and it is not this
            // viewer's place to guess what stream it belongs to.
            return Decoded {
                undecoded: true,
                ..Decoded::default()
            };
        };
        let Some(decoder) = region.decoder.as_mut() else {
            return Decoded {
                undecoded: true,
                ..Decoded::default()
            };
        };
        let Ok(Some(picture)) = decoder.decode(&frame.payload) else {
            // A frame that could not be decoded, usually because the one it predicts from never
            // arrived. The picture stands still and the repair layer reports the gap.
            return Decoded::default();
        };
        // Only now, having actually decoded it, may this reference be acknowledged.
        let holding = frame.token.and_then(|token| {
            if region.held.contains(&token) {
                return None;
            }
            region.held.push(token);
            while region.held.len() > MAX_VIDEO_TOKENS {
                region.held.remove(0);
            }
            Some(region.held.clone())
        });
        Decoded {
            picture: Some((region.rect, picture)),
            holding,
            undecoded: false,
        }
    }

    /// Forgets a surface's region, because the host demoted it or the session restarted.
    pub fn clear(&mut self, surface: u32) {
        self.regions.remove(&surface);
    }

    /// Forgets every region, because the session ended.
    pub fn clear_all(&mut self) {
        self.regions.clear();
    }

    /// Whether a surface currently has a decoder running. For tests and for the session header.
    pub fn decoding(&self, surface: u32) -> bool {
        self.regions
            .get(&surface)
            .is_some_and(|region| region.decoder.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use termirust_screen_protocol::MotionCodec;

    fn config(payload: Vec<u8>) -> VideoConfig {
        VideoConfig {
            surface: 1,
            codec: MotionCodec::Hevc,
            rect: Rect::new(0, 0, 64, 64),
            payload,
        }
    }

    fn frame(sequence: u64, token: Option<u32>) -> VideoFrame {
        VideoFrame {
            surface: 1,
            sequence,
            keyframe: sequence == 1,
            token,
            payload: vec![0, 0, 0, 1, 0x26, 0x01],
        }
    }

    #[test]
    fn a_frame_before_any_configuration_is_handed_up_rather_than_guessed_at() {
        let mut view = MotionView::default();
        let decoded = view.decode(&frame(1, Some(1)));
        assert!(decoded.undecoded);
        assert!(decoded.picture.is_none());
        assert!(decoded.holding.is_none());
    }

    #[test]
    fn parameter_sets_this_machine_cannot_use_leave_the_tile_path_running() {
        let mut view = MotionView::default();
        view.configure(&config(vec![0, 0, 0, 1, 0xFF, 0xFF]));
        assert!(
            !view.decoding(1),
            "a decoder that would not open must not be claimed"
        );
        // Frames still arrive; they are handed up rather than dropped silently.
        assert!(view.decode(&frame(1, Some(1))).undecoded);
    }

    #[test]
    fn a_new_configuration_replaces_the_decoder_and_its_references() {
        let mut view = MotionView::default();
        view.configure(&config(vec![0, 0, 0, 1, 0xFF]));
        view.regions.get_mut(&1).unwrap().held = vec![1, 2, 3];
        view.configure(&config(vec![0, 0, 0, 1, 0xFE]));
        assert!(
            view.regions[&1].held.is_empty(),
            "references from the old encoder describe a stream that no longer exists"
        );
    }

    #[test]
    fn clearing_forgets_the_region() {
        let mut view = MotionView::default();
        view.configure(&config(vec![0, 0, 0, 1, 0xFF]));
        view.clear(1);
        assert!(!view.decoding(1));
        assert!(view.decode(&frame(1, None)).undecoded);
    }
}
