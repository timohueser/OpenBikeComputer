//! Grid-coordinate terrain access for panoramic rendering. Heights and conservative bounds
//! share a validated geographic container; only the visible patches need height decoding.

use obc_formats::{
    io::{ByteSource, Error},
    obct::{self, CellIndexLayout, CrestDirectory, CrestLayout, SurfaceLayout, SurfaceLevel, NODATA, TILE_BYTES},
};

use crate::{TerrainHeader, TerrainReader};

/// A measured bilinear patch: h(x,y) = height + east*x + north*y + cross*x*y.
/// Coordinates x/y are fractions of one posting. Derivatives are rises per posting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Patch {
    pub height: f32,
    pub east: f32,
    pub north: f32,
    pub cross: f32,
}
impl Patch {
    fn from_corners([a, b, c, d]: [i16; 4]) -> Option<Self> {
        if [a, b, c, d].contains(&NODATA) {
            return None;
        }
        let (a, b, c, d) = (a as f32, b as f32, c as f32, d as f32);
        Some(Self { height: a, east: b - a, north: c - a, cross: a - b - c + d })
    }
}

/// Geometry on the common OBCT lattice (origin -2^28 microdegrees on both axes).
#[derive(Clone, Copy, Debug)]
pub struct Geometry {
    pub posting_log2: u8,
    pub cell_log2: u8,
    pub cell_min_i: u32,
    pub cell_min_j: u32,
    pub cell_rows: u16,
    pub cell_cols: u16,
}

struct Bank<const N: usize, const WORDS: usize> {
    words: [[i16; WORDS]; N],
    owner: u32,
    keys: [u32; N],
    stamps: [u32; N],
    tick: u32,
    recent: usize,
}
impl<const N: usize, const WORDS: usize> Bank<N, WORDS> {
    fn slot(&mut self, source: &dyn ByteSource, owner: u32, key: u32, bytes: usize) -> Result<usize, Error> {
        assert!(bytes <= WORDS * 2);
        if self.owner != owner {
            self.keys.fill(u32::MAX);
            self.stamps.fill(0);
            self.owner = owner;
        }
        let recent = self.recent;
        if self.keys[recent] == key {
            return Ok(recent);
        }
        self.tick = self.tick.wrapping_add(1);
        if let Some(i) = self.keys.iter().position(|&candidate| candidate == key) {
            self.stamps[i] = self.tick;
            self.recent = i;
            return Ok(i);
        }
        let i = (0..N).min_by_key(|&i| (self.keys[i] != u32::MAX, self.stamps[i])).unwrap();
        self.keys[i] = u32::MAX;
        // SAFETY: i16 has no invalid representations. The byte view stays within the
        // exclusively borrowed slot; words are interpreted as little-endian only after success.
        let out = unsafe { core::slice::from_raw_parts_mut(self.words[i].as_mut_ptr().cast::<u8>(), bytes) };
        source.read_at(key.into(), out)?;
        self.keys[i] = key;
        self.stamps[i] = self.tick;
        self.recent = i;
        Ok(i)
    }

    fn word(&self, slot: usize, byte: usize) -> i16 {
        i16::from_le(self.words[slot][byte / 2])
    }

    /// Crest planes hold one byte per sample, so they read through this bank as bytes.
    fn byte(&self, slot: usize, byte: usize) -> u8 {
        (self.words[slot][byte / 2].to_le_bytes())[byte % 2]
    }
}

/// One shared cache for all terrain levels/sources. Initialize directly in the memory arena.
/// Each bank binds to one source identity; changing sources invalidates its entries.
pub struct SurfaceCache {
    heights: Bank<18, 1024>,
    bounds: Bank<16, 256>,
    directory: Bank<6, 256>,
    last_cell: [u32; 4],
    last_crest: [u32; 4],
    // Whole-cell maxima are visited across many sectors. Keep their scalar values
    // separately instead of retaining an almost empty bounds block for each cell.
    root_keys: [u32; 1024],
    root_values: [i16; 1024],
    root_owner: u32,
    failed: bool,
}
impl SurfaceCache {
    /// # Safety
    /// `out` must be aligned, exclusive writable storage for one SurfaceCache.
    pub unsafe fn init_at(out: *mut Self) {
        // SAFETY: all fields admit zero, including the false failure flag.
        unsafe { core::ptr::write_bytes(out.cast::<u8>(), 0, core::mem::size_of::<Self>()) };
    }

    /// A failed read invalidates its cache entry and remains visible to the job owner.
    pub fn failed(&self) -> bool {
        self.failed
    }
}

#[derive(Clone, Copy, Default)]
struct BoundAddress {
    base: u32,
    group_rows_log2: u8,
    group_nodes_log2: u8,
    root: bool,
}
#[derive(Clone, Copy)]
struct Level {
    layout: SurfaceLevel,
    bounds: [BoundAddress; 16],
}

pub struct SurfaceReader<'a> {
    source: &'a dyn ByteSource,
    header: TerrainHeader,
    levels: [Option<Level>; obct::MAX_SURFACE_LEVELS],
    cell_index: Option<CellIndexLayout>,
    /// §9's crest layer, when the container carries one. Panorama-only: nothing else reads it.
    crest: Option<(CrestDirectory, CrestLayout)>,
    generation: u32,
}
impl<'a> SurfaceReader<'a> {
    pub fn parse(source: &'a dyn ByteSource) -> Result<Self, Error> {
        if source.len() > u32::MAX as u64 {
            return Err(Error::TooLarge);
        }
        let reader = TerrainReader::parse(source)?;
        let header = *reader.header();
        if header.flags & obct::SURFACE_FLAG == 0 {
            return Err(Error::BadVersion);
        }
        let cell_index = if header.flags & obct::CELL_INDEX_FLAG != 0 {
            Some(
                CellIndexLayout::new(header.cell_rows, header.cell_cols, header.directory_offset)
                    .ok_or(Error::BadOffset)?,
            )
        } else {
            None
        };
        let layout = SurfaceLayout::new(header.posting_log2, header.cell_log2).ok_or(Error::BadOffset)?;
        let crest = if header.flags & obct::CREST_FLAG != 0 {
            let entries = u32::from(header.cell_rows)
                .checked_mul(u32::from(header.cell_cols))
                .and_then(|n| n.checked_mul(4))
                .ok_or(Error::BadOffset)?;
            let after = match cell_index {
                Some(index) => index.end(),
                None => header.directory_offset.checked_add(entries).ok_or(Error::BadOffset)?,
            };
            let directory = CrestDirectory::new(header.cell_rows, header.cell_cols, after).ok_or(Error::BadOffset)?;
            Some((directory, CrestLayout::new(layout)))
        } else {
            None
        };
        let levels = core::array::from_fn(|index| {
            let level = layout.level(index)?;
            let mut bounds = [BoundAddress::default(); 16];
            for log in 2..=level.samples_log2 {
                let start = 2 + ((log - 2) & !3);
                bounds[log as usize] = BoundAddress {
                    base: level.bound_offset(0, 0, log)?,
                    group_rows_log2: level.samples_log2.saturating_sub(start + 3),
                    group_nodes_log2: 3 - (log - start),
                    root: log == level.samples_log2,
                };
            }
            Some(Level { layout: level, bounds })
        });
        Ok(Self { source, header, levels, cell_index, crest, generation: reader.generation() })
    }

    pub fn level_count(&self) -> usize {
        self.levels.iter().flatten().count()
    }

    pub fn geometry(&self, level: usize) -> Option<Geometry> {
        Some(Geometry {
            posting_log2: self.levels.get(level)?.as_ref()?.layout.posting_log2,
            cell_log2: self.header.cell_log2,
            cell_min_i: self.header.cell_min_i,
            cell_min_j: self.header.cell_min_j,
            cell_rows: self.header.cell_rows,
            cell_cols: self.header.cell_cols,
        })
    }

    pub fn cell_present(&self, cache: &mut SurfaceCache, y: u32, x: u32) -> bool {
        match self.cell(cache, y, x) {
            Ok(value) => value.is_some(),
            Err(_) => {
                cache.failed = true;
                false
            }
        }
    }

    /// Container-relative cell-group coordinates. Each node spans 2^log2 cells;
    /// native inclusive maxima conservatively bound every terrain level.
    pub fn max_cell_group(&self, cache: &mut SurfaceCache, y: u32, x: u32, log2: u8) -> Option<i16> {
        let at = self.cell_index?.node_offset(y, x, log2)?;
        let first = at & !511;
        match cache.bounds.slot(self.source, self.generation, first, 512) {
            Ok(slot) => {
                let value = cache.bounds.word(slot, (at - first) as usize);
                (value != i16::MAX).then_some(value)
            }
            Err(_) => {
                cache.failed = true;
                None
            }
        }
    }

    /// Global sample-lattice coordinates of the patch's southwest corner.
    pub fn patch(&self, cache: &mut SurfaceCache, level: usize, y: u32, x: u32) -> Option<Patch> {
        let index = level;
        let level = self.levels.get(level)?.as_ref()?.layout;
        match self.patch_inner(cache, index, level, y, x) {
            Ok(value) => value,
            Err(_) => {
                cache.failed = true;
                None
            }
        }
    }

    /// Corners of a larger bilinear patch, in this level's global sample coordinates.
    pub fn coarse_patch(&self, cache: &mut SurfaceCache, level: usize, y: u32, x: u32, log2: u8) -> Option<Patch> {
        if log2 > 15 || (y | x) & ((1u32 << log2) - 1) != 0 {
            return None;
        }
        // Coarse levels retain the exact same lattice vertices. Read these sparse corners
        // from the most compact level instead of fetching distant native-height tiles.
        let shift = usize::from(log2).min(self.level_count().checked_sub(level + 1)?);
        let (y, x, log2) = (y >> shift, x >> shift, log2 - shift as u8);
        let index = level + shift;
        let level = self.levels.get(index)?.as_ref()?.layout;
        if log2 > level.samples_log2 {
            return None;
        }
        let read = |cache: &mut SurfaceCache| -> Result<Option<Patch>, Error> {
            let (cy, cx) = (y >> level.samples_log2, x >> level.samples_log2);
            let Some(cell) = self.cell(cache, cy, cx)? else { return Ok(None) };
            let home = (cy, cx, cell);
            let width = 1u32 << log2;
            // Residuals describe the true inclusive corners. Clamping a missing neighbour
            // would change that surface while retaining its old error bound.
            for (yy, xx) in [(y, x + width), (y + width, x), (y + width, x + width)] {
                if self.cell(cache, yy >> level.samples_log2, xx >> level.samples_log2)?.is_none() {
                    return Ok(None);
                }
            }
            Ok(Patch::from_corners([
                self.corner(cache, index, level, home, y, x)?,
                self.corner(cache, index, level, home, y, x + width)?,
                self.corner(cache, index, level, home, y + width, x)?,
                self.corner(cache, index, level, home, y + width, x + width)?,
            ]))
        };
        match read(cache) {
            Ok(value) => value,
            Err(_) => {
                cache.failed = true;
                None
            }
        }
    }

    /// Conservative height and component-gradient residuals for the corner-defined patch.
    pub fn approximation(
        &self,
        cache: &mut SurfaceCache,
        index: usize,
        y: u32,
        x: u32,
        log2: u8,
    ) -> Option<(f32, f32)> {
        let level = self.levels.get(index)?.as_ref()?;
        if !(2..=level.layout.samples_log2).contains(&log2) {
            return None;
        }
        let address = level.bounds[log2 as usize];
        let nodes_log2 = level.layout.samples_log2 - log2;
        let read = |cache: &mut SurfaceCache| -> Result<Option<u8>, Error> {
            let Some(cell) = self.cell(cache, y >> nodes_log2, x >> nodes_log2)? else { return Ok(None) };
            let node_mask = (1u32 << nodes_log2) - 1;
            let (y, x) = (y & node_mask, x & node_mask);
            let group = ((y >> address.group_nodes_log2) << address.group_rows_log2) + (x >> address.group_nodes_log2);
            let mask = (1u32 << address.group_nodes_log2) - 1;
            let node = ((y & mask) << address.group_nodes_log2) + (x & mask);
            let relative = obct::approximation_offset(address.base + group * 256 + node * 2);
            let start = cell + level.layout.bounds_offset;
            let at = cell + relative;
            let first = start + (at - start) / 512 * 512;
            let bytes = (start + level.layout.bounds_bytes - first).min(512) as usize;
            let slot = cache.bounds.slot(self.source, self.generation, first, bytes)?;
            let word = cache.bounds.word(slot, ((at - first) & !1) as usize) as u16;
            Ok(Some((word >> ((at & 1) * 8)) as u8))
        };
        match read(cache) {
            Ok(Some(code)) => {
                Some((obct::approximation_error(code, false), obct::approximation_error(code >> 4, true)))
            }
            Ok(None) => None,
            Err(_) => {
                cache.failed = true;
                None
            }
        }
    }

    fn patch_inner(
        &self,
        cache: &mut SurfaceCache,
        index: usize,
        level: SurfaceLevel,
        y: u32,
        x: u32,
    ) -> Result<Option<Patch>, Error> {
        let cy = y >> level.samples_log2;
        let cx = x >> level.samples_log2;
        let Some(home) = self.cell(cache, cy, cx)? else { return Ok(None) };
        let mask = (1u32 << level.samples_log2) - 1;
        let (ly, lx) = (y & mask, x & mask);
        if ly & 15 != 15 && lx & 15 != 15 {
            let tile = home + level.offset + (((ly >> 4) << (level.samples_log2 - 4)) + (lx >> 4)) * TILE_BYTES as u32;
            let start = home + level.offset;
            let (slot, base) = self.height_slot(cache, level, start, tile)?;
            let at = base + ((ly & 15) * 16 + (lx & 15)) as usize * 2;
            let bank = &cache.heights;
            let corners =
                [bank.word(slot, at), bank.word(slot, at + 2), bank.word(slot, at + 32), bank.word(slot, at + 34)];
            // A crest read can evict this slot, so the heights are taken first.
            let mut lifted = corners;
            for (k, (dy, dx)) in [(0, 0), (0, 1), (1, 0), (1, 1)].into_iter().enumerate() {
                let lift = self.lift(cache, index, (cy, cx), ly + dy, lx + dx)?;
                lifted[k] = Self::lifted(corners[k], lift);
            }
            return Ok(Patch::from_corners(lifted));
        }
        let a = self.corner(cache, index, level, (cy, cx, home), y, x)?;
        let b = self.corner(cache, index, level, (cy, cx, home), y, x + 1)?;
        let c = self.corner(cache, index, level, (cy, cx, home), y + 1, x)?;
        let d = self.corner(cache, index, level, (cy, cx, home), y + 1, x + 1)?;
        Ok(Patch::from_corners([a, b, c, d]))
    }

    fn corner(
        &self,
        cache: &mut SurfaceCache,
        index: usize,
        level: SurfaceLevel,
        home: (u32, u32, u32),
        y: u32,
        x: u32,
    ) -> Result<i16, Error> {
        let (cy, cx) = (y >> level.samples_log2, x >> level.samples_log2);
        let mask = (1u32 << level.samples_log2) - 1;
        let mut ly = y & mask;
        let mut lx = x & mask;
        // Which cell the height is actually read from, which is also the cell whose crest plane
        // describes it: the neighbour when it is present, the home cell when the corner clamps.
        let mut from = (cy, cx);
        let cell = if (cy, cx) == (home.0, home.1) {
            home.2
        } else if let Some(cell) = self.cell(cache, cy, cx)? {
            cell
        } else {
            if cy != home.0 {
                ly = mask;
            }
            if cx != home.1 {
                lx = mask;
            }
            from = (home.0, home.1);
            home.2
        };
        let start = cell + level.offset;
        let tile = start + (((ly >> 4) << (level.samples_log2 - 4)) + (lx >> 4)) * TILE_BYTES as u32;
        let (slot, base) = self.height_slot(cache, level, start, tile)?;
        let height = cache.heights.word(slot, base + ((ly & 15) * 16 + (lx & 15)) as usize * 2);
        let lift = self.lift(cache, index, from, ly, lx)?;
        Ok(Self::lifted(height, lift))
    }

    fn height_slot(
        &self,
        cache: &mut SurfaceCache,
        level: SurfaceLevel,
        start: u32,
        tile: u32,
    ) -> Result<(usize, usize), Error> {
        let len = level.height_bytes.min(2048);
        let first = start + ((tile - start) & !(len - 1));
        let slot = cache.heights.slot(self.source, self.generation, first, len as usize)?;
        Ok((slot, (tile - first) as usize))
    }

    /// Global node coordinates, each node spanning 2^block_log2 sample intervals.
    /// Unknown bounds and nodes above a geographic cell return None so the caller
    /// descends without hiding terrain.
    pub fn max_height(
        &self,
        cache: &mut SurfaceCache,
        level: usize,
        block_y: u32,
        block_x: u32,
        block_log2: u8,
    ) -> Option<i16> {
        let level = self.levels.get(level)?.as_ref()?;
        if block_log2 > level.layout.samples_log2 || block_log2 < 2 {
            return None;
        }
        let nodes_log2 = level.layout.samples_log2 - block_log2;
        let (cy, cx) = (block_y >> nodes_log2, block_x >> nodes_log2);
        let node_mask = (1u32 << nodes_log2) - 1;
        match self.bound_inner(
            cache,
            level.layout,
            level.bounds[block_log2 as usize],
            (cy, cx),
            block_y & node_mask,
            block_x & node_mask,
        ) {
            Ok(value) => value,
            Err(_) => {
                cache.failed = true;
                None
            }
        }
    }

    fn bound_inner(
        &self,
        cache: &mut SurfaceCache,
        level: SurfaceLevel,
        address: BoundAddress,
        cell_at: (u32, u32),
        y: u32,
        x: u32,
    ) -> Result<Option<i16>, Error> {
        let Some(cell) = self.cell(cache, cell_at.0, cell_at.1)? else { return Ok(None) };
        let group = ((y >> address.group_nodes_log2) << address.group_rows_log2) + (x >> address.group_nodes_log2);
        let mask = (1u32 << address.group_nodes_log2) - 1;
        let node = ((y & mask) << address.group_nodes_log2) + (x & mask);
        let offset = address.base + group * 256 + node * 2;
        let start = cell + level.bounds_offset;
        let at = cell + offset;
        // A geographic window avoids collisions between neighbouring cells when a map
        // directory has a wide row stride. Full offsets below verify wrapped collisions.
        let memo = (((cell_at.0 & 31) << 5) | (cell_at.1 & 31)) as usize;
        if address.root {
            if cache.root_owner != self.generation {
                cache.root_keys.fill(0);
                cache.root_owner = self.generation;
            }
            if cache.root_keys[memo] == at {
                let value = cache.root_values[memo];
                return Ok((value != i16::MAX).then_some(value));
            }
        }
        let first = start + (at - start) / 512 * 512;
        let bytes = (start + level.bounds_bytes - first).min(512) as usize;
        let slot = cache.bounds.slot(self.source, self.generation, first, bytes)?;
        let value = cache.bounds.word(slot, (at - first) as usize);
        if address.root {
            cache.root_keys[memo] = at;
            cache.root_values[memo] = value;
        }
        Ok((value != i16::MAX).then_some(value))
    }

    /// Byte offset of a cell's crest block, or `None` when the cell has no reference data.
    fn crest_block(&self, cache: &mut SurfaceCache, y: u32, x: u32) -> Result<Option<u32>, Error> {
        let Some((directory, _)) = self.crest else { return Ok(None) };
        let last = cache.last_crest;
        if last[0] == self.generation && last[1] == y && last[2] == x {
            return Ok((last[3] != 0).then_some(last[3]));
        }
        let Some(dy) = y.checked_sub(self.header.cell_min_i) else { return Ok(None) };
        let Some(dx) = x.checked_sub(self.header.cell_min_j) else { return Ok(None) };
        if dy >= u32::from(self.header.cell_rows) || dx >= u32::from(self.header.cell_cols) {
            return Ok(None);
        }
        let Some(at) = directory.entry_offset(dy * u32::from(self.header.cell_cols) + dx) else {
            return Ok(None);
        };
        let first = at & !511;
        let bytes = (self.source.len() - u64::from(first)).min(512) as usize;
        let slot = cache.directory.slot(self.source, self.generation, first, bytes)?;
        let local = (at - first) as usize;
        let offset = u32::from(cache.directory.word(slot, local) as u16)
            | (u32::from(cache.directory.word(slot, local + 2) as u16) << 16);
        cache.last_crest = [self.generation, y, x, offset];
        Ok((offset != 0).then_some(offset))
    }

    /// Metres the panorama surface stands above the native sample (§9.2), or zero where the
    /// container carries no lift. Crest planes share the height bank: the keys are absolute file
    /// offsets, so one LRU divides itself between heights and lifts instead of a fixed split.
    fn lift(&self, cache: &mut SurfaceCache, index: usize, cell: (u32, u32), y: u32, x: u32) -> Result<i16, Error> {
        let Some((_, crest)) = self.crest else { return Ok(0) };
        let Some(block) = self.crest_block(cache, cell.0, cell.1)? else { return Ok(0) };
        let (Some(at), Some((plane, bytes))) = (crest.sample_offset(index, y, x), crest.plane(index)) else {
            return Ok(0);
        };
        let len = bytes.min(1024);
        let start = block + plane;
        let at = block + at;
        let first = start + (at - start) / len * len;
        let read = (start + bytes - first).min(len) as usize;
        let slot = cache.heights.slot(self.source, self.generation, first, read)?;
        Ok(i16::from(cache.heights.byte(slot, (at - first) as usize)) * obct::CREST_QUANTUM as i16)
    }

    /// A lift never applies to a hole: §9.2 keeps `NODATA` whatever the plane says.
    fn lifted(height: i16, lift: i16) -> i16 {
        if height == NODATA {
            height
        } else {
            height.saturating_add(lift)
        }
    }

    fn cell(&self, cache: &mut SurfaceCache, y: u32, x: u32) -> Result<Option<u32>, Error> {
        let last = cache.last_cell;
        if last[0] == self.generation && last[1] == y && last[2] == x {
            return Ok((last[3] != 0).then_some(last[3]));
        }
        let Some(dy) = y.checked_sub(self.header.cell_min_i) else { return Ok(None) };
        let Some(dx) = x.checked_sub(self.header.cell_min_j) else { return Ok(None) };
        if dy >= self.header.cell_rows as u32 || dx >= self.header.cell_cols as u32 {
            return Ok(None);
        }
        let index = dy * self.header.cell_cols as u32 + dx;
        // Fetch a directory sector once for its neighbouring cell entries.
        let at = self.header.directory_offset + index * 4;
        let first = at & !511;
        let bytes = (self.source.len() - u64::from(first)).min(512) as usize;
        let slot = cache.directory.slot(self.source, self.generation, first, bytes)?;
        let local = (at - first) as usize;
        let offset = u32::from(cache.directory.word(slot, local) as u16)
            | (u32::from(cache.directory.word(slot, local + 2) as u16) << 16);
        cache.last_cell = [self.generation, y, x, offset];
        Ok((offset != 0).then_some(offset))
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use core::cell::Cell;
    use std::{boxed::Box, vec, vec::Vec};

    struct Source {
        bytes: Vec<u8>,
        fail_next: Cell<bool>,
    }
    impl ByteSource for Source {
        fn len(&self) -> u64 {
            self.bytes.len() as u64
        }
        fn read_at(&self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
            if self.fail_next.replace(false) {
                out.fill(0);
                return Err(Error::Io);
            }
            obc_formats::io::SliceSource(&self.bytes).read_at(offset, out)
        }
    }
    fn cache() -> Box<SurfaceCache> {
        let mut slot = Box::<SurfaceCache>::new_uninit();
        unsafe {
            SurfaceCache::init_at(slot.as_mut_ptr());
            slot.assume_init()
        }
    }
    fn height(y: u32, x: u32) -> i16 {
        (300 + y * 7 + x * 3 + y * x / 8) as i16
    }
    fn source(second_cell: bool) -> Source {
        let layout = SurfaceLayout::new(9, 16).unwrap();
        let mut bytes = vec![0u8; 512 + layout.cell_bytes() as usize * 2];
        bytes[..4].copy_from_slice(&obct::MAGIC);
        bytes[4] = obct::SURFACE_VERSION;
        bytes[5] = 9;
        bytes[6] = 16;
        bytes[7] = obct::SURFACE_FLAG;
        bytes[16..18].copy_from_slice(&1u16.to_le_bytes());
        bytes[18..20].copy_from_slice(&2u16.to_le_bytes());
        bytes[20..24].copy_from_slice(&32u32.to_le_bytes());
        for cx in 0..2u32 {
            if cx == 1 && !second_cell {
                continue;
            }
            let base = 512 + cx * layout.cell_bytes();
            bytes[32 + cx as usize * 4..36 + cx as usize * 4].copy_from_slice(&base.to_le_bytes());
            for index in 0..layout.level_count() {
                let level = layout.level(index).unwrap();
                let side = 1u32 << level.samples_log2;
                for y in 0..side {
                    for x in 0..side {
                        let tile = ((y >> 4) << (level.samples_log2 - 4)) + (x >> 4);
                        let at = (base + level.offset + tile * 512 + ((y & 15) * 16 + (x & 15)) * 2) as usize;
                        bytes[at..at + 2].copy_from_slice(&height(y << index, (cx * side + x) << index).to_le_bytes());
                    }
                }
                for log in 2..=level.samples_log2 {
                    for y in (0..side).step_by(1 << log) {
                        for x in (0..side).step_by(1 << log) {
                            let yy = (y + (1 << log)).min(side - 1);
                            let xx =
                                (cx * side + x + (1 << log)).min(if second_cell { 2 * side - 1 } else { side - 1 });
                            let at = (base + level.bound_offset(y, x, log).unwrap()) as usize;
                            bytes[at..at + 2].copy_from_slice(&height(yy << index, xx << index).to_le_bytes());
                        }
                    }
                }
            }
        }
        Source { bytes, fail_next: Cell::new(false) }
    }

    #[test]
    fn patch_and_bounds_address_all_levels_and_cell_edges() {
        let terrain = source(true);
        let reader = SurfaceReader::parse(&terrain).unwrap();
        let mut cache = cache();
        assert_eq!(reader.level_count(), 4);
        for level in 0..reader.level_count() {
            let side = 128 >> level;
            for y in 0..side {
                for x in 0..side * 2 {
                    let got = reader.patch(&mut cache, level, y, x).unwrap();
                    let corner = |yy: u32, xx: u32| {
                        let (yy, xx) = if yy >= side || xx >= side * 2 {
                            let left = x / side * side;
                            (yy.min(side - 1), xx.clamp(left, left + side - 1))
                        } else {
                            (yy, xx)
                        };
                        height(yy << level, xx << level) as f32
                    };
                    let (a, b, c, d) = (corner(y, x), corner(y, x + 1), corner(y + 1, x), corner(y + 1, x + 1));
                    assert_eq!(got, Patch { height: a, east: b - a, north: c - a, cross: a - b - c + d });
                }
            }
            for log in 2..=(7 - level as u8) {
                for y in 0..side >> log {
                    for x in 0..(side * 2) >> log {
                        let yy = ((y + 1) << log).min(side - 1);
                        let xx = ((x + 1) << log).min(side * 2 - 1);
                        assert_eq!(
                            reader.max_height(&mut cache, level, y, x, log),
                            Some(height(yy << level, xx << level))
                        );
                    }
                }
            }
        }
        assert!(!cache.failed());
        let mut unknown = source(true);
        let level = SurfaceLayout::new(9, 16).unwrap().level(0).unwrap();
        let at = 512 + level.bound_offset(0, 0, 2).unwrap() as usize;
        unknown.bytes[at..at + 2].copy_from_slice(&i16::MAX.to_le_bytes());
        let reader = SurfaceReader::parse(&unknown).unwrap();
        assert!(reader.max_height(&mut cache, 0, 0, 0, 2).is_none());
        assert!(!cache.failed(), "an unbounded node is not an I/O error");
    }

    #[test]
    fn coarse_patches_require_true_corners_across_cell_seams() {
        let complete = source(true);
        let cropped = source(false);
        let reader = SurfaceReader::parse(&complete).unwrap();
        let edge = SurfaceReader::parse(&cropped).unwrap();
        let mut cache = cache();
        for level in 0..reader.level_count() {
            let side = 128 >> level;
            let corners =
                [(4, side - 4), (4, side), (8, side - 4), (8, side)].map(|(y, x)| height(y << level, x << level));
            assert_eq!(reader.coarse_patch(&mut cache, level, 4, side - 4, 2), Patch::from_corners(corners));
            assert_eq!(edge.coarse_patch(&mut cache, level, 4, side - 4, 2), None);
            assert!(edge.patch(&mut cache, level, 4, side - 1).is_some(), "native edge clamping remains available");
        }
        assert!(!cache.failed(), "absent neighbours are coverage gaps, not read faults");
    }

    #[test]
    fn cache_does_not_cross_serve_sources_or_reuse_failed_fills() {
        let source_a = source(true);
        let source_b = source(false);
        let a = SurfaceReader::parse(&source_a).unwrap();
        let b = SurfaceReader::parse(&source_b).unwrap();
        let mut cache = cache();
        let full = a.patch(&mut cache, 0, 2, 127).unwrap();
        let edge = b.patch(&mut cache, 0, 2, 127).unwrap();
        assert_ne!(full.east, edge.east);
        assert_eq!(edge.east, 0.0);
        assert!(b.patch(&mut cache, 0, 2, 128).is_none());
        assert_eq!(a.patch(&mut cache, 0, 2, 127), Some(full));
        assert_eq!(a.max_height(&mut cache, 0, 0, 0, 7), Some(height(127, 128)));
        assert_eq!(b.max_height(&mut cache, 0, 0, 0, 7), Some(height(127, 127)));
        assert_eq!(a.max_height(&mut cache, 0, 0, 0, 7), Some(height(127, 128)));
        source_a.fail_next.set(true);
        assert!(a.max_height(&mut cache, 1, 0, 0, 2).is_none());
        assert!(cache.failed());
        assert_eq!(a.max_height(&mut cache, 1, 0, 0, 2), Some(height(8, 8)));
        assert!(cache.failed(), "a read fault stays visible even after a successful retry");
    }

    #[test]
    fn cell_groups_are_container_relative_and_share_safe_cached_reads() {
        let indexed = |maximum: i16| {
            let mut terrain = source(true);
            let index = CellIndexLayout::new(1, 2, 32).unwrap();
            let shift = index.end() as usize - 512;
            terrain.bytes.splice(512..512, vec![0; shift]);
            terrain.bytes[7] |= obct::CELL_INDEX_FLAG;
            terrain.bytes[8..12].copy_from_slice(&3u32.to_le_bytes());
            terrain.bytes[12..16].copy_from_slice(&5u32.to_le_bytes());
            for at in [32, 36] {
                let old = u32::from_le_bytes(terrain.bytes[at..at + 4].try_into().unwrap());
                terrain.bytes[at..at + 4].copy_from_slice(&(old + shift as u32).to_le_bytes());
            }
            for (y, x, log, value) in [(0, 0, 0, 1000), (0, 1, 0, maximum), (0, 0, 1, maximum)] {
                let at = index.node_offset(y, x, log).unwrap() as usize;
                terrain.bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
            }
            terrain
        };
        let terrain = indexed(2000);
        let unknown = indexed(i16::MAX);
        let reader = SurfaceReader::parse(&terrain).unwrap();
        let other = SurfaceReader::parse(&unknown).unwrap();
        let mut cache = cache();
        assert_eq!(reader.max_cell_group(&mut cache, 0, 0, 0), Some(1000));
        assert_eq!(reader.max_cell_group(&mut cache, 0, 1, 0), Some(2000));
        assert_eq!(reader.max_cell_group(&mut cache, 0, 0, 1), Some(2000));
        assert_eq!(reader.max_cell_group(&mut cache, 3, 5, 1), None);
        assert_eq!(reader.max_cell_group(&mut cache, 0, 0, 2), None);
        assert_eq!(other.max_cell_group(&mut cache, 0, 0, 1), None);
        assert!(!cache.failed());
        terrain.fail_next.set(true);
        assert_eq!(reader.max_cell_group(&mut cache, 0, 0, 1), None);
        assert!(cache.failed());
        assert_eq!(reader.max_cell_group(&mut cache, 0, 0, 1), Some(2000));
        let plain = source(true);
        assert_eq!(SurfaceReader::parse(&plain).unwrap().max_cell_group(&mut cache, 0, 0, 1), None);
    }
}
