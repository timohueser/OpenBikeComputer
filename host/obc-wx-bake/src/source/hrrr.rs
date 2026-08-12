//! NOAA HRRR subhourly `PRATE` — the CONUS 3 km forecast, 15-minute steps.
//!
//! A source in its own right since #1246 deleted the composed `us` product it used to be the
//! forward half of. A subhourly HRRR object is ~200 MB and carries one 30-40 KB `PRATE` message
//! per 15-minute step,
//! so the baker reads the object's `.idx` text and fetches exactly the contracted message with an
//! HTTP Range request (WX1's pinned technique; NOMADS is never contacted). The `.idx` label is
//! never accepted as temporal identity — the selected lead must equal the decoded GRIB's valid
//! time minus its reference time, or the cycle fails.
//!
//! The native raster is Lambert conformal, so output cells are filled nearest-neighbour through
//! [`crate::lcc`] at the native 3 km cell size: one index map per cycle, shared by every frame,
//! no smoothing and no invented sub-cell detail. Cells outside the projected domain are no-data.

use obc_formats::precip4;
use std::fmt::Write as _;

use crate::fetch::{FetchOutcome, Upstream};
use crate::geometry::GridGeometry;
use crate::grib::{decode_field, ExpectedGrib, HRRR_CONUS_GRID_DEFINITION_HEX};
use crate::idx::{self, MAX_INDEX_BYTES};
use crate::lcc;
use crate::source::{Adapter, Attribution, BakedFrame, BakedSource, SourceClass, NOAA_TERMS_URL};

pub const ID: &str = "hrrr";

pub const BUCKET: &str = "https://noaa-hrrr-bdp-pds.s3.amazonaws.com";

pub const ATTRIBUTION: Attribution = Attribution {
    text:
        "Source: NOAA/NCEP HRRR subhourly PRATE; modified/quantized by OpenBikeComputer; no NOAA endorsement is implied",
    url: NOAA_TERMS_URL,
};

/// How far ahead of the cycle the published forward window reaches. Two hours is the timeline the
/// dataset publishes, so a lead beyond it is a lead nothing would ever sample.
pub const HORIZON_SECONDS: i64 = 2 * 3_600;

/// The published window: a regular lat/lon cover of the Lambert domain at ~3 km cells
/// (30,000 x 30,000 microdegrees — 3.34 km north-south, 2.61 km east-west at the domain's 38.5 N
/// standard parallel). Cells outside the projected raster are no-data.
///
/// **The strides are deliberately multiples of the canonical 10,000, and so is the origin**, so
/// the mosaic's cell replication onto the published lattice is an exact 3 x 3 block copy with no
/// third rounding on top of the two below. That alignment was originally bought for a different
/// reason — these frames shared a published timeline with a 1 km MRMS observation, and a client
/// assembling a bundle silently dropped any frame the coarsest one's window could not tile, which
/// on the shipped 27,000 x 34,000 lattice meant **every rider in CONUS lost the radar frame**.
/// #1237 made that a checked obligation; #1246 deleted the obligation along with the composed
/// product and the client-side assembly it protected. The alignment stays because the mosaic wants
/// it, which is a better reason than the one it was introduced for.
///
/// 30,000 in both axes is the choice that keeps the byte cost flat: 2,441 x 1,052 cells against
/// the old 2,153 x 1,168, +2 %. The alternative that undersamples least (20,000 x 30,000) costs
/// +50 % cells on every forward frame, which the corridor budget pays for by shrinking the
/// window — and a shrunken window hurts the 1 km observation this change exists to preserve.
///
/// The trade is paid in resolution, and it is worth stating as a number rather than a shrug.
/// Measured through [`crate::lcc::native_index`] over both lattices: of the 1,905,141 native 3 km
/// cells, **123,789 (6.50 %) reached no output cell on 27,000 x 34,000, and 204,758 (10.75 %)
/// reach none on 30,000 x 30,000**. So roughly one native cell in nine is now invisible in the
/// forward frames, against one in fifteen before. What makes that acceptable rather than alarming
/// is *how* the misses fall: the Lambert curvature scatters them across the **source** raster,
/// where no row and no column of the 1,799 x 1,059 native grid is fully unsampled on either
/// lattice, and no source cell is duplicated more than twice. (On the *output* side 2 of 1,052
/// rows and 1 of 2,441 columns resolve to no source index at all — but those lie wholly outside
/// the projected domain and emit NODATA, which is the correct answer rather than a gap: output
/// row 0's centre is 21.115 N, south of the domain's southernmost point at 21.138 N.) An isolated
/// convective cell can still be the one that vanishes — that is the honest residual risk of
/// nearest-neighbour resampling at this ratio, and it applies to the model frames only, never to
/// the radar observation.
///
/// Cell size in metres varies with latitude and the "~3 km" above is the standard parallel's
/// figure: the longitude cell is 2.61 km at 38.5 N, 2.03 km at the 52.6 N bulge and 3.12 km at
/// 21.1 N. `cell_size_m` stays 3,000 throughout: it states the *source's* ground resolution,
/// never the lattice.
///
/// This is a **source window**, not an output lattice (WXR3 #1242).
pub const GEOMETRY: GridGeometry = GridGeometry {
    south_lat_udeg: 21_100_000,
    west_lon_udeg: -134_100_000,
    cell_lat_udeg: 30_000,
    cell_lon_udeg: 30_000,
    width: 2_441,
    height: 1_052,
    cell_size_m: 3_000,
    tile_edge: 32,
    entries_per_page: 512,
};

/// Retained forward leads in minutes: 15-minute steps through +4 h, held in the four subhourly
/// objects `wrfsubhf01..f04`. The published set is the sub-window that lies ahead of the cycle
/// anchor, so a run up to two hours old still supplies a full +2 h of forward frames.
pub const LEADS_MIN: [u32; 16] = [15, 30, 45, 60, 75, 90, 105, 120, 135, 150, 165, 180, 195, 210, 225, 240];
/// Objects the run-completeness probe requires (`wrfsubhf01` ... `wrfsubhf04`).
pub const SUBHOURLY_FILES: [u32; 4] = [1, 2, 3, 4];
/// How many hourly runs back discovery will look before giving up.
pub const MAX_RUN_CANDIDATES: usize = 6;
/// One `PRATE` message is ~35 KB; the cap is generous but bounded well below a whole object.
const MAX_MESSAGE_BYTES: u64 = 4 * 1024 * 1024;

/// The pinned WX1 field contract (public so tests decode through the same contract).
pub const EXPECTED: ExpectedGrib = ExpectedGrib {
    discipline: 0,
    category: 1,
    parameter: 7,
    grid_template: 30,
    expected_points: (lcc::NATIVE_COLS * lcc::NATIVE_ROWS) as usize,
    expected_grid_definition_hex: HRRR_CONUS_GRID_DEFINITION_HEX,
    product_template: 0,
    representation_templates: &[3],
    missing_sentinels: &[],
    allowed_messages: &[1],
    require_identical_messages: false,
};

/// The subhourly object holding lead `lead_minutes` (`f01` holds 15...60, `f02` 75...120, ...).
pub fn subhourly_file(lead_minutes: u32) -> u32 {
    lead_minutes.div_ceil(60)
}

pub fn object_url(run: i64, file: u32) -> String {
    let run_time = chrono::DateTime::from_timestamp(run, 0).expect("run timestamp");
    let mut url = String::from(BUCKET);
    let _ = write!(
        url,
        "/hrrr.{}/conus/hrrr.t{}z.wrfsubhf{file:02}.grib2",
        run_time.format("%Y%m%d"),
        run_time.format("%H")
    );
    url
}

pub fn index_url(run: i64, file: u32) -> String {
    format!("{}.idx", object_url(run, file))
}

/// The `.idx` selector for one lead. The trailing colon matters: without it `0-1` would also
/// match `0-10`, and `15 min` would match `150 min`.
pub fn selector(lead_minutes: u32) -> String {
    format!(":PRATE:surface:{lead_minutes} min fcst:")
}

/// Hourly run candidates at or before `now`, newest first.
fn candidate_runs(now: i64) -> Vec<i64> {
    let newest = now - now.rem_euclid(3_600);
    (0..MAX_RUN_CANDIDATES as i64).map(|back| newest - back * 3_600).collect()
}

/// The newest run whose whole subhourly set is published. Completeness is probed before any
/// body moves, so a partially published run can never produce a partial product.
pub fn select_run(upstream: &mut dyn Upstream, now: i64) -> Result<Option<i64>, String> {
    'candidates: for run in candidate_runs(now) {
        // Probe the last file first: objects appear in lead order, so `f04` missing is the
        // cheapest possible proof that a run is still publishing.
        for file in SUBHOURLY_FILES.iter().rev() {
            if !upstream.exists(&index_url(run, *file))? {
                continue 'candidates;
            }
        }
        return Ok(Some(run));
    }
    Ok(None)
}

/// The leads of `run` whose valid times lie strictly after `anchor` and no more than
/// `horizon_seconds` beyond it — the forward window this source contributes.
pub fn published_leads(run: i64, anchor: i64, horizon_seconds: i64) -> Vec<u32> {
    LEADS_MIN
        .iter()
        .copied()
        .filter(|lead| {
            let valid_at = run + i64::from(*lead) * 60;
            valid_at > anchor && valid_at <= anchor + horizon_seconds
        })
        .collect()
}

/// Fetch, decode and bake the forward frames of `run` that lie ahead of `anchor`.
///
/// Each frame's `offset_min` is its real distance ahead of `anchor` and its `valid_at` is its own
/// upstream validity — never a re-stamped or interpolated cadence.
pub fn bake_forward_frames(
    upstream: &mut dyn Upstream,
    run: i64,
    anchor: i64,
    leads: &[u32],
) -> Result<Vec<BakedFrame>, String> {
    GEOMETRY.validate()?;
    if leads.is_empty() {
        return Ok(Vec::new());
    }
    let index_map = source_index_map();
    let mut frames = Vec::with_capacity(leads.len());
    // One `.idx` document and one HEAD per subhourly object, however many leads it serves.
    let mut cached: Option<(u32, u64, String)> = None;
    for lead in leads {
        let file = subhourly_file(*lead);
        if cached.as_ref().is_none_or(|(cached_file, _, _)| *cached_file != file) {
            let object = object_url(run, file);
            let object_len = upstream
                .content_length(&object)?
                .ok_or_else(|| format!("HRRR object {object} vanished between discovery and fetch"))?;
            let index = match upstream.fetch(&index_url(run, file), MAX_INDEX_BYTES, None)? {
                FetchOutcome::Body(fetched) => fetched,
                FetchOutcome::Unchanged => return Err("HRRR index fetch returned 304 without a validator".into()),
            };
            let text = String::from_utf8(index.bytes).map_err(|_| "HRRR .idx is not UTF-8".to_string())?;
            cached = Some((file, object_len, text));
        }
        let (_, object_len, index) = cached.as_ref().expect("just populated");
        let (range, _) = idx::resolve(index, &selector(*lead), *object_len, &[1])?;
        let message =
            upstream.fetch_range(&object_url(run, file), range.start, range.end_inclusive, MAX_MESSAGE_BYTES)?;
        let field = decode_field(&message.bytes, &EXPECTED)?;
        let valid_at = run + i64::from(*lead) * 60;
        // The selected lead must equal the byte-derived one; index text is not identity.
        if field.reference_unix_seconds != run
            || field.valid_start_unix_seconds != valid_at
            || field.valid_end_unix_seconds != valid_at
        {
            return Err(format!("HRRR +{lead} min message disagrees with its own GRIB timestamps"));
        }
        if field.values.len() != (lcc::NATIVE_COLS * lcc::NATIVE_ROWS) as usize {
            return Err("HRRR field does not have the contracted point count".into());
        }
        let offset_seconds = valid_at - anchor;
        if offset_seconds <= 0 || offset_seconds % 60 != 0 {
            return Err(format!("HRRR +{lead} min frame is not a positive whole-minute offset from the anchor"));
        }
        frames.push(BakedFrame {
            offset_min: u32::try_from(offset_seconds / 60).map_err(|_| "HRRR frame offset overflows")?,
            valid_at,
            class: SourceClass::Forecast,
            cells: resample(&field.values, &index_map),
        });
    }
    Ok(frames)
}

pub struct Hrrr;

impl Adapter for Hrrr {
    fn id(&self) -> &'static str {
        ID
    }

    fn bake(&self, upstream: &mut dyn Upstream, now: i64, warnings: &mut Vec<String>) -> Result<BakedSource, String> {
        let run = select_run(upstream, now)?.ok_or("no complete HRRR subhourly run among the recent cycles")?;
        // Anchored on the run, not on the wall clock: a frame's offset is its real lead, and the
        // mosaic places it by `valid_at` regardless. Leads already behind `now` are dropped here
        // rather than fetched and then ignored.
        let leads = published_leads(run, now, HORIZON_SECONDS);
        if leads.is_empty() {
            warnings.push(format!("hrrr: run {run} has no lead inside the +{HORIZON_SECONDS} s window ahead of {now}"));
        }
        let frames = bake_forward_frames(upstream, run, run, &leads)?;
        // HRRR's sub-hourly leads are already on the 15-minute cadence, so `derive` leaves it alone.
        Ok(BakedSource {
            id: ID,
            geometry: GEOMETRY,
            reference_time: run,
            attribution: ATTRIBUTION,
            frames,
            motion_history: Vec::new(),
        })
    }
}

/// The per-cycle nearest-neighbour map: for every output cell, the native raster index (or
/// `u32::MAX` outside the projected domain). One projection pass, shared by all frames.
fn source_index_map() -> Vec<u32> {
    let mut map = vec![u32::MAX; GEOMETRY.cells()];
    for row in 0..GEOMETRY.height {
        let lat = GEOMETRY.center_lat_deg(row);
        for col in 0..GEOMETRY.width {
            let lon = GEOMETRY.center_lon_deg(col);
            if let Some(index) = lcc::native_index(lat, lon) {
                map[(row * GEOMETRY.width + col) as usize] = index as u32;
            }
        }
    }
    map
}

fn resample(values: &[f32], index_map: &[u32]) -> Vec<u8> {
    index_map
        .iter()
        .map(|&source| {
            if source == u32::MAX {
                return precip4::INTENSITY_NODATA;
            }
            // WX1's pinned unit is kg/m2/s, numerically mm/s: an mm/hour rate is x 3,600.
            precip4::quantize_rate_mm_per_hour(f64::from(values[source as usize]) * 3_600.0)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_published_window_covers_the_whole_lambert_domain() {
        GEOMETRY.validate().expect("geometry is within the OBCG limits");
        // The domain's extreme corners and its northern bulge on the central meridian all fall
        // inside the window (computed from the pinned projection, not from prose).
        for (lat, lon) in [
            (21.138_123, -122.719_528),
            (21.140_547, -72.289_718),
            (47.838_623, -134.095_480),
            (47.842_195, -60.917_193),
            (52.615_653, -97.5),
        ] {
            let lat_udeg = (lat * 1e6) as i64;
            let lon_udeg = (lon * 1e6) as i64;
            assert!(
                lat_udeg >= i64::from(GEOMETRY.south_lat_udeg) && lat_udeg <= GEOMETRY.north_lat_udeg(),
                "latitude {lat} is outside the published window"
            );
            assert!(
                lon_udeg >= i64::from(GEOMETRY.west_lon_udeg) && lon_udeg <= GEOMETRY.east_lon_udeg(),
                "longitude {lon} is outside the published window"
            );
        }
    }

    #[test]
    fn leads_map_to_their_subhourly_objects() {
        assert_eq!(subhourly_file(15), 1);
        assert_eq!(subhourly_file(60), 1);
        assert_eq!(subhourly_file(75), 2);
        assert_eq!(subhourly_file(120), 2);
        assert_eq!(subhourly_file(240), 4);
        assert_eq!(
            object_url(crate::timefmt::parse_rfc3339("2026-08-09T15:00:00Z").unwrap(), 2),
            "https://noaa-hrrr-bdp-pds.s3.amazonaws.com/hrrr.20260809/conus/hrrr.t15z.wrfsubhf02.grib2"
        );
        assert_eq!(selector(15), ":PRATE:surface:15 min fcst:");
    }

    /// The forward window is the real one: strictly ahead of the observation anchor, capped at
    /// the horizon, and never re-spaced onto a fabricated cadence.
    #[test]
    fn published_leads_are_the_real_ones_ahead_of_the_anchor() {
        let run = 1_800_000_000; // an exact hour
        let anchor = run + 118 * 60; // an observation 118 minutes into the run
        let leads = published_leads(run, anchor, 2 * 3_600);
        assert_eq!(leads, vec![120, 135, 150, 165, 180, 195, 210, 225]);
        // A fresh run supplies its own first steps.
        assert_eq!(published_leads(run, run + 60, 2 * 3_600), vec![15, 30, 45, 60, 75, 90, 105, 120]);
        // An old run supplies fewer frames rather than inventing any.
        assert_eq!(published_leads(run, run + 230 * 60, 2 * 3_600), vec![240]);
        assert!(published_leads(run, run + 245 * 60, 2 * 3_600).is_empty());
    }
}
