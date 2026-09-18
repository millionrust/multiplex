//! The software half of the M5 network matrix: a conditioned link between a real host and a real
//! viewer, and the two pass criteria that can be checked without a phone in someone's hand.
//!
//! Section 7 of the plan asks for (RTT, loss, cap) × {typing, scrolling, video} × {Stage A,
//! Stage B}, with four pass criteria. Two of them — no stall longer than a second, and a converged
//! screen within three seconds of a ten-second outage — are properties of the protocol and the
//! codec, and a real network adds nothing to measuring them but noise and irreproducibility. The
//! other two, input-to-glass P95 and how a real iPhone behaves, need the device and stay device
//! work. This covers the first two so the device runs start from a known-good baseline rather than
//! discovering a protocol bug through a phone.
//!
//! **Loss is modelled where it actually happens, and that is the point of doing this properly.**
//! Stage A's tile path rides an ordered, reliable channel: a lost packet is retransmitted, so what
//! the session sees is delay and reduced goodput, never a missing batch. Dropping batches here to
//! "simulate 5% loss" would be modelling a transport nobody ships, and the codec would look broken
//! for the wrong reason — one lost tile batch leaves the screen wrong forever, which is exactly why
//! [`termirust_screen_transport::Class`] exists. Only the video class is allowed to lose anything,
//! because it is the only part built to survive it.

#![allow(dead_code)]

use std::collections::VecDeque;

use termirust_screen_codec::{FrameBuffer, Rect, Size};
use termirust_screen_host::{MotionSender, platform_encoders};
use termirust_screen_protocol::{Class, FeatureSet, Message, Profile, SurfaceInfo, encode_frame};
use termirust_screen_session::{
    Grants, HostConfig, HostEvent, HostSession, ResumeStore, TicketVerifier, ViewerSession,
};

const TICKET: [u8; 32] = [7; 32];
const SURFACE: u32 = 1;
const WIDTH: u32 = 1280;
const HEIGHT: u32 = 800;
/// What a real transport hands over at a time.
const CHUNK: usize = 1_400;
/// Frame interval of the capture side, in microseconds.
const FRAME_MICROS: u64 = 33_333;

/// One row of the plan's matrix: what the link does to the bytes.
#[derive(Clone, Copy, Debug)]
pub struct Profile7 {
    pub name: &'static str,
    pub rtt_millis: u64,
    /// Loss in tenths of a percent, so 5% is 50 and the table needs no floats.
    pub loss_permille: u32,
    /// Bytes a second, or `u64::MAX` for an uncapped link.
    pub bytes_per_second: u64,
}

/// The four profiles section 7 names.
pub const PROFILES: [Profile7; 4] = [
    Profile7 {
        name: "20 ms, 0%, uncapped",
        rtt_millis: 20,
        loss_permille: 0,
        bytes_per_second: u64::MAX,
    },
    Profile7 {
        name: "100 ms, 1%, 5 Mbps",
        rtt_millis: 100,
        loss_permille: 10,
        bytes_per_second: 5_000_000 / 8,
    },
    Profile7 {
        name: "300 ms, 5%, 1 Mbps",
        rtt_millis: 300,
        loss_permille: 50,
        bytes_per_second: 1_000_000 / 8,
    },
    Profile7 {
        name: "500 ms, 10%, 200 kbps",
        rtt_millis: 500,
        loss_permille: 100,
        bytes_per_second: 200_000 / 8,
    },
];

/// What the screen is doing while the link misbehaves.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Workload {
    Typing,
    Scrolling,
    Video,
}

impl Workload {
    pub const ALL: [Self; 3] = [Self::Typing, Self::Scrolling, Self::Video];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Typing => "typing",
            Self::Scrolling => "scrolling",
            Self::Video => "video",
        }
    }
}

/// What one cell of the matrix measured.
#[derive(Clone, Copy, Debug)]
pub struct Cell {
    pub profile: &'static str,
    pub workload: Workload,
    pub stage_b: bool,
    /// Longest the viewer went without its picture changing at all while the host had new frames
    /// to show, in milliseconds. The plan's bound is one second.
    ///
    /// Deliberately not "went without being pixel-exact". A picture region is sent as a lossy first
    /// pass and only becomes exact once it goes idle and refinement catches up, so a playing video
    /// is never exact — measuring exactness here reported the whole of every video run as one
    /// five-second stall. A stall is the screen not moving, which is what a person sees.
    pub longest_stall_millis: u64,
    /// How long after a ten-second outage the screen showed the current content again, in
    /// milliseconds. The plan's bound is three seconds.
    pub caught_up_after_millis: u64,
    /// How long until every picture tile was refined to exact pixels as well.
    ///
    /// Reported separately because the plan says "converged screen" and does not say which it
    /// means, and the two differ by seconds on a slow link. A viewer showing the right screen at a
    /// lossy first pass has caught up in the sense a person cares about; getting the last pixel
    /// exact is refinement, and refinement is bounded by the link.
    pub exact_after_millis: u64,
    pub bytes: usize,
}

impl Cell {
    pub fn kbps(self, seconds: f64) -> f64 {
        self.bytes as f64 * 8.0 / seconds / 1_000.0
    }
}

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
    Size::new(WIDTH, HEIGHT).unwrap()
}

/// The screen for a workload at `step`.
///
/// Deliberately the same shapes the codec's own workload report uses, so a number here can be set
/// beside one there: text that changes a line at a time, text that scrolls, and a picture region
/// that is smooth rather than noise.
pub fn screen(workload: Workload, step: u32) -> FrameBuffer {
    let mut buffer = FrameBuffer::new(size());
    buffer
        .fill_rect(size().bounds(), [246, 244, 242, 255])
        .unwrap();
    let first_line = if workload == Workload::Scrolling {
        step / 2
    } else {
        0
    };
    for row in 0..40u32 {
        let line = row + first_line;
        let width = (line * 53) % 900 + 120;
        buffer
            .fill_rect(Rect::new(48, 24 + row * 18, width, 10), [70, 72, 76, 255])
            .unwrap();
    }
    if workload == Workload::Typing {
        let typed = step / 4;
        buffer
            .fill_rect(
                Rect::new(48, 24 + 2 * 18, 40 + (typed % 60) * 8, 10),
                [40, 90, 160, 255],
            )
            .unwrap();
    }
    if workload == Workload::Video {
        let area = Rect::new(320, 256, 640, 384);
        let t = step * 3;
        let mut pixels = Vec::with_capacity((area.width * area.height * 4) as usize);
        for y in 0..area.height {
            for x in 0..area.width {
                // Smooth, like anything a camera or a codec has touched. Noise is a worst case and
                // is measured in the codec's own report, not here.
                let a = (x + t) * 3 / 4;
                let b = (y * 2 + t) / 3;
                let radial = ((x.abs_diff(320) + y.abs_diff(192)) * 2 + t) / 5;
                pixels.extend([(a % 256) as u8, (b % 256) as u8, (radial % 256) as u8, 255]);
            }
        }
        buffer.write_rect(area, &pixels).unwrap();
    }
    buffer
}

/// A message in flight, with the time it finishes arriving.
struct InFlight {
    at_micros: u64,
    message: Message,
}

/// A host and a viewer with a conditioned link between them.
struct Link {
    host: HostSession<Tickets>,
    viewer: ViewerSession,
    store: ResumeStore,
    profile: Profile7,
    now_micros: u64,
    /// When the link is next free to put a byte on the wire, for the bandwidth cap.
    wire_free_micros: u64,
    in_flight: VecDeque<InFlight>,
    burst: u64,
    bytes: usize,
    /// Advances on every dropped datagram, so loss is deterministic rather than random: a matrix
    /// that reports a different number every run cannot be a gate.
    loss_counter: u32,
    /// Bytes held back while the link is cut.
    severed: bool,
    /// Drives the motion path on Stage B. Without one, advertising the video feature changes
    /// nothing and the two stages measure the same thing — which is how the first version of this
    /// harness reported Stage B and Stage A as byte-for-byte identical.
    motion: Option<MotionSender>,
    features: FeatureSet,
    /// A cheap digest of the viewer's last picture, for spotting that it moved at all.
    last_picture: Option<u64>,
}

impl Link {
    fn new(profile: Profile7, features: FeatureSet) -> Self {
        let host = HostSession::new(
            vec![SurfaceInfo {
                id: SURFACE,
                size: size(),
                scale_milli: 1000,
                name: "Built-in Display".to_owned(),
            }],
            Tickets,
            HostConfig {
                features,
                ..HostConfig::default()
            },
        );
        let mut viewer = ViewerSession::with_features(64 << 20, features);
        let store = ResumeStore::default();
        viewer.connect(TICKET);
        viewer.subscribe(SURFACE, Profile::Interactive);
        let mut link = Self {
            host,
            viewer,
            store,
            profile,
            now_micros: 0,
            wire_free_micros: 0,
            in_flight: VecDeque::new(),
            burst: 0,
            bytes: 0,
            loss_counter: 0,
            severed: false,
            motion: features
                .has(FeatureSet::MOTION_VIDEO)
                .then(|| MotionSender::new(platform_encoders())),
            features,
            last_picture: None,
        };
        link.settle();
        link
    }

    /// Whether this datagram is one of the ones the link eats.
    ///
    /// Only the video class can lose anything. See the module note: the tile path is ordered and
    /// reliable, so its loss is delay, and that is already in the serialisation time below.
    fn drops(&mut self, class: Class) -> bool {
        if class != Class::Video || self.profile.loss_permille == 0 {
            return false;
        }
        self.loss_counter = self.loss_counter.wrapping_add(self.profile.loss_permille);
        if self.loss_counter >= 1_000 {
            self.loss_counter -= 1_000;
            return true;
        }
        false
    }

    /// How long `bytes` take to get across, including what retransmission costs.
    ///
    /// Reliable classes pay for loss in time: a lost segment is noticed and resent, which on a link
    /// with this round trip costs about one of them per loss event. That is the honest way to put
    /// 10% loss on an ordered stream.
    fn transit_micros(&mut self, bytes: usize, class: Class) -> u64 {
        let serialisation = if self.profile.bytes_per_second == u64::MAX {
            0
        } else {
            bytes as u64 * 1_000_000 / self.profile.bytes_per_second
        };
        let one_way = self.profile.rtt_millis * 1_000 / 2;
        let mut micros = serialisation + one_way;
        if class != Class::Video && self.profile.loss_permille > 0 {
            let segments = (bytes / CHUNK).max(1) as u64;
            let resends = segments * u64::from(self.profile.loss_permille) / 1_000;
            micros += resends * self.profile.rtt_millis * 1_000;
        }
        micros
    }

    /// Puts everything the host wants to send onto the wire.
    fn send(&mut self) {
        let mut burst_bytes = 0;
        while let Some(message) = self.host.poll_outgoing() {
            let class = message.class();
            let encoded = encode_frame(&message).expect("encodable");
            self.bytes += encoded.len();
            burst_bytes += encoded.len();
            if self.severed || self.drops(class) {
                continue;
            }
            // The cap is a queue, not a per-message delay: two messages sent at once do not both
            // arrive at one message's worth of serialisation time.
            let start = self.wire_free_micros.max(self.now_micros);
            let transit = self.transit_micros(encoded.len(), class);
            self.wire_free_micros = start + transit.min(u64::MAX / 4);
            self.in_flight.push_back(InFlight {
                at_micros: self.wire_free_micros,
                message,
            });
        }
        if burst_bytes > 0
            && self
                .host
                .agreed_features()
                .has(FeatureSet::BANDWIDTH_REPORTS)
        {
            self.burst += 1;
        }
    }

    /// Delivers everything that has arrived by `self.now_micros`, and answers the host.
    fn receive(&mut self) {
        while self
            .in_flight
            .front()
            .is_some_and(|flight| flight.at_micros <= self.now_micros)
        {
            let flight = self.in_flight.pop_front().expect("checked");
            let bytes = encode_frame(&flight.message).expect("encodable").len();
            self.viewer.observed_bytes(bytes, flight.at_micros);
            let _ = self.viewer.receive(flight.message);
        }
        while let Some(message) = self.viewer.poll_outgoing() {
            if self.severed {
                continue;
            }
            // The viewer's answers are small; they cost a one-way trip and nothing else.
            let Ok(events) = self.host.receive(message, &mut self.store) else {
                continue;
            };
            for event in events {
                // Without these the sender never learns the decoder started, and the tile path
                // keeps paying for a region the video is already carrying.
                match (event, self.motion.as_mut()) {
                    (HostEvent::VideoAcknowledged { tokens, .. }, Some(motion)) => {
                        motion.acknowledged(&tokens);
                    }
                    (HostEvent::VideoLost { .. }, Some(motion)) => motion.lost(),
                    _ => {}
                }
            }
        }
    }

    /// Runs the link until nothing more is in flight and nobody has anything to say.
    fn settle(&mut self) {
        for _ in 0..64 {
            self.send();
            if let Some(next) = self.in_flight.front().map(|flight| flight.at_micros) {
                self.now_micros = self.now_micros.max(next);
            }
            self.receive();
            if self.in_flight.is_empty() {
                let before = self.bytes;
                self.send();
                if self.bytes == before {
                    return;
                }
            }
        }
    }

    /// Whether the viewer's picture changed since the last time this was asked.
    fn picture_changed(&mut self) -> bool {
        let Some(drawn) = self.viewer.framebuffer(SURFACE) else {
            return false;
        };
        let mut sum: u64 = 0;
        for y in 0..HEIGHT {
            for byte in drawn.as_frame().row(y) {
                sum = sum.wrapping_mul(31).wrapping_add(u64::from(*byte));
            }
        }
        let changed = self.last_picture != Some(sum);
        self.last_picture = Some(sum);
        changed
    }

    /// Whether the viewer is showing the current screen, allowing the lossy first pass.
    ///
    /// Mean absolute error per channel, against a threshold the lossy pass stays well inside and a
    /// stale screen does not: a picture region that has not arrived at all is nowhere near.
    fn caught_up(&mut self, shown: &FrameBuffer) -> bool {
        let Some(drawn) = self.viewer.framebuffer(SURFACE) else {
            return false;
        };
        let mut error: u64 = 0;
        let mut count: u64 = 0;
        for y in (0..HEIGHT).step_by(4) {
            let a = drawn.as_frame().row(y);
            let b = shown.as_frame().row(y);
            for (left, right) in a.iter().zip(b.iter()).step_by(4) {
                error += u64::from(left.abs_diff(*right));
                count += 1;
            }
        }
        count > 0 && error / count <= 12
    }

    /// Whether the viewer is showing exactly what the host last drew.
    fn converged(&mut self, shown: &FrameBuffer) -> bool {
        let Some(drawn) = self.viewer.framebuffer(SURFACE) else {
            return false;
        };
        (0..HEIGHT).all(|y| drawn.as_frame().row(y) == shown.as_frame().row(y))
    }

    fn show(&mut self, workload: Workload, step: u32) -> FrameBuffer {
        let buffer = screen(workload, step);
        self.draw(&buffer);
        buffer
    }

    fn draw(&mut self, buffer: &FrameBuffer) {
        let frame = buffer.as_frame();
        let _ = self
            .host
            .frame(SURFACE, &frame, None, self.now_micros / 1_000);
        if let Some(motion) = self.motion.as_mut() {
            motion.frame(&mut self.host, SURFACE, &frame);
        }
    }

    /// The outage the plan asks about: the connection drops, and the viewer comes back and resumes.
    ///
    /// Not a severed wire with the session left running. A tile batch is a difference from the last
    /// picture, so bytes dropped on the floor mid-session leave the screen wrong forever — that is
    /// the whole reason the resume store exists, and modelling an outage without it would measure a
    /// transport nobody ships and call the codec broken for it.
    fn reconnect(&mut self) {
        self.host.close("connection_lost", &mut self.store);
        self.viewer.disconnected();
        self.in_flight.clear();
        self.severed = false;
        self.wire_free_micros = self.now_micros;

        let mut host = HostSession::new(
            vec![SurfaceInfo {
                id: SURFACE,
                size: size(),
                scale_milli: 1000,
                name: "Built-in Display".to_owned(),
            }],
            Tickets,
            HostConfig {
                features: self.features,
                ..HostConfig::default()
            },
        );
        std::mem::swap(&mut self.host, &mut host);
        self.viewer.connect(TICKET);
        self.viewer.subscribe(SURFACE, Profile::Interactive);
        // A reopened session has a new encoder, so the motion path starts again with it.
        self.motion = self
            .features
            .has(FeatureSet::MOTION_VIDEO)
            .then(|| MotionSender::new(platform_encoders()));
    }
}

/// Runs one cell of the matrix.
pub fn run(profile: Profile7, workload: Workload, features: FeatureSet) -> Cell {
    let mut link = Link::new(profile, features);
    let stage_b = features.has(FeatureSet::MOTION_VIDEO);

    // A first screen so the session is past its opening full frame before anything is timed.
    let first = link.show(workload, 0);
    link.settle();
    let _ = link.converged(&first);
    link.bytes = 0;

    let mut longest_stall = 0;
    let mut last_moved = link.now_micros;
    link.picture_changed();
    for step in 1..=150u32 {
        link.now_micros += FRAME_MICROS;
        link.show(workload, step);
        link.send();
        link.receive();
        if link.picture_changed() {
            last_moved = link.now_micros;
        } else {
            longest_stall = longest_stall.max(link.now_micros.saturating_sub(last_moved));
        }
    }

    // Ten seconds with the link cut, then let it come back.
    link.severed = true;
    let outage_end = link.now_micros + 10_000_000;
    let mut step = 151;
    while link.now_micros < outage_end {
        link.now_micros += FRAME_MICROS;
        link.show(workload, step);
        link.send();
        step += 1;
    }
    link.reconnect();

    let restored = link.now_micros;
    let mut caught_up_after = u64::MAX;
    let mut exact_after = u64::MAX;
    let deadline = link.now_micros + 30_000_000;
    // The screen stops changing while it catches up, which is what a person does after a tunnel.
    let settled = screen(workload, step);
    while link.now_micros < deadline {
        link.now_micros += FRAME_MICROS;
        link.draw(&settled);
        link.send();
        link.receive();
        if caught_up_after == u64::MAX && link.caught_up(&settled) {
            caught_up_after = link.now_micros - restored;
        }
        if link.converged(&settled) {
            exact_after = link.now_micros - restored;
            break;
        }
    }

    Cell {
        profile: profile.name,
        workload,
        stage_b,
        longest_stall_millis: longest_stall / 1_000,
        caught_up_after_millis: caught_up_after.saturating_div(1_000),
        exact_after_millis: exact_after.saturating_div(1_000),
        bytes: link.bytes,
    }
}

/// Every cell of the matrix.
pub fn run_all() -> Vec<Cell> {
    let stage_a = FeatureSet::none();
    let stage_b = FeatureSet::from_bits(FeatureSet::KNOWN);
    let mut cells = Vec::new();
    for profile in PROFILES {
        for workload in Workload::ALL {
            for features in [stage_a, stage_b] {
                cells.push(run(profile, workload, features));
            }
        }
    }
    cells
}

/// The seconds of screen time one cell covers, for a rate.
pub fn cell_seconds() -> f64 {
    150.0 * FRAME_MICROS as f64 / 1_000_000.0
}
