//! A synthetic screen the fixture shares, so a phone can be tested against a real host.
//!
//! No capture and no platform code: one surface with a caret that moves down a window, which is
//! enough to prove that pixels reach the phone, that only what changed is sent, and that input
//! arrives with the capability it claimed.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use termirust_controller_listener::{
    ControllerScreenSession, ScreenOutgoing, ScreenSessionFactory, ScreenWatcherReport,
};
use termirust_domain::AuthenticatedPeer;
use termirust_screen_codec::{FrameBuffer, Rect, Size};
use termirust_screen_host::{ScreenHost, ScreenHostEvent, ScreenHostHandle};
use termirust_screen_protocol::SurfaceInfo;
use termirust_screen_session::{HostConfig, InputEvent};

pub const SURFACE_ID: u32 = 1;
pub const SURFACE_WIDTH: u32 = 320;
pub const SURFACE_HEIGHT: u32 = 200;
/// Steps the caret takes before it returns to the top, so a test can watch it move.
pub const STEPS: u32 = 6;
const FRAME_INTERVAL: Duration = Duration::from_millis(100);

/// What the fixture saw, so a test can assert on it through the control channel.
#[derive(Debug, Default)]
pub struct ScreenObservations {
    pub opened: AtomicU32,
    pub pointer_events: AtomicU32,
    pub keyboard_events: AtomicU32,
    pub control_requests: AtomicU32,
}

/// Serves the synthetic screen to every device allowed to watch it.
///
/// The listener makes a screen session for every authenticated connection, long before a device
/// asks to watch anything, so this only takes a session to be the live one once its hello has
/// opened it. A connection that only carries a terminal never paints, and never takes control
/// away from the connection that is really watching.
pub struct FixtureScreens {
    observations: Arc<ScreenObservations>,
    watching: Arc<Mutex<Option<ScreenHostHandle>>>,
}

impl FixtureScreens {
    pub fn new(observations: Arc<ScreenObservations>) -> Self {
        Self {
            observations,
            watching: Arc::new(Mutex::new(None)),
        }
    }

    /// Gives control to the one device watching, the way a person would from Devices.
    pub fn give_control(&self) -> bool {
        // Copy the handle out before touching it: the session calls back into `watching` while
        // it holds its own lock, and taking them in both orders would deadlock.
        let handle = self
            .watching
            .lock()
            .expect("fixture screen handles")
            .clone();
        let Some(handle) = handle.filter(ScreenHostHandle::is_open) else {
            return false;
        };
        handle.set_control(termirust_screen_protocol::ControlHolder::You);
        true
    }
}

impl ScreenSessionFactory for FixtureScreens {
    fn open(
        &self,
        _peer: &AuthenticatedPeer,
        outgoing: ScreenOutgoing,
    ) -> Option<Box<dyn ControllerScreenSession>> {
        let observations = Arc::clone(&self.observations);
        let watching = Arc::clone(&self.watching);
        // The observer needs the handle the call it is part of has not returned yet.
        let opened_handle: Arc<Mutex<Option<ScreenHostHandle>>> = Arc::new(Mutex::new(None));
        let observer_handle = Arc::clone(&opened_handle);
        let (session, handle) = ScreenHost::new(
            vec![SurfaceInfo {
                id: SURFACE_ID,
                size: Size::new(SURFACE_WIDTH, SURFACE_HEIGHT).expect("a valid surface"),
                scale_milli: 2000,
                name: "Fixture Display".to_owned(),
            }],
            HostConfig::default(),
            outgoing,
            Arc::new(move |event| match event {
                ScreenHostEvent::Opened { .. } => {
                    observations.opened.fetch_add(1, Ordering::Release);
                    // A device is really watching now, so this is the session that paints and
                    // the one a person would hand control to.
                    let Some(handle) = observer_handle
                        .lock()
                        .expect("fixture screen handle")
                        .clone()
                    else {
                        return;
                    };
                    *watching.lock().expect("fixture screen handles") = Some(handle.clone());
                    let _ = std::thread::Builder::new()
                        .name("fixture-screen".to_owned())
                        .spawn(move || paint(&handle));
                }
                ScreenHostEvent::ControlRequested => {
                    observations
                        .control_requests
                        .fetch_add(1, Ordering::Release);
                }
                ScreenHostEvent::Input(input) => {
                    let counter = match input {
                        InputEvent::Key(_) | InputEvent::Text { .. } => {
                            &observations.keyboard_events
                        }
                        _ => &observations.pointer_events,
                    };
                    counter.fetch_add(1, Ordering::Release);
                }
                ScreenHostEvent::ControlReleased => {}
                ScreenHostEvent::Closed { .. } => {
                    *watching.lock().expect("fixture screen handles") = None;
                }
            }),
        );
        *opened_handle.lock().expect("fixture screen handle") = Some(handle);
        Some(Box::new(session))
    }

    fn watchers(&self) -> Vec<ScreenWatcherReport> {
        Vec::new()
    }
}

/// Paints the caret moving down the window until the device leaves.
fn paint(handle: &ScreenHostHandle) {
    let mut step = 0;
    let mut now_ms = 0;
    while handle.is_open() {
        let frame = screen(step % STEPS);
        if handle
            .frame(SURFACE_ID, &frame.as_frame(), None, now_ms)
            .is_err()
        {
            return;
        }
        step += 1;
        now_ms += FRAME_INTERVAL.as_millis() as u64;
        std::thread::sleep(FRAME_INTERVAL);
    }
}

/// A window on a desk, with a caret on line `step`.
pub fn screen(step: u32) -> FrameBuffer {
    let size = Size::new(SURFACE_WIDTH, SURFACE_HEIGHT).expect("a valid surface");
    let mut buffer = FrameBuffer::new(size);
    buffer
        .fill_rect(size.bounds(), [236, 232, 228, 255])
        .expect("the whole surface");
    buffer
        .fill_rect(Rect::new(24, 24, 272, 152), [30, 28, 26, 255])
        .expect("the window");
    for row in 0..STEPS {
        let length = (row * 37) % 240 + 12;
        buffer
            .fill_rect(
                Rect::new(36, 40 + row * 22, length, 9),
                [214, 208, 200, 255],
            )
            .expect("a line of text");
    }
    buffer
        .fill_rect(Rect::new(36, 40 + step * 22, 8, 12), [240, 200, 80, 255])
        .expect("the caret");
    buffer
}
