//! Pre-download wasm memory projection.
//!
//! `peak ≈ engine + resident_input + resident_output` — and after #1116 phase D the engine term is
//! **the sort budget, not the map**. That sentence is the epic's deliverable, and this module is
//! where it has to be cashed out as numbers a refusal can be built on.
//!
//! # The engine term, post-D
//!
//! The merge (D2–D4) and the §4.8 verify (D5) hold per-node/per-edge bookkeeping in *sorted passes*
//! spilled through the scratch seam, bounded by `merge_budget_bytes`. Measured on the finished
//! engine (`obcm-assemble --features mem-profile`, macOS arm64, published v12 catalog, the same
//! harness every prior coefficient came from):
//!
//! | budget | BW peak (296 MB nav) | freiburg peak (90 MB nav) |
//! | --: | --: | --: |
//! | 16 MiB | 50.4 MiB | 39.2 MiB |
//! | 64 MiB | 80.4 MiB | 64.7 MiB |
//! | 256 MiB | 248.2 MiB | 161.3 MiB |
//!
//! The peak tracks the budget (the run buffer plus its stable-sort companion **is** the budget, by
//! `extsort`'s `RUN_SHARE` construction) plus a residual floor that does *not* scale with the
//! region the way the old arrays did: per-cell transients bounded by the largest cell, the first-fit
//! bin table (4 B per 512-B output chunk), the seam table, the block caches. The largest measured
//! residual is BW's +34 MiB at the 16 MiB budget; extrapolated to DACH the floor's genuinely
//! data-dependent parts (bins ≈ 4 B × section/512 ≈ 30 MB, seams ≈ a few MB) stay under
//! [`ENGINE_FLOOR`], which is set at 96 MiB so the model absorbs them with the same kind of slack
//! every earlier coefficient carried. On top, [`WASM_ALLOC_MARGIN`] — the same ×1.15 as before, for
//! the allocator this was not measured on (dlmalloc in a linear memory that only grows).
//!
//! # The other two terms are modes
//!
//! One OPFS probe answers for all three seams — the cells (B2), the spill (D2's scratch), and the
//! map sink (D1) ride the same sync-access-handle capability — so [`Residency::input_on_disk`]
//! states the *host*, not one seam:
//!
//! * **OPFS host, sunk output** ([`Residency::output_sunk`]): input is the block caches plus the
//!   terrain squares (terrain is never stored on disk — the cells are handed over as buffers and
//!   spliced straight into the map's tail, so they are resident on the way in and nowhere on the way
//!   out); output is **nothing**, because the map goes to OPFS a megabyte at a time and is never
//!   wasm's. This is the only mode a country assembles in, and after the one-file slice it is not
//!   merely the cheapest — a DACH map is ~9 GB, so no address space can hold it and there is no
//!   resident variant of that selection to compare against.
//! * **OPFS host, resident output**: the caller keeps the whole file (it wants the bytes back — the
//!   device path, which needs the finished map in hand). The verdicts genuinely differ from ~1.4× BW
//!   up, so they are computed apart and never averaged.
//! * **No usable OPFS**: the buffered fallback. Cells resident, spill in `MemoryScratch` — which
//!   after phase D is *the edge and adjacency streams*, priced at [`SPILL_PER_NAV_BYTE`] × nav (D3
//!   measured 409 MiB of spill at BW pre-eviction; 2.5× covers it with margin) — and the whole map
//!   resident on top, because a browser with no sync access handles has nowhere else to put it. Such
//!   a browser honestly cannot do a country, and this model says so instead of letting the tab die
//!   trying.
//!
//! # What this does not cover
//!
//! Two regions on one machine, one schema revision, one skin; the floor's DACH extrapolation is
//! arithmetic over structures whose growth is understood, not a measurement at that scale — the
//! epic's closing end-to-end run is what turns it into one. Native tracked-heap numbers carried to
//! wasm by the same ×1.15 that has held since the C-series.

use crate::driver::{DEFAULT_READ_BLOCK, READ_CACHE_BLOCKS};

/// The engine's residual floor over the sort budget: per-cell transients, the first-fit bin table,
/// the seam table, both block caches, and slack. Largest measured residual is 34 MiB (BW at a
/// 16 MiB budget); 96 MiB covers the DACH extrapolation of the data-dependent parts with the same
/// ~2× kind of headroom every prior coefficient carried.
pub const ENGINE_FLOOR: f64 = 96.0 * 1024.0 * 1024.0;

/// wasm-dlmalloc margin over the natively measured peaks — unchanged since the C-series: the
/// harness counts bytes requested, macOS `System` added 4–5 % in touched pages, and wasm's linear
/// memory only ever grows, so fragmentation within a run is permanent in a way it is not natively.
pub const WASM_ALLOC_MARGIN: f64 = 1.15;

/// The buffered fallback's spill, resident in `MemoryScratch`, per byte of selected `network`
/// band: the collected edge stream, the adjacency entries, the claim sort. D3 measured 409 MiB at
/// BW (1.38×) before mid-stream run eviction; 2.5× covers the post-D4 sum of concurrent streams
/// with margin. Only the no-OPFS path pays it.
pub const SPILL_PER_NAV_BYTE: f64 = 2.5;

/// Output bytes per byte of input cell. Geometry chunks are copied verbatim and the nav section is
/// rewritten to about the size the cells' own had, so the map comes out the size of its inputs:
/// measured 0.9988 (freiburg) and 0.9989 (baden-württemberg). Kept at `1.0`, which rounds the
/// right way.
///
/// The raster is on both sides too, since the splice: a terrain square is an input byte
/// (`total_cell_bytes` includes it) and it is copied verbatim into the map's §1.3 region, so it
/// contributes the same count to each.
///
/// # Where OBCM v14's filler lands in this ratio (`OBCM_Spec.md` §1.2)
///
/// §1.2 quantifies two costs, and the coefficient absorbs them for two different reasons:
///
/// * **Per chunk** — a §5 geometry chunk is padded to the next `U = 16` boundary, `(U-1)/2 = 7.5`
///   bytes on average, which is **~0.47 %** at the measured average 1,600-byte chunk and at most
///   1.5 % at the 512-byte floor. It is on **both sides of this ratio**: the cells the builder
///   downloads are v14 too, so a chunk arrives already padded and the graft copies its bytes
///   verbatim (`OBCA_Spec.md` §2.3). The numerator and the denominator carry the same filler, so the
///   ratio is unmoved — which is why the measured 0.9988 did not shift with the version.
/// * **Per region and per section boundary** — one gap of `0..U-1` bytes each, about **50** of them
///   in a full-ladder map (two per LOD, plus the header, the style and LOD tables, the six POI
///   categories, the hours pool, the nav section's six). Those are the assembly's own and appear
///   only in the numerator, and they come to a few hundred bytes: `50 × 15 = 750 B` worst case,
///   which is `3 × 10⁻⁶` of the freiburg selection and vanishes against any real one. Rounding
///   `0.9989` up to `1.0` gives back 0.11 % — 315 KB on freiburg, over 400× what the region gaps can
///   consume, and the margin only widens with the selection — so they are covered, not ignored.
pub const OUTPUT_PER_CELL_BYTE: f64 = 1.0;

/// The §1.2 region gaps a full-ladder assembly adds over its inputs, worst case: about 50 boundaries
/// at `U - 1` bytes each. Stated so [`the_rounding_covers_the_v14_region_gaps`] can check the claim
/// above rather than leave it as a sentence.
#[cfg(test)]
const REGION_GAP_BYTES: f64 = 50.0 * 15.0;

/// One block cache's residency (`driver.rs`'s default geometry). Two exist on the sunk path — the
/// input cells' and the map read-back's.
pub const READ_CACHE_BYTES: f64 = (READ_CACHE_BLOCKS * DEFAULT_READ_BLOCK) as f64;

/// wasm32's hard address space. Nothing can be allocated past this, whatever the machine has.
pub const WASM32_ADDRESS_SPACE: f64 = 4.0 * 1024.0 * 1024.0 * 1024.0;

/// The **default** budget this crate reports `fits` against: **3 GiB**, 75 % of the address space.
///
/// Not a measurement — a judgement, and the reason is that the failure mode is unforgiving. A wasm
/// allocation that cannot be served aborts the module: there is no `Err` to render, the tab has
/// already spent the whole download and the whole rewrite, and the rider sees a crash. Browsers
/// also do not reliably grant the full 4 GiB. A quarter of the space is the margin those facts are
/// worth — and post-D the question has teeth only on the resident-output and fallback paths, because
/// the sunk path no longer gets anywhere near it.
pub const PRACTICAL_BUDGET: f64 = 3.0 * 1024.0 * 1024.0 * 1024.0;

/// Which escapes from linear memory this run will actually have. The projection is a property of
/// the selection *and the mode* — a caller that keeps the finished map and one that sinks it
/// genuinely disagree from ~1.4× BW up, and a no-OPFS browser runs a different engine profile
/// altogether.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Residency {
    /// This browser passed the sync-access-handle probe with room to spare: the cells read from
    /// OPFS (B2) and the spill lives there (D2's scratch). One capability, and `output_sunk` is the
    /// third seam that rides it.
    pub input_on_disk: bool,
    /// The map is written straight to the host through a `MapWrites` sink (D1) and is never
    /// resident. `false` keeps the whole file in wasm memory until the caller takes it.
    pub output_sunk: bool,
}

impl Residency {
    /// The browser's real path on an OPFS host: cells and spill on disk, the map written through
    /// the sink.
    pub fn streamed() -> Residency {
        Residency { input_on_disk: true, output_sunk: true }
    }

    /// No escapes at all: the no-OPFS fallback, keeping cells, spill and the whole map.
    pub fn resident() -> Residency {
        Residency { input_on_disk: false, output_sunk: false }
    }
}

/// What an assembly of this size would cost, and whether a browser can pay it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MemoryEstimate {
    /// The engine's working set: budget-bounded on an OPFS host, budget + resident spill on the
    /// fallback. The wasm allocator margin is already applied.
    pub engine_bytes: f64,
    /// The **resident** input: the whole selection, or the block caches plus terrain when the
    /// cells stay in OPFS.
    pub input_bytes: f64,
    /// The **resident** output: the whole map, or nothing when it goes to the host's own storage.
    pub output_bytes: f64,
    /// The sum: what wasm32 has to hold at once.
    pub peak_bytes: f64,
    /// The budget [`MemoryEstimate::fits`] was decided against — [`PRACTICAL_BUDGET`] unless the
    /// caller supplied its own.
    pub budget_bytes: f64,
    /// [`WASM32_ADDRESS_SPACE`].
    pub ceiling_bytes: f64,
    /// `peak_bytes <= budget_bytes` — the answer a caller gates the download on.
    pub fits: bool,
    /// `budget_bytes - peak_bytes`; negative when it does not fit, which is the number to show.
    pub headroom_bytes: f64,
}

/// Project the peak memory of assembling a selection, from the catalog's own byte counts.
///
/// * `network_band_bytes` — the selected cells of the `network` band (nav + POIs, no geometry).
/// * `total_cell_bytes` — every selected cell of every band **plus the terrain squares**.
/// * `terrain_bytes` — the terrain squares' share of that total (0 for a terrain-less catalog).
///   Terrain rides outside the input escape: it is handed over as buffers, never read from OPFS.
/// * `merge_budget_bytes` — the engine's sort budget (`Options::merge_budget_bytes`), which after
///   phase D **is** the engine term on an OPFS host. Non-positive or non-finite falls back to the
///   engine's own 64 MiB default, so a caller that has not chosen still gets a verdict about
///   something real.
///
/// See the module header for the model and its measured basis.
pub fn estimate_memory(
    network_band_bytes: f64,
    total_cell_bytes: f64,
    terrain_bytes: f64,
    merge_budget_bytes: f64,
    residency: Residency,
) -> MemoryEstimate {
    estimate_memory_with_budget(
        network_band_bytes,
        total_cell_bytes,
        terrain_bytes,
        merge_budget_bytes,
        residency,
        PRACTICAL_BUDGET,
    )
}

/// [`estimate_memory`], against a budget the caller chooses — the mobile-UA override, exactly as
/// before: a phone's tab is evicted rather than slowed, so a caller that knows should lower the
/// number the verdict is measured against. A non-finite or non-positive budget falls back to
/// [`PRACTICAL_BUDGET`].
pub fn estimate_memory_with_budget(
    network_band_bytes: f64,
    total_cell_bytes: f64,
    terrain_bytes: f64,
    merge_budget_bytes: f64,
    residency: Residency,
    budget: f64,
) -> MemoryEstimate {
    let budget_bytes = if budget.is_finite() && budget > 0.0 { budget } else { PRACTICAL_BUDGET };
    let nav = network_band_bytes.max(0.0);
    let cells = total_cell_bytes.max(nav);
    let terrain = terrain_bytes.clamp(0.0, cells);
    let sort_budget = if merge_budget_bytes.is_finite() && merge_budget_bytes > 0.0 {
        merge_budget_bytes
    } else {
        64.0 * 1024.0 * 1024.0
    };
    let whole_map = OUTPUT_PER_CELL_BYTE * cells;
    let output_bytes = if residency.output_sunk { 0.0 } else { whole_map };
    let (engine_bytes, input_bytes) = if residency.input_on_disk {
        ((sort_budget + ENGINE_FLOOR) * WASM_ALLOC_MARGIN, 2.0 * READ_CACHE_BYTES + terrain)
    } else {
        // The fallback: spill in MemoryScratch and cells in memory. No wasm margin on the spill
        // term — 2.5× is already the margin.
        ((sort_budget + ENGINE_FLOOR) * WASM_ALLOC_MARGIN + SPILL_PER_NAV_BYTE * nav, cells)
    };
    let peak_bytes = engine_bytes + input_bytes + output_bytes;
    MemoryEstimate {
        engine_bytes,
        input_bytes,
        output_bytes,
        peak_bytes,
        budget_bytes,
        ceiling_bytes: WASM32_ADDRESS_SPACE,
        fits: peak_bytes <= budget_bytes,
        headroom_bytes: budget_bytes - peak_bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: f64 = 1_000_000.0;
    /// The sort budget the builder runs with (`DownloadStep.svelte`).
    const SORT: f64 = 256.0 * 1024.0 * 1024.0;

    /// The published catalog's figures for the measured regions, plus the DACH shape from
    /// [`cell_size_survey.rs`](../../../host/obc-pack/examples/cell_size_survey.rs) — the one
    /// selection this file exists to answer for now.
    mod catalog {
        pub const FREIBURG_NAV: f64 = 90_052_777.0;
        pub const FREIBURG_TERRAIN: f64 = 23_069_068.0;
        pub const FREIBURG_CELLS: f64 = 263_616_395.0 + FREIBURG_TERRAIN;
        pub const BW_NAV: f64 = 295_921_548.0;
        pub const BW_TERRAIN: f64 = 58_721_264.0;
        pub const BW_CELLS: f64 = 794_735_626.0 + BW_TERRAIN;
        /// DACH: one file of ~9 GB, ~8.5 GB of cells, ~430 MiB of raster.
        pub const DACH_NAV: f64 = 3.0e9;
        pub const DACH_TERRAIN: f64 = 0.45e9;
        pub const DACH_CELLS: f64 = 8.5e9;
    }

    fn streamed(nav: f64, cells: f64, terrain: f64) -> MemoryEstimate {
        estimate_memory(nav, cells, terrain, SORT, Residency::streamed())
    }

    /// An OPFS host that keeps the finished map instead of sinking it.
    fn kept(nav: f64, cells: f64, terrain: f64) -> MemoryEstimate {
        estimate_memory(nav, cells, terrain, SORT, Residency { input_on_disk: true, output_sunk: false })
    }

    fn fallback(nav: f64, cells: f64, terrain: f64) -> MemoryEstimate {
        estimate_memory(nav, cells, terrain, SORT, Residency::resident())
    }

    /// **The assertion epic #1116 exists for, closed.** DACH — one ~9 GB map, 8.5 GB of cells —
    /// projects at ~0.88 GB on the sunk path of an OPFS host: the budget-bounded engine, two block
    /// caches, and the raster on the way in. It fits a 3 GiB tab with more headroom than BW had
    /// before this epic started.
    ///
    /// One file made the claim stronger rather than weaker. The map is now larger than the address
    /// space, so "the output is not wasm's" stopped being an optimisation and became the only shape
    /// in which the selection exists at all.
    #[test]
    fn dach_fits_the_sunk_path_and_that_is_the_epic() {
        let e = streamed(catalog::DACH_NAV, catalog::DACH_CELLS, catalog::DACH_TERRAIN);
        assert!(e.fits, "DACH must fit — {} B against {} B", e.peak_bytes, e.budget_bytes);
        assert!((e.peak_bytes - 0.877e9).abs() < 2e7, "{}", e.peak_bytes);
        assert!(e.headroom_bytes > 0.5 * PRACTICAL_BUDGET, "…with real headroom: {}", e.headroom_bytes);
        assert_eq!(e.output_bytes, 0.0, "a sunk map is never wasm's");
        // The engine term genuinely stopped scaling with the map: DACH and freiburg differ only in
        // their terrain, never in the merge.
        let f = streamed(catalog::FREIBURG_NAV, catalog::FREIBURG_CELLS, catalog::FREIBURG_TERRAIN);
        assert_eq!(e.engine_bytes, f.engine_bytes, "the engine term is the budget, not the map");
    }

    /// A caller that keeps the finished map pays for all of it, so DACH honestly refuses — 8.5 GB
    /// cannot be resident, and past this slice it cannot even be addressed — while BW fits. The two
    /// verdicts stay separate for exactly this reason.
    #[test]
    fn dach_refuses_a_resident_map_and_bw_does_not() {
        let dach = kept(catalog::DACH_NAV, catalog::DACH_CELLS, catalog::DACH_TERRAIN);
        assert!(!dach.fits);
        assert!(dach.peak_bytes > WASM32_ADDRESS_SPACE, "past the address space, not merely the budget");
        let bw = kept(catalog::BW_NAV, catalog::BW_CELLS, catalog::BW_TERRAIN);
        assert!(bw.fits, "{}", bw.peak_bytes);
    }

    /// A browser with no usable OPFS is the pre-phase-D world with the spill and the whole map on
    /// top: BW still fits (barely mattering-ly), a country does not, and the refusal is honest —
    /// that browser would die trying.
    #[test]
    fn the_no_opfs_fallback_admits_bw_and_refuses_dach() {
        let bw = fallback(catalog::BW_NAV, catalog::BW_CELLS, catalog::BW_TERRAIN);
        assert!(bw.fits, "{}", bw.peak_bytes);
        let dach = fallback(catalog::DACH_NAV, catalog::DACH_CELLS, catalog::DACH_TERRAIN);
        assert!(!dach.fits);
        assert!(dach.headroom_bytes < 0.0);
    }

    /// BW and freiburg on the sunk path: small numbers now, and the difference between them is
    /// terrain, not graph.
    #[test]
    fn the_measured_regions_are_comfortable_and_terrain_shaped() {
        let bw = streamed(catalog::BW_NAV, catalog::BW_CELLS, catalog::BW_TERRAIN);
        assert!((bw.peak_bytes - 0.485e9).abs() < 1e7, "{}", bw.peak_bytes);
        assert!(bw.headroom_bytes > 0.8 * PRACTICAL_BUDGET);
        let phone = estimate_memory_with_budget(
            catalog::FREIBURG_NAV,
            catalog::FREIBURG_CELLS,
            catalog::FREIBURG_TERRAIN,
            SORT,
            Residency::streamed(),
            1024.0 * 1024.0 * 1024.0,
        );
        assert!(phone.fits, "a Regierungsbezirk fits a phone's tab: {}", phone.peak_bytes);
        assert!(phone.headroom_bytes > 0.4 * phone.budget_bytes, "{}", phone.headroom_bytes);
    }

    /// The floor plus margin must cover every measured peak with the budget the run actually had —
    /// the same pinning discipline the 4.7 coefficient carried, restated for the new model.
    #[test]
    fn the_floor_covers_every_measured_peak_with_margin_to_declare() {
        // (budget, tracked peak) from the post-D4 harness — see the module header's table.
        let runs: [(f64, f64); 6] = [
            (16.0 * 1024.0 * 1024.0, 52_880_424.0),
            (64.0 * 1024.0 * 1024.0, 84_347_101.0),
            (256.0 * 1024.0 * 1024.0, 260_236_167.0),
            (16.0 * 1024.0 * 1024.0, 41_114_536.0),
            (64.0 * 1024.0 * 1024.0, 67_797_357.0),
            (256.0 * 1024.0 * 1024.0, 169_105_291.0),
        ];
        for (budget, peak) in runs {
            let modelled = (budget + ENGINE_FLOOR) * WASM_ALLOC_MARGIN;
            assert!(modelled > peak, "budget {budget}: modelled {modelled} does not cover measured {peak}");
        }
        // …and the floor is not so fat that it re-refuses what the epic exists to allow.
        let ceiling = 128.0 * 1024.0 * 1024.0;
        assert!(ENGINE_FLOOR < ceiling, "{ENGINE_FLOOR} has drifted past the {ceiling} sanity line");
    }

    /// v14's filler, priced against [`OUTPUT_PER_CELL_BYTE`]'s rounding.
    ///
    /// The per-chunk share is on both sides of the ratio — the downloaded cells are v14 too and
    /// their chunks are copied verbatim — so the only term this coefficient has to absorb is the
    /// per-region one, and the 0.11 % the rounding already gives back covers it by a wide margin on
    /// every selection the builder can offer.
    #[test]
    fn the_rounding_covers_the_v14_region_gaps() {
        // The measured ratio the coefficient rounds up from, and the headroom that buys.
        let measured = 0.9989;
        for cells in [catalog::FREIBURG_CELLS, catalog::BW_CELLS, catalog::DACH_CELLS] {
            let headroom = (OUTPUT_PER_CELL_BYTE - measured) * cells;
            assert!(
                headroom > 400.0 * REGION_GAP_BYTES,
                "{cells} B of cells: {headroom} B of rounding headroom is not a comfortable margin over the \
                 {REGION_GAP_BYTES} B of §1.2 region gaps"
            );
        }
    }

    /// Degenerate inputs must not produce a nonsense verdict.
    #[test]
    fn degenerate_inputs_clamp() {
        let zero = estimate_memory(-1.0, -1.0, -1.0, SORT, Residency::resident());
        assert!(zero.fits);
        let nav_only = estimate_memory(100.0 * MB, 0.0, 0.0, SORT, Residency::resident());
        assert_eq!(nav_only.input_bytes, 100.0 * MB, "total_cell_bytes cannot be below the network band's share");
        let bad_budget = estimate_memory(20.0 * MB, 60.0 * MB, 0.0, f64::NAN, Residency::streamed());
        assert!(bad_budget.engine_bytes > 0.0, "a NaN sort budget falls back to the engine default");
        let all_terrain = estimate_memory(0.0, 50.0 * MB, 80.0 * MB, SORT, Residency::streamed());
        assert!(all_terrain.input_bytes <= 50.0 * MB + 2.0 * READ_CACHE_BYTES, "terrain clamps to the total");
    }

    /// The mobile override, unchanged in spirit: the projection is about the selection and mode,
    /// the budget is the caller's to lower, and a nonsense budget falls back rather than refusing
    /// everything.
    #[test]
    fn the_budget_override_and_its_fallback_still_hold() {
        let r = Residency { input_on_disk: true, output_sunk: false };
        let desktop = estimate_memory(catalog::BW_NAV, catalog::BW_CELLS, catalog::BW_TERRAIN, SORT, r);
        let phone = estimate_memory_with_budget(
            catalog::BW_NAV,
            catalog::BW_CELLS,
            catalog::BW_TERRAIN,
            SORT,
            r,
            1024.0 * 1024.0 * 1024.0,
        );
        assert!(desktop.fits && !phone.fits, "a resident BW map fits a desktop tab and not a 1 GiB one");
        assert_eq!(desktop.peak_bytes, phone.peak_bytes);
        for bad in [f64::NAN, 0.0, -1.0, f64::INFINITY] {
            let e = estimate_memory_with_budget(20.0 * MB, 60.0 * MB, 0.0, SORT, Residency::streamed(), bad);
            assert_eq!(e.budget_bytes, PRACTICAL_BUDGET, "budget {bad} should have fallen back");
            assert!(e.fits);
        }
    }
}
