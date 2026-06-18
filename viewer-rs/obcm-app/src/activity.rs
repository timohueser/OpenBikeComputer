//! The ride/tracking model — what the device is *doing*.
//!
//! [`Activity`] holds the operating [`Mode`], which route is loaded, the live
//! map-match result (drives the riding views' cursor + off-route readout), and the
//! **actually-ridden** accumulators (distance / moving-time / climb) that fill the
//! Elevation stat grid. Kept separate from [`AppState`](crate::AppState) (the camera)
//! because the mode and the totals outlive any one screen and several screens read them.
//!
//! "Actually-ridden" (chosen with the user): `done`/`climbed` reflect what the rider
//! really did, not the route-relative position — so they keep counting off-route, while
//! `to go`/`to climb` stay route-relative (they have to). Distance comes from the GPS
//! [`Fix`] stream and climb from the **separate** barometric
//! [`AltimeterSource`](crate::AltimeterSource); the two integrate independently, on their
//! own cadences. [`App::tick`](crate::App::tick) feeds both.

use obcm_route::{ground_dist_m, Match};

use crate::hal::Fix;

/// A gap longer than this between fixes (s) is a GPS dropout, not real travel — skip the
/// interval so a reconnect doesn't book a straight-line jump across it.
const MAX_GAP_S: f32 = 10.0;
/// Implied speed above this (m/s ≈ 108 km/h) is a teleport / glitch (manual drag, GPS
/// jump) — skip the interval rather than crediting impossible distance.
const MAX_SPEED_MPS: f32 = 30.0;
/// Below this implied speed (m/s) the rider is stopped; don't count the time toward the
/// moving average, so red lights and rests don't drag Avg. Speed down.
const MOVING_MIN_MPS: f32 = 0.8;
/// Climb dead-band (m): ignore altitude wiggles below this so barometric noise doesn't
/// inflate the climb. Matches the converter's elevation dead-band (`ELE_THRESHOLD_M`), so
/// an actually-ridden climb on-route lands close to the route's precomputed ascent.
const ALT_THRESHOLD_M: f32 = 3.0;

/// The device's operating mode (`docs/ui_framework_brief.md` §"Operating modes").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// No route active — the Home screensaver.
    #[default]
    Idle,
    /// A route is loaded and tracking is running — Map / Elevation.
    Riding,
    /// Tracking paused — the Ride control overlay is up.
    Paused,
}

/// The active ride: the [`Mode`], which route is loaded, the live map-match, and the
/// actually-ridden accumulators. Small and `Copy` — the screens read it by value through
/// [`Ctx`](crate::screen::Ctx) / [`Render`](crate::screen::Render).
#[derive(Debug, Clone, Copy, Default)]
pub struct Activity {
    pub mode: Mode,
    /// Index into the app's route [`Catalog`](crate::route::Catalog) of the loaded
    /// route, or `None` when idle. The summary is read from the catalog; the geometry
    /// is opened separately by the host (only the active route is resident).
    pub active_route: Option<usize>,

    // --- live map-match (from the GPS fix, set by `App::tick`) ---
    /// Total distance of the active route (m), mirrored from its header so the riding
    /// views can compute the progress fraction (and the Statistics `handle` can seed a
    /// scrub from the live position) without re-reading the route. `0` when none loaded.
    pub route_total_m: u32,
    /// Matched distance along the route (m): the riding cursor / progress bar. Frozen
    /// while off-route.
    pub progress_m: u32,
    /// Whether the rider is currently off-route.
    pub off_route: bool,
    /// Live cross-track distance to the route (m) — the "off route · NNN m" readout.
    pub dist_to_route_m: u32,

    // --- actually-ridden accumulators ---
    /// Distance actually pedalled (m) — the `done` stat.
    pub ridden_m: f32,
    /// Moving time (s), accumulated only above [`MOVING_MIN_MPS`] — denominator of Avg.
    pub moving_s: f32,
    /// Climb actually done (m), barometric + dead-banded — the `climbed` stat.
    pub climb_m: f32,

    // --- integration state (private) ---
    /// Previous fix + its host timestamp, to integrate distance/time between ticks.
    last_fix: Option<Fix>,
    last_ms: Option<u32>,
    /// Hysteresis reference altitude for the climb dead-band.
    alt_ref: Option<f32>,
}

impl Activity {
    /// A fresh activity in the given mode, no route loaded and no ride recorded.
    pub fn new(mode: Mode) -> Self {
        Activity { mode, ..Default::default() }
    }

    /// Average speed (km/h) over the moving time, or `None` before any moving time has
    /// accrued (so the Statistics screen can show a placeholder rather than a `NaN`).
    pub fn avg_kmh(&self) -> Option<f32> {
        (self.moving_s > 0.0).then(|| self.ridden_m / self.moving_s * 3.6)
    }

    /// Clear the ride totals + match + integration state (keeps `mode`/`active_route`).
    /// Called when a route is loaded or swapped — tracking starts fresh (spec §6).
    pub(crate) fn reset_ride(&mut self) {
        self.progress_m = 0;
        self.off_route = false;
        self.dist_to_route_m = 0;
        self.ridden_m = 0.0;
        self.moving_s = 0.0;
        self.climb_m = 0.0;
        self.last_fix = None;
        self.last_ms = None;
        self.alt_ref = None;
    }

    /// Store the latest map-match result (cursor + off-route readout).
    pub(crate) fn apply_match(&mut self, m: Match) {
        self.progress_m = m.progress_m;
        self.off_route = m.off_route;
        self.dist_to_route_m = m.dist_m;
    }

    /// Integrate a position fix into the ridden distance + moving time. Only accumulates
    /// while [`Riding`](Mode::Riding); a sane-interval gate drops dropouts and teleports.
    /// Pausing drops the anchor so resuming doesn't book the gap (spec §6).
    pub(crate) fn record_motion(&mut self, fix: Fix, now_ms: u32) {
        if self.mode != Mode::Riding {
            self.last_fix = None;
            self.last_ms = None;
            return;
        }
        if let (Some(prev), Some(prev_ms)) = (self.last_fix, self.last_ms) {
            let dt = now_ms.saturating_sub(prev_ms) as f32 / 1000.0;
            let dist = ground_dist_m((prev.lon, prev.lat), (fix.lon, fix.lat));
            let implied = if dt > 0.0 { dist / dt } else { 0.0 };
            if dt > 0.0 && dt < MAX_GAP_S && implied < MAX_SPEED_MPS {
                self.ridden_m += dist;
                if implied >= MOVING_MIN_MPS {
                    self.moving_s += dt;
                }
            }
        }
        self.last_fix = Some(fix);
        self.last_ms = Some(now_ms);
    }

    /// Integrate one barometric altitude sample into the climbed total, dead-banded so
    /// sensor noise doesn't inflate it. Only while [`Riding`](Mode::Riding); pausing drops
    /// the reference so an altitude change *during* the pause isn't booked on resume.
    pub(crate) fn record_altitude(&mut self, alt_m: f32) {
        if self.mode != Mode::Riding {
            self.alt_ref = None;
            return;
        }
        match self.alt_ref {
            None => self.alt_ref = Some(alt_m),
            Some(r) => {
                let d = alt_m - r;
                if d >= ALT_THRESHOLD_M {
                    self.climb_m += d;
                    self.alt_ref = Some(alt_m);
                } else if d <= -ALT_THRESHOLD_M {
                    self.alt_ref = Some(alt_m);
                }
            }
        }
    }
}
