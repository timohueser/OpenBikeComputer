//! OPERA NIMBUS: Europe's 2 km instantaneous rain rate, every fifteen minutes (WXR6, #1245).
//!
//! NIMBUS is the honest, cheap half of the European pair: 1,900 x 2,200 cells of 2 km, already in
//! mm/h, published ten minutes after its valid time on exactly the 15-minute cadence the bucket
//! runs at. No Z-R decision is made here at all — OPERA has already made it, and this adapter
//! pins the `zr_a`/`zr_b` it declares so that [`super::opera_cirrus`], which must convert, uses
//! the same relation.
//!
//! It sits **below** CIRRUS in the mosaic, and it earns its place there twice over. It reaches
//! about 23 k of its 2 km cells (roughly 93,000 km²) that CIRRUS did not cover in the frame pair
//! measured on 2026-08-10, and it is a near-surface `PPI` rather than a column maximum, so where
//! CIRRUS drops out NIMBUS is not a degraded fallback but the physically closer measurement.
//!
//! The rejected third option is OPERA's `ACRR`, which the same key schema publishes. It is on the
//! same 2 km grid, and it is a **one-hour** accumulation (`prodname` says so; `startdate`/
//! `starttime` sit an hour before `enddate`/`endtime`). Dividing an hour of accumulation into a
//! rate smears a moving shower across sixty minutes of track, which is exactly the error a rider
//! is using radar to avoid.

use obc_formats::obcg::PRODUCT_OPERA_NIMBUS;

use crate::fetch::Upstream;
use crate::manifest::Product;
use crate::source::opera::{self, Contract, Quantity, OPERA_TERMS_URL};
use crate::source::{Adapter, AdapterOutcome, Attribution};

pub const ID: &str = "opera-nimbus";

/// Three times the fifteen-minute cadence: long enough that one skipped publication is not an
/// outage, short enough that a rider is never shown a rain field from an hour ago.
pub const STALENESS_SECONDS: i64 = 45 * 60;

pub const ATTRIBUTION: Attribution = Attribution {
    text: "Source: EUMETNET OPERA NIMBUS instantaneous rain rate composite (CC BY 4.0); modified/quantized by OpenBikeComputer",
    url: OPERA_TERMS_URL,
};

/// The pinned source contract, every field measured off the live objects on 2026-08-10.
pub const CONTRACT: Contract = Contract {
    id: ID,
    product_code: PRODUCT_OPERA_NIMBUS,
    quantity: Quantity::RainRate,
    prodname: "OPERA NIMBUS instantaneous rain rate composite",
    width: 1_900,
    height: 2_200,
    cell_m: 2_000.0,
    // The same registration as CIRRUS at twice the pitch: cell centres coincide exactly with the
    // even CIRRUS cells, so the two products describe one grid at two resolutions.
    ul_x: -1_000.000_271_433_265_9,
    ul_y: 999.999_912_387_225_8,
    cadence_seconds: 900,
    // Seventy-five minutes back, against a measured 10-minute publication lag.
    max_discovery_probes: 5,
    staleness_seconds: STALENESS_SECONDS,
    attribution: ATTRIBUTION,
};

pub struct OperaNimbus;

impl Adapter for OperaNimbus {
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
        opera::bake(&CONTRACT, upstream, previous, now, warnings)
    }
}
