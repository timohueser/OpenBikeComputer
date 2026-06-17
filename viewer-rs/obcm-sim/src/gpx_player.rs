//! GPX replay as a simulated GPS sensor.
//!
//! [`GpxPlayer`] is the simulator's stand-in for the device's GPS chip when
//! replaying a recorded [`Track`]: it implements [`LocationSource`] exactly like
//! [`SimLocationSource`](crate::sim_location::SimLocationSource), so the shared
//! [`App`](obcm_app::App) can't tell a replay from a live fix. This is why the
//! whole feature stays host-side — `obcm`/`obcm-app` never learn what a GPX file
//! is; they just consume [`Fix`]es.
//!
//! ## Fidelity
//! GPX stores only position + time, never course or speed, so we derive them the
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

use obcm_app::{Fix, LocationSource};

use crate::gpx::Track;

/// Earth radius (mean) in meters, for the haversine distance used to derive speed.
const EARTH_R_M: f64 = 6_371_000.0;

/// Below this ground speed the simulated receiver reports no course (`None`),
/// mirroring a real GPS that can't determine heading while stationary.
const MOVING_THRESHOLD_MPS: f32 = 0.5;

/// Seconds of look-ahead used to derive course/speed. A small window smooths the
/// per-point bearing jitter you'd otherwise get from dense, noisy track points.
const LOOK_AHEAD_S: f64 = 2.0;

/// Replays a parsed [`Track`] as a [`LocationSource`]. Holds the playback cursor
/// (`t`, seconds into the track), whether it's playing, and the speed multiplier.
pub struct GpxPlayer {
    track: Track,
    /// Current playback position, seconds from the start of the track.
    t: f64,
    playing: bool,
    /// Playback speed multiplier: `1.0` = real time, up to `10.0`.
    speed: f32,
}

impl GpxPlayer {
    /// Build a player for `track`, paused at the start at real-time (1×) speed.
    pub fn new(track: Track) -> Self {
        GpxPlayer { track, t: 0.0, playing: false, speed: 1.0 }
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
        Some(Fix {
            lat: lat.round() as i32,
            lon: lon.round() as i32,
            course,
            speed_mps: Some(speed),
        })
    }

    /// Linearly interpolate the position (microdegrees, unrounded) at time `t`.
    /// `t` is assumed already clamped to `[0, duration]`.
    fn interp_pos(&self, t: f64) -> (f64, f64) {
        let pts = &self.track.points;
        // `partition_point` gives the count of points at or before `t`; the
        // bracketing segment is therefore `[idx-1, idx]`. Times are sorted
        // ascending, so the predicate is monotone as required.
        let idx = pts.partition_point(|p| p.t <= t);
        if idx == 0 {
            return (pts[0].lat as f64, pts[0].lon as f64);
        }
        if idx >= pts.len() {
            let last = pts.last().unwrap();
            return (last.lat as f64, last.lon as f64);
        }
        let a = &pts[idx - 1];
        let b = &pts[idx];
        let span = b.t - a.t;
        let f = if span > 0.0 { (t - a.t) / span } else { 0.0 };
        (
            a.lat as f64 + (b.lat - a.lat) as f64 * f,
            a.lon as f64 + (b.lon - a.lon) as f64 * f,
        )
    }

    /// Derive `(course, speed)` at time `t` from motion over a short window.
    /// Looks `LOOK_AHEAD_S` ahead (or behind, near the end) and measures the
    /// displacement; below [`MOVING_THRESHOLD_MPS`] the course is `None`.
    fn course_speed(&self, t: f64) -> (Option<f32>, f32) {
        let dur = self.duration();
        let (lat0, lon0) = self.interp_pos(t);

        // Prefer a forward window; within `LOOK_AHEAD_S` of the end, look back so
        // the heading stays defined right up to the final fix.
        let (t1, behind) = if t + LOOK_AHEAD_S <= dur {
            (t + LOOK_AHEAD_S, false)
        } else {
            ((t - LOOK_AHEAD_S).max(0.0), true)
        };
        let dt = (t1 - t).abs();
        if dt <= 0.0 {
            return (None, 0.0);
        }
        let (lat1, lon1) = self.interp_pos(t1);

        // Order the endpoints along the direction of travel so the bearing points
        // the way the user is moving.
        let (from, to) = if behind {
            ((lat1, lon1), (lat0, lon0))
        } else {
            ((lat0, lon0), (lat1, lon1))
        };
        let dist = haversine_m(from.0, from.1, to.0, to.1);
        let speed = (dist / dt) as f32;
        let course = if speed >= MOVING_THRESHOLD_MPS {
            Some(bearing_deg(from.0, from.1, to.0, to.1) as f32)
        } else {
            None
        };
        (course, speed)
    }
}

impl LocationSource for GpxPlayer {
    fn poll(&mut self) -> Option<Fix> {
        self.fix_at(self.t)
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
        Track { points: pts.iter().map(|&(lat, lon, t)| TrackPoint { lat, lon, t }).collect() }
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
}
