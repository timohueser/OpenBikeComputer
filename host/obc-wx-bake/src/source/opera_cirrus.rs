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
//! rain rate — and, less obviously, a **calibration**, because CIRRUS is a *column maximum*
//! (`product=MAX`) while Marshall-Palmer relates surface reflectivity to surface rain. Cores
//! aloft and the stratiform bright band make the column max read high, measurably so: over the
//! 149,527 cells where both products saw an echo in the 2026-08-10T00:00 pair, the median
//! CIRRUS/NIMBUS rate ratio is 2.2 — a full intensity band, continent-wide and permanent.
//!
//! So this adapter applies Marshall-Palmer and then divides by that measured ratio
//! ([`super::opera::MAX_TO_SURFACE_RATIO`], equivalently `a_eff = 706.2` or -5.48 dBZ), leaving
//! OPERA's declared 200/1.6 as the NIMBUS contract check. **That correction is an empirical
//! anchor on one frame pair, not physics**, and `MAX_TO_SURFACE_RATIO`'s own doc records what
//! would settle it properly: split the ratio by regime at 30 dBZ over a full day, and score both
//! OPERA products against gauge-adjusted `dwd-rv` — which also answers whether CIRRUS belongs
//! above or below `dwd-rv` in the mosaic.
//!
//! **Frames are flagged observed, and that is a "primarily" claim.** The composite's own metadata
//! says some volume data is filled by Meteo France with a Lucas-Kanade advection
//! (`DBZH.meteo-france.advection.pysteps-1.5.0`), so a minority of cells are extrapolated rather
//! than seen. `OBCG_Spec.md` §3.2 defines the observed bit as "primarily an observation valid at
//! `valid_at`", and the frame *is* valid now rather than ahead of now — a forecast flag would be
//! the bigger lie. The posture is pinned indirectly: `product=MAX` is checked on every bake, so
//! CIRRUS turning into a different vertical sampling stops the cycle.

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
    text: "Source: EUMETNET OPERA CIRRUS maximum reflectivity composite (CC BY 4.0); reflectivity converted to surface rain rate with the Marshall-Palmer Z-R relation and an empirical column-maximum calibration, and quantized, by OpenBikeComputer",
    url: OPERA_TERMS_URL,
};

/// The pinned source contract, every field measured off the live objects on 2026-08-10.
pub const CONTRACT: Contract = Contract {
    id: ID,
    product_code: PRODUCT_OPERA_CIRRUS,
    quantity: Quantity::Reflectivity,
    prodname: "OPERA CIRRUS maximum reflectivity composite",
    odim_product: "MAX",
    width: 3_800,
    height: 4_400,
    cell_m: 1_000.0,
    // The grid's north-west corner, which OPERA's false origin exists to put at model (0, 0) and
    // which its ODIM corner attributes confirm — 3,800 x 4,400 cells of 1 km spanning exactly
    // 3,800,000 x 4,400,000 m. The COG's `ModelTiepoint` says half a pixel north-west of this;
    // `Contract::verify` requires exactly that offset. See `Contract::ul_x`.
    ul_x: 0.0,
    ul_y: 0.0,
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
