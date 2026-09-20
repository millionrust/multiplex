//! The motion path, from a rectangle that starts moving to the video a viewer receives.
//!
//! The encoder here is a fake, because what is being tested is the protocol around it: that a
//! region is only streamed once both sides agreed to it, that the decoder configuration goes out
//! before the first frame, that loss is answered with a reference refresh rather than a keyframe,
//! and that demotion stops the stream. The real encoder is tested against real hardware in
//! `multiplex-screen-video`.

use std::sync::{Arc, Mutex};

use multiplex_screen_codec::{FrameBuffer, Rect, Size};
use multiplex_screen_host::{
    MotionEncoder, MotionEncoders, MotionFrame, MotionRequest, MotionSender, NoEncoders,
};
use multiplex_screen_protocol::{FeatureSet, MotionCodec, Profile};
use multiplex_screen_session::{
    Grants, HostConfig, HostEvent, HostSession, ResumeStore, TicketVerifier, ViewerEvent,
    ViewerSession,
};

const TICKET: [u8; 32] = [7; 32];
const SURFACE: u32 = 1;
/// Four tiles across and three down, which clears the tracker's twelve-tile floor.
const REGION: Rect = Rect {
    x: 128,
    y: 64,
    width: 256,
    height: 192,
};

struct Tickets;

impl TicketVerifier for Tickets {
    fn verify(&mut self, proof: &[u8; 32]) -> Option<Grants> {
        (*proof == TICKET).then_some(Grants {
            device: 1,
            can_view: true,
            can_control: false,
        })
    }
}

fn size() -> Size {
    Size::new(640, 400).unwrap()
}

/// What the fake encoder was asked to do, so the test can read the loop back.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Asked {
    acknowledged: Vec<u32>,
    refresh: bool,
    keyframe: bool,
}

#[derive(Clone, Debug, Default)]
struct Recorder {
    opened: Vec<(u32, u32)>,
    asked: Vec<Asked>,
}

/// An encoder that answers every frame, so the test is about the protocol rather than about
/// VideoToolbox's buffering.
struct Fake {
    log: Arc<Mutex<Recorder>>,
    token: u32,
    /// What this encoder was opened for. A region grows as more of the window warms up, so the
    /// size is whatever the sender asked for, not a constant.
    width: u32,
    height: u32,
}

impl MotionEncoder for Fake {
    fn parameter_sets(&self) -> Vec<u8> {
        vec![0, 0, 0, 1, 0x40, 0x01]
    }

    fn encode(&mut self, bgra: &[u8], request: MotionRequest<'_>) -> Vec<MotionFrame> {
        assert_eq!(
            bgra.len(),
            self.width as usize * self.height as usize * 4,
            "the encoder is handed exactly the region it was opened for, tightly packed"
        );
        self.log.lock().unwrap().asked.push(Asked {
            acknowledged: request.acknowledged.to_vec(),
            refresh: request.refresh,
            keyframe: request.keyframe,
        });
        self.token += 1;
        vec![MotionFrame {
            payload: vec![0, 0, 0, 1, 0x26, self.token as u8],
            keyframe: request.keyframe,
            token: Some(self.token),
        }]
    }
}

struct Fakes(Arc<Mutex<Recorder>>);

impl MotionEncoders for Fakes {
    fn open(&self, width: u32, height: u32, _bitrate: u32) -> Option<Box<dyn MotionEncoder>> {
        self.0.lock().unwrap().opened.push((width, height));
        Some(Box::new(Fake {
            log: Arc::clone(&self.0),
            token: 0,
            width,
            height,
        }))
    }
}

/// The variant name of a message, so a test can say which classing it expected.
fn message_name(message: &multiplex_screen_protocol::Message) -> String {
    format!("{message:?}")
        .chars()
        .take_while(|character| character.is_alphanumeric())
        .collect()
}

/// The sequence of a video frame message, or zero for anything else. Zero is never a real
/// sequence, so it can stand for "not a video frame".
fn video_sequence(message: &multiplex_screen_protocol::Message) -> u64 {
    match message {
        multiplex_screen_protocol::Message::VideoFrame(frame) => frame.sequence,
        _ => 0,
    }
}

/// A screen with a rectangle of noise that changes every frame, and a still background.
fn screen(step: u32, moving: bool) -> FrameBuffer {
    let mut buffer = FrameBuffer::new(size());
    buffer
        .fill_rect(size().bounds(), [236, 232, 228, 255])
        .unwrap();
    let shade = if moving { (step * 37 % 200) as u8 } else { 40 };
    for row in 0..REGION.height / 8 {
        let y = REGION.y + row * 8;
        let width = if moving {
            (row * 29 + step * 17) % REGION.width + 8
        } else {
            REGION.width
        };
        buffer
            .fill_rect(
                Rect::new(REGION.x, y, width.min(REGION.width), 8),
                [shade, shade.wrapping_add(40), shade.wrapping_add(80), 255],
            )
            .unwrap();
    }
    // A clock in the corner, well away from the region and far too small to be promoted itself.
    // Something on a real desktop always ticks, and without it the tile path would go completely
    // silent and stop acknowledging, which is not what demotion should be tested against.
    let tick = (step % 18) * 2;
    buffer
        .fill_rect(Rect::new(560, 340, 40, 20), [250, 250, 250, 255])
        .unwrap();
    buffer
        .fill_rect(Rect::new(560 + tick, 340, 4, 20), [20, 20, 20, 255])
        .unwrap();
    buffer
}

struct Link {
    host: HostSession<Tickets>,
    viewer: ViewerSession,
    store: ResumeStore,
    sender: MotionSender,
    events: Vec<ViewerEvent>,
    now_ms: u64,
    /// Video frame sequences the link swallows, so a repair has something to repair.
    drop_video_frames: Vec<u64>,
    dropped: usize,
    /// Parity shards the host put on the wire.
    parity_sent: usize,
    /// The class of every message the host sent, and what it was.
    host_classes: Vec<multiplex_screen_protocol::Class>,
    host_message_classes: Vec<(multiplex_screen_protocol::Class, String)>,
}

impl Link {
    /// A host and viewer that both advertise everything, already open and subscribed.
    fn open(encoders: Box<dyn MotionEncoders>, viewer_features: FeatureSet) -> Self {
        let mut link = Self {
            host: HostSession::new(
                vec![multiplex_screen_protocol::SurfaceInfo {
                    id: SURFACE,
                    size: size(),
                    scale_milli: 2000,
                    name: "Built-in Display".to_owned(),
                }],
                Tickets,
                HostConfig {
                    features: FeatureSet::from_bits(FeatureSet::KNOWN),
                    ..HostConfig::default()
                },
            ),
            viewer: ViewerSession::with_features(64 << 20, viewer_features),
            store: ResumeStore::default(),
            sender: MotionSender::new(encoders),
            events: Vec::new(),
            now_ms: 0,
            drop_video_frames: Vec::new(),
            dropped: 0,
            parity_sent: 0,
            host_classes: Vec::new(),
            host_message_classes: Vec::new(),
        };
        link.viewer.connect(TICKET);
        link.viewer.subscribe(SURFACE, Profile::Interactive);
        link.pump();
        link
    }

    fn pump(&mut self) {
        loop {
            let mut moved = false;
            while let Some(message) = self.viewer.poll_outgoing() {
                moved = true;
                for event in self
                    .host
                    .receive(message, &mut self.store)
                    .expect("the host accepts the viewer")
                {
                    match event {
                        HostEvent::VideoAcknowledged { tokens, .. } => {
                            self.sender.acknowledged(&tokens)
                        }
                        HostEvent::VideoLost { .. } => self.sender.lost(),
                        _ => {}
                    }
                }
            }
            while let Some(message) = self.host.poll_outgoing() {
                moved = true;
                if matches!(message, multiplex_screen_protocol::Message::Parity(_)) {
                    self.parity_sent += 1;
                }
                let class = message.class();
                self.host_classes.push(class);
                self.host_message_classes
                    .push((class, message_name(&message)));
                if self.drop_video_frames.contains(&video_sequence(&message)) {
                    // The link ate it. Nothing tells the viewer directly; it finds out from the
                    // gap, and repairs it from the group's parity.
                    self.dropped += 1;
                    continue;
                }
                self.events
                    .extend(self.viewer.receive(message).expect("the viewer accepts"));
            }
            if !moved {
                return;
            }
        }
    }

    /// Shows one frame, exactly as `ScreenHostHandle::frame` does: tiles first, then video.
    fn show(&mut self, step: u32, moving: bool) {
        let buffer = screen(step, moving);
        let frame = buffer.as_frame();
        self.host
            .frame(SURFACE, &frame, None, self.now_ms)
            .expect("the tile encoder takes the frame");
        self.sender.frame(&mut self.host, SURFACE, &frame);
        self.now_ms += 40;
        self.pump();
    }

    /// Frames until the tile encoder promotes the region, or a bound is reached.
    fn until_promoted(&mut self) {
        for step in 0..40 {
            self.show(step, true);
            if self.sender.region().is_some() {
                return;
            }
        }
        panic!("the region was never promoted");
    }

    fn video(&self) -> Vec<&multiplex_screen_protocol::VideoFrame> {
        self.events
            .iter()
            .filter_map(|event| match event {
                ViewerEvent::VideoFrame(frame) => Some(frame),
                _ => None,
            })
            .collect()
    }
}

fn everything() -> FeatureSet {
    FeatureSet::from_bits(FeatureSet::KNOWN)
}

#[test]
fn a_promoted_region_is_streamed_with_its_decoder_configuration_first() {
    let log = Arc::new(Mutex::new(Recorder::default()));
    let mut link = Link::open(Box::new(Fakes(Arc::clone(&log))), everything());
    link.until_promoted();

    assert_eq!(
        log.lock().unwrap().opened,
        vec![(REGION.width, REGION.height)],
        "one encoder, opened for the region the tile encoder chose"
    );
    let configs: Vec<_> = link
        .events
        .iter()
        .filter_map(|event| match event {
            ViewerEvent::VideoConfig(config) => Some(config),
            _ => None,
        })
        .collect();
    assert_eq!(configs.len(), 1, "the configuration is sent once");
    assert_eq!(configs[0].codec, MotionCodec::Hevc);
    assert_eq!(configs[0].rect, REGION);
    assert!(!configs[0].payload.is_empty());

    // The configuration reaches the viewer before anything that needs it.
    let config_at = link
        .events
        .iter()
        .position(|event| matches!(event, ViewerEvent::VideoConfig(_)));
    let frame_at = link
        .events
        .iter()
        .position(|event| matches!(event, ViewerEvent::VideoFrame(_)));
    assert!(config_at < frame_at, "a decoder cannot start without it");

    let video = link.video();
    assert!(video[0].keyframe, "a new stream opens with a keyframe");
    assert_eq!(video[0].sequence, 1);
    assert!(
        video
            .windows(2)
            .all(|pair| pair[1].sequence > pair[0].sequence),
        "sequences only go up"
    );
    assert!(
        video[1..].iter().all(|frame| !frame.keyframe),
        "nothing after the first frame needs to be a keyframe"
    );
}

#[test]
fn a_viewer_that_never_negotiated_video_gets_tiles_only() {
    let log = Arc::new(Mutex::new(Recorder::default()));
    let mut link = Link::open(Box::new(Fakes(Arc::clone(&log))), FeatureSet::none());
    for step in 0..40 {
        link.show(step, true);
    }
    assert!(link.sender.region().is_none());
    assert!(
        log.lock().unwrap().opened.is_empty(),
        "no encoder was opened"
    );
    assert!(link.video().is_empty());
    assert!(
        link.events
            .iter()
            .any(|event| matches!(event, ViewerEvent::MotionRegion { rect: Some(_), .. })),
        "the region was still promoted; only the video is missing"
    );
}

#[test]
fn a_machine_without_an_encoder_keeps_the_tile_path() {
    let mut link = Link::open(Box::new(NoEncoders), everything());
    for step in 0..40 {
        link.show(step, true);
    }
    assert!(link.sender.region().is_none());
    assert!(link.video().is_empty());
}

#[test]
fn loss_is_answered_with_a_reference_refresh_and_not_a_keyframe() {
    let log = Arc::new(Mutex::new(Recorder::default()));
    let mut link = Link::open(Box::new(Fakes(Arc::clone(&log))), everything());
    link.until_promoted();

    // The viewer confirms what it holds, then reports one frame it could not rebuild.
    let held: Vec<u32> = link
        .video()
        .iter()
        .filter_map(|frame| frame.token)
        .collect();
    let lost = link.video().last().expect("a frame to lose").sequence;
    link.viewer
        .report_video(SURFACE, &held, Some(lost))
        .expect("a negotiated viewer may report");
    link.pump();
    let before = link.video().len();
    link.show(100, true);

    let asked = log.lock().unwrap().asked.clone();
    let recovery = asked.last().expect("a frame was submitted");
    assert!(recovery.refresh, "loss recovers from an older reference");
    assert!(
        !recovery.keyframe,
        "a keyframe is the cost the motion path exists to avoid"
    );
    assert_eq!(
        recovery.acknowledged, held,
        "the encoder may only predict from what the viewer said it holds"
    );
    let after = link.video();
    assert!(after.len() > before, "a recovery frame was sent");
    assert!(!after[before].keyframe);

    // The refresh is asked for once, not on every frame after the report.
    link.show(101, true);
    let asked = log.lock().unwrap().asked.clone();
    assert!(
        !asked.last().unwrap().refresh,
        "the refresh already happened"
    );
}

#[test]
fn a_frame_the_link_swallowed_is_rebuilt_from_parity() {
    let log = Arc::new(Mutex::new(Recorder::default()));
    let mut link = Link::open(Box::new(Fakes(Arc::clone(&log))), everything());
    link.until_promoted();
    // The second and third frames of the stream never arrive. Their groups carry parity, so the
    // viewer gets them anyway, without asking for anything.
    link.drop_video_frames = vec![2, 7];
    for step in 100..140 {
        link.show(step, true);
    }
    assert_eq!(link.dropped, 2, "the link really did eat two frames");

    let arrived: Vec<u64> = link.video().iter().map(|frame| frame.sequence).collect();
    assert!(
        arrived.contains(&2) && arrived.contains(&7),
        "the repaired frames are missing from {arrived:?}"
    );
    assert!(
        log.lock().unwrap().asked.iter().all(|asked| !asked.refresh),
        "parity repaired the loss, so no reference refresh was ever needed"
    );
}

#[test]
fn a_repaired_frame_is_the_frame_that_was_sent() {
    let log = Arc::new(Mutex::new(Recorder::default()));
    let mut link = Link::open(Box::new(Fakes(Arc::clone(&log))), everything());
    link.until_promoted();
    link.drop_video_frames = vec![3];
    for step in 200..220 {
        link.show(step, true);
    }
    let repaired = link
        .video()
        .into_iter()
        .find(|frame| frame.sequence == 3)
        .expect("the dropped frame came back")
        .clone();
    // The fake encoder tags every frame with a token equal to its own count, so the third frame
    // it produced has to come back carrying exactly that.
    assert_eq!(repaired.token, Some(3));
    assert!(!repaired.keyframe);
    assert_eq!(repaired.payload, vec![0, 0, 0, 1, 0x26, 3]);
}

#[test]
fn loss_beyond_what_parity_covers_is_reported_and_answered_with_a_refresh() {
    let log = Arc::new(Mutex::new(Recorder::default()));
    let mut link = Link::open(Box::new(Fakes(Arc::clone(&log))), everything());
    link.until_promoted();
    // A whole group, which no amount of parity for that group can rebuild.
    link.drop_video_frames = vec![5, 6, 7, 8];
    for step in 300..340 {
        link.show(step, true);
    }
    assert_eq!(link.dropped, 4);

    let arrived: Vec<u64> = link.video().iter().map(|frame| frame.sequence).collect();
    assert!(
        !arrived.contains(&5),
        "a group that cannot be repaired must not be invented"
    );
    let asked = log.lock().unwrap().asked.clone();
    assert!(
        asked.iter().any(|asked| asked.refresh),
        "the viewer reported the loss and the host answered with a reference refresh"
    );
    assert!(
        asked.iter().skip(1).all(|asked| !asked.keyframe),
        "and never with a keyframe"
    );
}

#[test]
fn a_viewer_without_the_parity_bit_is_sent_none() {
    let log = Arc::new(Mutex::new(Recorder::default()));
    let video_only = FeatureSet::none()
        .with(FeatureSet::MOTION_VIDEO)
        .with(FeatureSet::LONG_TERM_REFERENCES);
    let mut link = Link::open(Box::new(Fakes(Arc::clone(&log))), video_only);
    link.until_promoted();
    for step in 400..430 {
        link.show(step, true);
    }
    assert!(!link.video().is_empty(), "video is still streamed");
    assert_eq!(
        link.parity_sent, 0,
        "parity costs bandwidth, so a viewer that cannot use it is sent none"
    );
}

#[test]
fn only_the_motion_path_is_put_on_a_route_that_may_lose_it() {
    use multiplex_screen_protocol::Class;

    let log = Arc::new(Mutex::new(Recorder::default()));
    let mut link = Link::open(Box::new(Fakes(Arc::clone(&log))), everything());
    link.until_promoted();
    for step in 500..540 {
        link.show(step, true);
    }

    // Everything the host sent, by class. Tile batches and the messages that say what the session
    // is must never be classed as losable; the motion path must be, because it is the only part
    // built to survive loss.
    let classes = link.host_classes.clone();
    assert!(
        classes.contains(&Class::Video),
        "the motion path never ran, so this proves nothing"
    );
    assert!(classes.contains(&Class::Tiles) && classes.contains(&Class::Control));
    for (class, name) in &link.host_message_classes {
        let expected = match name.as_str() {
            "VideoFrame" | "Parity" => Class::Video,
            "Batch" => Class::Tiles,
            _ => Class::Control,
        };
        assert_eq!(*class, expected, "{name} was classed wrongly");
    }
}

#[test]
fn demotion_stops_the_stream_and_the_region_returns_to_tiles() {
    let log = Arc::new(Mutex::new(Recorder::default()));
    let mut link = Link::open(Box::new(Fakes(Arc::clone(&log))), everything());
    link.until_promoted();

    // The rectangle goes still. The picture it froze on is sent once, because that frame did
    // change; after that there is nothing to encode, and the tracker demotes the region.
    link.show(1_000, false);
    let streamed = link.video().len();
    for step in 1..40 {
        link.show(1_000 + step, false);
    }
    assert!(link.sender.region().is_none(), "the encoder was released");
    assert_eq!(
        link.video().len(),
        streamed,
        "nothing more was streamed once the region went still"
    );
    assert!(
        link.events
            .iter()
            .any(|event| matches!(event, ViewerEvent::MotionRegion { rect: None, .. })),
        "the viewer was told the region is back on the tile path"
    );
}
