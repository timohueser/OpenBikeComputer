//! Pre-download wasm memory projection.
//!
//! The model comes from PR #1027's Switzerland benchmark (717 MB of cells,
//! 271 MB of nav, 1.73 GB peak RSS):
//!
//! `peak ≈ 6.4 × nav_bytes + input_cell_bytes + output_cell_bytes`
//!
//! Browser input and output remain resident while the nav graph is rebuilt.
//! This is a conservative linear extrapolation intended to reject clearly
//! impossible selections before downloading them, not a precise RSS forecast.

/// Peak resident bytes per byte of rebuilt nav section: `1.73 GB / 271 MB`, measured on the
/// switzerland run in PR #1027.
pub const PEAK_PER_NAV_BYTE: f64 = 6.4;

/// Output bytes per byte of input cell: `716 692 620 B / 717 MB`, same run.
pub const OUTPUT_PER_CELL_BYTE: f64 = 1.0;

/// wasm32's hard address space. Nothing can be allocated past this, whatever the machine has.
pub const WASM32_ADDRESS_SPACE: f64 = 4.0 * 1024.0 * 1024.0 * 1024.0;

/// The **default** budget this crate reports `fits` against: **3 GiB**, 75 % of the address space.
///
/// Not a measurement — a judgement, and the reason is that the failure mode is unforgiving. A wasm
/// allocation that cannot be served aborts the module: there is no `Err` to render, the tab has
/// already spent the whole download and the whole nav rewrite, and the rider sees a crash. Browsers
/// also do not reliably grant the full 4 GiB (the limit is per-tab and platform-dependent), and the
/// model above is a one-point extrapolation. A quarter of the space is the margin those three facts
/// together are worth.
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
/// * `total_cell_bytes` — every selected cell of every band, which is also what the download costs.
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

    /// `fits` is a verdict against a **desktop-shaped judgement**, so a caller that knows better —
    /// a mobile UA, whose per-tab allowance is a fraction of this and whose tabs are evicted rather
    /// than slowed — can lower the number the verdict is measured against. Nothing else moves: the
    /// projection is a property of the selection, not of the device.
    #[test]
    fn a_caller_can_lower_the_budget_the_verdict_is_measured_against() {
        let desktop = estimate_memory(271.0 * MB, 717.0 * MB);
        let phone = estimate_memory_with_budget(271.0 * MB, 717.0 * MB, 1.0 * 1024.0 * 1024.0 * 1024.0);
        assert!(desktop.fits && !phone.fits, "switzerland fits a desktop tab and not a 1 GiB one");
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
