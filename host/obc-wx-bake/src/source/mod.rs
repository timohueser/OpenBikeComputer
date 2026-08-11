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

use obc_formats::obcg;

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
/// CONUS radar among the radars, 3 km CONUS model above the pan-European one.
///
/// **Rank decides which source paints a cell; it never decides which *frames* a source is offered
/// for.** That is [`crate::canonical::frame_is_eligible`]'s job, and the two are independent by
/// design. MRMS contributes one frame and it is an observation, so it is eligible for the anchor
/// alone: over CONUS f0 is 1 km radar and f+15 through f+120 are HRRR's real leads, with the floor
/// beneath. Being rank 1 buys MRMS nothing at f+15, because it has nothing valid at f+15 to offer.
///
/// This is the #1248 correction to what WXR7 shipped, where MRMS's single field was handed to every
/// canonical frame within [`crate::canonical::MAX_FRAME_SKEW_S`] (1,800 s) of it — the anchor and
/// the next two — and HRRR only took over at f+45. Those frames were labelled Forecast, honestly,
/// but a repeated "now" image is not a prediction of +15 whatever it is labelled. Doing radar
/// persistence deliberately, as an extrapolated forecast source with real forward frames, is WXR9
/// #1251's job; it will join this table as a source and be eligible for forward frames on the same
/// terms as any other forecast. Both OPERA rows (#1245) are single-frame observations too and the
/// rule reads them identically: f0 over Europe outside Germany is OPERA, f+15 onward is ICON-EU.
/// DWD RV is the exception that proves it is a rule about *data* and not about radar — RV is a
/// nowcast composite whose +5…+120 members are forecasts and are stamped as such, so Germany's
/// forward frames stay RV.
///
/// ## What the forward frames now fall through to
///
/// #1248 also changed the *shape* of the fall-through, not only who answers. A radar observation
/// used to sit above the regional models at every frame it was within skew of, so wherever a
/// regional model was absent — outside its domain, or a failed lead — the radar masked it and the
/// global floor was never reached. Forward frames now skip the radar row entirely, so the fall is
/// one step longer and lands on whatever is actually beneath: **regional model, else the 27.75 km
/// floor.** An HRRR or ICON-EU outage that used to cost nothing at f+15 and f+30 now costs those
/// frames their resolution, visibly.
///
/// The permanent version of that is the four strips where a radar footprint reaches past its
/// regional model's domain. At f+15 and f+30 these drop from radar-grade cells to the global floor;
/// f0 and f+45 onward are unchanged (f0 is still the observation, and from f+45 the observation was
/// already out of skew under the old rule too):
///
/// * **CONUS, 52.66–55.00 N** — MRMS reaches to 55 N, HRRR's Lambert domain stops at 52.66 N.
///   Northern-tier prairie and the Canadian border strip.
/// * **CONUS, 60.87–60.00 W** — the same mismatch on the eastern edge, a sliver of the maritime
///   approaches.
/// * **Europe, 70.53–73.00 N** — OPERA reaches into the Arctic, ICON-EU's domain stops at
///   70.53 N. Finnmark (70.9 N, 29 E) is inside it, and it is the one of the four with riders in.
/// * **Europe, 28.00–23.53 W** — OPERA's western reach past ICON-EU's west edge, over the
///   Atlantic approaches to Iceland.
///
/// Those bounds are the adapters' own `GEOMETRY`/`WINDOW` edges, not measurements of a rendered
/// frame, and `the_forward_frame_fall_through_strips_are_where_radar_outruns_its_model` derives
/// them from the constants so a domain change moves the documented strips or fails.
///
/// This is accepted rather than worked around: 27.75 km model fill that is a forecast of the frame's
/// instant is a truthful answer, and a 1 km radar image of half an hour ago published under that
/// instant is not. Narrowing the strips means a source whose forecasts cover them, which is a
/// [`MOSAIC_PRIORITY`] row, not an exception to the rule.
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

/// **What kind of statement a source frame is** — the classification the whole forward-frame rule
/// turns on ([`crate::canonical::frame_is_eligible`], #1248).
///
/// A two-variant enum rather than a `u16` of OBCG flag bits, and that is the point. It used to be
/// `flags: u16` with "`FLAG_OBSERVED` or `FLAG_FORECAST`" in a doc comment, which made three wrong
/// states representable — `0`, both bits, and any reserved bit — and the mosaic read it as
/// `flags & FLAG_OBSERVED != 0`, so **every** wrong state decoded to Forecast. An adapter that
/// forgot the field, or wrote `0` for a genuine observation, would silently become eligible for
/// every forward frame: the exact failure #1248 exists to make impossible, reintroduced one
/// careless adapter later. There is no `Default`, so a new adapter cannot omit the classification;
/// it has to say which of the two its data is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceClass {
    /// A measurement of one past instant. Eligible for the canonical anchor and nothing else.
    Observation,
    /// A prediction with its own forward validity — a model lead or step, or a nowcast member.
    Forecast,
}

impl SourceClass {
    /// The OBCG source-class bit this maps to (`OBCG_Spec.md` §3.2), for the emitter. Exactly one
    /// of the two is ever set, which the format requires and the enum now guarantees.
    pub fn obcg_flag(self) -> u16 {
        match self {
            Self::Observation => obcg::FLAG_OBSERVED,
            Self::Forecast => obcg::FLAG_FORECAST,
        }
    }

    pub fn is_observation(self) -> bool {
        matches!(self, Self::Observation)
    }
}

/// One quantized frame: canonical WX2 intensity codes on the source's own window.
#[derive(Debug)]
pub struct BakedFrame {
    /// Minutes ahead of this source's own reference time. Reported, and what an event pack names
    /// its files by; the mosaic places a frame by its `valid_at` alone.
    pub offset_min: u32,
    pub valid_at: i64,
    /// Observation or forecast — see [`SourceClass`], and note there is no third answer and no
    /// default. This is what decides which canonical frames the frame may paint at all.
    pub class: SourceClass,
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
