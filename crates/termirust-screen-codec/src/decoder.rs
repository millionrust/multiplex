//! Applies batches to a viewer's framebuffer.

use crate::{
    BYTES_PER_PIXEL, Batch, CacheMiss, CachedTile, CodecError, Frame, FrameBuffer, Generation,
    PHONE_CACHE_BYTES, Rect, Size, SurfaceId, TileGrid, TileOp, ViewerCache, decode_lossless,
    decode_lossy, hash_rect,
};

/// The result of applying one batch.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Applied {
    /// Sequence to acknowledge.
    pub sequence: u64,
    /// Cached references that could not be resolved; report them to the host.
    pub misses: Vec<CacheMiss>,
    /// Areas of the framebuffer that changed, for repainting.
    pub damaged: Vec<Rect>,
    /// True when the batch started a new surface generation and the framebuffer was replaced.
    pub reset: bool,
}

/// A viewer's picture of one remote surface.
#[derive(Debug)]
pub struct Decoder {
    surface: Option<(SurfaceId, Generation)>,
    framebuffer: Option<FrameBuffer>,
    cache: ViewerCache,
    last_sequence: u64,
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new(PHONE_CACHE_BYTES)
    }
}

impl Decoder {
    /// `cache_bytes` must equal the host encoder's `cache_bytes` for this viewer.
    pub fn new(cache_bytes: usize) -> Self {
        Self {
            surface: None,
            framebuffer: None,
            cache: ViewerCache::new(cache_bytes),
            last_sequence: 0,
        }
    }

    pub fn framebuffer(&self) -> Option<&FrameBuffer> {
        self.framebuffer.as_ref()
    }

    /// The framebuffer, to draw into.
    ///
    /// Only the motion region should ever be written this way. The host marks the region's tiles
    /// as unheld while it is streaming and re-sends them all when it demotes, so it never assumes
    /// those pixels are what it last sent — which is what makes overwriting them safe. Writing
    /// anywhere else would leave the two sides disagreeing about the screen.
    pub fn framebuffer_mut(&mut self) -> Option<&mut FrameBuffer> {
        self.framebuffer.as_mut()
    }

    pub const fn last_sequence(&self) -> u64 {
        self.last_sequence
    }

    /// Generation of the surface this decoder last applied, for resume requests.
    pub fn generation(&self) -> Option<Generation> {
        self.surface.map(|(_, generation)| generation)
    }

    /// Applies a batch. Batches from an older generation, repeated sequences, and lossless
    /// pixels that do not match their hash are rejected before anything is drawn from them.
    pub fn apply(&mut self, batch: &Batch) -> Result<Applied, CodecError> {
        let mut applied = Applied {
            sequence: batch.sequence,
            ..Applied::default()
        };
        let same_surface = matches!(self.surface, Some((surface, _)) if surface == batch.surface);
        match self.surface {
            Some((_, generation)) if same_surface && batch.generation < generation => {
                return Err(CodecError::StaleBatch);
            }
            Some((_, generation))
                if same_surface
                    && batch.generation == generation
                    && batch.sequence <= self.last_sequence =>
            {
                return Err(CodecError::StaleBatch);
            }
            _ => {}
        }
        let needs_reset = self.surface != Some((batch.surface, batch.generation))
            || self.framebuffer.as_ref().map(FrameBuffer::size) != Some(batch.size);
        if needs_reset {
            self.surface = Some((batch.surface, batch.generation));
            self.framebuffer = Some(FrameBuffer::new(batch.size));
            applied.reset = true;
        }
        let grid = TileGrid::new(batch.size);
        let framebuffer = self.framebuffer.as_mut().expect("framebuffer exists");
        for op in &batch.ops {
            match op {
                TileOp::Solid { tile, color } => {
                    let rect = tile_rect(grid, *tile)?;
                    framebuffer.fill_rect(rect, [color[0], color[1], color[2], 0xFF])?;
                    applied.damaged.push(rect);
                }
                TileOp::Cached { tile, hash } => {
                    let rect = tile_rect(grid, *tile)?;
                    match self.cache.get(*hash) {
                        Some(cached)
                            if cached.width == rect.width && cached.height == rect.height =>
                        {
                            framebuffer.write_rect(rect, &cached.bgra)?;
                            applied.damaged.push(rect);
                        }
                        _ => applied.misses.push(CacheMiss {
                            tile: *tile,
                            hash: *hash,
                        }),
                    }
                }
                TileOp::Lossless {
                    tile,
                    hash,
                    payload,
                } => {
                    let rect = tile_rect(grid, *tile)?;
                    let bgra = decode_lossless(payload, rect.width, rect.height)?;
                    let size = Size::new(rect.width, rect.height)?;
                    let tile_frame =
                        Frame::new(size, rect.width as usize * BYTES_PER_PIXEL, &bgra)?;
                    if hash_rect(&tile_frame, size.bounds()) != *hash {
                        return Err(CodecError::CorruptPayload);
                    }
                    framebuffer.write_rect(rect, &bgra)?;
                    self.cache.insert(
                        *hash,
                        CachedTile {
                            width: rect.width,
                            height: rect.height,
                            bgra,
                        },
                    );
                    applied.damaged.push(rect);
                }
                TileOp::Lossy { tile, payload } => {
                    let rect = tile_rect(grid, *tile)?;
                    let bgra = decode_lossy(payload, rect.width, rect.height)?;
                    framebuffer.write_rect(rect, &bgra)?;
                    applied.damaged.push(rect);
                }
                TileOp::Move { rect, dy } => {
                    framebuffer.apply_move(*rect, *dy)?;
                    applied.damaged.push(*rect);
                }
            }
        }
        self.last_sequence = batch.sequence;
        Ok(applied)
    }
}

fn tile_rect(grid: TileGrid, tile: crate::TileIndex) -> Result<Rect, CodecError> {
    grid.tile_rect(tile).ok_or(CodecError::TileOutOfRange)
}
