//! The ride/tracking model — what the device is *doing*.
//!
//! [`Activity`] holds the operating [`Mode`], which route is loaded, the live map-match result
//! (riding cursor + off-route readout), and the **actually-ridden** accumulators (distance /
//! moving-time / climb). Kept separate from [`AppState`](crate::AppState) (the camera) because the
//! mode and totals outlive any one screen and several screens read them.
//!
//! "Actually-ridden": `done`/`climbed` reflect what the rider did, not the route-relative position
//! — so they keep counting off-route, while `to go`/`to climb` stay route-relative. Distance comes
//! from the GPS [`Fix`] stream and climb from the **separate** barometric
//! [`AltimeterSource`](crate::AltimeterSource); the two integrate independently.

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

/// A one-shot disposition for the **current** ride log, set by a screen and drained by the host
/// (`take_track_action`) which owns the file I/O.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackAction {
    /// Finalise the open log to the host's saved-ride artifact (Finish, or "Save & start new")
    /// — a `.gpx` on the sim, the durable `RD{id}.ORD` ride object on the device.
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
/// actually-ridden accumulators. Small and `Copy` — the screens read it by value.
#[derive(Debug, Clone, Copy, Default)]
pub struct Activity {
    pub mode: Mode,
    /// Index into the route [`Catalog`](crate::route::Catalog) of the loaded route, or `None` when
    /// idle. The geometry is opened separately by the host (only the active route is resident).
    pub active_route: Option<usize>,

    // tracking session (distinct from the navigated route)
    /// The active **tracking session** id, or `None` when not tracking. A session spans from a
    /// route load to Finish/Discard, and survives a "Swap route only" — so it's keyed separately
    /// from [`active_route`](Activity::active_route) (which the matcher follows). The host
    /// reconciles the open ride log to this id.
    pub session: Option<u32>,
    /// Monotonic id source for [`session`](Activity::session); only increments, so a new session
    /// can't collide with a just-finished one.
    session_seq: u32,
    /// A one-shot disposition for the open log, drained by the host via
    /// [`take_track_action`](Activity::take_track_action).
    track_action: Option<TrackAction>,

    // live map-match (from the GPS fix)
    /// Total distance of the active route (m), mirrored from its header so the riding views can
    /// compute the progress fraction without re-reading the route. `0` when none loaded.
    pub route_total_m: u32,
    /// Matched distance along the route (m): the riding cursor / progress bar. Frozen while
    /// off-route.
    pub progress_m: u32,
    /// Whether the rider is currently off-route.
    pub off_route: bool,
    /// Live cross-track distance to the route (m) — the "off route · NNN m" readout.
    pub dist_to_route_m: u32,

    // actually-ridden accumulators
    /// Distance actually pedalled (m) — the `done` stat. Counts **every** sane fix, including
    /// sub-threshold creep, so it's the true total covered.
    pub ridden_m: f32,
    /// Distance covered **while moving** (m): only fixes at or above [`MOVING_MIN_MPS`] — the
    /// numerator of Avg. Kept separate from [`ridden_m`](Activity::ridden_m) so the average pairs
    /// moving distance with moving time (mixing them inflated Avg).
    moving_m: f32,
    /// Moving time (s), accumulated only above [`MOVING_MIN_MPS`] — denominator of Avg.
    pub moving_s: f32,

    // integration state (private)
    /// Previous fix + its host timestamp, to integrate distance/time between ticks.
    last_fix: Option<Fix>,
    last_ms: Option<u32>,
    /// Dead-banded barometric climb — the `climbed` stat. The same hysteresis integrator the route
    /// converter uses, so an on-route ride lands near the route's precomputed ascent.
    climb: DeadBand<f32>,
    /// Latest barometric altitude (m), stamped onto each logged [`TrackPoint`]'s elevation.
    last_alt: Option<f32>,
    /// `true` when a dropped fix (GPS gap / teleport) left a hole, so the next logged point starts a
    /// fresh track segment.
    segment_break: bool,
}

impl Activity {
    /// A fresh activity in the given mode, no route loaded and no ride recorded.
    pub fn new(mode: Mode) -> Self {
        Activity { mode, ..Default::default() }
    }

    /// Average speed (km/h) over the moving time, or `None` before any moving time has accrued (so
    /// the Statistics screen shows a placeholder, not `NaN`). Moving-only distance over moving time,
    /// so sub-threshold creep (counted in `ridden_m`) can't inflate it.
    pub fn avg_kmh(&self) -> Option<f32> {
        (self.moving_s > 0.0).then(|| self.moving_m / self.moving_s * 3.6)
    }

    /// Climb actually done (m) — barometric and dead-banded — the `climbed` stat.
    pub fn climb_m(&self) -> f32 {
        self.climb.ascent()
    }

    /// Average moving speed in cm/s — the ride object's `avg_speed` field, the same quotient as
    /// [`avg_kmh`](Activity::avg_kmh) in the wire's integer unit. 0 before any moving time; the
    /// `u16` saturates well above any bicycle speed.
    pub fn avg_speed_cms(&self) -> u16 {
        if self.moving_s <= 0.0 {
            return 0;
        }
        (self.moving_m / self.moving_s * 100.0) as u16 // float→int casts saturate
    }

    /// The current barometric elevation (m): the latest altimeter sample, or `None` before the
    /// first. Unlike [`climb_m`](Activity::climb_m) (dead-banded *ascent*) this is the raw present
    /// height and follows the altimeter in any [`Mode`]. Read by the
    /// [`Elevation`](crate::stat_fields::StatField::Elevation) tile.
    pub fn current_elevation_m(&self) -> Option<f32> {
        self.last_alt
    }

    /// Begin a fresh tracking session, assigning the next monotonic
    /// [`session`](Activity::session) id. The host opens a new ride log when it sees the id change;
    /// [`App`](crate::App) resets the accumulators + breadcrumb on the same change.
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
    /// and performs the file I/O (finalise / discard).
    pub fn take_track_action(&mut self) -> Option<TrackAction> {
        self.track_action.take()
    }

    /// Non-consuming peek at whether a [`TrackAction`] is pending — lets the host gate its per-tick
    /// storage reconcile on actual change without draining the one-shot.
    pub fn has_track_action(&self) -> bool {
        self.track_action.is_some()
    }

    /// The elevation (m) to stamp on a logged [`TrackPoint`](obc_route::TrackPoint): the
    /// latest barometric altitude, or 0 before any sample.
    pub(crate) fn track_ele(&self) -> i16 {
        self.last_alt.map_or(0, |a| a as i16)
    }

    /// Clear the ride totals + match + integration state (keeps `mode`/`active_route`/`session`).
    /// Called when a session starts, so tracking accumulators begin fresh.
    pub(crate) fn reset_ride(&mut self) {
        self.progress_m = 0;
        self.off_route = false;
        self.dist_to_route_m = 0;
        self.ridden_m = 0.0;
        self.moving_m = 0.0;
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
    /// [`LocationSource`](crate::LocationSource) contract this is called once per fresh GPS sample,
    /// so consecutive calls are a GPS period apart — the interval the gate below is sized for. Only
    /// accumulates while [`Riding`](Mode::Riding); a sane-interval gate drops dropouts and
    /// teleports. Pausing drops the anchor so resuming doesn't book the gap.
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
            // A non-advancing clock (`dt <= 0`: two fixes stamped the same ms, or a source replaying
            // a stale fix) can't be integrated — `dist / dt` would manufacture an infinite implied
            // speed and reject the *next* real move as a teleport. Coalesce into the anchor instead:
            // advance `last_fix`/`last_ms`, log nothing, and do **not** arm a segment break (no time
            // or travel elapsed).
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
                    // Above the moving threshold: book distance *and* time toward Avg. Sub-threshold
                    // creep adds to `ridden_m` but not here, so distance and time stay paired.
                    self.moving_m += dist;
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

    /// Integrate one barometric altitude sample into the climbed total, dead-banded so sensor noise
    /// doesn't inflate it. Only while [`Riding`](Mode::Riding); pausing drops the reference so an
    /// altitude change *during* the pause isn't booked on resume.
    pub(crate) fn record_altitude(&mut self, alt_m: f32) {
        // Reject non-finite samples (a baro driver hiccup): `+inf - ref = +inf >= DEADBAND` would
        // book *infinite* ascent, permanently poisoning `climbed`, and must never stamp a logged
        // elevation.
        if !alt_m.is_finite() {
            return;
        }
        // The latest altitude stamps logged track points regardless of mode; the climb dead-band
        // below only runs while riding.
        self.last_alt = Some(alt_m);
        if self.mode != Mode::Riding {
            // Drop the reference so a height change during the pause isn't booked on resume; the
            // accumulated climb is kept.
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

    /// A real 1 Hz fix stream (one `record_motion` per fix) integrates distance and moving time and
    /// is **never** rejected as a teleport.
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

    /// Avg must pair moving distance with moving time: a sub-threshold "creep" interval (a slow
    /// red-light roll, below [`MOVING_MIN_MPS`]) adds to the `done` total but *not* to Avg, so it
    /// can't drag the reported average above any speed actually sustained.
    #[test]
    fn sub_threshold_creep_does_not_inflate_avg() {
        // ~0.56 m north over 1 s ≈ 0.56 m/s — comfortably below MOVING_MIN_MPS (0.8).
        const CREEP_UD: i32 = 5;
        let mut a = Activity::new(Mode::Riding);
        a.record_motion(Fix::at(BASE_LAT, LON), 0); // anchor

        // One real moving step (~5 m/s), then one creep step (~0.56 m/s).
        a.record_motion(Fix::at(BASE_LAT + STEP_UD, LON), 1000);
        a.record_motion(Fix::at(BASE_LAT + STEP_UD + CREEP_UD, LON), 2000);

        // The creep distance lands in `done` but only the moving interval counts toward Avg.
        assert_eq!(a.moving_s, 1.0, "only the above-threshold interval counts as moving time");
        assert!(a.ridden_m > 5.2, "creep distance is still in the done total, got {}", a.ridden_m);

        let avg = a.avg_kmh().expect("moving time accrued");
        // Avg reflects the ~5 m/s moving step (~18 km/h), *not* the creep-padded total.
        assert!((16.0..=19.5).contains(&avg), "avg must track the moving step, got {avg}");
        let inflated = a.ridden_m / a.moving_s * 3.6; // the old (buggy) total-over-moving-time figure
        assert!(avg < inflated, "moving-only avg ({avg}) must be below the creep-inflated {inflated}");
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

    /// `avg_speed_cms` is the wire-unit twin of `avg_kmh`: same quotient, cm/s, 0 before any
    /// moving time (the ride object's field must never be NaN-derived garbage).
    #[test]
    fn avg_speed_cms_matches_avg_kmh() {
        let mut a = Activity::new(Mode::Riding);
        assert_eq!(a.avg_speed_cms(), 0, "no moving time yet → 0, not NaN");
        a.record_motion(Fix::at(BASE_LAT, LON), 0);
        for step in 1..=4u32 {
            a.record_motion(Fix::at(BASE_LAT + STEP_UD * step as i32, LON), step * 1000);
        }
        let kmh = a.avg_kmh().unwrap();
        let cms = a.avg_speed_cms();
        assert!((cms as f32 - kmh / 3.6 * 100.0).abs() < 1.0, "{cms} cm/s vs {kmh} km/h");
    }

    // Barometric climb — `record_altitude` / `climb_m`.

    /// The dead-band (3.0 m) is why climb isn't pure baro noise: a sub-3 m wiggle books *nothing*,
    /// while a clean climb past the band books its full delta.
    #[test]
    fn climb_ignores_sub_deadband_noise_and_books_clear_gains() {
        let mut a = Activity::new(Mode::Riding);
        a.record_altitude(100.0); // the reference; books nothing on its own
        assert_eq!(a.climb_m(), 0.0, "the first sample only anchors");

        // Noise: +2.9 m is inside the 3.0 m dead-band — ignored, and crucially does NOT
        // re-anchor, so the reference is still 100.0.
        a.record_altitude(102.9);
        assert_eq!(a.climb_m(), 0.0, "a 2.9 m wiggle is below the 3.0 m dead-band");

        // A clean climb to 105.0: 5 m above the *still-100* reference → books the whole 5 m.
        a.record_altitude(105.0);
        assert_eq!(a.climb_m(), 5.0, "a clear gain past the band books its full delta, got {}", a.climb_m());
    }

    /// Descending must never subtract from `climbed` (total ascent only). A rolling profile
    /// (up 6, down 6, up 6) books 12 m of climb, not 6.
    #[test]
    fn climb_accumulates_only_ascent_across_rolling_terrain() {
        let mut a = Activity::new(Mode::Riding);
        a.record_altitude(100.0);
        a.record_altitude(106.0); // +6 booked
        a.record_altitude(100.0); // -6 is descent, not climb
        a.record_altitude(106.0); // +6 booked again
        assert_eq!(a.climb_m(), 12.0, "two 6 m climbs book 12 m, the dip in between doesn't subtract");
    }

    /// Pausing drops the dead-band *reference* but keeps the accumulated total, so a height change
    /// during a rest (weather drift, carrying the bike upstairs) is not booked on resume.
    #[test]
    fn pause_drops_reference_so_climb_during_a_rest_is_not_booked() {
        let mut a = Activity::new(Mode::Riding);
        a.record_altitude(100.0); // anchor
        a.record_altitude(110.0); // a clean +10 m climb while riding
        assert_eq!(a.climb_m(), 10.0);

        // Pause. The next altitude samples arrive while not riding: each drops the reference and
        // books nothing, even though the height swings wildly during the rest.
        a.mode = Mode::Paused;
        a.record_altitude(160.0); // +50 m of drift during the stop
        a.record_altitude(160.0);
        assert_eq!(a.climb_m(), 10.0, "a height change during the pause must not accrue");

        // Resume. The first riding sample re-anchors at the *current* height (160) instead of
        // measuring the 50 m hole across the pause; only genuine post-resume climb adds.
        a.mode = Mode::Riding;
        a.record_altitude(160.0); // re-anchor at 160, books nothing
        assert_eq!(a.climb_m(), 10.0, "resuming re-anchors, it does not book the pause gap");
        a.record_altitude(165.0); // a real +5 m after resuming
        assert_eq!(a.climb_m(), 15.0, "only genuine post-resume climb adds, got {}", a.climb_m());
    }

    /// A NaN or infinite altitude (a baro driver hiccup) must not corrupt the climb total: it's
    /// silently ignored, never allowed to inflate or `NaN`-poison the stat.
    #[test]
    fn climb_ignores_nan_and_infinite_altitude() {
        let mut a = Activity::new(Mode::Riding);
        a.record_altitude(100.0);
        a.record_altitude(105.0); // a clean +5 m
        assert_eq!(a.climb_m(), 5.0);

        // Garbage samples: neither must change the total, and neither must poison it to NaN/inf.
        a.record_altitude(f32::NAN);
        a.record_altitude(f32::INFINITY);
        a.record_altitude(f32::NEG_INFINITY);
        assert_eq!(a.climb_m(), 5.0, "NaN/inf samples are ignored, total stays finite, got {}", a.climb_m());
        assert!(a.climb_m().is_finite(), "the climb total must never become non-finite");

        // …and a real sample after the garbage still integrates against the last good reference
        // (105), proving the bad samples didn't re-anchor either.
        a.record_altitude(110.0);
        assert_eq!(a.climb_m(), 10.0, "a good sample after garbage measures from the last good ref");
    }

    /// The latest altitude stamps logged track points regardless of mode, so a point logged during
    /// a paused frame still carries a sane elevation (distinct from the ride-only climb integrator).
    #[test]
    fn track_ele_tracks_latest_altitude_even_when_paused() {
        let mut a = Activity::new(Mode::Riding);
        assert_eq!(a.track_ele(), 0, "no sample yet → 0 elevation");
        a.record_altitude(123.7);
        assert_eq!(a.track_ele(), 123, "rounds toward zero into i16 metres");
        a.mode = Mode::Paused;
        a.record_altitude(200.4);
        assert_eq!(a.track_ele(), 200, "the stamped elevation follows the latest sample even paused");
    }

    /// `reset_ride` wipes the climb total *and* its reference, so a second ride doesn't inherit the
    /// first's ascent or measure its first climb against the first's last altitude.
    #[test]
    fn reset_ride_clears_the_climb_total_and_reference() {
        let mut a = Activity::new(Mode::Riding);
        a.record_altitude(100.0);
        a.record_altitude(120.0); // +20 m on ride one
        assert_eq!(a.climb_m(), 20.0);

        a.reset_ride();
        assert_eq!(a.climb_m(), 0.0, "a new session starts climb at zero");

        // Ride two: the first sample re-anchors fresh; the old 120 m must not be the reference.
        a.record_altitude(500.0); // far from ride one's last altitude
        a.record_altitude(505.0); // a clean +5 m on ride two
        assert_eq!(a.climb_m(), 5.0, "ride two measures from its own anchor, got {}", a.climb_m());
    }

    // `record_motion` numeric edges: the gate thresholds and `ground_dist_m` extremes the mid-band
    // cases above don't reach.

    /// The teleport gate is `implied < MAX_SPEED_MPS` (30 m/s): a move just under 30 m/s is counted,
    /// at/over 30 is dropped — pinning the `<` boundary against a `<=` off-by-one.
    #[test]
    fn teleport_gate_boundary_is_just_under_max_speed() {
        // dt = 100 ms. At 30 m/s the gate trips, so a move of just under 3.0 m must pass and a
        // move of 3.0 m (→ exactly 30 m/s) must be rejected.
        let mut a = Activity::new(Mode::Riding);
        a.record_motion(Fix::at(BASE_LAT, LON), 1000);
        // ~26 µdeg north ≈ 2.9 m → 29 m/s over 0.1 s: under the gate, counted.
        let under = a.record_motion(Fix::at(BASE_LAT + 26, LON), 1100);
        assert!(under.log, "29 m/s is under the 30 m/s teleport gate → counted");
        assert!(a.ridden_m > 2.5 && a.ridden_m < 3.3, "the ~2.9 m step is booked, got {}", a.ridden_m);

        // Now a step that implies >= 30 m/s over the next 0.1 s: ~45 µdeg ≈ 5 m → 50 m/s, dropped.
        let over = a.record_motion(Fix::at(BASE_LAT + 26 + STEP_UD, LON), 1200);
        assert!(!over.log, "50 m/s is over the gate → dropped as a teleport");
        assert!(a.ridden_m < 3.3, "the over-gate step booked nothing extra, got {}", a.ridden_m);
    }

    /// The moving-threshold gate is `implied >= MOVING_MIN_MPS` (0.8 m/s): at/above it the
    /// interval's time counts toward Avg, below it only distance does — pinning the `>=` boundary
    /// against a `>` regression. Distance is always booked.
    #[test]
    fn moving_threshold_boundary_counts_at_exactly_min_speed() {
        // dt = 1 s. 0.8 m/s ⇒ a 0.8 m step. 0.8 m north ≈ 7.2 µdeg; use 8 µdeg (~0.89 m) to land
        // just at/above the threshold, and 6 µdeg (~0.67 m) to land just below it.
        let mut at = Activity::new(Mode::Riding);
        at.record_motion(Fix::at(BASE_LAT, LON), 0);
        at.record_motion(Fix::at(BASE_LAT + 8, LON), 1000);
        assert_eq!(at.moving_s, 1.0, "≈0.89 m/s is at/above 0.8 → the interval counts as moving");
        assert!(at.ridden_m > 0.0, "and the distance is booked regardless");

        let mut below = Activity::new(Mode::Riding);
        below.record_motion(Fix::at(BASE_LAT, LON), 0);
        below.record_motion(Fix::at(BASE_LAT + 6, LON), 1000);
        assert_eq!(below.moving_s, 0.0, "≈0.67 m/s is below 0.8 → no moving time");
        assert!(below.ridden_m > 0.0, "but the creep distance is still in the done total");
    }

    /// `ground_dist_m` uses a local-equirectangular projection with a raw `lon_b - lon_a` delta and
    /// **no ±180° wrap**, so two points either side of the date line (physically ~2 µdeg apart) read
    /// as a ~40 000 km jump and are dropped by the teleport gate. Pins the current behaviour:
    /// crossing the antimeridian is unsupported but degrades to a dropped interval, not a crash. If
    /// wrap handling is added, flip the first assertion to expect the short real distance.
    #[test]
    fn motion_across_antimeridian_is_dropped_as_a_teleport() {
        const NEAR_180: i32 = 179_999_990; // ~1 µdeg west of +180°
        const PAST_180: i32 = -179_999_990; // ~1 µdeg east of -180° — physically ~2 µdeg apart
        let mut a = Activity::new(Mode::Riding);
        a.record_motion(Fix::at(0, NEAR_180), 0);
        let m = a.record_motion(Fix::at(0, PAST_180), 1000);
        // The unwrapped longitude delta is ~360° → an enormous implied speed → over the gate.
        assert!(!m.log, "an unwrapped date-line crossing reads as a teleport and is dropped");
        assert_eq!(a.ridden_m, 0.0, "no planet-circling distance is ever booked");
    }

    /// `ground_dist_m` near the pole: longitude lines converge, so a microdegree of longitude is
    /// almost no ground distance at 85°N. A polar fix stream must not manufacture a teleport from
    /// the raw longitude delta — `cos(lat)` has to shrink it. Guards the metric at a latitude
    /// extreme the Berlin-band cases never reach.
    #[test]
    fn motion_near_the_pole_shrinks_longitude_distance() {
        const HIGH_LAT: i32 = 85_000_000; // 85°N
        let mut a = Activity::new(Mode::Riding);
        a.record_motion(Fix::at(HIGH_LAT, 0), 0);
        // 100 µdeg of longitude ≈ 11 m at the equator, but ~1 m at 85°N (×cos 85°).
        a.record_motion(Fix::at(HIGH_LAT, 100), 1000);
        assert!(a.ridden_m < 3.0, "longitude distance is heavily foreshortened at 85°N, got {}", a.ridden_m);
        assert!(a.ridden_m > 0.0, "but it's still a real, non-zero step");
    }
}
