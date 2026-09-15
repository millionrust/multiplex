//! Platform-free tile codec for TermiRust Remote Screens.
//!
//! A remote screen is a surface split into 64×64 tiles. Only tiles whose pixels changed are
//! encoded, each in the cheapest form that reproduces it: skipped, a solid colour, a reference to
//! a tile the viewer already holds, a vertical move, lossless pixels for text and UI, or a lossy
//! first pass for pictures that is refined later. This crate has no capture, network, or UI code;
//! see `docs/remote-screens-implementation-plan.md`, section 4.3.

#![forbid(unsafe_code)]

mod classify;
mod error;
mod frame;
mod geometry;
mod hash;
mod lossless;
mod lossy;
mod wire;

pub use classify::{
    EDGE_CHANNEL_DELTA, MAX_PALETTE_COLORS, TEXT_EDGE_DENSITY_MILLI, TileClass, classify,
};
pub use error::CodecError;
pub use frame::{BYTES_PER_PIXEL, Frame, FrameBuffer};
pub use geometry::{MAX_SURFACE_DIMENSION, Rect, Size, TILE_SIZE, TileGrid, TileIndex, TileSet};
pub use hash::{TileHash, TileHashes, hash_rect};
pub use lossless::{decode_lossless, encode_lossless};
pub use lossy::{LossyDetail, decode_lossy, encode_lossy};
pub use wire::{
    BATCH_HEADER_BYTES, BATCH_MAGIC, BATCH_VERSION, Batch, Generation, MAX_BATCH_BYTES,
    MAX_TILE_PAYLOAD_BYTES, SurfaceId, TileOp,
};
