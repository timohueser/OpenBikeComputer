//! Gradient-aware ride time: the model behind the route summary's EST TIME and the ride's ETA and
//! TIME TO GO tiles.
//!
//! Speed on a bike is dominated by grade, so a distance-only estimate is wrong by hours in the
//! mountains. The model is deliberately tiny:
//!
//! ```text
//! t = dist / v_flat + ascent * k_climb
//! ```
//!
//! one flat-ground speed and one seconds-per-meter-climbed penalty per bike profile. No wind, no
//! rider mass, no power model, no learning from ride history.
//!
//! A meter lost never subtracts time. Braking, hairpins, traffic and fatigue eat most of what
//! physics would hand back, and the failure modes are not symmetric: a late ETA misses a train, an
//! early one is a pleasant surprise.
//!
//! A route with no elevation has zero ascent-to-go, so the second term vanishes and the estimate
//! is exactly `dist / v_flat`. There is no "is there elevation?" branch: the flat answer is the
//! model's answer for a flat input.
//!
//! Ascent-to-go comes from [`Profile::ascent_between_m`], the curve the elevation profile already
//! builds. This module only turns meters into seconds.

use crate::profile::Profile;

// The two tables below are the whole time policy. They stay plain module consts, not a config
// struct and not a settings item: there is one device policy per bike profile, and the rider has
// already said which bike this is.

/// Bike profiles the time model is keyed by, matching the shipped profile table
/// (`builder/presets/schema.json`): `0` Road, `1` Gravel, `2` MTB, `3` Touring.
pub const PROFILE_COUNT: usize = 4;

/// Sustainable speed on flat ground, km/h, indexed by bike profile. This is a route-average pace
/// over a long day, stops excluded but rolling terrain, junctions and surface included, not a
/// fresh-legs cruising speed.
pub const V_FLAT_KMH: [f32; PROFILE_COUNT] = [
    22.0, // Road
    19.0, // Gravel
    16.0, // MTB
    17.0, // Touring
];

/// Time cost of one meter of ascent, seconds per meter climbed, indexed by bike profile. It is
/// the "a meter up costs about ten flat meters" rule expressed as `k = equiv_m / v_flat`, so each
/// value sits near a ten-meter equivalent. At the Road value a 1000 m col adds 27 minutes on top
/// of its own length.
pub const K_CLIMB_S_PER_M: [f32; PROFILE_COUNT] = [
    1.6, // Road
    1.9, // Gravel
    2.3, // MTB
    2.2, // Touring
];

/// The row that applies for stored bike-profile index `idx`. Out of range falls back to entry 0,
/// the same rule the router uses, so a stale setting cannot make the clock and the router disagree
/// about which bike they describe.
#[inline]
const fn row(idx: u8) -> usize {
    if (idx as usize) < PROFILE_COUNT {
        idx as usize
    } else {
        0
    }
}

/// Flat-ground speed in m/s: [`V_FLAT_KMH`] in the unit the model divides by.
#[inline]
pub fn v_flat_mps(idx: u8) -> f32 {
    V_FLAT_KMH[row(idx)] / 3.6
}

#[inline]
pub(crate) fn k_climb_s_per_m(idx: u8) -> f32 {
    K_CLIMB_S_PER_M[row(idx)]
}

/// Seconds to cover `dist_m` meters of ground while climbing `ascent_m` meters.
///
/// It is non-decreasing in both arguments, which is what makes [`time_to_go_s`] count down as a
/// ride advances.
pub fn ride_time_s(dist_m: u32, ascent_m: u32, idx: u8) -> u32 {
    let t = dist_m as f32 / v_flat_mps(idx) + ascent_m as f32 * k_climb_s_per_m(idx);
    // Both terms are non-negative, so this cast is only the rounding-down convention the rest of
    // the readouts use.
    t as u32
}

/// Estimated time (s) for a whole route: the route-summary figure.
#[inline]
pub fn route_time_s(total_distance_m: u32, total_ascent_m: u32, idx: u8) -> u32 {
    ride_time_s(total_distance_m, total_ascent_m, idx)
}

/// Estimated time (s) still to ride from `progress_m` to the end: the TIME TO GO tile, and with
/// the wall clock added, the ETA tile.
///
/// Remaining ascent comes from [`Profile::ascent_between_m`], not from a second integration of the
/// geometry. Both remaining distance and remaining ascent fall as `progress_m` advances, so the
/// readout can only count down. Past the end it is `0`.
pub fn time_to_go_s(ele: &Profile, route_total_m: u32, progress_m: u32, idx: u8) -> u32 {
    let dist = route_total_m.saturating_sub(progress_m);
    let ascent = ele.ascent_between_m(progress_m, route_total_m, route_total_m);
    ride_time_s(dist, ascent, idx)
}
