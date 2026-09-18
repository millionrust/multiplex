//! Finds the part of the screen that behaves like video.
//!
//! Each tile keeps the times it changed during the last second. A connected area of at least
//! `min_tiles` tiles that each changed at `promote_hz` or faster for `sustain_ms` becomes the
//! motion region. The region ends once none of its tiles changes at `demote_hz` or faster for
//! `demote_ms`. There is one region at a time.

use std::collections::VecDeque;

use crate::{Rect, TileGrid, TileIndex, TileSet};

const WINDOW_MS: u64 = 1_000;

/// Thresholds for promoting and demoting a motion region.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MotionConfig {
    pub promote_hz: u32,
    pub sustain_ms: u64,
    pub demote_hz: u32,
    pub demote_ms: u64,
    pub min_tiles: usize,
    /// The area must span at least this many tiles across and down, so a strip of text revealed
    /// by scrolling or a line being typed is never mistaken for video.
    pub min_side_tiles: u32,
    /// Most tile updates per second sent for the region while it is on the tile path.
    pub tile_path_max_hz: u32,
}

impl Default for MotionConfig {
    fn default() -> Self {
        Self {
            promote_hz: 12,
            sustain_ms: 300,
            demote_hz: 4,
            demote_ms: 500,
            min_tiles: 12,
            min_side_tiles: 3,
            tile_path_max_hz: 5,
        }
    }
}

/// A change in the motion region.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MotionEvent {
    Promoted(Rect),
    Demoted(Rect),
}

/// Tracks tile change rates for one surface.
#[derive(Clone, Debug)]
pub struct MotionTracker {
    grid: TileGrid,
    config: MotionConfig,
    changes: Vec<VecDeque<u64>>,
    region: Option<(Rect, u64)>,
}

impl MotionTracker {
    pub fn new(grid: TileGrid, config: MotionConfig) -> Self {
        Self {
            grid,
            config,
            changes: vec![VecDeque::new(); grid.len()],
            region: None,
        }
    }

    pub const fn config(&self) -> MotionConfig {
        self.config
    }

    /// The current motion region, tile-aligned.
    pub fn region(&self) -> Option<Rect> {
        self.region.map(|(rect, _)| rect)
    }

    pub fn contains(&self, tile: TileIndex) -> bool {
        match (self.region, self.grid.tile_rect(tile)) {
            (Some((region, _)), Some(rect)) => region.contains_rect(rect),
            _ => false,
        }
    }

    /// Records the tiles that changed at `now_ms` and reports a promotion or demotion.
    pub fn observe(&mut self, changed: &TileSet, now_ms: u64) -> Option<MotionEvent> {
        for tile in changed.iter() {
            let times = &mut self.changes[tile.0 as usize];
            times.push_back(now_ms);
            if times.len() > 128 {
                times.pop_front();
            }
        }
        let horizon = now_ms.saturating_sub(WINDOW_MS);
        for times in &mut self.changes {
            while times.front().is_some_and(|at| *at < horizon) {
                times.pop_front();
            }
        }

        match self.region {
            None => {
                let hot = self.sustained_hot_tiles(now_ms);
                let area = largest_component(self.grid, &hot);
                let rect = area
                    .iter()
                    .filter_map(|tile| self.grid.tile_rect(*tile))
                    .fold(Rect::default(), Rect::union);
                let min_side = self.config.min_side_tiles.saturating_sub(1) * crate::TILE_SIZE + 1;
                if area.len() >= self.config.min_tiles
                    && rect.width >= min_side
                    && rect.height >= min_side
                {
                    self.region = Some((rect, now_ms));
                    return Some(MotionEvent::Promoted(rect));
                }
                None
            }
            Some((rect, since)) => {
                // A region is promoted the moment enough of it is hot, which is usually before
                // all of it is: a video that has just started has only warmed the rows it has
                // drawn. Left alone the region keeps whatever extent it had at that instant, and
                // the rest of the window is sent as tiles at full rate for as long as it plays —
                // measured at about 180 kbps of pure waste on a half-covered 896 x 512 window.
                // So it is allowed to grow, and only to grow: shrinking while playing would move
                // the boundary back and forth and re-key the encoder each time.
                // Only tiles touching the region may join it. Taking the largest hot component
                // anywhere would let a clock ticking in the far corner be absorbed, and a region
                // stretched across the screen to reach it never goes quiet enough to demote —
                // which is exactly what happened the first time this was written.
                let hot = self.sustained_hot_tiles(now_ms);
                let reach = Rect::new(
                    rect.x.saturating_sub(crate::TILE_SIZE),
                    rect.y.saturating_sub(crate::TILE_SIZE),
                    rect.width + crate::TILE_SIZE * 2,
                    rect.height + crate::TILE_SIZE * 2,
                );
                let grown = hot
                    .iter()
                    .filter_map(|tile| self.grid.tile_rect(tile))
                    .filter(|tile| !reach.intersect(*tile).is_empty())
                    .fold(rect, Rect::union);
                if grown != rect && grown.contains_rect(rect) {
                    self.region = Some((grown, since));
                    return Some(MotionEvent::Promoted(grown));
                }
                if now_ms.saturating_sub(since) < self.config.demote_ms {
                    return None;
                }
                let needed = rate_count(self.config.demote_hz, self.config.demote_ms);
                let from = now_ms.saturating_sub(self.config.demote_ms);
                let still_moving = self
                    .grid
                    .tiles_covering(rect)
                    .iter()
                    .any(|tile| count_since(&self.changes[tile.0 as usize], from) >= needed);
                if still_moving {
                    return None;
                }
                self.region = None;
                Some(MotionEvent::Demoted(rect))
            }
        }
    }

    /// Tiles changing at `promote_hz` or faster that have been changing for `sustain_ms`.
    fn sustained_hot_tiles(&self, now_ms: u64) -> TileSet {
        let needed = rate_count(self.config.promote_hz, self.config.sustain_ms);
        let since = now_ms.saturating_sub(self.config.sustain_ms);
        let mut set = TileSet::new(self.grid);
        for (index, times) in self.changes.iter().enumerate() {
            let sustained = times
                .front()
                .is_some_and(|first| now_ms >= first + self.config.sustain_ms);
            if sustained && count_since(times, since) >= needed {
                set.insert(TileIndex(index as u32));
            }
        }
        set
    }
}

/// Changes a tile must make within `ms` to be changing at `hz` or faster, at least one.
fn rate_count(hz: u32, ms: u64) -> usize {
    ((u64::from(hz) * ms).div_ceil(1_000)).max(1) as usize
}

fn count_since(times: &VecDeque<u64>, since_ms: u64) -> usize {
    times.iter().filter(|at| **at >= since_ms).count()
}

fn largest_component(grid: TileGrid, tiles: &TileSet) -> Vec<TileIndex> {
    let mut seen = TileSet::new(grid);
    let mut best = Vec::new();
    for start in tiles.iter() {
        if !seen.insert(start) {
            continue;
        }
        let mut component = vec![start];
        let mut stack = vec![start];
        while let Some(tile) = stack.pop() {
            let (column, row) = grid.position(tile).expect("tile from this grid");
            let neighbours = [
                column.checked_sub(1).and_then(|c| grid.index(c, row)),
                grid.index(column + 1, row),
                row.checked_sub(1).and_then(|r| grid.index(column, r)),
                grid.index(column, row + 1),
            ];
            for neighbour in neighbours.into_iter().flatten() {
                if tiles.contains(neighbour) && seen.insert(neighbour) {
                    component.push(neighbour);
                    stack.push(neighbour);
                }
            }
        }
        if component.len() > best.len() {
            best = component;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Size;

    fn grid() -> TileGrid {
        TileGrid::new(Size::new(1512, 982).unwrap())
    }

    fn feed(
        tracker: &mut MotionTracker,
        area: Rect,
        from_ms: u64,
        to_ms: u64,
        every_ms: u64,
    ) -> Vec<(u64, MotionEvent)> {
        let set = grid().tiles_covering(area);
        let empty = TileSet::new(grid());
        let mut events = Vec::new();
        let mut now = from_ms;
        while now < to_ms {
            let changed = if every_ms == 0 { &empty } else { &set };
            if let Some(event) = tracker.observe(changed, now) {
                events.push((now, event));
            }
            now += if every_ms == 0 { 33 } else { every_ms };
        }
        events
    }

    #[test]
    fn a_playing_video_is_promoted_then_demoted_when_it_stops() {
        let video = Rect::new(885, 605, 351, 281);
        let mut tracker = MotionTracker::new(grid(), MotionConfig::default());
        let events = feed(&mut tracker, video, 0, 1_000, 33);
        let (at, event) = events[0];
        assert!((264..=400).contains(&at), "promoted at {at} ms");
        let MotionEvent::Promoted(rect) = event else {
            panic!("expected promotion")
        };
        assert!(rect.contains_rect(video));
        assert!(tracker.contains(grid().index(15, 11).unwrap()));

        let quiet = feed(&mut tracker, video, 1_000, 2_000, 0);
        assert_eq!(quiet.len(), 1);
        assert!(matches!(quiet[0].1, MotionEvent::Demoted(_)));
        assert!(quiet[0].0 <= 1_600, "demoted at {} ms", quiet[0].0);
        assert_eq!(tracker.region(), None);
    }

    #[test]
    fn small_or_slow_changes_are_never_promoted() {
        let mut tracker = MotionTracker::new(grid(), MotionConfig::default());
        assert!(
            feed(&mut tracker, Rect::new(1296, 0, 210, 26), 0, 3_000, 33).is_empty(),
            "clock-sized area"
        );
        let mut tracker = MotionTracker::new(grid(), MotionConfig::default());
        assert!(
            feed(&mut tracker, Rect::new(0, 0, 640, 480), 0, 3_000, 250).is_empty(),
            "4 Hz updates"
        );
        let mut tracker = MotionTracker::new(grid(), MotionConfig::default());
        assert!(
            feed(&mut tracker, Rect::new(0, 900, 1512, 60), 0, 3_000, 33).is_empty(),
            "a one-tile strip of revealed text"
        );
    }

    #[test]
    fn rate_counts_round_up() {
        assert_eq!(rate_count(12, 300), 4);
        assert_eq!(rate_count(4, 500), 2);
        assert_eq!(rate_count(1, 10), 1);
    }
}
