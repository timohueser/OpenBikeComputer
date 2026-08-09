//! The adapter seam: one module per upstream source, all returning the same provider-neutral
//! baked product (WX1's prescribed layering). Fetch → decode → reproject (nearest-neighbour at
//! native cell size) → quantize (the WX2 table) → tile happens entirely inside an adapter; the
//! cycle, emitter, manifest and publisher never see a provider format.

pub mod dwd_rv;
pub mod icon_eu;

use crate::fetch::Upstream;
use crate::geometry::GridGeometry;
use crate::manifest::Product;

#[derive(Debug, Clone, Copy)]
pub struct Attribution {
    pub text: &'static str,
    pub url: &'static str,
}

/// One quantized frame: canonical WX2 intensity codes on the adapter's fixed lat/lon grid.
#[derive(Debug)]
pub struct BakedFrame {
    pub offset_min: u32,
    pub valid_at: i64,
    /// `obc_formats::obcg::FLAG_OBSERVED` or `FLAG_FORECAST`.
    pub flags: u16,
    pub cells: Vec<u8>,
}

#[derive(Debug)]
pub struct BakedProduct {
    pub id: &'static str,
    pub product_code: u8,
    pub tier: u8,
    pub geometry: GridGeometry,
    /// Upstream run/reference time (the immutable key's `<generated-utc>` component).
    pub reference_time: i64,
    pub staleness_deadline: i64,
    pub attribution: Attribution,
    /// Upstream validator for the next cycle's unchanged short-circuit, when the source has one.
    pub upstream_etag: Option<String>,
    pub frames: Vec<BakedFrame>,
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
    /// manifest (for validator/run short-circuits); `now` is injected for deterministic tests.
    fn bake(&self, upstream: &mut dyn Upstream, previous: Option<&Product>, now: i64)
        -> Result<AdapterOutcome, String>;
}
