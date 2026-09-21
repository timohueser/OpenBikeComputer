//! The map-referenced altimeter: the barometer's short-term dynamics pinned to the map's absolute
//! frame.
//!
//! The two sources are opposites, and each is the other's calibration. The barometer resolves single
//! metres of climbing at the fix rate but has no absolute frame, because its sea-level reference is
//! hard-coded while the air pressure drifts by metres per hour. The terrain raster at the GPS fix is
//! absolute orthometric metres but static, and its ~57 m posting lies about bridges, cuttings and
//! tunnels.
//!
//! So take the difference at every fix that resolves a terrain sample, `residual = map − baro`, and
//! low-pass it. That residual is the barometer's unknown offset, and it moves only as fast as the
//! air pressure does. Add it back and the reading is absolute: `fused = baro + offset`.
//!
//! Ride recording is not fused. Climb is a sum of differences, so the offset cancels and fusing
//! would only inject the estimator's settling transient as fake climbing; and the recorded track is
//! the rider's own measurement, which must stay independent of the map it is compared against. Only
//! the Current Elevation tile reads the fused value, and it falls back to the raw barometric reading
//! until the estimator has [`SETTLE_SAMPLES`] accepted residuals — which on a map with no terrain
//! beside it never happens.

// Every tuning const below is expressed per accepted residual, which is per GPS fix that resolved a
// terrain sample.

/// The steady-state EMA weight of one residual: `α` in `offset += α·(residual − offset)`.
///
/// `1/300` is a time constant of about 300 fixes, or 5 minutes at 1 Hz. It sits between two error
/// budgets: a single bilinear terrain sample is only good to a handful of metres, so a lot of
/// averaging is wanted; and a ~1 hPa/h pressure change moves apparent altitude by about 8 m/h,
/// against which a 5-minute lag is ~0.7 m, under the tile's own 1 m rounding. A slower fix interval
/// stretches the time constant proportionally.
pub const OFFSET_ALPHA: f32 = 1.0 / 300.0;

/// Accepted residuals before the estimator is settled. Before that the tile shows the raw
/// barometric reading, after it the fused one.
///
/// 20 fixes is about 20 s of riding under open sky. The first residual already seeds the offset and
/// the warm-up rule makes the first 20 a plain running mean, whose own spread drops below the tile's
/// 1 m rounding at about 20 samples.
pub const SETTLE_SAMPLES: u32 = 20;

/// How far (m) a residual may sit from the current offset and still be averaged in. Beyond this it
/// is gated: recorded, but never blended.
///
/// 40 m is above everything that is noise — posting quantisation on a steep face, fix wander, baro
/// scatter — and below everything that is geometry: a bridge over a gorge, a cutting, and above all
/// a tunnel, where the map reports the mountain overhead. In a tunnel the barometer carries the
/// elevation alone, which is the right answer.
pub const OUTLIER_GATE_M: f32 = 40.0;

/// Consecutive mutually consistent gated residuals before the estimator concludes the reference
/// genuinely moved and re-seeds on them.
///
/// It is the escape hatch against a permanently stuck filter: if the offset is wrong for a reason
/// the gate cannot tell from geometry — a lift ride, a barometer re-anchor — every residual becomes
/// an outlier and the filter would never recover. 60 fixes is about a minute, and with
/// [`RESEED_SPREAD_M`] it is hard to trip by accident: passing under terrain produces residuals that
/// scatter as the ground overhead rises and falls, which resets the run.
pub const RESEED_RUN: u16 = 60;

/// How tightly a run of gated residuals must agree (m) to count as "the reference moved" rather
/// than "we are crossing varied terrain we are not standing on". 12 m is a few times the per-sample
/// noise and far below what terrain overhead sweeps through over a minute of riding.
pub const RESEED_SPREAD_M: f32 = 12.0;

/// What [`AltitudeFusion::observe`] did with one residual, returned so tests and the RTT hook can
/// see the decision rather than infer it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observed {
    /// The offset was (re)established from this residual alone: the very first one, or a re-seed
    /// after [`RESEED_RUN`] consistent outliers.
    Seeded,
    /// Inside the gate — blended into the offset.
    Accepted,
    /// Outside the gate — counted, never blended.
    Gated,
}

/// The offset estimator: one slow EMA over `map − baro`, an outlier gate, and the re-seed escape.
///
/// Unlike the accumulators around it, it survives a ride reset: it calibrates the atmosphere, not
/// the ride, and starting a new ride does not change the air pressure.
#[derive(Debug, Clone, Copy, Default)]
pub struct AltitudeFusion {
    /// The current estimate of `map − baro` (m). `None` until the first residual seeds it.
    offset_m: Option<f32>,
    /// Residuals folded into the offset since the last seed — the EMA's warm-up weight. A seed
    /// resets it to 1, so a re-seed starts averaging afresh instead of dragging the old frame
    /// along.
    weight: u32,
    /// Residuals accepted or seeded since boot — the settle counter. Never reset: a re-seed
    /// replaces the absolute frame rather than taking it away, so the tile must not flick back to
    /// the raw reading each time one happens.
    accepted: u32,
    /// Residuals gated since boot — diagnostics only (the RTT line, the sim readout).
    gated: u32,
    /// The current consecutive-outlier run: its running-mean residual and its length. `len == 0`
    /// means no run is open.
    run_offset_m: f32,
    run_len: u16,
    /// Re-seeds since boot — diagnostics. The initial seed is not counted here.
    reseeds: u16,
    /// The terrain height (m) of the most recent accepted sample — what the offset is referenced
    /// to, so a suspicious offset can be traced to its sample.
    map_ref_m: Option<f32>,
}

impl AltitudeFusion {
    /// A fresh, unseeded estimator.
    pub const fn new() -> Self {
        AltitudeFusion {
            offset_m: None,
            weight: 0,
            accepted: 0,
            gated: 0,
            run_offset_m: 0.0,
            run_len: 0,
            reseeds: 0,
            map_ref_m: None,
        }
    }

    /// Feed one paired observation: the terrain height at the GPS fix and the barometric reading
    /// from the same tick. Non-finite inputs are dropped without touching any state.
    pub fn observe(&mut self, map_m: f32, baro_rel_m: f32) -> Observed {
        if !map_m.is_finite() || !baro_rel_m.is_finite() {
            return Observed::Gated;
        }
        let residual = map_m - baro_rel_m;
        let Some(offset) = self.offset_m else {
            self.seed(residual, map_m);
            return Observed::Seeded;
        };
        if (residual - offset).abs() <= OUTLIER_GATE_M {
            // Inside the gate: end any open outlier run and blend. The warm-up `1/weight` makes
            // the first `1/OFFSET_ALPHA` residuals a plain running mean, and hands over to the fixed
            // α the moment the running mean is the slower of the two.
            self.run_len = 0;
            self.weight = self.weight.saturating_add(1);
            self.accepted = self.accepted.saturating_add(1);
            let alpha = OFFSET_ALPHA.max(1.0 / self.weight as f32);
            self.offset_m = Some(offset + alpha * (residual - offset));
            self.map_ref_m = Some(map_m);
            return Observed::Accepted;
        }
        // Outside the gate. Extend the open run if this residual agrees with it, else start a new
        // one, so a scattering sequence can never accumulate.
        self.gated = self.gated.saturating_add(1);
        if self.run_len > 0 && (residual - self.run_offset_m).abs() <= RESEED_SPREAD_M {
            self.run_len += 1;
            self.run_offset_m += (residual - self.run_offset_m) / self.run_len as f32;
        } else {
            self.run_offset_m = residual;
            self.run_len = 1;
        }
        if self.run_len >= RESEED_RUN {
            let moved = self.run_offset_m;
            self.seed(moved, map_m);
            self.reseeds = self.reseeds.saturating_add(1);
            return Observed::Seeded;
        }
        Observed::Gated
    }

    /// Establish the offset from a single residual and restart the EMA weight.
    fn seed(&mut self, residual: f32, map_m: f32) {
        self.offset_m = Some(residual);
        self.weight = 1;
        self.accepted = self.accepted.saturating_add(1);
        self.run_len = 0;
        self.map_ref_m = Some(map_m);
    }

    /// Whether the estimator has enough accepted residuals for its answer to be shown. It latches:
    /// riding out of terrain coverage freezes the offset rather than withdrawing the frame.
    pub fn settled(&self) -> bool {
        self.accepted >= SETTLE_SAMPLES
    }

    /// The current offset estimate `map − baro` (m), or `None` before the first residual. It is
    /// available before [`settled`](Self::settled); the caller decides whether that is worth
    /// anything.
    pub fn offset_m(&self) -> Option<f32> {
        self.offset_m
    }

    /// The fused absolute elevation (m) for a barometric reading, or `None` while unsettled or on
    /// a terrain-less map.
    pub fn fused_m(&self, baro_rel_m: f32) -> Option<f32> {
        let offset = self.offset_m?;
        self.settled().then_some(baro_rel_m + offset)
    }

    /// Residuals accepted since boot — the settle counter.
    pub fn accepted(&self) -> u32 {
        self.accepted
    }

    /// Residuals gated since boot.
    pub fn gated(&self) -> u32 {
        self.gated
    }

    /// Re-seeds since boot, excluding the initial seed.
    pub fn reseeds(&self) -> u16 {
        self.reseeds
    }

    /// The terrain height (m) the offset is currently referenced to.
    pub fn map_reference_m(&self) -> Option<f32> {
        self.map_ref_m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive `n` fixes at a constant true elevation with the barometer reading
    /// `true + baro_bias`, so the residual is `−baro_bias`.
    fn run_flat(f: &mut AltitudeFusion, n: u32, true_m: f32, baro_bias: f32) {
        for _ in 0..n {
            f.observe(true_m, true_m + baro_bias);
        }
    }

    #[test]
    fn the_offset_converges_to_the_barometers_bias() {
        let mut f = AltitudeFusion::new();
        run_flat(&mut f, 60, 1800.0, 60.0);
        assert!(f.settled(), "60 clean residuals is well past the settle threshold");
        let offset = f.offset_m().expect("seeded");
        assert!((offset - -60.0).abs() < 0.5, "offset ≈ −60 m, got {offset}");
        // A barometer that now reads 1860 (still biased) fuses back to the real 1800.
        let fused = f.fused_m(1860.0).expect("settled");
        assert!((fused - 1800.0).abs() < 0.5, "fused ≈ 1800 m, got {fused}");
    }

    /// A noisy map converges in a handful of samples rather than the 300 the steady-state α alone
    /// would need.
    #[test]
    fn the_warm_up_averages_rather_than_crawling() {
        let mut f = AltitudeFusion::new();
        // Alternating ±6 m of map noise around a true −20 m residual.
        for i in 0..20 {
            let noise = if i % 2 == 0 { 6.0 } else { -6.0 };
            f.observe(500.0 + noise, 520.0);
        }
        let offset = f.offset_m().expect("seeded");
        assert!((offset - -20.0).abs() < 1.0, "the running mean cancels the noise, got {offset}");
        assert!(f.settled(), "20 samples is exactly the settle threshold");
    }

    #[test]
    fn an_unsettled_estimator_answers_none() {
        let mut f = AltitudeFusion::new();
        run_flat(&mut f, SETTLE_SAMPLES - 1, 300.0, 10.0);
        assert!(!f.settled());
        assert_eq!(f.fused_m(310.0), None, "unsettled → no fused elevation");
        assert!(f.offset_m().is_some(), "…even though an offset estimate exists");
        f.observe(300.0, 310.0);
        assert!(f.settled(), "the Nth accepted residual settles it");
        assert!(f.fused_m(310.0).is_some());
    }

    #[test]
    fn with_no_terrain_samples_nothing_is_claimed() {
        let f = AltitudeFusion::new();
        assert!(!f.settled());
        assert_eq!(f.offset_m(), None);
        assert_eq!(f.fused_m(742.0), None);
        assert_eq!(f.accepted(), 0);
    }

    #[test]
    fn a_single_large_excursion_is_gated_not_averaged() {
        let mut f = AltitudeFusion::new();
        run_flat(&mut f, 60, 400.0, 10.0);
        let before = f.offset_m().unwrap();
        // Five fixes crossing the bridge: the raster says 320 m, the rider is at 400 m.
        for _ in 0..5 {
            assert_eq!(f.observe(320.0, 410.0), Observed::Gated);
        }
        let after = f.offset_m().unwrap();
        assert_eq!(before, after, "a gated residual leaves the offset bit-for-bit untouched");
        assert_eq!(f.gated(), 5);
        assert_eq!(f.reseeds(), 0, "five is nowhere near the re-seed run");
    }

    /// Gated residuals that scatter as the mountain overhead rises and falls never accumulate a
    /// run, so a long tunnel cannot re-seed the filter onto the ridge above it.
    #[test]
    fn scattered_outliers_never_reach_the_re_seed_run() {
        let mut f = AltitudeFusion::new();
        run_flat(&mut f, 60, 800.0, 0.0);
        // Four minutes of tunnel; the terrain overhead climbs steadily from 900 to 1300 m, so
        // consecutive residuals differ by far more than RESEED_SPREAD_M.
        for i in 0..240 {
            let overhead = 900.0 + i as f32 * (400.0 / 240.0);
            assert_eq!(f.observe(overhead, 800.0), Observed::Gated);
        }
        assert_eq!(f.reseeds(), 0, "a sweeping overhead profile is not a moved reference");
        assert!((f.offset_m().unwrap()).abs() < 0.5, "the offset rode the tunnel out unchanged");
    }

    /// A reference that genuinely moved re-seeds, so the filter can never wedge permanently
    /// outside its own gate.
    #[test]
    fn a_consistent_outlier_run_re_seeds_the_offset() {
        let mut f = AltitudeFusion::new();
        run_flat(&mut f, 60, 800.0, 0.0);
        assert!(f.settled());
        // The barometer's frame jumps 150 m (a re-anchor / a lift ride): every residual is now
        // −150, consistently, fix after fix.
        let mut seeded_at = None;
        for i in 0..RESEED_RUN {
            let outcome = f.observe(800.0, 950.0);
            if outcome == Observed::Seeded {
                seeded_at = Some(i + 1);
            }
        }
        assert_eq!(seeded_at, Some(RESEED_RUN), "re-seeds on exactly the RESEED_RUN'th outlier");
        assert!((f.offset_m().unwrap() - -150.0).abs() < 0.5, "re-seeded onto the new frame");
        assert_eq!(f.reseeds(), 1);
        assert!(f.settled(), "a re-seed refreshes the frame, it does not withdraw it");
        // And it converges again from there without needing another re-seed.
        run_flat(&mut f, 30, 800.0, 150.0);
        assert_eq!(f.reseeds(), 1);
        assert!((f.fused_m(950.0).unwrap() - 800.0).abs() < 1.0);
    }

    /// The run counter is consecutive and consistent, not a tally.
    #[test]
    fn an_inconsistent_sample_restarts_the_run() {
        let mut f = AltitudeFusion::new();
        run_flat(&mut f, 30, 500.0, 0.0);
        for _ in 0..(RESEED_RUN - 1) {
            f.observe(500.0, 650.0); // a consistent −150 run, one short of re-seeding
        }
        assert_eq!(f.reseeds(), 0);
        f.observe(900.0, 650.0); // +250: disagrees with the run AND with the offset
        for _ in 0..(RESEED_RUN - 1) {
            f.observe(500.0, 650.0);
        }
        assert_eq!(f.reseeds(), 0, "the run restarted, so it is one short again");
        f.observe(500.0, 650.0);
        assert_eq!(f.reseeds(), 1, "…and re-seeds on the next one");
    }

    #[test]
    fn slow_pressure_drift_is_tracked_not_gated() {
        let mut f = AltitudeFusion::new();
        run_flat(&mut f, 60, 1000.0, 0.0);
        // 3 hours at 1 Hz, the barometer's apparent altitude creeping up 8 m per hour.
        for i in 0..10_800u32 {
            let bias = 8.0 * i as f32 / 3600.0;
            assert_ne!(f.observe(1000.0, 1000.0 + bias), Observed::Gated, "drift must never trip the gate");
        }
        assert_eq!(f.reseeds(), 0);
        let fused = f.fused_m(1000.0 + 24.0).unwrap();
        assert!((fused - 1000.0).abs() < 1.5, "the fused elevation stayed on the map, got {fused}");
    }

    #[test]
    fn non_finite_readings_are_dropped() {
        let mut f = AltitudeFusion::new();
        run_flat(&mut f, 60, 700.0, 5.0);
        let before = (f.offset_m().unwrap(), f.accepted(), f.gated());
        f.observe(f32::NAN, 705.0);
        f.observe(700.0, f32::INFINITY);
        f.observe(f32::NEG_INFINITY, f32::NAN);
        assert_eq!((f.offset_m().unwrap(), f.accepted()), (before.0, before.1), "offset untouched");
        assert_eq!(f.gated(), before.2, "…and they are not even counted as outliers");
        assert!(f.fused_m(705.0).is_some(), "the estimator is still usable afterwards");
    }
}
