//! The adapter seam: one module per upstream source, all returning the same provider-neutral
//! [`BakedSource`] (WX1's prescribed layering). Fetch → decode → reproject (nearest-neighbour) →
//! quantize (the WX2 table) happens entirely inside an adapter; the mosaic, the emitter, the
//! manifest and the publisher never see a provider format, and nothing downstream of the bakery
//! learns that a source exists at all.
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

use crate::fetch::Upstream;
use crate::geometry::GridGeometry;

/// One row of the mosaic priority table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MosaicSource {
    /// The adapter's [`Adapter::id`], which is also its [`BakedSource`] id.
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
/// MRMS and HRRR are two rows and not one. They were the single composed `us` product until #1246
/// — the last place in the bakery where two upstreams shared one published timeline — and under
/// the mosaic a source is a source, placed by the rule above with no exception attached: 1 km
/// CONUS radar among the radars, 3 km CONUS model above the pan-European one. The visible
/// consequence is that the MRMS observation now paints the forward frames it is within
/// [`crate::canonical::MAX_FRAME_SKEW_S`] of, rather than losing them to an equally close HRRR
/// forecast the composed product could see and a priority table cannot. That is exactly the
/// frozen-observation behaviour every other single-frame radar source in this table already has
/// (both OPERA rows, #1245); doing it deliberately and well is WXR9 #1251's job.
///
/// Adding a source is one row. Its position in this list *is* its priority; there is no separate
/// number to keep in sync, and [`mosaic_rank`] is the only reader.
pub const MOSAIC_PRIORITY: &[MosaicSource] = &[
    MosaicSource { id: dwd_rv::ID, why: "national 1 km radar nowcast (Germany) — the finest radar we ingest" },
    MosaicSource { id: mrms::ID, why: "national 1 km radar observation (CONUS)" },
    // WXR6 #1245's rows. Below every national radar and above every model, which is the stated
    // rule with no exception attached: MRMS covers CONUS and OPERA covers Europe, so the two never
    // contend for a cell and nothing is lost by reading the rule literally.
    MosaicSource { id: opera_cirrus::ID, why: "pan-European 1 km radar: 5-minute reflectivity, the finest thing over Europe" },
    MosaicSource {
        id: opera_nimbus::ID,
        why: "pan-European 2 km radar rain rate — coarser and later than CIRRUS, but native mm/h and near-surface, and it covers cells CIRRUS does not",
    },
    MosaicSource {
        id: hrrr::ID,
        why: "national 3 km model (CONUS) — fill ahead of the radar observation and where it does not reach",
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

/// One quantized frame: canonical WX2 intensity codes on the source's own window.
#[derive(Debug)]
pub struct BakedFrame {
    /// Minutes ahead of this source's own reference time. Reported, and what an event pack names
    /// its files by; the mosaic places a frame by its `valid_at` alone.
    pub offset_min: u32,
    pub valid_at: i64,
    /// `obc_formats::obcg::FLAG_OBSERVED` or `FLAG_FORECAST`.
    pub flags: u16,
    pub cells: Vec<u8>,
}

/// Everything one source contributes to a cycle: its window, its licence line and its frames.
///
/// One window for the whole set, not one per frame. The per-frame override existed only for the
/// composed `us` product, whose 1 km observation and 3 km forecast frames shared a published
/// timeline; #1246 deleted that, so a source that mixes pitches is two sources and says so in
/// [`MOSAIC_PRIORITY`].
#[derive(Debug)]
pub struct BakedSource {
    pub id: &'static str,
    /// Where this source has data and at what pitch — a **source window**, never an output
    /// lattice. [`crate::canonical`] resamples from it onto the one published lattice.
    pub geometry: GridGeometry,
    /// Upstream run/reference time. Reported, and the anchor an event pack states its frame
    /// offsets against; the published dataset has a reference time of its own.
    pub reference_time: i64,
    pub attribution: Attribution,
    pub frames: Vec<BakedFrame>,
}

#[cfg(test)]
mod priority_tests {
    use super::*;

    /// Every adapter the bakery ships must have exactly one row, and every row must name a real
    /// adapter. A source with no row cannot be mosaicked at all, and a duplicate row would make
    /// "its position is its priority" ambiguous.
    #[test]
    fn every_adapter_has_exactly_one_row_and_every_row_an_adapter() {
        let adapters: [&dyn Adapter; 7] = [
            &dwd_rv::DwdRv,
            &icon_eu::IconEu,
            &mrms::Mrms,
            &hrrr::Hrrr,
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
        assert!(rank(mrms::ID) < rank(hrrr::ID), "CONUS radar must beat the CONUS model it shared a product with");
        assert!(rank(hrrr::ID) < rank(icon_eu::ID), "a national model must beat the pan-European one");
        assert!(rank(icon_eu::ID) < rank(gfs::ID), "the regional model must beat the global floor");
        // WXR6 #1245: pan-European radar sits under **every** national radar composite and over
        // every model — the rule as stated, with no exception — and the finer, fresher OPERA
        // product outranks the coarser one.
        for national in [dwd_rv::ID, mrms::ID] {
            assert!(rank(national) < rank(opera_cirrus::ID), "{national} must beat pan-European radar");
        }
        assert!(rank(opera_cirrus::ID) < rank(opera_nimbus::ID), "1 km / 5 min must beat 2 km / 15 min");
        assert!(rank(opera_nimbus::ID) < rank(hrrr::ID), "any radar must beat any model");
        assert_eq!(
            mosaic_rank(gfs::ID),
            Some(MOSAIC_PRIORITY.len() - 1),
            "the global floor is the last row by construction"
        );
        assert_eq!(mosaic_rank("no-such-source"), None);
    }
}

pub trait Adapter {
    fn id(&self) -> &'static str;

    /// Run one idempotent bake. `now` is injected for deterministic tests; non-fatal observations
    /// (an upstream run regression, for example) go into `warnings`.
    ///
    /// There is no unchanged short-circuit and no previously published entry to compare against.
    /// The mosaic needs every source's **cells**, not the knowledge that its objects are already
    /// published, so a cycle re-decodes every source every time. The short-circuit belonged to the
    /// per-product path #1246 deleted, along with the manifest entry it read its validator out of.
    fn bake(&self, upstream: &mut dyn Upstream, now: i64, warnings: &mut Vec<String>) -> Result<BakedSource, String>;
}
