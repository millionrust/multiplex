//! Driving a video encoder over a surface's motion region.
//!
//! The tile encoder already decides *when* a rectangle is moving fast enough to be worth
//! streaming (`termirust-screen-codec`'s motion tracker) and tells the viewer about it. This
//! module decides *what to send* for that rectangle, once both sides have agreed to the motion
//! path: the decoder configuration, then one encoded frame per capture, then a reference refresh
//! whenever the viewer reports loss.
//!
//! The encoder itself is behind [`MotionEncoder`] so this crate stays free of platform code. The
//! real one is `termirust-screen-video`'s VideoToolbox session; the tests here use a fake.
//!
//! Everything degrades to tiles. No encoder, an encoder that will not open, a viewer that never
//! negotiated video: in each case nothing is sent and the tile path, which is already running,
//! carries the region on its own.

use termirust_screen_codec::{Frame, Rect};
use termirust_screen_protocol::{FeatureSet, MotionCodec, VideoConfig, VideoFrame};
use termirust_screen_session::{HostSession, TicketVerifier};

use crate::parity::{ParityPolicy, parity_for};

/// What the encoder should do with the frame being submitted.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MotionRequest<'a> {
    /// References the viewer has confirmed it holds.
    pub acknowledged: &'a [u32],
    /// Predict from the newest acknowledged reference rather than from the frame before. This is
    /// what loss is answered with.
    pub refresh: bool,
    /// Send something decodable from nothing. Only for a viewer with no references at all.
    pub keyframe: bool,
}

/// One encoded frame, as the encoder produced it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MotionFrame {
    pub payload: Vec<u8>,
    pub keyframe: bool,
    /// The acknowledgement token the encoder attached, if it marked this frame as a candidate
    /// reference. Matched back **by token**: encoders drop frames, so counting does not work.
    pub token: Option<u32>,
}

/// A live video encoder for one region.
pub trait MotionEncoder: Send {
    /// The decoder configuration, once the encoder has produced it. Empty before the first frame.
    fn parameter_sets(&self) -> Vec<u8>;

    /// Encodes one region-sized frame of tightly packed BGRA. May return nothing (the encoder
    /// buffers) or more than one frame.
    fn encode(&mut self, bgra: &[u8], request: MotionRequest<'_>) -> Vec<MotionFrame>;
}

/// Opens encoders. One per region: when the region moves or resizes, the old encoder's references
/// describe pixels that are no longer there, so it is replaced rather than reconfigured.
pub trait MotionEncoders: Send + Sync {
    /// `None` when this machine has no encoder, or none for a region this size. The caller keeps
    /// the tile path, which is always running anyway.
    fn open(&self, width: u32, height: u32) -> Option<Box<dyn MotionEncoder>>;
}

/// Sends a surface's motion region as video, for as long as there is one.
pub struct MotionSender {
    encoders: Box<dyn MotionEncoders>,
    state: Option<Active>,
    /// Set when the viewer reported loss, cleared once a refresh has been asked for.
    refresh: bool,
    /// References the viewer says it holds. Only ever what it told us.
    acknowledged: Vec<u32>,
    /// How much parity the link currently deserves.
    policy: ParityPolicy,
}

impl std::fmt::Debug for MotionSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MotionSender")
            .field("region", &self.state.as_ref().map(|active| active.region))
            .field(
                "sequence",
                &self.state.as_ref().map(|active| active.sequence),
            )
            .finish_non_exhaustive()
    }
}

/// An encoder running over one rectangle.
struct Active {
    region: Rect,
    encoder: Box<dyn MotionEncoder>,
    sequence: u64,
    /// The decoder configuration is only available after the first frame, so it is sent as soon
    /// as the encoder has one, and once only.
    sent_config: bool,
    /// The first frame of a new encoder has nothing to predict from.
    first: bool,
    /// Frames encoded since the last group closed, kept so their parity can be built.
    group: Vec<VideoFrame>,
    /// Groups closed on this encoder, which is what names a parity shard's group.
    groups: u64,
    /// The pixels last handed to the encoder. A region can stop changing long before the tile
    /// encoder demotes it — and while the rest of the screen is silent, demotion can wait a long
    /// time — so an identical region is not encoded again. A viewer holding the last frame of a
    /// frozen picture is already showing the right thing.
    last: Vec<u8>,
}

impl MotionSender {
    pub fn new(encoders: Box<dyn MotionEncoders>) -> Self {
        Self {
            encoders,
            state: None,
            refresh: false,
            acknowledged: Vec::new(),
            policy: ParityPolicy::default(),
        }
    }

    /// The rectangle currently being streamed, if any. For tests and for the session header.
    pub fn region(&self) -> Option<Rect> {
        self.state.as_ref().map(|active| active.region)
    }

    /// The viewer holds these references. Anything it does not name may have been lost, so the
    /// encoder must not predict from it.
    pub fn acknowledged(&mut self, tokens: &[u32]) {
        self.acknowledged.clear();
        self.acknowledged.extend_from_slice(tokens);
    }

    /// The viewer could not rebuild a frame. The next one is predicted from an older reference
    /// the viewer still holds — not a keyframe.
    pub fn lost(&mut self) {
        self.refresh = true;
        self.policy.lost();
    }

    /// Stops streaming, so the next frame starts a fresh encoder. Called when the viewer leaves.
    pub fn stop(&mut self) {
        self.state = None;
        self.refresh = false;
        self.acknowledged.clear();
    }

    /// Encodes and sends the motion region of `surface`, if this session has one and the viewer
    /// negotiated the motion path. Call it right after handing the same frame to the session, so
    /// the region it reports is the one this frame produced.
    pub fn frame<V: TicketVerifier>(
        &mut self,
        session: &mut HostSession<V>,
        surface: u32,
        frame: &Frame<'_>,
    ) {
        if !session.agreed_features().has(FeatureSet::MOTION_VIDEO) {
            self.stop();
            return;
        }
        // The region is clipped to the frame: a surface can be resized between the tile encoder
        // choosing a rectangle and this frame arriving.
        let region = session
            .motion_region(surface)
            .map(|region| region.intersect(frame.size().bounds()))
            .filter(|region| !region.is_empty());
        let Some(region) = region else {
            // Demoted, or never promoted. The tile path has the rectangle back.
            self.state = None;
            return;
        };
        if self
            .state
            .as_ref()
            .is_none_or(|active| active.region != region)
        {
            let Some(encoder) = self.encoders.open(region.width, region.height) else {
                self.state = None;
                return;
            };
            self.state = Some(Active {
                region,
                encoder,
                sequence: 0,
                sent_config: false,
                first: true,
                group: Vec::new(),
                groups: 0,
                last: Vec::new(),
            });
        }
        let Some(active) = self.state.as_mut() else {
            return;
        };

        let mut pixels = Vec::with_capacity(region.width as usize * region.height as usize * 4);
        frame.copy_rect_into(region, &mut pixels);
        // A frozen region needs no frames: the viewer is already showing the last one. Loss is
        // the exception, because the frame it is waiting for never arrived.
        if pixels == active.last && !self.refresh && !active.first {
            return;
        }
        active.last = pixels.clone();
        let produced = active.encoder.encode(
            &pixels,
            MotionRequest {
                acknowledged: &self.acknowledged,
                // A refresh predicts from an older reference, which the very first frame of an
                // encoder does not have; that case wants a keyframe instead.
                refresh: self.refresh && !active.first,
                keyframe: active.first,
            },
        );
        if !produced.is_empty() {
            self.refresh = false;
            active.first = false;
        }
        for encoded in produced {
            if !active.sent_config {
                let sets = active.encoder.parameter_sets();
                if sets.is_empty() {
                    // Without the parameter sets nothing after them can be decoded, so the frame
                    // is dropped rather than sent into a decoder that cannot start.
                    continue;
                }
                active.sent_config = session.send_video_config(VideoConfig {
                    surface,
                    codec: MotionCodec::Hevc,
                    rect: region,
                    payload: sets,
                });
                if !active.sent_config {
                    return;
                }
            }
            active.sequence += 1;
            let sent = VideoFrame {
                surface,
                sequence: active.sequence,
                keyframe: encoded.keyframe,
                token: encoded.token,
                payload: encoded.payload,
            };
            active.group.push(sent.clone());
            session.send_video_frame(sent);
        }
        // Parity closes the group after the frames, not before: the shards repair what has
        // already gone out, and a viewer that lost nothing simply ignores them.
        while self
            .state
            .as_ref()
            .is_some_and(|active| active.group.len() >= self.policy.group())
        {
            self.close_group(session, surface);
        }
    }

    /// Sends the parity for a full group, and moves the ratio to whatever the link is doing.
    fn close_group<V: TicketVerifier>(&mut self, session: &mut HostSession<V>, surface: u32) {
        let Some(active) = self.state.as_mut() else {
            return;
        };
        let taken = self.policy.group().min(active.group.len());
        let frames: Vec<VideoFrame> = active.group.drain(..taken).collect();
        let group = active.groups;
        active.groups += 1;
        let shards = self.policy.close_group();
        if !session.agreed_features().has(FeatureSet::VIDEO_PARITY) {
            return;
        }
        // A group that cannot be covered sends nothing; the reference path still covers it.
        for parity in parity_for(surface, group, &frames, shards).unwrap_or_default() {
            session.send_parity(parity);
        }
    }
}

/// The encoders this build has. On macOS that is VideoToolbox; elsewhere there are none yet, and
/// every session keeps the tile path (M6).
pub fn platform_encoders() -> Box<dyn MotionEncoders> {
    #[cfg(target_os = "macos")]
    {
        Box::new(crate::video::VideoToolbox)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Box::new(NoEncoders)
    }
}

/// Used where no encoder exists, and by tests that want the tile path.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoEncoders;

impl MotionEncoders for NoEncoders {
    fn open(&self, _width: u32, _height: u32) -> Option<Box<dyn MotionEncoder>> {
        None
    }
}
