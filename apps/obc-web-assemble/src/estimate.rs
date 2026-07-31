//! The memory model: **can this selection be assembled in a tab at all?**
//!
//! OBCA §5.7 already makes the catalog consumer refuse a selection whose *files* would exceed the
//! format's `4 GiB − 1 B` per-file ceiling (that is #1028's ledger, and it prices the output). This
//! prices something else and stricter: the **address space the assembly itself needs while it runs**.
//! An assembly can be far below the file ceiling and still be impossible in a browser, because
//! wasm32 has a hard 4 GiB address space and the engine's nav rewrite is the memory-hungry part of
//! the run.
//!
//! Both checks belong *before the download*, which is why this takes catalog byte counts rather than
//! cells: by the time the assembler is holding cells, the gigabytes it was supposed to refuse have
//! already been fetched.
//!
//! # The model, and where its constants come from
//!
//! Everything is measured on PR #1027's benchmark: **switzerland**, 410 cells / 717 MB, assembled
//! natively into one 716 692 620 B file in 19.9 s, **peak RSS 1.73 GB**, of which the rebuilt nav
//! section is **271 MB** (258.4 MiB) and the verbatim geometry copy 440 MB. Three terms:
//!
//! 1. **The engine's working set** — dominated by the nav rewrite (§4.6): the merged node set, its
//!    adjacency, the rebuilt edge pool, the renumbering maps. Measured
//!    `1.73 GB / 271 MB =` [`PEAK_PER_NAV_BYTE`] **≈ 6.4 bytes resident per byte of nav section**.
//!    PR #1027's own DACH projection (nav 11.1–11.9× switzerland's ⇒ peak ≈ 19–21 GB) is exactly
//!    this linear scaling, so the model here is the same one that decided the epic's verdict.
//! 2. **The input cells**, resident for the whole run. The native benchmark does **not** pay this —
//!    its `FileSource` reads cells from disk 256 KB at a time — but the browser does: cells arrive
//!    over the network as `Uint8Array`s and cross into wasm linear memory once.
//! 3. **The output**, also resident for the whole run, and also not paid natively. §4.8 requires
//!    every sealed shard to be **read back through the real reader before the manifest is written**,
//!    so a shard has to be randomly addressable, and a tab has nowhere but linear memory to put it.
//!    Measured 717 MB of cells → 716 692 620 B of output, i.e. [`OUTPUT_PER_CELL_BYTE`] ≈ 1.00 —
//!    principled rather than coincidental: geometry is copied verbatim (§2.3) and the nav section is
//!    rewritten to about the size the cells' own nav sections had.
//!
//!    The coefficient is **1.00 and not 1.5** because the driver's store allocates each shard buffer
//!    once, at its planned size (`HookedStore::begin` — §5 computes a shard's bytes before the write
//!    starts). A `Vec` grown by doubling would instead hold the old and the new allocation together
//!    for the copy, so the last doubling on a 717 MB shard would transiently want ~1.59 GB and a
//!    gigabyte-scale *contiguous* block. That is memory this model does not count and a tab may not
//!    be able to give, and on switzerland it is more than the whole 1.55 % headroom below.
//!
//! ```text
//! peak ≈ 6.4 × nav_bytes  +  cell_bytes  +  1.0 × cell_bytes
//! ```
//!
//! `nav_bytes` is the **`network` band's** share of the selection, because a `network` cell carries
//! the nav graph and the POIs and no geometry at all (`obc-pack`'s cutter, OBCA §1.2/§3.1) — so the
//! catalog's per-band byte totals already state it, with no extra field to publish.
//!
//! Where that lands, for scale: **the whole of switzerland as one map projects to 3.17 GB**, which
//! passes the budget below with under 2 % to spare, and DACH — PR #1027's own 11.1–11.9× nav
//! scaling — projects past wasm32's address space entirely. A country is the knife edge; a corridor
//! or a Bundesland is nowhere near it.
//!
//! # What the model is not
//!
//! It is a **linear extrapolation from one measured point**, and it is deliberately summed rather
//! than max'd — the three terms genuinely coexist (the cells are still being read from during the
//! write, and the merged nav graph is still alive while shards are written), but the nav coefficient
//! was measured on a run whose peak already included some of the same allocator slack. Treat a
//! verdict near the budget as "probably not", not as a number. What it is *for* is refusing the
//! selections that are obviously impossible before a rider spends ten minutes downloading them.

/// Peak resident bytes per byte of rebuilt nav section: `1.73 GB / 271 MB`, measured on the
/// switzerland run in PR #1027.
pub const PEAK_PER_NAV_BYTE: f64 = 6.4;

/// Output bytes per byte of input cell: `716 692 620 B / 717 MB`, same run.
pub const OUTPUT_PER_CELL_BYTE: f64 = 1.0;

/// wasm32's hard address space. Nothing can be allocated past this, whatever the machine has.
pub const WASM32_ADDRESS_SPACE: f64 = 4.0 * 1024.0 * 1024.0 * 1024.0;

/// The budget this crate reports `fits` against: **3 GiB**, 75 % of the address space.
///
/// Not a measurement — a judgement, and the reason is that the failure mode is unforgiving. A wasm
/// allocation that cannot be served aborts the module: there is no `Err` to render, the tab has
/// already spent the whole download and the whole nav rewrite, and the rider sees a crash. Browsers
/// also do not reliably grant the full 4 GiB (the limit is per-tab and platform-dependent), and the
/// model above is a one-point extrapolation. A quarter of the space is the margin those three facts
/// together are worth.
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
    /// [`PRACTICAL_BUDGET`].
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
/// * `total_cell_bytes` — every selected cell of every band, which is also what the download costs.
///
/// See the module header for the model and its measured constants. Both arguments are `f64` because
/// they cross from JS, where a byte count past 2^53 is not representable anyway — and 9 PB of cells
/// is not the case this function is for.
pub fn estimate_memory(network_band_bytes: f64, total_cell_bytes: f64) -> MemoryEstimate {
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
        budget_bytes: PRACTICAL_BUDGET,
        ceiling_bytes: WASM32_ADDRESS_SPACE,
        fits: peak_bytes <= PRACTICAL_BUDGET,
        headroom_bytes: PRACTICAL_BUDGET - peak_bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: f64 = 1_000_000.0;

    /// The measured point the model is calibrated on, and the knife edge documented in the module
    /// header: **all of switzerland as one map is the largest thing a tab can assemble**, and only
    /// just. 3.17 GB against a 3 GiB budget passes with under 2 % to spare.
    #[test]
    fn switzerland_is_the_knife_edge() {
        let e = estimate_memory(271.0 * MB, 717.0 * MB);
        // 6.4 × 271 MB = 1.73 GB — the measured native peak, reproduced by the coefficient.
        assert!((e.engine_bytes - 1.734e9).abs() < 1e7, "{} is not the measured 1.73 GB peak", e.engine_bytes);
        assert!((e.peak_bytes - 3.17e9).abs() < 2e7, "{}", e.peak_bytes);
        assert!(e.fits, "switzerland fits — it is the biggest thing that does");
        assert!(
            e.headroom_bytes < 0.05 * PRACTICAL_BUDGET,
            "…with {} B of headroom, which is what makes it the edge rather than a comfortable yes",
            e.headroom_bytes
        );
    }

    /// DACH, at PR #1027's own projection (nav 11.1–11.9× switzerland's). The epic's verdict is that
    /// this does not fit in a tab, and the helper has to say so long before the download.
    #[test]
    fn dach_does_not_fit_and_is_refused_before_the_download() {
        let e = estimate_memory(11.5 * 271.0 * MB, 11.5 * 717.0 * MB);
        assert!(!e.fits);
        assert!(e.peak_bytes > WASM32_ADDRESS_SPACE, "{} should be past wasm32 entirely", e.peak_bytes);
        assert!(e.headroom_bytes < 0.0, "a negative headroom is what the UI shows");
    }

    /// A corridor or a Bundesland — the common selection, which PR #1027 measured as "a fraction of
    /// switzerland's 20 seconds" and which must be waved straight through.
    #[test]
    fn a_corridor_sized_selection_fits_comfortably() {
        let e = estimate_memory(20.0 * MB, 60.0 * MB);
        assert!(e.fits);
        assert!(e.headroom_bytes > 2.9e9);
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
}
