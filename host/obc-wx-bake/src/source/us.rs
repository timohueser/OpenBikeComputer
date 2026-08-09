//! The composed US product: an MRMS radar observation followed by HRRR forward frames.
//!
//! One product, two upstreams, **no resampling anywhere**. OBCG stores geometry per object and
//! the manifest restates it per frame, so a 1 km observation and 3 km model frames live in the
//! same timeline at their own native cell sizes (`OBCG_Spec.md` preamble and §10). The frames
//! also carry their own product registry code and tier, so the seam is visible in the bytes:
//! frame 0 is `mrms`/tier 1/observed, the forward frames are `hrrr`/tier 2/forecast.
//!
//! Timeline shape, all real upstream timestamps:
//!
//! ```text
//! reference_time = the MRMS observation instant  (frame f0, observed, 1 km)
//! forward frames = the HRRR run's 15-minute steps that lie ahead of it, at their own valid
//!                  times (f<minutes ahead of the observation>, forecast, 3 km)
//! ```
//!
//! Because the product's reference time is its observation anchor, a frame's `offset_min` is its
//! true distance ahead of that observation — which is also what OBCG requires, since one product
//! has one `<generated-utc>` key segment and non-negative frame offsets. The consequence, stated
//! plainly: the HRRR frames' objects carry the anchor as their reference time, not the HRRR model
//! run time; their valid times, product code and tier remain their own. Nothing is interpolated,
//! re-spaced or re-stamped, and the two sides are never blended into one fictional model run.

use obc_formats::obcg::{PRODUCT_MRMS, TIER_RADAR};

use crate::fetch::Upstream;
use crate::manifest::Product;
use crate::source::{mrms, Adapter, AdapterOutcome, Attribution, BakedProduct, NOAA_TERMS_URL};
use crate::source::hrrr;

pub const ID: &str = "us";

/// How far ahead of the observation the published forward window reaches.
pub const HORIZON_SECONDS: i64 = 2 * 3_600;
/// The most forward frames the horizon can hold at HRRR's 15-minute step.
pub const FULL_FORWARD_FRAMES: usize = 8;
/// MRMS refreshes every two minutes; half an hour without a fresh observation is the epic's
/// stuck-baker horizon, so the product must not outlive it (the same rule as `dwd-rv`).
pub const STALENESS_SECONDS: i64 = 30 * 60;

pub const ATTRIBUTION: Attribution = Attribution {
    text: "Source: NOAA/NCEP MRMS PrecipRate (observation) and HRRR (forecast); modified/quantized by OpenBikeComputer; no NOAA endorsement is implied",
    url: NOAA_TERMS_URL,
};

pub struct UsComposite;

impl Adapter for UsComposite {
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
        // The observation anchors everything, so it is discovered first — by HEAD probes only,
        // which makes the unchanged short-circuit cost nothing but a request.
        let observation = mrms::discover_latest(upstream, now)?
            .ok_or("no MRMS observation published within the discovery window")?;
        let previous_reference = previous.and_then(|product| product.reference_unix());
        if previous_reference == Some(observation) {
            return Ok(AdapterOutcome::Unchanged);
        }
        // Upstream regression: the newest observation is older than the one already published (a
        // withdrawn object, or a clock going backwards). Never move reference_time or the
        // staleness deadline into the past while published frames stand.
        if previous_reference.is_some_and(|published| published > observation) {
            warnings.push(format!(
                "us: newest MRMS observation {observation} is older than the published {}; keeping the published product",
                previous_reference.expect("checked")
            ));
            return Ok(AdapterOutcome::Unchanged);
        }

        let run = hrrr::select_run(upstream, now)?
            .ok_or("no complete HRRR subhourly run among the recent cycles")?;
        let leads = hrrr::published_leads(run, observation, HORIZON_SECONDS);
        if leads.len() < FULL_FORWARD_FRAMES {
            // Honest degradation: a late HRRR publication shortens the forward window instead of
            // inventing frames. Zero forward frames still publishes the observation.
            warnings.push(format!(
                "us: HRRR run {run} supplies {} of {FULL_FORWARD_FRAMES} forward frames ahead of observation {observation}",
                leads.len()
            ));
        }

        let mut frames = Vec::with_capacity(1 + leads.len());
        frames.push(mrms::bake_observation(upstream, observation)?);
        frames.extend(hrrr::bake_forward_frames(upstream, run, observation, &leads)?);

        Ok(AdapterOutcome::Baked(Box::new(BakedProduct {
            id: ID,
            // The product-level provenance and lattice describe the anchor frame; every frame
            // carries its own (`BakedFrame::source`), and the manifest restates them per frame.
            product_code: PRODUCT_MRMS,
            tier: TIER_RADAR,
            geometry: mrms::GEOMETRY,
            reference_time: observation,
            staleness_deadline: observation + STALENESS_SECONDS,
            attribution: ATTRIBUTION,
            upstream_etag: None,
            frames,
        })))
    }
}
