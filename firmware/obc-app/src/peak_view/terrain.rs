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

    /// The height a rider standing here is standing **on**, which the bilinear surface is not.
    ///
    /// A lattice node can stand on a summit the posting cannot resolve, and the bilinear value
    /// between two such nodes is below both of them. An eye placed there is inside the surface:
    /// from the top of the Rigidalstock the panorama came back blocked at 19 m by the summit the
    /// rider was standing on. A rider is on the ground, and the ground under a summit is the
    /// summit, so the cell's corners decide — unless `measured`, the host's settled map-referenced
    /// altitude, is the better answer. [`super::eye_ground`] holds that rule.
    pub fn eye_ground(&mut self, lat: i32, lon: i32, measured: Option<f32>) -> Option<f32> {
        let (p, fy, fx) = self.patch_at(lat, lon)?;
        Some(super::eye_ground(bilinear(p, fy, fx), cell_top(p), measured))
    }

    /// The level 0 patch containing this position, with the position's place inside it.
    fn patch_at(&mut self, lat: i32, lon: i32) -> Option<(Patch, f32, f32)> {
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
        Some((p, fy, fx))
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
        let shift = g.cell_log2 - g.posting_log2;
        Some(SurfaceLevel {
            min_y: g.cell_min_i << shift,
            min_x: g.cell_min_j << shift,
            rows: u32::from(g.cell_rows) << shift,
            columns: u32::from(g.cell_cols) << shift,
            posting_log2: g.posting_log2,
            cell_log2: g.cell_log2,
        })
    }

    fn patch(&mut self, level: usize, y: u32, x: u32) -> Option<Patch> {
        self.reader.patch(&mut self.cache, level, y, x)
    }

    fn max_height(&mut self, level: usize, y: u32, x: u32, log2: u8) -> Option<i16> {
        self.reader.max_height(&mut self.cache, level, y, x, log2)
    }
}

/// The surface height inside a patch, `fy` north and `fx` east of its low corner.
fn bilinear(p: Patch, fy: f32, fx: f32) -> f32 {
    p.height + p.east * fx + p.north * fy + p.cross * fx * fy
}

/// The highest of a patch's four corners.
fn cell_top(p: Patch) -> f32 {
    let far = p.height + p.east + p.north + p.cross;
    p.height.max(p.height + p.east).max(p.height + p.north).max(far)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cell_offers_its_bilinear_surface_and_its_highest_corner() {
        // Rigidalstock: the summit node is lifted to 2592 m and the rider's own position
        // interpolates to 2588 m between it and the ridge below. An eye at 2588 + 2 is inside the
        // surface, and the panorama came back blocked at 19 m by the summit under the rider.
        let summit = Patch { height: 2570.0, east: 22.0, north: -14.0, cross: 14.0 };
        assert_eq!(cell_top(summit), 2592.0);
        // The far corner can be the highest one, which is the case the three-way max exists for.
        assert_eq!(cell_top(Patch { height: 2570.0, east: 4.0, north: 6.0, cross: 12.0 }), 2592.0);
        // Flat ground is unchanged, so nothing moves where the posting resolves the ground.
        assert_eq!(cell_top(Patch { height: 800.0, east: 0.0, north: 0.0, cross: 0.0 }), 800.0);
        // Half a posting into the cell the surface interpolates below every corner of it.
        assert_eq!(bilinear(summit, 0.5, 0.5), 2577.5);
    }
}
