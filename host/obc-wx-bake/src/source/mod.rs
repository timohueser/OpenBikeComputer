//! The adapter seam: one module per upstream source, all returning the same provider-neutral
//! baked product (WX1's prescribed layering). Fetch → decode → reproject (nearest-neighbour) →
//! quantize (the WX2 table) happens entirely inside an adapter; the cycle, emitter, manifest and
//! publisher never see a provider format.
//!
//! Since WXR3 (#1242) an adapter's `GEOMETRY` const is a **source-window description** — where
//! this source has data and at what pitch — not an output lattice. The last stage, resampling
//! onto the one canonical lattice, is [`crate::canonical`]'s single shared implementation, and
//! which source wins a cell where two overlap is decided by exactly one thing: the ordered
//! [`MOSAIC_PRIORITY`] table below.

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

/// One row of the mosaic priority table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MosaicSource {
    /// The adapter's [`Adapter::id`], which is also its baked product's id.
    pub id: &'static str,
    /// Why this source sits where it sits. Prose for the reader; nothing reads it.
    pub why: &'static str,
}

/// **The mosaic priority table — the one place a source's precedence lives.**
///
/// Ordered best first: a lattice cell is painted by the first source in this list that both
/// covers it and has data there. Everything below it is fill for cells it does not answer, and
/// the last row is the global floor that guarantees every cell **in the covered domain** always
/// carries a best-available value (which is what makes the no-provenance decision honest — see
/// [`crate::canonical`], and [`crate::canonical::Lattice::covered_rows`] for what the floor does
/// not reach).
///
/// The ordering rule, locked 2026-08-10 (#1242): **radar beats model, finer radar beats coarser,
/// national beats pan-European.** It is baker configuration and never client policy — nothing
/// downstream of the bakery is told which source painted which cell.
///
/// Adding a source is one row. Its position in this list *is* its priority; there is no separate
/// number to keep in sync, and [`mosaic_rank`] is the only reader.
pub const MOSAIC_PRIORITY: &[MosaicSource] = &[
    MosaicSource { id: dwd_rv::ID, why: "national 1 km radar nowcast (Germany) — the finest radar we ingest" },
    MosaicSource { id: us::ID, why: "national CONUS composite: 1 km MRMS radar observation, 3 km HRRR model ahead" },
    // WXR6 #1245's rows. Below every national radar composite and above every model, which is
    // the stated rule with no exception attached: `us` covers CONUS and OPERA covers Europe, so
    // the two never contend for a cell and nothing is lost by reading the rule literally.
    MosaicSource { id: opera_cirrus::ID, why: "pan-European 1 km radar: 5-minute reflectivity, the finest thing over Europe" },
    MosaicSource {
        id: opera_nimbus::ID,
        why: "pan-European 2 km radar rain rate — coarser and later than CIRRUS, but native mm/h and near-surface, and it covers cells CIRRUS does not",
    },
    MosaicSource { id: icon_eu::ID, why: "pan-European 6.5 km model — fill where no radar reaches" },
    MosaicSource { id: gfs::ID, why: "global 27.75 km model floor — the last row, and the reason no cell is blank" },
];

/// This source's rank in [`MOSAIC_PRIORITY`] (lower wins), or `None` if it has no row — which is
/// a bakery configuration bug, not a runtime condition, and the mosaic refuses to build.
pub fn mosaic_rank(id: &str) -> Option<usize> {
    MOSAIC_PRIORITY.iter().position(|source| source.id == id)
}

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
/// **Vestigial since WXR3 (#1242), removed by WXR7 (#1246).** The canonical dataset publishes one
/// lattice, which trivially nests under itself, so nothing the mosaic emits can violate this. It
/// still guards the per-product path that is live until WXR7 deletes it, and both callers
/// ([`us::UsComposite`] and [`crate::pack::crop`]) are on that path.
///
/// A client assembles a bundle on the coarsest frame's lattice and drops any frame that lattice
/// cannot tile ([`crate::geometry::GridGeometry::nests_under`]). A dropped frame is not a
/// degraded frame — it is a hole in the two-hour timeline, and the frame most likely to be
/// dropped is the fine-grained radar observation, because it is the one that differs. So this is
/// checked at bake time and fails the cycle closed: publishing a product the client will
/// silently dismantle is worse than publishing nothing and carrying the previous one forward.
/// Checked **pairwise**, not against the coarsest frame alone. The coarsest frame is the window
/// only for the frame set the client actually holds, and that set is a moving subset of what was
/// published: the client's freshness and horizon rules drop frames before assembly, and the
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
mod priority_tests {
    use super::*;

    /// Every adapter the bakery ships must have exactly one row, and every row must name a real
    /// adapter. A source with no row cannot be mosaicked at all, and a duplicate row would make
    /// "its position is its priority" ambiguous.
    #[test]
    fn every_adapter_has_exactly_one_row_and_every_row_an_adapter() {
        let adapters: [&dyn Adapter; 6] = [
            &dwd_rv::DwdRv,
            &icon_eu::IconEu,
            &us::UsComposite,
            &gfs::GfsFloor,
            &opera_cirrus::OperaCirrus,
            &opera_nimbus::OperaNimbus,
        ];
        for adapter in adapters {
            let rows = MOSAIC_PRIORITY.iter().filter(|source| source.id == adapter.id()).count();
            assert_eq!(rows, 1, "{} has {rows} rows in MOSAIC_PRIORITY", adapter.id());
        }
        for source in MOSAIC_PRIORITY {
            assert!(
                adapters.iter().any(|adapter| adapter.id() == source.id),
                "MOSAIC_PRIORITY names {} but no adapter answers to it",
                source.id
            );
        }
        assert_eq!(MOSAIC_PRIORITY.len(), adapters.len());
    }

    /// The locked ordering rule, spelled out so a reordering has to argue with a test: radar
    /// beats model, national beats pan-European, and the global floor is last so that every cell
    /// always has a best-available value.
    #[test]
    fn the_table_encodes_the_locked_ordering_rule() {
        let rank = |id| mosaic_rank(id).unwrap_or_else(|| panic!("{id} has no row"));
        assert!(rank(dwd_rv::ID) < rank(icon_eu::ID), "national radar must beat the pan-European model");
        assert!(rank(us::ID) < rank(icon_eu::ID), "a national composite must beat the pan-European model");
        assert!(rank(icon_eu::ID) < rank(gfs::ID), "the regional model must beat the global floor");
        // WXR6 #1245: pan-European radar sits under **every** national radar composite and over
        // every model — the rule as stated, with no exception — and the finer, fresher OPERA
        // product outranks the coarser one.
        for national in [dwd_rv::ID, us::ID] {
            assert!(rank(national) < rank(opera_cirrus::ID), "{national} must beat pan-European radar");
        }
        assert!(rank(opera_cirrus::ID) < rank(opera_nimbus::ID), "1 km / 5 min must beat 2 km / 15 min");
        assert!(rank(opera_nimbus::ID) < rank(icon_eu::ID), "any radar must beat the pan-European model");
        assert_eq!(
            mosaic_rank(gfs::ID),
            Some(MOSAIC_PRIORITY.len() - 1),
            "the global floor is the last row by construction"
        );
        assert_eq!(mosaic_rank("no-such-source"), None);
    }
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
