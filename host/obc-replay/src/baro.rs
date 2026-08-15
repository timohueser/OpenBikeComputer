//! Simulated barometric altimeter — a **simulator-only** stand-in for the device's
//! pressure sensor.
//!
//! On the device this is a barometer (e.g. a BMP390) on its own I2C bus, sampled
//! independently of the GPS. Here it is fed the elevation interpolated from the replayed
//! GPX track, but emits on the sensor's *own* cadence ([`SAMPLE_INTERVAL_S`] of playback
//! time), **not** locked to the per-frame GPS fixes. That keeps the simulator honest about
//! the real hardware: a baro sample and a GPS fix do not arrive together, so the app must
//! integrate climb from this asynchronous stream rather than assuming one reading per fix.
//!
//! The app reads it through the shared [`AltimeterSource`] trait, exactly as it will read
//! the real barometer driver — it never learns this one is backed by a GPX file.

use obc_ports::AltimeterSource;

/// Emit a fresh reading at most this often (seconds of playback time). Coarser than and
/// unaligned with the GPS fixes, modelling a barometer polled on its own schedule.
const SAMPLE_INTERVAL_S: f64 = 0.5;

/// A barometer fed from the GPX replay. The host calls [`feed`](BaroSensor::feed) each
/// frame with the track's current elevation + playback time; [`poll`](AltimeterSource::poll)
/// returns a value only when [`SAMPLE_INTERVAL_S`] has elapsed since the last emission.
#[derive(Debug, Default)]
pub struct BaroSensor {
    /// Latest elevation fed from the track (m), if any — already carrying
    /// [`drift_m_per_h`](BaroSensor::drift_m_per_h).
    current: Option<f32>,
    /// Playback time of the most recent `feed`.
    fed_t: f64,
    /// Playback time at the last emitted sample; a new sample is "due" once
    /// `fed_t - emitted_t >= SAMPLE_INTERVAL_S`.
    emitted_t: f64,
    due: bool,
    /// Synthetic **weather drift** (m of apparent altitude per hour of playback time), added to
    /// every fed elevation — see [`set_drift`](BaroSensor::set_drift). `0.0` = today's behaviour.
    drift_m_per_h: f32,
}

impl BaroSensor {
    pub fn new() -> Self {
        BaroSensor { current: None, fed_t: 0.0, emitted_t: f64::NEG_INFINITY, due: false, drift_m_per_h: 0.0 }
    }

    /// Inject a synthetic **barometric weather drift**: the emitted altitude walks away from the
    /// track's true elevation by `m_per_h` metres per hour of playback time (negative = pressure
    /// rising, the sensor under-reading).
    ///
    /// This is the simulator's stand-in for the one error the device's altimeter genuinely has and
    /// a GPX replay otherwise cannot show: `bmp581.rs` hard-codes sea-level `P0`, so a passing front
    /// moves every reading together. Real weather is on the order of 1 hPa/h ≈ 8 m/h; the map-
    /// referenced altimeter (epic #1068, EL8) exists to cancel exactly this, so this knob is how
    /// its cancellation is demonstrated and regression-tested. `0.0` restores the plain replay.
    pub fn set_drift(&mut self, m_per_h: f32) {
        self.drift_m_per_h = m_per_h;
    }

    /// Feed the track's elevation `ele_m` at playback time `t`. Marks a sample due once a
    /// sample interval of playback time has passed since the last emission (so the emitted
    /// stream is coarser than, and out of phase with, the per-frame fixes). A backward
    /// jump in `t` (seek / replay restart) re-arms immediately.
    pub fn feed(&mut self, ele_m: Option<f32>, t: f64) {
        // The drift is applied at feed time, so the emitted sample is what a drifting sensor would
        // actually have reported at this playback instant.
        self.current = ele_m.map(|e| e + self.drift_m_per_h * (t as f32) / 3600.0);
        self.fed_t = t;
        if t < self.emitted_t {
            self.emitted_t = f64::NEG_INFINITY;
        }
        if ele_m.is_some() && t - self.emitted_t >= SAMPLE_INTERVAL_S {
            self.due = true;
        }
    }

    /// Drop any pending reading — call when the replay is ejected so the manual panel
    /// (which has no barometer) doesn't keep feeding climb.
    pub fn clear(&mut self) {
        self.current = None;
        self.due = false;
        self.emitted_t = f64::NEG_INFINITY;
    }
}

impl AltimeterSource for BaroSensor {
    fn poll(&mut self) -> Option<f32> {
        if self.due {
            self.due = false;
            self.emitted_t = self.fed_t;
            self.current
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_on_its_own_cadence_not_every_feed() {
        let mut b = BaroSensor::new();
        // First feed at t=0 → due immediately (emitted_t starts at -inf).
        b.feed(Some(200.0), 0.0);
        assert_eq!(b.poll(), Some(200.0));
        // A feed only 0.1 s later is within the interval → no new sample.
        b.feed(Some(201.0), 0.1);
        assert_eq!(b.poll(), None);
        // Past the 0.5 s interval → a fresh sample, at the latest fed value.
        b.feed(Some(205.0), 0.6);
        assert_eq!(b.poll(), Some(205.0));
    }

    #[test]
    fn no_elevation_means_no_sample() {
        let mut b = BaroSensor::new();
        b.feed(None, 0.0);
        assert_eq!(b.poll(), None);
    }

    /// The EL8 drift injector: the emitted altitude walks away from the track's true elevation at
    /// the configured rate, and the plain replay (drift 0) is unchanged.
    #[test]
    fn injected_drift_walks_the_emitted_altitude_away() {
        let mut b = BaroSensor::new();
        b.set_drift(8.0); // 8 m/h ≈ 1 hPa/h, a realistic passing front
        b.feed(Some(500.0), 0.0);
        assert_eq!(b.poll(), Some(500.0), "no drift has accrued at t = 0");
        b.feed(Some(500.0), 3600.0);
        assert_eq!(b.poll(), Some(508.0), "one hour later the same ground reads 8 m higher");
        b.feed(Some(600.0), 7200.0);
        assert_eq!(b.poll(), Some(616.0), "real climbing and drift add");

        let mut plain = BaroSensor::new();
        plain.feed(Some(500.0), 7200.0);
        assert_eq!(plain.poll(), Some(500.0), "drift 0 is the pre-EL8 replay, bit for bit");
    }

    #[test]
    fn seek_backward_re_arms() {
        let mut b = BaroSensor::new();
        b.feed(Some(200.0), 5.0);
        assert_eq!(b.poll(), Some(200.0));
        // Replay restarts (t jumps back) → the next feed is due again.
        b.feed(Some(210.0), 0.0);
        assert_eq!(b.poll(), Some(210.0));
    }
}
