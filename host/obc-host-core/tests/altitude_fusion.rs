//! The map-referenced altimeter on a **real replay** (elevation epic #1068, EL8 / #1076).
//!
//! The unit tests in `obc-app`'s `altitude.rs` pin the estimator's arithmetic against synthetic
//! residuals. This file pins the thing those cannot: that the whole chain — GPX fixes → `App::tick`
//! → `App::sample_terrain` → the mounted `grimsel.obcd` raster → the Elevation tile's number —
//! actually cancels the one error the device's barometer really has.
//!
//! The experiment is a **paired replay**: the same 35 minutes of the Grimsel climb, ridden twice,
//! differing only in a synthetic barometric weather drift injected through
//! [`BaroSensor::set_drift`]. The control ride has none. Then:
//!
//! - the **raw** barometric reading must diverge between the two by exactly the injected drift —
//!   that is what a weather front does to `bmp581.rs`'s fixed-`P0` altitude, and why the Elevation
//!   tile could not be trusted before this epic;
//! - the **shown** elevation must not, because the terrain under each fix keeps re-pinning it.
//!
//! Fixes are stepped at 1 Hz, the device's default `fix_interval_s` — the estimator's τ is
//! expressed per fix, so a coarser replay step would stretch it and understate the correction.

use obc_app::{App, AppState};
use obc_elevation::{TerrainElevation, DEFAULT_TILE_SLOTS};
use obc_formats::io::SliceSource;
use obc_host_core::{replay_step, ReplaySensors};
use obc_replay::{gpx::Track, BaroSensor, GpxPlayer};

const MAP: &[u8] = include_bytes!("../../../apps/obc-sim/assets/grimsel.obcm");
const TERRAIN: &[u8] = include_bytes!("../../../apps/obc-sim/assets/grimsel.obcd");
const GPX: &str = include_str!("../../../apps/obc-sim/assets/grimsel-climb.gpx");

/// The device's default GPS cadence — the rate [`obc_app::altitude::OFFSET_ALPHA`] is stated at.
const FIX_DT_S: f64 = 1.0;

/// One replay's readings at a checkpoint: the raw barometric altitude and what the Elevation tile
/// would show (fused once settled).
#[derive(Clone, Copy, Debug)]
struct Reading {
    t_s: f64,
    raw_m: f32,
    shown_m: f32,
    offset_m: f32,
    accepted: u32,
    gated: u32,
}

/// Ride the Grimsel replay for `until_s` at 1 Hz with `drift_m_per_h` of injected weather, sampling
/// terrain behind every tick exactly as the board's ride loop does. Returns a reading at each
/// checkpoint in `at_s` (ascending).
fn ride(drift_m_per_h: f32, at_s: &[f64]) -> Vec<Reading> {
    let src = SliceSource(MAP);
    let tables = obc_reader::MapTables::parse(&src).expect("the grimsel fixture parses");
    let b = tables.bbox;
    let cam = (((b.min_lon as i64 + b.max_lon as i64) / 2) as i32, ((b.min_lat as i64 + b.max_lat as i64) / 2) as i32);

    let tsrc = SliceSource(TERRAIN);
    let mut terrain = TerrainElevation::<DEFAULT_TILE_SLOTS>::parse(&tsrc).expect("the grimsel terrain sidecar parses");

    let mut app = App::new(AppState::new(cam.0, cam.1, 1.0)); // boots Riding
    let mut player = GpxPlayer::new(Track::parse(GPX).expect("the grimsel climb GPX parses"));
    let mut baro = BaroSensor::new();
    baro.set_drift(drift_m_per_h);
    player.seek(0.0);
    player.play();

    let mut out = Vec::new();
    let mut next = 0usize;
    let mut t = 0.0;
    while next < at_s.len() {
        replay_step(&mut app, &mut player, &mut baro, None, FIX_DT_S, None, None, ReplaySensors::default());
        // The EL8 drain: one terrain read per fresh fix, never per frame.
        app.sample_terrain(&mut terrain);
        t += FIX_DT_S;
        if t >= at_s[next] {
            let a = app.activity.altitude();
            out.push(Reading {
                t_s: t,
                raw_m: app.activity.baro_elevation_m().expect("the replay has fed the altimeter"),
                shown_m: app.activity.current_elevation_m().expect("…so the tile has a number"),
                offset_m: a.offset_m().expect("terrain resolved under the Grimsel replay"),
                accepted: a.accepted(),
                gated: a.gated(),
            });
            next += 1;
        }
    }
    out
}

/// **The evidence.** A drifting barometer walks away from the truth by the full injected amount;
/// the map-referenced reading the tile shows stays put.
///
/// Run with `cargo test -p obc-host-core --test altitude_fusion -- --nocapture` to print the table
/// that goes in the PR.
#[test]
fn injected_weather_drift_walks_the_barometer_away_but_not_the_shown_elevation() {
    // −60 m/h ≈ −7 hPa/h: a front several times harsher than the classic storm threshold, chosen so
    // the divergence is unambiguous rather than realistic.
    const DRIFT_M_PER_H: f32 = -60.0;
    let checkpoints = [300.0, 600.0, 900.0, 1200.0, 1500.0, 1800.0, 2100.0];
    let control = ride(0.0, &checkpoints);
    let drifted = ride(DRIFT_M_PER_H, &checkpoints);

    println!(
        "\n  t      injected   raw Δ     shown Δ    offset(ctl→drift)   samples\n  \
         ---------------------------------------------------------------------"
    );
    for (c, d) in control.iter().zip(&drifted) {
        let injected = DRIFT_M_PER_H * (d.t_s as f32) / 3600.0;
        println!(
            "  {:>5.0}s  {:>7.1} m  {:>7.1} m  {:>7.1} m   {:>+6.1} → {:>+6.1} m   {}ok/{}gated",
            d.t_s,
            injected,
            d.raw_m - c.raw_m,
            d.shown_m - c.shown_m,
            c.offset_m,
            d.offset_m,
            d.accepted,
            d.gated
        );
    }

    for (c, d) in control.iter().zip(&drifted) {
        let injected = DRIFT_M_PER_H * (d.t_s as f32) / 3600.0;
        let raw_delta = d.raw_m - c.raw_m;
        let shown_delta = d.shown_m - c.shown_m;
        // The barometer takes the whole hit, to the metre — that is the disease.
        assert!(
            (raw_delta - injected).abs() < 0.5,
            "at {:.0}s the raw barometer must carry the full {injected:.1} m of drift, carried {raw_delta:.1} m",
            d.t_s
        );
        // The tile does not. Its residual error is the EMA's steady-state lag against a
        // *continuously* drifting reference — `rate × τ` = 60 m/h × 5 min = **5 m**, a constant,
        // whatever the ride's length. (Real weather at ~8 m/h leaves ~0.7 m, under the tile's own
        // 1 m rounding.) That constant-vs-linear split is the whole result.
        assert!(
            shown_delta.abs() < 6.0,
            "at {:.0}s the shown elevation must stay on the map (moved {shown_delta:.1} m of the \
             {injected:.1} m injected)",
            d.t_s
        );
    }

    // The claim stated as the shape of the two curves rather than a single number: the raw error
    // **grows** with the ride, the shown error **plateaus**. Compare the 15-minute checkpoint with
    // the 35-minute one.
    let (mid_c, mid_d) = (control[2], drifted[2]); // 900 s
    let (end_c, end_d) = (*control.last().unwrap(), *drifted.last().unwrap()); // 2100 s
    let (raw_mid, raw_end) = ((mid_d.raw_m - mid_c.raw_m).abs(), (end_d.raw_m - end_c.raw_m).abs());
    let (shown_mid, shown_end) = ((mid_d.shown_m - mid_c.shown_m).abs(), (end_d.shown_m - end_c.shown_m).abs());
    assert!(raw_end > raw_mid * 2.0, "the raw error grows with the ride ({raw_mid:.1} → {raw_end:.1} m)");
    assert!(shown_end < shown_mid + 1.0, "the shown error plateaus ({shown_mid:.1} → {shown_end:.1} m)");
    // …and by the end of the ride the fusion has removed the great majority of the injected drift.
    let injected_end = (DRIFT_M_PER_H * (end_d.t_s as f32) / 3600.0).abs();
    assert!(
        shown_end < 0.2 * injected_end,
        "the fusion removed only {:.0}% of the {injected_end:.0} m injected by the end",
        100.0 * (1.0 - shown_end / injected_end)
    );

    // The gate stayed shut on a genuine ride: this is a real alpine climb through a raster with
    // real quantisation, and it must not be a stream of rejections.
    let last = drifted.last().unwrap();
    assert!(
        last.gated * 20 < last.accepted,
        "the outlier gate rejected {} of {} residuals — far too many for a clean ride",
        last.gated,
        last.accepted
    );
}

/// The other half of the contract: with **no** terrain beside the map the estimator never settles,
/// so the shown elevation is the raw barometric reading, bit for bit — the pre-epic behaviour, and
/// the reason a terrain file stays removable.
#[test]
fn without_terrain_the_shown_elevation_is_the_raw_barometer() {
    let src = SliceSource(MAP);
    let tables = obc_reader::MapTables::parse(&src).expect("map parses");
    let b = tables.bbox;
    let mut app = App::new(AppState::new(
        ((b.min_lon as i64 + b.max_lon as i64) / 2) as i32,
        ((b.min_lat as i64 + b.max_lat as i64) / 2) as i32,
        1.0,
    ));
    let mut null = obc_route::NullElevation;
    let mut player = GpxPlayer::new(Track::parse(GPX).expect("GPX parses"));
    let mut baro = BaroSensor::new();
    baro.set_drift(-60.0);
    player.seek(0.0);
    player.play();
    for _ in 0..600 {
        replay_step(&mut app, &mut player, &mut baro, None, FIX_DT_S, None, None, ReplaySensors::default());
        app.sample_terrain(&mut null);
    }
    assert!(!app.activity.altitude().settled(), "no residual can arrive through the null source");
    assert_eq!(
        app.activity.current_elevation_m(),
        app.activity.baro_elevation_m(),
        "the tile shows exactly what it showed before the epic"
    );
}
