//! What the Stage B motion path actually costs, measured with the real encoder.
//!
//! `cargo run -p termirust-screen-host --release --example motion_report`
//!
//! The tile path's workloads are measured in `termirust-screen-codec`'s `workload_report`. This
//! is the other half: the target in section 4.8 for "watching a video region" — 300 kbps to
//! 2 Mbps, adaptive — which nothing had measured, because until M4 there was no motion path to
//! measure. It runs a real `HostSession`, a real `MotionSender` and this machine's hardware HEVC
//! encoder over synthetic screen content, and reports what went on the wire.
//!
//! It also reports what the same workload costs with parity on, and what the degradation ladder
//! does to it under a link too small to carry it, because both are decisions the plan makes on
//! numbers it did not have.

use termirust_screen_codec::{FrameBuffer, Rect, Size};
use termirust_screen_host::{Ladder, MotionSender, Rung, platform_encoders};
use termirust_screen_protocol::{
    FeatureSet, Message, Profile, SurfaceInfo, Viewport, encode_frame,
};
use termirust_screen_session::{
    Grants, HostConfig, HostEvent, HostSession, ResumeStore, TicketVerifier, ViewerSession,
};

const TICKET: [u8; 32] = [7; 32];
const SURFACE: u32 = 1;
const WIDTH: u32 = 1440;
const HEIGHT: u32 = 900;
/// The window the video plays in: tile-aligned, and a realistic share of a laptop screen.
const REGION: Rect = Rect {
    x: 192,
    y: 128,
    width: 896,
    height: 512,
};
const FPS: u64 = 30;
const SECONDS: u64 = 6;

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

/// A desktop with a video playing in a window: smooth moving bands inside the region, a still
/// document behind it, and a clock so the tile path is never completely silent.
fn screen(step: u32) -> FrameBuffer {
    let mut buffer = FrameBuffer::new(size());
    buffer
        .fill_rect(size().bounds(), [246, 244, 242, 255])
        .unwrap();
    // Text-like rows behind the window, unchanging: this is what the tile path is good at, and
    // what must not be disturbed by the video.
    for row in 0..44 {
        let y = 24 + row * 19;
        let width = (row * 53) % 900 + 120;
        buffer
            .fill_rect(Rect::new(48, y, width, 10), [70, 72, 76, 255])
            .unwrap();
    }
    // The video: diagonal bands that slide, which is the kind of content the motion path exists
    // for and which the tile path handles worst.
    for row in 0..REGION.height / 4 {
        let y = REGION.y + row * 4;
        let shade = ((row * 7 + step * 13) % 220 + 20) as u8;
        buffer
            .fill_rect(
                Rect::new(REGION.x, y, REGION.width, 4),
                [shade, shade.wrapping_add(35), shade.wrapping_add(70), 255],
            )
            .unwrap();
    }
    let tick = (step % 20) * 3;
    buffer
        .fill_rect(Rect::new(1_320, 840, 64, 24), [252, 252, 252, 255])
        .unwrap();
    buffer
        .fill_rect(Rect::new(1_320 + tick, 840, 4, 24), [24, 24, 24, 255])
        .unwrap();
    buffer
}

/// The first second is thrown away.
///
/// A session opens by sending the whole screen once, which on a 1440 × 900 desktop is a couple of
/// hundred kilobytes. Averaged over a short run that one frame swamps everything and every
/// workload measures roughly the same, which is how a benchmark can be precise and meaningless at
/// once. The targets in 4.8 are about what a session costs while someone uses it, so the warm-up
/// is measured separately and reported separately.
const WARM_UP_SECONDS: u64 = 1;

#[derive(Default)]
struct Measured {
    tiles: usize,
    video: usize,
    parity: usize,
    control: usize,
    /// Everything sent before the steady state, which is mostly the first full screen.
    warm_up: usize,
    video_frames: usize,
    batches: usize,
    tile_ops: usize,
    ops_in_region: usize,
    ops_outside: usize,
    promoted_at: Option<u32>,
    promoted_region: Option<Rect>,
}

impl Measured {
    const fn total(&self) -> usize {
        self.tiles + self.video + self.parity + self.control
    }

    const fn seconds() -> f64 {
        (SECONDS - WARM_UP_SECONDS) as f64
    }

    fn kbps(&self, bytes: usize) -> f64 {
        bytes as f64 * 8.0 / Self::seconds() / 1_000.0
    }
}

/// Runs the workload and reports what went on the wire.
fn run(features: FeatureSet, squeeze: Option<u64>) -> (Measured, Rung) {
    let mut host = HostSession::new(
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
    let mut store = ResumeStore::default();
    let mut sender = MotionSender::new(platform_encoders());
    let mut ladder = Ladder::new();
    let mut measured = Measured::default();

    viewer.connect(TICKET);
    viewer.subscribe(SURFACE, Profile::Interactive);
    // A viewer showing the top two thirds of the screen. Without one the ladder's viewport rung
    // has nothing to clip to and does nothing, which made the first version of this report claim
    // a degradation that never happened.
    viewer.set_viewport(Viewport {
        surface: SURFACE,
        rect: Rect::new(0, 0, WIDTH, HEIGHT * 2 / 3),
        scale_milli: 1000,
    });

    let frames = (SECONDS * FPS) as u32;
    for step in 0..frames {
        let now_ms = u64::from(step) * 1_000 / FPS;
        let warming = now_ms < WARM_UP_SECONDS * 1_000;
        let buffer = screen(step);
        let frame = buffer.as_frame();
        host.frame(SURFACE, &frame, None, now_ms).unwrap();
        sender.frame(&mut host, SURFACE, &frame);
        if let Some(region) = sender.region() {
            measured.promoted_at.get_or_insert(step);
            // The last one, not the first: a region grows as more of the window warms up, and
            // what matters is what it settled on.
            measured.promoted_region = Some(region);
        }

        loop {
            let mut moved = false;
            while let Some(message) = viewer.poll_outgoing() {
                moved = true;
                // The acknowledgements have to reach the sender, or it never learns the viewer is
                // decoding and the tile path keeps covering a region it does not need to.
                for event in host.receive(message, &mut store).unwrap() {
                    match event {
                        HostEvent::VideoAcknowledged { tokens, .. } => sender.acknowledged(&tokens),
                        HostEvent::VideoLost { .. } => sender.lost(),
                        _ => {}
                    }
                }
            }
            let mut burst = 0;
            while let Some(message) = host.poll_outgoing() {
                moved = true;
                let bytes = encode_frame(&message).unwrap().len();
                burst += bytes;
                if warming {
                    measured.warm_up += bytes;
                } else {
                    match &message {
                        Message::Batch(batch) => {
                            measured.tiles += bytes;
                            measured.batches += 1;
                            measured.tile_ops += batch.ops.len();
                            for op in &batch.ops {
                                let tile = match op {
                                    termirust_screen_codec::TileOp::Solid { tile, .. }
                                    | termirust_screen_codec::TileOp::Cached { tile, .. }
                                    | termirust_screen_codec::TileOp::Lossy { tile, .. }
                                    | termirust_screen_codec::TileOp::Lossless { tile, .. } => {
                                        Some(*tile)
                                    }
                                    _ => None,
                                };
                                let inside = tile.map(|tile| {
                                    let across = WIDTH.div_ceil(64);
                                    let x = (tile.0 % across) * 64;
                                    let y = (tile.0 / across) * 64;
                                    REGION.intersect(Rect::new(x, y, 64, 64))
                                });
                                if inside.is_some_and(|rect| !rect.is_empty()) {
                                    measured.ops_in_region += 1;
                                } else {
                                    measured.ops_outside += 1;
                                }
                            }
                        }
                        Message::VideoFrame(_) => {
                            measured.video += bytes;
                            measured.video_frames += 1;
                        }
                        Message::Parity(_) => measured.parity += bytes,
                        _ => measured.control += bytes,
                    }
                }
                viewer.receive(message).unwrap();
            }
            if burst > 0 {
                ladder.sent(burst as u64);
            }
            if !moved {
                break;
            }
        }
        // A link of a fixed size, if one was asked for, so the ladder has something to react to.
        if let Some(bytes_per_second) = squeeze {
            let rung = ladder.consider(now_ms, Some(bytes_per_second));
            host.set_limits(rung.limits());
        }
    }
    (measured, ladder.rung())
}

fn main() {
    let everything = FeatureSet::from_bits(FeatureSet::KNOWN);
    let stage_a = FeatureSet::none();

    println!(
        "Surface {WIDTH} x {HEIGHT}, {FPS} frames a second, {SECONDS} seconds of synthetic \
         screen content with a {} x {} video window.\n",
        REGION.width, REGION.height
    );

    let (tiles_only, _) = run(stage_a, None);
    let (motion, _) = run(everything, None);
    if motion.video_frames == 0 {
        println!(
            "No hardware encoder on this machine, so the motion path did not run. The Stage A \
             row below is still a measurement; the rest would be zeros."
        );
    }

    println!("| Path | Tiles kbps | Video kbps | Parity kbps | Total kbps | Target |");
    println!("|---|---:|---:|---:|---:|---|");
    println!(
        "| Stage A, tiles only | {:.0} | — | — | {:.0} | ≤ 600 kbps, visibly choppy |",
        tiles_only.kbps(tiles_only.tiles),
        tiles_only.kbps(tiles_only.total()),
    );
    println!(
        "| Stage B, motion path | {:.0} | {:.0} | {:.0} | {:.0} | 300 kbps – 2 Mbps, adaptive |",
        motion.kbps(motion.tiles),
        motion.kbps(motion.video),
        motion.kbps(motion.parity),
        motion.kbps(motion.total()),
    );

    println!(
        "\nThe region was promoted after {} frames. {} video frames were sent; parity added \
         {:.1}% to the motion path.",
        motion
            .promoted_at
            .map_or("no".to_owned(), |at| at.to_string()),
        motion.video_frames,
        if motion.video > 0 {
            motion.parity as f64 * 100.0 / motion.video as f64
        } else {
            0.0
        },
    );

    // What the ladder does when the link cannot carry it. The figure is deliberately well under
    // what the workload above wanted, so something has to give.
    let squeezed_to = 40_000;
    let (squeezed, rung) = run(everything, Some(squeezed_to));
    println!(
        "\nOn a {} kbps link the ladder settled at {:?}: {:.0} kbps, from {} batches carrying {} \
         tile operations.",
        squeezed_to * 8 / 1_000,
        rung,
        squeezed.kbps(squeezed.total()),
        squeezed.batches,
        squeezed.tile_ops,
    );
    println!(
        "Unsqueezed: {} batches, {} tile operations — {} inside the drawn video window, {} \
         outside.",
        motion.batches, motion.tile_ops, motion.ops_in_region, motion.ops_outside,
    );
    println!(
        "The window drawn was {} x {}; the region settled on {:?}.",
        REGION.width, REGION.height, motion.promoted_region,
    );
}
