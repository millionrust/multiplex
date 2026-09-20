//! The injector against a recording sink, and driven by a real host session.

use multiplex_screen_codec::Size;
use multiplex_screen_input::{
    DOUBLE_CLICK_MS, DisplayLayout, DisplayPlacement, Injector, InputError, InputSink, Point,
    RecordingSink, SinkEvent,
};
use multiplex_screen_protocol::{ControlHolder, KeyEvent, Modifiers, PointerButton, SurfaceInfo};
use multiplex_screen_session::{
    Grants, HostConfig, HostEvent, HostSession, InputEvent, ResumeStore, THUMBNAIL_SURFACE_BIT,
    TicketVerifier, ViewerSession,
};

const MAIN: u32 = 1;
const LEFT: u32 = 2;
const PHONE: u64 = 42;
const LAPTOP: u64 = 43;

/// A 1512×982-point Retina display captured at 3024×1964, and a 1920×1080 display to its left.
fn layout() -> DisplayLayout {
    DisplayLayout::new(vec![
        DisplayPlacement::new(
            MAIN,
            Size::new(3024, 1964).unwrap(),
            (0, 0),
            Size::new(1512, 982).unwrap(),
        ),
        DisplayPlacement::new(
            LEFT,
            Size::new(1920, 1080).unwrap(),
            (-1920, -98),
            Size::new(1920, 1080).unwrap(),
        ),
    ])
}

fn injector(holder: u64) -> Injector<RecordingSink> {
    let mut injector = Injector::new(RecordingSink::default(), layout());
    injector.set_holder(Some(holder)).unwrap();
    injector
}

fn key(usage: u16, modifiers: u8, pressed: bool) -> InputEvent {
    InputEvent::Key(KeyEvent {
        usage,
        modifiers: Modifiers::new(modifiers).unwrap(),
        pressed,
    })
}

fn click(x: u32, y: u32, pressed: bool) -> InputEvent {
    InputEvent::PointerButton {
        surface: MAIN,
        x,
        y,
        button: PointerButton::Primary,
        pressed,
    }
}

fn events(injector: &Injector<RecordingSink>) -> &[SinkEvent] {
    &injector.sink().events
}

fn at(x: f64, y: f64) -> Point {
    Point { x, y }
}

#[test]
fn only_the_lease_holder_can_inject() {
    let mut injector = Injector::new(RecordingSink::default(), layout());
    let press = key(0x04, 0, true);
    assert_eq!(
        injector.inject(PHONE, &press, 0),
        Err(InputError::NotHolder)
    );

    injector.set_holder(Some(PHONE)).unwrap();
    assert_eq!(
        injector.inject(LAPTOP, &press, 0),
        Err(InputError::NotHolder)
    );
    injector.inject(PHONE, &press, 0).unwrap();
    assert_eq!(events(&injector).len(), 1);
}

#[test]
fn surface_pixels_land_on_the_right_display_point() {
    let mut injector = injector(PHONE);
    let moves = [
        (MAIN, 0, 0),
        (MAIN, 3000, 1000),
        (MAIN, 9000, 9000),
        (LEFT, 960, 540),
    ];
    for (surface, x, y) in moves {
        injector
            .inject(PHONE, &InputEvent::PointerMove { surface, x, y }, 0)
            .unwrap();
    }
    let points: Vec<Point> = events(&injector)
        .iter()
        .map(|event| match event {
            SinkEvent::PointerMove { at, held: None, .. } => *at,
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert_eq!(
        points,
        [
            at(0.0, 0.0),
            at(1500.0, 500.0),
            at(1511.5, 981.5),
            at(-960.0, 442.0)
        ]
    );

    for surface in [3, MAIN | THUMBNAIL_SURFACE_BIT] {
        assert_eq!(
            injector.inject(
                PHONE,
                &InputEvent::PointerMove {
                    surface,
                    x: 1,
                    y: 1
                },
                0
            ),
            Err(InputError::UnknownSurface)
        );
    }
}

#[test]
fn a_drag_moves_with_the_button_held_and_releases_once() {
    let mut injector = injector(PHONE);
    injector.inject(PHONE, &click(100, 100, true), 0).unwrap();
    injector
        .inject(
            PHONE,
            &InputEvent::PointerMove {
                surface: MAIN,
                x: 300,
                y: 100,
            },
            16,
        )
        .unwrap();
    injector.inject(PHONE, &click(300, 100, false), 32).unwrap();
    injector.inject(PHONE, &click(300, 100, false), 48).unwrap();
    let none = Modifiers::default();
    assert_eq!(
        events(&injector),
        [
            SinkEvent::PointerMove {
                at: at(50.0, 50.0),
                held: None,
                modifiers: none
            },
            SinkEvent::PointerButton {
                at: at(50.0, 50.0),
                button: PointerButton::Primary,
                pressed: true,
                clicks: 1,
                modifiers: none
            },
            SinkEvent::PointerMove {
                at: at(150.0, 50.0),
                held: Some(PointerButton::Primary),
                modifiers: none
            },
            SinkEvent::PointerButton {
                at: at(150.0, 50.0),
                button: PointerButton::Primary,
                pressed: false,
                clicks: 1,
                modifiers: none
            },
        ]
    );
}

#[test]
fn quick_nearby_presses_count_as_double_and_triple_clicks() {
    let mut injector = injector(PHONE);
    let mut now = 0;
    let mut press_at = |injector: &mut Injector<RecordingSink>, x: u32, gap: u64| {
        now += gap;
        injector.inject(PHONE, &click(x, 200, true), now).unwrap();
        injector
            .inject(PHONE, &click(x, 200, false), now + 40)
            .unwrap();
    };
    press_at(&mut injector, 200, 0);
    press_at(&mut injector, 204, 200);
    press_at(&mut injector, 204, 200);
    press_at(&mut injector, 204, DOUBLE_CLICK_MS + 1);
    press_at(&mut injector, 240, 100);
    let clicks: Vec<u8> = events(&injector)
        .iter()
        .filter_map(|event| match event {
            SinkEvent::PointerButton {
                pressed: true,
                clicks,
                ..
            } => Some(*clicks),
            _ => None,
        })
        .collect();
    assert_eq!(clicks, [1, 2, 3, 1, 1]);
}

#[test]
fn moving_the_lease_releases_what_the_last_holder_pressed() {
    let mut injector = injector(PHONE);
    injector.inject(PHONE, &key(0xE3, 8, true), 0).unwrap();
    injector.inject(PHONE, &key(0x06, 8, true), 0).unwrap();
    injector.inject(PHONE, &click(10, 10, true), 0).unwrap();
    let before = events(&injector).len();

    injector.set_holder(Some(LAPTOP)).unwrap();
    let none = Modifiers::default();
    let released = &events(&injector)[before..];
    assert_eq!(
        released,
        [
            SinkEvent::Key {
                usage: 0x06,
                pressed: false,
                repeat: false,
                modifiers: none
            },
            SinkEvent::Key {
                usage: 0xE3,
                pressed: false,
                repeat: false,
                modifiers: none
            },
            SinkEvent::PointerButton {
                at: at(5.0, 5.0),
                button: PointerButton::Primary,
                pressed: false,
                clicks: 1,
                modifiers: none
            },
        ]
    );

    // The phone's late release changes nothing, and the laptop starts from a clean state.
    assert_eq!(
        injector.inject(PHONE, &key(0x06, 8, false), 0),
        Err(InputError::NotHolder)
    );
    injector.inject(LAPTOP, &key(0x06, 0, false), 0).unwrap();
    injector.set_holder(None).unwrap();
    assert_eq!(events(&injector).len(), before + 3);
}

#[test]
fn a_held_key_repeats_and_a_stray_release_is_dropped() {
    let mut injector = injector(PHONE);
    injector.inject(PHONE, &key(0x51, 0, true), 0).unwrap();
    injector.inject(PHONE, &key(0x51, 0, true), 30).unwrap();
    injector.inject(PHONE, &key(0x51, 0, false), 60).unwrap();
    injector.inject(PHONE, &key(0x51, 0, false), 90).unwrap();
    let repeats: Vec<(bool, bool)> = events(&injector)
        .iter()
        .map(|event| match event {
            SinkEvent::Key {
                pressed, repeat, ..
            } => (*pressed, *repeat),
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert_eq!(repeats, [(true, false), (true, true), (false, false)]);
    assert_eq!(
        injector.inject(PHONE, &key(0x100, 0, true), 0),
        Err(InputError::UnmappedKey)
    );
}

#[test]
fn modifiers_from_keys_apply_to_clicks() {
    let mut injector = injector(PHONE);
    injector.inject(PHONE, &key(0xE1, 1, true), 0).unwrap();
    injector.inject(PHONE, &click(0, 0, true), 0).unwrap();
    let shift = Modifiers::new(Modifiers::SHIFT).unwrap();
    assert!(events(&injector).iter().any(|event| matches!(
        event,
        SinkEvent::PointerButton { pressed: true, modifiers, .. } if *modifiers == shift
    )));
}

#[test]
fn scrolling_keeps_fractions_of_a_point() {
    let mut injector = injector(PHONE);
    for _ in 0..3 {
        injector
            .inject(
                PHONE,
                &InputEvent::Scroll {
                    surface: MAIN,
                    x: 20,
                    y: 20,
                    dx: 0,
                    dy: -3,
                },
                0,
            )
            .unwrap();
    }
    let scrolls: Vec<(i32, i32)> = events(&injector)
        .iter()
        .filter_map(|event| match event {
            SinkEvent::Scroll { dx, dy, .. } => Some((*dx, *dy)),
            _ => None,
        })
        .collect();
    // 1.5 points each: -1 (rest -0.5), -2 (rest 0), -1 (rest -0.5).
    assert_eq!(scrolls, [(0, -1), (0, -2), (0, -1)]);
}

#[test]
fn text_is_typed_in_chunks_the_platform_accepts() {
    let mut injector = injector(PHONE);
    let text = "echo héllo wörld && ls -la ~/Projects 🚀";
    injector
        .inject(
            PHONE,
            &InputEvent::Text {
                surface: MAIN,
                text: text.to_owned(),
            },
            0,
        )
        .unwrap();
    let chunks: Vec<&str> = events(&injector)
        .iter()
        .map(|event| match event {
            SinkEvent::Text(chunk) => chunk.as_str(),
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert!(chunks.len() > 1);
    assert!(
        chunks
            .iter()
            .all(|chunk| chunk.encode_utf16().count() <= 20)
    );
    assert_eq!(chunks.concat(), text);
}

/// Accepts presses and moves but fails every release, counting the releases it was asked for.
#[derive(Default)]
struct FailingReleases {
    releases: usize,
    fail_presses: bool,
}

impl InputSink for FailingReleases {
    fn post(&mut self, event: SinkEvent) -> Result<(), InputError> {
        match event {
            SinkEvent::Key { pressed: false, .. }
            | SinkEvent::PointerButton { pressed: false, .. } => {
                self.releases += 1;
                Err(InputError::Unavailable)
            }
            SinkEvent::Key { .. } if self.fail_presses => Err(InputError::Unavailable),
            _ => Ok(()),
        }
    }
}

#[test]
fn every_release_is_attempted_when_posting_fails() {
    let mut injector = Injector::new(FailingReleases::default(), layout());
    injector.set_holder(Some(PHONE)).unwrap();
    injector.inject(PHONE, &key(0x04, 0, true), 0).unwrap();
    injector.inject(PHONE, &key(0x05, 0, true), 0).unwrap();
    injector.inject(PHONE, &click(1, 1, true), 0).unwrap();

    assert_eq!(injector.set_holder(None), Err(InputError::Unavailable));
    assert_eq!(injector.sink().releases, 3);
    assert_eq!(injector.holder(), None, "the lease moves even so");

    // Nothing is still tracked as held, so the next change releases nothing.
    injector.set_holder(Some(LAPTOP)).unwrap();
    injector.set_holder(None).unwrap();
    assert_eq!(injector.sink().releases, 3);
}

#[test]
fn a_press_the_platform_refused_is_not_held() {
    let sink = FailingReleases {
        fail_presses: true,
        ..FailingReleases::default()
    };
    let mut injector = Injector::new(sink, layout());
    injector.set_holder(Some(PHONE)).unwrap();
    assert_eq!(
        injector.inject(PHONE, &key(0x04, 0, true), 0),
        Err(InputError::Unavailable)
    );
    injector.set_holder(None).unwrap();
    assert_eq!(injector.sink().releases, 0);
}

struct Tickets;

impl TicketVerifier for Tickets {
    fn verify(&mut self, proof: &[u8; 32]) -> Option<Grants> {
        (*proof == [7; 32]).then_some(Grants {
            device: PHONE,
            can_view: true,
            can_control: true,
        })
    }
}

/// Moves messages between a host and a viewer, feeding permitted input into the injector as a
/// host application would.
fn pump(
    host: &mut HostSession<Tickets>,
    viewer: &mut ViewerSession,
    injector: &mut Injector<RecordingSink>,
    device: &mut Option<u64>,
) {
    let mut store = ResumeStore::default();
    loop {
        let mut moved = false;
        while let Some(message) = viewer.poll_outgoing() {
            moved = true;
            for event in host.receive(message, &mut store).unwrap() {
                match event {
                    HostEvent::Opened { device: opened, .. } => *device = Some(opened),
                    HostEvent::ControlRequested => {
                        host.set_control(ControlHolder::You);
                        injector.set_holder(*device).unwrap();
                    }
                    HostEvent::ControlReleased => injector.set_holder(None).unwrap(),
                    HostEvent::Input(input) => {
                        injector.inject(device.unwrap(), &input, 0).unwrap();
                    }
                    _ => {}
                }
            }
        }
        while let Some(message) = host.poll_outgoing() {
            moved = true;
            viewer.receive(message).unwrap();
        }
        if !moved {
            return;
        }
    }
}

#[test]
fn a_viewer_that_lets_go_of_control_leaves_nothing_pressed() {
    let surfaces = vec![SurfaceInfo {
        id: MAIN,
        size: Size::new(3024, 1964).unwrap(),
        scale_milli: 2000,
        name: "Built-in Display".to_owned(),
    }];
    let mut host = HostSession::new(surfaces, Tickets, HostConfig::default());
    let mut viewer = ViewerSession::new(64 << 20);
    let mut injector = Injector::new(RecordingSink::default(), layout());
    let mut device = None;

    viewer.connect([7; 32]);
    viewer.send_input(click(10, 10, true));
    pump(&mut host, &mut viewer, &mut injector, &mut device);
    assert!(
        events(&injector).is_empty(),
        "input before control is refused"
    );

    viewer.request_control();
    viewer.send_input(key(0xE0, 2, true));
    viewer.send_input(click(10, 10, true));
    pump(&mut host, &mut viewer, &mut injector, &mut device);
    assert_eq!(injector.holder(), Some(PHONE));
    assert_eq!(events(&injector).len(), 3, "key, move, press");

    viewer.release_control();
    pump(&mut host, &mut viewer, &mut injector, &mut device);
    assert_eq!(injector.holder(), None);
    let released: Vec<&SinkEvent> = events(&injector)[3..].iter().collect();
    assert!(matches!(
        released[..],
        [
            SinkEvent::Key {
                usage: 0xE0,
                pressed: false,
                ..
            },
            SinkEvent::PointerButton { pressed: false, .. }
        ]
    ));
}
