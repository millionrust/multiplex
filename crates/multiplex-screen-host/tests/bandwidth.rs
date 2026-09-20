//! The bandwidth loop, end to end: the host marks each burst, the viewer times it, and the host
//! ends up with a number close to what the link really delivers.
//!
//! The link here is a fake with a known rate, which is the only way to test an estimator: against
//! a real network there is nothing to compare the answer to.

use multiplex_screen_codec::{FrameBuffer, Rect, Size};
use multiplex_screen_protocol::{
    FeatureSet, FrameReader, Message, Profile, SurfaceInfo, encode_frame,
};
use multiplex_screen_session::{
    Grants, HostConfig, HostEvent, HostSession, ResumeStore, TicketVerifier, ViewerSession,
};

const TICKET: [u8; 32] = [7; 32];
const SURFACE: u32 = 1;

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
    Size::new(800, 600).unwrap()
}

/// A screen with enough changing detail that each frame is worth measuring.
fn screen(step: u32) -> FrameBuffer {
    let mut buffer = FrameBuffer::new(size());
    buffer
        .fill_rect(size().bounds(), [240, 238, 236, 255])
        .unwrap();
    for row in 0..40 {
        let y = row * 15;
        let width = (row * 37 + step * 53) % 700 + 40;
        let shade = ((row * 13 + step * 29) % 200 + 20) as u8;
        buffer
            .fill_rect(
                Rect::new(20, y, width, 12),
                [shade, shade.wrapping_add(20), shade.wrapping_add(40), 255],
            )
            .unwrap();
    }
    buffer
}

/// Host and viewer over a link that delivers a fixed number of bytes per second.
struct Link {
    host: HostSession<Tickets>,
    viewer: ViewerSession,
    store: ResumeStore,
    /// What the fake link delivers.
    bytes_per_second: u64,
    /// The viewer's clock, advanced by however long the link took to carry each chunk.
    now_micros: u64,
    /// Bursts the host has closed, and the estimator's own view of them.
    measured: Vec<(u64, u64)>,
    burst: u64,
}

impl Link {
    fn new(bytes_per_second: u64, features: FeatureSet) -> Self {
        let mut link = Self {
            host: HostSession::new(
                vec![SurfaceInfo {
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
            viewer: ViewerSession::with_features(64 << 20, features),
            store: ResumeStore::default(),
            bytes_per_second,
            now_micros: 0,
            measured: Vec::new(),
            burst: 0,
        };
        link.viewer.connect(TICKET);
        link.viewer.subscribe(SURFACE, Profile::Interactive);
        link.pump();
        link
    }

    /// Carries one flush across the fake link, in chunks, advancing the clock as it goes.
    ///
    /// Chunking matters: a burst delivered in one lump has nothing to time, and a real transport
    /// hands bytes over in pieces. This is what the estimator sees on a real link.
    fn deliver(&mut self, bytes: &[u8]) {
        const CHUNK: usize = 1_400;
        let mut reader = FrameReader::new();
        for chunk in bytes.chunks(CHUNK) {
            // The link takes as long as its rate says it should.
            self.now_micros += chunk.len() as u64 * 1_000_000 / self.bytes_per_second;
            self.viewer.observed_bytes(chunk.len(), self.now_micros);
            reader.push(chunk);
            while let Some(message) = reader.next_message().expect("well formed") {
                self.viewer.receive(message).expect("the viewer accepts");
            }
        }
    }

    fn pump(&mut self) {
        loop {
            let mut moved = false;
            while let Some(message) = self.viewer.poll_outgoing() {
                moved = true;
                for event in self
                    .host
                    .receive(message, &mut self.store)
                    .expect("accepted")
                {
                    if let HostEvent::BurstMeasured {
                        bytes,
                        spread_micros,
                        ..
                    } = event
                    {
                        self.measured.push((bytes, spread_micros));
                    }
                }
            }
            let mut bytes = Vec::new();
            while let Some(message) = self.host.poll_outgoing() {
                moved = true;
                bytes.extend(encode_frame(&message).expect("encodable"));
            }
            if !bytes.is_empty() {
                // What `ScreenHost::flush` does: close the burst with a mark.
                if self
                    .host
                    .agreed_features()
                    .has(FeatureSet::BANDWIDTH_REPORTS)
                {
                    let mark = Message::BurstMark {
                        burst: self.burst,
                        bytes: bytes.len() as u64,
                    };
                    bytes.extend(encode_frame(&mark).expect("encodable"));
                    self.burst += 1;
                }
                self.deliver(&bytes);
            }
            if !moved {
                return;
            }
        }
    }

    fn show(&mut self, step: u32) {
        let buffer = screen(step);
        self.host
            .frame(SURFACE, &buffer.as_frame(), None, u64::from(step) * 40)
            .expect("the tile encoder takes the frame");
        self.pump();
    }
}

fn everything() -> FeatureSet {
    FeatureSet::from_bits(FeatureSet::KNOWN)
}

#[test]
fn the_host_measures_a_link_close_to_what_it_really_delivers() {
    // A megabyte a second, which is a fair home upload.
    let mut link = Link::new(1_000_000, everything());
    for step in 0..12 {
        link.show(step);
    }
    assert!(
        link.measured.len() >= 4,
        "only {} bursts were measured",
        link.measured.len()
    );

    // Every report should describe a burst at roughly the link's rate.
    for (bytes, spread) in &link.measured {
        let rate = bytes * 1_000_000 / spread;
        assert!(
            (700_000..=1_300_000).contains(&rate),
            "a burst measured {rate} bytes a second on a 1,000,000 link"
        );
    }
}

#[test]
fn a_slower_link_measures_slower() {
    let fast = {
        let mut link = Link::new(2_000_000, everything());
        for step in 0..12 {
            link.show(step);
        }
        link.measured.clone()
    };
    let slow = {
        let mut link = Link::new(200_000, everything());
        for step in 0..12 {
            link.show(step);
        }
        link.measured.clone()
    };
    let rate = |measured: &[(u64, u64)]| -> u64 {
        let bytes: u64 = measured.iter().map(|(bytes, _)| bytes).sum();
        let spread: u64 = measured.iter().map(|(_, spread)| spread).sum();
        bytes * 1_000_000 / spread.max(1)
    };
    let (fast, slow) = (rate(&fast), rate(&slow));
    assert!(
        fast > slow * 5,
        "the ten-times-faster link measured {fast} against {slow}"
    );
}

#[test]
fn a_link_that_cannot_keep_up_makes_the_session_give_things_up_in_order() {
    use multiplex_screen_host::{Ladder, RateEstimator, Rung};

    // The ladder and estimator, driven by what the real loop measures. This is the same pair the
    // host wires together; putting them here keeps the test honest about the numbers rather than
    // asserting on a private field.
    let mut estimator = RateEstimator::new();
    let mut ladder = Ladder::new();
    let mut seen = Vec::new();

    let mut link = Link::new(120_000, everything());
    let mut now_ms = 0;
    for step in 0..120 {
        link.show(step);
        for (bytes, spread) in link.measured.drain(..) {
            estimator.record(bytes, spread);
        }
        // What the host sends per frame, at the cadence it would send it.
        ladder.sent(4_000);
        now_ms += 40;
        seen.push(ladder.consider(now_ms, estimator.estimate(), false));
    }

    let ended = *seen.last().expect("the ladder ran");
    assert!(
        ended.is_degraded(),
        "a link far below what the screen wants should have given something up"
    );
    // Whatever it gave up, it gave up in order: no rung is ever skipped on the way down.
    let mut worst = Rung::Full;
    for rung in &seen {
        assert!(
            *rung <= worst.max(*rung),
            "the ladder jumped from {worst:?} to {rung:?}"
        );
        worst = worst.max(*rung);
    }
    // And the first thing it gave up is the one nobody notices.
    let first_change = seen.iter().find(|rung| **rung != Rung::Full);
    assert_eq!(first_change, Some(&Rung::NoRefinement));
}

#[test]
fn a_viewer_that_never_agreed_is_never_asked_to_measure() {
    let mut link = Link::new(1_000_000, FeatureSet::none());
    for step in 0..12 {
        link.show(step);
    }
    assert!(
        link.measured.is_empty(),
        "a viewer that cannot time bursts must not be sent marks"
    );
    assert_eq!(link.burst, 0, "and no burst was ever opened");
}

#[test]
fn a_viewer_that_never_reports_arrivals_claims_nothing() {
    // Everything negotiated, but the transport never says when bytes arrived — an integration
    // that forgot to call `observed_bytes`. Nothing should be invented from that silence.
    let mut link = Link::new(1_000_000, everything());
    link.viewer = ViewerSession::with_features(64 << 20, everything());
    link.host = HostSession::new(
        vec![SurfaceInfo {
            id: SURFACE,
            size: size(),
            scale_milli: 2000,
            name: "Built-in Display".to_owned(),
        }],
        Tickets,
        HostConfig {
            features: everything(),
            ..HostConfig::default()
        },
    );
    link.store = ResumeStore::default();
    link.measured.clear();
    link.viewer.connect(TICKET);
    link.viewer.subscribe(SURFACE, Profile::Interactive);

    // Deliver by hand, without ever reporting an arrival.
    for step in 0..12u32 {
        let buffer = screen(step);
        link.host
            .frame(SURFACE, &buffer.as_frame(), None, u64::from(step) * 40)
            .unwrap();
        loop {
            let mut moved = false;
            while let Some(message) = link.viewer.poll_outgoing() {
                moved = true;
                for event in link.host.receive(message, &mut link.store).unwrap() {
                    if let HostEvent::BurstMeasured { .. } = event {
                        panic!("a measurement was claimed without any arrival being reported");
                    }
                }
            }
            let mut bytes = Vec::new();
            while let Some(message) = link.host.poll_outgoing() {
                moved = true;
                bytes.extend(encode_frame(&message).unwrap());
            }
            if !bytes.is_empty() {
                let mark = Message::BurstMark {
                    burst: 0,
                    bytes: bytes.len() as u64,
                };
                bytes.extend(encode_frame(&mark).unwrap());
                let mut reader = FrameReader::new();
                reader.push(&bytes);
                while let Some(message) = reader.next_message().unwrap() {
                    link.viewer.receive(message).unwrap();
                }
            }
            if !moved {
                break;
            }
        }
    }
}
