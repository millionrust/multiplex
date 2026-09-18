//! A host and a viewer connected through real protocol framing, in one process.

use termirust_screen_codec::{FrameBuffer, Rect, Size};
use termirust_screen_protocol::{
    ControlHolder, FrameReader, KeyEvent, Message, Modifiers, PointerButton, Profile,
    ResumeOutcome, SurfaceInfo, encode_frame,
};
use termirust_screen_session::{
    Grants, HostConfig, HostEvent, HostSession, InputEvent, ResumeStore, SessionError,
    TicketVerifier, ViewerEvent, ViewerSession,
};

const CONTROL_TICKET: [u8; 32] = [7; 32];
const VIEW_TICKET: [u8; 32] = [8; 32];
const CACHE: usize = 64 << 20;

struct Tickets;

impl TicketVerifier for Tickets {
    fn verify(&mut self, proof: &[u8; 32]) -> Option<Grants> {
        match *proof {
            CONTROL_TICKET => Some(Grants {
                device: 42,
                can_view: true,
                can_control: true,
            }),
            VIEW_TICKET => Some(Grants {
                device: 43,
                can_view: true,
                can_control: false,
            }),
            _ => None,
        }
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

/// A dark editor with text lines, scrolled to `line`, with `typed` characters on line three.
fn screen(line: u32, typed: u32) -> FrameBuffer {
    let mut buffer = FrameBuffer::new(size());
    buffer
        .fill_rect(size().bounds(), [35, 30, 28, 255])
        .unwrap();
    for row in 0..22 {
        let y = 4 + row * 18;
        let length = if row == 3 {
            8 * typed
        } else {
            (row + line) * 53 % 500 + 40
        };
        for x in (20..20 + length.min(600)).step_by(8) {
            if !(x / 8 + row + line).is_multiple_of(3) {
                buffer
                    .fill_rect(Rect::new(x, y, 6, 12), [212, 205, 201, 255])
                    .unwrap();
            }
        }
    }
    buffer
}

struct Link {
    host: HostSession<Tickets>,
    viewer: ViewerSession,
    store: ResumeStore,
    host_events: Vec<HostEvent>,
    viewer_events: Vec<ViewerEvent>,
    host_errors: Vec<SessionError>,
    bytes_to_viewer: usize,
}

impl Link {
    fn new(viewer: ViewerSession, store: ResumeStore) -> Self {
        Self {
            host: HostSession::new(surfaces(), Tickets, HostConfig::default()),
            viewer,
            store,
            host_events: Vec::new(),
            viewer_events: Vec::new(),
            host_errors: Vec::new(),
            bytes_to_viewer: 0,
        }
    }

    /// Moves messages both ways through framing until both outboxes are empty.
    fn pump(&mut self) {
        loop {
            let mut moved = false;
            let mut to_viewer = FrameReader::new();
            while let Some(message) = self.host.poll_outgoing() {
                let frame = encode_frame(&message).unwrap();
                self.bytes_to_viewer += frame.len();
                to_viewer.push(&frame);
                moved = true;
            }
            while let Some(message) = to_viewer.next_message().unwrap() {
                if let Ok(events) = self.viewer.receive(message) {
                    self.viewer_events.extend(events);
                }
            }
            let mut to_host = FrameReader::new();
            while let Some(message) = self.viewer.poll_outgoing() {
                to_host.push(&encode_frame(&message).unwrap());
                moved = true;
            }
            while let Some(message) = to_host.next_message().unwrap() {
                match self.host.receive(message, &mut self.store) {
                    Ok(events) => self.host_events.extend(events),
                    Err(error) => self.host_errors.push(error),
                }
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

fn connected(ticket: [u8; 32]) -> Link {
    let mut viewer = ViewerSession::new(CACHE);
    viewer.connect(ticket);
    let mut link = Link::new(viewer, ResumeStore::default());
    link.pump();
    link
}

#[test]
fn a_rejected_ticket_closes_both_sides() {
    let mut link = connected([1; 32]);
    assert_eq!(link.host_errors, vec![SessionError::NotAuthorized]);
    assert_eq!(
        link.viewer_events,
        vec![ViewerEvent::Closed {
            reason: "ticket_rejected".to_owned()
        }]
    );
    assert!(!link.host.is_open());
    link.host
        .frame(1, &screen(0, 0).as_frame(), None, 0)
        .unwrap();
    assert!(
        link.host.poll_outgoing().is_none(),
        "a closed host sends no pixels"
    );
}

#[test]
fn an_interactive_view_stays_pixel_exact() {
    let mut link = connected(CONTROL_TICKET);
    assert!(matches!(
        link.viewer_events[0],
        ViewerEvent::Welcomed {
            resume: ResumeOutcome::NotRequested,
            ..
        }
    ));
    link.viewer.subscribe(1, Profile::Interactive);
    link.pump();
    let mut now = 0;
    for (line, typed) in [(0, 0), (0, 1), (0, 2), (3, 2), (3, 2), (1, 9)] {
        let frame = screen(line, typed);
        link.show(&frame, now);
        assert_eq!(
            link.viewer.framebuffer(1).unwrap(),
            &frame,
            "line {line}, typed {typed}"
        );
        now += 33;
    }
}

#[test]
fn previews_are_small_and_at_most_once_a_second() {
    let mut link = connected(VIEW_TICKET);
    link.viewer.subscribe(1, Profile::Thumbnail);
    link.pump();
    let mut updates = 0;
    for step in 0..30u32 {
        let before = link.viewer_events.len();
        link.show(&screen(step, step), u64::from(step) * 100);
        updates += link.viewer_events[before..]
            .iter()
            .filter(|event| matches!(event, ViewerEvent::Updated { preview: true, .. }))
            .count();
    }
    let preview = link.viewer.preview(1).expect("a preview arrived");
    assert!(preview.size().width() <= 320 && preview.size().height() <= 320);
    assert!(
        link.viewer.framebuffer(1).is_none(),
        "a preview subscription sends no full view"
    );
    assert!(
        (3..=3).contains(&updates),
        "previews updated {updates} times in 3 seconds"
    );
}

#[test]
fn input_is_injected_only_while_the_viewer_holds_control() {
    let key = InputEvent::Key(KeyEvent {
        usage: 0x04,
        modifiers: Modifiers::new(0).unwrap(),
        pressed: true,
    });

    let mut watcher = connected(VIEW_TICKET);
    watcher.viewer.request_control();
    watcher.viewer.send_input(key.clone());
    watcher.pump();
    assert_eq!(
        watcher
            .host_events
            .iter()
            .filter(|e| **e == HostEvent::InputRefused)
            .count(),
        2
    );
    assert!(
        !watcher
            .host_events
            .iter()
            .any(|e| matches!(e, HostEvent::Input(_) | HostEvent::ControlRequested))
    );

    let mut link = connected(CONTROL_TICKET);
    link.viewer.send_input(key.clone());
    link.pump();
    assert!(
        link.host_events.contains(&HostEvent::InputRefused),
        "control not yet granted"
    );

    link.viewer.request_control();
    link.pump();
    assert!(link.host_events.contains(&HostEvent::ControlRequested));
    link.host.set_control(ControlHolder::You);
    link.pump();
    assert_eq!(link.viewer.control(), ControlHolder::You);

    let click = InputEvent::PointerButton {
        surface: 1,
        x: 10,
        y: 20,
        button: PointerButton::Primary,
        pressed: true,
    };
    link.viewer.send_input(key.clone());
    link.viewer.send_input(click.clone());
    link.pump();
    assert!(link.host_events.contains(&HostEvent::Input(key)));
    assert!(link.host_events.contains(&HostEvent::Input(click)));

    link.viewer.release_control();
    link.pump();
    assert_eq!(link.viewer.control(), ControlHolder::Nobody);
}

#[test]
fn a_slow_viewer_is_not_flooded_and_catches_up_exactly() {
    let mut link = connected(CONTROL_TICKET);
    link.viewer.subscribe(1, Profile::Interactive);
    link.pump();
    let mut queued = Vec::new();
    for step in 0..20u32 {
        link.host
            .frame(1, &screen(step, 0).as_frame(), None, u64::from(step) * 33)
            .unwrap();
        while let Some(message) = link.host.poll_outgoing() {
            queued.push(message);
        }
    }
    let batches = queued
        .iter()
        .filter(|m| matches!(m, Message::Batch(_)))
        .count();
    assert!(
        batches <= HostConfig::default().min_unacked_batches as usize,
        "{batches} batches without acknowledgement"
    );

    for message in queued {
        let events = link.viewer.receive(message).unwrap();
        link.viewer_events.extend(events);
    }
    link.pump();
    let last = screen(19, 0);
    link.show(&last, 20 * 33);
    link.show(&last, 21 * 33);
    assert_eq!(link.viewer.framebuffer(1).unwrap(), &last);
}

/// How much a session gets out in a second, on links that differ in nothing but latency.
///
/// The window is how many batches may be unacknowledged at once, and a fixed one is wrong at both
/// ends of the range it has to cover. Four is about one round trip's worth at 130 ms; at 300 ms a
/// session allowed only four spends most of its time idle with the link empty, because every batch
/// has to be acknowledged before another may go and an acknowledgement takes a third of a second
/// to come back. That is not congestion, that is the sender waiting.
///
/// So this measures rather than asserts a rule: same frames, same screen, same everything, two
/// latencies, count what actually left. With the window fixed at four the slow column reads 12 —
/// four batches per round trip, three round trips in a second — and those are frames the link had
/// the capacity to carry and never saw.
///
/// The fast column is the control. A window that scales has to *not* scale when there is nothing
/// to scale for: an eight-millisecond link needs no more than the floor, and one that opened up
/// anyway would only be buffering.
#[test]
fn the_send_window_follows_the_round_trip_it_measures() {
    /// Runs a second of 33 ms frames with every message delayed by half of `rtt_ms` each way, and
    /// returns how many batches the host got onto the wire.
    fn batches_in_a_second(rtt_ms: u64) -> usize {
        const FRAME_MS: u64 = 33;
        let mut link = connected(CONTROL_TICKET);
        link.viewer.subscribe(1, Profile::Interactive);
        link.pump();

        let one_way = rtt_ms / 2;
        // Messages in flight, with the time each is allowed to arrive.
        let mut to_viewer: Vec<(Message, u64)> = Vec::new();
        let mut to_host: Vec<(Message, u64)> = Vec::new();
        let mut batches = 0usize;

        for step in 0..30u64 {
            let now = step * FRAME_MS;

            let (arrived, waiting) = to_viewer.into_iter().partition(|(_, at)| *at <= now);
            to_viewer = waiting;
            for (message, _) in arrived {
                if let Ok(events) = link.viewer.receive(message) {
                    link.viewer_events.extend(events);
                }
                while let Some(reply) = link.viewer.poll_outgoing() {
                    to_host.push((reply, now + one_way));
                }
            }

            let (arrived, waiting) = to_host.into_iter().partition(|(_, at)| *at <= now);
            to_host = waiting;
            for (message, _) in arrived {
                let _ = link.host.receive(message, &mut link.store);
            }

            // A screen that keeps changing, so every frame has something to send and a refusal is
            // the window and nothing else.
            link.host
                .frame(1, &screen(step as u32, 0).as_frame(), None, now)
                .unwrap();
            while let Some(message) = link.host.poll_outgoing() {
                if matches!(message, Message::Batch(_)) {
                    batches += 1;
                }
                to_viewer.push((message, now + one_way));
            }
        }
        batches
    }

    let fast = batches_in_a_second(8);
    let slow = batches_in_a_second(300);

    // The fast link is limited by the frame rate, not the window: 30 frames offered, near 30 sent.
    // It needs no more than the floor and must not be given more.
    assert!(
        fast >= 25,
        "a fast link should be limited by the frame rate, not the window: {fast} batches"
    );

    // The slow link cannot match that — a round trip is nine frames long, so acknowledgements are
    // genuinely scarce. What it must not do is sit at the fixed floor. At 300 ms and 33 ms frames
    // the window opens to about ten, which is most of those frames back.
    assert!(
        slow >= 20,
        "a 300 ms link should still get most frames out once the window scales: {slow} batches"
    );

    // The mechanism, not just the outcome. This is the assertion that fails at 12 when the ceiling
    // is pinned back to four, which is how the test was shown to be measuring anything at all.
    assert!(
        slow > 4 * 3 + 2,
        "the slow link is still capped by a fixed window: {slow} batches"
    );
}

#[test]
fn a_returning_viewer_resumes_without_a_full_refresh() {
    let mut link = connected(CONTROL_TICKET);
    link.viewer.subscribe(1, Profile::Interactive);
    link.pump();
    link.show(&screen(0, 0), 0);
    let first_view = link.bytes_to_viewer;
    link.show(&screen(0, 4), 33);

    // The connection drops while the host has already encoded more changes.
    link.host
        .frame(1, &screen(0, 8).as_frame(), None, 66)
        .unwrap();
    while link.host.poll_outgoing().is_some() {}
    let Link {
        mut host,
        mut viewer,
        mut store,
        ..
    } = link;
    host.close("connection_lost", &mut store);
    viewer.disconnected();
    assert_eq!(store.len(), 1);

    viewer.connect(CONTROL_TICKET);
    let mut again = Link::new(viewer, store);
    again.pump();
    assert!(again.viewer_events.contains(&ViewerEvent::Welcomed {
        surfaces: surfaces(),
        resume: ResumeOutcome::Partial
    }));
    again.viewer.subscribe(1, Profile::Interactive);
    again.pump();
    let current = screen(0, 9);
    again.show(&current, 99);
    assert_eq!(again.viewer.framebuffer(1).unwrap(), &current);
    assert!(
        again.bytes_to_viewer * 4 < first_view,
        "resume took {} bytes after a {first_view}-byte first view",
        again.bytes_to_viewer
    );
}

#[test]
fn viewers_cannot_send_host_messages() {
    let mut link = connected(CONTROL_TICKET);
    link.host
        .receive(Message::Control(ControlHolder::You), &mut link.store)
        .unwrap_err();
    assert!(!link.host.is_open());
}
