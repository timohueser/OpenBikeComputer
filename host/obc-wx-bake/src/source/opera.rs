//! EUMETNET OPERA: the shared machinery behind the two European radar adapters (WXR6, #1245).
//!
//! OPERA composites the national radars of 30-odd European services into one grid and publishes
//! it anonymously on CloudFerro under CC BY 4.0. Two of its products are useful here, and
//! [`opera_cirrus`](super::opera_cirrus) and [`opera_nimbus`](super::opera_nimbus) are the
//! adapters; everything they share — the key schema, the COG contract, the projection arithmetic,
//! the Z-R relation and the published window — lives in this module.
//!
//! ## The COG, not the ODIM HDF5
//!
//! Every composite is published twice, as `…@DBZH.h5` and `…@DBZH.tiff`. The baker ingests the
//! **COG**: the TIFF subset OPERA writes is a few hundred lines of [`crate::tiff`], the whole
//! object is 3 MB, and the georeferencing travels with the pixels instead of in a `projdef`
//! string. (The `openradar-archive` bucket, which reaches back to 2012, holds **only** the HDF5
//! twins. That matters for a future event pack, not for the bakery, which only ever reads the
//! live 24-hour bucket.)
//!
//! ## No coverage, versus no rain
//!
//! Band 0 of an OPERA composite says three different things and the difference is load-bearing:
//!
//! | Sample | Means | Becomes |
//! | --- | --- | --- |
//! | `GDAL_NODATA` (`-9999000`) | outside radar coverage | [`precip4::INTENSITY_NODATA`] |
//! | `NaN` (ODIM `undetect`) | covered, nothing detected | [`precip4::INTENSITY_DRY`] |
//! | finite | covered, this much | the quantized band |
//!
//! Collapsing the first two would paint the Atlantic, Ukraine and southern Italy dry — a rider
//! reading "no rain" off a region no radar can see. Only the mosaic's global floor source may
//! fill a no-coverage cell, and it can only do that if the cell arrives marked no-data.
//!
//! ## Coverage is static, and it is read per frame anyway
//!
//! Measured over four CIRRUS frames spanning 18 hours of 2026-08-10: 50.34 %, 50.34 %, 50.21 %,
//! 50.22 % of the domain covered, and the union and intersection of the four masks differ by
//! 21,936 cells — **0.13 % of the domain**, one or two national radars going in and out of
//! service. So coverage is static in shape but not to the cell, and this adapter therefore reads
//! it from each frame's own nodata sentinel rather than from a committed mask: the sentinel is
//! already decoded, it costs nothing, and it cannot freeze yesterday's radar outage into today's
//! product. The static part is used for exactly two things, both bake-time only and neither
//! reaching the manifest, an OBCG object or a client: it fixes the published window ([`WINDOW`],
//! derived once from the measured covered bounding box) and it arms a sanity warning
//! ([`COVERAGE_FRACTION`]) when a frame's coverage departs far enough from it to mean something
//! broke upstream.

use obc_formats::obcg::FLAG_OBSERVED;
use obc_formats::precip4;
use std::fmt::Write as _;

use crate::fetch::{FetchOutcome, Upstream};
use crate::geometry::GridGeometry;
use crate::laea;
use crate::manifest::Product;
use crate::source::{AdapterOutcome, Attribution, BakedFrame, BakedProduct};
use crate::tiff::{self, Cog};

/// The live 24-hour bucket. Anonymous, no credentials, CC BY 4.0.
pub const BUCKET: &str = "https://s3.waw3-1.cloudferro.com/openradar-24h";

/// CC BY 4.0, the licence every OPERA object names in its own `GDAL_METADATA`.
pub const OPERA_TERMS_URL: &str = "https://creativecommons.org/licenses/by/4.0/";

/// The no-coverage sentinel, `GDAL_NODATA` in every OPERA COG.
pub const NODATA: f64 = -9_999_000.0;

/// The Marshall-Palmer Z-R relation, `Z = a R^b` with `a = 200`, `b = 1.6` (Marshall & Palmer,
/// *The distribution of raindrops with size*, J. Meteor. 5, 1948).
///
/// It is not a free choice here. OPERA applies exactly this relation to turn reflectivity into
/// the NIMBUS rain rate and says so in the product's own metadata (`zr_a = 200.0`,
/// `zr_b = 1.6`) — [`Contract::verify`] pins those two items, so if OPERA ever re-tunes its
/// relation the bake fails instead of quietly disagreeing with itself. Using the same pair for
/// CIRRUS is therefore not a guess about European rain, it is the statement that both adapters
/// convert reflectivity the way the source does.
pub const ZR_A: f64 = 200.0;
pub const ZR_B: f64 = 1.6;

/// `R = (Z/a)^(1/b)` for a reflectivity in dBZ, `Z = 10^(dBZ/10)`.
pub fn rate_mm_per_hour_from_dbz(dbz: f64) -> f64 {
    let z = 10f64.powf(dbz / 10.0);
    (z / ZR_A).powf(1.0 / ZR_B)
}

/// The fraction of the composite domain the European radar network covers, measured 2026-08-10.
pub const COVERAGE_FRACTION: f64 = 0.503;
/// How far a frame's coverage may drift from [`COVERAGE_FRACTION`] before the cycle says so. The
/// measured spread over 18 hours is 0.13 percentage points, so five points is a wide margin
/// around "a couple of national radars are down" and a narrow one around "the upstream changed".
pub const COVERAGE_TOLERANCE: f64 = 0.05;

/// What the composite measures, and how a sample becomes a rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quantity {
    /// Column-maximum reflectivity in dBZ (CIRRUS). Converted with [`rate_mm_per_hour_from_dbz`].
    Reflectivity,
    /// Instantaneous rain rate in mm/h (NIMBUS). Used as it stands.
    RainRate,
}

impl Quantity {
    /// The `@<quantity>` segment of the object key, and the band's `DESCRIPTION` item.
    pub fn wire_name(self) -> &'static str {
        match self {
            Quantity::Reflectivity => "DBZH",
            Quantity::RainRate => "RATE",
        }
    }

    /// The bounds a finite sample must lie inside. Outside them the upstream contract has
    /// changed (a unit switch, a re-scaled encoding) and the cycle fails closed rather than
    /// publishing nonsense; both are far outside anything meteorological.
    fn sane_range(self) -> (f64, f64) {
        match self {
            Quantity::Reflectivity => (-40.0, 100.0),
            Quantity::RainRate => (0.0, 10_000.0),
        }
    }

    fn quantize(self, value: f64) -> u8 {
        precip4::quantize_rate_mm_per_hour(match self {
            Quantity::Reflectivity => rate_mm_per_hour_from_dbz(value),
            Quantity::RainRate => value,
        })
    }
}

/// One OPERA product's pinned source contract: the raster registration, the cadence and the
/// metadata the decoded object is checked against before a single cell is baked.
#[derive(Debug, Clone, Copy)]
pub struct Contract {
    pub id: &'static str,
    pub product_code: u8,
    pub quantity: Quantity,
    /// The composite's own name in `GDAL_METADATA`, e.g. `OPERA CIRRUS maximum reflectivity
    /// composite`. Pinning it is how a swapped product under a familiar key is caught.
    pub prodname: &'static str,
    pub width: u32,
    pub height: u32,
    /// Native cell size in metres, and the `ModelPixelScale` the COG must declare.
    pub cell_m: f64,
    /// Model coordinates of the upper-left corner of pixel (0, 0) — `ModelTiepoint`.
    pub ul_x: f64,
    pub ul_y: f64,
    /// How often a new object appears upstream.
    pub cadence_seconds: i64,
    /// How far back discovery probes before giving up.
    pub max_discovery_probes: usize,
    pub staleness_seconds: i64,
    pub attribution: Attribution,
}

impl Contract {
    /// The published window as this product's geometry: the canonical lattice, with the
    /// product's own native resolution in `cell_size_m`.
    pub fn geometry(&self) -> GridGeometry {
        GridGeometry { cell_size_m: self.cell_m as u16, ..WINDOW }
    }

    /// The immutable object key of the composite valid at `valid_at`.
    pub fn object_url(&self, valid_at: i64) -> String {
        let time = chrono::DateTime::from_timestamp(valid_at, 0).expect("composite timestamp");
        let mut url = String::from(BUCKET);
        let _ = write!(
            url,
            "/{}/OPERA/COMP/OPERA@{}T{}@0@{}.tiff",
            time.format("%Y/%m/%d"),
            time.format("%Y%m%d"),
            time.format("%H%M"),
            self.quantity.wire_name()
        );
        url
    }

    /// Check a decoded object against this contract: the raster registration, the sample layout,
    /// the sentinels, the product identity and the composite's own timestamps.
    pub fn verify(&self, cog: &Cog, valid_at: i64) -> Result<(), String> {
        let id = self.id;
        if cog.width != self.width || cog.height != self.height {
            return Err(format!(
                "{id}: raster is {} x {}, not the pinned {} x {}",
                cog.width, cog.height, self.width, self.height
            ));
        }
        if cog.samples_per_pixel != 2 {
            return Err(format!("{id}: {} samples per pixel, expected value + quality", cog.samples_per_pixel));
        }
        if cog.pixel_x != self.cell_m || cog.pixel_y != self.cell_m {
            return Err(format!("{id}: pixel scale {} x {} is not {} m", cog.pixel_x, cog.pixel_y, self.cell_m));
        }
        // The registration is stated to the millimetre and reproduced to a micrometre; a
        // tolerance of a centimetre catches a half-pixel re-registration (which is 500 m) while
        // absorbing the round-trip noise in the tiepoint OPERA's own converter writes.
        if (cog.ul_x - self.ul_x).abs() > 0.01 || (cog.ul_y - self.ul_y).abs() > 0.01 {
            return Err(format!(
                "{id}: raster origin ({}, {}) is not the pinned ({}, {})",
                cog.ul_x, cog.ul_y, self.ul_x, self.ul_y
            ));
        }
        if cog.nodata != NODATA {
            return Err(format!("{id}: GDAL_NODATA is {}, not {NODATA}", cog.nodata));
        }
        // The projection *method* and its units, before its numbers: `ProjCoordTransGeoKey` 10 is
        // `CT_LambertAzimEqualArea`, 9001 is the metre and 9102 the degree. Without this a
        // re-projected composite carrying the same false origin would sail through.
        for (key, name, expected) in [
            (3_075u16, "ProjCoordTransGeoKey", 10u16),
            (3_076, "ProjLinearUnitsGeoKey", 9_001),
            (2_054, "GeogAngularUnitsGeoKey", 9_102),
        ] {
            if cog.geo_key(key) != Some(expected) {
                return Err(format!("{id}: {name} is {:?}, not {expected}", cog.geo_key(key)));
            }
        }
        // `GeoDoubleParams` carries the projection this module's constants are derived from.
        let expected_params = [
            laea::LAT_0_DEG,
            laea::LON_0_DEG,
            laea::FALSE_EASTING_M,
            laea::FALSE_NORTHING_M,
            laea::INVERSE_FLATTENING,
            laea::SEMI_MAJOR_M,
        ];
        if cog.geo_double_params.len() < expected_params.len()
            || cog.geo_double_params[..expected_params.len()] != expected_params
        {
            return Err(format!(
                "{id}: projection parameters {:?} are not the pinned LAEA {expected_params:?} ({})",
                cog.geo_double_params,
                laea::OPERA_PROJDEF
            ));
        }

        let item = |name: &str| tiff::metadata_item(&cog.metadata, name);
        if item("object") != Some("COMP") {
            return Err(format!("{id}: metadata `object` is {:?}, not a composite", item("object")));
        }
        if item("prodname") != Some(self.prodname) {
            return Err(format!("{id}: metadata `prodname` is {:?}, not {:?}", item("prodname"), self.prodname));
        }
        if item("DESCRIPTION") != Some(self.quantity.wire_name()) {
            return Err(format!("{id}: band 0 is {:?}, not {}", item("DESCRIPTION"), self.quantity.wire_name()));
        }
        // ODIM's `undetect` is what a NaN sample means. If it ever stops being NaN, the
        // dry-versus-no-coverage mapping below is wrong and must not be guessed at.
        if !item("undetect").is_some_and(|value| value.eq_ignore_ascii_case("nan")) {
            return Err(format!("{id}: metadata `undetect` is {:?}, not NaN", item("undetect")));
        }
        if self.quantity == Quantity::RainRate {
            let zr = |name: &str| item(name).and_then(|value| value.parse::<f64>().ok());
            if zr("zr_a") != Some(ZR_A) || zr("zr_b") != Some(ZR_B) {
                return Err(format!(
                    "{id}: upstream Z-R is a={:?} b={:?}, not the pinned {ZR_A}/{ZR_B} the reflectivity adapter also uses",
                    item("zr_a"),
                    item("zr_b")
                ));
            }
        }
        // Temporal identity comes from the decoded bytes, never from the object name alone.
        let stamp = chrono::DateTime::from_timestamp(valid_at, 0).expect("composite timestamp");
        let expected = (stamp.format("%Y%m%d").to_string(), stamp.format("%H%M%S").to_string());
        let actual = (item("date").unwrap_or_default().to_string(), item("time").unwrap_or_default().to_string());
        if actual != expected {
            return Err(format!("{id}: composite is stamped {actual:?}, but its key claims {expected:?}"));
        }
        Ok(())
    }
}

/// The published lat/lon window, shared by both OPERA adapters.
///
/// It is the canonical 0.01-degree lattice on whole-degree edges — the lattice WXR1 (#1240) chose
/// and the one MRMS already publishes on — cropped to the measured covered bounding box
/// (34.08 N … 72.79 N, 27.63 W … 35.55 E over the union of four frames) rounded out to whole
/// degrees. Cropping matters: the composite domain is 3,800 x 4,400 km of which the radars see
/// half, and the corners it drops are the mid-Atlantic and the Kara Sea, never a rider.
///
/// Choosing the canonical lattice rather than a native-pitch window is the WXR3 (#1242) handoff:
/// the mosaic's job for OPERA becomes an exact cell-for-cell copy instead of a second resample.
pub const WINDOW: GridGeometry = GridGeometry {
    south_lat_udeg: 34_000_000,
    west_lon_udeg: -28_000_000,
    cell_lat_udeg: 10_000,
    cell_lon_udeg: 10_000,
    width: 6_400,
    height: 3_900,
    cell_size_m: 1_000,
    // WXR1's measured pair: 256-cell tiles with the deflate codec, 128 directory entries a page.
    tile_edge: 256,
    entries_per_page: 128,
};

/// The newest published composite at or before `now`, discovered by probing the immutable key
/// schema backwards. `None` means OPERA has published nothing recent, and the adapter then skips
/// the cycle so the product's staleness deadline expires honestly.
pub fn discover_latest(contract: &Contract, upstream: &mut dyn Upstream, now: i64) -> Result<Option<i64>, String> {
    let mut candidate = now - now.rem_euclid(contract.cadence_seconds);
    for _ in 0..contract.max_discovery_probes {
        if upstream.exists(&contract.object_url(candidate))? {
            return Ok(Some(candidate));
        }
        candidate -= contract.cadence_seconds;
    }
    Ok(None)
}

/// Decode, validate and resample one composite into the product's published window.
pub fn bake_frame(
    contract: &Contract,
    bytes: &[u8],
    valid_at: i64,
    warnings: &mut Vec<String>,
) -> Result<BakedFrame, String> {
    bake_frame_on(contract, bytes, contract.geometry(), valid_at, warnings)
}

/// The same bake onto an explicit window. Public so a fixture test can drive the whole path —
/// decode, contract verification, projection, quantization — over a checked-in crop of the real
/// object and its own footprint, rather than over a 25-million-cell continent.
pub fn bake_frame_on(
    contract: &Contract,
    bytes: &[u8],
    geometry: GridGeometry,
    valid_at: i64,
    warnings: &mut Vec<String>,
) -> Result<BakedFrame, String> {
    let cog = tiff::decode_band0(bytes)?;
    contract.verify(&cog, valid_at)?;
    Ok(BakedFrame {
        offset_min: 0,
        valid_at,
        flags: FLAG_OBSERVED,
        source: None,
        cells: resample(contract, &cog, geometry, warnings)?,
    })
}

/// Nearest-neighbour from the native LAEA raster onto the published window.
///
/// Forward-project each output cell centre, floor it into the native raster and take that cell —
/// the same no-interpolation rule as [`crate::stereo`] and [`crate::lcc`]. The latitude-dependent
/// half of the projection is hoisted per row, so the inner loop is a sine, a cosine and a square
/// root.
fn resample(
    contract: &Contract,
    cog: &Cog,
    geometry: GridGeometry,
    warnings: &mut Vec<String>,
) -> Result<Vec<u8>, String> {
    geometry.validate()?;
    let (low, high) = contract.quantity.sane_range();
    let mut covered = 0usize;
    for value in &cog.values {
        let value = f64::from(*value);
        if value == NODATA {
            continue;
        }
        covered += 1;
        if !value.is_nan() && !(low..=high).contains(&value) {
            return Err(format!("{}: sample {value} is outside the contracted {low}..={high}", contract.id));
        }
    }
    if covered == 0 {
        return Err(format!("{}: the composite covers nothing at all", contract.id));
    }
    let fraction = covered as f64 / cog.values.len() as f64;
    if (fraction - COVERAGE_FRACTION).abs() > COVERAGE_TOLERANCE {
        warnings.push(format!(
            "{}: radar coverage is {:.2} % of the composite domain, against the measured {:.1} %",
            contract.id,
            fraction * 100.0,
            COVERAGE_FRACTION * 100.0
        ));
    }

    let width = cog.width as usize;
    let mut cells = Vec::with_capacity(geometry.cells());
    for output_row in 0..geometry.height {
        let Some(row) = laea::row(geometry.center_lat_deg(output_row)) else {
            cells.extend(std::iter::repeat_n(precip4::INTENSITY_NODATA, geometry.width as usize));
            continue;
        };
        for output_col in 0..geometry.width {
            let projected = laea::forward_in_row(&row, geometry.center_lon_deg(output_col));
            let Some((x, y)) = projected else {
                cells.push(precip4::INTENSITY_NODATA);
                continue;
            };
            // Model coordinates, then whole native cells from the raster's upper-left corner.
            let column = ((x + laea::FALSE_EASTING_M - cog.ul_x) / contract.cell_m).floor();
            let line = ((cog.ul_y - (y + laea::FALSE_NORTHING_M)) / contract.cell_m).floor();
            if column < 0.0 || line < 0.0 || column >= f64::from(cog.width) || line >= f64::from(cog.height) {
                cells.push(precip4::INTENSITY_NODATA);
                continue;
            }
            let sample = f64::from(cog.values[line as usize * width + column as usize]);
            cells.push(if sample == NODATA {
                // Outside radar coverage: no-data, so the mosaic's floor source fills it.
                precip4::INTENSITY_NODATA
            } else if sample.is_nan() {
                // ODIM `undetect`: covered, and nothing came back. That is genuinely dry.
                precip4::INTENSITY_DRY
            } else {
                contract.quantity.quantize(sample)
            });
        }
    }
    Ok(cells)
}

/// One idempotent bake: discover the newest composite, short-circuit if it is the published one,
/// otherwise fetch it and lay it on the window.
pub fn bake(
    contract: &Contract,
    upstream: &mut dyn Upstream,
    previous: Option<&Product>,
    now: i64,
    warnings: &mut Vec<String>,
) -> Result<AdapterOutcome, String> {
    let id = contract.id;
    let valid_at = discover_latest(contract, upstream, now)?
        .ok_or_else(|| format!("{id}: no composite published within the discovery window"))?;
    let previous_reference = previous.and_then(|product| product.reference_unix());
    if previous_reference == Some(valid_at) {
        return Ok(AdapterOutcome::Unchanged);
    }
    // Upstream regression: the newest object is older than the one already published (a withdrawn
    // object, or a clock going backwards). Never move reference_time or the staleness deadline
    // into the past while published frames stand.
    if previous_reference.is_some_and(|published| published > valid_at) {
        warnings.push(format!(
            "{id}: newest composite {valid_at} is older than the published {}; keeping the published product",
            previous_reference.expect("checked")
        ));
        return Ok(AdapterOutcome::Unchanged);
    }
    let url = contract.object_url(valid_at);
    let fetched = match upstream.fetch(&url, tiff::MAX_OBJECT_BYTES, None)? {
        FetchOutcome::Body(fetched) => fetched,
        FetchOutcome::Unchanged => return Err(format!("{id}: object fetch returned 304 without a validator")),
    };
    let frame = bake_frame(contract, &fetched.bytes, valid_at, warnings)?;
    Ok(AdapterOutcome::Baked(Box::new(BakedProduct {
        id,
        product_code: contract.product_code,
        tier: obc_formats::obcg::TIER_RADAR,
        geometry: contract.geometry(),
        reference_time: valid_at,
        staleness_deadline: valid_at + contract.staleness_seconds,
        attribution: contract.attribution,
        // Keys are immutable per composite time, so run identity is the short-circuit; an ETag
        // from one key would be meaningless against the next one.
        upstream_etag: None,
        frames: vec![frame],
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{opera_cirrus, opera_nimbus};

    #[test]
    fn the_published_window_is_the_canonical_lattice_over_the_measured_coverage() {
        WINDOW.validate().expect("within the OBCG format limits");
        assert_eq!((WINDOW.cell_lat_udeg, WINDOW.cell_lon_udeg), (10_000, 10_000), "the canonical 0.01-degree lattice");
        assert_eq!(WINDOW.north_lat_udeg(), 73_000_000);
        assert_eq!(WINDOW.east_lon_udeg(), 36_000_000);
        // Measured covered bounding box of the union of four frames, 2026-08-10.
        assert!(f64::from(WINDOW.south_lat_udeg) / 1e6 <= 34.0783);
        assert!(WINDOW.north_lat_udeg() as f64 / 1e6 >= 72.7867);
        assert!(f64::from(WINDOW.west_lon_udeg) / 1e6 <= -27.6285);
        assert!(WINDOW.east_lon_udeg() as f64 / 1e6 >= 35.5462);
        // Both products publish the same lattice, so the mosaic never has to reconcile two.
        assert_eq!(opera_cirrus::CONTRACT.geometry().cell_lat_udeg, opera_nimbus::CONTRACT.geometry().cell_lat_udeg);
        assert_eq!(opera_cirrus::CONTRACT.geometry().south_lat_udeg, opera_nimbus::CONTRACT.geometry().south_lat_udeg);
        assert_eq!(opera_cirrus::CONTRACT.geometry().cell_size_m, 1_000);
        assert_eq!(opera_nimbus::CONTRACT.geometry().cell_size_m, 2_000);
    }

    #[test]
    fn object_keys_follow_the_pinned_schema() {
        let valid_at = crate::manifest::parse_rfc3339("2026-08-10T00:00:00Z").expect("timestamp");
        assert_eq!(
            opera_cirrus::CONTRACT.object_url(valid_at),
            "https://s3.waw3-1.cloudferro.com/openradar-24h/2026/08/10/OPERA/COMP/OPERA@20260810T0000@0@DBZH.tiff"
        );
        assert_eq!(
            opera_nimbus::CONTRACT.object_url(valid_at + 45 * 60),
            "https://s3.waw3-1.cloudferro.com/openradar-24h/2026/08/10/OPERA/COMP/OPERA@20260810T0045@0@RATE.tiff"
        );
    }

    /// The Z-R relation, at the band edges that matter. `Z = 200 R^1.6` inverted must put 0.1,
    /// 1 and 10 mm/h where the reflectivity textbooks do, and the two adapters must agree: a
    /// NIMBUS rate and the CIRRUS reflectivity OPERA would have derived it from quantize alike.
    #[test]
    fn the_zr_relation_is_marshall_palmer_and_both_adapters_share_it() {
        for rate in [0.05f64, 0.1, 0.25, 1.0, 4.0, 10.0, 50.0, 120.0] {
            let dbz = 10.0 * (ZR_A * rate.powf(ZR_B)).log10();
            let round_trip = rate_mm_per_hour_from_dbz(dbz);
            assert!((round_trip - rate).abs() < 1e-9, "{rate} mm/h -> {dbz} dBZ -> {round_trip}");
            assert_eq!(
                Quantity::Reflectivity.quantize(dbz),
                Quantity::RainRate.quantize(rate),
                "{rate} mm/h must quantize the same whichever product it arrived in"
            );
        }
        // The textbook anchors: 0.1 mm/h is ~7 dBZ and 10 mm/h is ~39 dBZ under Marshall-Palmer.
        assert!((10.0 * (ZR_A * 0.1f64.powf(ZR_B)).log10() - 7.01).abs() < 0.01);
        assert!((10.0 * (ZR_A * 10.0f64.powf(ZR_B)).log10() - 39.02).abs() < 0.01);
        // The composite's floor, -32 dBZ, is a detection, not rain: it lands in the trace band.
        assert_eq!(Quantity::Reflectivity.quantize(-32.0), 1);
    }
}
