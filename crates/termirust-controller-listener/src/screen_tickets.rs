//! Screen tickets: what a device is allowed to do with a Remote Screens session, and the
//! one-time proof that ties a screen session to the Controller command that authorized it.
//!
//! The ticket never leaves the authenticated Controller channel, is spent by the screen
//! protocol's hello, and dies with the connection, the epoch, or a `CloseScreen` command.

use termirust_domain::{ControllerCapabilities, ControllerCapability, ControllerDeviceId};

use crate::{HandshakeEntropy, ListenerError, ListenerErrorCode};

/// What the paired device's capabilities allow a screen session to do.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScreenGrants {
    pub device_id: ControllerDeviceId,
    pub can_view: bool,
    pub can_control_pointer: bool,
    pub can_control_keyboard: bool,
}

impl ScreenGrants {
    pub fn from_capabilities(
        device_id: ControllerDeviceId,
        capabilities: ControllerCapabilities,
    ) -> Self {
        Self {
            device_id,
            can_view: capabilities.contains(ControllerCapability::ObserveScreens),
            can_control_pointer: capabilities.contains(ControllerCapability::ControlPointer),
            can_control_keyboard: capabilities.contains(ControllerCapability::ControlKeyboard),
        }
    }

    pub const fn can_control(self) -> bool {
        self.can_control_pointer || self.can_control_keyboard
    }
}

/// One connection's screen ticket. There is at most one, and using it consumes it.
#[derive(Debug, Default)]
pub struct ScreenTicketStore {
    issued: Option<([u8; 32], ScreenGrants)>,
    spent: Option<ScreenGrants>,
}

impl ScreenTicketStore {
    /// Mints a ticket for `grants`, replacing any ticket this connection had. Fails when the
    /// device may not watch screens at all.
    pub fn issue(
        &mut self,
        grants: ScreenGrants,
        entropy: &mut impl HandshakeEntropy,
    ) -> Result<[u8; 32], ListenerError> {
        if !grants.can_view {
            return Err(ListenerError::new(ListenerErrorCode::Unauthorized));
        }
        let ticket = entropy.nonce()?;
        self.issued = Some((ticket, grants));
        self.spent = None;
        Ok(ticket)
    }

    /// Spends the ticket the device presented. A second use, or a wrong ticket, is refused.
    pub fn spend(&mut self, presented: &[u8; 32]) -> Option<ScreenGrants> {
        let (ticket, grants) = self.issued.as_ref()?;
        if !constant_time_eq(ticket, presented) {
            return None;
        }
        let grants = *grants;
        self.issued = None;
        self.spent = Some(grants);
        Some(grants)
    }

    /// The grants of the running screen session, if the ticket was spent and not closed.
    pub const fn active(&self) -> Option<ScreenGrants> {
        self.spent
    }

    /// Ends the session: the outstanding ticket and the running session are both invalid.
    pub fn close(&mut self) {
        self.issued = None;
        self.spent = None;
    }
}

/// Compares two tickets without leaking how far they matched.
fn constant_time_eq(left: &[u8; 32], right: &[u8; 32]) -> bool {
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use termirust_controller_security::StaticPrivateKey;

    struct Counting(u8);

    impl HandshakeEntropy for Counting {
        fn nonce(&mut self) -> Result<[u8; 32], ListenerError> {
            self.0 += 1;
            Ok([self.0; 32])
        }

        fn ephemeral_private(&mut self) -> Result<StaticPrivateKey, ListenerError> {
            Ok(StaticPrivateKey::from_fixture_bytes([9; 32]))
        }
    }

    fn capabilities(bits: &[ControllerCapability]) -> ControllerCapabilities {
        bits.iter()
            .fold(ControllerCapabilities::default(), |set, capability| {
                set.with(*capability)
            })
    }

    fn device() -> ControllerDeviceId {
        ControllerDeviceId::new()
    }

    #[test]
    fn grants_follow_the_paired_capabilities() {
        let watcher = ScreenGrants::from_capabilities(
            device(),
            capabilities(&[
                ControllerCapability::ObserveSessions,
                ControllerCapability::ObserveScreens,
            ]),
        );
        assert!(watcher.can_view && !watcher.can_control());

        let driver = ScreenGrants::from_capabilities(
            device(),
            capabilities(&[
                ControllerCapability::ObserveScreens,
                ControllerCapability::ControlKeyboard,
            ]),
        );
        assert!(driver.can_control() && !driver.can_control_pointer);
        assert!(driver.can_control_keyboard);

        // Terminal input says nothing about screens.
        let terminal_only = ScreenGrants::from_capabilities(
            device(),
            capabilities(&[ControllerCapability::SendInput]),
        );
        assert!(!terminal_only.can_view && !terminal_only.can_control());
    }

    #[test]
    fn a_device_that_cannot_watch_gets_no_ticket() {
        let mut store = ScreenTicketStore::default();
        let grants = ScreenGrants::from_capabilities(
            device(),
            capabilities(&[ControllerCapability::ControlPointer]),
        );
        assert_eq!(
            store
                .issue(grants, &mut Counting(0))
                .map_err(|error| error.code),
            Err(ListenerErrorCode::Unauthorized)
        );
        assert!(store.active().is_none());
    }

    #[test]
    fn a_ticket_works_once_and_a_wrong_one_never_does() {
        let mut store = ScreenTicketStore::default();
        let grants = ScreenGrants::from_capabilities(
            device(),
            capabilities(&[
                ControllerCapability::ObserveScreens,
                ControllerCapability::ControlPointer,
            ]),
        );
        let ticket = store.issue(grants, &mut Counting(0)).unwrap();
        let mut wrong = ticket;
        wrong[31] ^= 1;
        assert_eq!(store.spend(&wrong), None);
        assert_eq!(store.spend(&ticket), Some(grants));
        assert_eq!(store.active(), Some(grants));
        assert_eq!(store.spend(&ticket), None, "a ticket is spent only once");

        // Asking again replaces the ticket, and the old one is dead.
        let second = store.issue(grants, &mut Counting(7)).unwrap();
        assert_ne!(second, ticket);
        assert_eq!(store.spend(&ticket), None);
        assert_eq!(store.spend(&second), Some(grants));

        store.close();
        assert!(store.active().is_none());
        assert_eq!(store.spend(&second), None);
    }
}
