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

    /// Non-consuming peek at whether a [`TrackAction`] is pending. Lets the host gate its
    /// per-tick storage reconcile on actual change without draining the one-shot — the action is
    /// still consumed only by [`take_track_action`](Activity::take_track_action), once processed.
    pub fn has_track_action(&self) -> bool {
        self.track_action.is_some()
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

    /// Integrate one position fix into the ridden distance + moving time. By the
    /// [`LocationSource`](crate::LocationSource) contract this is called exactly once per fresh
    /// GPS sample (the source returns `None` between fixes), so consecutive calls are a GPS
    /// period apart — the per-second interval the gate below is sized for. Only accumulates
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
            // Defensive guard on the fresh-fix contract: a fresh sample always carries a later
            // `RideClock`, so `dt` is normally the GPS period (~1 s). A non-advancing clock
            // (`dt <= 0` — two fixes stamped the same millisecond, or a misbehaving source that
            // replays a stale fix) can't be integrated: `dist / dt` would manufacture an
            // infinite implied speed and reject the *next* real move as a teleport. Coalesce it
            // into the anchor instead — advance `last_fix`/`last_ms`, log nothing, and (unlike a
            // real gap) do **not** arm a segment break, since no time and no travel elapsed.
            if dt <= 0.0 {
                self.last_fix = Some(fix);
                self.last_ms = Some(now_ms);
                return Motion { log: false, segment_start: false };
            }
            let dist = ground_dist_m((prev.lon, prev.lat), (fix.lon, fix.lat));
            let implied = dist / dt;
            if dt < MAX_GAP_S && implied < MAX_SPEED_MPS {
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

#[cfg(test)]
mod tests {
    use super::*;

    // A point near Berlin. `record_motion` only reads lon/lat + the clock, so a fix's course /
    // speed are irrelevant here and we build with the stationary `Fix::at` constructor.
    const LON: i32 = 13_405_000;
    const BASE_LAT: i32 = 52_520_000;
    /// ~45 microdegrees of latitude ≈ 5.0 m north — roughly one second of riding at ~5 m/s,
    /// comfortably inside the [`MOVING_MIN_MPS`]..[`MAX_SPEED_MPS`] band.
    const STEP_UD: i32 = 45;

    /// The headline #43 case: a real 1 Hz fix stream — the source returns `None` between the
    /// per-second `Some`s, so `record_motion` runs once per fix — integrates distance and moving
    /// time and is **never** rejected as a teleport. (The old "same fix every ~8 ms" replay made
    /// the once-a-second move look like an 8 ms teleport and recorded nothing.)
    #[test]
    fn one_hz_fix_stream_integrates_without_teleport_rejection() {
        let mut a = Activity::new(Mode::Riding);

        // t = 0: the segment anchor. Logged, starts a segment, books no distance yet.
        let m0 = a.record_motion(Fix::at(BASE_LAT, LON), 0);
        assert!(m0.log && m0.segment_start);
        assert_eq!(a.ridden_m, 0.0);
        assert_eq!(a.moving_s, 0.0);

        // Four more fixes, one per second, each ~5 m further north.
        for step in 1..=4u32 {
            let lat = BASE_LAT + STEP_UD * step as i32;
            let m = a.record_motion(Fix::at(lat, LON), step * 1000);
            assert!(m.log, "every per-second fix is logged");
            assert!(!m.segment_start, "a continuous ride stays one segment");
        }

        // Four ~5 m steps ⇒ ~20 m ridden, all four 1 s intervals counted as moving (~5 m/s).
        assert!((16.0..=24.0).contains(&a.ridden_m), "ridden ≈ 20 m, got {}", a.ridden_m);
        assert_eq!(a.moving_s, 4.0, "every 1 s interval counts toward moving time");
        let avg = a.avg_kmh().expect("moving time accrued");
        assert!((10.0..=25.0).contains(&avg), "~18 km/h, got {avg}");
    }

    /// A stopped rider still emits fresh, identical-position fixes at the GPS rate. They keep
    /// logging (the `.gpx` records the stop) and advance the clock, but book no distance and no
    /// moving time — and must not be mistaken for a dropout / segment break.
    #[test]
    fn stationary_fixes_log_but_book_no_distance() {
        let mut a = Activity::new(Mode::Riding);
        let f = Fix::at(BASE_LAT, LON);
        assert!(a.record_motion(f, 0).log);
        for s in 1..=3u32 {
            let m = a.record_motion(f, s * 1000);
            assert!(m.log, "an identical fix is a real sample, still logged");
            assert!(!m.segment_start, "standing still is not a segment break");
        }
        assert_eq!(a.ridden_m, 0.0);
        assert_eq!(a.moving_s, 0.0);
        assert_eq!(a.avg_kmh(), None);
    }

    /// Defensive guard: two fixes stamped the same millisecond (a contract violation / clock
    /// stall) are coalesced — the duplicate logs nothing and, crucially, does **not** arm a
    /// segment break, so the following real fix integrates normally instead of being split off
    /// or rejected as an infinite-speed teleport.
    #[test]
    fn same_millisecond_duplicate_is_coalesced_not_a_teleport() {
        let mut a = Activity::new(Mode::Riding);
        a.record_motion(Fix::at(BASE_LAT, LON), 1000);
        // A second fix at the *same* now_ms, already moved ~5 m: dt == 0, can't be integrated.
        let dup = a.record_motion(Fix::at(BASE_LAT + STEP_UD, LON), 1000);
        assert!(!dup.log, "a same-instant fix isn't logged");
        assert!(!dup.segment_start);
        assert_eq!(a.ridden_m, 0.0, "no distance booked on a zero-length interval");

        // One second later the next genuine fix is a clean, counted, single-segment step — the
        // coalesced duplicate left the anchor at the *latest* position, so there's no teleport.
        let next = a.record_motion(Fix::at(BASE_LAT + 2 * STEP_UD, LON), 2000);
        assert!(next.log && !next.segment_start);
        assert!((4.0..=6.0).contains(&a.ridden_m), "one ~5 m step, got {}", a.ridden_m);
        assert_eq!(a.moving_s, 1.0);
    }

    /// A genuine teleport (a >30 m/s jump in one GPS period — manual drag / GPS glitch) is still
    /// dropped and arms a fresh segment, so the breadcrumb and `<trkseg>` don't leap the hole.
    #[test]
    fn teleport_is_dropped_and_breaks_the_segment() {
        let mut a = Activity::new(Mode::Riding);
        a.record_motion(Fix::at(BASE_LAT, LON), 0);
        // ~1000 µdeg north in 1 s ≈ 111 m/s — far over MAX_SPEED_MPS.
        let jump = a.record_motion(Fix::at(BASE_LAT + 1_000, LON), 1000);
        assert!(!jump.log, "the teleport itself isn't logged");
        assert_eq!(a.ridden_m, 0.0);
        // The next sane fix opens a new segment across the hole.
        let after = a.record_motion(Fix::at(BASE_LAT + 1_000 + STEP_UD, LON), 2000);
        assert!(after.log && after.segment_start, "resume starts a fresh <trkseg>");
    }

    /// A long gap (GPS dropout > MAX_GAP_S) is skipped rather than booked as a straight-line
    /// sprint across the missing time, and likewise breaks the segment.
    #[test]
    fn long_gap_is_skipped_and_breaks_the_segment() {
        let mut a = Activity::new(Mode::Riding);
        a.record_motion(Fix::at(BASE_LAT, LON), 0);
        // 30 s later, only ~5 m away (a slow reconnect): within MAX_SPEED but past MAX_GAP_S.
        let reconnect = a.record_motion(Fix::at(BASE_LAT + STEP_UD, LON), 30_000);
        assert!(!reconnect.log, "the dropout interval is skipped, not booked");
        assert_eq!(a.ridden_m, 0.0);
        let after = a.record_motion(Fix::at(BASE_LAT + 2 * STEP_UD, LON), 31_000);
        assert!(after.log && after.segment_start);
    }

    /// Outside [`Riding`](Mode::Riding) no motion is integrated and the anchor is dropped, so
    /// resuming can't book the distance covered while paused.
    #[test]
    fn paused_drops_anchor_and_books_nothing() {
        let mut a = Activity::new(Mode::Paused);
        let m = a.record_motion(Fix::at(BASE_LAT, LON), 0);
        assert_eq!(m, Motion::default());
        a.record_motion(Fix::at(BASE_LAT + STEP_UD, LON), 1000);
        assert_eq!(a.ridden_m, 0.0);
    }
}
