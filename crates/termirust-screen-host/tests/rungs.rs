//! Every rung of the degradation ladder has to actually save something.
//!
//! RS5 measured the ladder against a workload it could not shrink — on that content refinement was
//! never a cost and the viewport covered everything that moved — so the ladder descended and the
//! bytes did not move. That is a benchmark proving nothing, and it left the ladder's whole premise
//! unverified.
//!
//! This is the other test: a workload built so that each rung has something to give up, measured
//! rung by rung. A rung that costs the same as the one above it is either wired to nothing or
//! taking quality for free, and both are worth failing over.

use termirust_screen_codec::{FrameBuffer, LossyDetail, Rect, Size};
use termirust_screen_host::Rung;
use termirust_screen_protocol::{Profile, SurfaceInfo, Viewport, encode_frame};
use termirust_screen_session::{
    Grants, HostConfig, HostSession, Limits, ResumeStore, TicketVerifier, ViewerSession,
};

const TICKET: [u8; 32] = [7; 32];
const SURFACE: u32 = 1;
const WIDTH: u32 = 640;
const HEIGHT: u32 = 384;
/// What the viewer is looking at: the top half. The strip below it is what the viewport rung has
/// to save.
const VIEWPORT: Rect = Rect::new(0, 0, WIDTH, 192);

/// Where the changing content is. Each strip is 128 pixels — two tiles — tall, tile-aligned, and
/// they are separated by static gaps.
///
/// All three are deliberate. An area only becomes a motion region once it is at least
/// `min_side_tiles` (3) tiles down as well as across, so a tile-aligned two-row strip can never be
/// promoted no matter how fast it changes, and the gaps keep neighbouring strips from joining into
/// one that could be. That matters because this is a test of the ladder, which governs the tile
/// path. If the content were promoted the motion path would carry it, the tile throttle would
/// flatten every rung to roughly the same number, and the test would be measuring the wrong
/// subsystem. What the motion path costs is RS5's measurement, not this one.
const STRIPS: [u32; 2] = [0, 192];
const STRIP_HEIGHT: u32 = 128;

/// Frames in one cycle, and how many of them change.
///
/// The still half is what gives the refinement queue something to do: refinement only runs on
/// tiles that have been idle for `REFINE_IDLE_MS` (250 ms), so a screen that never settles would
/// leave the first rung with nothing to give up and the test would pass while proving nothing.
/// Ten frames at 33 ms is 330 ms, comfortably past it.
const CYCLE: u32 = 20;
const MOVING: u32 = 10;
const CYCLES: u32 = 5;

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

/// Which content to draw at `step`: advances for `MOVING` frames, then holds still.
fn phase(step: u32) -> u32 {
    (step / CYCLE) * MOVING + (step % CYCLE).min(MOVING)
}

/// A screen built so every rung has something to take away.
///
/// The strips are a smooth per-pixel ramp rather than flat bands, because the lossy first pass and
/// the refinement queue only apply to tiles that classify as `Picture` — many colours and few hard
/// edges. Flat bands have a small palette, so they are sent losslessly, and a workload made of
/// them leaves both the refinement rung and the quality rung with nothing to give up. That is the
/// mistake this test exists to avoid, so it is worth stating: the content has to be photographic
/// or two of the five rungs are untested.
fn screen(phase: u32) -> FrameBuffer {
    let mut buffer = FrameBuffer::new(size());
    buffer
        .fill_rect(size().bounds(), [250, 250, 250, 255])
        .unwrap();
    // Static text-like rows in the gaps, which the tile path is good at and which no rung is
    // allowed to damage.
    for row in 0..HEIGHT / 32 {
        let y = row * 32 + 8;
        if STRIPS
            .iter()
            .any(|top| (*top..top + STRIP_HEIGHT).contains(&y))
        {
            continue;
        }
        buffer
            .fill_rect(
                Rect::new(32, y, (row * 61) % 420 + 96, 10),
                [70, 72, 76, 255],
            )
            .unwrap();
    }
    let mut pixels = Vec::with_capacity((WIDTH * STRIP_HEIGHT * 4) as usize);
    for top in STRIPS {
        pixels.clear();
        for y in 0..STRIP_HEIGHT {
            for x in 0..WIDTH {
                let t = x * 2 + y * 3 + phase * 11 + top;
                pixels.extend([
                    (t % 256) as u8,
                    ((t / 2 + 40) % 256) as u8,
                    ((t / 3 + 90) % 256) as u8,
                    255,
                ]);
            }
        }
        buffer
            .write_rect(Rect::new(0, top, WIDTH, STRIP_HEIGHT), &pixels)
            .unwrap();
    }
    buffer
}

/// Bytes on the wire for one rung, over a fixed run.
fn cost(frames: &[FrameBuffer], limits: Limits) -> usize {
    let mut host = HostSession::new(
        vec![SurfaceInfo {
            id: SURFACE,
            size: size(),
            scale_milli: 1000,
            name: "Built-in Display".to_owned(),
        }],
        Tickets,
        HostConfig::default(),
    );
    let mut viewer = ViewerSession::new(64 << 20);
    let mut store = ResumeStore::default();
    viewer.connect(TICKET);
    viewer.subscribe(SURFACE, Profile::Interactive);
    viewer.set_viewport(Viewport {
        surface: SURFACE,
        rect: VIEWPORT,
        scale_milli: 1000,
    });
    host.set_limits(limits);

    let mut bytes = 0;
    let mut now_ms = 0;
    for step in 0..CYCLE * CYCLES {
        let frame = frames[phase(step) as usize].as_frame();
        host.frame(SURFACE, &frame, None, now_ms).unwrap();
        now_ms += 33;
        loop {
            let mut moved = false;
            while let Some(message) = viewer.poll_outgoing() {
                moved = true;
                host.receive(message, &mut store).unwrap();
            }
            while let Some(message) = host.poll_outgoing() {
                moved = true;
                // A session opens by sending the whole screen once, and that one frame is large
                // enough to swamp a short run and make every rung measure roughly the same. That
                // is how a benchmark can be precise and meaningless at once, so the first cycle
                // does not count.
                if step >= CYCLE {
                    bytes += encode_frame(&message).unwrap().len();
                }
                viewer.receive(message).unwrap();
            }
            if !moved {
                break;
            }
        }
    }
    bytes
}

#[test]
fn every_rung_costs_less_than_the_one_above_it() {
    // Built once and shared: the same pixels for every rung, so the only thing that differs
    // between the runs is what the ladder allowed.
    let frames: Vec<FrameBuffer> = (0..=phase(CYCLE * CYCLES)).map(screen).collect();
    let measured: Vec<(Rung, usize)> = [
        Rung::Full,
        Rung::NoRefinement,
        Rung::ViewportOnly,
        Rung::SlowerFrames,
        Rung::LowerQuality,
    ]
    .iter()
    .map(|rung| (*rung, cost(&frames, rung.limits())))
    .collect();

    // Printed so a run with --nocapture reports the table, not just a verdict: what these rungs
    // are worth is the number, and the number is what the evidence note records.
    for (rung, bytes) in &measured {
        println!(
            "{rung:?}: {bytes} bytes, {:.0} kbps",
            *bytes as f64 * 8.0 / (f64::from(CYCLE * (CYCLES - 1)) * 0.033) / 1_000.0
        );
    }

    for pair in measured.windows(2) {
        let (above, above_bytes) = pair[0];
        let (below, below_bytes) = pair[1];
        assert!(
            below_bytes < above_bytes,
            "{below:?} cost {below_bytes} bytes against {above:?}'s {above_bytes}: the rung \
             either saves nothing or is wired to nothing"
        );
    }

    // And the ladder as a whole is worth the quality it costs: giving up everything has to be a
    // large saving, not a rounding error, or it cannot rescue a link that is over budget.
    let full = measured[0].1;
    let worst = measured[measured.len() - 1].1;
    assert!(
        worst * 2 < full,
        "the whole ladder saved only {}%, which will not rescue a link that is over budget",
        100 - worst * 100 / full
    );
}

#[test]
fn the_last_rung_touches_only_the_video_encoder() {
    // CappedVideo is the one rung that changes nothing the tile path can see, so measuring it
    // against LowerQuality would only re-measure LowerQuality. What is worth asserting is why:
    // identical limits, and a bitrate cap that is the entire difference.
    assert_eq!(Rung::CappedVideo.limits(), Rung::LowerQuality.limits());
    assert_eq!(Rung::LowerQuality.video_bitrate(4_000_000), 4_000_000);
    assert!(Rung::CappedVideo.video_bitrate(4_000_000) < 4_000_000);
}

#[test]
fn the_rungs_give_up_what_they_say_they_give_up() {
    // Each rung's limits read back, so an edit that renumbers the ladder has to face what it
    // changed rather than only a byte count moving.
    assert_eq!(Rung::Full.limits().refine_budget_bytes, 2_000);
    assert_eq!(Rung::NoRefinement.limits().refine_budget_bytes, 0);
    assert!(!Rung::NoRefinement.limits().viewport_only);
    assert!(Rung::ViewportOnly.limits().viewport_only);
    assert_eq!(Rung::ViewportOnly.limits().minimum_interval_ms, 0);
    assert!(Rung::SlowerFrames.limits().minimum_interval_ms > 0);
    assert_eq!(
        Rung::SlowerFrames.limits().lossy_detail,
        LossyDetail::STANDARD
    );
    assert_eq!(Rung::LowerQuality.limits().lossy_detail, LossyDetail::LOW);
}
