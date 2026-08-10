//! OPERA CIRRUS: Europe's 1 km reflectivity composite, every five minutes (WXR6, #1245).
//!
//! CIRRUS is the composite that justifies a 1 km lattice in Europe — 3,800 x 4,400 cells of 1 km,
//! a new one every five minutes, on the object store 4.1 minutes after its valid time (measured
//! 2026-08-10). It is the **primary** European radar source, above [`super::opera_nimbus`] in the
//! mosaic, because resolution and age are what a rider feels: at a 15-minute bake CIRRUS offers a
//! frame about five minutes old where NIMBUS offers one ten to fifteen minutes old, at half the
//! linear resolution.
//!
//! What it costs is a Z-R relation, because CIRRUS carries reflectivity and the lattice carries
//! rain rate. That conversion is [`super::opera::ZR_A`]/[`super::opera::ZR_B`] — Marshall-Palmer,
//! and the very relation OPERA itself uses to derive NIMBUS, pinned against NIMBUS's own metadata
//! so the two adapters can never silently disagree.
//!
//! One honest caveat, measured rather than assumed. CIRRUS is a *column maximum* (`product=MAX`)
//! while NIMBUS is a near-surface `PPI`, so converting CIRRUS reads wetter than NIMBUS: over the
//! 149 k cells where both saw an echo in the 2026-08-10T00:00 pair, the median rate ratio is
//! 2.2 — roughly one intensity band. That is the known cost of preferring the fresher, finer
//! product, not a calibration bug, and it is why NIMBUS stays in the mosaic rather than being
//! deleted in favour of CIRRUS alone.

use obc_formats::obcg::PRODUCT_OPERA_CIRRUS;

use crate::fetch::Upstream;
use crate::manifest::Product;
use crate::source::opera::{self, Contract, Quantity, OPERA_TERMS_URL};
use crate::source::{Adapter, AdapterOutcome, Attribution};

pub const ID: &str = "opera-cirrus";

/// A radar composite refreshes every five minutes; half an hour without a fresh one is the epic's
/// stuck-baker detection horizon, so the product must not outlive it (the `dwd-rv` rule).
pub const STALENESS_SECONDS: i64 = 30 * 60;

pub const ATTRIBUTION: Attribution = Attribution {
    text: "Source: EUMETNET OPERA CIRRUS maximum reflectivity composite (CC BY 4.0); reflectivity converted to rain rate with the Marshall-Palmer Z-R relation and quantized by OpenBikeComputer",
    url: OPERA_TERMS_URL,
};

/// The pinned source contract, every field measured off the live objects on 2026-08-10.
pub const CONTRACT: Contract = Contract {
    id: ID,
    product_code: PRODUCT_OPERA_CIRRUS,
    quantity: Quantity::Reflectivity,
    prodname: "OPERA CIRRUS maximum reflectivity composite",
    width: 3_800,
    height: 4_400,
    cell_m: 1_000.0,
    // `ModelTiepoint`: the upper-left pixel corner, half a cell north-west of the projected
    // (-1,950,000, +2,100,000) the ODIM corner attributes name. The sub-millimetre tail is
    // OPERA's own converter round-tripping the corner through PROJ, and it is pinned as written.
    ul_x: -500.000_271_433_265_9,
    ul_y: 499.999_912_387_225_8,
    cadence_seconds: 300,
    // Forty minutes back, against a measured 4.1-minute publication lag.
    max_discovery_probes: 8,
    staleness_seconds: STALENESS_SECONDS,
    attribution: ATTRIBUTION,
};

pub struct OperaCirrus;

impl Adapter for OperaCirrus {
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
