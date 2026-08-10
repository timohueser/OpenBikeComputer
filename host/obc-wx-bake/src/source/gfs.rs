//! NOAA GFS `APCP` — the worldwide tier-3 floor, 0.25 degrees, hourly forward frames.
//!
//! Every rideable coordinate on Earth is covered by this product; the radar and model tiers sit
//! above it where they exist. IMERG Early is an explicit v1 NO-GO (WX1), so the floor is
//! GFS-only: it never synthesizes an observation frame, never backdates a forecast, and never
//! emits an empty frame that would look like dry weather.
//!
//! Fetching is the `.idx` byte-range path (`crate::idx`): a GFS 0.25-degree object is ~500 MB and
//! the contracted `APCP:surface:0-N hour acc fcst` record is 300-700 KB. NOAA currently
//! advertises that record twice for leads inside the first six-hour bucket, so the selection is
//! resolved as one consecutive span and the decoded fields must be identical — never an
//! undocumented "first occurrence".
//!
//! Accumulations are de-accumulated run-scoped: hour 1 is the `0-1` field itself (differenced
//! from zero), hour N is `0-N` minus `0-(N-1)` of the **same** run. A decrease beyond the two
//! fields' packing roundoff is a contract failure, not dry weather, and a run transition
//! republishes a whole new run rather than subtracting across the seam.
//!
//! ## Antimeridian
//!
//! OBCG v1 grids may not cross +/-180 degrees. GFS grid points sit on exact 0.25-degree
//! multiples, so the column centred on 180 degrees is a cell spanning 179.875 E to 180.125 E: no
//! conforming window can contain it, whether the globe is published as one window or split into
//! an eastern and a western one. This product therefore publishes the 1,439 columns from
//! 179.75 W to 179.75 E as **one** non-crossing global window, and the single 0.25-degree column
//! straddling the antimeridian is not published. The manifest bbox states that honestly, so a
//! corridor there finds no product rather than a fabricated one. The same reasoning drops the two
//! polar rows (a cell centred on a pole would need an edge beyond +/-90 degrees).

use obc_formats::obcg::{FLAG_FORECAST, PRODUCT_GFS, TIER_FLOOR};
use obc_formats::precip4;
use std::fmt::Write as _;

use crate::fetch::{FetchOutcome, Upstream};
use crate::geometry::GridGeometry;
use crate::grib::{decode_field, DecodedField, ExpectedGrib, GFS_GLOBAL_GRID_DEFINITION_HEX};
use crate::idx::{self, MAX_INDEX_BYTES};
use crate::manifest::Product;
use crate::source::{Adapter, AdapterOutcome, Attribution, BakedFrame, BakedProduct, NOAA_TERMS_URL};

pub const ID: &str = "gfs";
pub const BUCKET: &str = "https://noaa-gfs-bdp-pds.s3.amazonaws.com";

/// The **source window**: the GFS lattice minus the antimeridian column and the two polar rows
/// (see the module docs). Cell centres coincide **exactly** with GFS grid points, so the mapping
/// from source point to source-window cell is an integer remap — no resampling of any kind.
///
/// It stays at the native 0.25 degree pitch (WXR3 #1242). This is the mosaic's **floor**: the
/// last row of `MOSAIC_PRIORITY`, and the reason every canonical cell always carries a
/// best-available value instead of a coverage flag. Upsampling it eagerly would be 648 M cells a
/// frame, so the mosaic cell-replicates it lazily, one shard at a time — 750 canonical cells to
/// one GFS cell at the equator, which is exactly as coarse as it looks and honestly so.
pub const GEOMETRY: GridGeometry = GridGeometry {
    south_lat_udeg: -89_875_000,
    west_lon_udeg: -179_875_000,
    cell_lat_udeg: 250_000,
    cell_lon_udeg: 250_000,
    width: 1_439,
    height: 719,
    // 0.25 degrees is ~27.75 km at the equator; this is the truthful nominal resolution a client
    // shows and selects on, and the reason the floor looks visibly coarse on the device.
    cell_size_m: 27_750,
    tile_edge: 16,
    entries_per_page: 512,
};

/// The native GRIB raster: 1,440 x 721 points scanned west-east, north-south, from (90 N, 0 E).
const NATIVE_COLS: usize = 1_440;
const NATIVE_ROWS: usize = 721;

/// Retained forward leads in hours, sized so the product never stops offering +2 h of forward
/// coverage while it is the newest one published. The arithmetic is over the real operational
/// numbers, not a round guess:
///
/// ```text
/// leads >= run interval + publication delay + poll pickup lag + forward coverage
///       =      6 h      +       <= 6 h      +    1 h 05 min   +       2 h        = 15.1 h
/// ```
///
/// - **run interval 6 h**: the 00/06/12/18 Z cycles.
/// - **publication delay**: WX1 measured f003 of the 06 Z run appearing 3 h 32 min after its
///   reference, and this crate's own fixture capture found the 12 Z run's last retained lead
///   published just under 5 h after it. Six hours is the *tolerated* worst case, not the typical
///   one — a run later than that briefly costs the floor its forward window rather than its
///   existence, because the deadline below moves with the last frame.
/// - **poll pickup lag**: `ops/weather/adapters.conf` runs this adapter hourly (`*:25`) with
///   `RandomizedDelaySec=300`, so a run that completes just after a tick waits at most 1 h 05 min
///   to be picked up. A cadence change there must be re-checked against this constant — the
///   `retention_covers_two_hours_ahead_until_the_next_run_is_picked_up` test is that check.
///
/// Sixteen leads round that 15.1 h up with an hour of slack. Four extra leads over the naive
/// twelve cost ~1.9 MB of ingress and ~1 MB of published objects per run — noise against WX1's
/// 15.5 MB/run ceiling and the R2 budget, and cheap insurance for the tier every rideable
/// coordinate on Earth falls back to.
pub const LEADS_H: [u32; 16] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
/// The product must not outlive its own last retained frame — which, by the sizing above, is also
/// always later than the worst-case moment its replacement is picked up.
pub const STALENESS_SECONDS: i64 = 16 * 3_600;
/// The sizing inputs of [`LEADS_H`], public because they are an operational contract shared with
/// `ops/weather/adapters.conf`: changing the cadence there without re-checking them here is
/// exactly the drift the retention test guards against.
///
/// The `adapters.conf` pickup lag: an hourly timer plus its 300 s randomized delay.
pub const POLL_PICKUP_LAG_SECONDS: i64 = 3_600 + 300;
/// The publication delay a complete run may suffer before the floor loses its forward window.
pub const TOLERATED_PUBLICATION_DELAY_SECONDS: i64 = 6 * 3_600;
/// The forward coverage the floor promises whenever it is the newest published run.
pub const FORWARD_COVERAGE_SECONDS: i64 = 2 * 3_600;
/// The upstream cycle interval.
pub const RUN_INTERVAL_SECONDS: i64 = 6 * 3_600;
/// WX1's enforced ingress ceiling for one run's selected spans.
pub const MAX_RUN_SPAN_BYTES: u64 = 15_500_000;
/// One APCP span is 300-700 KB; the cap bounds a single range far below a whole object.
const MAX_SPAN_BYTES: u64 = 8 * 1024 * 1024;

const CYCLE_HOURS: [u32; 4] = [0, 6, 12, 18];

pub const ATTRIBUTION: Attribution = Attribution {
    text: "Source: NOAA/NCEP GFS; modified/quantized by OpenBikeComputer; no NOAA endorsement is implied",
    url: NOAA_TERMS_URL,
};

/// The pinned WX1 field contract (public so tests decode through the same contract).
pub const EXPECTED: ExpectedGrib = ExpectedGrib {
    discipline: 0,
    category: 1,
    parameter: 8,
    grid_template: 0,
    expected_points: NATIVE_COLS * NATIVE_ROWS,
    expected_grid_definition_hex: GFS_GLOBAL_GRID_DEFINITION_HEX,
    product_template: 8,
    representation_templates: &[3],
    missing_sentinels: &[],
    // NOAA advertises the contracted record once or (inside the first six-hour bucket) twice.
    allowed_messages: &[1, 2],
    require_identical_messages: true,
};

pub fn object_url(run: i64, lead_hours: u32) -> String {
    let run_time = chrono::DateTime::from_timestamp(run, 0).expect("run timestamp");
    let mut url = String::from(BUCKET);
    let _ = write!(
        url,
        "/gfs.{}/{}/atmos/gfs.t{}z.pgrb2.0p25.f{lead_hours:03}",
        run_time.format("%Y%m%d"),
        run_time.format("%H"),
        run_time.format("%H")
    );
    url
}

pub fn index_url(run: i64, lead_hours: u32) -> String {
    format!("{}.idx", object_url(run, lead_hours))
}

/// The `.idx` selector for one lead. The record is always the run-scoped `0-N` accumulation, so
/// its interval starts at the reference time even past the six-hour bucket boundary where NOAA
/// also publishes a `6-N` record. The trailing colon keeps `0-1` from matching `0-10`.
pub fn selector(lead_hours: u32) -> String {
    format!(":APCP:surface:0-{lead_hours} hour acc fcst:")
}

/// Candidate runs, newest first: the six-hourly cycles within the last ~30 hours.
fn candidate_runs(now: i64) -> Vec<i64> {
    let mut candidates = Vec::new();
    let day = now - now.rem_euclid(86_400);
    for day_offset in 0..=1 {
        for hour in CYCLE_HOURS.iter().rev() {
            let run = day - day_offset * 86_400 + i64::from(*hour) * 3_600;
            if run <= now {
                candidates.push(run);
            }
        }
    }
    candidates
}

pub struct GfsFloor;

impl Adapter for GfsFloor {
    fn id(&self) -> &'static str {
        ID
    }

    fn bake(
        &self,
        upstream: &mut dyn Upstream,
        previous: Option<&Product>,
        now: i64,
        warnings: &mut Vec<String>,
    ) -> Result<AdapterOutcome, String> {
        GEOMETRY.validate()?;
        // Newest sufficiently complete run: every retained lead's index must exist before the run
        // is selectable, which is also what keeps the baker from racing a partial publication.
        // Indexes are probed newest lead first, so an in-flight run is rejected on one request.
        let mut selected = None;
        'candidates: for run in candidate_runs(now) {
            for lead in LEADS_H.iter().rev() {
                if !upstream.exists(&index_url(run, *lead))? {
                    continue 'candidates;
                }
            }
            selected = Some(run);
            break;
        }
        let run = selected.ok_or("no complete GFS run among the recent cycles")?;
        let previous_run = previous.and_then(|product| product.reference_unix());
        if previous_run == Some(run) {
            return Ok(AdapterOutcome::Unchanged);
        }
        if previous_run.is_some_and(|published| published > run) {
            warnings.push(format!(
                "gfs: newest complete run {run} is older than the published {}; keeping the published product",
                previous_run.expect("checked")
            ));
            return Ok(AdapterOutcome::Unchanged);
        }

        let mut previous_field: Option<(u32, DecodedField)> = None;
        let mut frames = Vec::with_capacity(LEADS_H.len());
        let mut span_bytes = 0u64;
        for lead in LEADS_H {
            let object = object_url(run, lead);
            let object_len = upstream
                .content_length(&object)?
                .ok_or_else(|| format!("GFS object {object} vanished between discovery and fetch"))?;
            let index = match upstream.fetch(&index_url(run, lead), MAX_INDEX_BYTES, None)? {
                FetchOutcome::Body(fetched) => fetched,
                FetchOutcome::Unchanged => return Err("GFS index fetch returned 304 without a validator".into()),
            };
            let text = String::from_utf8(index.bytes).map_err(|_| "GFS .idx is not UTF-8".to_string())?;
            let (range, _) = idx::resolve(&text, &selector(lead), object_len, &[1, 2])?;
            span_bytes += range.len();
            if span_bytes > MAX_RUN_SPAN_BYTES {
                return Err(format!(
                    "GFS run {run} selects {span_bytes} bytes, above WX1's {MAX_RUN_SPAN_BYTES}-byte ceiling"
                ));
            }
            let span = upstream.fetch_range(&object, range.start, range.end_inclusive, MAX_SPAN_BYTES)?;
            let field = decode_field(&span.bytes, &EXPECTED)?;
            let valid_at = run + i64::from(lead) * 3_600;
            if field.reference_unix_seconds != run {
                return Err(format!("GFS f{lead:03} reference time is not the selected run"));
            }
            if field.valid_start_unix_seconds != run {
                return Err("cumulative GFS APCP does not start at the model reference time".into());
            }
            if field.valid_end_unix_seconds != valid_at {
                return Err(format!("GFS f{lead:03} interval end does not match its lead"));
            }
            frames.push(match previous_field.take() {
                // Hour 1 of a run is differenced from zero: the run's own baseline, never the
                // previous run's last field.
                None if lead == 1 => deaccumulate(None, &field, run, lead)?,
                None => return Err("GFS de-accumulation must start at the run's first lead".into()),
                Some((previous_lead, earlier)) => {
                    if previous_lead + 1 != lead {
                        return Err("GFS leads are not consecutive".into());
                    }
                    deaccumulate(Some(&earlier), &field, run, lead)?
                }
            });
            previous_field = Some((lead, field));
        }
        Ok(AdapterOutcome::Baked(Box::new(BakedProduct {
            id: ID,
            product_code: PRODUCT_GFS,
            tier: TIER_FLOOR,
            geometry: GEOMETRY,
            reference_time: run,
            staleness_deadline: run + STALENESS_SECONDS,
            attribution: ATTRIBUTION,
            upstream_etag: None,
            frames,
        })))
    }
}

/// One hourly rate frame from consecutive run-scoped cumulative fields. `earlier` is `None` only
/// for the run's first hour, whose baseline is exactly zero.
pub fn deaccumulate(
    earlier: Option<&DecodedField>,
    later: &DecodedField,
    run: i64,
    lead: u32,
) -> Result<BakedFrame, String> {
    if later.values.len() != NATIVE_COLS * NATIVE_ROWS {
        return Err("GFS cumulative field does not have the contracted point count".into());
    }
    if let Some(earlier) = earlier {
        if earlier.values.len() != later.values.len() {
            return Err("GFS cumulative fields disagree on geometry".into());
        }
        if earlier.reference_unix_seconds != later.reference_unix_seconds {
            return Err("GFS de-accumulation across two runs is forbidden".into());
        }
    }
    // WX1's exact roundoff rule: independently packed cumulative fields may disagree by at most
    // half the sum of their packing increments; a larger decrease fails rather than being clamped.
    let roundoff_limit =
        earlier.map(|earlier| f64::from(earlier.packing_increment + later.packing_increment) / 2.0).unwrap_or(0.0);
    let mut cells = Vec::with_capacity(GEOMETRY.cells());
    for row in 0..GEOMETRY.height as usize {
        // Output row 0 is the southernmost published centre (-89.75); the GRIB scans from +90 N,
        // so the native row is `height - row` and the polar rows are simply never addressed.
        let native_row = GEOMETRY.height as usize - row;
        for col in 0..GEOMETRY.width as usize {
            // Output column 0 is centred on 179.75 W = 180.25 E: native column `col + 721`,
            // wrapping at the prime meridian. The antimeridian column 720 is never addressed.
            let native_col = (col + 721) % NATIVE_COLS;
            let index = native_row * NATIVE_COLS + native_col;
            let later_mm = f64::from(later.values[index]);
            let earlier_mm = earlier.map_or(0.0, |earlier| f64::from(earlier.values[index]));
            let delta = later_mm - earlier_mm;
            if delta < -roundoff_limit {
                return Err(format!(
                    "GFS cumulative precipitation decreased by {} mm at point {index} (f{lead:03})",
                    -delta
                ));
            }
            // One hour between interval ends, so the mm delta is numerically an mm/h rate.
            cells.push(precip4::quantize_rate_mm_per_hour(delta.max(0.0)));
        }
    }
    Ok(BakedFrame {
        offset_min: lead * 60,
        valid_at: run + i64::from(lead) * 3_600,
        flags: FLAG_FORECAST,
        source: None,
        cells,
    })
}

/// The native point index a published cell samples — the exact integer remap the bake performs,
/// exposed so tests can prove the georeferencing independently.
pub fn native_index(col: u32, row: u32) -> Option<usize> {
    if col >= GEOMETRY.width || row >= GEOMETRY.height {
        return None;
    }
    let native_row = GEOMETRY.height as usize - row as usize;
    let native_col = (col as usize + 721) % NATIVE_COLS;
    Some(native_row * NATIVE_COLS + native_col)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_global_window_is_conforming_and_centred_on_the_gfs_lattice() {
        GEOMETRY.validate().expect("geometry is within the OBCG limits");
        assert_eq!(GEOMETRY.north_lat_udeg(), 89_875_000);
        assert_eq!(GEOMETRY.east_lon_udeg(), 179_875_000);
        // Every published cell centre is an exact GFS grid point.
        for col in [0u32, 1, 719, 720, 1_438] {
            let centre = GEOMETRY.center_lon_deg(col);
            assert!((centre * 4.0 - (centre * 4.0).round()).abs() < 1e-9, "centre {centre} is off-lattice");
        }
        for row in [0u32, 1, 359, 718] {
            let centre = GEOMETRY.center_lat_deg(row);
            assert!((centre * 4.0 - (centre * 4.0).round()).abs() < 1e-9, "centre {centre} is off-lattice");
        }
        assert!((GEOMETRY.center_lon_deg(0) - -179.75).abs() < 1e-9);
        assert!((GEOMETRY.center_lon_deg(719) - 0.0).abs() < 1e-9);
        assert!((GEOMETRY.center_lon_deg(1_438) - 179.75).abs() < 1e-9);
        assert!((GEOMETRY.center_lat_deg(0) - -89.75).abs() < 1e-9);
        assert!((GEOMETRY.center_lat_deg(718) - 89.75).abs() < 1e-9);
    }

    /// The published lattice maps back onto the native scan exactly: south-to-north row flip and
    /// a prime-meridian column rotation, with the antimeridian column never addressed.
    #[test]
    fn the_native_remap_is_an_exact_integer_mapping() {
        // (-89.75, -179.75) is native row 719, column 721.
        assert_eq!(native_index(0, 0), Some(719 * NATIVE_COLS + 721));
        // The prime meridian: output column 719 is native column 0.
        assert_eq!(native_index(719, 0), Some(719 * NATIVE_COLS));
        // (89.75, 179.75) is native row 1, column 719.
        assert_eq!(native_index(1_438, 718), Some(NATIVE_COLS + 719));
        assert_eq!(native_index(GEOMETRY.width, 0), None);
        assert_eq!(native_index(0, GEOMETRY.height), None);
        // No published cell ever samples the antimeridian column or a pole row.
        let mut sampled = std::collections::BTreeSet::new();
        for col in 0..GEOMETRY.width {
            sampled.insert(native_index(col, 0).unwrap() % NATIVE_COLS);
        }
        assert!(!sampled.contains(&720), "the antimeridian column must not be published");
        assert_eq!(sampled.len(), GEOMETRY.width as usize, "columns must not be published twice");
        for row in 0..GEOMETRY.height {
            let native_row = native_index(0, row).unwrap() / NATIVE_COLS;
            assert!((1..=NATIVE_ROWS - 2).contains(&native_row), "pole row {native_row} must not be published");
        }
    }

    /// The retention promise, pinned at its worst wall-clock moment: a run that lands 5 h 55 min
    /// late is not picked up until the following hourly tick plus its randomized delay, and right
    /// up to that instant the *previous* run must still offer two hours of forward frames — and
    /// must not have expired underneath them.
    #[test]
    fn retention_covers_two_hours_ahead_until_the_next_run_is_picked_up() {
        let run = 0i64;
        let next_run = run + RUN_INTERVAL_SECONDS;
        // The worst tolerated moment the replacement becomes visible to a client.
        let pickup = next_run + TOLERATED_PUBLICATION_DELAY_SECONDS + POLL_PICKUP_LAG_SECONDS;
        let last_frame = run + i64::from(*LEADS_H.last().expect("leads are non-empty")) * 3_600;
        assert!(
            last_frame >= pickup + FORWARD_COVERAGE_SECONDS,
            "the last retained frame {last_frame} is inside the +2 h window of the worst-case pickup {pickup}"
        );
        // And the product must not expire before its replacement arrives, or the worldwide floor
        // — the tier with no fallback beneath it — briefly vanishes.
        assert!(run + STALENESS_SECONDS >= pickup, "the floor expires before its replacement is picked up");
        // The review's named scenario, spelled out: a run 5 h 55 min late, evaluated one second
        // before the tick that picks it up.
        let late_replacement = next_run + 5 * 3_600 + 55 * 60;
        let evaluated_at = late_replacement + POLL_PICKUP_LAG_SECONDS - 1;
        assert!(last_frame >= evaluated_at + FORWARD_COVERAGE_SECONDS);
        assert!(run + STALENESS_SECONDS > evaluated_at);
        // Leads are the consecutive hours the de-accumulation walk requires.
        assert!(LEADS_H.windows(2).all(|pair| pair[1] == pair[0] + 1) && LEADS_H[0] == 1);
    }

    #[test]
    fn selectors_and_keys_follow_the_pinned_schema() {
        let run = crate::manifest::parse_rfc3339("2026-08-09T12:00:00Z").unwrap();
        assert_eq!(
            object_url(run, 3),
            "https://noaa-gfs-bdp-pds.s3.amazonaws.com/gfs.20260809/12/atmos/gfs.t12z.pgrb2.0p25.f003"
        );
        assert_eq!(selector(1), ":APCP:surface:0-1 hour acc fcst:");
        assert_eq!(selector(12), ":APCP:surface:0-12 hour acc fcst:");
    }
}
