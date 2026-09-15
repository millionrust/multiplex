use crate::CodecError;

/// Edge length of a tile in pixels. Tiles on the right and bottom edges may be smaller.
pub const TILE_SIZE: u32 = 64;

/// Largest accepted surface width or height, which bounds every tile index and allocation.
pub const MAX_SURFACE_DIMENSION: u32 = 16_384;

/// Size of a surface in pixels. Both sides are non-zero and at most [`MAX_SURFACE_DIMENSION`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Size {
    width: u32,
    height: u32,
}

impl Size {
    pub const fn new(width: u32, height: u32) -> Result<Self, CodecError> {
        if width == 0
            || height == 0
            || width > MAX_SURFACE_DIMENSION
            || height > MAX_SURFACE_DIMENSION
        {
            return Err(CodecError::InvalidSize);
        }
        Ok(Self { width, height })
    }

    pub const fn width(self) -> u32 {
        self.width
    }

    pub const fn height(self) -> u32 {
        self.height
    }

    /// The whole surface as a rectangle at the origin.
    pub const fn bounds(self) -> Rect {
        Rect::new(0, 0, self.width, self.height)
    }
}

/// A rectangle in surface pixels. An empty rectangle has zero width or height.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub const fn right(self) -> u32 {
        self.x.saturating_add(self.width)
    }

    pub const fn bottom(self) -> u32 {
        self.y.saturating_add(self.height)
    }

    pub const fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// The overlapping area, or an empty rectangle when there is none.
    pub fn intersect(self, other: Rect) -> Rect {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        if right <= x || bottom <= y {
            return Rect::default();
        }
        Rect::new(x, y, right - x, bottom - y)
    }

    /// The smallest rectangle containing both. An empty side is ignored.
    pub fn union(self, other: Rect) -> Rect {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Rect::new(
            x,
            y,
            self.right().max(other.right()) - x,
            self.bottom().max(other.bottom()) - y,
        )
    }

    pub fn contains_rect(self, other: Rect) -> bool {
        other.is_empty()
            || (other.x >= self.x
                && other.y >= self.y
                && other.right() <= self.right()
                && other.bottom() <= self.bottom())
    }

    pub fn clamp_to(self, size: Size) -> Rect {
        self.intersect(size.bounds())
    }
}

/// Index of a tile in row-major order.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TileIndex(pub u32);

/// The fixed 64×64 tile layout of one surface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TileGrid {
    size: Size,
    columns: u32,
    rows: u32,
}

impl TileGrid {
    pub const fn new(size: Size) -> Self {
        Self {
            size,
            columns: size.width.div_ceil(TILE_SIZE),
            rows: size.height.div_ceil(TILE_SIZE),
        }
    }

    pub const fn size(self) -> Size {
        self.size
    }

    pub const fn columns(self) -> u32 {
        self.columns
    }

    pub const fn rows(self) -> u32 {
        self.rows
    }

    /// Number of tiles. At most 256 × 256 because of [`MAX_SURFACE_DIMENSION`].
    pub const fn len(self) -> usize {
        (self.columns * self.rows) as usize
    }

    pub const fn is_empty(self) -> bool {
        self.len() == 0
    }

    pub const fn contains(self, tile: TileIndex) -> bool {
        (tile.0 as usize) < self.len()
    }

    pub const fn index(self, column: u32, row: u32) -> Option<TileIndex> {
        if column >= self.columns || row >= self.rows {
            return None;
        }
        Some(TileIndex(row * self.columns + column))
    }

    pub const fn position(self, tile: TileIndex) -> Option<(u32, u32)> {
        if !self.contains(tile) {
            return None;
        }
        Some((tile.0 % self.columns, tile.0 / self.columns))
    }

    /// The pixels a tile covers, clipped to the surface on the right and bottom edges.
    pub fn tile_rect(self, tile: TileIndex) -> Option<Rect> {
        let (column, row) = self.position(tile)?;
        let x = column * TILE_SIZE;
        let y = row * TILE_SIZE;
        Some(Rect::new(
            x,
            y,
            TILE_SIZE.min(self.size.width - x),
            TILE_SIZE.min(self.size.height - y),
        ))
    }

    /// Every tile that overlaps any pixel of `rect`. Parts outside the surface are ignored.
    pub fn tiles_covering(self, rect: Rect) -> TileSet {
        let mut set = TileSet::new(self);
        set.insert_rect(rect);
        set
    }

    /// Every tile touched by any damage rectangle.
    pub fn tiles_for_damage(self, damage: &[Rect]) -> TileSet {
        let mut set = TileSet::new(self);
        for rect in damage {
            set.insert_rect(*rect);
        }
        set
    }
}

/// A set of tiles of one grid, stored as a bitset.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TileSet {
    grid: TileGrid,
    words: Vec<u64>,
    count: usize,
}

impl TileSet {
    pub fn new(grid: TileGrid) -> Self {
        Self {
            grid,
            words: vec![0; grid.len().div_ceil(64)],
            count: 0,
        }
    }

    /// A set containing every tile of the grid.
    pub fn full(grid: TileGrid) -> Self {
        let mut set = Self::new(grid);
        set.insert_rect(grid.size().bounds());
        set
    }

    pub const fn grid(&self) -> TileGrid {
        self.grid
    }

    pub const fn len(&self) -> usize {
        self.count
    }

    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn contains(&self, tile: TileIndex) -> bool {
        self.grid.contains(tile) && self.words[(tile.0 / 64) as usize] & (1 << (tile.0 % 64)) != 0
    }

    /// Adds a tile. Returns false when it was already present or is outside the grid.
    pub fn insert(&mut self, tile: TileIndex) -> bool {
        if !self.grid.contains(tile) {
            return false;
        }
        let word = &mut self.words[(tile.0 / 64) as usize];
        let bit = 1 << (tile.0 % 64);
        if *word & bit != 0 {
            return false;
        }
        *word |= bit;
        self.count += 1;
        true
    }

    /// Removes a tile. Returns false when it was not present.
    pub fn remove(&mut self, tile: TileIndex) -> bool {
        if !self.contains(tile) {
            return false;
        }
        self.words[(tile.0 / 64) as usize] &= !(1 << (tile.0 % 64));
        self.count -= 1;
        true
    }

    pub fn clear(&mut self) {
        self.words.iter_mut().for_each(|word| *word = 0);
        self.count = 0;
    }

    /// Adds every tile overlapping `rect`.
    pub fn insert_rect(&mut self, rect: Rect) {
        let rect = rect.clamp_to(self.grid.size());
        if rect.is_empty() {
            return;
        }
        let first_column = rect.x / TILE_SIZE;
        let last_column = (rect.right() - 1) / TILE_SIZE;
        let first_row = rect.y / TILE_SIZE;
        let last_row = (rect.bottom() - 1) / TILE_SIZE;
        for row in first_row..=last_row {
            for column in first_column..=last_column {
                if let Some(tile) = self.grid.index(column, row) {
                    self.insert(tile);
                }
            }
        }
    }

    /// Adds every tile of `other`. Sets from different grids are left unchanged.
    pub fn union_with(&mut self, other: &TileSet) {
        if other.grid != self.grid {
            return;
        }
        let mut count = 0;
        for (word, other_word) in self.words.iter_mut().zip(&other.words) {
            *word |= *other_word;
            count += word.count_ones() as usize;
        }
        self.count = count;
    }

    /// Tiles in ascending row-major order.
    pub fn iter(&self) -> impl Iterator<Item = TileIndex> + '_ {
        self.words
            .iter()
            .enumerate()
            .flat_map(|(word_index, word)| {
                let mut bits = *word;
                std::iter::from_fn(move || {
                    if bits == 0 {
                        return None;
                    }
                    let bit = bits.trailing_zeros();
                    bits &= bits - 1;
                    Some(TileIndex(word_index as u32 * 64 + bit))
                })
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn grid(width: u32, height: u32) -> TileGrid {
        TileGrid::new(Size::new(width, height).unwrap())
    }

    #[test]
    fn sizes_reject_zero_and_oversized_sides() {
        assert_eq!(Size::new(0, 10), Err(CodecError::InvalidSize));
        assert_eq!(Size::new(10, 0), Err(CodecError::InvalidSize));
        assert_eq!(
            Size::new(MAX_SURFACE_DIMENSION + 1, 10),
            Err(CodecError::InvalidSize)
        );
        assert!(Size::new(MAX_SURFACE_DIMENSION, MAX_SURFACE_DIMENSION).is_ok());
    }

    #[test]
    fn grid_counts_partial_edge_tiles() {
        let grid = grid(1512, 982);
        assert_eq!((grid.columns(), grid.rows()), (24, 16));
        assert_eq!(grid.len(), 384);
        let last = grid.index(23, 15).unwrap();
        assert_eq!(grid.tile_rect(last), Some(Rect::new(1472, 960, 40, 22)));
        assert_eq!(grid.tile_rect(TileIndex(0)), Some(Rect::new(0, 0, 64, 64)));
        assert_eq!(grid.tile_rect(TileIndex(384)), None);
        assert_eq!(grid.index(24, 0), None);
    }

    #[test]
    fn damage_maps_to_every_touched_tile() {
        let grid = grid(1512, 982);
        let clock = grid.tiles_covering(Rect::new(1296, 0, 210, 26));
        assert_eq!(
            clock.iter().collect::<Vec<_>>(),
            vec![TileIndex(20), TileIndex(21), TileIndex(22), TileIndex(23)]
        );
        let one_pixel = grid.tiles_covering(Rect::new(64, 64, 1, 1));
        assert_eq!(one_pixel.iter().collect::<Vec<_>>(), vec![TileIndex(25)]);
        assert!(
            grid.tiles_covering(Rect::new(2000, 2000, 10, 10))
                .is_empty()
        );
        assert!(grid.tiles_covering(Rect::new(10, 10, 0, 5)).is_empty());
    }

    #[test]
    fn rect_intersection_and_union() {
        let a = Rect::new(0, 0, 100, 100);
        let b = Rect::new(50, 60, 100, 100);
        assert_eq!(a.intersect(b), Rect::new(50, 60, 50, 40));
        assert_eq!(a.union(b), Rect::new(0, 0, 150, 160));
        assert!(a.intersect(Rect::new(100, 0, 10, 10)).is_empty());
        assert_eq!(Rect::default().union(b), b);
        assert!(a.contains_rect(Rect::new(10, 10, 90, 90)));
        assert!(!a.contains_rect(Rect::new(10, 10, 91, 90)));
    }

    #[test]
    fn tile_set_insert_remove_and_union() {
        let grid = grid(300, 200);
        let mut set = TileSet::new(grid);
        assert!(set.insert(TileIndex(3)));
        assert!(!set.insert(TileIndex(3)));
        assert!(!set.insert(TileIndex(grid.len() as u32)));
        let mut other = TileSet::new(grid);
        other.insert(TileIndex(0));
        other.insert(TileIndex(3));
        set.union_with(&other);
        assert_eq!(set.len(), 2);
        assert!(set.remove(TileIndex(0)));
        assert!(!set.remove(TileIndex(0)));
        assert_eq!(set.iter().collect::<Vec<_>>(), vec![TileIndex(3)]);
        assert_eq!(TileSet::full(grid).len(), grid.len());
        set.clear();
        assert!(set.is_empty());
    }

    proptest! {
        #[test]
        fn covering_set_matches_pixel_overlap(
            width in 1u32..700, height in 1u32..700,
            x in 0u32..800, y in 0u32..800, w in 0u32..300, h in 0u32..300,
        ) {
            let grid = grid(width, height);
            let rect = Rect::new(x, y, w, h);
            let set = grid.tiles_covering(rect);
            for index in 0..grid.len() as u32 {
                let tile = TileIndex(index);
                let overlaps = !grid.tile_rect(tile).unwrap().intersect(rect).is_empty();
                prop_assert_eq!(set.contains(tile), overlaps);
            }
            prop_assert_eq!(set.len(), set.iter().count());
        }

        #[test]
        fn tile_rects_partition_the_surface(width in 1u32..900, height in 1u32..900) {
            let grid = grid(width, height);
            let area: u64 = (0..grid.len() as u32)
                .map(|i| grid.tile_rect(TileIndex(i)).unwrap())
                .map(|r| u64::from(r.width) * u64::from(r.height))
                .sum();
            prop_assert_eq!(area, u64::from(width) * u64::from(height));
        }
    }
}
