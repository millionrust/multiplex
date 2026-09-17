//! Turns successive captured frames into batches of tile operations for one viewer.
//!
//! The encoder remembers which batch last changed each tile, which batches moved which areas,
//! and which batches added which cache entries. A viewer that reconnects says which batch it
//! applied last, and only tiles touched after that are sent again.

use std::collections::VecDeque;

use crate::{
    Batch, CacheMiss, CacheShadow, CodecError, DESKTOP_CACHE_BYTES, Frame, FrameBuffer, Generation,
    LossyDetail, MotionConfig, MotionEvent, MotionTracker, Rect, Size, SurfaceId, TileClass,
    TileGrid, TileHash, TileHashes, TileIndex, TileOp, TileSet, classify, detect_vertical_move,
    encode_lossless, encode_lossy, hash_rect,
};

/// Moves and cache insertions remembered for resuming. A viewer that acknowledged a batch older
/// than this history receives a full refresh instead.
pub const MAX_RESUME_HISTORY: usize = 4_096;

/// How a reconnecting viewer is brought up to date.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Resume {
    /// Only tiles changed after the acknowledged batch are sent again.
    Partial,
    /// The acknowledgement was unknown or too old; every tile is sent again.
    Full,
}

/// Tunables for one viewer's encoder.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EncoderConfig {
    /// Must equal the viewer's cache budget so the shadow evicts in the same order.
    pub cache_bytes: usize,
    pub lossy_detail: LossyDetail,
    pub detect_scrolls: bool,
    /// Motion region thresholds, used by [`Encoder::encode_at`].
    pub motion: MotionConfig,
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self {
            cache_bytes: DESKTOP_CACHE_BYTES,
            lossy_detail: LossyDetail::STANDARD,
            detect_scrolls: true,
            motion: MotionConfig::default(),
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
    /// Batch that last changed each tile's viewer state.
    sent_at: Vec<u64>,
    moves: VecDeque<(u64, Rect)>,
    inserted: VecDeque<(u64, TileHash)>,
    acked: u64,
    /// A resume must acknowledge at least this batch; older history was dropped.
    oldest_resumable: u64,
    motion: MotionTracker,
    motion_event: Option<MotionEvent>,
    motion_last_sent_ms: Option<u64>,
    /// When each tile's source pixels last changed, from [`Encoder::encode_at`].
    changed_at_ms: Vec<u64>,
}

/// How long a lossy tile must stay unchanged before it is refined to exact pixels.
pub const REFINE_IDLE_MS: u64 = 250;

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
            sent_at: vec![0; grid.len()],
            moves: VecDeque::new(),
            inserted: VecDeque::new(),
            acked: 0,
            oldest_resumable: 1,
            motion: MotionTracker::new(grid, config.motion),
            motion_event: None,
            motion_last_sent_ms: None,
            changed_at_ms: vec![0; grid.len()],
        }
    }

    /// Number of tiles the viewer holds only as a lossy approximation.
    pub fn approximate_tiles(&self) -> usize {
        self.viewer
            .iter()
            .filter(|state| matches!(state, ViewerTile::Approximate(_)))
            .count()
    }

    /// Replaces lossy tiles that have been idle for [`REFINE_IDLE_MS`] with exact pixels, oldest
    /// first, until about `budget_bytes` of payload is used. Tiles in the motion region wait for
    /// it to end. Returns `None` when nothing is ready. Callers pause refinement by not calling it.
    pub fn refine_at(
        &mut self,
        now_ms: u64,
        budget_bytes: usize,
    ) -> Result<Option<Batch>, CodecError> {
        let Some(hashes) = self.hashes.as_ref() else {
            return Ok(None);
        };
        let mut ready: Vec<(u64, TileIndex)> = self
            .viewer
            .iter()
            .enumerate()
            .filter_map(|(index, state)| {
                let tile = TileIndex(index as u32);
                let ViewerTile::Approximate(hash) = *state else {
                    return None;
                };
                let idle = now_ms.saturating_sub(self.changed_at_ms[index]) >= REFINE_IDLE_MS;
                (idle && hashes.get(tile) == Some(hash) && !self.motion.contains(tile))
                    .then_some((self.changed_at_ms[index], tile))
            })
            .collect();
        if ready.is_empty() {
            return Ok(None);
        }
        ready.sort_unstable();

        let mut ops = Vec::new();
        let mut spent = 0;
        for (_, tile) in ready {
            if spent >= budget_bytes && !ops.is_empty() {
                break;
            }
            let hash = hashes.get(tile).expect("tile from this grid");
            let rect = self.grid.tile_rect(tile).expect("tile from this grid");
            let op = if self.shadow.use_if_held(hash) {
                spent += 13;
                TileOp::Cached { tile, hash }
            } else {
                let payload = encode_lossless(&self.source.as_frame(), rect);
                spent += payload.len() + 17;
                self.shadow.record_sent(hash, pixel_bytes(rect));
                remember(
                    &mut self.inserted,
                    &mut self.oldest_resumable,
                    (self.next_sequence, hash),
                );
                TileOp::Lossless {
                    tile,
                    hash,
                    payload,
                }
            };
            self.viewer[tile.0 as usize] = ViewerTile::Exact(hash);
            self.sent_at[tile.0 as usize] = self.next_sequence;
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
        Ok(Some(batch))
    }

    /// How much detail the first pass currently keeps.
    pub const fn lossy_detail(&self) -> LossyDetail {
        self.config.lossy_detail
    }

    /// The area currently treated as video, if any.
    pub fn motion_region(&self) -> Option<Rect> {
        self.motion.region()
    }

    /// The motion region change produced by the last [`Encoder::encode_at`], if any.
    pub const fn motion_event(&self) -> Option<MotionEvent> {
        self.motion_event
    }

    /// The viewer applied every batch up to `sequence`; history at or before it is dropped.
    pub fn acknowledge(&mut self, sequence: u64) {
        let sequence = sequence.min(self.next_sequence - 1);
        if sequence <= self.acked {
            return;
        }
        self.acked = sequence;
        while self.moves.front().is_some_and(|(at, _)| *at <= sequence) {
            self.moves.pop_front();
        }
        while self.inserted.front().is_some_and(|(at, _)| *at <= sequence) {
            self.inserted.pop_front();
        }
    }

    /// Prepares the next batch for a viewer that reconnected having applied batches up to
    /// `acknowledged`. Tiles, moves, and cache entries from later batches are treated as lost.
    pub fn resume(&mut self, acknowledged: u64) -> Resume {
        let acknowledged = acknowledged.min(self.next_sequence - 1);
        if acknowledged == 0 || acknowledged < self.acked || acknowledged < self.oldest_resumable {
            self.reset_viewer();
            return Resume::Full;
        }
        for (state, at) in self.viewer.iter_mut().zip(&self.sent_at) {
            if *at > acknowledged {
                *state = ViewerTile::Unknown;
            }
        }
        for (_, rect) in self.moves.iter().filter(|(at, _)| *at > acknowledged) {
            for tile in self.grid.tiles_covering(*rect).iter() {
                self.viewer[tile.0 as usize] = ViewerTile::Unknown;
            }
        }
        for (_, hash) in self.inserted.iter().filter(|(at, _)| *at > acknowledged) {
            self.shadow.forget(*hash);
        }
        self.moves.clear();
        self.inserted.clear();
        self.acked = acknowledged;
        Resume::Partial
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
    /// operating system's changed rectangles; `None` means compare every tile. No motion region
    /// is tracked; use [`Encoder::encode_at`] for live capture.
    pub fn encode(
        &mut self,
        frame: &Frame<'_>,
        damage: Option<&[Rect]>,
    ) -> Result<Batch, CodecError> {
        self.encode_inner(frame, damage, None)
    }

    /// Encodes like [`Encoder::encode`] for a frame captured at `now_ms`, and tracks the motion
    /// region. On the tile path, tiles inside the region are sent as lossy tiles no more often
    /// than `tile_path_max_hz`; when the region ends, its tiles are sent again.
    pub fn encode_at(
        &mut self,
        frame: &Frame<'_>,
        damage: Option<&[Rect]>,
        now_ms: u64,
    ) -> Result<Batch, CodecError> {
        self.encode_inner(frame, damage, Some(now_ms))
    }

    fn encode_inner(
        &mut self,
        frame: &Frame<'_>,
        damage: Option<&[Rect]>,
        now_ms: Option<u64>,
    ) -> Result<Batch, CodecError> {
        if frame.size() != self.grid.size() {
            return Err(CodecError::FrameSizeMismatch);
        }
        let mut ops = Vec::new();
        let content_changed = match &mut self.hashes {
            None => {
                self.hashes = Some(TileHashes::compute(frame));
                TileSet::full(self.grid)
            }
            Some(hashes) => {
                let candidates = damage.map(|rects| self.grid.tiles_for_damage(rects));
                hashes.update(frame, candidates.as_ref())?
            }
        };
        let mut changed = content_changed.clone();
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

        self.motion_event = None;
        let mut motion_throttled = false;
        if let Some(now) = now_ms {
            // A scroll the move already explains is not motion: only tiles still different count.
            let hashes = self
                .hashes
                .as_ref()
                .expect("hashes exist after the first frame");
            let mut still_different = TileSet::new(self.grid);
            for tile in content_changed.iter() {
                self.changed_at_ms[tile.0 as usize] = now;
                let hash = hashes.get(tile).expect("tile from this grid");
                if self.viewer[tile.0 as usize] != ViewerTile::Exact(hash) {
                    still_different.insert(tile);
                }
            }
            self.motion_event = self.motion.observe(&still_different, now);
            if let Some(MotionEvent::Demoted(rect)) = self.motion_event {
                for tile in self.grid.tiles_covering(rect).iter() {
                    self.viewer[tile.0 as usize] = ViewerTile::Unknown;
                    changed.insert(tile);
                }
                self.motion_last_sent_ms = None;
            }
            if self.motion.region().is_some() {
                let interval = 1_000 / u64::from(self.config.motion.tile_path_max_hz.max(1));
                motion_throttled = self
                    .motion_last_sent_ms
                    .is_some_and(|last| now.saturating_sub(last) < interval);
                if !motion_throttled {
                    self.motion_last_sent_ms = Some(now);
                }
            }
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
            let in_motion = now_ms.is_some() && self.motion.contains(tile);
            if in_motion && motion_throttled {
                // Sent by a later batch, at the region's update rate.
                self.viewer[tile.0 as usize] = ViewerTile::Unknown;
                continue;
            }
            let rect = self.grid.tile_rect(tile).expect("tile from this grid");
            let class = match classify(frame, rect) {
                TileClass::Solid(color) => TileClass::Solid(color),
                _ if in_motion => TileClass::Picture,
                class => class,
            };
            let (op, state) = match class {
                TileClass::Solid(color) => (TileOp::Solid { tile, color }, ViewerTile::Exact(hash)),
                _ if self.shadow.use_if_held(hash) => {
                    (TileOp::Cached { tile, hash }, ViewerTile::Exact(hash))
                }
                TileClass::Text => {
                    self.shadow.record_sent(hash, pixel_bytes(rect));
                    remember(
                        &mut self.inserted,
                        &mut self.oldest_resumable,
                        (self.next_sequence, hash),
                    );
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
            self.sent_at[tile.0 as usize] = self.next_sequence;
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
        self.moves.clear();
        self.inserted.clear();
        self.oldest_resumable = self.next_sequence;
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
            self.sent_at[tile.0 as usize] = self.next_sequence;
            changed.insert(tile);
        }
        remember(
            &mut self.moves,
            &mut self.oldest_resumable,
            (self.next_sequence, found.rect),
        );
        Some(TileOp::Move {
            rect: found.rect,
            dy: found.dy,
        })
    }
}

/// Appends to a bounded history; dropping an entry means resumes must acknowledge past it.
fn remember<T>(history: &mut VecDeque<(u64, T)>, oldest_resumable: &mut u64, entry: (u64, T)) {
    history.push_back(entry);
    if history.len() > MAX_RESUME_HISTORY
        && let Some((dropped, _)) = history.pop_front()
    {
        *oldest_resumable = (*oldest_resumable).max(dropped);
    }
}

const fn pixel_bytes(rect: Rect) -> usize {
    rect.width as usize * rect.height as usize * crate::BYTES_PER_PIXEL
}
