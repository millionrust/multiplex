//! Spike 5.3b: the QUIC route for Stage B, over iroh.
//!
//! **Why this is a spike outside the workspace rather than a crate inside it.** iroh 1.2 cannot be
//! added to the TermiRust workspace today. Its `iroh-base` needs `zeroize ^1.9`, and the desktop
//! crate pins `zeroize = "=1.8.2"` — a pin the controller-security ADR records by name as part of
//! its dependency review. Its `ed25519-dalek 3.0.0-rc` needs a released `rand_core ^0.10`, and
//! `russh` up to 0.59 pins `rand_core = "=0.10.0-rc-3"` exactly; only russh 0.60 relaxes that.
//! Adopting iroh therefore means upgrading the SSH stack *and* moving a version the security ADR
//! pins, which that ADR says must be amended first and whose acceptance is a release gate.
//!
//! So the code is proved here, with its own lock file, and the decision is left where it belongs.
//! Everything above the seam talks to [`Transport`], so when the dependency question is settled
//! this moves into `termirust-screen-transport` as a module and nothing else changes.
//!
//! The seam's three types are copied here rather than depended on, because depending on the
//! workspace crate would drag the workspace lock back in and defeat the point. They are small, and
//! the copy is checked against the original by a test.

/// What a route promises. Mirrors `termirust_screen_transport::Delivery`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Delivery {
    Reliable,
    Unreliable,
}

/// What a message needs. Mirrors `termirust_screen_protocol::Class`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Class {
    Control,
    Tiles,
    Video,
}

/// Why a send failed. Mirrors `termirust_screen_transport::TransportError`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransportError {
    Closed,
    TooLarge,
    Io,
}

/// Somewhere a session's bytes go. Mirrors `termirust_screen_transport::Transport`.
pub trait Transport: Send {
    fn delivery(&self, class: Class) -> Delivery;
    fn send(&mut self, class: Class, bytes: &[u8]) -> Result<(), TransportError>;
    fn maximum_chunk(&self, _class: Class) -> Option<usize> {
        None
    }
}

mod transport;

pub use transport::{ALPN, IrohTransport, RESUME_WINDOW};
