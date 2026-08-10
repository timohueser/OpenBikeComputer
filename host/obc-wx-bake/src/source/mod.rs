//! The adapter seam: one module per upstream source, all returning the same provider-neutral
//! baked product (WX1's prescribed layering). Fetch → decode → reproject (nearest-neighbour at
//! native cell size) → quantize (the WX2 table) → tile happens entirely inside an adapter; the
//! cycle, emitter, manifest and publisher never see a provider format.

pub mod dwd_rv;
pub mod gfs;
pub mod hrrr;
pub mod icon_eu;
pub mod mrms;
pub mod us;

use crate::fetch::Upstream;
use crate::geometry::GridGeometry;
use crate::manifest::Product;

/// NOAA Open Data Dissemination terms, the attribution URL of every NOAA-sourced product
/// (WX1's license record: public-use U.S. government data, no endorsement implied).
pub const NOAA_TERMS_URL: &str = "https://www.noaa.gov/information-technology/open-data-dissemination";

#[derive(Debug, Clone, Copy)]
pub struct Attribution {
    pub text: &'static str,
    pub url: &'static str,
}

/// Per-frame provenance and lattice, for a **composed** product whose frames do not all come
/// from one upstream (WX6's US product: a 1 km MRMS radar observation followed by 3 km HRRR
/// forward frames). OBCG stores geometry per object and the manifest restates it per frame, so
/// heterogeneous frames compose with no resampling — this override is how an adapter says so.
#[derive(Debug, Clone, Copy)]
pub struct FrameSource {
    /// `obc_formats::obcg` product registry code of the frame's own upstream.
    pub product_code: u8,
    /// The frame's own tier (a composed product's observation and model frames differ).
    pub tier: u8,
    pub geometry: GridGeometry,
}

/// One quantized frame: canonical WX2 intensity codes on the adapter's fixed lat/lon grid.
#[derive(Debug)]
pub struct BakedFrame {
    pub offset_min: u32,
    pub valid_at: i64,
    /// `obc_formats::obcg::FLAG_OBSERVED` or `FLAG_FORECAST`.
    pub flags: u16,
    /// `None` for a single-source product: the frame carries the product's own code, tier and
    /// geometry.
    pub source: Option<FrameSource>,
    pub cells: Vec<u8>,
}

impl BakedFrame {
    pub fn product_code(&self, product: &BakedProduct) -> u8 {
        self.source.map_or(product.product_code, |source| source.product_code)
    }

    pub fn tier(&self, product: &BakedProduct) -> u8 {
        self.source.map_or(product.tier, |source| source.tier)
    }

    pub fn geometry(&self, product: &BakedProduct) -> GridGeometry {
        self.source.map_or(product.geometry, |source| source.geometry)
    }
}

#[derive(Debug)]
pub struct BakedProduct {
    pub id: &'static str,
    pub product_code: u8,
    pub tier: u8,
    /// The product's nominal lattice. A composed product states its **anchor** frame's geometry
    /// here; every frame's exact geometry travels with the frame.
    pub geometry: GridGeometry,
    /// Upstream run/reference time (the immutable key's `<generated-utc>` component).
    pub reference_time: i64,
    pub staleness_deadline: i64,
    pub attribution: Attribution,
    /// Upstream validator for the next cycle's unchanged short-circuit, when the source has one.
    pub upstream_etag: Option<String>,
    pub frames: Vec<BakedFrame>,
}

/// Refuse a composed product whose frames cannot all be laid onto one bundle window.
///
/// A client assembles a bundle on the coarsest frame's lattice and drops any frame that lattice
/// cannot tile ([`crate::geometry::GridGeometry::nests_under`]). A dropped frame is not a
/// degraded frame — it is a hole in the two-hour timeline, and the frame most likely to be
/// dropped is the fine-grained radar observation, because it is the one that differs. So this is
/// checked at bake time and fails the cycle closed: publishing a product the client will
/// silently dismantle is worse than publishing nothing and carrying the previous one forward.
pub fn verify_frames_nest(product: &BakedProduct) -> Result<(), String> {
    let geometries: Vec<GridGeometry> = product.frames.iter().map(|frame| frame.geometry(product)).collect();
    let Some(coarsest) = geometries.iter().copied().max_by_key(GridGeometry::cell_area) else {
        return Ok(());
    };
    for (frame, geometry) in product.frames.iter().zip(&geometries) {
        if !geometry.nests_under(&coarsest) {
            return Err(format!(
                "{}: frame f{} on a {} x {} lattice at ({}, {}) does not nest under the coarsest \
                 frame's {} x {} lattice at ({}, {}) — a client would drop it",
                product.id,
                frame.offset_min,
                geometry.cell_lat_udeg,
                geometry.cell_lon_udeg,
                geometry.south_lat_udeg,
                geometry.west_lon_udeg,
                coarsest.cell_lat_udeg,
                coarsest.cell_lon_udeg,
                coarsest.south_lat_udeg,
                coarsest.west_lon_udeg,
            ));
        }
    }
    Ok(())
}

#[derive(Debug)]
pub enum AdapterOutcome {
    /// A fresh product to emit and publish.
    Baked(Box<BakedProduct>),
    /// The upstream run is the one already published; the previous manifest entry stands.
    Unchanged,
}

pub trait Adapter {
    fn id(&self) -> &'static str;

    /// Run one idempotent bake. `previous` is this product's entry in the currently published
    /// manifest (for validator/run short-circuits); `now` is injected for deterministic tests;
    /// non-fatal observations (an upstream run regression, for example) go into `warnings`.
    fn bake(
        &self,
        upstream: &mut dyn Upstream,
        previous: Option<&Product>,
        now: i64,
        warnings: &mut Vec<String>,
    ) -> Result<AdapterOutcome, String>;
}
