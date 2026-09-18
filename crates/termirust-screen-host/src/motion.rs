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

use termirust_screen_codec::{Frame, REGION_COST_WINDOW_MS, Rect};
use termirust_screen_protocol::{FeatureSet, MotionCodec, VideoConfig, VideoFrame};
use termirust_screen_session::{HostSession, TicketVerifier};

use crate::parity::{ParityPolicy, parity_for};

/// Assumed gap between captured frames, for turning a frame count into a window.
///
/// The sender is not given a clock — it is called once per captured frame and nothing more — and a
/// rate only has to be good enough to set two paths against each other, so 30 a second is close
/// enough. A wrong assumption here biases both sides of the comparison the same way.
const FRAME_INTERVAL_MS: u64 = 33;

/// What the motion encoder spends on a link nothing is known about yet.
pub const DEFAULT_VIDEO_BITRATE: u32 = 8_000_000;

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
    ///
    /// `bitrate` is what the rate controller currently allows, in bits per second.
    fn open(&self, width: u32, height: u32, bitrate: u32) -> Option<Box<dyn MotionEncoder>>;
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
    /// What the rate controller allows the motion encoder to spend, in bits per second.
    bitrate: u32,
    /// The viewer has acknowledged at least one frame, so the video is really arriving.
    confirmed: bool,
    /// This region was measured and the tile path was cheaper, so it is being left there. Cleared
    /// when the region ends, which is the only honest moment to ask again.
    declined: bool,
    /// Whether the choice between the two paths has been made for this region.
    decided: bool,
    /// Comparisons made for the current region.
    ///
    /// The choice is deliberately not taken at the first opportunity. Both figures are cumulative
    /// averages over the same period, so they are like for like whenever they are read — but at
    /// one second the video average is dominated by the keyframe and the ramp before the encoder
    /// reaches the rate it settles at, and the answer there is not the answer a few seconds later.
    /// RS7 caught exactly that: the 5 Mbps cell kept video that went on to cost 2,582 kbps against
    /// a tile path worth 675, because at one second video still looked like the cheaper of the two.
    compared: u32,
}

/// How many windows pass before the first comparison, so the encoder has settled.
///
/// This is the constant that matters. At one window the 5 Mbps cell kept video that went on to
/// cost 2,582 kbps against a tile path worth 675, because a second is not long enough for the
/// encoder to get past its keyframe and reach the rate it settles at. At two it declines, and the
/// cell costs 703. The extra second is paid as overlap — one more window of sending the rectangle
/// twice — which in a five-second matrix cell reads as the uncapped cell going from 906 to 1,087.
/// That is a fixed cost that amortises over a session of any length; getting the choice wrong does
/// not, because it lasts as long as the region does.
const FIRST_COMPARISON_WINDOWS: u64 = 2;
/// The window the choice is made on and stops being revisited.
///
/// While it is open the tile path is still covering the region, so every extra window is one paid
/// for twice — that overlap is the only way to observe both costs, and it is not free.
///
/// Measured, this changes nothing: every matrix cell that declines does so at the first
/// comparison, and 3 produces a table identical to 4. It is here for the case the matrix does not
/// contain — a region that starts cheap and becomes expensive — and the asymmetry is what picks
/// the larger value. A wrong "keep video" lasts the life of the region; an extra window of overlap
/// is bounded and paid once.
const LAST_COMPARISON_WINDOWS: u64 = 4;

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
    /// What this encoder was opened for, so a changed allowance is noticed.
    bitrate: u32,
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
    /// What this stream has produced, and over how long, so it can be set against what the tile
    /// path is spending on the same rectangle. Only needed until the choice is made.
    video_bytes: u64,
    video_window_ms: u64,
}

impl MotionSender {
    pub fn new(encoders: Box<dyn MotionEncoders>) -> Self {
        Self {
            encoders,
            state: None,
            refresh: false,
            acknowledged: Vec::new(),
            policy: ParityPolicy::default(),
            bitrate: DEFAULT_VIDEO_BITRATE,
            confirmed: false,
            declined: false,
            decided: false,
            compared: 0,
        }
    }

    /// Sets what the motion encoder may spend. Takes effect on the next frame, which reopens the
    /// encoder when the figure actually changed.
    pub const fn set_bitrate(&mut self, bitrate: u32) {
        self.bitrate = bitrate;
    }

    /// The rectangle currently being streamed, if any. For tests and for the session header.
    /// Whether a promoted region was measured and left on the tile path because tiles were
    /// cheaper. Worth reporting: a motion path that quietly declines looks identical to one that
    /// is broken.
    pub const fn declined(&self) -> bool {
        self.declined
    }

    pub fn region(&self) -> Option<Rect> {
        self.state.as_ref().map(|active| active.region)
    }

    /// The viewer holds these references. Anything it does not name may have been lost, so the
    /// encoder must not predict from it.
    pub fn acknowledged(&mut self, tokens: &[u32]) {
        self.acknowledged.clear();
        self.acknowledged.extend_from_slice(tokens);
        // The first acknowledgement is proof the viewer's decoder started. Until one arrives the
        // tile path keeps covering the region, so nothing is ever blank.
        //
        // Never for a region that has been declined. Acknowledgements for frames sent before the
        // decision keep arriving afterwards, and letting them set this again told the tile path to
        // stand down for a stream that no longer exists — nothing sent the region at all, which
        // the matrix caught as a 3.7 second freeze on a link with bandwidth to spare.
        self.confirmed = !self.declined && !self.acknowledged.is_empty();
    }

    /// Whether the viewer has confirmed it is decoding the motion region.
    pub const fn is_confirmed(&self) -> bool {
        self.confirmed
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
        self.confirmed = false;
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
        // Tell the tile path whether it still has to cover the region. This is the whole saving
        // of the motion path: without it the region goes twice, once as video and once as tiles.
        // The tile path keeps covering the region until the choice below has been made, even once
        // the viewer has confirmed the video. It costs about a second of sending the rectangle
        // twice — RS5 puts that at roughly 180 kbps — and it is the only way both paths can be
        // measured under the same conditions: the moment tiles stop, their cost decays to nothing
        // and video would always look like the cheaper one.
        session.set_motion_carried(surface, self.confirmed && self.decided && !self.declined);
        // The region is clipped to the frame: a surface can be resized between the tile encoder
        // choosing a rectangle and this frame arriving.
        let region = session
            .motion_region(surface)
            .map(|region| region.intersect(frame.size().bounds()))
            .filter(|region| !region.is_empty());
        let Some(region) = region else {
            // Demoted, or never promoted. The tile path has the rectangle back.
            self.state = None;
            self.confirmed = false;
            self.declined = false;
            self.decided = false;
            self.compared = 0;
            return;
        };
        // A region already judged not worth encoding stays on the tile path until it ends.
        if self.declined {
            return;
        }
        // A new region needs a new encoder; so does a new bitrate, because the rate controller
        // has decided the old one is spending more than the link has. That costs a keyframe, but
        // rung changes are rare by construction, and a stream at the wrong bitrate costs more.
        if self
            .state
            .as_ref()
            .is_none_or(|active| active.region != region || active.bitrate != self.bitrate)
        {
            let Some(encoder) = self
                .encoders
                .open(region.width, region.height, self.bitrate)
            else {
                self.state = None;
                return;
            };
            self.state = Some(Active {
                region,
                encoder,
                bitrate: self.bitrate,
                sequence: 0,
                sent_config: false,
                first: true,
                group: Vec::new(),
                groups: 0,
                last: Vec::new(),
                video_bytes: 0,
                video_window_ms: 0,
            });
        }
        // Is this region actually worth encoding?
        //
        // A promoted region does not automatically deserve a video stream. The tile path throttles
        // it to `tile_path_max_hz` and sends lossy tiles, and for a modest rectangle of smooth
        // content that can cost far less than encoding it: RS7 measured 713 kbps of tiles against
        // 3,082 kbps of video for the same rectangle on a link with room to spare. RS5 measured the
        // opposite on a larger window of harsher content, which is what the motion path exists for.
        // Both are true, so this is decided by measuring rather than by assuming.
        //
        // Both paths are measured over the same window, which is possible because they overlap:
        // until the viewer acknowledges a video frame the tile path keeps covering the region, so
        // for that period the real cost of each is observable. Comparing against the encoder's
        // configured bitrate instead would be comparing a measurement to a ceiling the encoder
        // rarely reaches, and declined almost everything.
        let due = (FIRST_COMPARISON_WINDOWS + u64::from(self.compared)) * REGION_COST_WINDOW_MS;
        if !self.decided
            && let Some(tiles) = session.motion_region_bytes_per_second(surface)
            && let Some(active) = self.state.as_ref()
            && active.video_window_ms >= due
        {
            let video = active.video_bytes * 1_000 / active.video_window_ms.max(1);
            self.compared = self.compared.saturating_add(1);
            // Asked again each window until the deadline, because the averages only get more
            // representative as the period grows and the first answer is the least trustworthy.
            if FIRST_COMPARISON_WINDOWS + u64::from(self.compared) > LAST_COMPARISON_WINDOWS {
                self.decided = true;
            }
            if tiles < video {
                // Tiles are cheaper. Give the rectangle back and stop paying twice for it.
                //
                // `confirmed` has to go with it: it means "video is arriving", and it is what tells
                // the tile path to stand down. Leaving it set while the encoder is gone left
                // nothing at all sending the region, which the matrix caught as a 3.7 second stall
                // on a link with bandwidth to spare -- the worst possible outcome of a change meant
                // to save bytes.
                self.state = None;
                self.confirmed = false;
                self.declined = true;
                return;
            }
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
        active.video_bytes = active.video_bytes.saturating_add(
            produced
                .iter()
                .map(|frame| frame.payload.len() as u64)
                .sum::<u64>(),
        );
        // Frames arrive at the capture rate, which is what the tile path is measured against too.
        active.video_window_ms = active.video_window_ms.saturating_add(FRAME_INTERVAL_MS);
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
    fn open(&self, _width: u32, _height: u32, _bitrate: u32) -> Option<Box<dyn MotionEncoder>> {
        None
    }
}
