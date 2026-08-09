//! The HRRR CONUS grid's pinned Lambert conformal conic projection (WX1 decision record).
//!
//! Same technique as [`crate::stereo`]: forward-project every output lat/lon cell centre once per
//! cycle to get a source-index map for nearest-neighbour resampling — no inverse projection, no
//! interpolation, no smoothing. The formulas are the spherical Lambert conformal conic (Snyder
//! 15-1..15-4) on the GRIB shape-of-earth-6 sphere, with a tangent cone because HRRR's two
//! standard parallels are equal.

use crate::grib::HRRR_CONUS_GRID_DEFINITION_HEX;

/// GRIB Section-3 shape of earth 6: a sphere of radius 6,371,229 m.
const EARTH_RADIUS_M: f64 = 6_371_229.0;
/// Latin1 == Latin2 == LaD == 38.5 degrees: one tangent standard parallel.
const STANDARD_PARALLEL_DEG: f64 = 38.5;
/// LoV, the projection's central meridian (262.5 E).
const CENTRAL_MERIDIAN_DEG: f64 = -97.5;

/// The pinned native raster: 1,799 x 1,059 cells of 3 km, first point at the south-west corner,
/// scanning +i east and +j north — the same row order OBCG uses, so no reindexing.
pub const NATIVE_COLS: u32 = 1_799;
pub const NATIVE_ROWS: u32 = 1_059;
const CELL_M: f64 = 3_000.0;
const FIRST_POINT_LAT_DEG: f64 = 21.138_123;
const FIRST_POINT_LON_DEG: f64 = -122.719_528;

/// The Section-3 bytes this projection is derived from; an adapter pins the two together so a
/// silent upstream re-registration can never keep using these constants.
pub const GRID_DEFINITION_HEX: &str = HRRR_CONUS_GRID_DEFINITION_HEX;

fn cone_constant() -> f64 {
    STANDARD_PARALLEL_DEG.to_radians().sin()
}

/// Snyder's `F`: the scaled polar distance factor of the tangent cone.
fn scale_factor() -> f64 {
    let phi1 = STANDARD_PARALLEL_DEG.to_radians();
    let n = cone_constant();
    phi1.cos() * (core::f64::consts::FRAC_PI_4 + phi1 / 2.0).tan().powf(n) / n
}

fn rho(lat_deg: f64) -> f64 {
    let n = cone_constant();
    EARTH_RADIUS_M * scale_factor() / (core::f64::consts::FRAC_PI_4 + lat_deg.to_radians() / 2.0).tan().powf(n)
}

/// Forward-project a geographic coordinate onto the projection's metres, origin on the central
/// meridian at the standard parallel (Snyder's `x`, `y`).
pub fn forward(lat_deg: f64, lon_deg: f64) -> (f64, f64) {
    let n = cone_constant();
    // Longitudes are normalized into (-180, 180] before scaling by the cone constant, so a
    // coordinate east of the antimeridian cannot wrap onto the domain.
    let mut delta_lon = lon_deg - CENTRAL_MERIDIAN_DEG;
    delta_lon -= 360.0 * ((delta_lon + 180.0) / 360.0).floor();
    let theta = n * delta_lon.to_radians();
    let radius = rho(lat_deg);
    (radius * theta.sin(), rho(STANDARD_PARALLEL_DEG) - radius * theta.cos())
}

/// Nearest source cell for a geographic coordinate, or `None` outside the native raster. The
/// returned index is row-major over the HRRR raster with row 0 at the **south** edge.
pub fn native_index(lat_deg: f64, lon_deg: f64) -> Option<usize> {
    if !(-90.0..=90.0).contains(&lat_deg) {
        return None;
    }
    let (origin_x, origin_y) = forward(FIRST_POINT_LAT_DEG, FIRST_POINT_LON_DEG);
    let (x, y) = forward(lat_deg, lon_deg);
    // Grid points are cell centres, so the cell owning a coordinate is the nearest point: half a
    // cell of the first/last point is inside the raster, exactly as `stereo` treats DWD's edges.
    let col = ((x - origin_x) / CELL_M + 0.5).floor();
    let row = ((y - origin_y) / CELL_M + 0.5).floor();
    if col < 0.0 || row < 0.0 || col >= f64::from(NATIVE_COLS) || row >= f64::from(NATIVE_ROWS) {
        return None;
    }
    Some(row as usize * NATIVE_COLS as usize + col as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The published HRRR domain corners must land on the raster's corner cell centres. These
    /// four coordinates are NOAA's documented CONUS domain corners; agreement to well under a
    /// cell pins the cone constant, the sphere radius and the first-point registration together.
    #[test]
    fn hrrr_domain_corners_pin_the_projection() {
        let corners = [
            (21.138_123, -122.719_528, 0u32, 0u32),                        // SW = the first point
            (21.140_547, -72.289_718, NATIVE_COLS - 1, 0),                 // SE
            (47.838_623, -134.095_480, 0, NATIVE_ROWS - 1),                // NW
            (47.842_195, -60.917_193, NATIVE_COLS - 1, NATIVE_ROWS - 1),   // NE
        ];
        let (origin_x, origin_y) = forward(FIRST_POINT_LAT_DEG, FIRST_POINT_LON_DEG);
        for (lat, lon, col, row) in corners {
            let (x, y) = forward(lat, lon);
            let dx = (x - origin_x) / CELL_M - f64::from(col);
            let dy = (y - origin_y) / CELL_M - f64::from(row);
            assert!(dx.abs() < 0.01 && dy.abs() < 0.01, "corner {lat},{lon} lands at +({dx},{dy}) cells");
            assert_eq!(native_index(lat, lon), Some(row as usize * NATIVE_COLS as usize + col as usize));
        }
    }

    #[test]
    fn the_central_meridian_is_straight_and_the_domain_is_bounded() {
        // On LoV the projection has no easting: the cone is symmetric about it.
        let (x, _) = forward(40.0, CENTRAL_MERIDIAN_DEG);
        assert!(x.abs() < 1e-6, "central meridian easting {x}");
        // Outside the domain there is no clamping onto the raster.
        assert_eq!(native_index(60.0, -97.5), None, "north of the domain");
        assert_eq!(native_index(15.0, -97.5), None, "south of the domain");
        assert_eq!(native_index(40.0, -20.0), None, "east of the domain");
        assert_eq!(native_index(40.0, 140.0), None, "west of the domain, past the antimeridian");
        assert_eq!(native_index(f64::NAN, -97.5), None);
    }

    /// A handful of interior cities resolve to plausible interior cells, and the mapping is
    /// monotone in both axes near them (a mirrored or transposed raster would fail this).
    #[test]
    fn interior_coordinates_are_monotone() {
        let denver = native_index(39.74, -104.99).expect("Denver is inside CONUS");
        let denver_east = native_index(39.74, -104.90).expect("east of Denver");
        let denver_north = native_index(39.83, -104.99).expect("north of Denver");
        assert!(denver_east > denver, "eastward must increase the column");
        assert!(denver_north >= denver + NATIVE_COLS as usize, "northward must increase the row");
        assert!(native_index(25.79, -80.22).is_some(), "Miami");
        assert!(native_index(47.61, -122.33).is_some(), "Seattle");
    }
}
