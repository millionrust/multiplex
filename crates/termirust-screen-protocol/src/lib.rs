//! Control messages and stream framing for a Remote Screens session.
//!
//! A session runs over any ordered byte stream (a QUIC stream, or a Controller channel in tests).
//! Each frame is a big-endian `u32` length followed by that many bytes: one kind byte and a body.
//! Tile batches travel as [`Message::Batch`] frames carrying a `TSB1` batch from
//! `termirust-screen-codec`. Everything is bounded and parsed without panics; see
//! `docs/remote-screens-implementation-plan.md`, sections 4.5 to 4.7.
//!
//! This crate performs no I/O and no authentication. The Controller channel issues the ticket
//! whose proof opens a session, and the transport authenticates the peer.

#![forbid(unsafe_code)]

mod error;
mod framing;
mod message;

pub use error::ProtocolError;
pub use framing::{FrameReader, MAX_CONTROL_FRAME_BYTES, MAX_FRAME_BYTES, encode_frame};
pub use message::{
    ControlHolder, Hello, KeyEvent, Message, Modifiers, PROTOCOL_VERSION, PointerButton, Profile,
    ResumeOutcome, ResumeRequest, SurfaceInfo, Viewport, Welcome,
};
