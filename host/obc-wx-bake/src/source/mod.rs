//! The adapter seam: one module per upstream source, all returning the same provider-neutral
//! baked product (WX1's prescribed layering). Fetch → decode → reproject (nearest-neighbour at
//! native cell size) → quantize (the WX2 table) → tile happens entirely inside an adapter; the
//! cycle, emitter, manifest and publisher never see a provider format.

pub mod dwd_rv;
pub mod gfs;
pub mod hrrr;
pub mod icon_eu;
pub mod mrms;
pub mod opera;
pub mod opera_cirrus;
pub mod opera_nimbus;
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
/// Checked **pairwise**, not against the coarsest frame alone. The coarsest frame is the window
/// only for the frame set the client actually holds, and that set is a moving subset of what was
/// published: `select`'s freshness and horizon rules drop frames before assembly, and the
/// producer cap can drop the furthest-future one. So a product whose coarsest frame masks a
/// non-nesting pair beneath it is a product that assembles correctly right up until the frame
/// doing the masking ages out. Three lattices of 20,000 / 30,000 / 60,000 microdegrees are the
/// smallest example: all nest under 60,000, and 20,000 does not nest under 30,000.
pub fn verify_frames_nest(product: &BakedProduct) -> Result<(), String> {
    let geometries: Vec<GridGeometry> = product.frames.iter().map(|frame| frame.geometry(product)).collect();
    for (fine_index, fine) in geometries.iter().enumerate() {
        for (coarse_index, coarse) in geometries.iter().enumerate() {
            if fine_index == coarse_index || coarse.cell_area() < fine.cell_area() {
                continue;
            }
            if !fine.nests_under(coarse) {
                return Err(format!(
                    "{}: frame f{} on a {} x {} lattice at ({}, {}) does not nest under frame f{}'s \
                     {} x {} lattice at ({}, {}) — a client holding both would drop the finer one",
                    product.id,
                    product.frames[fine_index].offset_min,
                    fine.cell_lat_udeg,
                    fine.cell_lon_udeg,
                    fine.south_lat_udeg,
                    fine.west_lon_udeg,
                    product.frames[coarse_index].offset_min,
                    coarse.cell_lat_udeg,
                    coarse.cell_lon_udeg,
                    coarse.south_lat_udeg,
                    coarse.west_lon_udeg,
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod nesting_tests {
    use super::*;

    fn geometry(cell: u32, south: i32, west: i32) -> GridGeometry {
        anisotropic(cell, cell, south, west)
    }

    fn anisotropic(cell_lat: u32, cell_lon: u32, south: i32, west: i32) -> GridGeometry {
        GridGeometry {
            south_lat_udeg: south,
            west_lon_udeg: west,
            cell_lat_udeg: cell_lat,
            cell_lon_udeg: cell_lon,
            width: 10,
            height: 10,
            cell_size_m: 1_000,
            tile_edge: 32,
            entries_per_page: 512,
        }
    }

    fn product(lattices: &[u32]) -> BakedProduct {
        BakedProduct {
            id: "test",
            product_code: 1,
            tier: 1,
            geometry: geometry(lattices.first().copied().unwrap_or(10_000), 20_000_000, -130_000_000),
            reference_time: 0,
            staleness_deadline: 0,
            attribution: Attribution { text: "test", url: "https://example.invalid" },
            upstream_etag: None,
            frames: lattices
                .iter()
                .enumerate()
                .map(|(index, cell)| BakedFrame {
                    offset_min: index as u32 * 15,
                    valid_at: index as i64 * 900,
                    flags: 0,
                    source: Some(FrameSource {
                        product_code: 1,
                        tier: 1,
                        geometry: geometry(*cell, 20_000_000, -130_000_000),
                    }),
                    cells: Vec::new(),
                })
                .collect(),
        }
    }

    /// The reason this is pairwise. All three nest under the 60,000 lattice, so a coarsest-only
    /// check passes — and then the 60,000 frame ages out of the client's window, 30,000 becomes
    /// the window, and the 20,000 frame is dropped. The product was always broken; the coarsest
    /// frame was only hiding it.
    #[test]
    fn a_non_nesting_pair_masked_by_a_coarser_frame_is_still_refused() {
        let error = verify_frames_nest(&product(&[20_000, 30_000, 60_000])).expect_err("must refuse");
        assert!(error.contains("does not nest"), "{error}");
    }

    /// Equal cell *area*, transposed strides: neither lattice nests under the other, and the
    /// pairwise loop only sees the pair at all because it skips on `coarse < fine` rather than
    /// `coarse <= fine`. Without this case that comparison can be relaxed to `<=` — which reads
    /// like a harmless "skip equal areas" tidy-up — with the entire workspace suite still green,
    /// while `bundle::build` drops a frame for this pair in most corridors.
    #[test]
    fn equal_area_lattices_with_transposed_strides_are_refused() {
        let mut product = product(&[20_000, 20_000]);
        product.frames[0].source.as_mut().expect("source").geometry =
            anisotropic(20_000, 60_000, 20_000_000, -130_000_000);
        product.frames[1].source.as_mut().expect("source").geometry =
            anisotropic(60_000, 20_000, 20_000_000, -130_000_000);
        let error = verify_frames_nest(&product).expect_err("neither nests under the other");
        assert!(error.contains("does not nest"), "{error}");
    }

    #[test]
    fn a_genuinely_nesting_ladder_is_accepted() {
        verify_frames_nest(&product(&[10_000, 30_000, 60_000])).expect("10k | 30k | 60k all nest");
    }

    /// Degenerate shapes must not be treated as violations: a product can legitimately publish a
    /// single frame, and a fully degraded cycle can publish none.
    #[test]
    fn one_frame_and_no_frames_are_both_fine() {
        verify_frames_nest(&product(&[10_000])).expect("a lone frame nests under itself");
        verify_frames_nest(&product(&[])).expect("no frames is not a violation");
    }
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
