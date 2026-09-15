//! Tiles a viewer already holds, so repeated content travels as an 8-byte hash.
//!
//! The viewer keeps decoded exact tiles; the host keeps a shadow with the same eviction order.
//! Both apply the same sequence: a lossless tile inserts, a cached reference touches. When they
//! diverge (a lost batch, a reconnect) the viewer reports a [`CacheMiss`] and the host forgets
//! the hash and resends the tile. Lossy tiles are never cached.

use std::collections::{BTreeMap, HashMap};

use crate::{TileHash, TileIndex};

/// Default viewer cache on phones.
pub const PHONE_CACHE_BYTES: usize = 64 << 20;
/// Default viewer cache on desktops.
pub const DESKTOP_CACHE_BYTES: usize = 256 << 20;

/// A cached reference the viewer could not resolve.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CacheMiss {
    pub tile: TileIndex,
    pub hash: TileHash,
}

/// Least-recently-used order and byte accounting shared by both sides.
#[derive(Clone, Debug)]
pub struct CacheIndex {
    budget: usize,
    used: usize,
    clock: u64,
    entries: HashMap<TileHash, (usize, u64)>,
    order: BTreeMap<u64, TileHash>,
}

impl CacheIndex {
    pub fn new(budget: usize) -> Self {
        Self {
            budget,
            used: 0,
            clock: 0,
            entries: HashMap::new(),
            order: BTreeMap::new(),
        }
    }

    pub fn contains(&self, hash: TileHash) -> bool {
        self.entries.contains_key(&hash)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub const fn used_bytes(&self) -> usize {
        self.used
    }

    /// Marks an entry as most recently used. Returns false when absent.
    pub fn touch(&mut self, hash: TileHash) -> bool {
        let Some((_, stamp)) = self.entries.get_mut(&hash) else {
            return false;
        };
        self.order.remove(stamp);
        self.clock += 1;
        *stamp = self.clock;
        self.order.insert(self.clock, hash);
        true
    }

    /// Inserts or refreshes an entry and returns the hashes evicted to make room. An entry larger
    /// than the whole budget is not stored.
    pub fn insert(&mut self, hash: TileHash, bytes: usize) -> Vec<TileHash> {
        if bytes > self.budget {
            return Vec::new();
        }
        self.remove(hash);
        let mut evicted = Vec::new();
        while self.used + bytes > self.budget {
            let (_, oldest) = self.order.pop_first().expect("used bytes imply an entry");
            let (size, _) = self
                .entries
                .remove(&oldest)
                .expect("order and entries agree");
            self.used -= size;
            evicted.push(oldest);
        }
        self.clock += 1;
        self.entries.insert(hash, (bytes, self.clock));
        self.order.insert(self.clock, hash);
        self.used += bytes;
        evicted
    }

    /// Removes an entry. Returns false when absent.
    pub fn remove(&mut self, hash: TileHash) -> bool {
        let Some((size, stamp)) = self.entries.remove(&hash) else {
            return false;
        };
        self.order.remove(&stamp);
        self.used -= size;
        true
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.used = 0;
    }
}

/// Decoded exact tile pixels.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CachedTile {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// The viewer's cache of exact tiles.
#[derive(Clone, Debug)]
pub struct ViewerCache {
    index: CacheIndex,
    tiles: HashMap<TileHash, CachedTile>,
}

impl ViewerCache {
    pub fn new(budget: usize) -> Self {
        Self {
            index: CacheIndex::new(budget),
            tiles: HashMap::new(),
        }
    }

    /// Looks up a tile and marks it as recently used.
    pub fn get(&mut self, hash: TileHash) -> Option<&CachedTile> {
        if !self.index.touch(hash) {
            return None;
        }
        self.tiles.get(&hash)
    }

    pub fn insert(&mut self, hash: TileHash, tile: CachedTile) {
        for evicted in self.index.insert(hash, tile.bgra.len()) {
            self.tiles.remove(&evicted);
        }
        if self.index.contains(hash) {
            self.tiles.insert(hash, tile);
        }
    }

    pub fn index(&self) -> &CacheIndex {
        &self.index
    }

    pub fn clear(&mut self) {
        self.index.clear();
        self.tiles.clear();
    }
}

/// The host's model of one viewer's cache.
#[derive(Clone, Debug)]
pub struct CacheShadow {
    index: CacheIndex,
}

impl CacheShadow {
    pub fn new(budget: usize) -> Self {
        Self {
            index: CacheIndex::new(budget),
        }
    }

    /// Whether a cached reference is expected to resolve; touches the entry when it is, exactly
    /// as the viewer will when it applies the reference.
    pub fn use_if_held(&mut self, hash: TileHash) -> bool {
        self.index.touch(hash)
    }

    /// Records that a lossless tile of `bytes` decoded bytes was sent.
    pub fn record_sent(&mut self, hash: TileHash, bytes: usize) {
        self.index.insert(hash, bytes);
    }

    /// Drops a hash the viewer reported missing.
    pub fn forget(&mut self, hash: TileHash) {
        self.index.remove(hash);
    }

    pub fn index(&self) -> &CacheIndex {
        &self.index
    }

    pub fn clear(&mut self) {
        self.index.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn tile(bytes: usize) -> CachedTile {
        CachedTile {
            width: 1,
            height: 1,
            bgra: vec![0; bytes],
        }
    }

    #[test]
    fn evicts_least_recently_used_first() {
        let mut index = CacheIndex::new(300);
        assert!(index.insert(TileHash(1), 100).is_empty());
        assert!(index.insert(TileHash(2), 100).is_empty());
        assert!(index.insert(TileHash(3), 100).is_empty());
        assert!(index.touch(TileHash(1)));
        assert_eq!(
            index.insert(TileHash(4), 150),
            vec![TileHash(2), TileHash(3)]
        );
        assert!(index.contains(TileHash(1)) && index.contains(TileHash(4)));
        assert_eq!(index.used_bytes(), 250);
    }

    #[test]
    fn oversized_entries_are_not_stored() {
        let mut cache = ViewerCache::new(10);
        cache.insert(TileHash(1), tile(11));
        assert!(cache.get(TileHash(1)).is_none());
        let mut shadow = CacheShadow::new(10);
        shadow.record_sent(TileHash(1), 11);
        assert!(!shadow.use_if_held(TileHash(1)));
    }

    #[test]
    fn reinserting_refreshes_size_and_order() {
        let mut index = CacheIndex::new(200);
        index.insert(TileHash(1), 100);
        index.insert(TileHash(2), 50);
        index.insert(TileHash(1), 20);
        assert_eq!(index.used_bytes(), 70);
        assert_eq!(index.insert(TileHash(3), 150), vec![TileHash(2)]);
    }

    #[test]
    fn a_forgotten_hash_is_resent() {
        let mut shadow = CacheShadow::new(1000);
        shadow.record_sent(TileHash(5), 100);
        assert!(shadow.use_if_held(TileHash(5)));
        shadow.forget(TileHash(5));
        assert!(!shadow.use_if_held(TileHash(5)));
    }

    proptest! {
        #[test]
        fn shadow_and_viewer_agree_when_they_see_the_same_sequence(
            steps in proptest::collection::vec((any::<bool>(), 0u64..40, 1usize..400), 0..300),
        ) {
            let mut viewer = ViewerCache::new(2_000);
            let mut shadow = CacheShadow::new(2_000);
            for (send_pixels, hash, bytes) in steps {
                let hash = TileHash(hash);
                if send_pixels || !shadow.use_if_held(hash) {
                    shadow.record_sent(hash, bytes);
                    viewer.insert(hash, tile(bytes));
                } else {
                    prop_assert!(viewer.get(hash).is_some(), "shadow promised a tile the viewer lacks");
                }
                prop_assert_eq!(shadow.index().used_bytes(), viewer.index().used_bytes());
                prop_assert_eq!(shadow.index().len(), viewer.index().len());
            }
        }
    }
}
