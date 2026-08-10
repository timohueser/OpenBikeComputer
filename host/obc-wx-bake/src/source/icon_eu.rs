//! DWD ICON-EU adapter: Europe's 0.0625-degree hourly forecast, tier 2.
//!
//! The newest run whose whole retained lead set exists is selected — never wall-clock
//! arithmetic alone. Cumulative `TOT_PREC` is de-accumulated to hourly rates between
//! consecutive leads of the same run; a negative difference within the packing-roundoff bound
//! is dry, anything larger fails the cycle (WX1's tightly bounded rule — no clamping of real
//! decreases). The native grid is already regular lat/lon, so "reprojection" is the identity;
//! no smoothing, no resampling.

use obc_formats::obcg::{FLAG_FORECAST, PRODUCT_ICON_EU, TIER_MODEL};
use obc_formats::precip4;

use crate::fetch::{FetchOutcome, Upstream};
use crate::geometry::GridGeometry;
use crate::grib::{decode_bzip2_field, DecodedField, ExpectedGrib, ICON_EU_GRID_DEFINITION_HEX, MAX_COMPRESSED_BYTES};
use crate::manifest::Product;
use crate::source::{Adapter, AdapterOutcome, Attribution, BakedFrame, BakedProduct};

pub const ID: &str = "icon-eu";

/// The **source window**: the native grid restated as geometry, with cell centres from
/// (29.5 N, -23.5 E) on exact 0.0625-degree strides, so the south/west **edges** sit half a cell
/// lower. GRIB scanning is +i west-east, +j south-north — identical to OBCG's row order, no
/// reindexing.
///
/// The window stays at the native 6.5 km pitch (WXR3 #1242): eagerly upsampling a continental
/// model onto the canonical 1 km lattice would cost 28 M cells a frame for no information, so the
/// mosaic cell-replicates it lazily, one shard at a time. `cell_size_m` here states the source's
/// true ground resolution; the *published* frames state the lattice's.
pub const GEOMETRY: GridGeometry = GridGeometry {
    south_lat_udeg: 29_468_750,
    west_lon_udeg: -23_531_250,
    cell_lat_udeg: 62_500,
    cell_lon_udeg: 62_500,
    width: 1_377,
    height: 657,
    cell_size_m: 6_500,
    tile_edge: 16,
    entries_per_page: 512,
};

/// Retained forward leads in hours. Twelve hourly frames cover +2 h of forward frames at any
/// wall-clock moment before the staleness deadline: runs land at most ~4 h after their
/// reference, the next run at latest ~10 h after, and `10 h + 2 h = 12 h`.
pub const LEADS_H: [u32; 12] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
pub const STALENESS_SECONDS: i64 = 10 * 3_600;

pub const ATTRIBUTION: Attribution = Attribution {
    text: "Source: Deutscher Wetterdienst (DWD), ICON-EU; modified/quantized by OpenBikeComputer",
    url: "https://www.dwd.de/EN/service/copyright/copyright_artikel.html",
};

/// The pinned WX1 field contract (public so tests decode through the same contract).
pub const EXPECTED: ExpectedGrib = ExpectedGrib {
    discipline: 0,
    category: 1,
    parameter: 52,
    grid_template: 0,
    expected_points: 904_689,
    expected_grid_definition_hex: ICON_EU_GRID_DEFINITION_HEX,
    product_template: 8,
    representation_templates: &[42],
    missing_sentinels: &[],
    allowed_messages: &[1],
    require_identical_messages: false,
};

const CYCLE_HOURS: [u32; 4] = [0, 6, 12, 18];

pub fn lead_url(run: i64, lead_hours: u32) -> String {
    let run_time = chrono::DateTime::from_timestamp(run, 0).expect("run timestamp");
    format!(
        "https://opendata.dwd.de/weather/nwp/icon-eu/grib/{:02}/tot_prec/icon-eu_europe_regular-lat-lon_single-level_{}_{:03}_TOT_PREC.grib2.bz2",
        run_time.format("%H"),
        run_time.format("%Y%m%d%H"),
        lead_hours
    )
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

pub struct IconEu;

impl Adapter for IconEu {
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
        // Newest complete run: every retained lead (plus the f000 baseline) must exist before a
        // run is selectable (WX1). Candidates are probed newest-first.
        let mut selected = None;
        'candidates: for run in candidate_runs(now) {
            for lead in std::iter::once(0).chain(LEADS_H) {
                if !upstream.exists(&lead_url(run, lead))? {
                    continue 'candidates;
                }
            }
            selected = Some(run);
            break;
        }
        let run = selected.ok_or("no complete ICON-EU run among the recent cycles")?;
        let previous_run = previous.and_then(|product| product.reference_unix());
        if previous_run == Some(run) {
            return Ok(AdapterOutcome::Unchanged);
        }
        // Upstream regression: the newest complete run is older than the one already published
        // (files withdrawn upstream). Never re-bake backwards — reference_time and the staleness
        // deadline must not move into the past while published frames stand.
        if previous_run.is_some_and(|published| published > run) {
            warnings.push(format!(
                "icon-eu: newest complete run {run} is older than the published {}; keeping the published product",
                previous_run.expect("checked")
            ));
            return Ok(AdapterOutcome::Unchanged);
        }

        // Fetch and decode the cumulative fields, then de-accumulate consecutive pairs.
        let mut previous_field: Option<(u32, DecodedField)> = None;
        let mut frames = Vec::with_capacity(LEADS_H.len());
        for lead in std::iter::once(0).chain(LEADS_H) {
            let fetched = match upstream.fetch(&lead_url(run, lead), MAX_COMPRESSED_BYTES, None)? {
                FetchOutcome::Body(fetched) => fetched,
                FetchOutcome::Unchanged => return Err("ICON-EU lead fetch returned 304 without a validator".into()),
            };
            let field = decode_bzip2_field(&fetched.bytes, &EXPECTED)?;
            if field.reference_unix_seconds != run {
                return Err(format!("ICON-EU f{lead:03} reference time is not the selected run"));
            }
            if field.valid_start_unix_seconds != run {
                return Err("cumulative ICON-EU TOT_PREC does not start at the model reference time".into());
            }
            if field.valid_end_unix_seconds != run + i64::from(lead) * 3_600 {
                return Err(format!("ICON-EU f{lead:03} interval end does not match its lead"));
            }
            if let Some((previous_lead, earlier)) = previous_field.take() {
                if previous_lead + 1 != lead {
                    return Err("ICON-EU leads are not consecutive".into());
                }
                frames.push(deaccumulate(&earlier, &field, run, lead)?);
            }
            previous_field = Some((lead, field));
        }
        Ok(AdapterOutcome::Baked(Box::new(BakedProduct {
            id: ID,
            product_code: PRODUCT_ICON_EU,
            tier: TIER_MODEL,
            geometry: GEOMETRY,
            reference_time: run,
            staleness_deadline: run + STALENESS_SECONDS,
            attribution: ATTRIBUTION,
            upstream_etag: None,
            frames,
        })))
    }
}

/// One hourly rate frame from two consecutive cumulative fields (public for the fixture tests).
pub fn deaccumulate(earlier: &DecodedField, later: &DecodedField, run: i64, lead: u32) -> Result<BakedFrame, String> {
    if earlier.values.len() != later.values.len() || later.values.len() != GEOMETRY.cells() {
        return Err("ICON-EU cumulative fields disagree on geometry".into());
    }
    // WX1's exact roundoff rule: independently packed cumulative fields may disagree by at most
    // half the sum of their packing increments; treat only that as dry roundoff.
    let roundoff_limit = f64::from(earlier.packing_increment + later.packing_increment) / 2.0;
    let mut cells = Vec::with_capacity(GEOMETRY.cells());
    for (index, (&earlier_mm, &later_mm)) in earlier.values.iter().zip(&later.values).enumerate() {
        let delta = f64::from(later_mm) - f64::from(earlier_mm);
        if delta < -roundoff_limit {
            return Err(format!(
                "ICON-EU cumulative precipitation decreased by {} mm at cell {index} (f{lead:03})",
                -delta
            ));
        }
        // One hour between interval ends, so the mm delta is numerically an mm/h rate.
        cells.push(precip4::quantize_rate_mm_per_hour(delta.max(0.0)));
    }
    Ok(BakedFrame {
        offset_min: lead * 60,
        valid_at: run + i64::from(lead) * 3_600,
        flags: FLAG_FORECAST,
        source: None,
        cells,
    })
}
