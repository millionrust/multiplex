use xxhash_rust::xxh3::Xxh3;

use crate::{CodecError, Frame, Rect, TileGrid, TileIndex, TileSet};

/// 64-bit xxh3 of a tile's pixels. The seed includes the tile's width and height, so edge tiles of
/// different shapes with equal bytes never collide.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TileHash(pub u64);

/// Hashes the pixels of `rect`. Panics when `rect` is outside the frame.
pub fn hash_rect(frame: &Frame<'_>, rect: Rect) -> TileHash {
    let mut hasher = Xxh3::with_seed((u64::from(rect.width) << 32) | u64::from(rect.height));
    for y in rect.y..rect.bottom() {
        hasher.update(frame.row_span(y, rect));
    }
    TileHash(hasher.digest())
}

/// The current hash of every tile of a surface.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TileHashes {
    grid: TileGrid,
    hashes: Vec<TileHash>,
}

impl TileHashes {
    pub fn compute(frame: &Frame<'_>) -> Self {
        let grid = TileGrid::new(frame.size());
        let hashes = (0..grid.len() as u32)
            .map(|index| hash_rect(frame, tile_rect(grid, TileIndex(index))))
            .collect();
        Self { grid, hashes }
    }

    pub const fn grid(&self) -> TileGrid {
        self.grid
    }

    pub fn get(&self, tile: TileIndex) -> Option<TileHash> {
        self.hashes.get(tile.0 as usize).copied()
    }

    /// Replaces the stored hash of one tile.
    pub fn set(&mut self, tile: TileIndex, hash: TileHash) {
        if let Some(slot) = self.hashes.get_mut(tile.0 as usize) {
            *slot = hash;
        }
    }

    /// Rehashes `candidates`, or every tile when `None`, and returns the tiles whose pixels
    /// changed. Operating-system damage is passed as candidates so untouched tiles are never read.
    pub fn update(
        &mut self,
        frame: &Frame<'_>,
        candidates: Option<&TileSet>,
    ) -> Result<TileSet, CodecError> {
        if frame.size() != self.grid.size() {
            return Err(CodecError::FrameSizeMismatch);
        }
        let mut changed = TileSet::new(self.grid);
        let mut visit = |tile: TileIndex| {
            let hash = hash_rect(frame, tile_rect(self.grid, tile));
            let slot = &mut self.hashes[tile.0 as usize];
            if *slot != hash {
                *slot = hash;
                changed.insert(tile);
            }
        };
        match candidates {
            Some(set) if set.grid() == self.grid => set.iter().for_each(&mut visit),
            Some(_) => return Err(CodecError::FrameSizeMismatch),
            None => (0..self.grid.len() as u32).for_each(|i| visit(TileIndex(i))),
        }
        Ok(changed)
    }
}

fn tile_rect(grid: TileGrid, tile: TileIndex) -> Rect {
    grid.tile_rect(tile)
        .expect("tile index comes from the same grid")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FrameBuffer, Size};
    use proptest::prelude::*;

    fn patterned(size: Size, seed: u8) -> FrameBuffer {
        let mut buffer = FrameBuffer::new(size);
        let width = size.width();
        let pixels: Vec<u8> = (0..size.height())
            .flat_map(|y| {
                (0..width)
                    .flat_map(move |x| [(x as u8) ^ seed, (y as u8).wrapping_mul(3), seed, 255])
            })
            .collect();
        buffer.write_rect(size.bounds(), &pixels).unwrap();
        buffer
    }

    #[test]
    fn unchanged_frames_report_no_tiles() {
        let size = Size::new(200, 130).unwrap();
        let frame = patterned(size, 7);
        let mut hashes = TileHashes::compute(&frame.as_frame());
        assert!(hashes.update(&frame.as_frame(), None).unwrap().is_empty());
    }

    #[test]
    fn one_pixel_change_marks_only_its_tile() {
        let size = Size::new(200, 130).unwrap();
        let mut frame = patterned(size, 7);
        let mut hashes = TileHashes::compute(&frame.as_frame());
        frame
            .fill_rect(Rect::new(130, 70, 1, 1), [1, 2, 3, 255])
            .unwrap();
        let changed = hashes.update(&frame.as_frame(), None).unwrap();
        assert_eq!(changed.iter().collect::<Vec<_>>(), vec![TileIndex(6)]);
        assert!(hashes.update(&frame.as_frame(), None).unwrap().is_empty());
    }

    #[test]
    fn damage_limits_which_tiles_are_read() {
        let size = Size::new(200, 130).unwrap();
        let mut frame = patterned(size, 7);
        let mut hashes = TileHashes::compute(&frame.as_frame());
        frame
            .fill_rect(Rect::new(0, 0, 200, 130), [0, 0, 0, 255])
            .unwrap();
        let damage = hashes.grid().tiles_covering(Rect::new(0, 0, 10, 10));
        let changed = hashes.update(&frame.as_frame(), Some(&damage)).unwrap();
        assert_eq!(
            changed.len(),
            1,
            "tiles outside damage are trusted to be unchanged"
        );
        assert_eq!(
            hashes.update(&frame.as_frame(), None).unwrap().len(),
            hashes.grid().len() - 1
        );
    }

    #[test]
    fn edge_tile_shape_is_part_of_the_hash() {
        let wide = FrameBuffer::new(Size::new(2, 1).unwrap());
        let tall = FrameBuffer::new(Size::new(1, 2).unwrap());
        assert_ne!(
            hash_rect(&wide.as_frame(), Rect::new(0, 0, 2, 1)),
            hash_rect(&tall.as_frame(), Rect::new(0, 0, 1, 2))
        );
    }

    #[test]
    fn stride_padding_does_not_affect_hashes() {
        let size = Size::new(70, 3).unwrap();
        let tight = patterned(size, 3);
        let mut padded = Vec::new();
        for y in 0..3 {
            padded.extend_from_slice(tight.as_frame().row(y));
            padded.extend_from_slice(&[0xAB; 8]);
        }
        let frame = Frame::new(size, 288, &padded).unwrap();
        assert_eq!(
            TileHashes::compute(&frame),
            TileHashes::compute(&tight.as_frame())
        );
    }

    proptest! {
        #[test]
        fn changed_tiles_are_exactly_the_tiles_that_differ(
            width in 1u32..300, height in 1u32..300,
            edits in proptest::collection::vec((0u32..300, 0u32..300, 0u8..255), 0..12),
        ) {
            let size = Size::new(width, height).unwrap();
            let before = patterned(size, 11);
            let mut after = before.clone();
            for (x, y, v) in edits {
                if x < width && y < height {
                    after.fill_rect(Rect::new(x, y, 1, 1), [v, v, v, 255]).unwrap();
                }
            }
            let mut hashes = TileHashes::compute(&before.as_frame());
            let changed = hashes.update(&after.as_frame(), None).unwrap();
            let grid = TileGrid::new(size);
            for index in 0..grid.len() as u32 {
                let rect = grid.tile_rect(TileIndex(index)).unwrap();
                let mut a = Vec::new();
                let mut b = Vec::new();
                before.as_frame().copy_rect_into(rect, &mut a);
                after.as_frame().copy_rect_into(rect, &mut b);
                prop_assert_eq!(changed.contains(TileIndex(index)), a != b);
            }
        }
    }
}
