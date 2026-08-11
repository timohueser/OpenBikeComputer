//! Exact integer grid geometry shared by adapters, the OBCG emitter and the manifest.
//!
//! Everything is microdegrees and cell counts — the same fields OBCG stores — so the geometry a
//! frame is baked on, the geometry its header declares and the geometry the manifest restates
//! can never be three subtly different numbers.

use obc_formats::obcg;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridGeometry {
    pub south_lat_udeg: i32,
    pub west_lon_udeg: i32,
    pub cell_lat_udeg: u32,
    pub cell_lon_udeg: u32,
    pub width: u32,
    pub height: u32,
    /// For a **source window**: the source's nominal ground resolution in metres. For a
    /// **published frame**: [`crate::canonical::LATTICE_CELL_SIZE_M`], a constant stating the
    /// lattice. Under the mosaic a frame has no single source resolution to state, and the
    /// decision (#1242) was to remove that information rather than transport it.
    pub cell_size_m: u16,
    pub tile_edge: u16,
    pub entries_per_page: u16,
}

impl GridGeometry {
    pub fn cells(&self) -> usize {
        self.width as usize * self.height as usize
    }

    pub fn north_lat_udeg(&self) -> i64 {
        i64::from(self.south_lat_udeg) + i64::from(self.height) * i64::from(self.cell_lat_udeg)
    }

    pub fn east_lon_udeg(&self) -> i64 {
        i64::from(self.west_lon_udeg) + i64::from(self.width) * i64::from(self.cell_lon_udeg)
    }

    /// Cell-center latitude of `row` (row 0 = south) in degrees.
    pub fn center_lat_deg(&self, row: u32) -> f64 {
        (f64::from(self.south_lat_udeg) + (f64::from(row) + 0.5) * f64::from(self.cell_lat_udeg)) / 1e6
    }

    /// Cell-center longitude of `col` (col 0 = west) in degrees.
    pub fn center_lon_deg(&self, col: u32) -> f64 {
        (f64::from(self.west_lon_udeg) + (f64::from(col) + 0.5) * f64::from(self.cell_lon_udeg)) / 1e6
    }

    /// Sanity gate an adapter constant set against the OBCG format limits before any decode work.
    pub fn validate(&self) -> Result<(), String> {
        if self.width == 0
            || self.height == 0
            || self.width > obcg::MAX_GRID_DIM
            || self.height > obcg::MAX_GRID_DIM
            || u64::from(self.width) * u64::from(self.height) > obcg::MAX_GRID_CELLS
            || self.cell_lat_udeg == 0
            || self.cell_lon_udeg == 0
            || self.cell_size_m == 0
            || !self.tile_edge.is_power_of_two()
            || !(obcg::MIN_TILE_EDGE..=obcg::MAX_TILE_EDGE).contains(&self.tile_edge)
            || self.entries_per_page == 0
            || self.entries_per_page > obcg::MAX_ENTRIES_PER_PAGE
            || i64::from(self.south_lat_udeg) < -90_000_000
            || self.north_lat_udeg() > 90_000_000
            || i64::from(self.west_lon_udeg) < -180_000_000
            || self.east_lon_udeg() > 180_000_000
        {
            return Err(format!("grid geometry violates the OBCG format limits: {self:?}"));
        }
        Ok(())
    }
}
