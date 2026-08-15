//! OPERA NIMBUS: Europe's 2 km instantaneous rain rate, every fifteen minutes (WXR6, #1245).
//!
//! NIMBUS is the honest, cheap half of the European pair: 1,900 x 2,200 cells of 2 km, already in
//! mm/h, published ten minutes after its valid time on exactly the 15-minute cadence the bucket
//! runs at. No Z-R decision is made here at all — OPERA has already made it, and this adapter
//! pins the `zr_a`/`zr_b` it declares so that [`super::opera_cirrus`], which must convert, uses
//! the same relation.
//!
//! It sits **below** CIRRUS in the mosaic, and it earns its place there three times over. It
//! reaches about 23 k of its 2 km cells (roughly 93,000 km²) that CIRRUS did not cover in the
//! frame pair measured on 2026-08-10; it is a near-surface `PPI` rather than a column maximum, so
//! where CIRRUS drops out NIMBUS is not a degraded fallback but the physically closer
//! measurement; and it is the *reference* CIRRUS's column-max correction is calibrated against
//! ([`super::opera::MAX_TO_SURFACE_RATIO`]), so deleting it would leave that correction with
//! nothing to be measured from.
//!
//! The rejected third option is OPERA's `ACRR`, which the same key schema publishes. It is on the
//! same 2 km grid, and it is a **one-hour** accumulation (`prodname` says so; `startdate`/
//! `starttime` sit an hour before `enddate`/`endtime`). Dividing an hour of accumulation into a
//! rate smears a moving shower across sixty minutes of track, which is exactly the error a rider
//! is using radar to avoid.

use crate::fetch::Upstream;
use crate::source::opera::{self, Contract, Quantity, OPERA_TERMS_URL};
use crate::source::{Adapter, Attribution, BakedSource};

pub const ID: &str = "opera-nimbus";

pub const ATTRIBUTION: Attribution = Attribution {
    text: "Source: EUMETNET OPERA NIMBUS instantaneous rain rate composite (CC BY 4.0); modified/quantized by OpenBikeComputer",
    url: OPERA_TERMS_URL,
};

/// The pinned source contract, every field measured off the live objects on 2026-08-10.
pub const CONTRACT: Contract = Contract {
    id: ID,
    quantity: Quantity::RainRate,
    prodname: "OPERA NIMBUS instantaneous rain rate composite",
    odim_product: "PPI",
    width: 1_900,
    height: 2_200,
    cell_m: 2_000.0,
    // The same grid corner as CIRRUS at twice the pitch, so NIMBUS cell (r, c) is exactly the
    // 2 x 2 block of CIRRUS cells (2r, 2c) … (2r+1, 2c+1) — one composite at two resolutions, and
    // the property `opera::tests::a_nimbus_cell_is_an_exact_two_by_two_block_of_cirrus_cells`
    // exists to keep it true. The COG's tiepoint is half of *this* product's pixel north-west of
    // it, which is the same half-pixel converter bug at twice the size.
    ul_x: 0.0,
    ul_y: 0.0,
    cadence_seconds: 900,
    // Seventy-five minutes back, against a measured 10-minute publication lag.
    max_discovery_probes: 5,
    // **No nowcast, and it is a cost decision.** NIMBUS is the coarser backfill under CIRRUS over
    // largely the same ground, and a motion history for it would be a second 4.2 M-cell decode and
    // a second resample onto the 25 M-cell window for forward frames CIRRUS's nowcast already
    // covers almost everywhere. Where CIRRUS genuinely has nothing and NIMBUS does, the forward
    // frames stay ICON-EU — one step coarser than they could be, over a strip whose f0 is already
    // the 2 km product. Revisit with a measurement, not a preference.
    motion_lag_seconds: None,
    attribution: ATTRIBUTION,
};

pub struct OperaNimbus;

impl Adapter for OperaNimbus {
    fn id(&self) -> &'static str {
        ID
    }

    fn bake(&self, upstream: &mut dyn Upstream, now: i64, warnings: &mut Vec<String>) -> Result<BakedSource, String> {
        opera::bake(&CONTRACT, upstream, now, warnings)
    }
}
