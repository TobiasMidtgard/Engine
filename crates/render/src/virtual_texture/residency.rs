//! Which tiles of which virtual texture sit in which slot of a physical cache, and the page
//! tables that tell the shader where to find them. Pure CPU bookkeeping (no GPU), unit tested.
//!
//! Every virtual texture has a page table with one entry per tile of every level, finest
//! first. An entry names the slot and level of the finest *resident* tile covering it: the
//! tile itself when it is resident, otherwise its nearest resident ancestor. The root tile (the
//! level where the whole image fits one tile) is always resident, so every entry is valid and
//! the shader never waits: it falls back to a coarser level until the finer tile arrives.
//!
//! Tiles are only made resident once their parent is, and only leaves (tiles without resident
//! children) are evicted, so the resident tiles of a texture always form a tree hanging from
//! its root and fallback entries always point at an ancestor.

/// Texels per tile side (the content, without the border).
pub const TILE: u32 = 128;
/// Texels of the neighbors' content copied around each tile, so bilinear, trilinear and
/// anisotropic filtering read the right texels across tile edges.
pub const BORDER: u32 = 8;
/// Side of a tile's slot in a physical cache texture: the tile and its border.
pub const SLOT: u32 = TILE + 2 * BORDER;
/// Most levels a virtual texture can have (the feedback packs the level into 4 bits).
pub const MAX_LEVELS: usize = 16;
/// Most tiles per side at level 0 (the feedback packs tile coordinates into 10 bits).
pub const MAX_PAGES: u32 = 1024;

/// Bit set in a page table entry that is valid.
pub const ENTRY_VALID: u32 = 1 << 31;

/// Pack a page table entry: slot index and the level that slot holds.
pub fn entry(slot: u32, level: u32) -> u32 {
    ENTRY_VALID | (level << 16) | (slot & 0xffff)
}

/// Slot of an entry.
pub fn entry_slot(e: u32) -> u32 {
    e & 0xffff
}

/// Level of an entry.
pub fn entry_level(e: u32) -> u32 {
    (e >> 16) & 31
}

/// The levels and tiles of a virtual texture of a given size.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub width: u32,
    pub height: u32,
    /// The level whose whole image fits one tile (it and every coarser level live in that one
    /// tile's slot, with the slot's own mips).
    pub root: u32,
    /// Index of each level's first entry in the page table.
    pub level_offsets: [u32; MAX_LEVELS],
    /// Page table entries in all.
    pub entries: u32,
}

impl Layout {
    /// The layout of a `width`×`height` texture, or why it can't be virtual: every level down
    /// to the root must halve exactly (width and height divisible by 2^root), and level 0 may
    /// have at most [`MAX_PAGES`] tiles per side.
    pub fn new(width: u32, height: u32) -> Result<Self, String> {
        if width == 0 || height == 0 {
            return Err("a virtual texture needs a size".into());
        }
        if width.div_ceil(TILE) > MAX_PAGES || height.div_ceil(TILE) > MAX_PAGES {
            return Err(format!(
                "{width}x{height} is larger than a virtual texture can be ({} texels per side)",
                MAX_PAGES * TILE
            ));
        }
        let mut root = 0;
        while (width >> root) > TILE || (height >> root) > TILE {
            root += 1;
        }
        if root as usize >= MAX_LEVELS {
            return Err(format!("{width}x{height} has too many levels"));
        }
        let mask = (1u32 << root) - 1;
        if width & mask != 0 || height & mask != 0 {
            return Err(format!(
                "{width}x{height} does not halve evenly down to {}x{} (width and height must be \
                 divisible by {})",
                width >> root,
                height >> root,
                1u32 << root
            ));
        }
        let mut level_offsets = [0u32; MAX_LEVELS];
        let mut entries = 0;
        for (level, offset) in level_offsets.iter_mut().enumerate().take(root as usize + 1) {
            *offset = entries;
            let (px, py) = pages_of(width, height, level as u32);
            entries += px * py;
        }
        for offset in level_offsets.iter_mut().skip(root as usize + 1) {
            *offset = entries;
        }
        Ok(Self {
            width,
            height,
            root,
            level_offsets,
            entries,
        })
    }

    /// Texels of level `level` (at least 1×1).
    pub fn level_size(&self, level: u32) -> (u32, u32) {
        ((self.width >> level).max(1), (self.height >> level).max(1))
    }

    /// Tiles per side of level `level`.
    pub fn pages(&self, level: u32) -> (u32, u32) {
        pages_of(self.width, self.height, level)
    }

    /// Page table index of tile (`x`, `y`) of `level`.
    pub fn index(&self, level: u32, x: u32, y: u32) -> usize {
        let (px, _) = self.pages(level);
        (self.level_offsets[level as usize] + y * px + x) as usize
    }

    /// Whether the tile exists.
    pub fn contains(&self, level: u32, x: u32, y: u32) -> bool {
        let (px, py) = self.pages(level);
        level <= self.root && x < px && y < py
    }

    /// Tiles at every level, finest first.
    pub fn tile_count(&self) -> u32 {
        self.entries
    }
}

fn pages_of(width: u32, height: u32, level: u32) -> (u32, u32) {
    (
        (width >> level).max(1).div_ceil(TILE),
        (height >> level).max(1).div_ceil(TILE),
    )
}

/// A tile of a virtual texture: level, x, y.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Tile {
    pub level: u32,
    pub x: u32,
    pub y: u32,
}

impl Tile {
    pub fn new(level: u32, x: u32, y: u32) -> Self {
        Self { level, x, y }
    }

    /// The tile one level coarser that covers this one.
    pub fn parent(self) -> Self {
        Self::new(self.level + 1, self.x >> 1, self.y >> 1)
    }
}

/// One virtual texture's residency: its page table and which of its tiles are resident.
#[derive(Debug, Clone)]
pub struct TextureResidency {
    pub layout: Layout,
    /// The page table as the shader sees it.
    pub table: Vec<u32>,
    /// Slot + 1 of each tile that is itself resident (0 = not resident).
    own: Vec<u32>,
    /// Resident children of each tile (a tile with none can be evicted).
    children: Vec<u8>,
    /// Range of `table` changed since [`TextureResidency::take_dirty`].
    dirty: Option<(usize, usize)>,
}

impl TextureResidency {
    pub fn new(layout: Layout) -> Self {
        let n = layout.entries as usize;
        Self {
            layout,
            table: vec![0; n],
            own: vec![0; n],
            children: vec![0; n],
            dirty: Some((0, n)),
        }
    }

    /// Whether the root tile is resident (the texture can be drawn).
    pub fn ready(&self) -> bool {
        self.resident(self.root())
    }

    /// The root tile.
    pub fn root(&self) -> Tile {
        Tile::new(self.layout.root, 0, 0)
    }

    /// Whether `tile` is resident itself.
    pub fn resident(&self, tile: Tile) -> bool {
        self.layout.contains(tile.level, tile.x, tile.y)
            && self.own[self.layout.index(tile.level, tile.x, tile.y)] != 0
    }

    /// Slot of a resident tile.
    pub fn slot(&self, tile: Tile) -> Option<u32> {
        if !self.layout.contains(tile.level, tile.x, tile.y) {
            return None;
        }
        self.own[self.layout.index(tile.level, tile.x, tile.y)].checked_sub(1)
    }

    /// Whether `tile` can be made resident now: it exists, isn't resident, and its parent is
    /// (or it is the root).
    pub fn can_commit(&self, tile: Tile) -> bool {
        self.layout.contains(tile.level, tile.x, tile.y)
            && !self.resident(tile)
            && (tile.level == self.layout.root || self.resident(tile.parent()))
    }

    /// Whether a resident tile has no resident children (eviction takes only those).
    pub fn is_leaf(&self, tile: Tile) -> bool {
        self.layout.contains(tile.level, tile.x, tile.y)
            && self.children[self.layout.index(tile.level, tile.x, tile.y)] == 0
    }

    /// Make `tile` resident in `slot` (see [`TextureResidency::can_commit`]): its entry and every
    /// entry below it now point at it.
    pub fn commit(&mut self, tile: Tile, slot: u32) {
        debug_assert!(self.can_commit(tile));
        let idx = self.layout.index(tile.level, tile.x, tile.y);
        self.own[idx] = slot + 1;
        if tile.level < self.layout.root {
            let p = tile.parent();
            let pi = self.layout.index(p.level, p.x, p.y);
            self.children[pi] = self.children[pi].saturating_add(1);
        }
        self.fill_subtree(tile, entry(slot, tile.level));
    }

    /// Evict a resident leaf: its entries fall back to its parent's. Returns its slot.
    pub fn evict(&mut self, tile: Tile) -> Option<u32> {
        if !self.resident(tile) || !self.is_leaf(tile) || tile.level == self.layout.root {
            return None;
        }
        let idx = self.layout.index(tile.level, tile.x, tile.y);
        let slot = self.own[idx] - 1;
        self.own[idx] = 0;
        let p = tile.parent();
        let pi = self.layout.index(p.level, p.x, p.y);
        self.children[pi] = self.children[pi].saturating_sub(1);
        let fallback = self.table[pi];
        self.fill_subtree(tile, fallback);
        Some(slot)
    }

    /// Set the entries of `tile` and every finer tile under it to `value`.
    fn fill_subtree(&mut self, tile: Tile, value: u32) {
        for level in (0..=tile.level).rev() {
            let k = tile.level - level;
            let (px, py) = self.layout.pages(level);
            let x0 = (tile.x << k).min(px);
            let x1 = ((tile.x + 1) << k).min(px);
            let y0 = (tile.y << k).min(py);
            let y1 = ((tile.y + 1) << k).min(py);
            for y in y0..y1 {
                let row = self.layout.index(level, 0, y);
                for e in &mut self.table[row + x0 as usize..row + x1 as usize] {
                    *e = value;
                }
                self.mark(row + x0 as usize, row + x1 as usize);
            }
        }
    }

    fn mark(&mut self, start: usize, end: usize) {
        if start >= end {
            return;
        }
        self.dirty = Some(match self.dirty {
            Some((a, b)) => (a.min(start), b.max(end)),
            None => (start, end),
        });
    }

    /// The range of the page table changed since the last call.
    pub fn take_dirty(&mut self) -> Option<(usize, usize)> {
        self.dirty.take()
    }

    /// Mark the whole page table for upload (it moved in GPU memory).
    pub fn mark_all(&mut self) {
        self.dirty = Some((0, self.table.len()));
    }

    /// Every resident tile with its slot.
    pub fn resident_tiles(&self) -> impl Iterator<Item = (Tile, u32)> + '_ {
        let layout = &self.layout;
        (0..=layout.root).flat_map(move |level| {
            let (px, py) = layout.pages(level);
            (0..py).flat_map(move |y| {
                (0..px).filter_map(move |x| {
                    let s = self.own[layout.index(level, x, y)];
                    (s != 0).then(|| (Tile::new(level, x, y), s - 1))
                })
            })
        })
    }
}

/// Who holds a slot of a physical cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotOwner {
    /// Virtual texture id (1..).
    pub texture: u8,
    pub tile: Tile,
    /// Frame the tile was last asked for.
    pub last_used: u64,
}

/// The slots of one physical cache texture: which are free and who holds the others.
#[derive(Debug, Clone)]
pub struct SlotPool {
    owners: Vec<Option<SlotOwner>>,
    free: Vec<u32>,
}

impl SlotPool {
    pub fn new(capacity: u32) -> Self {
        Self {
            owners: vec![None; capacity as usize],
            free: (0..capacity).rev().collect(),
        }
    }

    pub fn capacity(&self) -> u32 {
        self.owners.len() as u32
    }

    /// Slots in use.
    pub fn used(&self) -> u32 {
        self.capacity() - self.free.len() as u32
    }

    /// Take a free slot.
    pub fn alloc(&mut self, owner: SlotOwner) -> Option<u32> {
        let slot = self.free.pop()?;
        self.owners[slot as usize] = Some(owner);
        Some(slot)
    }

    /// Give a slot back.
    pub fn release(&mut self, slot: u32) {
        if let Some(o) = self.owners.get_mut(slot as usize)
            && o.take().is_some()
        {
            self.free.push(slot);
        }
    }

    pub fn owner(&self, slot: u32) -> Option<SlotOwner> {
        self.owners.get(slot as usize).copied().flatten()
    }

    /// Record that the tile in `slot` was asked for in `frame`.
    pub fn touch(&mut self, slot: u32, frame: u64) {
        if let Some(Some(o)) = self.owners.get_mut(slot as usize) {
            o.last_used = o.last_used.max(frame);
        }
    }

    /// The least recently used slot whose tile `evictable` accepts and that wasn't asked for
    /// since `before` (exclusive).
    pub fn lru(&self, before: u64, mut evictable: impl FnMut(&SlotOwner) -> bool) -> Option<u32> {
        let mut best: Option<(u64, u32)> = None;
        for (slot, owner) in self.owners.iter().enumerate() {
            let Some(o) = owner else { continue };
            if o.last_used >= before || best.is_some_and(|(t, _)| o.last_used >= t) {
                continue;
            }
            if evictable(o) {
                best = Some((o.last_used, slot as u32));
            }
        }
        best.map(|(_, s)| s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_cover_every_level_down_to_one_tile() {
        let l = Layout::new(16384, 16384).unwrap();
        assert_eq!(l.root, 7);
        assert_eq!(l.pages(0), (128, 128));
        assert_eq!(l.pages(7), (1, 1));
        let expected: u32 = (0..=7).map(|m| (128u32 >> m).pow(2)).sum();
        assert_eq!(l.entries, expected);
        assert_eq!(l.index(7, 0, 0), expected as usize - 1);

        let wide = Layout::new(2048, 512).unwrap();
        assert_eq!(wide.root, 4);
        assert_eq!(wide.level_size(4), (128, 32));
        assert_eq!(wide.pages(1), (8, 2));
        assert_eq!(Layout::new(128, 128).unwrap().root, 0);
        assert_eq!(Layout::new(6144, 6144).unwrap().root, 6);
        assert!(Layout::new(5000, 5000).is_err());
        assert!(Layout::new(MAX_PAGES * TILE * 2, 128).is_err());
    }

    #[test]
    fn fallback_entries_point_at_the_finest_resident_ancestor() {
        let mut r = TextureResidency::new(Layout::new(512, 512).unwrap());
        let root = r.root();
        assert_eq!(root.level, 2);
        assert!(!r.ready());
        assert!(!r.can_commit(Tile::new(1, 0, 0)), "parent first");
        r.commit(root, 7);
        assert!(r.ready());
        assert!(r.table.iter().all(|&e| e == entry(7, 2)));
        r.commit(Tile::new(1, 1, 0), 3);
        r.commit(Tile::new(0, 3, 1), 4);
        let l = &r.layout;
        assert_eq!(r.table[l.index(0, 3, 1)], entry(4, 0));
        assert_eq!(r.table[l.index(0, 2, 0)], entry(3, 1));
        assert_eq!(r.table[l.index(0, 1, 1)], entry(7, 2));
        assert_eq!(r.table[l.index(1, 0, 1)], entry(7, 2));
        // The level 1 tile has a resident child: not a leaf, can't go.
        assert!(r.evict(Tile::new(1, 1, 0)).is_none());
        assert_eq!(r.evict(Tile::new(0, 3, 1)), Some(4));
        let l = &r.layout;
        assert_eq!(r.table[l.index(0, 3, 1)], entry(3, 1));
        assert_eq!(r.evict(Tile::new(1, 1, 0)), Some(3));
        assert!(r.table.iter().all(|&e| e == entry(7, 2)));
        assert!(r.evict(root).is_none(), "the root stays");
        assert_eq!(r.resident_tiles().count(), 1);
    }

    #[test]
    fn dirty_ranges_cover_changed_entries() {
        let mut r = TextureResidency::new(Layout::new(1024, 1024).unwrap());
        assert_eq!(r.take_dirty(), Some((0, r.table.len())));
        assert_eq!(r.take_dirty(), None);
        r.commit(r.root(), 0);
        r.take_dirty();
        r.commit(Tile::new(2, 1, 1), 1);
        let (a, b) = r.take_dirty().unwrap();
        let l = r.layout.clone();
        for (i, &e) in r.table.iter().enumerate() {
            if e == entry(1, 2) {
                assert!(i >= a && i < b);
            }
        }
        // Its subtree: level 1 pages 2..4, level 0 pages 4..8 (each axis).
        assert!(a <= l.index(0, 4, 4) && b > l.index(2, 1, 1));
    }

    #[test]
    fn the_pool_evicts_the_least_recently_used_allowed_slot() {
        let mut pool = SlotPool::new(3);
        let owner = |t: u32, last_used| SlotOwner {
            texture: 1,
            tile: Tile::new(0, t, 0),
            last_used,
        };
        let a = pool.alloc(owner(0, 5)).unwrap();
        let b = pool.alloc(owner(1, 2)).unwrap();
        let c = pool.alloc(owner(2, 9)).unwrap();
        assert!(pool.alloc(owner(3, 0)).is_none());
        assert_eq!(pool.lru(10, |_| true), Some(b));
        assert_eq!(pool.lru(10, |o| o.tile.x != 1), Some(a));
        assert_eq!(pool.lru(3, |o| o.tile.x != 1), None);
        pool.touch(b, 20);
        assert_eq!(pool.lru(10, |_| true), Some(a));
        pool.release(c);
        assert_eq!(pool.used(), 2);
        assert_eq!(pool.alloc(owner(4, 1)), Some(c));
    }
}
