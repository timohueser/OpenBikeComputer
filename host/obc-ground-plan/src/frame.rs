//! The integer frame: Web Mercator in 2^-34 of the square, the zoom-16 cells and their input margin.
//!
//! Every position is an integer. Tile units become frame units by one exact shift; metres come from
//! `metres_per_unit` at the point and enter a position only through one rounding.

use core::f64::consts::PI;

/// A frame unit is 2^-34 of the Web Mercator square: 1/16 of a world unit, about 1.6 mm at 47° N.
pub const FRAME_BITS: u32 = 34;
const SQUARE: f64 = (1u64 << FRAME_BITS) as f64;
/// The bake's metres (the format's along, grades and widening are in them).
pub const EQUATOR_M: f64 = 40075016.686;
pub const CELL_ZOOM: u32 = 16;
pub const CELL_UNITS: i64 = 1 << (FRAME_BITS - CELL_ZOOM);
/// The input margin about a cell.
pub const MARGIN_M: f64 = 192.0;
/// The longest chain of rules: 1 m of the margin is left for the Mercator scale over a cell and the
/// rounding to frame units.
pub const CHAIN_MAX_M: f64 = MARGIN_M - 1.0;
/// A curve leaves its chord by up to this much, so the curve within `MARGIN_M` lies on segments that
/// meet the cell grown by `margin()`.
const CHORD_M: f64 = 2.0;
/// The zoom of the feature tiles that a cell reads.
pub const FEATURE_ZOOM: u32 = 13;
const FEATURE_UNITS: i64 = 1 << (FRAME_BITS - FEATURE_ZOOM);

/// A point of the frame: x east, y south, from the north-west corner of the square.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GlobalPoint {
    pub x: i64,
    pub y: i64,
}

/// Frame units from a cell's origin. A cell and its margin span about 4 × 10^5 units.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CellPoint {
    pub x: i32,
    pub y: i32,
}

/// The zoom-16 tile that a cell is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CellKey {
    pub x: u32,
    pub y: u32,
}

impl CellKey {
    /// The cell that holds a point: a point on a cell line belongs to the cell east or south of it.
    pub fn of(p: GlobalPoint) -> CellKey {
        let shift = FRAME_BITS - CELL_ZOOM;
        CellKey { x: (p.x >> shift) as u32, y: (p.y >> shift) as u32 }
    }

    pub fn origin(self) -> GlobalPoint {
        GlobalPoint { x: i64::from(self.x) * CELL_UNITS, y: i64::from(self.y) * CELL_UNITS }
    }

    pub fn contains(self, p: GlobalPoint) -> bool {
        let o = self.origin();
        (o.x..o.x + CELL_UNITS).contains(&p.x) && (o.y..o.y + CELL_UNITS).contains(&p.y)
    }

    /// The margin in frame units: `MARGIN_M` and the chord's 2 m at the cell edge nearer the pole, so
    /// at least that on the ground at every point of the cell.
    pub fn margin(self) -> i64 {
        let o = self.origin();
        let pole = if o.y < 1 << (FRAME_BITS - 1) { o.y } else { o.y + CELL_UNITS };
        ((MARGIN_M + CHORD_M) / metres_per_unit(pole)).floor() as i64
    }

    /// The closed square [x0, y0, x1, y1] of the cell grown by its margin.
    pub fn grown(self) -> [i64; 4] {
        let (o, m) = (self.origin(), self.margin());
        [o.x - m, o.y - m, o.x + CELL_UNITS + m, o.y + CELL_UNITS + m]
    }

    /// The zoom-13 tiles (column, row) that the cell grown by its margin meets, by row, then column:
    /// one to four.
    pub fn feature_tiles(self) -> Vec<(u32, u32)> {
        let [x0, y0, x1, y1] = self.grown();
        let last = (1i64 << FEATURE_ZOOM) - 1;
        let span = |lo: i64, hi: i64| (lo / FEATURE_UNITS).max(0)..=(hi / FEATURE_UNITS).min(last);
        span(y0.max(0), y1).flat_map(|row| span(x0.max(0), x1).map(move |column| (column as u32, row as u32))).collect()
    }

    /// The point in units from the origin, or None past `i32`.
    pub fn local(self, p: GlobalPoint) -> Option<CellPoint> {
        let o = self.origin();
        Some(CellPoint { x: i32::try_from(p.x - o.x).ok()?, y: i32::try_from(p.y - o.y).ok()? })
    }

    pub fn global(self, p: CellPoint) -> GlobalPoint {
        let o = self.origin();
        GlobalPoint { x: o.x + i64::from(p.x), y: o.y + i64::from(p.y) }
    }
}

/// Metres per frame unit at frame row y: the Mercator scale. `1 / cosh t` is the bake's
/// `cos(atan(sinh t))`.
pub fn metres_per_unit(y: i64) -> f64 {
    metres_per_unit_at(y as f64)
}

/// `metres_per_unit` at a row between two frame units.
pub fn metres_per_unit_at(y: f64) -> f64 {
    EQUATOR_M / SQUARE / libm::cosh(PI * (1.0 - 2.0 * y / SQUARE))
}

/// The point at tile units (quarters of them for `fine`) of tile (column, row) at zoom z. Exact.
pub fn tile_point(z: u32, column: u32, row: u32, fine: bool, x: i64, y: i64) -> GlobalPoint {
    assert!(z <= 18, "a tile of zoom {z} is finer than the frame");
    let k = if fine { 16 } else { 14 };
    let s = FRAME_BITS - z - k;
    GlobalPoint { x: ((i64::from(column) << k) + x) << s, y: ((i64::from(row) << k) + y) << s }
}

/// The point at a longitude and a latitude in degrees, rounded half to even. For tools and tests.
pub fn from_lon_lat(lon: f64, lat: f64) -> GlobalPoint {
    let x = (lon + 180.0) / 360.0;
    let y = (1.0 - libm::asinh(libm::tan(lat.to_radians())) / PI) / 2.0;
    GlobalPoint { x: (x * SQUARE).round_ties_even() as i64, y: (y * SQUARE).round_ties_even() as i64 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell_at(lat: f64) -> CellKey {
        CellKey::of(from_lon_lat(8.4, lat))
    }

    #[test]
    fn tile_units_and_quarters_are_exact_and_reversible() {
        let (column, row) = (4286, 2888);
        // Both ends of the buffer, about 40 m (200 units) past the square, and a far cut end.
        for (x, y) in [(-210, 16594), (0, 0), (16383, 8192), (-90_000, 400_000)] {
            let p = tile_point(13, column, row, false, x, y);
            let q = tile_point(13, column, row, true, 4 * x, 4 * y);
            assert_eq!(p, q);
            let unit = 1 << 7;
            assert_eq!(((p.x >> 7) - (i64::from(column) << 14), (p.y >> 7) - (i64::from(row) << 14)), (x, y));
            assert_eq!((p.x % unit, p.y % unit), (0, 0));
        }
        let quarter = tile_point(13, column, row, true, 1, 3);
        assert_eq!(quarter.x - tile_point(13, column, row, false, 0, 0).x, 32);
    }

    #[test]
    fn a_point_on_a_cell_line_is_in_one_cell() {
        let key = cell_at(46.8);
        let line = GlobalPoint { x: key.origin().x + CELL_UNITS, y: key.origin().y };
        assert!(!key.contains(line));
        assert_eq!(CellKey::of(line), CellKey { x: key.x + 1, y: key.y });
        assert!(CellKey::of(line).contains(line));
        assert!(key.contains(key.origin()));
        let local = key.local(GlobalPoint { x: line.x - 1, y: line.y + 5 }).unwrap();
        assert_eq!(key.global(local), GlobalPoint { x: line.x - 1, y: line.y + 5 });
        assert!(key.local(GlobalPoint { x: 0, y: 0 }).is_none());
    }

    #[test]
    fn the_scale_is_the_bakes_and_the_margin_holds_194_m() {
        for lat in [0.0, 47.0, 70.0] {
            let y = from_lon_lat(8.4, lat).y;
            let t = PI * (1.0 - 2.0 * y as f64 / SQUARE);
            let bake = EQUATOR_M * libm::cos(libm::atan(libm::sinh(t))) / SQUARE;
            assert!((metres_per_unit(y) / bake - 1.0).abs() < 1e-12, "{lat}");
            let key = cell_at(lat);
            let pole = if key.origin().y < 1 << 33 { key.origin().y } else { key.origin().y + CELL_UNITS };
            let metres = key.margin() as f64 * metres_per_unit(pole);
            assert!(metres <= 194.0 && metres > 194.0 - metres_per_unit(pole), "{lat}: {metres}");
        }
    }

    #[test]
    fn a_cell_reads_one_two_or_four_feature_tiles() {
        let tile = |column: i64, row: i64| GlobalPoint { x: column * FEATURE_UNITS, y: row * FEATURE_UNITS };
        let middle = tile(4286, 2888);
        let inside = CellKey::of(GlobalPoint { x: middle.x + FEATURE_UNITS / 2, y: middle.y + FEATURE_UNITS / 2 });
        assert_eq!(inside.feature_tiles(), [(4286, 2888)]);
        let west_edge = CellKey::of(GlobalPoint { x: middle.x, y: middle.y + FEATURE_UNITS / 2 });
        assert_eq!(west_edge.feature_tiles(), [(4285, 2888), (4286, 2888)]);
        let corner = CellKey::of(middle);
        assert_eq!(corner.feature_tiles(), [(4285, 2887), (4286, 2887), (4285, 2888), (4286, 2888)]);
    }
}
