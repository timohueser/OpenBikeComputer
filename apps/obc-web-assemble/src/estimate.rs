//! Pre-download wasm memory projection: `peak = engine + resident_input + resident_output`.
//!
//! The engine term is the sort budget, not the map. The merge and the verify pass hold their
//! per-node and per-edge bookkeeping in sorted passes spilled through the scratch seam, bounded by
//! `merge_budget_bytes`, over a residual floor that does not scale with the region.
//!
//! The other two terms are modes. One OPFS probe answers for all three seams, because the cells,
//! the spill and the map sink ride the same sync-access-handle capability, so
//! [`Residency::input_on_disk`] states the host and not one seam.
//!
//! On an OPFS host with a sunk output, the input is the block caches plus the terrain squares, which
//! are always handed over as buffers, and the output is nothing. This is the only mode a country
//! assembles in: a country-scale map is larger than the address space, so no resident variant of
//! that selection exists. An OPFS host that keeps the finished map pays for all of it, and the two
//! verdicts differ enough to be computed apart. With no usable OPFS, the cells, the spill and the
//! whole map are all resident, and such a browser honestly cannot do a country.

use crate::driver::{DEFAULT_READ_BLOCK, READ_CACHE_BLOCKS, VERIFY_READ_BLOCK};

/// The engine's residual floor over the sort budget: per-cell transients, the first-fit bin table,
/// the seam table, both block caches, and slack. It covers a country-scale selection with about
/// twice the headroom the measured residuals need.
pub const ENGINE_FLOOR: f64 = 96.0 * 1024.0 * 1024.0;

/// wasm-dlmalloc margin over the natively measured peaks. wasm's linear memory only grows, so
/// fragmentation within a run is permanent in a way it is not natively.
pub const WASM_ALLOC_MARGIN: f64 = 1.15;

/// The buffered fallback's spill, resident in `MemoryScratch`, per byte of selected `network`
/// band: the collected edge stream, the adjacency entries, the claim sort. It covers the sum of the
/// concurrent streams with margin. Only the no-OPFS path pays it.
pub const SPILL_PER_NAV_BYTE: f64 = 2.5;

/// Output bytes per byte of input cell. Geometry chunks are copied verbatim and the nav section is
/// rewritten to about the size the cells' own had, so the map comes out the size of its inputs. The
/// measured ratio is a little under `1.0`, and rounding up gives back the margin that covers the
/// gaps an assembly adds at its own region and section boundaries.
///
/// A terrain square is an input byte and is copied verbatim into the map, so it contributes the same
/// count to each side.
pub const OUTPUT_PER_CELL_BYTE: f64 = 1.0;

/// The region gaps a full-ladder assembly adds over its inputs, worst case: about 50 boundaries at
/// `U - 1` bytes each.
#[cfg(test)]
const REGION_GAP_BYTES: f64 = 50.0 * 15.0;

/// Default input cache residency, shared across all source cells.
pub const INPUT_READ_CACHE_BYTES: f64 = (READ_CACHE_BLOCKS * DEFAULT_READ_BLOCK) as f64;
/// Sealed-output verification cache residency.
pub const VERIFY_READ_CACHE_BYTES: f64 = (READ_CACHE_BLOCKS * VERIFY_READ_BLOCK) as f64;

/// wasm32's hard address space. Nothing can be allocated past this, whatever the machine has.
pub const WASM32_ADDRESS_SPACE: f64 = 4.0 * 1024.0 * 1024.0 * 1024.0;

/// The default budget this crate reports `fits` against: 75 % of the address space.
///
/// A judgement, not a measurement. A wasm allocation that cannot be served aborts the module, so
/// there is no `Err` to render after the tab has paid for the whole download. Browsers also do not
/// reliably grant the full 4 GiB.
pub const PRACTICAL_BUDGET: f64 = 3.0 * 1024.0 * 1024.0 * 1024.0;

/// Which escapes from linear memory this run will have. The projection is a property of the
/// selection and the mode: a caller that keeps the finished map and one that sinks it disagree, and
/// a no-OPFS browser runs a different engine profile.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Residency {
    /// This browser passed the sync-access-handle probe with room to spare, so the cells read from
    /// OPFS and the spill lives there. `output_sunk` rides the same capability.
    pub input_on_disk: bool,
    /// The map is written straight to the host through a `MapWrites` sink and is never resident.
    /// `false` keeps the whole file in wasm memory until the caller takes it.
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
    /// The engine's working set: budget-bounded on an OPFS host, budget plus resident spill on the
    /// fallback. The wasm allocator margin is already applied.
    pub engine_bytes: f64,
    /// The resident input: the whole selection, or the block caches plus terrain when the cells stay
    /// in OPFS.
    pub input_bytes: f64,
    /// The resident output: the whole map, or nothing when it goes to the host's own storage.
    pub output_bytes: f64,
    /// The sum: what wasm32 has to hold at once.
    pub peak_bytes: f64,
    /// The budget [`MemoryEstimate::fits`] was decided against.
    pub budget_bytes: f64,
    /// [`WASM32_ADDRESS_SPACE`].
    pub ceiling_bytes: f64,
    /// `peak_bytes <= budget_bytes`, which is what a caller gates the download on.
    pub fits: bool,
    /// `budget_bytes - peak_bytes`, negative when it does not fit.
    pub headroom_bytes: f64,
}

/// Project the peak memory of assembling a selection, from the catalog's own byte counts.
///
/// `network_band_bytes` is the selected cells of the `network` band. `total_cell_bytes` is every
/// selected cell of every band plus the terrain squares, and `terrain_bytes` is the terrain squares'
/// share of that total: terrain is handed over as buffers and never read from OPFS.
/// `merge_budget_bytes` is the engine's sort budget, which is the engine term on an OPFS host; a
/// non-positive or non-finite value falls back to the engine's own default.
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

/// [`estimate_memory`], against a budget the caller chooses. A phone's tab is evicted rather than
/// slowed, so a caller that knows it is on one should lower the number. A non-finite or non-positive
/// budget falls back to [`PRACTICAL_BUDGET`].
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
        ((sort_budget + ENGINE_FLOOR) * WASM_ALLOC_MARGIN, INPUT_READ_CACHE_BYTES + VERIFY_READ_CACHE_BYTES + terrain)
    } else {
        // The fallback: spill in MemoryScratch and cells in memory. No wasm margin on the spill
        // term, because its coefficient is already the margin.
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
    /// The sort budget the builder runs with.
    const SORT: f64 = 256.0 * 1024.0 * 1024.0;

    /// The published catalog's figures for the measured regions, plus the country-scale shape.
    mod catalog {
        pub const FREIBURG_NAV: f64 = 90_052_777.0;
        pub const FREIBURG_TERRAIN: f64 = 23_069_068.0;
        pub const FREIBURG_CELLS: f64 = 263_616_395.0 + FREIBURG_TERRAIN;
        pub const BW_NAV: f64 = 295_921_548.0;
        pub const BW_TERRAIN: f64 = 58_721_264.0;
        pub const BW_CELLS: f64 = 794_735_626.0 + BW_TERRAIN;
        /// DACH: one file of about 9 GB, 8.5 GB of cells and 430 MiB of raster.
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

    /// A country-scale selection projects at about 0.88 GB on the sunk path: the budget-bounded
    /// engine, two block caches, and the raster on the way in.
    #[test]
    fn dach_fits_the_sunk_path_and_that_is_the_epic() {
        let e = streamed(catalog::DACH_NAV, catalog::DACH_CELLS, catalog::DACH_TERRAIN);
        assert!(e.fits, "DACH must fit — {} B against {} B", e.peak_bytes, e.budget_bytes);
        assert!((e.peak_bytes - 0.877e9).abs() < 2e7, "{}", e.peak_bytes);
        assert!(e.headroom_bytes > 0.5 * PRACTICAL_BUDGET, "…with real headroom: {}", e.headroom_bytes);
        assert_eq!(e.output_bytes, 0.0, "a sunk map is never wasm's");
        // The engine term does not scale with the map: the two selections differ only in terrain.
        let f = streamed(catalog::FREIBURG_NAV, catalog::FREIBURG_CELLS, catalog::FREIBURG_TERRAIN);
        assert_eq!(e.engine_bytes, f.engine_bytes, "the engine term is the budget, not the map");
    }

    /// A caller that keeps the finished map pays for all of it, so a country refuses and a
    /// Bundesland does not. The two verdicts stay separate for this reason.
    #[test]
    fn dach_refuses_a_resident_map_and_bw_does_not() {
        let dach = kept(catalog::DACH_NAV, catalog::DACH_CELLS, catalog::DACH_TERRAIN);
        assert!(!dach.fits);
        assert!(dach.peak_bytes > WASM32_ADDRESS_SPACE, "past the address space, not merely the budget");
        let bw = kept(catalog::BW_NAV, catalog::BW_CELLS, catalog::BW_TERRAIN);
        assert!(bw.fits, "{}", bw.peak_bytes);
    }

    /// With no usable OPFS the spill and the whole map are resident, so a Bundesland still fits and
    /// a country does not.
    #[test]
    fn the_no_opfs_fallback_admits_bw_and_refuses_dach() {
        let bw = fallback(catalog::BW_NAV, catalog::BW_CELLS, catalog::BW_TERRAIN);
        assert!(bw.fits, "{}", bw.peak_bytes);
        let dach = fallback(catalog::DACH_NAV, catalog::DACH_CELLS, catalog::DACH_TERRAIN);
        assert!(!dach.fits);
        assert!(dach.headroom_bytes < 0.0);
    }

    /// On the sunk path the difference between the two regions is terrain, not graph.
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

    /// The floor plus margin must cover every measured peak at the budget the run had.
    #[test]
    fn the_floor_covers_every_measured_peak_with_margin_to_declare() {
        // (budget, tracked peak) from the memory harness.
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
        // The floor must not be so fat that it refuses a selection the model should admit.
        let ceiling = 128.0 * 1024.0 * 1024.0;
        assert!(ENGINE_FLOOR < ceiling, "{ENGINE_FLOOR} has drifted past the {ceiling} sanity line");
    }

    /// The chunk filler is on both sides of the ratio, because the downloaded cells carry it too,
    /// so the only term [`OUTPUT_PER_CELL_BYTE`] has to absorb is the per-region one.
    #[test]
    fn the_rounding_covers_the_v14_region_gaps() {
        // The measured ratio the coefficient rounds up from.
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
        assert!(
            all_terrain.input_bytes <= 50.0 * MB + INPUT_READ_CACHE_BYTES + VERIFY_READ_CACHE_BYTES,
            "terrain clamps to the total"
        );
    }

    /// The projection is about the selection and the mode; the budget is the caller's to lower, and
    /// a nonsense budget falls back rather than refusing everything.
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
