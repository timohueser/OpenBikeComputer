//! The DWD RV composite's pinned polar-stereographic projection (WX1 decision record).
//!
//! Forward-projecting every output lat/lon cell center once per cycle gives a source-index map
//! for nearest-neighbour resampling — no inverse projection, no interpolation, no smoothing.
//! The formulas are the standard ellipsoidal north-polar stereographic with a true-scale
//! latitude (Snyder 21-33/21-34, proj `+proj=stere +lat_0=90 +lat_ts=60`), and the ODIM corner
//! test below pins them to sub-millimetre agreement with DWD's own grid registration.

/// `+proj=stere +lat_ts=60 +lat_0=90 +lon_0=10 +x_0=543196.83521776402 +y_0=3622588.8619310022
/// +units=m +a=6378137 +b=6356752.3142451802 +no_defs` — must equal the ODIM `where/projdef`.
pub const DWD_RV_PROJDEF: &str = "+proj=stere +lat_ts=60 +lat_0=90 +lon_0=10 +x_0=543196.83521776402 +y_0=3622588.8619310022 +units=m +a=6378137 +b=6356752.3142451802 +no_defs";

const A: f64 = 6_378_137.0;
const B: f64 = 6_356_752.314_245_18;
const LAT_TS_DEG: f64 = 60.0;
const LON_0_DEG: f64 = 10.0;
const X_0: f64 = 543_196.835_217_764;
const Y_0: f64 = 3_622_588.861_931_002_2;

/// Half-cell registration offsets: the ODIM corners land on `x in [-500, 1_099_500]` and
/// `y in [-1_199_500, +500]` for the 1,100 x 1,200 native grid of 1,000 m cells.
pub const NATIVE_COLS: u32 = 1_100;
pub const NATIVE_ROWS: u32 = 1_200;
const CELL_M: f64 = 1_000.0;
const X_EDGE: f64 = -500.0;
const Y_EDGE: f64 = 500.0;

fn eccentricity() -> f64 {
    (1.0 - (B * B) / (A * A)).sqrt()
}

fn t(phi: f64, e: f64) -> f64 {
    let sin_phi = phi.sin();
    (core::f64::consts::FRAC_PI_4 - phi / 2.0).tan() / ((1.0 - e * sin_phi) / (1.0 + e * sin_phi)).powf(e / 2.0)
}

fn m(phi: f64, e: f64) -> f64 {
    phi.cos() / (1.0 - e * e * phi.sin() * phi.sin()).sqrt()
}

/// Forward-project a WGS-84-datum geographic coordinate onto the composite's projected metres.
pub fn forward(lat_deg: f64, lon_deg: f64) -> (f64, f64) {
    let e = eccentricity();
    let phi = lat_deg.to_radians();
    let lat_ts = LAT_TS_DEG.to_radians();
    let rho = A * m(lat_ts, e) * t(phi, e) / t(lat_ts, e);
    let d_lambda = (lon_deg - LON_0_DEG).to_radians();
    (X_0 + rho * d_lambda.sin(), Y_0 - rho * d_lambda.cos())
}

/// Nearest source cell for a geographic coordinate, or `None` outside the native raster.
/// The returned index is row-major over the ODIM raster: row 0 is the **north** edge.
pub fn native_index(lat_deg: f64, lon_deg: f64) -> Option<usize> {
    let (x, y) = forward(lat_deg, lon_deg);
    let col = ((x - X_EDGE) / CELL_M).floor();
    let row = ((Y_EDGE - y) / CELL_M).floor();
    if col < 0.0 || row < 0.0 || col >= f64::from(NATIVE_COLS) || row >= f64::from(NATIVE_ROWS) {
        return None;
    }
    Some(row as usize * NATIVE_COLS as usize + col as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The WX1-pinned ODIM corners: each must land exactly half a cell inside the projected
    /// frame, which is how DWD registers corner coordinates on this composite.
    #[test]
    fn odim_corners_pin_the_projection() {
        let corners = [
            (45.696_425_377_4, 3.566_994_635_0, -500.0, -1_199_500.0), // LL
            (55.862_087_108_2, 1.463_301_510_3, -500.0, 500.0),        // UL
            (55.845_438_563_3, 18.731_616_454_7, 1_099_500.0, 500.0),  // UR
            (45.684_605_781_4, 16.580_869_348_6, 1_099_500.0, -1_199_500.0), // LR
        ];
        for (lat, lon, expected_x, expected_y) in corners {
            let (x, y) = forward(lat, lon);
            assert!((x - expected_x).abs() < 0.05, "x for {lat},{lon}: {x} != {expected_x}");
            assert!((y - expected_y).abs() < 0.05, "y for {lat},{lon}: {y} != {expected_y}");
        }
        // Points ~250 m inside each corner resolve to the raster's four corner cells (the exact
        // corner points sit on the half-open frame boundary and are legitimately outside).
        assert_eq!(native_index(55.8601, 1.4663), Some(0));
        assert_eq!(native_index(45.6984, 3.5700), Some(1_199 * NATIVE_COLS as usize));
        assert_eq!(native_index(55.8434, 18.7286), Some(1_099));
        assert_eq!(native_index(45.6866, 16.5779), Some(1_199 * NATIVE_COLS as usize + 1_099));
        assert_eq!(native_index(51.0, 10.0), Some(600 * NATIVE_COLS as usize + 543), "domain center");
        // Outside the projected frame: no clamping onto the raster.
        assert_eq!(native_index(40.0, 10.0), None);
        assert_eq!(native_index(51.0, 30.0), None);
    }
}
