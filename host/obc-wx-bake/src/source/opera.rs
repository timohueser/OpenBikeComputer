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

use obc_formats::precip4;
use std::fmt::Write as _;

use crate::fetch::{FetchOutcome, Upstream};
use crate::geometry::GridGeometry;
use crate::laea;
use crate::source::{Attribution, BakedFrame, BakedSource, SourceClass};
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
/// This pair is **OPERA's**, not a choice made here: it is what OPERA applies to derive the
/// NIMBUS rain rate, declared in that product's own metadata as `zr_a = 200.0` / `zr_b = 1.6`,
/// and [`Contract::verify`] pins both items so an upstream re-tuning fails the bake.
///
/// It is a *surface* relation, and that is the whole reason [`MAX_TO_SURFACE_RATIO`] exists.
pub const ZR_A: f64 = 200.0;
pub const ZR_B: f64 = 1.6;

/// The column-max to surface-rate correction applied to CIRRUS reflectivity — a **first-cut
/// empirical calibration, not physics**.
///
/// Marshall-Palmer relates *surface* reflectivity to *surface* rain rate, and it is exactly what
/// OPERA applies to the near-surface `PPI` that becomes NIMBUS. CIRRUS is a different measured
/// quantity: a column **maximum**, which carries cores aloft and, in stratiform rain, the bright
/// band. Feeding it a surface relation therefore overstates the rate, and it does so by a
/// measurable amount: over the 149,527 cells where both products saw an echo in the
/// 2026-08-10T00:00 pair, the median CIRRUS/NIMBUS rate ratio is **2.2** — a full intensity band,
/// continent-wide and permanent, at the moment a rider is deciding whether to shelter.
///
/// So the reflectivity path divides its Marshall-Palmer rate by that measured ratio, which is
/// identical to using an effective coefficient `a_eff = 200 x 2.2^1.6 = 706.2` (equivalently
/// -5.48 dBZ) while leaving the pinned 200/1.6 as the NIMBUS contract check. `a` near 700 sits
/// inside the published range for non-surface and convective relations, and it makes the two
/// products agree with each other — which the shared-lattice story needs anyway.
///
/// **This is one number from one frame pair and it should not stay that way.** What would settle
/// it: split the CIRRUS/NIMBUS ratio by regime (stratiform versus convective, at a 30 dBZ
/// threshold) over a full day. A regime-flat ratio means a scalar is exactly right; a much larger
/// ratio in the stratiform population means bright-band contamination, and a scalar is still a
/// large improvement but a regime-aware correction is better. The available ground truth is
/// gauge-adjusted `dwd-rv` over Germany, and scoring both OPERA products against it also answers
/// the open question of whether CIRRUS belongs above or below `dwd-rv` in the mosaic.
pub const MAX_TO_SURFACE_RATIO: f64 = 2.2;

/// `R = (Z/a)^(1/b)` for a reflectivity in dBZ, `Z = 10^(dBZ/10)` — Marshall-Palmer exactly as
/// OPERA declares it, with no column-max correction. This is the relation the NIMBUS contract
/// pins; the CIRRUS path uses [`surface_rate_from_column_max_dbz`].
pub fn rate_mm_per_hour_from_dbz(dbz: f64) -> f64 {
    let z = 10f64.powf(dbz / 10.0);
    (z / ZR_A).powf(1.0 / ZR_B)
}

/// The CIRRUS path: Marshall-Palmer, then [`MAX_TO_SURFACE_RATIO`].
pub fn surface_rate_from_column_max_dbz(dbz: f64) -> f64 {
    rate_mm_per_hour_from_dbz(dbz) / MAX_TO_SURFACE_RATIO
}

/// `a_eff` of the corrected relation, for the record and for the test that pins the two forms
/// against each other.
pub fn effective_zr_a() -> f64 {
    ZR_A * MAX_TO_SURFACE_RATIO.powf(ZR_B)
}

/// The fraction of the composite domain the European radar network covers, measured 2026-08-10.
pub const COVERAGE_FRACTION: f64 = 0.503;
/// How far a frame's coverage may drift from [`COVERAGE_FRACTION`] before the cycle says so. The
/// measured spread over 18 hours is 0.13 percentage points, so five points is a wide margin
/// around "a couple of national radars are down" and a narrow one around "the upstream changed".
pub const COVERAGE_TOLERANCE: f64 = 0.05;

/// The share of covered samples that may fall outside [`Quantity::sane_range`] before the bake
/// fails rather than marking them no-data. One in a thousand of a 16.7-million-cell composite is
/// ~8,400 cells: far more than any plausible glitch, far less than the whole field a unit change
/// or a re-scaled encoding would produce.
pub const MAX_INSANE_FRACTION: f64 = 0.001;

/// What the composite measures, and how a sample becomes a rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quantity {
    /// Column-maximum reflectivity in dBZ (CIRRUS). Converted with
    /// [`surface_rate_from_column_max_dbz`].
    Reflectivity,
    /// Instantaneous near-surface rain rate in mm/h (NIMBUS). Used as it stands.
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

    /// The bounds a finite sample must lie inside. Both are far outside anything meteorological,
    /// so a *population* of samples outside them means the upstream contract changed (a unit
    /// switch, a re-scaled encoding) — see [`MAX_INSANE_FRACTION`] for what happens then, and
    /// what happens instead to a lone glitch cell.
    fn sane_range(self) -> (f64, f64) {
        match self {
            Quantity::Reflectivity => (-40.0, 100.0),
            Quantity::RainRate => (0.0, 10_000.0),
        }
    }

    fn quantize(self, value: f64) -> u8 {
        precip4::quantize_rate_mm_per_hour(match self {
            Quantity::Reflectivity => surface_rate_from_column_max_dbz(value),
            Quantity::RainRate => value,
        })
    }
}

/// One OPERA product's pinned source contract: the raster registration, the cadence and the
/// metadata the decoded object is checked against before a single cell is baked.
#[derive(Debug, Clone, Copy)]
pub struct Contract {
    pub id: &'static str,
    pub quantity: Quantity,
    /// The composite's own name in `GDAL_METADATA`, e.g. `OPERA CIRRUS maximum reflectivity
    /// composite`. Pinning it is how a swapped product under a familiar key is caught.
    pub prodname: &'static str,
    /// The ODIM `product` item: `MAX` (column maximum) or `PPI` (near-surface). This is the fact
    /// [`MAX_TO_SURFACE_RATIO`] is conditioned on, so it is pinned rather than assumed.
    pub odim_product: &'static str,
    pub width: u32,
    pub height: u32,
    /// Native cell size in metres, and the `ModelPixelScale` the COG must declare.
    pub cell_m: f64,
    /// Model coordinates of the upper-left corner of pixel (0, 0) — the **corrected**
    /// registration, which is not what the file's `ModelTiepoint` says.
    ///
    /// OPERA's own false origin (`x_0 = 1,950,000`, `y_0 = -2,100,000`) exists to put the
    /// composite's north-west corner at model (0, 0), and its ODIM corner attributes agree:
    /// `LL` to `UR` spans exactly 3,800,000 x 4,400,000 m, which is exactly 3,800 x 4,400 cells
    /// of 1 km, so those are the raster's **outer** corners. The COG's `ModelTiepoint` instead
    /// says (-500.0002714, +499.9999124) on CIRRUS and (-1000.0002714, +999.9999124) on NIMBUS:
    /// the same corner minus half of each product's *own* pixel, with a bit-identical residual
    /// tail across the two files. That is a converter that read the ODIM corners as pixel
    /// centres, and following it would put the two products' rasters 500 m apart — at which
    /// point a NIMBUS cell straddles half of one CIRRUS cell, all of the next and half of the
    /// one after, and the "one grid at two resolutions" this adapter pair promises WXR3 is not
    /// true.
    ///
    /// So the grid is pinned here and the tiepoint is still *read* and required to equal
    /// `(ul_x - cell/2, ul_y + cell/2)` ([`Contract::verify`]) — if OPERA ever fixes its
    /// converter, the bake fails loudly instead of silently moving half a cell.
    pub ul_x: f64,
    pub ul_y: f64,
    /// How often a new object appears upstream.
    pub cadence_seconds: i64,
    /// How far back discovery probes before giving up.
    pub max_discovery_probes: usize,
    /// How far before the anchor composite to fetch the motion-history frame WXR9's nowcast is
    /// estimated from, or `None` for a product this bakery does not nowcast.
    ///
    /// A multiple of [`Contract::cadence_seconds`], and long enough that a 20 m/s system has moved
    /// a dozen 1 km cells rather than two — see [`crate::source::mrms::MOTION_LAG_SECONDS`] for the
    /// same trade written out. `None` on NIMBUS is a cost decision, not a capability one.
    pub motion_lag_seconds: Option<i64>,
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
        // The tiepoint must be the pinned grid corner minus half a pixel — the exact offset
        // OPERA's converter introduces by reading its own ODIM corners as pixel centres (see
        // `Contract::ul_x`). A tolerance of a centimetre absorbs the sub-millimetre PROJ
        // round-trip in the written value while catching any real re-registration, including
        // the one that would happen if the converter were fixed.
        let (tiepoint_x, tiepoint_y) = (self.ul_x - self.cell_m / 2.0, self.ul_y + self.cell_m / 2.0);
        if (cog.ul_x - tiepoint_x).abs() > 0.01 || (cog.ul_y - tiepoint_y).abs() > 0.01 {
            return Err(format!(
                "{id}: ModelTiepoint ({}, {}) is not the pinned grid corner ({}, {}) less half a pixel",
                cog.ul_x, cog.ul_y, self.ul_x, self.ul_y
            ));
        }
        if cog.nodata != NODATA {
            return Err(format!("{id}: GDAL_NODATA is {}, not {NODATA}", cog.nodata));
        }
        // The projection *method*, its units and its raster convention, before its numbers:
        // `ProjCoordTransGeoKey` 10 is `CT_LambertAzimEqualArea`, 9001 is the metre and 9102 the
        // degree. Without the first, a re-projected composite carrying the same false origin
        // would sail through. `GTRasterTypeGeoKey` 1 is `RasterPixelIsArea`, which is what makes
        // the tiepoint a pixel *corner*: were OPERA ever to write 2 (`RasterPixelIsPoint`) the
        // same bytes would mean a grid half a pixel away, on top of the half pixel above.
        for (key, name, expected) in [
            (1_025u16, "GTRasterTypeGeoKey", 1u16),
            (3_075, "ProjCoordTransGeoKey", 10),
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
        // ODIM `product`: `MAX` is a column maximum, `PPI` a near-surface plan position. Which of
        // the two it is decides whether the Marshall-Palmer relation applies as OPERA declares it
        // or needs `MAX_TO_SURFACE_RATIO` — so a product silently changing its vertical sampling
        // must stop the bake, not quietly re-point the calibration at a different quantity.
        if item("product") != Some(self.odim_product) {
            return Err(format!("{id}: ODIM `product` is {:?}, not {:?}", item("product"), self.odim_product));
        }
        // ODIM's `undetect` is what a NaN sample means. If it ever stops being NaN, the
        // dry-versus-no-coverage mapping below is wrong and must not be guessed at.
        if !item("undetect").is_some_and(|value| value.eq_ignore_ascii_case("nan")) {
            return Err(format!("{id}: metadata `undetect` is {:?}, not NaN", item("undetect")));
        }
        // The Z-R declaration, in both directions. NIMBUS must keep declaring the pinned pair,
        // because that pair is the surface relation this module borrows. CIRRUS must keep
        // declaring *nothing*: it carries no `zr_a`/`zr_b` today, and if one ever appears it
        // means OPERA has taken a position on converting its own column max, which is exactly
        // the position `MAX_TO_SURFACE_RATIO` is a stand-in for.
        let zr = |name: &str| item(name).and_then(|value| value.parse::<f64>().ok());
        match self.quantity {
            Quantity::RainRate if zr("zr_a") != Some(ZR_A) || zr("zr_b") != Some(ZR_B) => {
                return Err(format!(
                    "{id}: upstream Z-R is a={:?} b={:?}, not the pinned {ZR_A}/{ZR_B} the reflectivity calibration is anchored on",
                    item("zr_a"),
                    item("zr_b")
                ));
            }
            Quantity::Reflectivity if item("zr_a").is_some() || item("zr_b").is_some() => {
                return Err(format!(
                    "{id}: the column-max composite now declares its own Z-R (a={:?} b={:?}); \
                     revisit MAX_TO_SURFACE_RATIO rather than ignoring it",
                    item("zr_a"),
                    item("zr_b")
                ));
            }
            _ => {}
        }
        // Temporal identity comes from the decoded bytes, never from the object name alone. Both
        // the nominal stamp (`date`/`time`) and the end of the composite's own integration window
        // (`enddate`/`endtime`) are the key's instant; the *start* deliberately is not, because
        // CIRRUS integrates a roughly ten-minute window (`starttime` 235001 for a 000000 frame)
        // and NIMBUS none at all. Ten minutes is inside the frame cadence and is why CIRRUS still
        // reads as an instantaneous field — unlike `ACRR`, whose window is a full hour, which is
        // the axis it is rejected on (see `opera_nimbus`).
        let stamp = chrono::DateTime::from_timestamp(valid_at, 0).expect("composite timestamp");
        let expected = (stamp.format("%Y%m%d").to_string(), stamp.format("%H%M%S").to_string());
        for (date_item, time_item) in [("date", "time"), ("enddate", "endtime")] {
            let actual =
                (item(date_item).unwrap_or_default().to_string(), item(time_item).unwrap_or_default().to_string());
            if actual != expected {
                return Err(format!(
                    "{id}: composite `{date_item}`/`{time_item}` is {actual:?}, but its key claims {expected:?}"
                ));
            }
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
/// schema backwards.
///
/// `None` means OPERA has published nothing inside the discovery window. [`bake`] turns that into
/// an `Err`, which is this adapter's whole failure surface: the cycle publishes no OPERA frame,
/// the previously published entry is carried forward untouched, and its staleness deadline
/// expires on its own schedule. Every other product in the same invocation is unaffected — see
/// [`crate::cycle::run_cycle`], which isolates a failing adapter rather than failing the cycle.
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
        class: SourceClass::Observation,
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
    let mut insane = 0usize;
    for value in &cog.values {
        let value = f64::from(*value);
        if value == NODATA {
            continue;
        }
        covered += 1;
        if !value.is_nan() && !(low..=high).contains(&value) {
            insane += 1;
        }
    }
    if covered == 0 {
        return Err(format!("{}: the composite covers nothing at all", contract.id));
    }
    // A *population* outside the sane range is a changed unit or a re-scaled encoding, and the
    // cycle must fail closed. A handful of cells is a glitch, and failing a five-minute cycle on
    // one bad cell out of 16.7 million sits oddly beside a coverage gate that only warns — so
    // those cells become no-data below (never dry, never a fabricated rate) and the frame stands.
    if insane as f64 > MAX_INSANE_FRACTION * covered as f64 {
        return Err(format!(
            "{}: {insane} of {covered} covered samples are outside the contracted {low}..={high}",
            contract.id
        ));
    }
    if insane > 0 {
        warnings.push(format!(
            "{}: {insane} of {covered} covered samples are outside {low}..={high} and were marked no-data",
            contract.id
        ));
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
            // Model coordinates, then whole native cells from the raster's upper-left corner —
            // the contract's corrected corner, not the file's half-pixel-shifted tiepoint.
            let column = ((x + laea::FALSE_EASTING_M - contract.ul_x) / contract.cell_m).floor();
            let line = ((contract.ul_y - (y + laea::FALSE_NORTHING_M)) / contract.cell_m).floor();
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
            } else if !(low..=high).contains(&sample) {
                // One of the tolerated glitch cells counted above: unusable, never dry.
                precip4::INTENSITY_NODATA
            } else {
                contract.quantity.quantize(sample)
            });
        }
    }
    Ok(cells)
}

/// One idempotent bake: discover the newest composite, fetch it and lay it on the window.
pub fn bake(
    contract: &Contract,
    upstream: &mut dyn Upstream,
    now: i64,
    warnings: &mut Vec<String>,
) -> Result<BakedSource, String> {
    let id = contract.id;
    let valid_at = discover_latest(contract, upstream, now)?
        .ok_or_else(|| format!("{id}: no composite published within the discovery window"))?;
    let url = contract.object_url(valid_at);
    let fetched = match upstream.fetch(&url, tiff::MAX_OBJECT_BYTES, None)? {
        FetchOutcome::Body(fetched) => fetched,
        FetchOutcome::Unchanged => return Err(format!("{id}: object fetch returned 304 without a validator")),
    };
    let frame = bake_frame(contract, &fetched.bytes, valid_at, warnings)?;
    let motion_history = match contract.motion_lag_seconds {
        Some(lag) => motion_history(contract, upstream, valid_at, lag, warnings),
        None => Vec::new(),
    };
    Ok(BakedSource {
        id,
        geometry: contract.geometry(),
        reference_time: valid_at,
        attribution: contract.attribution,
        frames: vec![frame],
        motion_history,
    })
}

/// The earlier composite [`crate::derive::radar_nowcast`] estimates motion from (WXR9 #1251), or an
/// empty vector with a warning.
///
/// Best-effort on exactly the terms [`crate::source::mrms`]'s equivalent is, and for the same
/// reason: the anchor observation is what a rider over Europe sees at f0 and it must never be put at
/// risk to make a forecast layer possible. One extra HEAD, and one extra object — which for OPERA is
/// a second 16.7 M-cell COG decode and a second resample onto the 25 M-cell window, the most
/// expensive thing WXR9 adds to the cycle. That cost is why only CIRRUS carries a lag: NIMBUS is the
/// coarser backfill under it, and paying it twice over largely the same ground buys very little.
fn motion_history(
    contract: &Contract,
    upstream: &mut dyn Upstream,
    valid_at: i64,
    lag: i64,
    warnings: &mut Vec<String>,
) -> Vec<BakedFrame> {
    let id = contract.id;
    let earlier = valid_at - lag;
    let url = contract.object_url(earlier);
    match upstream.exists(&url) {
        Ok(true) => match upstream.fetch(&url, tiff::MAX_OBJECT_BYTES, None) {
            Ok(FetchOutcome::Body(fetched)) => match bake_frame(contract, &fetched.bytes, earlier, warnings) {
                Ok(frame) => return vec![frame],
                Err(error) => warnings.push(format!("{id}: the motion-history composite failed to bake ({error})")),
            },
            Ok(FetchOutcome::Unchanged) => {
                warnings.push(format!("{id}: the motion-history fetch returned 304 without a validator"))
            }
            Err(error) => warnings.push(format!("{id}: the motion-history composite failed to fetch ({error})")),
        },
        Ok(false) => warnings.push(format!(
            "{id}: no composite published at {url}, so this cycle has no motion baseline and no nowcast"
        )),
        Err(error) => warnings.push(format!("{id}: probing for the motion-history composite failed ({error})")),
    }
    Vec::new()
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
        let valid_at = crate::timefmt::parse_rfc3339("2026-08-10T00:00:00Z").expect("timestamp");
        assert_eq!(
            opera_cirrus::CONTRACT.object_url(valid_at),
            "https://s3.waw3-1.cloudferro.com/openradar-24h/2026/08/10/OPERA/COMP/OPERA@20260810T0000@0@DBZH.tiff"
        );
        assert_eq!(
            opera_nimbus::CONTRACT.object_url(valid_at + 45 * 60),
            "https://s3.waw3-1.cloudferro.com/openradar-24h/2026/08/10/OPERA/COMP/OPERA@20260810T0045@0@RATE.tiff"
        );
    }

    /// The uncorrected relation is Marshall-Palmer exactly, and the corrected one is that
    /// relation with the measured column-max ratio divided out — which is identical to an
    /// effective coefficient of 706.2, i.e. -5.48 dBZ.
    #[test]
    fn the_zr_relation_is_marshall_palmer_and_the_column_max_correction_is_a_coefficient() {
        for rate in [0.05f64, 0.1, 0.25, 1.0, 4.0, 10.0, 50.0, 120.0] {
            let dbz = 10.0 * (ZR_A * rate.powf(ZR_B)).log10();
            let round_trip = rate_mm_per_hour_from_dbz(dbz);
            assert!((round_trip - rate).abs() < 1e-9, "{rate} mm/h -> {dbz} dBZ -> {round_trip}");
            // Dividing the rate by the ratio and using `a_eff` are the same operation.
            let via_coefficient = (10f64.powf(dbz / 10.0) / effective_zr_a()).powf(1.0 / ZR_B);
            let via_ratio = surface_rate_from_column_max_dbz(dbz);
            assert!(
                (via_coefficient - via_ratio).abs() < 1e-12 * rate.max(1.0),
                "{rate}: {via_coefficient} vs {via_ratio}"
            );
        }
        assert!((effective_zr_a() - 706.165).abs() < 0.001, "a_eff {}", effective_zr_a());
        assert!((10.0 * MAX_TO_SURFACE_RATIO.powf(ZR_B).log10() - 5.4788).abs() < 0.001, "the dBZ offset");
        // The textbook anchors of the uncorrected relation: 0.1 mm/h is ~7 dBZ, 10 mm/h ~39 dBZ.
        assert!((10.0 * (ZR_A * 0.1f64.powf(ZR_B)).log10() - 7.01).abs() < 0.01);
        assert!((10.0 * (ZR_A * 10.0f64.powf(ZR_B)).log10() - 39.02).abs() < 0.01);
        // The composite's floor, -32 dBZ, is a detection, not rain: it lands in the trace band.
        assert_eq!(Quantity::Reflectivity.quantize(-32.0), 1);
    }

    /// The correction is exactly what makes the two products agree: a surface rate `R` and the
    /// column-maximum reflectivity a `2.2 x R` echo would show must land in the same band.
    /// Without the correction this is the 2.2x disagreement the review measured.
    #[test]
    fn a_corrected_column_max_and_a_native_rate_quantize_alike() {
        for rate in [0.05f64, 0.12, 0.3, 1.0, 3.0, 8.0, 20.0, 60.0] {
            let column_max_dbz = 10.0 * (ZR_A * (rate * MAX_TO_SURFACE_RATIO).powf(ZR_B)).log10();
            assert_eq!(
                Quantity::Reflectivity.quantize(column_max_dbz),
                Quantity::RainRate.quantize(rate),
                "{rate} mm/h at the surface reads {column_max_dbz} dBZ in the column max"
            );
        }
    }

    /// **The decisive registration property.** One composite at two resolutions: NIMBUS cell
    /// `(r, c)` must be exactly the 2 x 2 block of CIRRUS cells `(2r, 2c) … (2r+1, 2c+1)`, in
    /// model metres and with no remainder anywhere.
    ///
    /// Under the COG's own tiepoints this is false — the two rasters would be offset by 500 m and
    /// a NIMBUS cell would straddle half of one CIRRUS cell, all of the next and half of the one
    /// after — which is how the half-pixel error was caught. It is the property the "exact
    /// cell-for-cell copy" promise to WXR3 rests on, so it is asserted rather than described.
    #[test]
    fn a_nimbus_cell_is_an_exact_two_by_two_block_of_cirrus_cells() {
        let (fine, coarse) = (opera_cirrus::CONTRACT, opera_nimbus::CONTRACT);
        assert_eq!((fine.ul_x, fine.ul_y), (coarse.ul_x, coarse.ul_y), "one grid, one corner");
        assert_eq!(coarse.cell_m, 2.0 * fine.cell_m);
        assert_eq!(fine.width, 2 * coarse.width);
        assert_eq!(fine.height, 2 * coarse.height);
        // Cell bounds in model metres, from each product's own registration.
        let bounds = |contract: &Contract, row: u32, col: u32| {
            let west = contract.ul_x + f64::from(col) * contract.cell_m;
            let north = contract.ul_y - f64::from(row) * contract.cell_m;
            (west, north, west + contract.cell_m, north - contract.cell_m)
        };
        for (row, col) in [(0u32, 0u32), (1, 1), (525, 975), (1_099, 949), (coarse.height - 1, coarse.width - 1)] {
            let (west, north, east, south) = bounds(&coarse, row, col);
            let (fine_west, fine_north, ..) = bounds(&fine, 2 * row, 2 * col);
            let (.., fine_east, fine_south) = bounds(&fine, 2 * row + 1, 2 * col + 1);
            assert_eq!(
                (west, north, east, south),
                (fine_west, fine_north, fine_east, fine_south),
                "NIMBUS ({row}, {col})"
            );
        }
        // And the grid corner is the one OPERA's false origin names, so (55 N, 10 E) is a shared
        // cell corner on both rasters rather than an interior point of either.
        let (model_x, model_y) = laea::forward_model(laea::LAT_0_DEG, laea::LON_0_DEG).expect("origin");
        for contract in [fine, coarse] {
            assert_eq!((contract.ul_x, contract.ul_y), (0.0, 0.0));
            assert!(((model_x - contract.ul_x) / contract.cell_m).fract().abs() < 1e-9);
            assert!(((contract.ul_y - model_y) / contract.cell_m).fract().abs() < 1e-9);
        }
    }
}
