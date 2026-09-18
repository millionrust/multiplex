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
pub mod fec;
mod framing;
mod message;

pub use error::ProtocolError;
pub use fec::{Fec, MAX_SHARDS, pack_shard, shard_length, unpack_shard};
pub use framing::{FrameReader, MAX_CONTROL_FRAME_BYTES, MAX_FRAME_BYTES, encode_frame};
pub use message::{
    Class, ControlHolder, FEATURES_PROTOCOL_VERSION, FeatureSet, Hello, InputKind, KeyEvent,
    MAX_PANES, MAX_PARITY_BYTES, MAX_VIDEO_FRAME_BYTES, MAX_VIDEO_TOKENS, MINIMUM_PROTOCOL_VERSION,
    Message, Modifiers, MotionCodec, PROTOCOL_VERSION, PanePlacement, PaneSession, Parity,
    PointerButton, Profile, ResumeOutcome, ResumeRequest, SurfaceInfo, VideoConfig, VideoFrame,
    Viewport, Welcome,
};
