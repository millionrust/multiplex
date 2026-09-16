//! Hardware video encoding for the Remote Screens motion path.
//!
//! A screen's motion region — the video someone is watching, the animation a compositor is
//! running — costs far less as an encoded stream than as tiles. What makes it usable over a real
//! network is not the codec but **long-term references**: the viewer says which frames it
//! actually decoded, and when something is lost the encoder predicts the next frame from an older
//! one the viewer still holds, instead of sending a keyframe. Spike 0.3 measured that recovery at
//! about a hundredth of a keyframe (`docs/engineering-evidence/RS4-videotoolbox-ltr.md`).
//!
//! # What comes out
//!
//! Payloads are **Annex B**: NAL units separated by four-byte start codes. VideoToolbox hands
//! back length-prefixed units instead, so this crate converts them. The reason is the far end:
//! Android's MediaCodec wants Annex B parameter sets as `csd-0`, and Apple's decoder is happy to
//! be given parameter sets either way. One format that every target decodes beats the one that is
//! cheapest to produce.
//!
//! [`Encoded::token`] is the acknowledgement token the encoder attached, if any. Match tokens to
//! frames **by token**, never by counting: the encoder drops frames under load, so its output is
//! not one-to-one with what was submitted.
//!
//! Off macOS, and on a Mac whose encoder refuses long-term references, [`HevcEncoder::open`]
//! fails and the caller keeps the tile path. Nothing here is required for a working session.

#![deny(missing_debug_implementations)]

mod error;

#[cfg(target_os = "macos")]
mod ffi;
#[cfg(target_os = "macos")]
mod hevc;
#[cfg(not(target_os = "macos"))]
mod unsupported;

pub use error::VideoError;
#[cfg(target_os = "macos")]
pub use hevc::HevcEncoder;
#[cfg(not(target_os = "macos"))]
pub use unsupported::HevcEncoder;

/// What the encoder should be set up to produce.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EncoderConfig {
    pub width: u32,
    pub height: u32,
    /// What the encoder should expect, which it uses to pace its rate control. Not a promise.
    pub frame_rate: u32,
    /// The target the rate controller aims at, in bits per second.
    pub bitrate: u32,
}

impl EncoderConfig {
    /// A configuration for a region of this size, at the rates the motion path assumes.
    pub const fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            frame_rate: 60,
            bitrate: 8_000_000,
        }
    }

    pub const fn with_bitrate(self, bitrate: u32) -> Self {
        Self { bitrate, ..self }
    }
}

/// One encoded frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Encoded {
    /// NAL units in Annex B form.
    pub payload: Vec<u8>,
    /// Everything after this frame can be decoded without anything before it. Expensive, and on
    /// this path only ever the first frame or one the caller asked for.
    pub keyframe: bool,
    /// The acknowledgement token the encoder attached. Send it with the frame; when the viewer
    /// reports holding it, pass it back in [`Request::acknowledged`].
    pub token: Option<u32>,
}

/// What to tell the encoder about the frame being submitted.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Request<'a> {
    /// Tokens the viewer has confirmed it decoded and still holds. The encoder will only predict
    /// from a reference once it knows the viewer has it.
    pub acknowledged: &'a [u32],
    /// Recover from loss: predict this frame from the newest acknowledged reference rather than
    /// from whatever came immediately before it. This is what a report of loss answers, and it is
    /// not a keyframe.
    pub refresh: bool,
    /// Send a full keyframe. Reserved for a viewer that has nothing at all to predict from, such
    /// as one that just subscribed; ordinary loss must not come here.
    pub keyframe: bool,
}
