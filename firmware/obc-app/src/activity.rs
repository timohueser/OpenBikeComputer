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

use obc_route::{ground_dist_m, DeadBand, Match};

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

/// A one-shot disposition for the **current** ride log, set by a screen and drained by the
/// host (`take_track_action`) which owns the file I/O. The screens never touch storage —
/// they record intent here, exactly as they record `active_route` for the route reader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackAction {
    /// Finalise the open log to a `.gpx` (Finish, or "Save & start new").
    Save,
    /// Throw the open log away (Discard).
    Discard,
}

/// What [`record_motion`](Activity::record_motion) decided about one fix: whether to **log**
/// it (feed the breadcrumb + ride log) and whether it **starts a new track segment** (the
/// first fix of a session, or the first after a pause / GPS gap → a fresh GPX `<trkseg>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Motion {
    pub log: bool,
    pub segment_start: bool,
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

    // tracking session (distinct from the navigated route)
    /// The active **tracking session** id, or `None` when not tracking. A session spans
    /// from a route load (from Idle, or "Save & start new") to Finish/Discard, and survives
    /// a "Swap route only" — so it's keyed separately from [`active_route`](Activity::active_route)
    /// (which the matcher follows). The host reconciles the open ride log to this id.
    pub session: Option<u32>,
    /// Monotonic id source for [`session`](Activity::session); only ever increments, so a
    /// new session can never collide with a just-finished one.
    session_seq: u32,
    /// A one-shot disposition (`Save`/`Discard`) for the open log, set by a screen and
    /// drained by the host via [`take_track_action`](Activity::take_track_action).
    track_action: Option<TrackAction>,

    // live map-match (from the GPS fix, set by `App::tick`)
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

    // actually-ridden accumulators
    /// Distance actually pedalled (m) — the `done` stat.
    pub ridden_m: f32,
    /// Moving time (s), accumulated only above [`MOVING_MIN_MPS`] — denominator of Avg.
    pub moving_s: f32,

    // integration state (private)
    /// Previous fix + its host timestamp, to integrate distance/time between ticks.
    last_fix: Option<Fix>,
    last_ms: Option<u32>,
    /// Dead-banded barometric climb — the `climbed` stat, read via
    /// [`climb_m`](Activity::climb_m). The same hysteresis integrator (and dead-band) the
    /// route converter uses, so an on-route ride lands near the route's precomputed ascent.
    climb: DeadBand<f32>,
    /// Latest barometric altitude (m), stamped onto each logged [`TrackPoint`]'s elevation.
    last_alt: Option<f32>,
    /// `true` when a dropped fix (GPS gap / teleport) left a hole, so the next logged point
    /// must start a fresh track segment.
    segment_break: bool,
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

    /// Climb actually done (m) — barometric and dead-banded — the `climbed` stat.
    pub fn climb_m(&self) -> f32 {
        self.climb.ascent()
    }

    /// Begin a fresh tracking session (a route load from Idle, or "Save & start new"),
    /// assigning the next monotonic [`session`](Activity::session) id. The host opens a new
    /// ride log when it sees the id change; [`App`](crate::App) resets the accumulators +
    /// breadcrumb on the same change.
    pub fn start_session(&mut self) {
        self.session_seq = self.session_seq.wrapping_add(1);
        self.session = Some(self.session_seq);
    }

    /// End the tracking session (Finish / Discard). The disposition of the open log is set
    /// separately with [`request_track`](Activity::request_track).
    pub fn end_session(&mut self) {
        self.session = None;
    }

    /// Whether a tracking session is currently active (riding or paused).
    pub fn is_tracking(&self) -> bool {
        self.session.is_some()
    }

    /// Record a one-shot disposition for the open ride log, drained by the host.
    pub fn request_track(&mut self, action: TrackAction) {
        self.track_action = Some(action);
    }

    /// Take (and clear) the pending [`TrackAction`], if any — the host calls this each frame
    /// and performs the file I/O (finalise-to-GPX / discard).
    pub fn take_track_action(&mut self) -> Option<TrackAction> {
        self.track_action.take()
    }

    /// The elevation (m) to stamp on a logged [`TrackPoint`](obc_route::TrackPoint): the
    /// latest barometric altitude, or 0 before any sample.
    pub(crate) fn track_ele(&self) -> i16 {
        self.last_alt.map_or(0, |a| a as i16)
    }

    /// Clear the ride totals + match + integration state (keeps `mode`/`active_route`/
    /// `session`). Called when a session starts — tracking accumulators begin fresh (spec §6).
    pub(crate) fn reset_ride(&mut self) {
        self.progress_m = 0;
        self.off_route = false;
        self.dist_to_route_m = 0;
        self.ridden_m = 0.0;
        self.moving_s = 0.0;
        self.climb = DeadBand::new();
        self.last_fix = None;
        self.last_ms = None;
        self.last_alt = None;
        self.segment_break = false;
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
    pub(crate) fn record_motion(&mut self, fix: Fix, now_ms: u32) -> Motion {
        if self.mode != Mode::Riding {
            self.last_fix = None;
            self.last_ms = None;
            return Motion::default();
        }
        let first = self.last_fix.is_none();
        let mut counted = false;
        if let (Some(prev), Some(prev_ms)) = (self.last_fix, self.last_ms) {
            let dt = now_ms.saturating_sub(prev_ms) as f32 / 1000.0;
            let dist = ground_dist_m((prev.lon, prev.lat), (fix.lon, fix.lat));
            let implied = if dt > 0.0 { dist / dt } else { 0.0 };
            if dt > 0.0 && dt < MAX_GAP_S && implied < MAX_SPEED_MPS {
                self.ridden_m += dist;
                if implied >= MOVING_MIN_MPS {
                    self.moving_s += dt;
                }
                counted = true;
            }
        }
        // Log the segment anchor (first fix) and every sane fix. A dropped fix (gap /
        // teleport) isn't logged and arms a segment break, so the drawn line and the GPX
        // `<trkseg>` don't leap across the hole.
        let log = first || counted;
        let segment_start = first || self.segment_break;
        self.segment_break = !log;
        self.last_fix = Some(fix);
        self.last_ms = Some(now_ms);
        Motion { log, segment_start }
    }

    /// Integrate one barometric altitude sample into the climbed total, dead-banded so
    /// sensor noise doesn't inflate it. Only while [`Riding`](Mode::Riding); pausing drops
    /// the reference so an altitude change *during* the pause isn't booked on resume.
    pub(crate) fn record_altitude(&mut self, alt_m: f32) {
        // The latest altitude stamps logged track points regardless of mode (it's just the
        // current height); the climb dead-band below only runs while riding.
        self.last_alt = Some(alt_m);
        if self.mode != Mode::Riding {
            // Drop the reference so a height change *during* the pause isn't booked on
            // resume; the accumulated climb is kept.
            self.climb.pause();
            return;
        }
        self.climb.push(alt_m);
    }
}
