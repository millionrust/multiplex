//! Platform-free tile codec for TermiRust Remote Screens.
//!
//! A remote screen is a surface split into 64×64 tiles. Only tiles whose pixels changed are
//! encoded, each in the cheapest form that reproduces it: skipped, a solid colour, a reference to
//! a tile the viewer already holds, a vertical move, lossless pixels for text and UI, or a lossy
//! first pass for pictures that is refined later. This crate has no capture, network, or UI code;
//! see `docs/remote-screens-implementation-plan.md`, section 4.3.

#![forbid(unsafe_code)]

mod error;
mod geometry;

pub use error::CodecError;
pub use geometry::{MAX_SURFACE_DIMENSION, Rect, Size, TILE_SIZE, TileGrid, TileIndex, TileSet};
