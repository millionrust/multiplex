//! The bounded binary layout of one batch of tile operations.
//!
//! All integers are big-endian. A batch is a 36-byte header followed by `op_count` operations:
//!
//! | Offset | Size | Field |
//! |---:|---:|---|
//! | 0 | 4 | ASCII `TSB1` |
//! | 4 | 2 | version, exactly 1 |
//! | 6 | 2 | reserved, zero |
//! | 8 | 4 | surface id |
//! | 12 | 4 | surface generation |
//! | 16 | 8 | batch sequence |
//! | 24 | 4 | surface width, 1–16384 |
//! | 28 | 4 | surface height, 1–16384 |
//! | 32 | 4 | operation count |
//!
//! Operations start with a kind byte:
//!
//! | Kind | Operation | Body |
//! |---:|---|---|
//! | 1 | solid | tile u32, B, G, R |
//! | 2 | cached | tile u32, hash u64 |
//! | 3 | lossless | tile u32, hash u64, length u32, payload |
//! | 4 | lossy | tile u32, length u32, payload |
//! | 5 | move | x u32, y u32, width u32, height u32, dy i32 |
//!
//! Unknown kinds, non-zero reserved bytes, tiles outside the grid, empty or oversized payloads,
//! moves that read outside the surface, and trailing bytes are rejected.

use crate::{CodecError, MAX_SURFACE_DIMENSION, Rect, Size, TileGrid, TileHash, TileIndex};

pub const BATCH_MAGIC: [u8; 4] = *b"TSB1";
pub const BATCH_VERSION: u16 = 1;
pub const BATCH_HEADER_BYTES: usize = 36;
/// Largest encoded batch, matching the Controller terminal frame bound.
pub const MAX_BATCH_BYTES: usize = 1 << 20;
/// Largest single tile payload. A 64×64 delta tile inflates to 12,289 bytes before deflate.
pub const MAX_TILE_PAYLOAD_BYTES: usize = 16_384;

const KIND_SOLID: u8 = 1;
const KIND_CACHED: u8 = 2;
const KIND_LOSSLESS: u8 = 3;
const KIND_LOSSY: u8 = 4;
const KIND_MOVE: u8 = 5;

/// Identifies one captured surface (a display, all displays, or a window) within a session.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SurfaceId(pub u32);

/// Changes whenever a surface's size or meaning changes; viewers discard older state.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Generation(pub u32);

/// One instruction for the viewer's framebuffer. Operations apply in order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TileOp {
    /// Fill the tile with one colour.
    Solid { tile: TileIndex, color: [u8; 3] },
    /// Draw the cached tile with this hash.
    Cached { tile: TileIndex, hash: TileHash },
    /// Exact pixels; the viewer caches them under `hash`.
    Lossless {
        tile: TileIndex,
        hash: TileHash,
        payload: Vec<u8>,
    },
    /// An approximate first pass; never cached.
    Lossy { tile: TileIndex, payload: Vec<u8> },
    /// Each row `y` of `rect` takes the pixels of row `y + dy`, read before the move.
    Move { rect: Rect, dy: i32 },
}

/// Operations for one surface, numbered so a viewer can acknowledge them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Batch {
    pub surface: SurfaceId,
    pub generation: Generation,
    pub sequence: u64,
    pub size: Size,
    pub ops: Vec<TileOp>,
}

impl Batch {
    /// Serialises the batch after checking every operation against the surface.
    pub fn encode(&self) -> Result<Vec<u8>, CodecError> {
        let grid = TileGrid::new(self.size);
        let mut out = Vec::with_capacity(BATCH_HEADER_BYTES + self.ops.len() * 13);
        out.extend(BATCH_MAGIC);
        out.extend(BATCH_VERSION.to_be_bytes());
        out.extend([0, 0]);
        out.extend(self.surface.0.to_be_bytes());
        out.extend(self.generation.0.to_be_bytes());
        out.extend(self.sequence.to_be_bytes());
        out.extend(self.size.width().to_be_bytes());
        out.extend(self.size.height().to_be_bytes());
        let count = u32::try_from(self.ops.len()).map_err(|_| CodecError::BatchTooLarge)?;
        out.extend(count.to_be_bytes());
        for op in &self.ops {
            validate_op(op, grid)?;
            match op {
                TileOp::Solid { tile, color } => {
                    out.push(KIND_SOLID);
                    out.extend(tile.0.to_be_bytes());
                    out.extend(color);
                }
                TileOp::Cached { tile, hash } => {
                    out.push(KIND_CACHED);
                    out.extend(tile.0.to_be_bytes());
                    out.extend(hash.0.to_be_bytes());
                }
                TileOp::Lossless {
                    tile,
                    hash,
                    payload,
                } => {
                    out.push(KIND_LOSSLESS);
                    out.extend(tile.0.to_be_bytes());
                    out.extend(hash.0.to_be_bytes());
                    out.extend((payload.len() as u32).to_be_bytes());
                    out.extend(payload);
                }
                TileOp::Lossy { tile, payload } => {
                    out.push(KIND_LOSSY);
                    out.extend(tile.0.to_be_bytes());
                    out.extend((payload.len() as u32).to_be_bytes());
                    out.extend(payload);
                }
                TileOp::Move { rect, dy } => {
                    out.push(KIND_MOVE);
                    for field in [rect.x, rect.y, rect.width, rect.height] {
                        out.extend(field.to_be_bytes());
                    }
                    out.extend(dy.to_be_bytes());
                }
            }
            if out.len() > MAX_BATCH_BYTES {
                return Err(CodecError::BatchTooLarge);
            }
        }
        Ok(out)
    }

    /// Parses and validates a batch. Nothing is allocated before its length is checked.
    pub fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        if bytes.len() > MAX_BATCH_BYTES {
            return Err(CodecError::BatchTooLarge);
        }
        let mut reader = Reader { bytes, at: 0 };
        if reader.take(4)? != BATCH_MAGIC {
            return Err(CodecError::MalformedBatch);
        }
        if reader.u16()? != BATCH_VERSION {
            return Err(CodecError::UnsupportedVersion);
        }
        if reader.u16()? != 0 {
            return Err(CodecError::MalformedBatch);
        }
        let surface = SurfaceId(reader.u32()?);
        let generation = Generation(reader.u32()?);
        let sequence = reader.u64()?;
        let width = reader.u32()?;
        let height = reader.u32()?;
        if width > MAX_SURFACE_DIMENSION || height > MAX_SURFACE_DIMENSION {
            return Err(CodecError::MalformedBatch);
        }
        let size = Size::new(width, height).map_err(|_| CodecError::MalformedBatch)?;
        let grid = TileGrid::new(size);
        let count = reader.u32()? as usize;
        // The smallest operation is 8 bytes, so a count the remaining bytes cannot hold is a lie.
        if count > reader.remaining() / 8 {
            return Err(CodecError::MalformedBatch);
        }
        let mut ops = Vec::with_capacity(count);
        for _ in 0..count {
            let op = match reader.u8()? {
                KIND_SOLID => TileOp::Solid {
                    tile: TileIndex(reader.u32()?),
                    color: reader.take(3)?.try_into().expect("three bytes"),
                },
                KIND_CACHED => TileOp::Cached {
                    tile: TileIndex(reader.u32()?),
                    hash: TileHash(reader.u64()?),
                },
                KIND_LOSSLESS => TileOp::Lossless {
                    tile: TileIndex(reader.u32()?),
                    hash: TileHash(reader.u64()?),
                    payload: reader.payload()?,
                },
                KIND_LOSSY => TileOp::Lossy {
                    tile: TileIndex(reader.u32()?),
                    payload: reader.payload()?,
                },
                KIND_MOVE => TileOp::Move {
                    rect: Rect::new(reader.u32()?, reader.u32()?, reader.u32()?, reader.u32()?),
                    dy: reader.u32()? as i32,
                },
                _ => return Err(CodecError::MalformedBatch),
            };
            validate_op(&op, grid)?;
            ops.push(op);
        }
        if reader.remaining() != 0 {
            return Err(CodecError::MalformedBatch);
        }
        Ok(Self {
            surface,
            generation,
            sequence,
            size,
            ops,
        })
    }
}

fn validate_op(op: &TileOp, grid: TileGrid) -> Result<(), CodecError> {
    match op {
        TileOp::Solid { tile, .. } | TileOp::Cached { tile, .. } => check_tile(*tile, grid),
        TileOp::Lossless { tile, payload, .. } | TileOp::Lossy { tile, payload } => {
            check_tile(*tile, grid)?;
            if payload.is_empty() || payload.len() > MAX_TILE_PAYLOAD_BYTES {
                return Err(CodecError::MalformedBatch);
            }
            Ok(())
        }
        TileOp::Move { rect, dy } => {
            let bounds = grid.size().bounds();
            let source_y = i64::from(rect.y) + i64::from(*dy);
            let source_bottom = i64::from(rect.bottom()) + i64::from(*dy);
            if rect.is_empty()
                || *dy == 0
                || !bounds.contains_rect(*rect)
                || source_y < 0
                || source_bottom > i64::from(bounds.height)
            {
                return Err(CodecError::InvalidMove);
            }
            Ok(())
        }
    }
}

fn check_tile(tile: TileIndex, grid: TileGrid) -> Result<(), CodecError> {
    if grid.contains(tile) {
        Ok(())
    } else {
        Err(CodecError::TileOutOfRange)
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn remaining(&self) -> usize {
        self.bytes.len() - self.at
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], CodecError> {
        if self.remaining() < count {
            return Err(CodecError::MalformedBatch);
        }
        let slice = &self.bytes[self.at..self.at + count];
        self.at += count;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, CodecError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, CodecError> {
        Ok(u16::from_be_bytes(
            self.take(2)?.try_into().expect("two bytes"),
        ))
    }

    fn u32(&mut self) -> Result<u32, CodecError> {
        Ok(u32::from_be_bytes(
            self.take(4)?.try_into().expect("four bytes"),
        ))
    }

    fn u64(&mut self) -> Result<u64, CodecError> {
        Ok(u64::from_be_bytes(
            self.take(8)?.try_into().expect("eight bytes"),
        ))
    }

    fn payload(&mut self) -> Result<Vec<u8>, CodecError> {
        let length = self.u32()? as usize;
        if length == 0 || length > MAX_TILE_PAYLOAD_BYTES {
            return Err(CodecError::MalformedBatch);
        }
        Ok(self.take(length)?.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn sample() -> Batch {
        Batch {
            surface: SurfaceId(7),
            generation: Generation(2),
            sequence: 0x0102_0304_0506_0708,
            size: Size::new(1512, 982).unwrap(),
            ops: vec![
                TileOp::Solid {
                    tile: TileIndex(1),
                    color: [0x17, 0x19, 0x1D],
                },
                TileOp::Cached {
                    tile: TileIndex(2),
                    hash: TileHash(0xAABB_CCDD_EEFF_0011),
                },
                TileOp::Lossless {
                    tile: TileIndex(383),
                    hash: TileHash(9),
                    payload: vec![0xDE, 0xAD],
                },
                TileOp::Lossy {
                    tile: TileIndex(0),
                    payload: vec![0x01],
                },
                TileOp::Move {
                    rect: Rect::new(891, 91, 294, 418),
                    dy: 18,
                },
            ],
        }
    }

    #[test]
    fn golden_layout_is_stable() {
        let mut expected: Vec<u8> = Vec::new();
        expected.extend(b"TSB1");
        expected.extend([0x00, 0x01, 0x00, 0x00]);
        expected.extend([0, 0, 0, 7, 0, 0, 0, 2, 1, 2, 3, 4, 5, 6, 7, 8]);
        expected.extend([0x00, 0x00, 0x05, 0xE8, 0x00, 0x00, 0x03, 0xD6, 0, 0, 0, 5]);
        expected.extend([1, 0, 0, 0, 1, 0x17, 0x19, 0x1D]);
        expected.extend([
            2, 0, 0, 0, 2, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x00, 0x11,
        ]);
        expected.extend([
            3, 0, 0, 1, 0x7F, 0, 0, 0, 0, 0, 0, 0, 9, 0, 0, 0, 2, 0xDE, 0xAD,
        ]);
        expected.extend([4, 0, 0, 0, 0, 0, 0, 0, 1, 0x01]);
        expected.extend([
            5, 0, 0, 0x03, 0x7B, 0, 0, 0, 0x5B, 0, 0, 0x01, 0x26, 0, 0, 0x01, 0xA2, 0, 0, 0, 0x12,
        ]);
        let encoded = sample().encode().unwrap();
        assert_eq!(encoded, expected);
        assert_eq!(Batch::decode(&expected).unwrap(), sample());
    }

    #[test]
    fn header_fields_are_checked() {
        let good = sample().encode().unwrap();
        let mut bad_magic = good.clone();
        bad_magic[0] = b'X';
        assert_eq!(Batch::decode(&bad_magic), Err(CodecError::MalformedBatch));
        let mut bad_version = good.clone();
        bad_version[5] = 2;
        assert_eq!(
            Batch::decode(&bad_version),
            Err(CodecError::UnsupportedVersion)
        );
        let mut reserved = good.clone();
        reserved[7] = 1;
        assert_eq!(Batch::decode(&reserved), Err(CodecError::MalformedBatch));
        let mut trailing = good.clone();
        trailing.push(0);
        assert_eq!(Batch::decode(&trailing), Err(CodecError::MalformedBatch));
        let mut huge_count = good.clone();
        huge_count[32..36].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(Batch::decode(&huge_count), Err(CodecError::MalformedBatch));
        assert_eq!(
            Batch::decode(&good[..good.len() - 1]),
            Err(CodecError::MalformedBatch)
        );
    }

    #[test]
    fn operations_must_fit_the_surface() {
        let mut batch = sample();
        batch.ops = vec![TileOp::Solid {
            tile: TileIndex(384),
            color: [0; 3],
        }];
        assert_eq!(batch.encode(), Err(CodecError::TileOutOfRange));
        batch.ops = vec![TileOp::Move {
            rect: Rect::new(0, 900, 10, 82),
            dy: 1,
        }];
        assert_eq!(batch.encode(), Err(CodecError::InvalidMove));
        batch.ops = vec![TileOp::Move {
            rect: Rect::new(0, 5, 10, 10),
            dy: -6,
        }];
        assert_eq!(batch.encode(), Err(CodecError::InvalidMove));
        batch.ops = vec![TileOp::Move {
            rect: Rect::new(0, 5, 10, 10),
            dy: 0,
        }];
        assert_eq!(batch.encode(), Err(CodecError::InvalidMove));
        batch.ops = vec![TileOp::Lossy {
            tile: TileIndex(0),
            payload: vec![],
        }];
        assert_eq!(batch.encode(), Err(CodecError::MalformedBatch));
        batch.ops = vec![TileOp::Lossy {
            tile: TileIndex(0),
            payload: vec![0; MAX_TILE_PAYLOAD_BYTES + 1],
        }];
        assert_eq!(batch.encode(), Err(CodecError::MalformedBatch));
    }

    #[test]
    fn oversized_batches_are_refused() {
        let mut batch = sample();
        batch.ops = (0..70)
            .map(|_| TileOp::Lossy {
                tile: TileIndex(0),
                payload: vec![0; MAX_TILE_PAYLOAD_BYTES],
            })
            .collect();
        assert_eq!(batch.encode(), Err(CodecError::BatchTooLarge));
        assert_eq!(
            Batch::decode(&vec![0; MAX_BATCH_BYTES + 1]),
            Err(CodecError::BatchTooLarge)
        );
    }

    proptest! {
        #[test]
        fn random_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
            let _ = Batch::decode(&bytes);
        }

        #[test]
        fn mutated_batches_never_panic(index in 0usize..96, value in any::<u8>()) {
            let mut bytes = sample().encode().unwrap();
            let at = index % bytes.len();
            bytes[at] = value;
            if let Ok(batch) = Batch::decode(&bytes) {
                prop_assert_eq!(Batch::decode(&batch.encode().unwrap()).unwrap(), batch);
            }
        }
    }
}
