//! One recorded screen session, replayed by this crate and by Swift.
//!
//! The fixture holds what a host actually sent: the frames, and the picture the viewer ends up
//! with. Rust replays it here; `scripts/test/swift-screen-bindings.sh` replays the same bytes
//! through the generated Swift bindings. A change in the codec, the protocol, or the session
//! shows up as a changed fixture instead of two implementations drifting apart.
//!
//! Regenerate deliberately, after reviewing why the bytes moved:
//!
//! ```sh
//! TERMIRUST_WRITE_SCREEN_FIXTURE=1 cargo test -p termirust-screen-bindings --test golden_session
//! ```

use sha2::{Digest, Sha256};
use termirust_screen_bindings::{ScreenEvent, ScreenRect, ScreenViewer};
use termirust_screen_codec::{FrameBuffer, Rect, Size};
use termirust_screen_protocol::{FrameReader, Profile, SurfaceInfo, encode_frame};
use termirust_screen_session::{
    Grants, HostConfig, HostSession, ResumeStore, TicketVerifier, ViewerSession,
};

const TICKET: [u8; 32] = [0x5C; 32];
const SURFACE: u32 = 1;
const WIDTH: u32 = 320;
const HEIGHT: u32 = 200;
/// What this build's host and viewer say to each other.
const FIXTURE: &str = "tests/vectors/screen-session-v2.json";
/// The same session as a version 1 host recorded it, kept exactly as it was. A phone that speaks
/// the current version has to keep painting the same picture from those bytes, so this file is
/// never regenerated: if it has to change, version 1 compatibility is what broke.
const STAGE_A_FIXTURE: &str = "tests/vectors/screen-session-v1.json";

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

/// A window on a desk, with a caret that moves down it.
fn screen(step: u32) -> FrameBuffer {
    let size = Size::new(WIDTH, HEIGHT).unwrap();
    let mut buffer = FrameBuffer::new(size);
    buffer
        .fill_rect(size.bounds(), [236, 232, 228, 255])
        .unwrap();
    buffer
        .fill_rect(Rect::new(24, 24, 272, 152), [30, 28, 26, 255])
        .unwrap();
    for row in 0..6 {
        let length = (row * 37 + step * 23) % 240 + 12;
        buffer
            .fill_rect(
                Rect::new(36, 40 + row * 22, length, 9),
                [214, 208, 200, 255],
            )
            .unwrap();
    }
    buffer
        .fill_rect(Rect::new(36, 40 + step * 22, 8, 12), [240, 200, 80, 255])
        .unwrap();
    buffer
}

/// Records what a host sends while a viewer watches three frames.
fn record() -> (Vec<Vec<u8>>, u32) {
    let surfaces = vec![SurfaceInfo {
        id: SURFACE,
        size: Size::new(WIDTH, HEIGHT).unwrap(),
        scale_milli: 2000,
        name: "Built-in Display".to_owned(),
    }];
    let mut host = HostSession::new(surfaces, Tickets, HostConfig::default());
    let mut store = ResumeStore::default();
    let mut viewer = ViewerSession::new(16 << 20);
    let mut frames = Vec::new();

    viewer.connect(TICKET);
    viewer.subscribe(SURFACE, Profile::Interactive);
    let mut drain = |host: &mut HostSession<Tickets>, viewer: &mut ViewerSession| {
        let mut to_host = FrameReader::new();
        while let Some(message) = viewer.poll_outgoing() {
            to_host.push(&encode_frame(&message).unwrap());
        }
        while let Some(message) = to_host.next_message().unwrap() {
            host.receive(message, &mut store).unwrap();
        }
        let mut bytes = Vec::new();
        while let Some(message) = host.poll_outgoing() {
            bytes.extend(encode_frame(&message).unwrap());
        }
        if !bytes.is_empty() {
            let mut reader = FrameReader::new();
            reader.push(&bytes);
            while let Some(message) = reader.next_message().unwrap() {
                viewer.receive(message).unwrap();
            }
            frames.push(bytes);
        }
    };

    drain(&mut host, &mut viewer);
    for step in 0..3u32 {
        host.frame(
            SURFACE,
            &screen(step).as_frame(),
            None,
            u64::from(step) * 100,
        )
        .unwrap();
        drain(&mut host, &mut viewer);
    }
    drain(&mut host, &mut viewer);
    let last = frames.len() as u32;
    (frames, last)
}

fn whole_surface() -> ScreenRect {
    ScreenRect {
        x: 0,
        y: 0,
        width: WIDTH,
        height: HEIGHT,
    }
}

/// Replays recorded frames through the boundary and returns what a phone would draw.
fn replay(frames: &[Vec<u8>]) -> (u32, String) {
    let viewer = ScreenViewer::new(16 << 20);
    viewer.connect(TICKET.to_vec()).unwrap();
    let mut updates = 0;
    for frame in frames {
        for event in viewer.receive(frame.clone()).unwrap() {
            if matches!(event, ScreenEvent::Updated { preview: false, .. }) {
                updates += 1;
            }
        }
    }
    let pixels = viewer.copy_pixels(SURFACE, false, whole_surface()).unwrap();
    assert_eq!(pixels.bgra.len() as u32, WIDTH * HEIGHT * 4);
    (updates, hex::encode(Sha256::digest(&pixels.bgra)))
}

#[test]
fn the_recorded_session_is_the_one_the_fixture_pins() {
    let (frames, _) = record();
    let (updates, pixels_sha256) = replay(&frames);

    // The picture the viewer built is the picture the host captured.
    let expected = screen(2);
    let mut source = Vec::new();
    for y in 0..HEIGHT {
        source.extend_from_slice(expected.as_frame().row(y));
    }
    assert_eq!(
        pixels_sha256,
        hex::encode(Sha256::digest(&source)),
        "the replayed picture is the last captured frame"
    );

    let fixture = serde_json::json!({
        "ticket_hex": hex::encode(TICKET),
        "surface": {"id": SURFACE, "width": WIDTH, "height": HEIGHT},
        "host_frames_hex": frames.iter().map(hex::encode).collect::<Vec<_>>(),
        "expected_updates": updates,
        "pixels_sha256": pixels_sha256,
    });
    let rendered = format!("{}\n", serde_json::to_string_pretty(&fixture).unwrap());
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE);
    if std::env::var_os("TERMIRUST_WRITE_SCREEN_FIXTURE").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &rendered).unwrap();
        panic!("wrote {FIXTURE}; review the change and run the test again without the variable");
    }
    let committed = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{FIXTURE} is missing ({error}); regenerate it"));
    assert_eq!(
        committed, rendered,
        "the host now sends different bytes than {FIXTURE} pins"
    );
}

#[test]
fn a_version_1_host_still_paints_the_same_picture() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(STAGE_A_FIXTURE);
    let recorded: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("the Stage A fixture"))
            .expect("valid JSON");
    let frames: Vec<Vec<u8>> = recorded["host_frames_hex"]
        .as_array()
        .expect("recorded frames")
        .iter()
        .map(|frame| hex::decode(frame.as_str().expect("hex")).expect("hex"))
        .collect();

    let (updates, pixels_sha256) = replay(&frames);
    assert_eq!(updates, recorded["expected_updates"].as_u64().unwrap() as u32);
    assert_eq!(
        pixels_sha256,
        recorded["pixels_sha256"].as_str().unwrap(),
        "a phone on the current version no longer understands a version 1 host"
    );
}
