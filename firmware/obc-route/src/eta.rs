//! Gradient-aware ride time: the "how long will this take?" model behind the route summary's
//! EST TIME and the ride's ETA / TIME TO GO tiles (elevation epic #1068, EL9).
//!
//! **Why not distance ÷ average speed.** On a bike, speed is dominated by grade, so a
//! distance-only estimate is not merely imprecise in the mountains — it is wrong by hours. The
//! model here is physics-lite and deliberately tiny:
//!
//! ```text
//! t = dist / v_flat + ascent × k_climb
//! ```
//!
//! one flat-ground speed and one seconds-per-metre-climbed penalty per bike profile. That is the
//! whole thing: no wind, no rider mass, no power model, no learning from ride history (all
//! explicitly out of scope for this issue — the win over "distance ÷ average speed" is already
//! most of the available accuracy).
//!
//! **Descent credits nothing.** A metre lost never *subtracts* time. Braking, hairpins, traffic and
//! fatigue eat most of what physics would hand back, and the failure modes are not symmetric: an
//! ETA that is 20 minutes late is a rider missing a train, while an ETA that is 20 minutes early is
//! a pleasant surprise. Under-promising is the correct bias. (It also keeps this module honest
//! about its role — time only. Descent shaping in *cost* is banned by the epic: a cost reduction
//! below profile-weighted ground length breaks A\* admissibility, and EL6 owns cost.)
//!
//! **Elevation-free routes degrade, they don't special-case.** A route whose points carry no
//! elevation (today's device-planned routes, until EL7 fills them from terrain) has zero
//! ascent-to-go, so the second term vanishes and the estimate is exactly `dist / v_flat`. There is
//! no "is there elevation?" branch anywhere here — the flat answer *is* the model's answer for a
//! flat input, and imported GPX routes (which already carry per-point elevation) get the full
//! treatment through the identical path.
//!
//! **Where ascent-to-go comes from.** Not from a new sweep: [`Profile::ascent_between_m`] reads the
//! cumulative-ascent curve the elevation profile already builds (the same curve the `TO CLIMB` tile
//! and the Up-ahead rows' climb-to-go read). This module only turns metres into seconds.

use crate::profile::Profile;

// ---------------------------------------------------------------------------------------------
// Tuning knobs — the whole time policy in two tables.
//
// These are the *initial* seeds; like `climb.rs`'s detection gates they are meant to be retuned
// AFTER eyeballing real routes against real ride times, so they are grouped here and trivial to
// change. Plain module consts, not a config struct and not a settings item: there is one device
// policy per bike profile, and a struct would invite per-call variation nothing wants (the #678-era
// "no configuration-about-configuration" rule — the rider already told us the bike type once).
// ---------------------------------------------------------------------------------------------

/// Bike profiles the time model is keyed by — the **shipped** §8.6 profile table
/// (`builder/presets/schema.json`): `0` Road, `1` Gravel, `2` MTB, `3` Touring.
pub const PROFILE_COUNT: usize = 4;

/// Sustainable speed on flat ground, km/h, indexed by bike-profile ([`PROFILE_COUNT`]). This is a
/// *route-average* flat pace over a long day — stops excluded, but rolling terrain, junctions and
/// surface included — not a fresh-legs cruising speed.
pub const V_FLAT_KMH: [f32; PROFILE_COUNT] = [
    22.0, // Road    — tarmac, light bike
    19.0, // Gravel  — mixed surface, some loose
    16.0, // MTB     — trail-biased, rolling resistance
    17.0, // Touring — loaded bikepacking rig
];

/// Time cost of one metre of ascent, **seconds per metre climbed**, indexed by bike-profile. This
/// is the classic "a metre up costs ~8–10 flat metres" rule expressed in time: `k = equiv_m /
/// v_flat`, so the seeds below sit near a 10-metre equivalent on every profile (Road 9.8 m, Gravel
/// 10.0 m, MTB 10.2 m, Touring 10.4 m — a loaded bike pays a little more, a road bike a little
/// less). At the Road seed a 1000 m col adds 27 minutes on top of its own length, i.e. roughly a
/// 1000 m/h VAM on a sustained 8 % climb, which is where a fit, unhurried rider actually lands.
pub const K_CLIMB_S_PER_M: [f32; PROFILE_COUNT] = [
    1.6, // Road
    1.9, // Gravel
    2.3, // MTB
    2.2, // Touring
];

/// The knob row that applies for stored bike-profile index `idx`. Out of range → **entry 0**
/// (Road), mirroring the router's own locked out-of-range rule (obc-route's `ProfileMult::resolve`,
/// routing-v2 N3, and the UI's `NavProfiles::effective`): a stale device setting must never make the
/// clock and the router disagree about which bike they're describing.
#[inline]
const fn row(idx: u8) -> usize {
    if (idx as usize) < PROFILE_COUNT {
        idx as usize
    } else {
        0
    }
}

/// Flat-ground speed in **m/s** for bike-profile `idx` — [`V_FLAT_KMH`] in the unit the model
/// divides by.
#[inline]
pub fn v_flat_mps(idx: u8) -> f32 {
    V_FLAT_KMH[row(idx)] / 3.6
}

/// Seconds per metre climbed for bike-profile `idx` — [`K_CLIMB_S_PER_M`]'s entry.
#[inline]
pub fn k_climb_s_per_m(idx: u8) -> f32 {
    K_CLIMB_S_PER_M[row(idx)]
}

/// The model itself: seconds to cover `dist_m` metres of ground while climbing `ascent_m` metres,
/// on bike-profile `idx`.
///
/// `ascent_m == 0` gives exactly `dist_m / v_flat` — the natural degradation for a route with no
/// elevation, not a special case. Monotonic non-decreasing in **both** arguments, which is what
/// makes [`time_to_go_s`] monotonic as a ride advances.
pub fn ride_time_s(dist_m: u32, ascent_m: u32, idx: u8) -> u32 {
    let t = dist_m as f32 / v_flat_mps(idx) + ascent_m as f32 * k_climb_s_per_m(idx);
    // Saturating cast: `as` on a float already clamps to the integer range in Rust, and a negative
    // is unreachable (both terms are non-negative), so this is just the rounding-down convention
    // the rest of the readouts use.
    t as u32
}

/// Estimated time (s) for a **whole** route: its length and its total ascent through
/// [`ride_time_s`]. The route-summary / route-planning figure.
#[inline]
pub fn route_time_s(total_distance_m: u32, total_ascent_m: u32, idx: u8) -> u32 {
    ride_time_s(total_distance_m, total_ascent_m, idx)
}

/// Estimated time (s) still to ride from `progress_m` to the end of a route of `route_total_m`,
/// given its cached elevation `ele` profile — the TIME TO GO tile, and (added to the wall clock)
/// the ETA tile.
///
/// Remaining ascent comes from the profile's cumulative-ascent curve
/// ([`Profile::ascent_between_m`]) — the same prefix machinery the `TO CLIMB` tile and the Up-ahead
/// rows' climb-to-go already read, not a second integration of the geometry.
///
/// **Monotonic**: both remaining distance and remaining ascent are non-increasing as `progress_m`
/// advances (the ascent curve is monotonic non-decreasing by construction), and [`ride_time_s`] is
/// non-decreasing in both — so the readout can only count down. Past the end it is `0`.
pub fn time_to_go_s(ele: &Profile, route_total_m: u32, progress_m: u32, idx: u8) -> u32 {
    let dist = route_total_m.saturating_sub(progress_m);
    let ascent = ele.ascent_between_m(progress_m, route_total_m, route_total_m);
    ride_time_s(dist, ascent, idx)
}
