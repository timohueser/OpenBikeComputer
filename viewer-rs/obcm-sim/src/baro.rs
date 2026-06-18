//! Simulated barometric altimeter — a **simulator-only** stand-in for the device's
//! pressure sensor.
//!
//! On the device this is a barometer (e.g. a BMP390) on its own I2C bus, sampled
//! independently of the GPS. Here we feed it the elevation interpolated from the replayed
//! GPX track — but emit on the sensor's *own* cadence ([`SAMPLE_INTERVAL_S`] of playback
//! time), **not** locked to the per-frame GPS fixes. That keeps the simulator honest about
//! the real hardware: a baro sample and a GPS fix do not arrive together, so the app must
//! integrate climb from this asynchronous stream rather than assuming one reading per fix.
//!
//! The app reads it through the shared [`ElevationSource`] trait, exactly as it will read
//! the real barometer driver — it never learns this one is backed by a GPX file.

use obcm_app::ElevationSource;

/// Emit a fresh reading at most this often (seconds of playback time). Coarser than and
/// unaligned with the GPS fixes, modelling a barometer polled on its own schedule.
const SAMPLE_INTERVAL_S: f64 = 0.5;

/// A barometer fed from the GPX replay. The host calls [`feed`](BaroSensor::feed) each
/// frame with the track's current elevation + playback time; [`poll`](ElevationSource::poll)
/// returns a value only when [`SAMPLE_INTERVAL_S`] has elapsed since the last emission.
#[derive(Debug, Default)]
pub struct BaroSensor {
    /// Latest elevation fed from the track (m), if any.
    current: Option<f32>,
    /// Playback time of the most recent `feed`.
    fed_t: f64,
    /// Playback time at the last emitted sample; a new sample is "due" once
    /// `fed_t - emitted_t >= SAMPLE_INTERVAL_S`.
    emitted_t: f64,
    due: bool,
}

impl BaroSensor {
    pub fn new() -> Self {
        BaroSensor { current: None, fed_t: 0.0, emitted_t: f64::NEG_INFINITY, due: false }
    }

    /// Feed the track's elevation `ele_m` at playback time `t`. Marks a sample due once a
    /// sample interval of playback time has passed since the last emission (so the emitted
    /// stream is coarser than, and out of phase with, the per-frame fixes). A backward
    /// jump in `t` (seek / replay restart) re-arms immediately.
    pub fn feed(&mut self, ele_m: Option<f32>, t: f64) {
        self.current = ele_m;
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

impl ElevationSource for BaroSensor {
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
