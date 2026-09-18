//! Remote Screens sessions carried by Controller screen frames.
//!
//! The listener moves opaque bytes: a screen frame's payload is a chunk of the screen protocol's
//! byte stream, so a message larger than one frame simply spans two. What the bytes mean is the
//! host application's business; what the listener enforces is the capability each frame claims,
//! against the device's current record, and the one-time ticket that started the session.

use async_trait::async_trait;
use termirust_controller_security::{
    ControllerCapability as SecurityCapability, MAX_SCREEN_FRAME_BYTES,
};

use crate::{ListenerError, ListenerErrorCode, ScreenTicketStore};

/// The screen bytes one frame can carry: the frame limit less its header and tag.
pub const MAX_SCREEN_PAYLOAD_BYTES: usize = MAX_SCREEN_FRAME_BYTES - 48;

/// What a screen frame claims to exercise. Anything else on a screen frame is refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScreenFrameCapability {
    /// Watching: subscriptions, acknowledgements, everything that is not input.
    Observe,
    /// Pointer input: moves, buttons, scroll.
    Pointer,
    /// Keyboard input: keys and typed text.
    Keyboard,
}

impl ScreenFrameCapability {
    pub const fn from_security(capability: SecurityCapability) -> Result<Self, ListenerError> {
        match capability {
            SecurityCapability::ObserveScreens => Ok(Self::Observe),
            SecurityCapability::ControlPointer => Ok(Self::Pointer),
            SecurityCapability::ControlKeyboard => Ok(Self::Keyboard),
            _ => Err(ListenerError::new(ListenerErrorCode::Unauthorized)),
        }
    }

    pub const fn security(self) -> SecurityCapability {
        match self {
            Self::Observe => SecurityCapability::ObserveScreens,
            Self::Pointer => SecurityCapability::ControlPointer,
            Self::Keyboard => SecurityCapability::ControlKeyboard,
        }
    }
}

/// Where a screen session pushes bytes for the device. Each send is sealed into screen frames,
/// split across frames when it is larger than [`MAX_SCREEN_PAYLOAD_BYTES`]. Dropping every sender
/// ends the screen session and leaves the Controller connection open.
///
/// The queue is unbounded here because dropping screen bytes would desynchronise the tile
/// encoder from the viewer's cache. What bounds it is the session itself, which stops encoding
/// new frames while the viewer's acknowledgements are outstanding, so a stalled connection stops
/// producing rather than piling up.
pub type ScreenOutgoing = tokio::sync::mpsc::UnboundedSender<Vec<u8>>;

/// One device's screen session on the host. Implemented by the application that owns the screens.
#[async_trait]
pub trait ControllerScreenSession: Send {
    /// Feeds screen bytes the device sent under `capability`. The implementation spends the
    /// ticket with `tickets` when the session's first message presents it, and refuses messages
    /// the frame's capability does not cover.
    async fn receive(
        &mut self,
        capability: ScreenFrameCapability,
        bytes: &[u8],
        tickets: &mut ScreenTicketStore,
    ) -> Result<(), ListenerError>;

    /// The device closed the session, or the connection is ending.
    fn close(&mut self);
}

/// A device watching this computer's screens right now, as the app's indicator shows it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScreenWatcherReport {
    pub device_id: termirust_domain::ControllerDeviceId,
    /// Whether this device holds the writer lease and can point and type.
    pub controlling: bool,
}

/// Opens screen sessions for connections, when the host application can serve screens.
pub trait ScreenSessionFactory: Send + Sync {
    fn open(
        &self,
        peer: &termirust_domain::AuthenticatedPeer,
        outgoing: ScreenOutgoing,
    ) -> Option<Box<dyn ControllerScreenSession>>;

    /// Who is watching. The listener polls this and tells the application, so the computer being
    /// watched can say so.
    fn watchers(&self) -> Vec<ScreenWatcherReport> {
        Vec::new()
    }

    /// Called once when the worker starts with screen sharing on, before any device connects.
    ///
    /// A host that has to ask a person before it can capture anything does the asking here rather
    /// than when a phone arrives. On Wayland the compositor's portal owns the choice of screen, so
    /// asking at the moment a device connects would be both a surprise and too late: the session
    /// needs to know what it is sharing before it can offer it. Hosts that can simply enumerate
    /// their displays have nothing to prepare and keep the default.
    ///
    /// `restore_token` is what a previous run was given back, so the person is asked once per
    /// machine rather than once per listener. Must not block: it is called on the worker's startup
    /// path.
    fn prepare(&self, _restore_token: Option<&str>) {}

    /// A token worth remembering for next time, once the host has one.
    ///
    /// The listener polls this alongside [`Self::watchers`] and reports it to the application,
    /// which is the only part that can persist anything. Hosts that need no permission to capture
    /// have nothing to remember.
    fn restore_token(&self) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_frame_carries_a_whole_tile_batch_less_its_header_and_tag() {
        assert_eq!(MAX_SCREEN_PAYLOAD_BYTES, (1 << 20) - 48);
    }

    #[test]
    fn only_the_three_screen_capabilities_ride_screen_frames() {
        for (capability, expected) in [
            (
                SecurityCapability::ObserveScreens,
                ScreenFrameCapability::Observe,
            ),
            (
                SecurityCapability::ControlPointer,
                ScreenFrameCapability::Pointer,
            ),
            (
                SecurityCapability::ControlKeyboard,
                ScreenFrameCapability::Keyboard,
            ),
        ] {
            let frame = ScreenFrameCapability::from_security(capability).unwrap();
            assert_eq!(frame, expected);
            assert_eq!(frame.security(), capability);
        }
        for capability in [
            SecurityCapability::ObserveSessions,
            SecurityCapability::AttachOutput,
            SecurityCapability::SendInput,
            SecurityCapability::Resize,
            SecurityCapability::RespondToApproval,
        ] {
            assert_eq!(
                ScreenFrameCapability::from_security(capability).map_err(|error| error.code),
                Err(ListenerErrorCode::Unauthorized)
            );
        }
    }
}
