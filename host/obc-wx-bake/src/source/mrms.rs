//! NOAA MRMS `PrecipRate` — the CONUS radar observation, 1 km at a two-minute cadence.
//!
//! A source in its own right since #1246 deleted the composed `us` product it used to be the
//! observation half of. It discovers the newest published two-minute object on NOAA Open Data
//! Dissemination, decodes the gzipped GRIB2 against WX1's pinned contract, and quantizes it onto
//! the native 0.01-degree lattice. The MRMS grid is already a regular lat/lon grid whose points
//! are the cell centres of a clean 0.01-degree window — the canonical lattice's own window, cell
//! for cell — so the only "reprojection" is the row flip from the GRIB's north-to-south scan into
//! OBCG's south-to-north row order: an exact one-to-one remap, no resampling, no interpolation.
//!
//! One frame per cycle, and it is an observation. Where it reaches, it is the best thing the
//! mosaic has over CONUS; ahead of it [`crate::source::hrrr`] is the next row down.
//!
//! The whole 7,000 x 3,500 field is held in RAM (about 98 MB of `f32` plus 24.5 MB of quantized
//! cells) — the deliberate sizing WX1's 161 MB spike measurement called for, and the reason the
//! baker never runs two MRMS decodes concurrently.

use obc_formats::precip4;
use std::fmt::Write as _;

use crate::fetch::{FetchOutcome, Upstream};
use crate::geometry::GridGeometry;
use crate::grib::{decode_gzip_field, ExpectedGrib, MAX_COMPRESSED_BYTES, MRMS_CONUS_GRID_DEFINITION_HEX};
use crate::source::{Adapter, Attribution, BakedFrame, BakedSource, DerivedNowcast, SourceClass, NOAA_TERMS_URL};

pub const ID: &str = "mrms";

/// The nowcast the bakery derives from this source (WXR9 #1251): the same 1 km field, advected.
pub const NOWCAST: DerivedNowcast = DerivedNowcast {
    parent: ID,
    id: "mrms-nowcast",
    attribution: Attribution {
        text: "Source: NOAA/NCEP MRMS PrecipRate; quantized and extrapolated forward by optical-flow advection by OpenBikeComputer; no NOAA endorsement is implied",
        url: NOAA_TERMS_URL,
    },
};

/// How far before the anchor observation the motion-history frame is fetched from, in seconds.
///
/// Ten minutes, which is five of MRMS's two-minute steps. The trade is between two errors: a short
/// baseline measures a displacement of a couple of cells and is mostly reading quantization noise,
/// and a long one measures the motion of a field that has changed shape in the meantime. Ten
/// minutes puts a 20 m/s storm twelve 1 km cells along — a displacement the estimator resolves
/// comfortably — while keeping the two images recognisably the same weather.
pub const MOTION_LAG_SECONDS: i64 = 600;

pub const BUCKET: &str = "https://noaa-mrms-pds.s3.amazonaws.com";

pub const ATTRIBUTION: Attribution = Attribution {
    text: "Source: NOAA/NCEP MRMS PrecipRate; modified/quantized by OpenBikeComputer; no NOAA endorsement is implied",
    url: NOAA_TERMS_URL,
};

/// The **source window**, and already a window of the canonical 0.01 degree lattice: 7,000 x 3,500
/// cells whose **edges** are the clean multiples 20 N/55 N and 130 W/60 W, so the GRIB's first
/// point (54.995 N, 230.005 E) is the centre of the north-west cell — and every cell is exactly one
/// canonical cell, so the mosaic copies rather than resamples. MRMS is CONUS-only in WX1's
/// contract: Alaska, Hawaii and Puerto Rico are separate measured decisions, never a clamp of this
/// grid.
pub const GEOMETRY: GridGeometry = GridGeometry {
    south_lat_udeg: 20_000_000,
    west_lon_udeg: -130_000_000,
    cell_lat_udeg: 10_000,
    cell_lon_udeg: 10_000,
    width: 7_000,
    height: 3_500,
    cell_size_m: 1_000,
    // 64-cell tiles: 110 x 55 = 6,050 entries, twelve directory pages (OBCG_Spec §12).
    tile_edge: 64,
    entries_per_page: 512,
};

/// The native GRIB raster: same dimensions, scanned north-to-south.
const NATIVE_COLS: usize = 7_000;
const NATIVE_ROWS: usize = 3_500;

/// WX1's documented sentinels: `-1` missing and `-3` no radar coverage. Neither is dry.
const MISSING: f32 = -1.0;
const NO_COVERAGE: f32 = -3.0;

/// Objects are published every two minutes on the minute.
pub const CADENCE_SECONDS: i64 = 120;
/// How far back discovery probes before giving up: WX1 measured a 2 min 44 s publication delay,
/// so ten two-minute steps is roughly seven times the observed latency.
pub const MAX_DISCOVERY_PROBES: usize = 10;

/// The pinned WX1 field contract (public so tests decode through the same contract).
pub const EXPECTED: ExpectedGrib = ExpectedGrib {
    discipline: 209,
    category: 6,
    parameter: 1,
    grid_template: 0,
    expected_points: NATIVE_COLS * NATIVE_ROWS,
    expected_grid_definition_hex: MRMS_CONUS_GRID_DEFINITION_HEX,
    product_template: 0,
    representation_templates: &[41],
    missing_sentinels: &[MISSING, NO_COVERAGE],
    allowed_messages: &[1],
    require_identical_messages: false,
};

/// The immutable object key of one two-minute observation.
pub fn object_url(valid_at: i64) -> String {
    let time = chrono::DateTime::from_timestamp(valid_at, 0).expect("observation timestamp");
    let mut url = String::from(BUCKET);
    let _ = write!(
        url,
        "/CONUS/PrecipRate_00.00/{}/MRMS_PrecipRate_00.00_{}-{}.grib2.gz",
        time.format("%Y%m%d"),
        time.format("%Y%m%d"),
        time.format("%H%M%S")
    );
    url
}

/// The newest published two-minute observation at or before `now`, discovered by probing the
/// immutable key schema backwards. `None` means MRMS has published nothing recent — the US
/// product then skips this cycle and its staleness deadline expires honestly.
pub fn discover_latest(upstream: &mut dyn Upstream, now: i64) -> Result<Option<i64>, String> {
    let mut candidate = now - now.rem_euclid(CADENCE_SECONDS);
    for _ in 0..MAX_DISCOVERY_PROBES {
        if upstream.exists(&object_url(candidate))? {
            return Ok(Some(candidate));
        }
        candidate -= CADENCE_SECONDS;
    }
    Ok(None)
}

/// Fetch and bake the observation frame valid at `valid_at`, anchored at that same instant.
pub fn bake_observation(upstream: &mut dyn Upstream, valid_at: i64) -> Result<BakedFrame, String> {
    GEOMETRY.validate()?;
    let url = object_url(valid_at);
    let fetched = match upstream.fetch(&url, MAX_COMPRESSED_BYTES, None)? {
        FetchOutcome::Body(fetched) => fetched,
        FetchOutcome::Unchanged => return Err("MRMS object fetch returned 304 without a validator".into()),
    };
    let field = decode_gzip_field(&fetched.bytes, &EXPECTED)?;
    // Temporal identity comes from the decoded bytes, never from the object name: MRMS
    // `PrecipRate` is an analysis, so its reference time, interval start and interval end are all
    // the observation instant the key claims.
    if field.reference_unix_seconds != valid_at
        || field.valid_start_unix_seconds != valid_at
        || field.valid_end_unix_seconds != valid_at
    {
        return Err(format!("MRMS object {url} disagrees with its own GRIB timestamps"));
    }
    if field.values.len() != GEOMETRY.cells() {
        return Err("MRMS field does not have the contracted cell count".into());
    }
    Ok(BakedFrame { offset_min: 0, valid_at, class: SourceClass::Observation, cells: quantize(&field.values) })
}

/// The earlier observation [`crate::derive::radar_nowcast`] estimates motion from, or an empty
/// vector with a warning.
///
/// **Best-effort by design.** One HEAD probe decides it, and everything from here on tolerates
/// failure: a missing object, a decode that disagrees with its own timestamps, or an upstream that
/// simply has not kept ten minutes of history. None of those may cost the cycle its MRMS anchor —
/// the observation is the thing riders over CONUS actually see at f0, and losing it to make a
/// forecast layer possible would be exactly backwards. What is lost instead is the nowcast layer,
/// and the mosaic falls back to HRRR at f+15, which is where it was before WXR9.
///
/// One probe rather than a walk backwards, too. Discovery already walks; this asks for one specific
/// instant, so a cycle whose upstream is healthy pays a single extra HEAD and a single extra body.
fn motion_history(upstream: &mut dyn Upstream, observation: i64, warnings: &mut Vec<String>) -> Vec<BakedFrame> {
    let earlier = observation - MOTION_LAG_SECONDS;
    let url = object_url(earlier);
    match upstream.exists(&url) {
        Ok(true) => match bake_observation(upstream, earlier) {
            Ok(frame) => return vec![frame],
            Err(error) => warnings.push(format!(
                "mrms: the {MOTION_LAG_SECONDS} s motion-history frame failed to bake ({error}); no nowcast this cycle"
            )),
        },
        Ok(false) => warnings.push(format!(
            "mrms: no observation published at {url}, so this cycle has no motion baseline and no nowcast"
        )),
        Err(error) => warnings.push(format!("mrms: probing for the motion-history frame failed ({error})")),
    }
    Vec::new()
}

pub struct Mrms;

impl Adapter for Mrms {
    fn id(&self) -> &'static str {
        ID
    }

    fn bake(&self, upstream: &mut dyn Upstream, now: i64, warnings: &mut Vec<String>) -> Result<BakedSource, String> {
        // Discovery is HEAD probes only, so finding the newest published object costs a request
        // and no body bytes.
        let observation =
            discover_latest(upstream, now)?.ok_or("no MRMS observation published within the discovery window")?;
        let frame = bake_observation(upstream, observation)?;
        Ok(BakedSource {
            id: ID,
            geometry: GEOMETRY,
            reference_time: observation,
            attribution: ATTRIBUTION,
            frames: vec![frame],
            motion_history: motion_history(upstream, observation, warnings),
        })
    }
}

/// Quantize the native field into OBCG cell order. Row `r` of the output (row 0 = south) is
/// native row `NATIVE_ROWS - 1 - r`, because the GRIB scans north-to-south; columns are
/// unchanged. Missing and no-coverage cells become the no-data intensity — never dry.
fn quantize(values: &[f32]) -> Vec<u8> {
    let mut cells = Vec::with_capacity(GEOMETRY.cells());
    for row in 0..NATIVE_ROWS {
        let native_row = NATIVE_ROWS - 1 - row;
        let start = native_row * NATIVE_COLS;
        for value in &values[start..start + NATIVE_COLS] {
            cells.push(if EXPECTED.is_missing(*value) {
                precip4::INTENSITY_NODATA
            } else {
                // The field is already an mm/hour rate (WX1's pinned unit).
                precip4::quantize_rate_mm_per_hour(f64::from(*value))
            });
        }
    }
    cells
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_published_window_matches_the_pinned_native_registration() {
        GEOMETRY.validate().expect("geometry is within the OBCG limits");
        assert_eq!(GEOMETRY.north_lat_udeg(), 55_000_000);
        assert_eq!(GEOMETRY.east_lon_udeg(), -60_000_000);
        // The GRIB's first point is the centre of the north-west cell, and its last point the
        // centre of the south-east cell (WX1: 54.995,230.005 to 20.005001,299.994998).
        assert!((GEOMETRY.center_lat_deg(GEOMETRY.height - 1) - 54.995).abs() < 1e-9);
        assert!((GEOMETRY.center_lon_deg(0) - -129.995).abs() < 1e-9);
        assert!((GEOMETRY.center_lat_deg(0) - 20.005).abs() < 1e-9);
        assert!((GEOMETRY.center_lon_deg(GEOMETRY.width - 1) - -60.005).abs() < 1e-9);
    }

    #[test]
    fn object_keys_follow_the_pinned_schema() {
        let valid_at = crate::timefmt::parse_rfc3339("2026-08-09T16:58:00Z").unwrap();
        assert_eq!(
            object_url(valid_at),
            "https://noaa-mrms-pds.s3.amazonaws.com/CONUS/PrecipRate_00.00/20260809/MRMS_PrecipRate_00.00_20260809-165800.grib2.gz"
        );
    }

    #[test]
    fn quantization_keeps_missing_and_no_coverage_out_of_dry() {
        let mut values = vec![0.0f32; GEOMETRY.cells()];
        // Native row 0 is the northernmost, so it lands in the *last* output row.
        values[0] = MISSING;
        values[1] = NO_COVERAGE;
        values[2] = f32::NAN;
        values[3] = 12.0;
        let cells = quantize(&values);
        let north_row = (GEOMETRY.height - 1) as usize * NATIVE_COLS;
        assert_eq!(cells[north_row], precip4::INTENSITY_NODATA);
        assert_eq!(cells[north_row + 1], precip4::INTENSITY_NODATA);
        assert_eq!(cells[north_row + 2], precip4::INTENSITY_NODATA);
        assert_eq!(cells[north_row + 3], precip4::quantize_rate_mm_per_hour(12.0));
        assert_eq!(cells[0], precip4::INTENSITY_DRY);
    }
}
