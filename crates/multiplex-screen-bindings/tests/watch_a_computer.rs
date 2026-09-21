//! The phone boundary against a real host session: pixels in, drawn rectangles out.

use multiplex_screen_codec::{FrameBuffer, Rect, Size};
use multiplex_screen_protocol::{FrameReader, SurfaceInfo, encode_frame};
use multiplex_screen_session::{
    Grants, HostConfig, HostEvent, HostSession, ResumeStore, TicketVerifier,
};
use multiplex_screen_bindings::{
    ScreenBindingError, ScreenCapability, ScreenEvent, ScreenPointerButton, ScreenRect,
    ScreenViewer,
};

const TICKET: [u8; 32] = [7; 32];

struct Tickets;

impl TicketVerifier for Tickets {
    fn verify(&mut self, proof: &[u8; 32]) -> Option<Grants> {
        (*proof == TICKET).then_some(Grants {
            device: 1,
            can_view: true,
            can_control: true,
        })
    }
}

fn size() -> Size {
    Size::new(320, 200).unwrap()
}

fn screen(step: u32) -> FrameBuffer {
    let mut buffer = FrameBuffer::new(size());
    buffer
        .fill_rect(size().bounds(), [30, 28, 26, 255])
        .unwrap();
    buffer
        .fill_rect(Rect::new(20, 20 + step * 4, 120, 40), [200, 180, 40, 255])
        .unwrap();
    buffer
}

/// Moves everything the viewer queued into the host, and everything the host queued back.
fn pump(
    host: &mut HostSession<Tickets>,
    viewer: &ScreenViewer,
    store: &mut ResumeStore,
) -> Vec<HostEvent> {
    let mut events = Vec::new();
    loop {
        let mut moved = false;
        let mut to_host = FrameReader::new();
        while let Some(outgoing) = viewer.poll_outgoing() {
            to_host.push(&outgoing.bytes);
            moved = true;
        }
        while let Some(message) = to_host.next_message().unwrap() {
            events.extend(host.receive(message, store).unwrap());
        }
        let mut bytes = Vec::new();
        while let Some(message) = host.poll_outgoing() {
            bytes.extend(encode_frame(&message).unwrap());
            moved = true;
        }
        if !bytes.is_empty() {
            viewer.receive(bytes).unwrap();
        }
        if !moved {
            return events;
        }
    }
}

#[test]
fn a_phone_watches_a_computer_and_draws_only_what_changed() {
    let surfaces = vec![SurfaceInfo {
        id: 1,
        size: size(),
        scale_milli: 2000,
        name: "Built-in Display".to_owned(),
    }];
    let mut host = HostSession::new(surfaces, Tickets, HostConfig::default());
    let mut store = ResumeStore::default();
    let viewer = ScreenViewer::new(16 << 20);

    viewer.connect(TICKET.to_vec()).unwrap();
    pump(&mut host, &viewer, &mut store);
    viewer.subscribe(1, false);
    pump(&mut host, &viewer, &mut store);

    let first = screen(0);
    host.frame(1, &first.as_frame(), None, 0).unwrap();
    pump(&mut host, &viewer, &mut store);
    assert_eq!(
        viewer.surface_size(1, false),
        Some(ScreenRect {
            x: 0,
            y: 0,
            width: 320,
            height: 200
        })
    );

    // Every pixel the computer sent is the pixel it captured.
    let whole = ScreenRect {
        x: 0,
        y: 0,
        width: 320,
        height: 200,
    };
    let copied = viewer.copy_pixels(1, false, whole).unwrap();
    assert_eq!(copied.rect, whole);
    assert_eq!(copied.bgra.len(), 320 * 200 * 4);
    let expected: Vec<u8> = (0..200)
        .flat_map(|y| first.as_frame().row(y).to_vec())
        .collect();
    assert_eq!(copied.bgra, expected);

    // A small change reports a small damaged area, and a phone draws just that.
    let moved = screen(3);
    host.frame(1, &moved.as_frame(), None, 100).unwrap();
    let mut damaged = Vec::new();
    let mut bytes = Vec::new();
    while let Some(message) = host.poll_outgoing() {
        bytes.extend(encode_frame(&message).unwrap());
    }
    for event in viewer.receive(bytes).unwrap() {
        if let ScreenEvent::Updated {
            damaged: rects,
            reset,
            preview,
            surface,
        } = event
        {
            assert_eq!(surface, 1);
            assert!(!preview && !reset);
            damaged.extend(rects);
        }
    }
    assert!(!damaged.is_empty(), "the move was reported");
    let touched: u32 = damaged.iter().map(|rect| rect.width * rect.height).sum();
    assert!(
        touched < 320 * 200,
        "a moved box should not repaint the screen: {touched} pixels"
    );
    for rect in damaged {
        let after = viewer.copy_pixels(1, false, rect).unwrap();
        let mut expected = Vec::new();
        for y in rect.y..rect.y + rect.height {
            let row = moved.as_frame().row(y);
            let start = rect.x as usize * 4;
            expected.extend_from_slice(&row[start..start + rect.width as usize * 4]);
        }
        assert_eq!(after.bgra, expected, "rect {rect:?}");
    }
}

#[test]
fn control_and_input_reach_the_computer_only_after_it_is_given() {
    let surfaces = vec![SurfaceInfo {
        id: 1,
        size: size(),
        scale_milli: 2000,
        name: "Built-in Display".to_owned(),
    }];
    let mut host = HostSession::new(surfaces, Tickets, HostConfig::default());
    let mut store = ResumeStore::default();
    let viewer = ScreenViewer::new(16 << 20);
    viewer.connect(TICKET.to_vec()).unwrap();
    pump(&mut host, &viewer, &mut store);

    viewer.send_pointer_button(1, 10, 10, ScreenPointerButton::Primary, true);
    let refused = pump(&mut host, &viewer, &mut store);
    assert!(refused.contains(&HostEvent::InputRefused));
    assert!(
        !refused
            .iter()
            .any(|event| matches!(event, HostEvent::Input(_)))
    );

    viewer.request_control();
    let asked = pump(&mut host, &viewer, &mut store);
    assert!(asked.contains(&HostEvent::ControlRequested));
    host.set_control(multiplex_screen_protocol::ControlHolder::You);
    pump(&mut host, &viewer, &mut store);
    assert_eq!(
        viewer.control(),
        multiplex_screen_bindings::ScreenControlHolder::You
    );

    viewer.send_pointer_button(1, 10, 10, ScreenPointerButton::Primary, true);
    let clicked = pump(&mut host, &viewer, &mut store);
    assert!(
        clicked
            .iter()
            .any(|event| matches!(event, HostEvent::Input(_)))
    );
}

#[test]
fn a_preview_subscription_stays_small_and_separate_from_the_full_view() {
    let surfaces = vec![SurfaceInfo {
        id: 1,
        size: size(),
        scale_milli: 2000,
        name: "Built-in Display".to_owned(),
    }];
    let mut host = HostSession::new(surfaces, Tickets, HostConfig::default());
    let mut store = ResumeStore::default();
    let viewer = ScreenViewer::new(16 << 20);
    viewer.connect(TICKET.to_vec()).unwrap();
    pump(&mut host, &viewer, &mut store);
    viewer.subscribe(1, true);
    pump(&mut host, &viewer, &mut store);
    host.frame(1, &screen(0).as_frame(), None, 0).unwrap();
    pump(&mut host, &viewer, &mut store);

    let preview = viewer.surface_size(1, true).expect("a preview arrived");
    assert!(preview.width <= 320 && preview.height <= 200);
    assert_eq!(
        viewer.surface_size(1, false),
        None,
        "a preview is not the full view"
    );
    assert_eq!(
        viewer.copy_pixels(1, false, preview),
        Err(ScreenBindingError::Closed),
        "there is nothing to copy from a view that was never subscribed"
    );
    assert_eq!(
        viewer.poll_outgoing().map(|out| out.capability),
        None,
        "watching queues nothing once acknowledgements are sent"
    );
    assert!(viewer.copy_pixels(1, true, preview).is_ok());
    let _ = ScreenCapability::Observe;
}
