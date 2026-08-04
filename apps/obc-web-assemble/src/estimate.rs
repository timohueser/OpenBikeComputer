//! Pre-download wasm memory projection.
//!
//! `peak ≈ ENGINE × nav_bytes + input_cell_bytes + output_cell_bytes`
//!
//! The **shape** is a property of this bridge rather than of the engine, and it has not moved:
//! [`Assembler::addCell`](crate) copies every downloaded cell into linear memory and the driver
//! holds those buffers for the whole run, while the finished shards accumulate in memory until the
//! caller takes them one by one (OBCA §4.8 needs the written bytes addressable to read them back).
//! So two full copies of the selection are resident on top of whatever the engine itself is doing.
//!
//! The **`ENGINE` term** is what epic #1116's C-series moved, and this is where the model was
//! re-derived. It is no longer PR #1027's one-run extrapolation: `obcm-assemble --features
//! mem-profile` (PR #1117) reports peak heap per phase, and two published catalog regions were run
//! end to end on the streamed engine — records by handle and sink emission (#1118), sorted dedup
//! and CSR adjacency (#1120), the two-walk verify over dense ids (#1119).
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
//! That is the whole margin, and deliberately not a blanket 2×, because the additive form above is
//! already a fourth and unpriced one: the engine's peak is the *nav merge*, which happens before a
//! single output byte exists, so summing the peak engine term with a full output copy over-counts
//! the phase-exact truth by about 1.3× at BW. Doubling on top of that would re-refuse the very
//! selections the epic exists to allow — and refusing wrongly is not free either. The other
//! direction is worse, though: a wasm allocation that cannot be served *aborts the module*, after
//! the download and the whole rewrite are already spent, so the coefficient errs high on purpose.
//!
//! **What this does not cover.** Two regions on one machine, one schema revision, one skin. Nothing
//! at all above BW's 296 MB of nav has been measured, and the linear form is an assumption about
//! junction density that the two points are consistent with rather than proof of. Terrain rides in
//! the input and output terms (the builder's ledger counts it) but never in the engine term — the
//! raster is copied cell by cell, it does not join the graph.

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

/// What an assembly of this size would cost, and whether a browser can pay it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MemoryEstimate {
    /// The engine's working set — the nav rewrite (§4.6) and everything it drags with it.
    pub engine_bytes: f64,
    /// The downloaded cells, resident for the whole run.
    pub input_bytes: f64,
    /// The assembled set, resident until the caller takes each file (§4.8 needs it addressable).
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
///
/// See the module header for the model and its measured constants. Both arguments are `f64` because
/// they cross from JS, where a byte count past 2^53 is not representable anyway — and 9 PB of cells
/// is not the case this function is for.
pub fn estimate_memory(network_band_bytes: f64, total_cell_bytes: f64) -> MemoryEstimate {
    estimate_memory_with_budget(network_band_bytes, total_cell_bytes, PRACTICAL_BUDGET)
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
pub fn estimate_memory_with_budget(network_band_bytes: f64, total_cell_bytes: f64, budget: f64) -> MemoryEstimate {
    let budget_bytes = if budget.is_finite() && budget > 0.0 { budget } else { PRACTICAL_BUDGET };
    let nav = network_band_bytes.max(0.0);
    let cells = total_cell_bytes.max(nav);
    let engine_bytes = PEAK_PER_NAV_BYTE * nav;
    let output_bytes = OUTPUT_PER_CELL_BYTE * cells;
    let peak_bytes = engine_bytes + cells + output_bytes;
    MemoryEstimate {
        engine_bytes,
        input_bytes: cells,
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

    /// The published catalog's own figures for the two regions the model is measured on, and the
    /// only region byte counts in this file that are not extrapolations.
    ///
    /// `cells` is what the builder's ledger passes as `total_cell_bytes`: every band **plus** the
    /// terrain squares (`ledger.totalBytes`), because those are downloaded and handed to
    /// `addTerrainCell` like any other buffer.
    mod catalog {
        /// `europe/germany/baden-wuerttemberg/freiburg-regbez` — 77 cells.
        pub const FREIBURG_NAV: f64 = 90_052_777.0;
        pub const FREIBURG_CELLS: f64 = 263_616_395.0 + 23_069_068.0;
        /// `europe/germany/baden-wuerttemberg` — 215 cells.
        pub const BW_NAV: f64 = 295_921_548.0;
        pub const BW_CELLS: f64 = 794_735_626.0 + 58_721_264.0;
    }

    /// **The assertion epic #1116 exists for.** A whole Bundesland — Baden-Württemberg, 215 cells,
    /// 296 MB of navigation graph — was refused outright by the old model (3.60 GB projected against
    /// a 3 GiB budget) and is now assemblable in a tab.
    ///
    /// The headroom is honest rather than comfortable: ~123 MB, under 4 % of the budget, which is
    /// inside the builder's own 15 % caution band — so the UI presents it as "it will probably
    /// assemble", not as a green light. That is the true state of things and the reason the epic has
    /// a phase B: see [`bayern_scale_still_refuses_because_input_and_output_now_dominate`].
    #[test]
    fn baden_wuerttemberg_fits_now_and_did_not_before() {
        let e = estimate_memory(catalog::BW_NAV, catalog::BW_CELLS);
        assert!(e.fits, "BW must fit — {} B against {} B", e.peak_bytes, e.budget_bytes);
        assert!((e.peak_bytes - 3.098e9).abs() < 1e7, "{}", e.peak_bytes);
        assert!(
            e.headroom_bytes > 100.0 * MB,
            "{} B of headroom is thinner than the derivation allows",
            e.headroom_bytes
        );
        assert!(
            e.headroom_bytes < 0.15 * PRACTICAL_BUDGET,
            "…and thinner than the builder's caution band, which is the honest verdict to show: {} B",
            e.headroom_bytes
        );
        // What the pre-#1116 engine coefficient would have said about the same selection.
        let before = 6.4 * catalog::BW_NAV + 2.0 * catalog::BW_CELLS;
        assert!(before > PRACTICAL_BUDGET, "the old model refused BW at {before} B — that is the thing that changed");
    }

    /// The other measured point, and the shape of an ordinary selection: a Regierungsbezirk is waved
    /// straight through with two thirds of the budget untouched.
    ///
    /// It is also the largest published region that fits a **phone**, and only just — 997 MB against
    /// the builder's 1 GiB mobile judgement.
    #[test]
    fn freiburg_regbez_fits_a_desktop_easily_and_a_phone_barely() {
        let e = estimate_memory(catalog::FREIBURG_NAV, catalog::FREIBURG_CELLS);
        assert!(e.fits);
        assert!((e.peak_bytes - 996.6 * MB).abs() < 5.0 * MB, "{}", e.peak_bytes);
        assert!(e.headroom_bytes > 0.65 * PRACTICAL_BUDGET, "{}", e.headroom_bytes);

        let phone =
            estimate_memory_with_budget(catalog::FREIBURG_NAV, catalog::FREIBURG_CELLS, 1024.0 * 1024.0 * 1024.0);
        assert!(phone.fits, "{} B against a 1 GiB tab", phone.peak_bytes);
        assert!(phone.headroom_bytes < 0.15 * phone.budget_bytes, "…with the caution the builder should show");
    }

    /// **Where the new edge is**, and it is barely past BW: at the catalog's measured density
    /// (nav ≈ 0.347 × cells, terrain included) the budget runs out at about **890 MB of cells**,
    /// against 763 MB before the C-series. A 16 % gain, not a doubling — because the engine is no
    /// longer the expensive part.
    #[test]
    fn the_knife_edge_is_now_just_past_baden_wuerttemberg() {
        let density = catalog::BW_NAV / catalog::BW_CELLS;
        let edge = PRACTICAL_BUDGET / (PEAK_PER_NAV_BYTE * density + 2.0);
        assert!((edge - 887.0 * MB).abs() < 10.0 * MB, "the edge moved to {edge} B");
        assert!(edge > catalog::BW_CELLS, "BW is inside it");
        assert!(edge < 1.1 * catalog::BW_CELLS, "…and only by ~4 %, which is what makes it an edge");

        let at_the_edge = estimate_memory(density * edge, edge);
        assert!(at_the_edge.fits && at_the_edge.headroom_bytes < 1.0 * MB, "{}", at_the_edge.headroom_bytes);
    }

    /// **What phase B is for.** The engine is now the *minority* of the projected peak — the two
    /// resident copies of the selection (the cells `addCell` copied in, the shards waiting to be
    /// taken) are 55 % of it. So the next Bundesland up still refuses, and no further work on
    /// `nav.rs` can change that: only streaming the input and the output out of linear memory can.
    ///
    /// Bayern has no published catalog entry, so this extrapolates from BW at 1.7× — the ratio of
    /// the two Geofabrik extracts, and the *low* end of the plausible range (by area it is 1.97×).
    /// Even there the projection is past wasm32's address space entirely, so the verdict does not
    /// depend on the guess being tight.
    #[test]
    fn bayern_scale_still_refuses_because_input_and_output_now_dominate() {
        let e = estimate_memory(1.7 * catalog::BW_NAV, 1.7 * catalog::BW_CELLS);
        assert!(!e.fits);
        assert!(
            e.peak_bytes > WASM32_ADDRESS_SPACE,
            "{} is past the address space, not merely the budget",
            e.peak_bytes
        );
        assert!(e.headroom_bytes < 0.0, "a negative headroom is what the UI shows");

        // The share that phase B would remove, at BW's own scale.
        let bw = estimate_memory(catalog::BW_NAV, catalog::BW_CELLS);
        let resident_share = (bw.input_bytes + bw.output_bytes) / bw.peak_bytes;
        assert!(
            resident_share > 0.5,
            "input + output are {resident_share} of the peak — the engine is no longer the lever"
        );
    }

    /// DACH stays a volume-set-and-native job, and the helper has to say so long before the
    /// download. From [`cell_size_survey.rs`](../../../host/obc-pack/examples/cell_size_survey.rs)'s
    /// shape — a core of 2.8–3.0 GiB and ~8.5 GB of cells all told — it is an order of magnitude
    /// past the budget, which no coefficient in this file could argue with.
    #[test]
    fn dach_does_not_fit_and_is_refused_before_the_download() {
        let e = estimate_memory(3.0e9, 8.5e9);
        assert!(!e.fits);
        assert!(e.peak_bytes > 7.0 * WASM32_ADDRESS_SPACE, "{} should be far past wasm32 entirely", e.peak_bytes);
        assert!(e.headroom_bytes < 0.0);
    }

    /// A corridor — the common selection, which must be waved straight through.
    #[test]
    fn a_corridor_sized_selection_fits_comfortably() {
        let e = estimate_memory(20.0 * MB, 60.0 * MB);
        assert!(e.fits);
        assert!(e.headroom_bytes > 2.9e9);
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
    /// and a caller that passes only the network total (forgetting the geometry bands) still gets an
    /// estimate that at least counts the nav cells as resident.
    #[test]
    fn degenerate_inputs_clamp() {
        let zero = estimate_memory(-1.0, -1.0);
        assert_eq!(zero.peak_bytes, 0.0);
        assert!(zero.fits);
        let nav_only = estimate_memory(100.0 * MB, 0.0);
        assert_eq!(nav_only.input_bytes, 100.0 * MB, "total_cell_bytes cannot be below the network band's own share");
    }

    /// `fits` is a verdict against a **desktop-shaped judgement**, so a caller that knows better —
    /// a mobile UA, whose per-tab allowance is a fraction of this and whose tabs are evicted rather
    /// than slowed — can lower the number the verdict is measured against. Nothing else moves: the
    /// projection is a property of the selection, not of the device.
    #[test]
    fn a_caller_can_lower_the_budget_the_verdict_is_measured_against() {
        let desktop = estimate_memory(catalog::BW_NAV, catalog::BW_CELLS);
        let phone = estimate_memory_with_budget(catalog::BW_NAV, catalog::BW_CELLS, 1.0 * 1024.0 * 1024.0 * 1024.0);
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
            let e = estimate_memory_with_budget(20.0 * MB, 60.0 * MB, bad);
            assert_eq!(e.budget_bytes, PRACTICAL_BUDGET, "budget {bad} should have fallen back");
            assert!(e.fits);
        }
    }
}
