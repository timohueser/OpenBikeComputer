//! NOAA MRMS `PrecipRate` — the CONUS radar observation, 1 km at a two-minute cadence.
//!
//! This module owns the observation half of the composed US product ([`crate::source::us`]): it
//! discovers the newest published two-minute object on NOAA Open Data Dissemination, decodes the
//! gzipped GRIB2 against WX1's pinned contract, and quantizes it onto the native 0.01-degree
//! lattice. The MRMS grid is already a regular lat/lon grid whose points are the cell centres of
//! a clean 0.01-degree window, so the only "reprojection" is the row flip from the GRIB's
//! north-to-south scan into OBCG's south-to-north row order: an exact one-to-one remap, no
//! resampling, no interpolation.
//!
//! The whole 7,000 x 3,500 field is held in RAM (about 98 MB of `f32` plus 24.5 MB of quantized
//! cells) — the deliberate sizing WX1's 161 MB spike measurement called for, and the reason the
//! baker never runs two MRMS decodes concurrently.

use obc_formats::obcg::{FLAG_OBSERVED, PRODUCT_MRMS, TIER_RADAR};
use obc_formats::precip4;
use std::fmt::Write as _;

use crate::fetch::{FetchOutcome, Upstream};
use crate::geometry::GridGeometry;
use crate::grib::{decode_gzip_field, ExpectedGrib, MAX_COMPRESSED_BYTES, MRMS_CONUS_GRID_DEFINITION_HEX};
use crate::source::{BakedFrame, FrameSource};

pub const BUCKET: &str = "https://noaa-mrms-pds.s3.amazonaws.com";

/// The published window, stated as OBCG geometry: 7,000 x 3,500 cells of 0.01 degrees whose
/// **edges** are the clean multiples 20 N/55 N and 130 W/60 W, so the GRIB's first point
/// (54.995 N, 230.005 E) is the centre of the north-west cell. MRMS is CONUS-only in WX1's
/// contract: Alaska, Hawaii and Puerto Rico are separate measured decisions, never a clamp of
/// this grid.
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

/// Fetch and bake the observation frame valid at `valid_at`, as frame 0 of the composed product
/// anchored at that same instant.
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
    Ok(BakedFrame {
        offset_min: 0,
        valid_at,
        flags: FLAG_OBSERVED,
        source: Some(FrameSource { product_code: PRODUCT_MRMS, tier: TIER_RADAR, geometry: GEOMETRY }),
        cells: quantize(&field.values),
    })
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
        let valid_at = crate::manifest::parse_rfc3339("2026-08-09T16:58:00Z").unwrap();
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
