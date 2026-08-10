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

    /// Can a frame on `self`'s lattice be laid onto any window a frame on `coarser`'s lattice
    /// defines?
    ///
    /// This is the assembly contract for a **composed** product. A client builds one bundle by
    /// choosing the coarsest frame's crop as the common window and then tiling every other frame
    /// onto it, refusing any frame the window is not a whole number of cells of, aligned to that
    /// frame's own origin (`obc-wx-client`'s `bundle::rain_frame`, mirroring the phone). Crops
    /// only ever start on their product's lattice and span a whole number of its cells, so that
    /// per-corridor test succeeds for *every* corridor exactly when the coarse strides are
    /// integer multiples of the fine ones and the two origins are congruent modulo the fine
    /// strides. Anything less is a lattice that drops frames for some corridors and not others.
    ///
    /// Getting this wrong is not a rounding artefact — the refused frame vanishes from the
    /// timeline. The US product shipped a 27,000 x 34,000 forecast lattice over a 10,000 x 10,000
    /// observation, and every rider in CONUS lost the radar frame.
    pub fn nests_under(&self, coarser: &GridGeometry) -> bool {
        coarser.cell_lat_udeg.is_multiple_of(self.cell_lat_udeg)
            && coarser.cell_lon_udeg.is_multiple_of(self.cell_lon_udeg)
            && (i64::from(coarser.south_lat_udeg) - i64::from(self.south_lat_udeg))
                .rem_euclid(i64::from(self.cell_lat_udeg))
                == 0
            && (i64::from(coarser.west_lon_udeg) - i64::from(self.west_lon_udeg))
                .rem_euclid(i64::from(self.cell_lon_udeg))
                == 0
    }

    /// Cell area in square microdegrees — the key a client picks the common window by, so the
    /// nesting invariant has to agree with it about which lattice is "coarsest".
    pub fn cell_area(&self) -> u64 {
        u64::from(self.cell_lat_udeg) * u64::from(self.cell_lon_udeg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FINE: GridGeometry = GridGeometry {
        south_lat_udeg: 20_000_000,
        west_lon_udeg: -130_000_000,
        cell_lat_udeg: 10_000,
        cell_lon_udeg: 10_000,
        width: 7_000,
        height: 3_500,
        cell_size_m: 1_000,
        tile_edge: 64,
        entries_per_page: 512,
    };

    fn coarse(cell_lat: u32, cell_lon: u32, south: i32, west: i32) -> GridGeometry {
        GridGeometry {
            south_lat_udeg: south,
            west_lon_udeg: west,
            cell_lat_udeg: cell_lat,
            cell_lon_udeg: cell_lon,
            width: 100,
            height: 100,
            cell_size_m: 3_000,
            tile_edge: 32,
            entries_per_page: 512,
        }
    }

    #[test]
    fn integer_multiples_with_congruent_origins_nest() {
        assert!(FINE.nests_under(&coarse(30_000, 30_000, 21_100_000, -134_100_000)));
    }

    /// The exact lattice the US product shipped before this was a contract: neither 27,000 nor
    /// 34,000 is a multiple of 10,000, so the observation frame could not tile the window the
    /// forecast frames defined.
    #[test]
    fn the_shipped_hrrr_lattice_did_not_nest() {
        assert!(!FINE.nests_under(&coarse(27_000, 34_000, 21_100_000, -134_100_000)));
    }

    /// Multiples alone are not enough: an origin off the fine lattice shifts every window by a
    /// fraction of a cell.
    #[test]
    fn an_incongruent_origin_does_not_nest() {
        assert!(!FINE.nests_under(&coarse(30_000, 30_000, 21_105_000, -134_100_000)));
        assert!(!FINE.nests_under(&coarse(30_000, 30_000, 21_100_000, -134_103_000)));
    }
}
