//! GPX replay as a simulated GPS sensor.
//!
//! [`GpxPlayer`] is the host's stand-in for the device's GPS chip when replaying a recorded
//! [`Track`]: it implements [`LocationSource`] like a real receiver's driver would, so the
//! shared app can't tell a replay from a live fix.
//!
//! ## Fidelity
//! GPX stores only position + time, never course or speed, so they're derived the
//! way a GPS receiver does — from motion. Course is the bearing over a short
//! look-ahead window (smoothing per-point jitter), and **when the track is
//! stationary the reported course is `None`**, matching a real receiver that
//! drops its heading when it isn't moving. That `None` flows straight into the
//! user marker (becomes a non-directional dot) and heading-up rotation (holds its
//! last orientation), so heading behaves identically to a live sensor.
//!
//! ## Clock
//! The player is a pure *playback-time → [`Fix`]* function; the host drives it by
//! calling [`advance`](GpxPlayer::advance) with each frame's elapsed wall-clock
//! time (scaled by the playback-speed multiplier). Keeping the clock external
//! makes the interpolation unit-testable without a real timer.

use obc_ports::{Fix, LocationSource};

use crate::gpx::Track;

/// Earth radius (mean) in meters, for the haversine distance used to derive speed.
const EARTH_R_M: f64 = 6_371_000.0;

/// Below this ground speed the simulated receiver reports no course (`None`),
/// mirroring a real GPS that can't determine heading while stationary.
const MOVING_THRESHOLD_MPS: f32 = 0.5;

/// Seconds of look-ahead used to derive course/speed. A small window smooths the
/// per-point bearing jitter you'd otherwise get from dense, noisy track points.
const LOOK_AHEAD_S: f64 = 2.0;

/// Simulated GPS fix cadence, in **seconds of playback time**. Real consumer GPS / bike
/// computers deliver fixes on a fixed ~1 Hz clock, not once per render frame — throttling
/// [`poll`](GpxPlayer::poll) to this keeps the recorded track + breadcrumb at a realistic
/// point density no matter how fast the host renders or how high the replay speed is set.
const GPS_PERIOD_S: f64 = 1.0;

/// Replays a parsed [`Track`] as a [`LocationSource`]. Holds the playback cursor
/// (`t`, seconds into the track), whether it's playing, and the speed multiplier.
pub struct GpxPlayer {
    track: Track,
    /// Current playback position, seconds from the start of the track.
    t: f64,
    playing: bool,
    /// Playback speed multiplier: `1.0` = real time, up to `10.0`.
    speed: f32,
    /// Playback time of the last fix [`poll`](GpxPlayer::poll) emitted, to throttle to
    /// ~[`GPS_PERIOD_S`]. `None` forces the next poll to emit (set on new / seek / play).
    last_fix_t: Option<f64>,
}

impl GpxPlayer {
    /// Build a player for `track`, paused at the start at real-time (1×) speed.
    pub fn new(track: Track) -> Self {
        GpxPlayer { track, t: 0.0, playing: false, speed: 1.0, last_fix_t: None }
    }

    /// Total track length in seconds.
    pub fn duration(&self) -> f64 {
        self.track.duration()
    }

    /// Number of points in the underlying track (for the panel's readout).
    pub fn point_count(&self) -> usize {
        self.track.points.len()
    }

    /// Current playback position in seconds.
    pub fn time(&self) -> f64 {
        self.t
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    pub fn speed(&self) -> f32 {
        self.speed
    }

    /// Set the playback-speed multiplier, clamped to the real-time..10× range.
    pub fn set_speed(&mut self, speed: f32) {
        self.speed = speed.clamp(1.0, 10.0);
    }

    /// Start playback. If the cursor is already at the end, restart from the top
    /// (so pressing play after an auto-pause-at-end replays the track).
    pub fn play(&mut self) {
        if self.t >= self.duration() {
            self.t = 0.0;
        }
        self.playing = true;
        self.last_fix_t = None; // emit a fix promptly on (re)start
    }

    pub fn pause(&mut self) {
        self.playing = false;
    }

    /// Toggle play/pause (the panel's play button).
    pub fn toggle(&mut self) {
        if self.playing {
            self.pause();
        } else {
            self.play();
        }
    }

    /// Jump to an absolute time (seconds), clamped to the track. Leaves the
    /// play/pause state unchanged — the scrubber works while playing or paused.
    pub fn seek(&mut self, t: f64) {
        self.t = t.clamp(0.0, self.duration());
        self.last_fix_t = None; // a scrub jumps the fix immediately, ignoring the GPS cadence
    }

    /// Advance the playback cursor by `real_dt` seconds of wall-clock time, scaled
    /// by the speed multiplier. When the cursor reaches the end it stops there
    /// (pause-at-last-fix). A no-op while paused.
    pub fn advance(&mut self, real_dt: f64) {
        if !self.playing {
            return;
        }
        let d = self.duration();
        if d <= 0.0 {
            self.playing = false;
            return;
        }
        self.t += real_dt * self.speed as f64;
        if self.t >= d {
            self.t = d;
            self.playing = false;
        }
    }

    /// The fix at playback time `t`: interpolated position plus derived
    /// course/speed. `None` only for an empty track.
    fn fix_at(&self, t: f64) -> Option<Fix> {
        if self.track.points.is_empty() {
            return None;
        }
        let t = t.clamp(0.0, self.duration());
        let (lat, lon) = self.interp_pos(t);
        let (course, speed) = self.course_speed(t);
        Some(Fix { lat: lat.round() as i32, lon: lon.round() as i32, course, speed_mps: Some(speed) })
    }

    /// The bracketing track points and interpolation factor for time `t`: the indices
    /// `(lo, hi)` whose segment contains `t` and the fraction `f ∈ [0, 1]` from `lo`
    /// toward `hi`. Before the first / after the last point both indices collapse to that
    /// endpoint (so any interpolation reproduces it), and a zero-span segment uses `f = 0`.
    /// `None` only for an empty track. `t` is assumed already clamped to `[0, duration]`.
    /// Uses `partition_point`, so it relies on points being sorted ascending by `t`.
    fn bracket(&self, t: f64) -> Option<(usize, usize, f64)> {
        let pts = &self.track.points;
        let idx = pts.partition_point(|p| p.t <= t);
        if idx == 0 {
            return (!pts.is_empty()).then_some((0, 0, 0.0));
        }
        if idx >= pts.len() {
            let last = pts.len() - 1;
            return Some((last, last, 0.0));
        }
        let (a, b) = (&pts[idx - 1], &pts[idx]);
        let span = b.t - a.t;
        let f = if span > 0.0 { (t - a.t) / span } else { 0.0 };
        Some((idx - 1, idx, f))
    }

    /// The barometric elevation (m) at playback time `t`, linearly interpolated from the
    /// track's `<ele>` values — the simulator's stand-in for a pressure-altimeter reading,
    /// fed into the [`BaroSensor`](crate::baro::BaroSensor). `None` where the track carries
    /// no elevation around `t`. Deliberately separate from [`poll`](Self::poll): the baro
    /// and the GPS fix are independent sensors, sampled on their own cadences.
    pub fn elevation_at(&self, t: f64) -> Option<f32> {
        let t = t.clamp(0.0, self.duration());
        let (lo, hi, f) = self.bracket(t)?;
        let pts = &self.track.points;
        match (pts[lo].ele, pts[hi].ele) {
            (Some(ea), Some(eb)) => Some(ea + (eb - ea) * f as f32),
            // A lone elevation on one side still gives a reading; none on either → None.
            (Some(e), None) | (None, Some(e)) => Some(e),
            (None, None) => None,
        }
    }

    /// Linearly interpolate the position (microdegrees, unrounded) at time `t`.
    /// `t` is assumed already clamped to `[0, duration]`.
    fn interp_pos(&self, t: f64) -> (f64, f64) {
        let pts = &self.track.points;
        let Some((lo, hi, f)) = self.bracket(t) else {
            return (0.0, 0.0); // empty track — callers guard this via `fix_at`
        };
        let (a, b) = (&pts[lo], &pts[hi]);
        (a.lat as f64 + (b.lat - a.lat) as f64 * f, a.lon as f64 + (b.lon - a.lon) as f64 * f)
    }

    /// Derive `(course, speed)` at time `t` from motion over a short window.
    /// Looks `LOOK_AHEAD_S` ahead (or behind, near the end) and measures the
    /// displacement; below [`MOVING_THRESHOLD_MPS`] the course is `None`.
    fn course_speed(&self, t: f64) -> (Option<f32>, f32) {
        let dur = self.duration();
        let (lat0, lon0) = self.interp_pos(t);

        // Prefer a forward window; within `LOOK_AHEAD_S` of the end, look back so
        // the heading stays defined right up to the final fix.
        let (t1, behind) =
            if t + LOOK_AHEAD_S <= dur { (t + LOOK_AHEAD_S, false) } else { ((t - LOOK_AHEAD_S).max(0.0), true) };
        let dt = (t1 - t).abs();
        if dt <= 0.0 {
            return (None, 0.0);
        }
        let (lat1, lon1) = self.interp_pos(t1);

        // Order the endpoints along the direction of travel so the bearing points
        // the way the user is moving.
        let (from, to) = if behind { ((lat1, lon1), (lat0, lon0)) } else { ((lat0, lon0), (lat1, lon1)) };
        let dist = haversine_m(from.0, from.1, to.0, to.1);
        let speed = (dist / dt) as f32;
        let course =
            if speed >= MOVING_THRESHOLD_MPS { Some(bearing_deg(from.0, from.1, to.0, to.1) as f32) } else { None };
        (course, speed)
    }
}

impl LocationSource for GpxPlayer {
    fn poll(&mut self) -> Option<Fix> {
        // Throttle to ~GPS_PERIOD_S of playback time: real GPS delivers fixes on a fixed
        // cadence, so a 60 fps host (or a 10× replay) must not flood the matcher / recorder /
        // breadcrumb with a fix every frame. `None` between ticks = "no new fix yet".
        if self.last_fix_t.is_some_and(|last| (self.t - last).abs() < GPS_PERIOD_S) {
            return None;
        }
        let fix = self.fix_at(self.t)?;
        self.last_fix_t = Some(self.t);
        Some(fix)
    }
}

/// Microdegrees → radians.
fn to_rad(microdeg: f64) -> f64 {
    microdeg * 1e-6 * core::f64::consts::PI / 180.0
}

/// Great-circle distance in meters between two microdegree positions (haversine).
fn haversine_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let p1 = to_rad(lat1);
    let p2 = to_rad(lat2);
    let dphi = to_rad(lat2 - lat1);
    let dlam = to_rad(lon2 - lon1);
    let a = (dphi / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dlam / 2.0).sin().powi(2);
    2.0 * EARTH_R_M * a.sqrt().atan2((1.0 - a).sqrt())
}

/// Initial great-circle bearing in degrees clockwise from north (`0..360`) from
/// point 1 to point 2, both in microdegrees.
fn bearing_deg(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let p1 = to_rad(lat1);
    let p2 = to_rad(lat2);
    let dlam = to_rad(lon2 - lon1);
    let y = dlam.sin() * p2.cos();
    let x = p1.cos() * p2.sin() - p1.sin() * p2.cos() * dlam.cos();
    (y.atan2(x).to_degrees() + 360.0) % 360.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpx::{Track, TrackPoint};

    fn track(pts: &[(i32, i32, f64)]) -> Track {
        Track { points: pts.iter().map(|&(lat, lon, t)| TrackPoint { lat, lon, ele: None, t }).collect() }
    }

    #[test]
    fn elevation_interpolates_between_points() {
        // 200 m at t=0, 300 m at t=10 → 250 m at the midpoint; clamps past the ends.
        let pts = vec![
            TrackPoint { lat: 0, lon: 0, ele: Some(200.0), t: 0.0 },
            TrackPoint { lat: 10_000, lon: 0, ele: Some(300.0), t: 10.0 },
        ];
        let p = GpxPlayer::new(Track { points: pts });
        assert_eq!(p.elevation_at(5.0), Some(250.0));
        assert_eq!(p.elevation_at(0.0), Some(200.0));
        assert_eq!(p.elevation_at(10.0), Some(300.0));
    }

    #[test]
    fn interpolates_midpoint() {
        // Two points 10 s apart; at t=5 the position is the midpoint.
        let p = GpxPlayer::new(track(&[(0, 0, 0.0), (10_000, 20_000, 10.0)]));
        let f = p.fix_at(5.0).unwrap();
        assert_eq!(f.lat, 5_000);
        assert_eq!(f.lon, 10_000);
    }

    #[test]
    fn fix_at_exact_point() {
        let p = GpxPlayer::new(track(&[(0, 0, 0.0), (10_000, 0, 10.0), (10_000, 5_000, 20.0)]));
        let f = p.fix_at(10.0).unwrap();
        assert_eq!((f.lat, f.lon), (10_000, 0));
    }

    #[test]
    fn course_due_north_is_zero() {
        // Move 0.01° north over 10 s (~111 m, well above the moving threshold).
        let p = GpxPlayer::new(track(&[(0, 0, 0.0), (10_000, 0, 10.0)]));
        let c = p.fix_at(0.0).unwrap().course.expect("moving -> has course");
        assert!(!(1.0..=359.0).contains(&c), "due north should be ~0°, got {c}");
    }

    #[test]
    fn course_due_east_is_ninety() {
        // Same latitude, move east; initial bearing ~90°.
        let p = GpxPlayer::new(track(&[(45_000_000, 0, 0.0), (45_000_000, 10_000, 10.0)]));
        let c = p.fix_at(0.0).unwrap().course.expect("moving -> has course");
        assert!((c - 90.0).abs() < 1.0, "due east should be ~90°, got {c}");
    }

    #[test]
    fn stationary_has_no_course() {
        // Identical points: zero speed -> no heading, like a real receiver.
        let p = GpxPlayer::new(track(&[(1_000, 1_000, 0.0), (1_000, 1_000, 10.0)]));
        assert_eq!(p.fix_at(0.0).unwrap().course, None);
    }

    #[test]
    fn crawling_below_threshold_has_no_course() {
        // ~0.11 m over 10 s ≈ 0.01 m/s, below the 0.5 m/s threshold.
        let p = GpxPlayer::new(track(&[(0, 0, 0.0), (1, 0, 10.0)]));
        assert_eq!(p.fix_at(0.0).unwrap().course, None);
    }

    #[test]
    fn advance_pauses_at_end() {
        let mut p = GpxPlayer::new(track(&[(0, 0, 0.0), (10_000, 0, 10.0)]));
        p.play();
        p.advance(5.0);
        assert!(p.is_playing());
        assert_eq!(p.time(), 5.0);
        p.advance(10.0); // overshoots the 10 s duration
        assert!(!p.is_playing());
        assert_eq!(p.time(), 10.0);
    }

    #[test]
    fn poll_throttles_to_a_realistic_gps_rate() {
        // A 60 fps host replaying 5 s of track at 1×: poll runs every frame, but a real GPS
        // only delivers ~1 fix/s — so we expect ~5 fixes, not ~300.
        let mut p = GpxPlayer::new(track(&[(0, 0, 0.0), (1_000_000, 0, 100.0)]));
        p.play();
        let mut fixes = 0;
        for _ in 0..300 {
            p.advance(1.0 / 60.0);
            if p.poll().is_some() {
                fixes += 1;
            }
        }
        assert!((4..=6).contains(&fixes), "≈1 Hz over 5 s, got {fixes} fixes");
    }

    #[test]
    fn seek_re_arms_the_fix_throttle() {
        // A scrub must jump the fix immediately, not wait out the GPS period.
        let mut p = GpxPlayer::new(track(&[(0, 0, 0.0), (1_000_000, 0, 100.0)]));
        p.play();
        assert!(p.poll().is_some(), "first fix after play");
        assert!(p.poll().is_none(), "throttled — no second fix without time passing");
        p.seek(50.0);
        assert!(p.poll().is_some(), "a seek delivers a fresh fix at once");
    }

    #[test]
    fn speed_multiplier_scales_advance() {
        let mut p = GpxPlayer::new(track(&[(0, 0, 0.0), (10_000, 0, 100.0)]));
        p.set_speed(4.0);
        p.play();
        p.advance(2.0); // 2 s wall-clock × 4 = 8 s of track
        assert_eq!(p.time(), 8.0);
    }

    #[test]
    fn speed_clamped_to_range() {
        let mut p = GpxPlayer::new(track(&[(0, 0, 0.0), (1, 0, 1.0)]));
        p.set_speed(50.0);
        assert_eq!(p.speed(), 10.0);
        p.set_speed(0.1);
        assert_eq!(p.speed(), 1.0);
    }

    #[test]
    fn seek_clamps_to_track() {
        let mut p = GpxPlayer::new(track(&[(0, 0, 0.0), (10_000, 0, 40.0)]));
        p.seek(10.0);
        assert_eq!(p.time(), 10.0);
        p.seek(1000.0); // past the end -> clamped to duration
        assert_eq!(p.time(), 40.0);
        p.seek(-5.0); // before the start -> clamped to zero
        assert_eq!(p.time(), 0.0);
    }

    #[test]
    fn play_after_end_restarts() {
        let mut p = GpxPlayer::new(track(&[(0, 0, 0.0), (10_000, 0, 10.0)]));
        p.seek(10.0);
        p.play();
        assert_eq!(p.time(), 0.0);
        assert!(p.is_playing());
    }

    /// A single-point track has zero duration: `fix_at` must return the lone point and park at
    /// `t=0` without dividing by a zero span or panicking.
    #[test]
    fn single_point_track_has_zero_duration() {
        let p = GpxPlayer::new(track(&[(1_000, 2_000, 0.0)]));
        assert_eq!(p.duration(), 0.0, "one point → zero-length track");
        let f = p.fix_at(0.0).expect("a one-point track still yields its single fix");
        assert_eq!((f.lat, f.lon), (1_000, 2_000));
        // A zero-duration track can't move, so there's no derived course.
        assert_eq!(f.course, None, "no span to derive a heading from");
    }

    /// `interp_pos` / `elevation_at` assume `points` are sorted ascending by `t` (both use
    /// `partition_point`, correct only on a monotone predicate). With out-of-order times the
    /// result is unspecified; we assert only that the player stays memory-safe, not sensible.
    #[test]
    fn non_monotonic_times_are_a_documented_precondition() {
        // Times descend then jump — violating the sorted-`t` precondition on purpose.
        let p = GpxPlayer::new(track(&[(0, 0, 10.0), (10_000, 0, 5.0), (20_000, 0, 20.0)]));
        // We make NO claim about which segment is chosen — only that it doesn't panic and
        // returns a real, in-range coordinate (the points span lat 0..20_000).
        let f = p.fix_at(7.0).expect("non-empty track yields a fix");
        assert!((0..=20_000).contains(&f.lat), "stays within the track's points, got lat {}", f.lat);
    }

    /// `elevation_at` with elevation missing on one or both bracketing points: a lone elevation
    /// on either side still reads (last good value), but a gap with none on either side returns
    /// `None` so the baro reports "no reading", not 0.
    #[test]
    fn elevation_at_handles_missing_endpoints() {
        let ele = |a: Option<f32>, b: Option<f32>| {
            let pts = vec![
                TrackPoint { lat: 0, lon: 0, ele: a, t: 0.0 },
                TrackPoint { lat: 10_000, lon: 0, ele: b, t: 10.0 },
            ];
            GpxPlayer::new(Track { points: pts }).elevation_at(5.0)
        };
        // Both present → interpolated (covered elsewhere, included for contrast).
        assert_eq!(ele(Some(200.0), Some(300.0)), Some(250.0));
        // One side missing → the present side's reading carries (no interpolation).
        assert_eq!(ele(Some(200.0), None), Some(200.0), "a lone leading elevation still reads");
        assert_eq!(ele(None, Some(300.0)), Some(300.0), "a lone trailing elevation still reads");
        // Neither side has elevation → no reading at all.
        assert_eq!(ele(None, None), None, "a gap with no elevation either side returns None");
    }

    /// Within `LOOK_AHEAD_S` of the end there's no forward window, so `course_speed` looks
    /// *behind* and reverses the endpoints so the bearing still points forward. On a due-east
    /// track the course at the last fix must read ~90°, not ~270° (a sign-flip).
    #[test]
    fn course_at_track_end_still_points_forward() {
        // Due east, three points over 10 s; sample the final fix (look-behind territory).
        let p = GpxPlayer::new(track(&[(45_000_000, 0, 0.0), (45_000_000, 5_000, 5.0), (45_000_000, 10_000, 10.0)]));
        let end = p.fix_at(p.duration()).unwrap();
        let c = end.course.expect("moving east → has a course at the end");
        assert!((c - 90.0).abs() < 1.0, "look-behind must keep east ~90° at the finish, got {c} (a flip → ~270°)");
    }
}
