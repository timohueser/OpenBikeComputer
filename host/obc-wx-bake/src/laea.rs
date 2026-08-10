//! The EUMETNET OPERA composite grid's Lambert azimuthal equal-area projection (WXR6, #1245).
//!
//! Same technique as [`crate::stereo`] and [`crate::lcc`]: forward-project every output lat/lon
//! cell centre to get the native cell it samples — no inverse projection, no interpolation, no
//! smoothing. The formulas are Snyder's **ellipsoidal oblique** Lambert azimuthal equal-area
//! (24-2 … 24-6 with the authalic latitude of 3-11/3-12), on the WGS-84 ellipsoid; OPERA's grid
//! is oblique (centred on 55 N, 10 E) and its own `projdef` names an ellipsoid, so the spherical
//! form is not good enough — it is kilometres wrong at the domain edges.
//!
//! Both OPERA products share this projection exactly; only the pixel size and the raster
//! registration differ, and those live with the raster in [`crate::source::opera`].

use std::sync::OnceLock;

/// The ODIM `where/projdef` of both OPERA composites, byte for byte. An adapter pins the
/// constants below to it, so a silent re-registration upstream cannot keep reusing them.
pub const OPERA_PROJDEF: &str =
    "+proj=laea +lat_0=55.0 +lon_0=10.0 +x_0=1950000.0 +y_0=-2100000.0 +units=m +ellps=WGS84";

/// WGS-84, as `GeoDoubleParams` states it in the COG.
pub const SEMI_MAJOR_M: f64 = 6_378_137.0;
pub const INVERSE_FLATTENING: f64 = 298.257_223_563;
pub const LAT_0_DEG: f64 = 55.0;
pub const LON_0_DEG: f64 = 10.0;
/// The projection's false origin: model coordinates are projected metres plus these.
pub const FALSE_EASTING_M: f64 = 1_950_000.0;
pub const FALSE_NORTHING_M: f64 = -2_100_000.0;

struct Params {
    e: f64,
    e2: f64,
    q_p: f64,
    r_q: f64,
    sin_beta_0: f64,
    cos_beta_0: f64,
    d: f64,
}

fn params() -> &'static Params {
    static PARAMS: OnceLock<Params> = OnceLock::new();
    PARAMS.get_or_init(|| {
        let f = 1.0 / INVERSE_FLATTENING;
        let e2 = f * (2.0 - f);
        let e = e2.sqrt();
        let q = |phi: f64| authalic_q(phi, e, e2);
        let q_p = q(core::f64::consts::FRAC_PI_2);
        let r_q = SEMI_MAJOR_M * (q_p / 2.0).sqrt();
        let phi_0 = LAT_0_DEG.to_radians();
        let beta_0 = (q(phi_0) / q_p).asin();
        let m_0 = phi_0.cos() / (1.0 - e2 * phi_0.sin() * phi_0.sin()).sqrt();
        Params {
            e,
            e2,
            q_p,
            r_q,
            sin_beta_0: beta_0.sin(),
            cos_beta_0: beta_0.cos(),
            d: SEMI_MAJOR_M * m_0 / (r_q * beta_0.cos()),
        }
    })
}

/// Snyder 3-12: the authalic-area function `q(phi)`.
fn authalic_q(phi: f64, e: f64, e2: f64) -> f64 {
    let sin_phi = phi.sin();
    (1.0 - e2)
        * (sin_phi / (1.0 - e2 * sin_phi * sin_phi)
            - (1.0 / (2.0 * e)) * ((1.0 - e * sin_phi) / (1.0 + e * sin_phi)).ln())
}

/// The part of the projection that depends only on latitude.
///
/// A bake evaluates the forward projection once per output cell — tens of millions of times — and
/// a fixed lat/lon window walks one latitude per row, so hoisting the authalic latitude out of
/// the column loop removes a `ln` and an `asin` from the inner loop.
#[derive(Debug, Clone, Copy)]
pub struct Row {
    sin_beta: f64,
    cos_beta: f64,
}

pub fn row(lat_deg: f64) -> Option<Row> {
    if !lat_deg.is_finite() || !(-90.0..=90.0).contains(&lat_deg) {
        return None;
    }
    let p = params();
    let ratio = authalic_q(lat_deg.to_radians(), p.e, p.e2) / p.q_p;
    let beta = ratio.clamp(-1.0, 1.0).asin();
    Some(Row { sin_beta: beta.sin(), cos_beta: beta.cos() })
}

/// Project a longitude on an already-prepared latitude row. Returns **projected** metres, without
/// the false origin. `None` at the projection's antipode, where the equal-area azimuthal mapping
/// is singular.
pub fn forward_in_row(row: &Row, lon_deg: f64) -> Option<(f64, f64)> {
    if !lon_deg.is_finite() {
        return None;
    }
    let p = params();
    // Normalize into (-180, 180] so a coordinate east of the antimeridian cannot wrap onto the
    // domain, exactly as `lcc` does.
    let mut delta_lon = lon_deg - LON_0_DEG;
    delta_lon -= 360.0 * ((delta_lon + 180.0) / 360.0).floor();
    let lambda = delta_lon.to_radians();
    let denominator = 1.0 + p.sin_beta_0 * row.sin_beta + p.cos_beta_0 * row.cos_beta * lambda.cos();
    if denominator <= 1e-12 {
        return None;
    }
    let b = p.r_q * (2.0 / denominator).sqrt();
    let x = b * p.d * row.cos_beta * lambda.sin();
    let y = (b / p.d) * (p.cos_beta_0 * row.sin_beta - p.sin_beta_0 * row.cos_beta * lambda.cos());
    Some((x, y))
}

/// Forward-project one geographic coordinate onto projected metres (no false origin).
pub fn forward(lat_deg: f64, lon_deg: f64) -> Option<(f64, f64)> {
    forward_in_row(&row(lat_deg)?, lon_deg)
}

/// Forward-project onto the **model** coordinates the COG's `ModelTiepoint` is stated in: the
/// projected metres plus OPERA's false origin.
pub fn forward_model(lat_deg: f64, lon_deg: f64) -> Option<(f64, f64)> {
    forward(lat_deg, lon_deg).map(|(x, y)| (x + FALSE_EASTING_M, y + FALSE_NORTHING_M))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The projection origin is exactly the false origin, and the false origin is what puts the
    /// composite's north-west corner at model (0, 0): `x_0 = 1,950,000` and `y_0 = -2,100,000`
    /// are precisely the distances from (55 N, 10 E) to that corner. So (55 N, 10 E) sits on the
    /// **corner shared by pixels (1949, 2099), (1950, 2099), (1949, 2100) and (1950, 2100)** of
    /// the 1 km raster, at fractional pixel (1950.0, 2100.0).
    ///
    /// #1245 recorded (1950.5, 2100.5), which is what GDAL prints — and which is true of the
    /// COG's `ModelTiepoint` rather than of the grid. Both are asserted below, because the
    /// difference between them *is* the half-pixel the tiepoint carries
    /// ([`crate::source::opera::Contract::verify`] requires it to be exactly that).
    #[test]
    fn the_projection_origin_lands_on_the_grid_corner_and_explains_the_tiepoint() {
        let (x, y) = forward(LAT_0_DEG, LON_0_DEG).expect("the origin projects");
        assert!(x.abs() < 1e-9 && y.abs() < 1e-9, "origin at ({x}, {y})");
        let (model_x, model_y) = forward_model(LAT_0_DEG, LON_0_DEG).expect("the origin projects");
        assert_eq!((model_x, model_y), (FALSE_EASTING_M, FALSE_NORTHING_M));

        // The grid: upper-left corner of pixel (0, 0) at model (0, 0).
        assert!((model_x / 1_000.0 - 1_950.0).abs() < 1e-9, "col {}", model_x / 1_000.0);
        assert!((-model_y / 1_000.0 - 2_100.0).abs() < 1e-9, "row {}", -model_y / 1_000.0);

        // What GDAL prints, and why: the tiepoint is that corner minus half a pixel, so every
        // fractional pixel coordinate read off the file is half a cell larger.
        let (tiepoint_x, tiepoint_y): (f64, f64) = (-500.000_271_433_265_9, 499.999_912_387_225_8);
        assert!((tiepoint_x + 500.0).abs() < 0.001 && (tiepoint_y - 500.0).abs() < 0.001);
        assert!(((model_x - tiepoint_x) / 1_000.0 - 1_950.5).abs() < 1e-6);
        assert!(((tiepoint_y - model_y) / 1_000.0 - 2_100.5).abs() < 1e-6);
    }

    /// OPERA's ODIM `where` attributes give the composite's **outer** corners, and all four of
    /// them are asserted here because that is what makes this test self-defending against the
    /// mistake that was actually made (#1267 review round 1).
    ///
    /// `LL` and `UR` alone are not enough: read as corner-*pixel centres* they stay numerically
    /// plausible, which is exactly how a half-pixel error shipped. Two things break that reading.
    /// First the arithmetic — `LL` to `UR` spans exactly 3,800,000 x 4,400,000 m, which is exactly
    /// 3,800 x 4,400 cells of the `xscale = 1000.0` OPERA also states, and centres would need a
    /// 3,801 x 4,401 raster to span the same distance. Second, and decisively, **`UL` names the
    /// grid's north-west corner directly**: it projects to model (0, 0), the corner the adapters
    /// pin, to within 0.09 mm. Under the discarded registration that corner sits at model
    /// (-500, +500) and `UL_lat` would be wrong by ~4.5e-3 degrees — six orders of magnitude out.
    /// There is no reading of these four numbers left in which the grid is anywhere else.
    ///
    /// PROJ itself produced these lat/lons, so reproducing them to well under a millimetre also
    /// pins this implementation against the reference one — the ellipsoidal form, the authalic
    /// latitude, `R_q`, `D` and the oblique rotation all at once. The spherical approximation
    /// misses these corners by kilometres.
    #[test]
    fn the_odim_corner_coordinates_pin_the_projection_and_the_registration() {
        // The `where` group of `OPERA@20260810T0000@0@DBZH.h5`, and the model coordinates of the
        // grid corner each one names. The pinned grid is model (0, 0) … (3,800,000, -4,400,000).
        let corners = [
            ("UL", 67.022_832_762_437_2, -39.535_786_412_503_4, 0.0, 0.0),
            ("UR", 67.621_037_107_163_1, 57.811_964_750_149_9, 3_800_000.0, 0.0),
            ("LL", 31.746_215_318_267_5, -10.434_576_838_640_4, 0.0, -4_400_000.0),
            ("LR", 31.987_650_276_733_0, 29.421_038_635_578_0, 3_800_000.0, -4_400_000.0),
        ];
        for (name, lat, lon, expected_x, expected_y) in corners {
            let (x, y) = forward_model(lat, lon).expect("a corner projects");
            assert!((x - expected_x).abs() < 0.002, "{name} model x: {x} != {expected_x}");
            assert!((y - expected_y).abs() < 0.002, "{name} model y: {y} != {expected_y}");
        }
        // The span is a whole number of `xscale`/`yscale` cells, so these are edges, not centres.
        let (ll_x, ll_y) = forward(31.746_215_318_267_5, -10.434_576_838_640_4).expect("LL");
        let (ur_x, ur_y) = forward(67.621_037_107_163_1, 57.811_964_750_149_9).expect("UR");
        assert!(((ur_x - ll_x) / 1_000.0 - 3_800.0).abs() < 1e-5, "x span {}", (ur_x - ll_x) / 1_000.0);
        assert!(((ur_y - ll_y) / 1_000.0 - 4_400.0).abs() < 1e-5, "y span {}", (ur_y - ll_y) / 1_000.0);
    }

    /// Equal-area, oblique and centred: the mapping is monotone in both axes near the origin,
    /// the central meridian has no easting, and degenerate inputs are refused rather than
    /// clamped onto the domain.
    #[test]
    fn the_mapping_is_oriented_and_degenerate_inputs_are_refused() {
        let (x, _) = forward(40.0, LON_0_DEG).expect("on the central meridian");
        assert!(x.abs() < 1e-6, "central meridian easting {x}");
        let (origin_x, origin_y) = forward(55.0, 10.0).expect("origin");
        let (east_x, _) = forward(55.0, 12.0).expect("east");
        let (_, north_y) = forward(57.0, 10.0).expect("north");
        assert!(east_x > origin_x, "east must increase x");
        assert!(north_y > origin_y, "north must increase y");
        assert!(row(f64::NAN).is_none());
        assert!(row(91.0).is_none());
        assert!(forward(50.0, f64::INFINITY).is_none());
        // The antipode of (55 N, 10 E) is (55 S, 170 W): the azimuthal mapping is singular there.
        assert!(forward(-55.0, -170.0).is_none());
    }

    /// The defining property, checked against the closed form rather than against another
    /// projection: a lat/lon cell's projected area must equal its area on the ellipsoid.
    ///
    /// This is what a corner test cannot catch. Corner agreement can survive a mis-derived `R_q`
    /// or an authalic latitude replaced by the geodetic one, because a false origin and a scale
    /// error absorb each other at two points; area cannot be faked anywhere. The ellipsoidal
    /// zone area over `dlon` is `(dlon/2) a^2 [q(phi2) - q(phi1)]`, from the same `q` the
    /// projection uses, and the residual below is the shoelace's own chord error on a
    /// 0.1-degree cell (measured at 4-7 x 10^-7).
    #[test]
    fn a_cell_projects_to_its_own_geodetic_area() {
        let p = params();
        const CELL_DEG: f64 = 0.1;
        for (lat, lon) in [(35.0, -20.0), (50.0, 10.0), (70.0, 30.0), (55.0, 10.0)] {
            let corners = [(lat, lon), (lat + CELL_DEG, lon), (lat + CELL_DEG, lon + CELL_DEG), (lat, lon + CELL_DEG)];
            let points: Vec<(f64, f64)> = corners.iter().map(|(a, o)| forward(*a, *o).expect("cell")).collect();
            let mut projected = 0.0;
            for index in 0..points.len() {
                let (x1, y1) = points[index];
                let (x2, y2) = points[(index + 1) % points.len()];
                projected += x1 * y2 - x2 * y1;
            }
            let projected = projected.abs() / 2.0;
            let geodetic = (CELL_DEG.to_radians() / 2.0)
                * SEMI_MAJOR_M
                * SEMI_MAJOR_M
                * (authalic_q((lat + CELL_DEG).to_radians(), p.e, p.e2) - authalic_q(lat.to_radians(), p.e, p.e2));
            assert!(
                (projected / geodetic - 1.0).abs() < 1e-5,
                "cell at {lat},{lon}: projected {projected} m^2 vs geodetic {geodetic} m^2"
            );
        }
    }
}
