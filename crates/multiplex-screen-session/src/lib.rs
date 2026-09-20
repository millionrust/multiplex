//! Host and viewer state machines for a Remote Screens session.
//!
//! Neither side performs I/O. The host feeds received messages and captured frames in and takes
//! messages out; the viewer does the same with batches and user actions. The caller moves the
//! messages over a transport with `multiplex-screen-protocol` framing, authenticates the peer, and
//! decides who holds control. See `docs/remote-screens-implementation-plan.md`, sections 4.1–4.7.

#![forbid(unsafe_code)]

mod error;
mod host;
mod input;
mod motion;
mod video;
mod viewer;

pub use error::SessionError;
pub use host::{
    Grants, HostConfig, HostEvent, HostSession, Limits, PANE_MASK_BGRA, ResumeStore,
    THUMBNAIL_CACHE_BYTES, THUMBNAIL_SURFACE_BIT, TicketVerifier,
};
pub use input::InputEvent;
pub use viewer::{ViewerEvent, ViewerSession};
