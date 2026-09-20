//! The motion path end to end, with the real hardware encoder and decoder.
//!
//! Everything else about the motion path is tested with a fake encoder, because what those tests
//! are about is the protocol. This one is about the pixels: a region that moves is captured,
//! encoded, sent, decoded, and ends up in the viewer's framebuffer looking like the screen it
//! came from. If there is no hardware encoder on the machine, it skips.

use multiplex_screen_codec::{FrameBuffer, Rect, Size};
use multiplex_screen_host::{MotionSender, platform_encoders};
use multiplex_screen_protocol::{FeatureSet, Message, Profile, SurfaceInfo};
use multiplex_screen_session::{
    Grants, HostConfig, HostEvent, HostSession, ResumeStore, TicketVerifier, ViewerSession,
};

const TICKET: [u8; 32] = [7; 32];
const SURFACE: u32 = 1;
/// Tile-aligned, and large enough to clear the tracker's twelve-tile floor.
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

/// A screen with a video-like rectangle: smooth bands that slide, which is what the motion path
/// exists for and what a lossy codec handles well.
fn screen(step: u32) -> FrameBuffer {
    let mut buffer = FrameBuffer::new(size());
    buffer
        .fill_rect(size().bounds(), [236, 232, 228, 255])
        .unwrap();
    for row in 0..REGION.height / 4 {
        let y = REGION.y + row * 4;
        let shade = ((row * 6 + step * 11) % 200 + 20) as u8;
        buffer
            .fill_rect(
                Rect::new(REGION.x, y, REGION.width, 4),
                [shade, shade.wrapping_add(30), shade.wrapping_add(60), 255],
            )
            .unwrap();
    }
    // A clock outside the region, so the tile path keeps acknowledging while the region streams.
    let tick = (step % 18) * 2;
    buffer
        .fill_rect(Rect::new(560, 340, 40, 20), [250, 250, 250, 255])
        .unwrap();
    buffer
        .fill_rect(Rect::new(560 + tick, 340, 4, 20), [20, 20, 20, 255])
        .unwrap();
    buffer
}

/// How far apart two pictures are, per colour channel. Zero is identical.
fn difference(left: &FrameBuffer, right: &FrameBuffer, rect: Rect) -> u64 {
    let (left, right) = (left.as_frame(), right.as_frame());
    let mut total = 0u64;
    for y in rect.y..rect.bottom() {
        let a = left.row_span(y, rect);
        let b = right.row_span(y, rect);
        for (a, b) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
            total += (0..3).map(|at| a[at].abs_diff(b[at]) as u64).sum::<u64>();
        }
    }
    total / (rect.width as u64 * rect.height as u64 * 3)
}

#[test]
fn a_moving_region_arrives_as_video_and_ends_up_in_the_viewers_framebuffer() {
    let everything = FeatureSet::from_bits(FeatureSet::KNOWN);
    let mut host = HostSession::new(
        vec![SurfaceInfo {
            id: SURFACE,
            size: size(),
            scale_milli: 2000,
            name: "Built-in Display".to_owned(),
        }],
        Tickets,
        HostConfig {
            features: everything,
            ..HostConfig::default()
        },
    );
    let mut viewer = ViewerSession::with_features(64 << 20, everything);
    let mut store = ResumeStore::default();
    let mut sender = MotionSender::new(platform_encoders());
    let mut acknowledged: Vec<u32> = Vec::new();
    let mut video_frames = 0usize;

    viewer.connect(TICKET);
    viewer.subscribe(SURFACE, Profile::Interactive);

    let mut now_ms = 0u64;
    let mut last = FrameBuffer::new(size());
    for step in 0..60u32 {
        let captured = screen(step);
        let frame = captured.as_frame();
        host.frame(SURFACE, &frame, None, now_ms).unwrap();
        sender.frame(&mut host, SURFACE, &frame);
        now_ms += 33;
        last = captured;

        loop {
            let mut moved = false;
            while let Some(message) = viewer.poll_outgoing() {
                moved = true;
                for event in host.receive(message, &mut store).expect("host accepts") {
                    match event {
                        HostEvent::VideoAcknowledged { tokens, .. } => {
                            acknowledged = tokens.clone();
                            sender.acknowledged(&tokens);
                        }
                        HostEvent::VideoLost { .. } => sender.lost(),
                        _ => {}
                    }
                }
            }
            while let Some(message) = host.poll_outgoing() {
                moved = true;
                if matches!(message, Message::VideoFrame(_)) {
                    video_frames += 1;
                }
                viewer.receive(message).expect("viewer accepts");
            }
            if !moved {
                break;
            }
        }
    }

    if sender.region().is_none() {
        eprintln!("no hardware encoder on this machine; skipping the pixel comparison");
        return;
    }

    // Let the video pipeline drain before comparing, by holding the last frame still.
    //
    // A hardware encoder runs several frames behind: it takes frames in, and the picture that
    // comes out the far end is the one it was working on, not the one just handed over. On this
    // Mac that is four frames, so comparing the decoded region against the last frame shown was
    // comparing it against a screen the decoder had not been given yet -- it measured 63 against
    // frame 59 and 6 against frame 55, which is the picture being right and the question being
    // wrong. Holding the screen still is also what a viewer sees whenever anyone stops moving.
    let still = last.as_frame();
    for _ in 0..30 {
        host.frame(SURFACE, &still, None, now_ms).unwrap();
        sender.frame(&mut host, SURFACE, &still);
        now_ms += 33;
        loop {
            let mut moved = false;
            while let Some(message) = viewer.poll_outgoing() {
                moved = true;
                for event in host.receive(message, &mut store).expect("host accepts") {
                    match event {
                        HostEvent::VideoAcknowledged { tokens, .. } => {
                            acknowledged = tokens.clone();
                            sender.acknowledged(&tokens);
                        }
                        HostEvent::VideoLost { .. } => sender.lost(),
                        _ => {}
                    }
                }
            }
            while let Some(message) = host.poll_outgoing() {
                moved = true;
                viewer.receive(message).expect("viewer accepts");
            }
            if !moved {
                break;
            }
        }
    }
    assert!(video_frames > 4, "only {video_frames} frames were streamed");
    assert!(
        !acknowledged.is_empty(),
        "the viewer decoded frames but never told the host which references it holds, \
         so the host could never recover from loss without a keyframe"
    );

    let drawn = viewer
        .framebuffer(SURFACE)
        .expect("the viewer has a picture");
    // The region came through the video path, which is lossy, so this is a resemblance test and
    // not an equality one. What it rules out is the region being blank, stale, or torn.
    let inside = difference(drawn, &last, REGION);
    assert!(
        inside < 20,
        "the decoded region is {inside} off the screen it came from"
    );
    // Outside the region the tile path is exact, as it always was.
    let outside = difference(drawn, &last, Rect::new(480, 300, 160, 100));
    assert_eq!(outside, 0, "the tile path is still pixel exact");
}
