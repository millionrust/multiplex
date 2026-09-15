//! Turns successive captured frames into batches of tile operations for one viewer.

use crate::{
    Batch, CacheMiss, CacheShadow, CodecError, DESKTOP_CACHE_BYTES, Frame, FrameBuffer, Generation,
    LossyDetail, Rect, Size, SurfaceId, TileClass, TileGrid, TileHash, TileHashes, TileIndex,
    TileOp, TileSet, classify, detect_vertical_move, encode_lossless, encode_lossy, hash_rect,
};

/// Tunables for one viewer's encoder.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EncoderConfig {
    /// Must equal the viewer's cache budget so the shadow evicts in the same order.
    pub cache_bytes: usize,
    pub lossy_detail: LossyDetail,
    pub detect_scrolls: bool,
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self {
            cache_bytes: DESKTOP_CACHE_BYTES,
            lossy_detail: LossyDetail::STANDARD,
            detect_scrolls: true,
        }
    }
}

/// What the viewer is believed to show in one tile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ViewerTile {
    /// Nothing trustworthy; the tile must be sent.
    Unknown,
    /// Exactly the source pixels with this hash.
    Exact(TileHash),
    /// A lossy approximation of the source pixels with this hash.
    Approximate(TileHash),
}

/// Encodes frames of one surface for one viewer. Frames must be opaque: alpha is not transmitted.
#[derive(Debug)]
pub struct Encoder {
    surface: SurfaceId,
    generation: Generation,
    grid: TileGrid,
    config: EncoderConfig,
    source: FrameBuffer,
    hashes: Option<TileHashes>,
    pub(crate) viewer: Vec<ViewerTile>,
    shadow: CacheShadow,
    next_sequence: u64,
}

impl Encoder {
    pub fn new(
        surface: SurfaceId,
        generation: Generation,
        size: Size,
        config: EncoderConfig,
    ) -> Self {
        let grid = TileGrid::new(size);
        Self {
            surface,
            generation,
            grid,
            config,
            source: FrameBuffer::new(size),
            hashes: None,
            viewer: vec![ViewerTile::Unknown; grid.len()],
            shadow: CacheShadow::new(config.cache_bytes),
            next_sequence: 1,
        }
    }

    pub const fn surface(&self) -> SurfaceId {
        self.surface
    }

    pub const fn generation(&self) -> Generation {
        self.generation
    }

    pub const fn size(&self) -> Size {
        self.grid.size()
    }

    /// Sequence the next batch will carry.
    pub const fn next_sequence(&self) -> u64 {
        self.next_sequence
    }

    /// Changes the lossy detail used for new picture tiles, for the degradation steps.
    pub fn set_lossy_detail(&mut self, detail: LossyDetail) {
        self.config.lossy_detail = detail;
    }

    /// Encodes the difference between the viewer's picture and `frame`. `damage` is the
    /// operating system's changed rectangles; `None` means compare every tile.
    pub fn encode(
        &mut self,
        frame: &Frame<'_>,
        damage: Option<&[Rect]>,
    ) -> Result<Batch, CodecError> {
        if frame.size() != self.grid.size() {
            return Err(CodecError::FrameSizeMismatch);
        }
        let mut ops = Vec::new();
        let mut changed = match &mut self.hashes {
            None => {
                self.hashes = Some(TileHashes::compute(frame));
                TileSet::full(self.grid)
            }
            Some(hashes) => {
                let candidates = damage.map(|rects| self.grid.tiles_for_damage(rects));
                hashes.update(frame, candidates.as_ref())?
            }
        };
        for (index, state) in self.viewer.iter().enumerate() {
            if *state == ViewerTile::Unknown {
                changed.insert(TileIndex(index as u32));
            }
        }

        if self.config.detect_scrolls
            && changed.len() >= 4
            && let Some(op) = self.try_move(frame, &mut changed)
        {
            ops.push(op);
        }

        let hashes = self
            .hashes
            .as_ref()
            .expect("hashes exist after the first frame");
        let mut payload = Vec::new();
        for tile in changed.iter() {
            let hash = hashes.get(tile).expect("tile from this grid");
            if self.viewer[tile.0 as usize] == ViewerTile::Exact(hash) {
                continue;
            }
            let rect = self.grid.tile_rect(tile).expect("tile from this grid");
            let (op, state) = match classify(frame, rect) {
                TileClass::Solid(color) => (TileOp::Solid { tile, color }, ViewerTile::Exact(hash)),
                _ if self.shadow.use_if_held(hash) => {
                    (TileOp::Cached { tile, hash }, ViewerTile::Exact(hash))
                }
                TileClass::Text => {
                    self.shadow.record_sent(hash, pixel_bytes(rect));
                    let payload = encode_lossless(frame, rect);
                    (
                        TileOp::Lossless {
                            tile,
                            hash,
                            payload,
                        },
                        ViewerTile::Exact(hash),
                    )
                }
                TileClass::Picture => {
                    let payload = encode_lossy(frame, rect, self.config.lossy_detail);
                    (
                        TileOp::Lossy { tile, payload },
                        ViewerTile::Approximate(hash),
                    )
                }
            };
            self.viewer[tile.0 as usize] = state;
            payload.clear();
            frame.copy_rect_into(rect, &mut payload);
            self.source.write_rect(rect, &payload)?;
            ops.push(op);
        }

        let batch = Batch {
            surface: self.surface,
            generation: self.generation,
            sequence: self.next_sequence,
            size: self.grid.size(),
            ops,
        };
        self.next_sequence += 1;
        Ok(batch)
    }

    /// Handles a cached reference the viewer could not resolve: the tile is resent next time.
    pub fn cache_miss(&mut self, miss: CacheMiss) {
        self.shadow.forget(miss.hash);
        if let Some(state) = self.viewer.get_mut(miss.tile.0 as usize) {
            *state = ViewerTile::Unknown;
        }
    }

    /// Forgets everything the viewer was believed to hold, for a new or reset viewer.
    pub fn reset_viewer(&mut self) {
        self.viewer
            .iter_mut()
            .for_each(|state| *state = ViewerTile::Unknown);
        self.shadow.clear();
    }

    /// Detects a scroll inside the changed area and applies it to the source copy and to the
    /// per-tile viewer state. Every tile the move touches joins `changed`, so tiles the move left
    /// wrong are resent and tiles it made exact are skipped.
    fn try_move(&mut self, frame: &Frame<'_>, changed: &mut TileSet) -> Option<TileOp> {
        let region = changed
            .iter()
            .filter_map(|tile| self.grid.tile_rect(tile))
            .fold(Rect::default(), Rect::union);
        let found = detect_vertical_move(&self.source.as_frame(), frame, region)?;
        let before = self.viewer.clone();
        self.source.apply_move(found.rect, found.dy).ok()?;
        for tile in self.grid.tiles_covering(found.rect).iter() {
            let rect = self.grid.tile_rect(tile).expect("tile from this grid");
            let moved_part = rect.intersect(found.rect);
            let source_rows = Rect::new(
                moved_part.x,
                (i64::from(moved_part.y) + i64::from(found.dy)) as u32,
                moved_part.width,
                moved_part.height,
            );
            let mut sources = self.grid.tiles_covering(source_rows);
            if moved_part != rect {
                sources.insert(tile);
            }
            let all_exact = sources
                .iter()
                .all(|source| matches!(before[source.0 as usize], ViewerTile::Exact(_)));
            self.viewer[tile.0 as usize] = if all_exact {
                ViewerTile::Exact(hash_rect(&self.source.as_frame(), rect))
            } else {
                ViewerTile::Unknown
            };
            changed.insert(tile);
        }
        Some(TileOp::Move {
            rect: found.rect,
            dy: found.dy,
        })
    }
}

const fn pixel_bytes(rect: Rect) -> usize {
    rect.width as usize * rect.height as usize * crate::BYTES_PER_PIXEL
}
