//! A board-agnostic synthetic moving [`LocationSource`] — the **`debug-link`-off fallback** fake
//! GPS, lifted out of the board crate so every OBC board reuses one copy.
//!
//! A board with no real receiver but a need to exercise the ride loop drives [`SynthLocation`] in
//! place of a chip: the fix walks a slow square loop around a centre, on the wall clock. Unlike a
//! constant fix, this gives the ride accumulators, breadcrumb and `.obct` log real motion — so a
//! saved ride is a non-degenerate `.gpx` that re-imports cleanly. Always-compiled (NOT behind
//! `debug-link`) because it *is* the debug-link-off path.

use embassy_time::Instant;
use obc_ports::{Fix, LocationSource};

/// Side length (m) and speed (m/s) of the square loop [`SynthLocation`] walks. Slow enough to watch
/// the user marker / breadcrumb crawl, big enough that a saved ride is a real ~0.8 km loop.
const SYNTH_LEG_M: f32 = 200.0;
const SYNTH_SPEED_MPS: f32 = 5.0;

/// The synthetic GPS emits a fresh fix at this cadence (ms), `None` between — the same ~1 Hz
/// fresh-fix contract a real receiver honours, exercising the integrate-one-sample path.
const SYNTH_FIX_INTERVAL_MS: u64 = 1000;

/// Microdegrees of latitude per metre north (the map/route coordinate convention). Longitude scales
/// this by 1/cos(lat), via [`obc_map_scene::cos_lat`].
const UDEG_PER_M: f32 = 1_000_000.0 / 111_320.0;

/// A stand-in moving [`LocationSource`] for the **`debug-link`-off** build: the fix walks a slow
/// square loop around a centre on the wall clock, so a saved ride is a non-degenerate `.gpx`. The
/// centre is the map (or loaded route's) start, re-pointed via [`recenter`](Self::recenter).
pub struct SynthLocation {
    center_lon: i32,
    center_lat: i32,
    /// 1/cos(lat) folded into the east-metres → microdegrees scale, refreshed on recenter.
    udeg_per_m_east: f32,
    start: Instant,
    /// Elapsed-millis at the last fix [`poll`](LocationSource::poll) emitted, to throttle to
    /// [`SYNTH_FIX_INTERVAL_MS`]. `None` forces the first poll to emit.
    last_fix_ms: Option<u64>,
}

impl SynthLocation {
    pub fn new(center_lon: i32, center_lat: i32, start: Instant) -> Self {
        let mut s = SynthLocation { center_lon, center_lat, udeg_per_m_east: 0.0, start, last_fix_ms: None };
        s.recenter(center_lon, center_lat);
        s
    }

    /// Move the loop's centre (e.g. onto a freshly-loaded route's start) and refresh the
    /// longitude scale for the new latitude.
    pub fn recenter(&mut self, lon: i32, lat: i32) {
        self.center_lon = lon;
        self.center_lat = lat;
        self.udeg_per_m_east = UDEG_PER_M / obc_map_scene::cos_lat(lat);
    }
}

impl LocationSource for SynthLocation {
    fn poll(&mut self) -> Option<Fix> {
        // `saturating_duration_since`, not `Instant::elapsed()`: embassy's `elapsed()` `unwrap!`s a
        // `checked_sub`, so it panics → HardFault if `now()` momentarily reads *before* `start` (a
        // known embassy time-driver race when a narrow hardware timer is extended to 64-bit ticks).
        // A transient backwards read meaning "zero time passed" is harmless here, so clamp it.
        let elapsed_ms = Instant::now().saturating_duration_since(self.start).as_millis();
        if let Some(last) = self.last_fix_ms {
            if elapsed_ms.wrapping_sub(last) < SYNTH_FIX_INTERVAL_MS {
                return None;
            }
        }
        self.last_fix_ms = Some(elapsed_ms);

        // Position along the square vs. elapsed time; the heading is each leg's constant bearing.
        // Take the loop modulus on the integer millis *before* the `f32` cast: `as_millis()` grows
        // unbounded and `f32` carries only a 24-bit mantissa, so casting first would quantise the
        // phase (jitter, then freeze) past ~4.6 h of uptime.
        let leg_s = SYNTH_LEG_M / SYNTH_SPEED_MPS;
        let loop_ms = (4.0 * leg_s * 1000.0) as u64;
        let t = (elapsed_ms % loop_ms) as f32 / 1000.0;
        let leg = (t / leg_s) as u32;
        let d = (t - leg as f32 * leg_s) * SYNTH_SPEED_MPS; // metres into this leg
        let (east, north, course) = match leg {
            0 => (d, 0.0, 90.0),                        // →E along the south edge
            1 => (SYNTH_LEG_M, d, 0.0),                 // →N up the east edge
            2 => (SYNTH_LEG_M - d, SYNTH_LEG_M, 270.0), // →W along the north edge
            _ => (0.0, SYNTH_LEG_M - d, 180.0),         // →S down the west edge
        };
        let east = east - SYNTH_LEG_M / 2.0; // centre the square on the centre point
        let north = north - SYNTH_LEG_M / 2.0;
        Some(Fix {
            lon: self.center_lon + (east * self.udeg_per_m_east) as i32,
            lat: self.center_lat + (north * UDEG_PER_M) as i32,
            course: Some(course),
            speed_mps: Some(SYNTH_SPEED_MPS),
        })
    }
}
