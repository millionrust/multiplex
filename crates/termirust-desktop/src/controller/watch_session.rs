//! Watching another computer's screen from this one.
//!
//! The mirror of `screen_sharing.rs`: that serves this Mac's displays to a device, this makes this
//! Mac the device. A session runs on its own thread with its own runtime, because the Controller
//! channel is async and the interface is not, and publishes two things the interface reads
//! whenever it draws: the latest picture and what the session is doing.
//!
//! Input goes the other way on a channel, so a pointer move never blocks the interface on the
//! network.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::RenderImage;
use image::{Frame, RgbaImage};
use smallvec::SmallVec;
use termirust_controller_listener::{
    ControllerClientChannel, ControllerCommand, ControllerIncoming, ControllerResponse,
    ScreenFrameCapability, SystemHandshakeEntropy,
};
use termirust_controller_security::CapabilitySet;
use termirust_screen_protocol::{
    ControlHolder, FrameReader, InputKind, Profile, SurfaceInfo, encode_frame,
};
use termirust_screen_session::{InputEvent, ViewerEvent, ViewerSession};

use super::watched::{WatchedComputer, WatchedComputers};

/// How much of the computer's screen this Mac keeps, so tiles it has already seen are not resent.
const CACHE_BYTES: usize = 64 * 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// What a watch session is doing, for the interface to say.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WatchState {
    Connecting,
    /// Pictures are arriving.
    Watching,
    /// The session ended. The last picture, if any, is still worth showing.
    Ended(String),
}

/// Something the person did that has to reach the other computer.
#[derive(Clone, Debug)]
pub enum WatchInput {
    Pointer(InputEvent),
    RequestControl,
    ReleaseControl,
    /// Watch a different display of the same computer.
    Display(u32),
}

/// What the interface reads while a session runs.
#[derive(Debug, Default)]
struct Shared {
    picture: Mutex<Option<Arc<RenderImage>>>,
    size: Mutex<(u32, u32)>,
    state: Mutex<Option<WatchState>>,
    control: Mutex<Option<ControlHolder>>,
    displays: Mutex<Vec<SurfaceInfo>>,
    /// Bumped for every picture, so the interface can tell a repaint from a stall.
    pictures: AtomicU64,
}

/// A running session. Dropping it ends the session.
pub struct WatchSession {
    shared: Arc<Shared>,
    input: std::sync::mpsc::Sender<WatchInput>,
    cancel: Arc<std::sync::atomic::AtomicBool>,
}

impl std::fmt::Debug for WatchSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WatchSession")
            .field("state", &self.state())
            .field("pictures", &self.pictures())
            .finish()
    }
}

impl Drop for WatchSession {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

impl WatchSession {
    /// Opens a session to `computer`. `preview` asks for the computer's thumbnail profile,
    /// about one small picture a second, which is what the Devices list shows.
    pub fn start(
        computer: &WatchedComputer,
        computers: &WatchedComputers,
        preview: bool,
    ) -> Option<Self> {
        let private = computers.private_key(computer)?;
        let capabilities = computer.capabilities()?;
        let shared = Arc::new(Shared::default());
        *shared.state.lock().expect("watch state") = Some(WatchState::Connecting);
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (sender, receiver) = std::sync::mpsc::channel();

        let worker_shared = Arc::clone(&shared);
        let worker_cancel = Arc::clone(&cancel);
        let address = computer.address.clone();
        let host_key = computer.host_key();
        let identity_generation = computer.identity_generation;
        let revocation_epoch = computer.revocation_epoch;
        let session_generation = computer.session_generation;

        std::thread::Builder::new()
            .name("screen-watch".to_owned())
            .spawn(move || {
                let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    worker_shared.end("could not start");
                    return;
                };
                let outcome = runtime.block_on(run(
                    address,
                    identity_generation,
                    revocation_epoch,
                    session_generation,
                    host_key,
                    private,
                    capabilities,
                    preview,
                    &worker_shared,
                    &worker_cancel,
                    receiver,
                ));
                worker_shared.end(match outcome {
                    Ok(()) => "the session ended",
                    Err(reason) => reason,
                });
            })
            .ok()?;
        Some(Self {
            shared,
            input: sender,
            cancel,
        })
    }

    pub fn picture(&self) -> Option<Arc<RenderImage>> {
        self.shared.picture.lock().expect("watch picture").clone()
    }

    /// The computer's screen, in its own pixels.
    pub fn size(&self) -> (u32, u32) {
        *self.shared.size.lock().expect("watch size")
    }

    pub fn state(&self) -> WatchState {
        self.shared
            .state
            .lock()
            .expect("watch state")
            .clone()
            .unwrap_or(WatchState::Connecting)
    }

    pub fn control(&self) -> ControlHolder {
        self.shared
            .control
            .lock()
            .expect("watch control")
            .unwrap_or(ControlHolder::Nobody)
    }

    pub fn displays(&self) -> Vec<SurfaceInfo> {
        self.shared.displays.lock().expect("watch displays").clone()
    }

    pub fn pictures(&self) -> u64 {
        self.shared.pictures.load(Ordering::Acquire)
    }

    /// Queues something for the other computer. Never blocks the interface.
    pub fn send(&self, input: WatchInput) {
        let _ = self.input.send(input);
    }
}

impl Shared {
    fn end(&self, reason: &str) {
        *self.state.lock().expect("watch state") = Some(WatchState::Ended(reason.to_owned()));
    }
}

#[allow(clippy::too_many_arguments)]
async fn run(
    address: String,
    identity_generation: u64,
    revocation_epoch: u64,
    session_generation: u64,
    host_key: termirust_controller_security::HostStaticPublicKey,
    private: termirust_controller_security::StaticPrivateKey,
    capabilities: CapabilitySet,
    preview: bool,
    shared: &Arc<Shared>,
    cancel: &Arc<std::sync::atomic::AtomicBool>,
    input: std::sync::mpsc::Receiver<WatchInput>,
) -> Result<(), &'static str> {
    let stream = tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::TcpStream::connect(&address))
        .await
        .map_err(|_| "that computer did not answer")?
        .map_err(|_| "that computer did not answer")?;
    let mut entropy = SystemHandshakeEntropy;
    let mut channel = ControllerClientChannel::connect(
        stream,
        identity_generation,
        revocation_epoch,
        session_generation,
        host_key,
        private,
        capabilities,
        &mut entropy,
    )
    .await
    .map_err(|_| "that computer refused this Mac")?;

    channel
        .send(ControllerCommand::OpenScreen, deadline())
        .await
        .map_err(|_| "that computer would not open a screen")?;
    let ControllerResponse::ScreenOpened { ticket, .. } = channel
        .read_response()
        .await
        .map_err(|_| "that computer would not open a screen")?
    else {
        return Err("that computer would not open a screen");
    };
    let proof: [u8; 32] = ticket
        .as_slice()
        .try_into()
        .map_err(|_| "that computer sent a malformed ticket")?;

    let mut session = ViewerSession::new(CACHE_BYTES);
    session.connect(proof);
    let mut reader = FrameReader::new();
    let mut watching: Option<u32> = None;
    let profile = if preview {
        Profile::Thumbnail
    } else {
        Profile::Interactive
    };

    loop {
        if cancel.load(Ordering::Acquire) {
            return Ok(());
        }
        // Everything the person did since the last frame, then everything the session queued.
        while let Ok(command) = input.try_recv() {
            match command {
                WatchInput::Pointer(event) => session.send_input(event),
                WatchInput::RequestControl => session.request_control(),
                WatchInput::ReleaseControl => session.release_control(),
                WatchInput::Display(surface) => {
                    if let Some(previous) = watching {
                        session.unsubscribe(previous);
                    }
                    watching = Some(surface);
                    session.subscribe(surface, profile);
                }
            }
        }
        while let Some(message) = session.poll_outgoing() {
            let capability = match message.input_kind() {
                None => ScreenFrameCapability::Observe,
                Some(InputKind::Pointer) => ScreenFrameCapability::Pointer,
                Some(InputKind::Keyboard) => ScreenFrameCapability::Keyboard,
            };
            let bytes = encode_frame(&message).map_err(|_| "this Mac sent something malformed")?;
            channel
                .send_screen(capability, &bytes)
                .await
                .map_err(|_| "the connection stopped")?;
        }

        let incoming =
            match tokio::time::timeout(Duration::from_millis(250), channel.read_incoming()).await {
                // Nothing arrived; loop so input and cancellation are still noticed.
                Err(_) => continue,
                Ok(incoming) => incoming.map_err(|_| "the connection stopped")?,
            };
        let ControllerIncoming::Screen(bytes) = incoming else {
            continue;
        };
        reader.push(&bytes);
        while let Some(message) = reader
            .next_message()
            .map_err(|_| "that computer sent something malformed")?
        {
            let events = session
                .receive(message)
                .map_err(|_| "that computer ended the session")?;
            for event in events {
                match event {
                    ViewerEvent::Welcomed { surfaces, .. } => {
                        *shared.displays.lock().expect("watch displays") = surfaces.clone();
                        let Some(first) = surfaces.first() else {
                            continue;
                        };
                        watching = Some(first.id);
                        session.subscribe(first.id, profile);
                    }
                    ViewerEvent::Control(holder) => {
                        *shared.control.lock().expect("watch control") = Some(holder);
                    }
                    ViewerEvent::Closed { reason } => {
                        shared.end(&reason);
                        return Ok(());
                    }
                    ViewerEvent::Updated { surface, .. } => {
                        if Some(surface) != watching {
                            continue;
                        }
                        publish(&session, surface, preview, shared);
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Copies the decoded picture into something the interface can draw.
fn publish(session: &ViewerSession, surface: u32, preview: bool, shared: &Arc<Shared>) {
    let frame = if preview {
        session.preview(surface)
    } else {
        session.framebuffer(surface)
    };
    let Some(frame) = frame else { return };
    let size = frame.size();
    let view = frame.as_frame();
    let mut rgba = Vec::with_capacity(size.width() as usize * size.height() as usize * 4);
    for y in 0..size.height() {
        let row = view.row(y);
        for x in 0..size.width() as usize {
            let pixel = &row[x * 4..x * 4 + 4];
            // The codec speaks BGRA; this buffer is what GPUI blits, which is BGRA too, so the
            // bytes go across unchanged and only the alpha is forced opaque.
            rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
        }
    }
    let Some(image) = RgbaImage::from_raw(size.width(), size.height(), rgba) else {
        return;
    };
    *shared.size.lock().expect("watch size") = (size.width(), size.height());
    *shared.picture.lock().expect("watch picture") =
        Some(Arc::new(RenderImage::new(SmallVec::from_vec(vec![
            Frame::new(image),
        ]))));
    shared.pictures.fetch_add(1, Ordering::Release);
    let mut state = shared.state.lock().expect("watch state");
    if !matches!(*state, Some(WatchState::Ended(_))) {
        *state = Some(WatchState::Watching);
    }
}

fn deadline() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or_default()
        .saturating_add(10_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_starts_out_connecting_and_shows_nothing_yet() {
        let shared = Arc::new(Shared::default());
        assert_eq!(*shared.state.lock().expect("state"), None);
        shared.end("stopped");
        assert!(matches!(
            shared.state.lock().expect("state").clone(),
            Some(WatchState::Ended(reason)) if reason == "stopped"
        ));
    }

    /// A picture that arrives after the session ended must not claim it is still watching.
    #[test]
    fn a_late_picture_does_not_reopen_an_ended_session() {
        let shared = Arc::new(Shared::default());
        shared.end("that computer stopped sharing");
        let mut state = shared.state.lock().expect("state");
        if !matches!(*state, Some(WatchState::Ended(_))) {
            *state = Some(WatchState::Watching);
        }
        drop(state);
        assert!(matches!(
            shared.state.lock().expect("state").clone(),
            Some(WatchState::Ended(_))
        ));
    }
}
