//! What the rate controller's limits actually do to a session.
//!
//! The one that could be quietly wrong is the viewport clip. Damage outside what the viewer is
//! looking at is not sent — but it must not be *lost*, or a person who scrolls down finds stale
//! pixels there with nothing to correct them, and nothing anywhere would report it. That property
//! rests on how the tile encoder tracks hashes, which is exactly the kind of reasoning that holds
//! until someone changes the encoder.

use termirust_screen_codec::{FrameBuffer, LossyDetail, Rect, Size};
use termirust_screen_protocol::{Message, Profile, SurfaceInfo, Viewport};
use termirust_screen_session::{
    Grants, HostConfig, HostSession, Limits, ResumeStore, TicketVerifier, ViewerSession,
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
    Size::new(640, 480).unwrap()
}

/// A screen with a distinct block at the top and another at the bottom, each changing on its own
/// schedule so a test can watch one without the other.
fn screen(top: u8, bottom: u8) -> FrameBuffer {
    let mut buffer = FrameBuffer::new(size());
    buffer
        .fill_rect(size().bounds(), [240, 240, 240, 255])
        .unwrap();
    buffer
        .fill_rect(Rect::new(0, 0, 640, 200), [top, top, top, 255])
        .unwrap();
    buffer
        .fill_rect(Rect::new(0, 280, 640, 200), [bottom, bottom, bottom, 255])
        .unwrap();
    buffer
}

struct Link {
    host: HostSession<Tickets>,
    viewer: ViewerSession,
    store: ResumeStore,
    now_ms: u64,
}

impl Link {
    fn open() -> Self {
        let mut link = Self {
            host: HostSession::new(
                vec![SurfaceInfo {
                    id: SURFACE,
                    size: size(),
                    scale_milli: 1000,
                    name: "Built-in Display".to_owned(),
                }],
                Tickets,
                HostConfig::default(),
            ),
            viewer: ViewerSession::new(64 << 20),
            store: ResumeStore::default(),
            now_ms: 0,
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
                self.host
                    .receive(message, &mut self.store)
                    .expect("the host accepts the viewer");
            }
            while let Some(message) = self.host.poll_outgoing() {
                moved = true;
                self.viewer.receive(message).expect("the viewer accepts");
            }
            if !moved {
                return;
            }
        }
    }

    /// Shows a frame with `damage` named, as a capture backend would.
    fn show(&mut self, top: u8, bottom: u8, damage: &[Rect]) {
        let buffer = screen(top, bottom);
        self.host
            .frame(SURFACE, &buffer.as_frame(), Some(damage), self.now_ms)
            .expect("the tile encoder takes the frame");
        self.now_ms += 40;
        self.pump();
    }

    /// The colour the viewer currently shows at a point.
    fn shown(&self, x: u32, y: u32) -> u8 {
        let framebuffer = self.viewer.framebuffer(SURFACE).expect("a picture");
        framebuffer.as_frame().row(y)[x as usize * 4]
    }
}

#[test]
fn damage_outside_the_viewport_is_deferred_and_not_lost() {
    let mut link = Link::open();
    link.show(10, 10, &[size().bounds()]);
    assert_eq!(link.shown(0, 100), 10);
    assert_eq!(link.shown(0, 380), 10);

    // The viewer is looking at the top half only.
    link.viewer.set_viewport(Viewport {
        surface: SURFACE,
        rect: Rect::new(0, 0, 640, 240),
        scale_milli: 1000,
    });
    link.pump();
    link.host.set_limits(Limits {
        viewport_only: true,
        ..Limits::default()
    });

    // Both halves change. Only the half being looked at should arrive.
    link.show(90, 200, &[size().bounds()]);
    assert_eq!(link.shown(0, 100), 90, "the viewport was updated");
    assert_eq!(
        link.shown(0, 380),
        10,
        "the half nobody is looking at was not sent"
    );

    // Now the viewer looks down. The change it never received has to arrive, without the screen
    // having changed again — otherwise scrolling shows stale pixels forever.
    link.viewer.set_viewport(Viewport {
        surface: SURFACE,
        rect: Rect::new(0, 240, 640, 240),
        scale_milli: 1000,
    });
    link.pump();
    link.show(90, 200, &[size().bounds()]);
    assert_eq!(
        link.shown(0, 380),
        200,
        "deferred damage was lost rather than deferred"
    );
}

#[test]
fn the_interval_delays_a_frame_without_dropping_what_it_carried() {
    let mut link = Link::open();
    link.show(10, 10, &[size().bounds()]);
    link.host.set_limits(Limits {
        minimum_interval_ms: 200,
        ..Limits::default()
    });

    // Three frames inside one interval. The last one's content is what must end up on screen.
    link.show(20, 20, &[size().bounds()]);
    link.show(30, 30, &[size().bounds()]);
    link.show(40, 40, &[size().bounds()]);
    // Past the interval, so the accumulated damage goes.
    link.now_ms += 400;
    link.show(40, 40, &[size().bounds()]);
    assert_eq!(
        link.shown(0, 100),
        40,
        "the frames inside the interval were dropped rather than folded together"
    );
}

#[test]
fn lowering_the_detail_does_not_disturb_what_was_already_sent() {
    let mut link = Link::open();
    link.show(10, 10, &[size().bounds()]);
    let before = link.shown(0, 100);
    link.host.set_limits(Limits {
        lossy_detail: LossyDetail::LOW,
        ..Limits::default()
    });
    // Nothing changed on screen, so nothing should be re-sent at the new detail.
    link.show(10, 10, &[size().bounds()]);
    assert_eq!(
        link.shown(0, 100),
        before,
        "a rung change re-sent pixels that had not changed"
    );
}

#[test]
fn a_session_starts_at_the_budget_its_config_asked_for() {
    let host = HostSession::new(
        vec![SurfaceInfo {
            id: SURFACE,
            size: size(),
            scale_milli: 1000,
            name: "Built-in Display".to_owned(),
        }],
        Tickets,
        HostConfig {
            refine_budget_bytes: 5_000,
            ..HostConfig::default()
        },
    );
    assert_eq!(host.limits().refine_budget_bytes, 5_000);
    assert!(!host.limits().viewport_only);
    assert_eq!(host.limits().minimum_interval_ms, 0);
}

#[test]
fn the_viewport_clip_does_nothing_when_the_viewer_never_said_where_it_is_looking() {
    let mut link = Link::open();
    link.show(10, 10, &[size().bounds()]);
    // Viewport-only, but no viewport has ever been reported. Clipping to an unknown rectangle
    // would send nothing at all, which is worse than sending everything.
    link.host.set_limits(Limits {
        viewport_only: true,
        ..Limits::default()
    });
    link.show(90, 200, &[size().bounds()]);
    assert_eq!(link.shown(0, 100), 90);
    assert_eq!(
        link.shown(0, 380),
        200,
        "with no viewport to clip to, the whole screen still arrives"
    );
}

#[test]
fn nothing_the_limits_do_ever_stops_a_session_answering() {
    // A session squeezed as hard as the ladder can squeeze it still talks: control messages are
    // not on the ladder, so a viewer asking for something always gets an answer.
    let mut link = Link::open();
    link.host.set_limits(Limits {
        refine_budget_bytes: 0,
        viewport_only: true,
        minimum_interval_ms: 1_000,
        lossy_detail: LossyDetail::LOW,
    });
    link.viewer.request_control();
    link.pump();
    let answered = link
        .host
        .poll_outgoing()
        .is_none_or(|message| !matches!(message, Message::Batch(_)));
    assert!(answered, "the session stopped answering under its limits");
}
