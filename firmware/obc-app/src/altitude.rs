//! The **map-referenced altimeter** (elevation epic #1068, EL8): the barometer's short-term
//! dynamics pinned to the map's absolute frame.
//!
//! ## Why fusion, not replacement
//!
//! The two sources this device already has are exact opposites, and each is the other's
//! calibration:
//!
//! | | absolute? | short-term | fails on |
//! | :-- | :-- | :-- | :-- |
//! | BMP581 (`obc-sensors/bmp581.rs`) | **no** — hard-coded sea-level `P0`, so the value is offset by whatever the weather is doing | resolves single metres of real climbing, at the fix rate | weather drift (metres per hour), and any absolute reading at all |
//! | OBCT terrain at the GPS fix | **yes** — orthometric metres, weather-immune | none: it is a static raster | ~57 m posting quantisation, bridges, cuttings, tunnels, cliff-edge postings |
//!
//! So: take the **difference** of the two at every fix that resolves a terrain sample —
//! `residual = map − baro` — and low-pass it. That residual *is* the barometer's unknown offset,
//! and it moves only as fast as the weather does. Add it back and the tile finally reads a
//! trustworthy absolute height: `fused = baro + offset`. The barometer keeps supplying every
//! short-term metre; the map only ever moves the frame those metres are measured in.
//!
//! ## Ride recording is deliberately NOT fused
//!
//! [`Activity::track_ele`](crate::activity::Activity::track_ele) (the logged `TrackPoint`
//! elevation) and the climb accumulator keep reading **raw dead-banded barometric** deltas, exactly
//! as before this module existed. That is not an oversight:
//!
//! - Climb is a sum of *differences*, and the offset cancels in a difference. Fusing would change
//!   nothing about it except to inject the estimator's own settling transient as fake climbing.
//! - The recorded track is the rider's own measurement. Folding the map into it would double-count:
//!   a ride ridden on terrain the map already describes would come back carrying the map's numbers
//!   dressed as the barometer's, and the two would no longer be independent when compared.
//!
//! The one thing this module changes is what the **Current Elevation tile** (#222) shows — a
//! read-out, not a record.
//!
//! ## No fake precision
//!
//! Until the estimator has [`SETTLE_SAMPLES`] accepted residuals — and forever on a map with no
//! terrain beside it, where no residual ever arrives — [`fused_m`](AltitudeFusion::fused_m) answers
//! `None` and the tile falls back to exactly today's baro-relative reading and presentation.
//!
//! ## #529 groundwork
//!
//! [`reference_pressure_hpa`](AltitudeFusion::reference_pressure_hpa) inverts the barometer's own
//! standard-atmosphere curve and re-reduces the measured pressure to sea level using the *fused*
//! altitude — the "pressure at a fixed reference" a storm heuristic needs. On a bike the raw
//! pressure trend is dominated by the hill you are riding; this one is not. The alert itself stays
//! parked (#529): this module only makes the signal exist.

use libm::powf;

// ---------------------------------------------------------------------------------------------
// Tuning knobs — the whole estimator policy in five consts, `climb.rs`-style: plain module consts,
// one device policy, easy to retune from a sim replay. Every one of them is expressed **per
// accepted residual**, i.e. per GPS fix that resolved a terrain sample (the app samples terrain at
// the fix cadence, never per frame — see `App::sample_terrain`).
// ---------------------------------------------------------------------------------------------

/// The steady-state EMA weight of one residual, i.e. `α` in `offset += α·(residual − offset)`.
///
/// `1/300` ⇒ a time constant of ~300 fixes ≈ **5 minutes** at the default 1 Hz
/// [`fix_interval_s`](crate::settings::Settings::fix_interval_s). Chosen against the two error
/// budgets it sits between:
///
/// - **Above** the map's noise: a single bilinear terrain sample is only good to a handful of
///   metres (57 m posting, and the fix itself wanders), so a *lot* of averaging is wanted. 300
///   samples cuts that noise by ~17×.
/// - **Below** the weather's rate: sea-level pressure moves ~1 hPa/h in weather worth noticing,
///   ≈ 8 m/h of apparent altitude. A 5-minute lag against 8 m/h is ~0.7 m — under the tile's own
///   1 m rounding, so tracking is effectively free.
///
/// A rider who configured a slower fix interval stretches τ proportionally (10 s fixes ⇒ ~50 min);
/// the lag against weather grows to ~7 m, still small, and the warm-up below is unaffected.
pub const OFFSET_ALPHA: f32 = 1.0 / 300.0;

/// Accepted residuals before the estimator is **settled** — before this the tile shows the raw
/// barometric reading, after it the fused one.
///
/// 20 fixes ≈ 20 s of riding under open sky. The first residual already *seeds* the offset (so the
/// frame is roughly right immediately), and the warm-up rule below makes those first 20 a plain
/// running mean; 20 samples is where the mean's own spread drops below the tile's 1 m rounding for
/// a typical few-metre per-sample error. Waiting longer would only leave the rider staring at the
/// old, wrong number for no gain.
pub const SETTLE_SAMPLES: u32 = 20;

/// How far (m) a residual may sit from the current offset and still be averaged in. Beyond this it
/// is **gated** — recorded, but never blended.
///
/// 40 m is comfortably above everything that is *noise* (posting quantisation on a steep face, fix
/// wander, baro sample scatter) and comfortably below everything that is *geometry*: a bridge over
/// a gorge, a cutting, and above all a tunnel, where the map faithfully reports the mountain
/// hundreds of metres over your head. Those excursions are real facts about the raster and wrong
/// facts about the rider, so the filter must follow the trend and ignore them — during a tunnel
/// the barometer carries the elevation alone, which is precisely the right answer.
pub const OUTLIER_GATE_M: f32 = 40.0;

/// Consecutive **mutually consistent** gated residuals before the estimator concludes the reference
/// genuinely moved and re-seeds on them.
///
/// The escape hatch against a permanently stuck filter: if the offset is wrong for any reason the
/// gate cannot distinguish from geometry — the device was carried up in a lift, the barometer
/// re-anchored, a long tunnel spat the rider out somewhere the old frame no longer fits — every
/// residual becomes an outlier and without this the filter would never recover.
///
/// 60 fixes ≈ 1 minute. Combined with [`RESEED_SPREAD_M`] this is deliberately hard to trip by
/// accident: passing *under* terrain produces gated residuals that scatter as the ground overhead
/// rises and falls, which resets the run. If a flat-topped tunnel does fool it, the failure
/// self-heals — on daylight the true residual is itself a consistent run, and the estimator
/// re-seeds back within another minute.
pub const RESEED_RUN: u16 = 60;

/// How tightly a run of gated residuals must agree (m) to count as "the reference moved" rather
/// than "we are traversing varied terrain we are not standing on". 12 m is a few times the
/// per-sample noise and far below the tens-to-hundreds of metres that terrain overhead sweeps
/// through over a minute of riding.
pub const RESEED_SPREAD_M: f32 = 12.0;

// ---------------------------------------------------------------------------------------------
// The standard-atmosphere constants. These MUST mirror `obc_sensors::bmp581::pa_to_m`
// (`h = 44330·(1 − (P/P0)^0.190284)`), which is the curve every `AltimeterSource` reading on the
// device came through; `reference_pressure_hpa` inverts exactly it. `obc-app` deliberately does not
// depend on the board's sensor crate (it is a driver, not app vocabulary), so the pin is this
// comment plus `pressure_round_trips_the_sensor_curve` below.
// ---------------------------------------------------------------------------------------------

/// The sea-level pressure the barometer's altitude curve is anchored on, in hPa
/// (`obc_sensors::bmp581::P0_PA` = 101 325 Pa). A fused ride whose offset is zero reports exactly
/// this as its reference pressure.
pub const P0_HPA: f32 = 1013.25;
/// The scale height (m) in the standard-atmosphere altitude formula.
const HYPSO_K: f32 = 44_330.0;
/// The exponent that turns an altitude ratio back into a pressure ratio — the reciprocal of the
/// `0.190284` the sensor crate applies in the forward direction.
const HYPSO_EXP: f32 = 1.0 / 0.190_284;

/// What [`AltitudeFusion::observe`] did with one residual — the estimator's whole decision surface,
/// returned so tests (and the RTT hook) can see it rather than infer it.
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
/// Lives on [`Activity`](crate::activity::Activity) beside the raw barometric reading it corrects,
/// and — unlike every accumulator around it — **survives a ride reset**: it is a calibration of the
/// atmosphere, not a tally of the ride, and starting a new ride does not change the weather.
#[derive(Debug, Clone, Copy, Default)]
pub struct AltitudeFusion {
    /// The current estimate of `map − baro` (m). `None` until the first residual seeds it.
    offset_m: Option<f32>,
    /// Residuals folded into the offset since the last seed — the EMA's warm-up weight (see
    /// [`OFFSET_ALPHA`]). Reset to 1 by a seed, so a re-seed starts averaging afresh instead of
    /// dragging the old frame along at `α = 1/300`.
    weight: u32,
    /// Residuals accepted (or seeded) **since boot** — the settle counter. Never reset: a re-seed
    /// replaces the absolute frame with fresher map truth, it does not take the frame away, so the
    /// tile must not flick back to the raw reading for 20 s every time one happens.
    accepted: u32,
    /// Residuals gated since boot — diagnostics only (the RTT line, the sim readout).
    gated: u32,
    /// The current consecutive-outlier run: its running-mean residual and its length. `len == 0`
    /// means no run is open.
    run_offset_m: f32,
    run_len: u16,
    /// Re-seeds since boot — diagnostics. A healthy ride has zero or one (the initial seed is not
    /// counted here).
    reseeds: u16,
    /// The terrain height (m) of the most recent accepted sample — what the offset is referenced
    /// to, surfaced for the RTT/sim readout so a suspicious offset can be traced to its sample.
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
    /// from the same tick. Non-finite inputs (a baro driver hiccup) are dropped without touching
    /// any state — the same rule
    /// [`record_altitude`](crate::activity::Activity::record_altitude) applies.
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
            // Inside the gate: end any open outlier run and blend. The warm-up `1/weight` makes the
            // first `1/OFFSET_ALPHA` residuals a plain running mean — optimal early convergence —
            // and hands over to the fixed α smoothly the moment the running mean is the slower of
            // the two.
            self.run_len = 0;
            self.weight = self.weight.saturating_add(1);
            self.accepted = self.accepted.saturating_add(1);
            let alpha = OFFSET_ALPHA.max(1.0 / self.weight as f32);
            self.offset_m = Some(offset + alpha * (residual - offset));
            self.map_ref_m = Some(map_m);
            return Observed::Accepted;
        }
        // Outside the gate. Extend the open run if this residual agrees with it, else start a new
        // one — a scattering sequence (terrain sweeping overhead in a tunnel) can never accumulate.
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

    /// Whether the estimator has enough accepted residuals ([`SETTLE_SAMPLES`]) for its answer to
    /// be shown. Latching: once settled it stays settled for the boot, so riding out of terrain
    /// coverage freezes the offset rather than withdrawing the absolute frame.
    pub fn settled(&self) -> bool {
        self.accepted >= SETTLE_SAMPLES
    }

    /// The current offset estimate `map − baro` (m), or `None` before the first residual. Available
    /// before [`settled`](Self::settled) — the caller decides whether an unsettled offset is worth
    /// anything (the tile says no).
    pub fn offset_m(&self) -> Option<f32> {
        self.offset_m
    }

    /// The fused **absolute** elevation (m) for a barometric reading, or `None` while unsettled /
    /// on a terrain-less map. This is the whole point of the module.
    pub fn fused_m(&self, baro_rel_m: f32) -> Option<f32> {
        let offset = self.offset_m?;
        self.settled().then_some(baro_rel_m + offset)
    }

    /// **#529 groundwork.** The measured pressure re-reduced to sea level using the fused altitude,
    /// in hPa — the barometric trend with the ride's own climbing subtracted out. `None` while
    /// unsettled (an uncorrected trend is worse than no trend) or if the arithmetic leaves the
    /// standard atmosphere's domain.
    ///
    /// The measured pressure is recovered by inverting the sensor's own curve, so no pressure
    /// plumbing is needed: `p = P0·(1 − baro/K)^E`, then `p_ref = p / (1 − fused/K)^E`. With a zero
    /// offset this is exactly [`P0_HPA`]; near sea level the scale is ~8.2 m of offset per hPa, so
    /// the classic "≥ 4 hPa in 3 h" storm heuristic is "the offset fell ≥ ~33 m in 3 h".
    pub fn reference_pressure_hpa(&self, baro_rel_m: f32) -> Option<f32> {
        let fused = self.fused_m(baro_rel_m)?;
        let (measured, reference) = (1.0 - baro_rel_m / HYPSO_K, 1.0 - fused / HYPSO_K);
        (measured > 0.0 && reference > 0.0).then(|| P0_HPA * powf(measured / reference, HYPSO_EXP))
    }

    /// Residuals accepted since boot — the settle counter (RTT / sim readout).
    pub fn accepted(&self) -> u32 {
        self.accepted
    }

    /// Residuals gated since boot (RTT / sim readout).
    pub fn gated(&self) -> u32 {
        self.gated
    }

    /// Re-seeds since boot, excluding the initial seed (RTT / sim readout).
    pub fn reseeds(&self) -> u16 {
        self.reseeds
    }

    /// The terrain height (m) the offset is currently referenced to (RTT / sim readout).
    pub fn map_reference_m(&self) -> Option<f32> {
        self.map_ref_m
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive `n` fixes at a constant true elevation with the barometer offset by `baro_bias`
    /// (i.e. the barometer reads `true + baro_bias`, so the residual is `−baro_bias`).
    fn run_flat(f: &mut AltitudeFusion, n: u32, true_m: f32, baro_bias: f32) {
        for _ in 0..n {
            f.observe(true_m, true_m + baro_bias);
        }
    }

    /// The headline: a barometer reading 60 m too high is corrected to the map's frame, and the
    /// fused answer is the *true* elevation, not the map's quantised sample.
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

    /// The warm-up rule is a running mean, so a noisy map converges in a handful of samples rather
    /// than the 300 the steady-state α alone would need.
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

    /// Before `SETTLE_SAMPLES` there is no fused answer at all — the tile keeps today's raw reading
    /// rather than showing a half-converged number.
    #[test]
    fn an_unsettled_estimator_answers_none() {
        let mut f = AltitudeFusion::new();
        run_flat(&mut f, SETTLE_SAMPLES - 1, 300.0, 10.0);
        assert!(!f.settled());
        assert_eq!(f.fused_m(310.0), None, "unsettled → no fused elevation");
        assert_eq!(f.reference_pressure_hpa(310.0), None, "unsettled → no reference pressure either");
        assert!(f.offset_m().is_some(), "…even though an offset estimate exists");
        f.observe(300.0, 310.0);
        assert!(f.settled(), "the Nth accepted residual settles it");
        assert!(f.fused_m(310.0).is_some());
    }

    /// A map with no terrain beside it feeds nothing, so nothing settles and nothing is claimed —
    /// the `NullElevation` behaviour end of the seam, at this layer.
    #[test]
    fn with_no_terrain_samples_nothing_is_claimed() {
        let f = AltitudeFusion::new();
        assert!(!f.settled());
        assert_eq!(f.offset_m(), None);
        assert_eq!(f.fused_m(742.0), None);
        assert_eq!(f.reference_pressure_hpa(742.0), None);
        assert_eq!(f.accepted(), 0);
    }

    /// The gate: a bridge over a gorge (the map reports the river 80 m below) must not drag the
    /// offset down, and the ride continues on the pre-bridge frame.
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

    /// A tunnel: gated residuals that **scatter** as the mountain overhead rises and falls never
    /// accumulate a run, so a long tunnel cannot re-seed the filter onto the ridge above it.
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

    /// …but a reference that genuinely moved — a consistent run of outliers — re-seeds, so the
    /// filter can never wedge permanently outside its own gate.
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

    /// A run broken by one disagreeing residual restarts — the run counter is *consecutive and
    /// consistent*, not a tally.
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

    /// Weather drift is exactly what the filter is *supposed* to follow: a barometer walking away
    /// at a realistic 8 m/h is tracked with sub-metre lag.
    #[test]
    fn slow_weather_drift_is_tracked_not_gated() {
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

    /// The #529 signal: with the offset at zero the reference pressure is the anchor pressure, and
    /// it moves ~1 hPa per ~8.2 m of offset — the sign a storm heuristic keys on (pressure falling
    /// ⇒ the barometer over-reads ⇒ the offset goes negative).
    #[test]
    fn the_reference_pressure_reads_the_weather_not_the_hill() {
        let mut f = AltitudeFusion::new();
        run_flat(&mut f, 60, 500.0, 0.0);
        let flat = f.reference_pressure_hpa(500.0).unwrap();
        assert!((flat - P0_HPA).abs() < 0.05, "a zero offset is the anchor pressure, got {flat}");
        // The same estimator 1000 m higher up the pass still reports the same weather.
        let uphill = f.reference_pressure_hpa(1500.0).unwrap();
        assert!((uphill - flat).abs() < 0.05, "climbing 1000 m must not move the reference pressure");

        // Now the weather turns: the barometer drifts up 42 m over the ride.
        let mut g = AltitudeFusion::new();
        run_flat(&mut g, 600, 500.0, 42.0);
        let stormy = g.reference_pressure_hpa(542.0).unwrap();
        assert!(stormy < flat, "a falling pressure reads lower than the anchor");
        assert!((flat - stormy - 5.1).abs() < 0.3, "≈ 8.2 m per hPa, got Δ{}", flat - stormy);
    }

    /// `reference_pressure_hpa` inverts `obc_sensors::bmp581::pa_to_m` exactly — pinned here
    /// because `obc-app` does not (and should not) depend on the driver crate. The forward curve is
    /// restated literally; if the sensor's ever changes, this fails.
    #[test]
    fn pressure_round_trips_the_sensor_curve() {
        // The driver's own conversion, verbatim: h = 44330·(1 − (P/P0)^0.190284).
        let pa_to_m = |pa: f32| 44_330.0 * (1.0 - powf(pa / 101_325.0, 0.190_284));
        let mut f = AltitudeFusion::new();
        // A rider at true 0 m under a genuine 980 hPa sky: the sensor reports the altitude its
        // fixed P0 implies, and the map says 0.
        let baro = pa_to_m(98_000.0);
        run_flat(&mut f, 60, 0.0, baro);
        let recovered = f.reference_pressure_hpa(baro).unwrap();
        assert!((recovered - 980.0).abs() < 0.05, "recovered the real sea-level pressure, got {recovered}");
    }

    /// A non-finite reading (a baro driver hiccup) is dropped whole — it must not poison the offset
    /// the way an infinity would poison a running sum.
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
