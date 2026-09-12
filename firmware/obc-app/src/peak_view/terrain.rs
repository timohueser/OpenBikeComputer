//! The geographic terrain source shared by the simulator and device panorama jobs.

use super::surface::{SurfaceLevel, SurfaceTerrain};
use obc_elevation::surface::{Patch, SurfaceCache, SurfaceReader};
use obc_formats::io::{ByteSource, Error};

pub struct Terrain<'a> {
    reader: SurfaceReader<'a>,
    cache: SurfaceCache,
}

impl<'a> Terrain<'a> {
    pub fn parse(source: &'a dyn ByteSource) -> Result<Self, Error> {
        let mut slot = core::mem::MaybeUninit::uninit();
        // SAFETY: exclusive storage, read only after successful initialization.
        unsafe {
            Self::init_at(slot.as_mut_ptr(), source)?;
            Ok(slot.assume_init())
        }
    }

    /// # Safety
    /// `out` must be aligned, writable, exclusive storage for a Terrain.
    pub unsafe fn init_at(out: *mut Self, source: &'a dyn ByteSource) -> Result<(), Error> {
        let reader = SurfaceReader::parse(source)?;
        unsafe {
            core::ptr::addr_of_mut!((*out).reader).write(reader);
            SurfaceCache::init_at(core::ptr::addr_of_mut!((*out).cache));
        }
        Ok(())
    }

    pub fn failed(&self) -> bool {
        self.cache.failed()
    }

    pub fn ground_height(&mut self, lat: i32, lon: i32) -> Option<f32> {
        let g = self.reader.geometry(0)?;
        let y = i64::from(lat) + (1 << 28);
        let x = i64::from(lon) + (1 << 28);
        if !(0..1 << 29).contains(&y) || !(0..1 << 29).contains(&x) {
            return None;
        }
        let mask = (1u32 << g.posting_log2) - 1;
        let fy = (y as u32 & mask) as f32 / (mask + 1) as f32;
        let fx = (x as u32 & mask) as f32 / (mask + 1) as f32;
        let p = self.reader.patch(&mut self.cache, 0, y as u32 >> g.posting_log2, x as u32 >> g.posting_log2)?;
        Some(p.height + p.east * fx + p.north * fy + p.cross * fx * fy)
    }
}

impl SurfaceTerrain for Terrain<'_> {
    fn approximation(&mut self, level: usize, y: u32, x: u32, log2: u8) -> Option<(f32, f32)> {
        self.reader.approximation(&mut self.cache, level, y, x, log2)
    }

    fn coarse_patch(&mut self, level: usize, y: u32, x: u32, log2: u8) -> Option<Patch> {
        self.reader.coarse_patch(&mut self.cache, level, y, x, log2)
    }

    fn max_cell_group(&mut self, y: u32, x: u32, log2: u8) -> Option<i16> {
        self.reader.max_cell_group(&mut self.cache, y, x, log2)
    }

    fn cell_present(&mut self, y: u32, x: u32) -> bool {
        self.reader.cell_present(&mut self.cache, y, x)
    }

    fn level(&self, index: usize) -> Option<SurfaceLevel> {
        let g = self.reader.geometry(index)?;
        if index > 0 && g.posting_log2 > 11 {
            return None;
        }
        let shift = g.cell_log2 - g.posting_log2;
        Some(SurfaceLevel {
            min_y: g.cell_min_i << shift,
            min_x: g.cell_min_j << shift,
            rows: u32::from(g.cell_rows) << shift,
            columns: u32::from(g.cell_cols) << shift,
            posting_log2: g.posting_log2,
            cell_log2: g.cell_log2,
            max_distance_m: if index + 1 == self.reader.level_count() {
                100_000.0
            } else {
                // Preserve geographic detail near the observer without letting a finer
                // native source multiply fine-cell traversal all the way to 25 km.
                (25_000.0 * (1u32 << g.posting_log2) as f32 / 512.0).min(100_000.0)
            },
        })
    }

    fn patch(&mut self, level: usize, y: u32, x: u32) -> Option<Patch> {
        self.reader.patch(&mut self.cache, level, y, x)
    }

    fn max_height(&mut self, level: usize, y: u32, x: u32, log2: u8) -> Option<i16> {
        self.reader.max_height(&mut self.cache, level, y, x, log2)
    }
}
