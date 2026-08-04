//! Pre-download wasm memory projection.
//!
//! `peak ≈ ENGINE × nav_bytes + resident_input + resident_output`
//!
//! The engine term is measured; the other two are **modes**, because #1116's phase B made them
//! modes. Before it, this bridge held two full copies of the selection on top of the engine — every
//! downloaded cell in linear memory for the whole run, every finished shard accumulating until the
//! caller took them. B1 (#1124) hands each shard out the moment its §4.8 read-back passes, so the
//! resident output is **one shard**. B2 (#1126) leaves downloaded cells in OPFS and reads them
//! through a synchronous callback and a small block cache, so the resident input is **the cache**.
//! A projection that still charged both full copies would refuse selections the run can now do —
//! and a refusal is not free: it is the instruction "cover less ground", given wrongly.
//!
//! Neither mode is unconditional, and the projection must not assume what the run cannot deliver:
//!
//! * **Input** streams only when the browser grants an OPFS with room *and* the worker's
//!   sync-read probe passes ([`Residency::input_on_disk`]). The fallback reads the cells into wasm
//!   memory — the pre-B shape, full cells resident. Terrain squares are deliberately never stored
//!   on disk (they are small and downloaded last), so they stay resident even when the cells
//!   stream; the streamed input term is `cache + terrain_bytes`, not zero.
//! * **Output** streams only for a caller that can take a file before `planned` — the browser
//!   download path. The device path sends `planned`'s counts first and keeps the whole set until
//!   then ([`Residency::streamed_shard_bytes`] `= 0`), so one selection can honestly fit a
//!   download and not fit a direct device send, and the two verdicts are computed separately
//!   rather than averaged. The streamed term is one shard plus the terrain sink, capped at the
//!   whole set — a set smaller than a shard is wholly resident just before it is taken.
//!
//! The **`ENGINE` term** is where the C-series was measured, and it has not moved since:
//! `obcm-assemble --features mem-profile` (PR #1117) reports peak heap per phase, and two published
//! catalog regions were run end to end on the streamed engine — records by handle and sink emission
//! (#1118), sorted dedup and CSR adjacency (#1120), the two-walk verify over dense ids (#1119).
//!
//! | | network band | tracked peak | ×nav | peak RSS | ×nav | peak phase |
//! | :-- | --: | --: | --: | --: | --: | :-- |
//! | freiburg-regbez (77 cells) | 90.1 MB | 352.4 MB | 3.91 | 367.7 MB | 4.08 | merge nav |
//! | baden-württemberg (215 cells) | 295.9 MB | 1069.6 MB | 3.62 | 1126.9 MB | 3.81 | merge nav |
//!
//! macOS arm64, release, published v12 catalog, verify **on**. Native runs read their cells from
//! disk and stream their shards to it, so the tracked heap peak *is* the engine term with nothing
//! else in it — which is exactly the quantity this model needs.
//!
//! **From 4.08 to the shipped 4.7.** The base is the larger of the two *resident* ratios, 4.08×,
//! and taking the small region's is deliberate: the ratio **falls** with scale (4.08 → 3.81),
//! because the merge's per-junction structures grow with the graph while the per-cell and per-shard
//! overheads do not. Extrapolating with the pessimistic end means a bigger selection inherits slack
//! rather than debt. On top of that, ×1.15 for the allocator this was *not* measured on: the harness
//! counts bytes requested, macOS `System` added 4–5 % of that in touched pages, and wasm runs
//! dlmalloc inside a linear memory that only ever **grows** — a freed block is reusable but never
//! returned, so fragmentation within a run is permanent in a way it is not natively. 4.08 × 1.15 =
//! 4.69, shipped as **4.7**.
//!
//! In the streamed modes the sum is nearly phase-exact — the engine's peak is the nav merge, before
//! a single output byte exists, and a 256 MB shard is small next to it. In the resident modes the
//! sum still over-counts that peak by the full output copy, exactly as it always did, and that
//! conservatism is kept on purpose: those are the modes where a mis-projection ends in a wasm abort
//! after the whole download and rewrite are spent, with no error left to render.
//!
//! **What this does not cover.** Two regions on one machine, one schema revision, one skin. Nothing
//! at all above BW's 296 MB of nav has been measured, and the linear form is an assumption about
//! junction density that the two points are consistent with rather than proof of. The read cache is
//! priced at its default size ([`crate::driver`]'s 16 × 64 KiB); a caller that raises
//! `read_block_bytes` to its 4 MiB ceiling grows that term to 64 MiB, which this model rounds into
//! the engine margin rather than parameterizing.

use crate::driver::{DEFAULT_READ_BLOCK, READ_CACHE_BLOCKS};

/// Peak engine bytes per byte of selected `network` band: `4.08 × 1.15`, from the #1116 harness runs
/// on the streamed engine. See the module header for the two measured points and the margin.
///
/// Was 6.4 before the C-series, on PR #1027's single switzerland run.
pub const PEAK_PER_NAV_BYTE: f64 = 4.7;

/// Output bytes per byte of input cell. Geometry chunks are copied verbatim and the nav section is
/// rewritten to about the size the cells' own had, so the set comes out the size of its inputs:
/// measured 0.9988 (freiburg) and 0.9989 (baden-württemberg) on the runs above. Kept at `1.0`, which
/// rounds the right way.
pub const OUTPUT_PER_CELL_BYTE: f64 = 1.0;

/// The streamed input path's whole residency: the block cache the wasm side serves the engine's
/// record-at-a-time walks from, at its default geometry (`driver.rs`). Independent of how many
/// cells the selection has — that independence is what B2 bought.
pub const READ_CACHE_BYTES: f64 = (READ_CACHE_BLOCKS * DEFAULT_READ_BLOCK) as f64;

/// wasm32's hard address space. Nothing can be allocated past this, whatever the machine has.
pub const WASM32_ADDRESS_SPACE: f64 = 4.0 * 1024.0 * 1024.0 * 1024.0;

/// The **default** budget this crate reports `fits` against: **3 GiB**, 75 % of the address space.
///
/// Not a measurement — a judgement, and the reason is that the failure mode is unforgiving. A wasm
/// allocation that cannot be served aborts the module: there is no `Err` to render, the tab has
/// already spent the whole download and the whole nav rewrite, and the rider sees a crash. Browsers
/// also do not reliably grant the full 4 GiB (the limit is per-tab and platform-dependent), and the
/// engine model above is measured at two scales on one machine. A quarter of the space is the margin
/// those three facts together are worth.
///
/// It is also a **desktop-shaped** judgement. A phone's per-tab limit is far lower and its tabs are
/// evicted rather than merely slowed, so a caller that knows it is on a mobile UA should lower it —
/// see [`estimate_memory_with_budget`], which is what the browser wrapper's `budgetBytes` override
/// reaches.
pub const PRACTICAL_BUDGET: f64 = 3.0 * 1024.0 * 1024.0 * 1024.0;

/// Which of #1116 phase B's two escapes from linear memory this run will actually have. The
/// projection is a property of the selection *and the mode* — pretending otherwise is how the
/// pre-fix model came to refuse selections the download path could do and pass ones the device
/// path could not.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Residency {
    /// Cells are read through the OPFS seam (#1116 B2): the browser granted a writable store with
    /// room, and the worker's sync-read probe passed. Resident input is the read cache plus the
    /// terrain squares (never stored on disk) instead of the selection.
    pub input_on_disk: bool,
    /// `> 0`: shards are handed out the moment their §4.8 pass ends (#1116 B1), split at this many
    /// bytes — the resident output is one shard plus the terrain sink, capped at the whole set.
    /// `0`: the caller keeps the set until the run ends (the device path, which needs `planned`'s
    /// counts before it can take a file), and the whole output is resident.
    pub streamed_shard_bytes: f64,
}

impl Residency {
    /// Both escapes on, at the builder's shard split. What the browser **download** path runs.
    pub fn streamed(shard_bytes: f64) -> Residency {
        Residency { input_on_disk: true, streamed_shard_bytes: shard_bytes }
    }

    /// Neither escape: cells in memory, set kept to the end. The pre-B shape, and still what a
    /// browser without a usable OPFS gives the device path.
    pub fn resident() -> Residency {
        Residency { input_on_disk: false, streamed_shard_bytes: 0.0 }
    }
}

/// What an assembly of this size would cost, and whether a browser can pay it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MemoryEstimate {
    /// The engine's working set — the nav rewrite (§4.6) and everything it drags with it.
    pub engine_bytes: f64,
    /// The **resident** input: the whole selection, or the read cache plus terrain when the cells
    /// stay in OPFS.
    pub input_bytes: f64,
    /// The **resident** output: the whole set (§4.8 needs written bytes addressable, and the device
    /// path keeps them), or one shard plus the terrain sink when shards stream out.
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
/// * `total_cell_bytes` — every selected cell of every band **plus the terrain squares**, which is
///   also what the download costs.
/// * `terrain_bytes` — the terrain squares' share of that total (0 for a terrain-less catalog).
///   Needed separately because terrain rides outside both of phase B's escapes: it is never stored
///   in OPFS and its sink accumulates for the whole run.
///
/// See the module header for the model and its measured constants. Byte counts are `f64` because
/// they cross from JS, where a count past 2^53 is not representable anyway — and 9 PB of cells is
/// not the case this function is for.
pub fn estimate_memory(
    network_band_bytes: f64,
    total_cell_bytes: f64,
    terrain_bytes: f64,
    residency: Residency,
) -> MemoryEstimate {
    estimate_memory_with_budget(network_band_bytes, total_cell_bytes, terrain_bytes, residency, PRACTICAL_BUDGET)
}

/// [`estimate_memory`], against a budget the caller chooses.
///
/// [`PRACTICAL_BUDGET`] is a desktop-shaped judgement, and `fits` is a *verdict* — so the one knob
/// worth exposing is the number the verdict is measured against. The case that needs it is a mobile
/// UA, where the per-tab allowance is a fraction of a desktop's and the tab is killed rather than
/// slowed; a caller that can detect that should pass what it believes the device will grant.
///
/// A non-finite or non-positive budget falls back to [`PRACTICAL_BUDGET`]: `fits` must always be a
/// verdict about something, and silently answering "nothing fits" to a caller that passed `NaN`
/// through from an unparsed setting would be the worst of both.
pub fn estimate_memory_with_budget(
    network_band_bytes: f64,
    total_cell_bytes: f64,
    terrain_bytes: f64,
    residency: Residency,
    budget: f64,
) -> MemoryEstimate {
    let budget_bytes = if budget.is_finite() && budget > 0.0 { budget } else { PRACTICAL_BUDGET };
    let nav = network_band_bytes.max(0.0);
    let cells = total_cell_bytes.max(nav);
    let terrain = terrain_bytes.clamp(0.0, cells);
    let engine_bytes = PEAK_PER_NAV_BYTE * nav;
    let input_bytes = if residency.input_on_disk { READ_CACHE_BYTES + terrain } else { cells };
    let whole_set = OUTPUT_PER_CELL_BYTE * cells;
    let output_bytes = if residency.streamed_shard_bytes > 0.0 {
        // One shard between its verify and its eviction, plus the terrain sink — but never more
        // than the set itself: a map smaller than a shard is wholly resident just before it is
        // taken, not larger than itself.
        whole_set.min(residency.streamed_shard_bytes + terrain)
    } else {
        whole_set
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
    /// The split the builder always asks for (`DownloadStep.svelte`'s `TARGET_SHARD_BYTES`).
    const SHARD: f64 = 256.0 * 1024.0 * 1024.0;

    /// The published catalog's own figures for the two regions the model is measured on, and the
    /// only region byte counts in this file that are not extrapolations.
    ///
    /// `cells` is what the builder's ledger passes as `total_cell_bytes`: every band **plus** the
    /// terrain squares (`ledger.totalBytes`), because those are downloaded and handed to
    /// `addTerrainCell` like any other buffer. `terrain` is the raster's own share.
    mod catalog {
        /// `europe/germany/baden-wuerttemberg/freiburg-regbez` — 77 cells.
        pub const FREIBURG_NAV: f64 = 90_052_777.0;
        pub const FREIBURG_TERRAIN: f64 = 23_069_068.0;
        pub const FREIBURG_CELLS: f64 = 263_616_395.0 + FREIBURG_TERRAIN;
        /// `europe/germany/baden-wuerttemberg` — 215 cells.
        pub const BW_NAV: f64 = 295_921_548.0;
        pub const BW_TERRAIN: f64 = 58_721_264.0;
        pub const BW_CELLS: f64 = 794_735_626.0 + BW_TERRAIN;
    }

    fn streamed(nav: f64, cells: f64, terrain: f64) -> MemoryEstimate {
        estimate_memory(nav, cells, terrain, Residency::streamed(SHARD))
    }

    /// The device path's shape: cells stream from OPFS, but the set is kept until `planned`.
    fn device(nav: f64, cells: f64, terrain: f64) -> MemoryEstimate {
        estimate_memory(nav, cells, terrain, Residency { input_on_disk: true, streamed_shard_bytes: 0.0 })
    }

    /// **The assertion epic #1116 exists for**, phase B included. Baden-Württemberg — 215 cells,
    /// 296 MB of navigation graph — was refused outright by the pre-#1116 model (3.60 GB projected
    /// against a 3 GiB budget), fit with under 4 % to spare after the C-series alone, and now
    /// projects at ~1.78 GB on the download path: the engine plus one shard plus a cache, with the
    /// selection itself on disk. Even the device path — whole set resident — clears the budget.
    #[test]
    fn baden_wuerttemberg_fits_every_mode_and_did_not_before() {
        let e = streamed(catalog::BW_NAV, catalog::BW_CELLS, catalog::BW_TERRAIN);
        assert!(e.fits, "BW must fit — {} B against {} B", e.peak_bytes, e.budget_bytes);
        assert!((e.peak_bytes - 1.778e9).abs() < 1e7, "{}", e.peak_bytes);
        assert!(
            e.headroom_bytes > 0.15 * PRACTICAL_BUDGET,
            "no caution band on the download path: {}",
            e.headroom_bytes
        );

        let d = device(catalog::BW_NAV, catalog::BW_CELLS, catalog::BW_TERRAIN);
        assert!(d.fits, "BW to a device must fit too — {} B", d.peak_bytes);
        assert!((d.peak_bytes - 2.304e9).abs() < 1e7, "{}", d.peak_bytes);
        assert!(d.peak_bytes > e.peak_bytes, "keeping the set is strictly dearer than streaming it");

        // What the pre-B shape — both escapes off — says about the same selection: the pre-fix
        // model's own number, reproduced as the worst mode rather than as the only one.
        let worst = estimate_memory(catalog::BW_NAV, catalog::BW_CELLS, catalog::BW_TERRAIN, Residency::resident());
        assert!((worst.peak_bytes - 3.098e9).abs() < 1e7, "{}", worst.peak_bytes);
        // …and what the pre-#1116 engine coefficient would have said: refused outright.
        let before = 6.4 * catalog::BW_NAV + 2.0 * catalog::BW_CELLS;
        assert!(before > PRACTICAL_BUDGET, "the old model refused BW at {before} B — that is the thing that changed");
    }

    /// The other measured point: a Regierungsbezirk streams at ~0.74 GB — comfortable even on the
    /// builder's 1 GiB phone judgement, where the C-series model had it at 997 MB, inside the
    /// caution band. A set smaller than ~a shard is wholly resident just before it is taken, so the
    /// streamed and kept output terms converge here instead of the streamed one over-charging.
    #[test]
    fn freiburg_regbez_fits_a_phone_with_room_now() {
        let e = streamed(catalog::FREIBURG_NAV, catalog::FREIBURG_CELLS, catalog::FREIBURG_TERRAIN);
        assert!(e.fits);
        assert!((e.peak_bytes - 734.1 * MB).abs() < 5.0 * MB, "{}", e.peak_bytes);

        let phone = estimate_memory_with_budget(
            catalog::FREIBURG_NAV,
            catalog::FREIBURG_CELLS,
            catalog::FREIBURG_TERRAIN,
            Residency::streamed(SHARD),
            1024.0 * 1024.0 * 1024.0,
        );
        assert!(phone.fits, "{} B against a 1 GiB tab", phone.peak_bytes);
        assert!(
            phone.headroom_bytes > 0.15 * phone.budget_bytes,
            "no longer the caution case: {}",
            phone.headroom_bytes
        );

        let d = device(catalog::FREIBURG_NAV, catalog::FREIBURG_CELLS, catalog::FREIBURG_TERRAIN);
        assert_eq!(d.output_bytes, e.output_bytes, "below one shard the two output modes are the same bytes");
    }

    /// **Where the edge is now.** On the download path the input and output terms are constants, so
    /// the budget runs out where the *engine* does: at the catalog's measured density
    /// (nav ≈ 0.347 × cells, terrain ≈ 0.069 × cells) that is ~1.67 GB of cells — about 1.96× BW,
    /// against 890 MB before phase B. The device path keeps the whole set, so its edge is nearer:
    /// ~1.19 GB of cells, 1.4× BW. Both edges are engine-bound or output-bound, and **neither is
    /// DACH-shaped** — see `dach_does_not_fit_in_any_mode`.
    #[test]
    fn the_edge_is_engine_bound_on_the_download_path() {
        let nav_density = catalog::BW_NAV / catalog::BW_CELLS;
        let terrain_density = catalog::BW_TERRAIN / catalog::BW_CELLS;

        // peak(c) = E·d·c + (CACHE + t·c) + (SHARD + t·c), solved for peak = budget.
        let edge =
            (PRACTICAL_BUDGET - READ_CACHE_BYTES - SHARD) / (PEAK_PER_NAV_BYTE * nav_density + 2.0 * terrain_density);
        assert!((edge - 1_670.0 * MB).abs() < 20.0 * MB, "the download edge moved to {edge} B");
        assert!(edge > 1.9 * catalog::BW_CELLS, "phase B roughly doubled the C-series' 890 MB edge");
        let at_edge = streamed(nav_density * edge, edge, terrain_density * edge);
        assert!(at_edge.fits && at_edge.headroom_bytes < 1.0 * MB, "{}", at_edge.headroom_bytes);

        // The device path's edge: the set itself joins the sum.
        let device_edge = (PRACTICAL_BUDGET - READ_CACHE_BYTES)
            / (PEAK_PER_NAV_BYTE * nav_density + terrain_density + OUTPUT_PER_CELL_BYTE);
        assert!((device_edge - 1_193.0 * MB).abs() < 20.0 * MB, "the device edge moved to {device_edge} B");
        assert!(device_edge < edge, "keeping the set must always bind earlier than streaming it");
    }

    /// **What phase B bought, stated as the next Bundesland up.** Bayern has no published catalog
    /// entry, so this extrapolates from BW at 1.7× — the ratio of the two Geofabrik extracts, and
    /// the *low* end of the plausible range (by area it is 1.97×). The pre-fix model put it past
    /// wasm32's address space entirely; on the download path it now projects at ~2.83 GB and fits,
    /// inside the caution band. The device path keeps a 1.45 GB set next to a 2.36 GB merge and is
    /// honestly refused — the two verdicts differ, which is exactly why they are computed apart.
    #[test]
    fn bayern_scale_fits_the_download_path_and_not_the_device_path() {
        let (nav, cells, terrain) = (1.7 * catalog::BW_NAV, 1.7 * catalog::BW_CELLS, 1.7 * catalog::BW_TERRAIN);

        let e = streamed(nav, cells, terrain);
        assert!(e.fits, "Bayern-scale must fit the download path now — {} B", e.peak_bytes);
        assert!((e.peak_bytes - 2.834e9).abs() < 1e7, "{}", e.peak_bytes);
        assert!(e.headroom_bytes < 0.15 * PRACTICAL_BUDGET, "…but inside the caution band, which the UI should show");

        let d = device(nav, cells, terrain);
        assert!(!d.fits, "keeping a 1.45 GB set next to the merge does not fit — {} B", d.peak_bytes);
        assert!(d.headroom_bytes < 0.0, "a negative headroom is what the UI shows");
    }

    /// DACH stays a native-bakery job, and no residency mode can argue otherwise: from
    /// [`cell_size_survey.rs`](../../../host/obc-pack/examples/cell_size_survey.rs)'s shape — a
    /// core of 2.8–3.0 GiB and ~8.5 GB of cells all told — the **engine term alone** is ~14 GB,
    /// past wasm32's whole address space more than three times over. Phase B removed the input and
    /// output copies; it did not touch the merge's own working set, which is what binds here.
    #[test]
    fn dach_does_not_fit_in_any_mode() {
        let e = streamed(3.0e9, 8.5e9, 0.5e9);
        assert!(!e.fits);
        assert!(
            e.engine_bytes > 3.0 * WASM32_ADDRESS_SPACE,
            "the engine term alone is the refusal: {}",
            e.engine_bytes
        );
        assert!(e.headroom_bytes < 0.0);
    }

    /// A corridor — the common selection, which must be waved straight through in every mode,
    /// including the pre-B shape a browser with no usable OPFS still runs.
    #[test]
    fn a_corridor_sized_selection_fits_comfortably_in_every_mode() {
        for r in [Residency::streamed(SHARD), Residency::resident()] {
            let e = estimate_memory(20.0 * MB, 60.0 * MB, 10.0 * MB, r);
            assert!(e.fits);
            assert!(e.headroom_bytes > 2.9e9, "{}", e.headroom_bytes);
        }
    }

    /// The margin is a claim about numbers, so it is pinned: the shipped coefficient must cover both
    /// measured *resident* peaks with the derivation's 1.15 to spare, and must not have drifted so
    /// far above them that it re-refuses what the epic set out to allow.
    #[test]
    fn the_coefficient_covers_both_measured_runs_with_the_stated_margin() {
        // (network band bytes, peak RSS) — see the module header.
        let runs = [(catalog::FREIBURG_NAV, 367_656_960.0), (catalog::BW_NAV, 1_126_858_752.0)];
        let worst = runs.iter().map(|(nav, rss)| rss / nav).fold(f64::MIN, f64::max);
        assert!((worst - 4.083).abs() < 0.01, "the freiburg run is the pessimistic end: {worst}");
        assert!(PEAK_PER_NAV_BYTE >= worst, "{PEAK_PER_NAV_BYTE} must cover every measured ratio");
        assert!(
            PEAK_PER_NAV_BYTE <= 1.16 * worst,
            "{PEAK_PER_NAV_BYTE} is further above the worst measured {worst} than the derivation's 1.15 justifies"
        );
    }

    /// Degenerate inputs must not produce a nonsense verdict: a negative byte count clamps to zero,
    /// a terrain share past the total clamps to the total, and a caller that passes only the network
    /// total (forgetting the geometry bands) still gets an estimate that counts the nav cells.
    #[test]
    fn degenerate_inputs_clamp() {
        let zero = estimate_memory(-1.0, -1.0, -1.0, Residency::resident());
        assert_eq!(zero.peak_bytes, 0.0);
        assert!(zero.fits);
        let nav_only = estimate_memory(100.0 * MB, 0.0, 0.0, Residency::resident());
        assert_eq!(nav_only.input_bytes, 100.0 * MB, "total_cell_bytes cannot be below the network band's own share");
        let all_terrain = estimate_memory(0.0, 50.0 * MB, 80.0 * MB, Residency::streamed(SHARD));
        assert!(all_terrain.input_bytes <= 50.0 * MB + READ_CACHE_BYTES, "terrain clamps to the total");
    }

    /// `fits` is a verdict against a **desktop-shaped judgement**, so a caller that knows better —
    /// a mobile UA, whose per-tab allowance is a fraction of this and whose tabs are evicted rather
    /// than slowed — can lower the number the verdict is measured against. Nothing else moves: the
    /// projection is a property of the selection and the mode, not of the device.
    #[test]
    fn a_caller_can_lower_the_budget_the_verdict_is_measured_against() {
        let r = Residency { input_on_disk: true, streamed_shard_bytes: 0.0 };
        let desktop = estimate_memory(catalog::BW_NAV, catalog::BW_CELLS, catalog::BW_TERRAIN, r);
        let phone = estimate_memory_with_budget(
            catalog::BW_NAV,
            catalog::BW_CELLS,
            catalog::BW_TERRAIN,
            r,
            1.0 * 1024.0 * 1024.0 * 1024.0,
        );
        assert!(desktop.fits && !phone.fits, "BW fits a desktop tab and nothing like a 1 GiB one");
        assert_eq!(desktop.peak_bytes, phone.peak_bytes, "the projection is about the selection, not the device");
        assert_eq!(phone.budget_bytes, 1.0 * 1024.0 * 1024.0 * 1024.0, "the estimate reports what it judged against");
        assert!(phone.headroom_bytes < 0.0);
        assert_eq!(phone.ceiling_bytes, WASM32_ADDRESS_SPACE, "wasm32's own ceiling is not the caller's to move");
    }

    /// A budget that is not a number is a caller's bug (an unparsed setting, a missing field), and
    /// the answer to it is the default verdict — never "nothing fits", which would refuse every
    /// selection on the strength of a `NaN`.
    #[test]
    fn a_nonsense_budget_falls_back_to_the_default() {
        for bad in [f64::NAN, 0.0, -1.0, f64::INFINITY] {
            let e = estimate_memory_with_budget(20.0 * MB, 60.0 * MB, 0.0, Residency::streamed(SHARD), bad);
            assert_eq!(e.budget_bytes, PRACTICAL_BUDGET, "budget {bad} should have fallen back");
            assert!(e.fits);
        }
    }
}
