//! Sharing this computer's screens with paired devices.
//!
//! One thread per watching device captures its display and feeds the screen session; another
//! injects the input that session allows, because Core Graphics events must be posted from a
//! thread that owns its event source. Both stop when the device leaves or the user stops sharing.
//!
//! Sharing is off until [`ScreenSharing::set_enabled`] turns it on, and the UI reads
//! [`ScreenSharing::watchers`] for the sharing indicator.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use termirust_controller_listener::{
    ControllerScreenSession, ScreenOutgoing, ScreenSessionFactory, ScreenWatcherReport,
};
use termirust_domain::AuthenticatedPeer;
use termirust_screen_capture::{CaptureConfig, Damage, FrameSource};
use termirust_screen_codec::{Frame, Size};
use termirust_screen_host::{ScreenHost, ScreenHostEvent, ScreenHostHandle};
use termirust_screen_input::{DisplayLayout, DisplayPlacement, Injector};
use termirust_screen_protocol::{ControlHolder, SurfaceInfo};
use termirust_screen_session::{HostConfig, InputEvent};

/// How long a capture thread waits for a changed frame before checking whether to stop.
const CAPTURE_POLL: Duration = Duration::from_millis(250);

/// A device currently watching this computer, for the sharing indicator.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScreenWatcher {
    pub device_id: termirust_domain::ControllerDeviceId,
    pub display_name: String,
    /// Whether this device currently holds the writer lease.
    pub controlling: bool,
}

/// The host side of Remote Screens for the running app.
#[derive(Clone)]
pub struct ScreenSharing {
    enabled: Arc<AtomicBool>,
    watchers: Arc<Mutex<Vec<ScreenWatcher>>>,
}

impl Default for ScreenSharing {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ScreenSharing {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ScreenSharing")
            .field("enabled", &self.is_enabled())
            .field("watchers", &self.watchers().len())
            .finish()
    }
}

impl ScreenSharing {
    pub fn new() -> Self {
        Self::with_enabled(false)
    }

    /// Already sharing: what a listener process built from a descriptor that asked for screens.
    pub fn enabled() -> Self {
        Self::with_enabled(true)
    }

    fn with_enabled(enabled: bool) -> Self {
        Self {
            enabled: Arc::new(AtomicBool::new(enabled)),
            watchers: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Turns sharing on or off. Turning it off refuses new sessions; sessions already running
    /// end when their device leaves, so nobody loses a screen mid-gesture without being told.
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Release);
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    /// Who is watching right now.
    pub fn watchers(&self) -> Vec<ScreenWatcher> {
        self.watchers.lock().expect("watchers").clone()
    }

    fn add_watcher(&self, watcher: ScreenWatcher) {
        let mut watchers = self.watchers.lock().expect("watchers");
        watchers.retain(|existing| existing.device_id != watcher.device_id);
        watchers.push(watcher);
    }

    fn remove_watcher(&self, device_id: termirust_domain::ControllerDeviceId) {
        self.watchers
            .lock()
            .expect("watchers")
            .retain(|watcher| watcher.device_id != device_id);
    }

    fn set_controlling(&self, device_id: termirust_domain::ControllerDeviceId, controlling: bool) {
        for watcher in self.watchers.lock().expect("watchers").iter_mut() {
            watcher.controlling = watcher.device_id == device_id && controlling;
        }
    }
}

impl ScreenSessionFactory for ScreenSharing {
    fn open(
        &self,
        peer: &AuthenticatedPeer,
        outgoing: ScreenOutgoing,
    ) -> Option<Box<dyn ControllerScreenSession>> {
        if !self.is_enabled() {
            return None;
        }
        let displays = displays().ok()?;
        if displays.is_empty() {
            return None;
        }
        let device_id = peer.device_id;
        let sharing = self.clone();
        let (input_sender, input_receiver) = channel();
        let layout = layout(&displays);
        // The listener makes a screen session for every Controller connection, so capture waits
        // for a device to actually open one. A phone that only watches a terminal must never turn
        // this computer's screen recording on.
        let pending = Arc::new(Mutex::new(Some((displays.clone(), layout, input_receiver))));
        // The observer needs the handle that this call has not produced yet.
        let opened_handle: Arc<Mutex<Option<ScreenHostHandle>>> = Arc::new(Mutex::new(None));
        let observer_handle = Arc::clone(&opened_handle);
        let (session, handle) = ScreenHost::new(
            displays.iter().map(|display| display.surface()).collect(),
            HostConfig::default(),
            outgoing,
            Arc::new(move |event| match event {
                ScreenHostEvent::Opened { .. } => {
                    sharing.add_watcher(ScreenWatcher {
                        device_id,
                        display_name: String::new(),
                        controlling: false,
                    });
                    let handle = observer_handle.lock().expect("screen handle").clone();
                    let started = pending.lock().expect("screen capture start").take();
                    if let (Some(handle), Some((displays, layout, input))) = (handle, started) {
                        for display in &displays {
                            spawn_capture(display.clone(), handle.clone());
                        }
                        spawn_injection(layout, input, handle);
                    }
                }
                ScreenHostEvent::Input(input) => {
                    let _ = input_sender.send(Some(input));
                }
                ScreenHostEvent::ControlRequested => {
                    sharing.set_controlling(device_id, true);
                    // The person already gave this device pointer or keyboard access in Devices,
                    // and the session only asks when it holds one of those; the lease is what
                    // keeps two devices from driving at once, not a second permission.
                    hand_over_control(&observer_handle, ControlHolder::You);
                }
                ScreenHostEvent::ControlReleased => {
                    sharing.set_controlling(device_id, false);
                    hand_over_control(&observer_handle, ControlHolder::Nobody);
                }
                ScreenHostEvent::Closed { .. } => {
                    sharing.remove_watcher(device_id);
                    let _ = input_sender.send(None);
                }
            }),
        );
        *opened_handle.lock().expect("screen handle") = Some(handle);
        Some(Box::new(session))
    }

    fn watchers(&self) -> Vec<ScreenWatcherReport> {
        self.watchers()
            .into_iter()
            .map(|watcher| ScreenWatcherReport {
                device_id: watcher.device_id,
                controlling: watcher.controlling,
            })
            .collect()
    }
}

/// Answers a device's request for the writer lease, off the session's own thread.
///
/// The session calls its observer while it holds its lock, and telling it who holds control takes
/// that same lock, so this hands the answer to a short-lived thread instead of deadlocking.
fn hand_over_control(handle: &Arc<Mutex<Option<ScreenHostHandle>>>, holder: ControlHolder) {
    let Some(handle) = handle.lock().expect("screen handle").clone() else {
        return;
    };
    let _ = std::thread::Builder::new()
        .name("screen-control".to_owned())
        .spawn(move || handle.set_control(holder));
}

/// One display this computer can share.
#[derive(Clone, Debug)]
struct SharedDisplay {
    id: u32,
    name: String,
    pixels: Size,
    size_points: Size,
    origin_points: (i32, i32),
    scale_milli: u16,
}

impl SharedDisplay {
    fn surface(&self) -> SurfaceInfo {
        SurfaceInfo {
            id: self.id,
            size: self.pixels,
            scale_milli: self.scale_milli,
            name: self.name.clone(),
        }
    }
}

#[cfg(target_os = "macos")]
fn displays() -> Result<Vec<SharedDisplay>, termirust_screen_capture::CaptureError> {
    // Capture at the display's own scale; the viewer downscales for its viewport.
    const SCALE: f32 = 2.0;
    Ok(termirust_screen_capture::displays()?
        .into_iter()
        .enumerate()
        .filter_map(|(index, display)| {
            let pixels = Size::new(
                (f64::from(display.size_points.width()) * f64::from(SCALE)).round() as u32,
                (f64::from(display.size_points.height()) * f64::from(SCALE)).round() as u32,
            )
            .ok()?;
            Some(SharedDisplay {
                id: display.id,
                name: if index == 0 {
                    "Main Display".to_owned()
                } else {
                    format!("Display {}", index + 1)
                },
                pixels,
                size_points: display.size_points,
                origin_points: display.origin_points,
                scale_milli: (SCALE * 1000.0) as u16,
            })
        })
        .collect())
}

#[cfg(target_os = "windows")]
fn displays() -> Result<Vec<SharedDisplay>, termirust_screen_capture::CaptureError> {
    // Desktop Duplication hands back the desktop's own pixels and cannot resample, so there is no
    // scale to choose here the way there is on macOS: what DXGI calls the desktop coordinates is
    // already the pixel count.
    Ok(termirust_screen_capture::displays()?
        .into_iter()
        .enumerate()
        .map(|(index, display)| SharedDisplay {
            id: display.id,
            name: if index == 0 {
                "Main Display".to_owned()
            } else {
                format!("Display {}", index + 1)
            },
            pixels: display.size_points,
            size_points: display.size_points,
            origin_points: display.origin_points,
            scale_milli: 1000,
        })
        .collect())
}

/// Linux shares nothing yet, and an empty list is the honest answer rather than a placeholder.
///
/// The backend exists and works; what does not fit is this flow. Every other platform lets the
/// host enumerate screens, publish them, and let the watching device pick one. On Wayland the
/// compositor's portal does the picking, and it cannot be asked until a session is being started —
/// so there is nothing truthful to publish beforehand. Offering one invented display would make
/// the device's picker work and the pick mean nothing.
///
/// Closing this needs a different shape: a "share a screen" action on this computer that opens the
/// portal, and a display list published from what came back. That is product work, not wiring.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn displays() -> Result<Vec<SharedDisplay>, termirust_screen_capture::CaptureError> {
    Ok(Vec::new())
}

fn layout(displays: &[SharedDisplay]) -> DisplayLayout {
    DisplayLayout::new(
        displays
            .iter()
            .map(|display| {
                DisplayPlacement::new(
                    display.id,
                    display.pixels,
                    display.origin_points,
                    display.size_points,
                )
            })
            .collect(),
    )
}

/// Feeds a session from a capture source until it closes. Platform-free on purpose: the backends
/// differ in how they start, not in what a frame means once it arrives.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn pump(mut source: impl FrameSource, display_id: u32, handle: &ScreenHostHandle) {
    while handle.is_open() {
        match source.next_frame(CAPTURE_POLL) {
            Ok(Some(captured)) => {
                let Ok(frame) = Frame::new(captured.size, captured.stride, &captured.pixels) else {
                    continue;
                };
                let damage = match &captured.damage {
                    Damage::Rects(rects) => Some(rects.as_slice()),
                    Damage::Unknown => None,
                };
                if handle
                    .frame(display_id, &frame, damage, captured.timestamp_ms)
                    .is_err()
                {
                    return;
                }
            }
            Ok(None) => {}
            Err(_) => {
                handle.stop("capture_ended");
                return;
            }
        }
    }
}

/// Captures one display until the session closes.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn spawn_capture(display: SharedDisplay, handle: ScreenHostHandle) {
    std::thread::Builder::new()
        .name(format!("screen-capture-{}", display.id))
        .spawn(move || {
            let config = CaptureConfig {
                display_id: display.id,
                scale: f32::from(display.scale_milli) / 1000.0,
                max_fps: 60,
                show_cursor: true,
            };
            #[cfg(target_os = "macos")]
            let started = termirust_screen_capture::ScreenCaptureKitSource::start(config);
            #[cfg(target_os = "windows")]
            let started = termirust_screen_capture::DesktopDuplicationSource::start(config);
            let Ok(source) = started else {
                handle.stop("capture_not_permitted");
                return;
            };
            pump(source, display.id, &handle);
        })
        .ok();
}

/// Nothing to capture where nothing was published; see `displays` above.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn spawn_capture(_display: SharedDisplay, _handle: ScreenHostHandle) {}

/// Injects the input the session allowed, on a thread that owns the platform's event source.
fn spawn_injection(
    layout: DisplayLayout,
    input: Receiver<Option<InputEvent>>,
    handle: ScreenHostHandle,
) {
    std::thread::Builder::new()
        .name("screen-input".to_owned())
        .spawn(move || {
            let Some(mut injector) = open_injector(layout) else {
                // Without the Accessibility permission the device can still watch.
                drain(&input);
                return;
            };
            const DEVICE: u64 = 1;
            if injector.set_holder(Some(DEVICE)).is_err() {
                return;
            }
            while let Ok(Some(event)) = input.recv() {
                if injector.inject(DEVICE, &event, now_ms()).is_err() {
                    break;
                }
            }
            let _ = injector.set_holder(None);
            let _ = handle;
        })
        .ok();
}

fn drain(input: &Receiver<Option<InputEvent>>) {
    loop {
        match input.try_recv() {
            Ok(_) => {}
            Err(TryRecvError::Empty) => std::thread::sleep(CAPTURE_POLL),
            Err(TryRecvError::Disconnected) => return,
        }
    }
}

#[cfg(target_os = "macos")]
fn open_injector(
    layout: DisplayLayout,
) -> Option<Injector<termirust_screen_input::CoreGraphicsSink>> {
    termirust_screen_input::CoreGraphicsSink::new()
        .ok()
        .map(|sink| Injector::new(sink, layout))
}

#[cfg(not(target_os = "macos"))]
fn open_injector(
    _layout: DisplayLayout,
) -> Option<Injector<termirust_screen_input::RecordingSink>> {
    None
}

fn now_ms() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device() -> termirust_domain::ControllerDeviceId {
        termirust_domain::ControllerDeviceId::new()
    }

    #[test]
    fn sharing_is_off_until_the_user_turns_it_on() {
        let sharing = ScreenSharing::new();
        assert!(!sharing.is_enabled());
        assert!(sharing.watchers().is_empty());
        sharing.set_enabled(true);
        assert!(sharing.is_enabled());
    }

    #[test]
    fn the_indicator_follows_who_watches_and_who_controls() {
        let sharing = ScreenSharing::new();
        let phone = device();
        let laptop = device();
        for (device_id, name) in [(phone, "Phone"), (laptop, "Laptop")] {
            sharing.add_watcher(ScreenWatcher {
                device_id,
                display_name: name.to_owned(),
                controlling: false,
            });
        }
        assert_eq!(sharing.watchers().len(), 2);

        sharing.set_controlling(laptop, true);
        let watchers = sharing.watchers();
        assert!(
            watchers
                .iter()
                .any(|w| w.device_id == laptop && w.controlling)
        );
        assert!(
            watchers
                .iter()
                .any(|w| w.device_id == phone && !w.controlling)
        );

        // Only one device holds the lease at a time.
        sharing.set_controlling(phone, true);
        assert_eq!(
            sharing
                .watchers()
                .iter()
                .filter(|watcher| watcher.controlling)
                .count(),
            1
        );

        sharing.remove_watcher(phone);
        sharing.remove_watcher(laptop);
        assert!(sharing.watchers().is_empty());
    }

    #[test]
    fn display_placements_match_the_surfaces_that_are_shared() {
        let displays = vec![SharedDisplay {
            id: 7,
            name: "Main Display".to_owned(),
            pixels: Size::new(3024, 1964).unwrap(),
            size_points: Size::new(1512, 982).unwrap(),
            origin_points: (0, 0),
            scale_milli: 2000,
        }];
        let layout = layout(&displays);
        let placement = layout.placement(7).expect("the display is placed");
        assert_eq!(placement.to_global(3023, 1963).x, 1511.5);
        assert_eq!(displays[0].surface().size, displays[0].pixels);
    }
}
