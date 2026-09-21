//! Geographic surface levels, conservative maxima, and smooth-patch error bounds.

use super::cell_samples_log2;

pub const SURFACE_VERSION: u8 = 3;
pub const SURFACE_FLAG: u8 = 1;
pub const CELL_INDEX_FLAG: u8 = 2;
/// Flag bit 2: the container carries the §9 crest directory and its blocks.
pub const CREST_FLAG: u8 = 4;
/// A crest byte is a lift in this many metres (spec §9.2). Unsigned: the panorama surface is
/// never below the native one, which is what keeps §8.2's maxima sound for a reader that skips it.
pub const CREST_QUANTUM: i32 = 2;
pub const MAX_GROUP_BYTES: u32 = 256;
pub const MAX_LEAF_LOG2: u8 = 2;
pub const MAX_SURFACE_LEVELS: usize = super::MAX_CELL_TILES_LOG2 as usize + 1;

/// One native-height maximum per geographic cell, followed by 2×2 maximum levels.
#[derive(Clone, Copy, Debug)]
pub struct CellIndexLayout {
    pub offset: u32,
    pub bytes: u32,
    rows: u16,
    cols: u16,
}

impl CellIndexLayout {
    pub fn new(rows: u16, cols: u16, directory_offset: u32) -> Option<Self> {
        if rows == 0 || cols == 0 {
            return None;
        }
        let entries = u32::from(rows).checked_mul(u32::from(cols))?;
        let offset = align_block(directory_offset.checked_add(entries.checked_mul(4)?)?)?;
        let mut side_y = u32::from(rows);
        let mut side_x = u32::from(cols);
        let mut bytes = 0u32;
        loop {
            bytes = bytes.checked_add(side_y.checked_mul(side_x)?.checked_mul(2)?)?;
            if side_y == 1 && side_x == 1 {
                break;
            }
            side_y = side_y.div_ceil(2);
            side_x = side_x.div_ceil(2);
        }
        align_block(offset.checked_add(bytes)?)?;
        Some(Self { offset, bytes, rows, cols })
    }

    /// End of the padded index, and the first possible cell-block offset.
    pub fn end(self) -> u32 {
        align_block(self.offset + self.bytes).expect("validated cell index")
    }

    /// Absolute container offset. Node indices are relative to the container's cell rectangle.
    pub fn node_offset(self, y: u32, x: u32, log2: u8) -> Option<u32> {
        let (mut rows, mut cols) = (u32::from(self.rows), u32::from(self.cols));
        let mut offset = self.offset;
        for _ in 0..log2 {
            if rows == 1 && cols == 1 {
                return None;
            }
            offset += rows * cols * 2;
            rows = rows.div_ceil(2);
            cols = cols.div_ceil(2);
        }
        if y >= rows || x >= cols {
            return None;
        }
        Some(offset + (y * cols + x) * 2)
    }
}

/// One `uint32` per geographic cell, then the crest blocks those entries point at (spec §9.1).
/// Absent as a whole when [`CREST_FLAG`] is clear, and per cell when an entry is zero — national
/// LiDAR coverage stops at borders, so a container routinely carries blocks for some cells only.
#[derive(Clone, Copy, Debug)]
pub struct CrestDirectory {
    pub offset: u32,
    pub bytes: u32,
    slots: u32,
}

impl CrestDirectory {
    /// `after` is the first free byte: [`CellIndexLayout::end`], or the end of the cell directory
    /// when flag bit 1 is clear.
    pub fn new(rows: u16, cols: u16, after: u32) -> Option<Self> {
        if rows == 0 || cols == 0 {
            return None;
        }
        let slots = u32::from(rows).checked_mul(u32::from(cols))?;
        let offset = align_block(after)?;
        let bytes = slots.checked_mul(4)?;
        align_block(offset.checked_add(bytes)?)?;
        Some(Self { offset, bytes, slots })
    }

    /// End of the padded directory, and the first possible crest-block offset.
    pub fn end(self) -> u32 {
        align_block(self.offset + self.bytes).expect("validated crest directory")
    }

    /// Absolute container offset of one entry, in the cell directory's row-major order.
    pub fn entry_offset(self, slot: u32) -> Option<u32> {
        (slot < self.slots).then(|| self.offset + slot * 4)
    }
}

/// A crest block: one unsigned lift byte per sample, for every [`SurfaceLayout`] level (§9.2).
/// The renderer changes level with distance, so a lift that stopped at the native level would
/// step where a ridge crosses that boundary. The quarter-per-level series converges, so carrying
/// them all costs four thirds of the native plane rather than a multiple of it.
#[derive(Clone, Copy, Debug)]
pub struct CrestLayout {
    surface: SurfaceLayout,
}

impl CrestLayout {
    pub fn new(surface: SurfaceLayout) -> Self {
        Self { surface }
    }

    pub fn level_count(self) -> usize {
        self.surface.level_count()
    }

    /// Offset of a level's plane relative to the crest block, with its byte length.
    pub fn plane(self, index: usize) -> Option<(u32, u32)> {
        let mut offset = 0u32;
        for i in 0..=index {
            let level = self.surface.level(i)?;
            let bytes = 1u32.checked_shl(2 * u32::from(level.samples_log2))?;
            if i == index {
                return Some((offset, bytes));
            }
            offset = align_block(offset.checked_add(bytes)?)?;
        }
        None
    }

    fn checked_block_bytes(self) -> Option<u32> {
        let (offset, bytes) = self.plane(self.level_count() - 1)?;
        align_block(offset.checked_add(bytes)?)
    }

    pub fn block_bytes(self) -> u32 {
        self.checked_block_bytes().expect("a crest block is smaller than its surface cell")
    }

    /// Offset of one sample's lift byte relative to the crest block. The plane keeps §2's 16×16
    /// tile order, at one byte per sample rather than two.
    pub fn sample_offset(self, index: usize, y: u32, x: u32) -> Option<u32> {
        let level = self.surface.level(index)?;
        let side = 1u32.checked_shl(u32::from(level.samples_log2))?;
        if y >= side || x >= side {
            return None;
        }
        let (plane, _) = self.plane(index)?;
        let tiles_log2 = level.samples_log2 - super::TILE_LOG2 as u8;
        let tile = ((y >> super::TILE_LOG2) << tiles_log2) + (x >> super::TILE_LOG2);
        let within = (y & 15) * super::TILE_SAMPLES as u32 + (x & 15);
        Some(plane + tile * (super::TILE_SAMPLES * super::TILE_SAMPLES) as u32 + within)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SurfaceLayout {
    posting_log2: u8,
    cell_log2: u8,
}

#[derive(Clone, Copy, Debug)]
pub struct SurfaceLevel {
    /// Derived spacing; coarse levels can exceed the native posting limit.
    pub posting_log2: u8,
    pub samples_log2: u8,
    pub offset: u32,
    pub height_bytes: u32,
    pub bounds_offset: u32,
    pub bounds_bytes: u32,
}

impl SurfaceLayout {
    pub fn new(posting_log2: u8, cell_log2: u8) -> Option<Self> {
        cell_samples_log2(posting_log2, cell_log2)?;
        let layout = Self { posting_log2, cell_log2 };
        layout.checked_cell_bytes()?;
        Some(layout)
    }

    pub fn level_count(self) -> usize {
        usize::from(self.cell_log2 - self.posting_log2 - 4 + 1)
    }

    pub fn level(self, index: usize) -> Option<SurfaceLevel> {
        if index >= self.level_count() {
            return None;
        }
        let mut offset = 0u32;
        for i in 0..=index {
            let posting_log2 = self.posting_log2 + i as u8;
            let samples_log2 = self.cell_log2 - posting_log2;
            let height_bytes = 1u32.checked_shl(2 * u32::from(samples_log2) + 1)?;
            let bounds_bytes = bounds_bytes(samples_log2)?;
            let bounds_offset = offset.checked_add(height_bytes)?;
            if i == index {
                return Some(SurfaceLevel {
                    posting_log2,
                    samples_log2,
                    offset,
                    height_bytes,
                    bounds_offset,
                    bounds_bytes,
                });
            }
            offset = align_block(bounds_offset.checked_add(bounds_bytes)?)?;
        }
        None
    }

    fn checked_cell_bytes(self) -> Option<u32> {
        let last = self.level(self.level_count() - 1)?;
        align_block(last.bounds_offset.checked_add(last.bounds_bytes)?)
    }

    pub fn cell_bytes(self) -> u32 {
        self.checked_cell_bytes().expect("validated surface layout")
    }
}

impl SurfaceLevel {
    /// Offset relative to the cell block. Coordinates name a source interval, in this level.
    pub fn bound_offset(self, y: u32, x: u32, block_log2: u8) -> Option<u32> {
        if !(MAX_LEAF_LOG2..=self.samples_log2).contains(&block_log2)
            || y >= 1 << self.samples_log2
            || x >= 1 << self.samples_log2
        {
            return None;
        }
        let start = MAX_LEAF_LOG2 + (block_log2 - MAX_LEAF_LOG2) / 4 * 4;
        let mut offset = self.bounds_offset;
        for base in (MAX_LEAF_LOG2..start).step_by(4) {
            let groups = 1u32 << self.samples_log2.saturating_sub(base + 3);
            offset += groups * groups * MAX_GROUP_BYTES;
        }
        let groups = 1u32 << self.samples_log2.saturating_sub(start + 3);
        let group = (y >> (start + 3)) * groups + (x >> (start + 3));
        let depth = block_log2 - start;
        let width = 8u32 >> depth;
        let prefix = [0u32, 64, 80, 84][depth as usize];
        let node = prefix + ((y >> block_log2) & (width - 1)) * width + ((x >> block_log2) & (width - 1));
        Some(offset + group * MAX_GROUP_BYTES + node * 2)
    }
}

/// The 85 maxima occupy170 bytes; one error byte per node fits in the same256-byte group.
/// Low nibble bounds height error in metres; high nibble bounds each derivative error in
/// metres per source interval. Codes0 and15 mean exact and unknown, respectively.
pub fn approximation_offset(bound_offset: u32) -> u32 {
    (bound_offset & !255) + 170 + (bound_offset & 255) / 2
}

pub fn approximation_error(code: u8, gradient: bool) -> f32 {
    match code & 15 {
        0 => 0.0,
        15 => f32::INFINITY,
        n => (1u32 << (n - 1)) as f32 * if gradient { 0.0625 } else { 0.25 },
    }
}

fn bounds_bytes(samples_log2: u8) -> Option<u32> {
    let mut bytes = 0u32;
    for start in (MAX_LEAF_LOG2..=samples_log2).step_by(4) {
        let groups = 1u32.checked_shl(samples_log2.saturating_sub(start + 3).into())?;
        bytes = bytes.checked_add(groups.checked_mul(groups)?.checked_mul(MAX_GROUP_BYTES)?)?;
    }
    Some(bytes)
}

fn align_block(bytes: u32) -> Option<u32> {
    Some(bytes.checked_add(511)? & !511)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_crest_block_carries_every_level_and_costs_four_thirds_of_the_native_plane() {
        let surface = SurfaceLayout::new(9, 19).unwrap();
        let crest = CrestLayout::new(surface);
        assert_eq!(crest.level_count(), 7);
        let (_, native_plane) = crest.plane(0).unwrap();
        assert_eq!(native_plane, 1 << 20, "one byte per native sample");
        let mut end = 0;
        let mut total = 0;
        for i in 0..crest.level_count() {
            let (offset, bytes) = crest.plane(i).unwrap();
            assert_eq!(offset, (end + 511) & !511, "each plane starts on a block boundary");
            assert_eq!(bytes, native_plane >> (2 * i), "a level holds a quarter of its predecessor");
            total += bytes;
            end = offset + bytes;
        }
        assert_eq!(total, 1_398_016);
        assert!(crest.block_bytes() < surface.cell_bytes());
        assert_eq!(crest.plane(crest.level_count()), None);
    }

    #[test]
    fn crest_samples_keep_the_tile_order_at_one_byte_each() {
        let crest = CrestLayout::new(SurfaceLayout::new(9, 19).unwrap());
        let side = 1u32 << 10;
        assert_eq!(crest.sample_offset(0, 0, 0), Some(0));
        assert_eq!(crest.sample_offset(0, 0, 1), Some(1));
        assert_eq!(crest.sample_offset(0, 1, 0), Some(16), "rows advance latitude inside a tile");
        assert_eq!(crest.sample_offset(0, 0, 16), Some(256), "the next tile east");
        assert_eq!(crest.sample_offset(0, 16, 0), Some(256 * (side / 16)), "the next tile north");
        assert_eq!(crest.sample_offset(0, side, 0), None);
        assert_eq!(crest.sample_offset(0, 0, side), None);
        // Every sample of every level lands exactly once inside its own plane.
        let small = CrestLayout::new(SurfaceLayout::new(12, 16).unwrap());
        let (plane, bytes) = small.plane(0).unwrap();
        let mut seen = std::collections::BTreeSet::new();
        for y in 0..1u32 << 4 {
            for x in 0..1u32 << 4 {
                let at = small.sample_offset(0, y, x).unwrap();
                assert!((plane..plane + bytes).contains(&at));
                assert!(seen.insert(at));
            }
        }
        assert_eq!(seen.len(), 256);
    }

    #[test]
    fn the_crest_directory_follows_the_cell_index_and_pads_to_a_block() {
        let index = CellIndexLayout::new(3, 5, 32).unwrap();
        let crest = CrestDirectory::new(3, 5, index.end()).unwrap();
        assert_eq!(crest.offset, index.end());
        assert_eq!(crest.bytes, 15 * 4);
        assert_eq!(crest.end(), crest.offset + 512);
        assert_eq!(crest.entry_offset(0), Some(crest.offset));
        assert_eq!(crest.entry_offset(14), Some(crest.offset + 56));
        assert_eq!(crest.entry_offset(15), None, "one entry per cell, no more");
        assert!(CrestDirectory::new(0, 5, 512).is_none());
    }

    #[test]
    fn derived_postings_do_not_restrict_legal_native_cells() {
        let layout = SurfaceLayout::new(13, 28).unwrap();
        assert_eq!(layout.level_count(), MAX_SURFACE_LEVELS);
        let last = layout.level(layout.level_count() - 1).unwrap();
        assert_eq!(last.posting_log2, 24);
        assert_eq!(last.height_bytes, 512);
        assert!(layout.cell_bytes() > 1 << 31);
    }

    #[test]
    fn cell_index_packs_partial_rectangles_and_aligns_the_cell_blocks() {
        let index = CellIndexLayout::new(3, 5, 32).unwrap();
        assert_eq!(index.offset, 512);
        assert_eq!(index.bytes, (15 + 6 + 2 + 1) * 2);
        assert_eq!(index.end(), 1024);
        assert_eq!(index.node_offset(2, 4, 0), Some(540));
        assert_eq!(index.node_offset(1, 2, 1), Some(552));
        assert_eq!(index.node_offset(0, 1, 2), Some(556));
        assert_eq!(index.node_offset(0, 0, 3), Some(558));
        assert_eq!(index.node_offset(0, 0, 4), None);
        assert_eq!(index.node_offset(1, 0, 3), None);
        assert_eq!(index.node_offset(u32::MAX, u32::MAX, 0), None);
    }

    #[test]
    fn packed_groups_cover_each_node_once_and_levels_do_not_overlap() {
        let layout = SurfaceLayout::new(9, 19).unwrap();
        assert_eq!(layout.level_count(), 7);
        let mut end = 0;
        for i in 0..layout.level_count() {
            let level = layout.level(i).unwrap();
            assert_eq!(level.offset, (end + 511) & !511);
            let side = 1 << level.samples_log2;
            let mut offsets = std::collections::BTreeSet::new();
            for log in MAX_LEAF_LOG2..=level.samples_log2 {
                for y in (0..side).step_by(1 << log) {
                    for x in (0..side).step_by(1 << log) {
                        let at = level.bound_offset(y, x, log).unwrap();
                        assert!(at >= level.bounds_offset && at + 2 <= level.bounds_offset + level.bounds_bytes);
                        assert!(offsets.insert(at));
                    }
                }
            }
            end = level.bounds_offset + level.bounds_bytes;
        }
        assert_eq!(layout.cell_bytes(), (end + 511) & !511);
        assert_eq!(SurfaceLayout::new(12, 16).unwrap().level_count(), 1);
        assert!(SurfaceLayout::new(4, 28).is_none());
    }
}
