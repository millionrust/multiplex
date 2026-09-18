//! Terminal panes drawn from their text stream are kept out of the pixel stream.

use termirust_screen_codec::{FrameBuffer, Rect, Size, downscale};
use termirust_screen_protocol::{
    FrameReader, Message, PanePlacement, PaneSession, Profile, SurfaceInfo, encode_frame,
};
use termirust_screen_session::{
    Grants, HostConfig, HostSession, PANE_MASK_BGRA, ResumeStore, TicketVerifier, ViewerEvent,
    ViewerSession,
};

const TICKET: [u8; 32] = [7; 32];
const SHELL: PaneSession = [0x5E; 16];
const LOGS: PaneSession = [0x10; 16];

struct Tickets;

impl TicketVerifier for Tickets {
    fn verify(&mut self, proof: &[u8; 32]) -> Option<Grants> {
        (*proof == TICKET).then_some(Grants {
            device: 42,
            can_view: true,
            can_control: false,
        })
    }
}

fn size() -> Size {
    Size::new(640, 400).unwrap()
}

fn surfaces() -> Vec<SurfaceInfo> {
    vec![SurfaceInfo {
        id: 1,
        size: size(),
        scale_milli: 2000,
        name: "Built-in Display".to_owned(),
    }]
}

fn pane(session: PaneSession, rect: Rect) -> PanePlacement {
    PanePlacement {
        session,
        rect,
        cell_width: 8,
        cell_height: 18,
    }
}

/// A desktop with a sidebar, and a terminal window at `terminal` whose output grows with `lines`.
fn desktop(terminal: Rect, lines: u32) -> FrameBuffer {
    let mut buffer = FrameBuffer::new(size());
    buffer
        .fill_rect(size().bounds(), [240, 236, 232, 255])
        .unwrap();
    buffer
        .fill_rect(Rect::new(0, 0, 120, 400), [60, 52, 48, 255])
        .unwrap();
    for row in 0..12 {
        buffer
            .fill_rect(Rect::new(12, 16 + row * 30, 90, 10), [200, 196, 190, 255])
            .unwrap();
    }
    buffer.fill_rect(terminal, [35, 30, 28, 255]).unwrap();
    for line in 0..lines.min(terminal.height / 18) {
        for column in 0..(line * 7 + 11) % (terminal.width / 8) {
            if (column + line) % 4 != 0 {
                buffer
                    .fill_rect(
                        Rect::new(
                            terminal.x + 2 + column * 8,
                            terminal.y + 3 + line * 18,
                            6,
                            12,
                        ),
                        [212, 205, 201, 255],
                    )
                    .unwrap();
            }
        }
    }
    buffer
}

fn masked(mut frame: FrameBuffer, rects: &[Rect]) -> FrameBuffer {
    for rect in rects {
        frame.fill_rect(*rect, PANE_MASK_BGRA).unwrap();
    }
    frame
}

struct Link {
    host: HostSession<Tickets>,
    viewer: ViewerSession,
    store: ResumeStore,
    events: Vec<ViewerEvent>,
    bytes: usize,
}

impl Link {
    fn new(viewer: ViewerSession) -> Self {
        Self {
            host: HostSession::new(surfaces(), Tickets, HostConfig::default()),
            viewer,
            store: ResumeStore::default(),
            events: Vec::new(),
            bytes: 0,
        }
    }

    fn open(attached: &[PaneSession], profile: Profile) -> Self {
        let mut viewer = ViewerSession::new(64 << 20);
        viewer.connect(TICKET);
        viewer.attach_panes(attached.to_vec());
        let mut link = Self::new(viewer);
        link.viewer.subscribe(1, profile);
        link.pump();
        link
    }

    fn pump(&mut self) {
        loop {
            let mut moved = false;
            let mut reader = FrameReader::new();
            while let Some(message) = self.host.poll_outgoing() {
                let frame = encode_frame(&message).unwrap();
                self.bytes += frame.len();
                reader.push(&frame);
                moved = true;
            }
            while let Some(message) = reader.next_message().unwrap() {
                self.events.extend(self.viewer.receive(message).unwrap());
            }
            while let Some(message) = self.viewer.poll_outgoing() {
                self.host.receive(message, &mut self.store).unwrap();
                moved = true;
            }
            if !moved {
                return;
            }
        }
    }

    fn show(&mut self, frame: &FrameBuffer, now_ms: u64) {
        self.host.frame(1, &frame.as_frame(), None, now_ms).unwrap();
        self.pump();
    }
}

const TERMINAL: Rect = Rect::new(200, 96, 400, 270);

#[test]
fn an_attached_pane_costs_nothing_while_its_text_changes() {
    let mut costs = Vec::new();
    for attached in [&[SHELL][..], &[]] {
        let mut link = Link::open(attached, Profile::Interactive);
        link.host.set_panes(1, vec![pane(SHELL, TERMINAL)]);
        link.show(&desktop(TERMINAL, 1), 0);
        let first = link.bytes;
        for lines in 2..16 {
            let frame = desktop(TERMINAL, lines);
            link.show(&frame, u64::from(lines) * 100);
            let expected = if attached.is_empty() {
                frame
            } else {
                masked(frame, &[TERMINAL])
            };
            assert_eq!(link.viewer.framebuffer(1).unwrap(), &expected);
        }
        assert_eq!(link.viewer.panes(1), [pane(SHELL, TERMINAL)]);
        costs.push(link.bytes - first);
    }
    let [attached, pixels] = costs[..] else {
        unreachable!()
    };
    assert!(pixels > 2_000, "typing as pixels took {pixels} bytes");
    assert!(
        attached * 20 < pixels,
        "typing in an attached pane took {attached} bytes, as pixels {pixels}"
    );
}

#[test]
fn placements_arrive_before_the_masked_pixels() {
    let mut viewer = ViewerSession::new(64 << 20);
    viewer.attach_panes(vec![SHELL]);
    viewer.connect(TICKET);
    let mut link = Link::new(viewer);
    link.host.set_panes(1, vec![pane(SHELL, TERMINAL)]);
    link.viewer.subscribe(1, Profile::Interactive);
    link.pump();
    link.show(&desktop(TERMINAL, 4), 0);

    let panes = link
        .events
        .iter()
        .position(|event| matches!(event, ViewerEvent::Panes { .. }))
        .expect("placements were sent on subscribe");
    let pixels = link
        .events
        .iter()
        .position(|event| matches!(event, ViewerEvent::Updated { .. }))
        .unwrap();
    assert!(panes < pixels);
    assert_eq!(
        link.viewer.framebuffer(1).unwrap(),
        &masked(desktop(TERMINAL, 4), &[TERMINAL])
    );
}

#[test]
fn moving_or_detaching_a_pane_restores_the_pixels_it_uncovered() {
    let mut link = Link::open(&[SHELL, LOGS], Profile::Interactive);
    link.host.set_panes(1, vec![pane(SHELL, TERMINAL)]);
    link.show(&desktop(TERMINAL, 6), 0);

    let moved = Rect::new(150, 40, 400, 270);
    link.host.set_panes(1, vec![pane(SHELL, moved)]);
    link.show(&desktop(moved, 6), 100);
    assert_eq!(
        link.viewer.framebuffer(1).unwrap(),
        &masked(desktop(moved, 6), &[moved])
    );
    assert!(link.events.iter().any(
        |event| matches!(event, ViewerEvent::Panes { panes, .. } if panes == &[pane(SHELL, moved)])
    ));

    // A pane that hangs off the surface is masked only where it is visible.
    let edge = Rect::new(500, 300, 400, 270);
    link.host
        .set_panes(1, vec![pane(SHELL, moved), pane(LOGS, edge)]);
    link.show(&desktop(moved, 6), 200);
    let visible = Rect::new(500, 300, 140, 100);
    assert_eq!(link.host.masked_rects(1), [moved, visible]);
    assert_eq!(
        link.viewer.framebuffer(1).unwrap(),
        &masked(desktop(moved, 6), &[moved, visible])
    );

    link.viewer.attach_panes(Vec::new());
    link.pump();
    link.show(&desktop(moved, 6), 300);
    assert_eq!(link.viewer.framebuffer(1).unwrap(), &desktop(moved, 6));
}

#[test]
fn attachments_survive_a_reconnect() {
    let mut link = Link::open(&[SHELL], Profile::Interactive);
    link.host.set_panes(1, vec![pane(SHELL, TERMINAL)]);
    link.show(&desktop(TERMINAL, 3), 0);

    let Link {
        mut host,
        mut viewer,
        mut store,
        ..
    } = link;
    host.close("connection_lost", &mut store);
    viewer.disconnected();
    viewer.connect(TICKET);

    let mut again = Link::new(viewer);
    again.store = store;
    again.host.set_panes(1, vec![pane(SHELL, TERMINAL)]);
    again.viewer.subscribe(1, Profile::Interactive);
    again.pump();
    assert_eq!(
        again.host.masked_rects(1),
        [TERMINAL],
        "the new connection heard which panes the viewer draws"
    );
    assert_eq!(again.viewer.panes(1), [pane(SHELL, TERMINAL)]);
    again.show(&desktop(TERMINAL, 9), 100);
    assert_eq!(
        again.viewer.framebuffer(1).unwrap(),
        &masked(desktop(TERMINAL, 9), &[TERMINAL])
    );
}

#[test]
fn previews_show_the_pane_as_pixels() {
    let mut link = Link::open(&[SHELL], Profile::Thumbnail);
    link.host.set_panes(1, vec![pane(SHELL, TERMINAL)]);
    let frame = desktop(TERMINAL, 8);
    link.show(&frame, 0);
    assert!(
        !link
            .events
            .iter()
            .any(|event| matches!(event, ViewerEvent::Panes { .. })),
        "a preview needs no placements"
    );
    let expected = downscale(&frame.as_frame(), 2);
    assert_eq!(link.viewer.preview(1).unwrap(), &expected);
}

#[test]
fn a_viewer_cannot_publish_placements() {
    let mut link = Link::open(&[], Profile::Interactive);
    let result = link.host.receive(
        Message::PanePlacements {
            surface: 1,
            panes: vec![pane(SHELL, TERMINAL)],
        },
        &mut link.store,
    );
    assert!(result.is_err());
    assert!(!link.host.is_open());
}
