//! The simulator's synthetic BLE sensor sources: heart rate, power and cadence.
//!
//! [`SimHeartRate`], [`SimPower`] and [`SimCadence`] implement the HAL sensor traits, so
//! [`obc_app::App`] cannot tell them from a real strap. Each honours the fresh-mailbox contract: a
//! value is emitted about once per second of ride-clock time and `None` between polls. A source
//! that returned a value on every frame would defeat the app's staleness gate.
//!
//! The control panel edits a [`SensorConfig`]. [`feed`] is called once per frame with the ride clock
//! and the current speed; it resolves each quantity's target and hands it to the gate. Disabling a
//! quantity mid-ride feeds `None`, so it goes stale on the tiles and is absent from the log.
//!
//! [`feed`]: SimSensors::feed

use obc_ports::{CadenceSource, HeartRateSource, PowerSource};

/// Emit at most one sample per this many milliseconds of ride-clock time, which is playback time
/// under a GPX replay and wall clock under manual control. It is the cadence a real BLE sensor
/// notifies at, and stays inside the app's staleness window at every replay speed.
const EMIT_MS: u32 = 1000;

/// The shared fresh-mailbox gate, one per quantity. [`feed`](Self::feed) sets the pending value,
/// or `None` to go quiet, and [`take`](Self::take) yields it at most once per [`EMIT_MS`].
struct Emitter<T> {
    /// The value most recently fed. `None` means the quantity is off or stale, so nothing is
    /// emitted.
    current: Option<T>,
    /// Ride-clock ms of the most recent `feed`.
    fed_ms: u32,
    /// Ride-clock ms at the last emitted sample. `None` forces the next enabled feed to emit.
    emitted_ms: Option<u32>,
    /// A sample is due: armed by `feed`, disarmed by `take`.
    due: bool,
}

impl<T: Copy> Emitter<T> {
    fn new() -> Self {
        Emitter { current: None, fed_ms: 0, emitted_ms: None, due: false }
    }

    /// Feed this frame's value at ride-clock `now_ms`. It arms a sample once a full [`EMIT_MS`]
    /// has elapsed since the last emission, and a backward jump in `now_ms`, such as a replay seek,
    /// re-arms immediately. A `None` feed disarms, so the source goes stale.
    fn feed(&mut self, v: Option<T>, now_ms: u32) {
        self.current = v;
        self.fed_ms = now_ms;
        if self.emitted_ms.is_some_and(|e| now_ms < e) {
            self.emitted_ms = None;
        }
        match v {
            Some(_) if self.emitted_ms.is_none_or(|e| now_ms.wrapping_sub(e) >= EMIT_MS) => self.due = true,
            None => self.due = false,
            _ => {}
        }
    }

    /// Yield the pending value at most once per [`EMIT_MS`]. It is the `poll` body of each source
    /// trait.
    fn take(&mut self) -> Option<T> {
        if self.due {
            self.due = false;
            self.emitted_ms = Some(self.fed_ms);
            self.current
        } else {
            None
        }
    }
}

/// The control-panel state driving the synthetic sensors: a per-quantity enable and fixed slider
/// value, plus the effort-follows-speed switch, which synthesizes all three from the replayed speed
/// and ignores the individual toggles.
#[derive(Debug, Clone, Copy)]
pub struct SensorConfig {
    pub hr_enabled: bool,
    pub power_enabled: bool,
    pub cadence_enabled: bool,
    /// Slider values, used when effort-follows-speed is off.
    pub hr_bpm: u16,
    pub power_w: u16,
    pub cadence_rpm: u8,
    /// Synthesize all three from the replayed GPX speed, with light noise.
    pub effort_follows_speed: bool,
}

impl Default for SensorConfig {
    fn default() -> Self {
        // All off at boot, with the sliders seeded mid-range so enabling one lands on a believable
        // value.
        SensorConfig {
            hr_enabled: false,
            power_enabled: false,
            cadence_enabled: false,
            hr_bpm: 140,
            power_w: 200,
            cadence_rpm: 85,
            effort_follows_speed: false,
        }
    }
}

/// The three synthetic sensor sources plus their shared [`SensorConfig`]. The GUI holds it, hands
/// the three source fields to `Sensors` each tick as disjoint borrows, and calls
/// [`feed`](Self::feed) once per frame to advance the gates.
#[derive(Default)]
pub struct SimSensors {
    pub hr: SimHeartRate,
    pub power: SimPower,
    pub cadence: SimCadence,
    pub cfg: SensorConfig,
}

impl SimSensors {
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolve each quantity's target from the config, synthesizing from `speed_mps` when
    /// effort-follows-speed is set, and feed the gates at ride-clock `now_ms`. Call it once per
    /// frame, before building the `Sensors` for the tick.
    pub fn feed(&mut self, now_ms: u32, speed_mps: f32) {
        if self.cfg.effort_follows_speed {
            // A light deterministic wobble keyed on the ride second, so a replayed ride records
            // lifelike curves rather than three flat lines.
            let e = obc_replay::effort_from_speed(speed_mps, now_ms / 1000);
            self.hr.0.feed(Some(e.hr_bpm), now_ms);
            self.power.0.feed(Some(e.power_w), now_ms);
            self.cadence.0.feed(Some(e.cadence_rpm), now_ms);
        } else {
            self.hr.0.feed(self.cfg.hr_enabled.then_some(self.cfg.hr_bpm), now_ms);
            self.power.0.feed(self.cfg.power_enabled.then_some(self.cfg.power_w), now_ms);
            self.cadence.0.feed(self.cfg.cadence_enabled.then_some(self.cfg.cadence_rpm), now_ms);
        }
    }
}

/// The headless `--sensors demo` effort at replay second `t_s`, as `(bpm, watts)`: power swings
/// from an easy spin past threshold and back every few minutes, with short surges on top, and heart
/// rate follows it half a minute late. Against the demo limits (185 bpm, 250 W) it reaches every
/// zone within five minutes.
pub fn demo_effort(t_s: f64) -> (u16, u16) {
    let watts = 190.0 + 70.0 * (t_s / 40.0).sin() + 40.0 * (t_s / 13.0).sin();
    let bpm = 150.0 + 25.0 * ((t_s - 30.0) / 40.0).sin();
    (bpm as u16, watts.max(0.0) as u16)
}

/// Synthetic heart rate, driven by the panel. Hand `&mut SimHeartRate` to `Sensors::hr`.
#[derive(Default)]
pub struct SimHeartRate(Emitter<u16>);
impl HeartRateSource for SimHeartRate {
    fn poll(&mut self) -> Option<u16> {
        self.0.take()
    }
}

/// Synthetic power, driven by the panel. Hand `&mut SimPower` to `Sensors::power`.
#[derive(Default)]
pub struct SimPower(Emitter<u16>);
impl PowerSource for SimPower {
    fn poll(&mut self) -> Option<u16> {
        self.0.take()
    }
}

/// Synthetic cadence, driven by the panel. Hand `&mut SimCadence` to `Sensors::cadence`.
#[derive(Default)]
pub struct SimCadence(Emitter<u8>);
impl CadenceSource for SimCadence {
    fn poll(&mut self) -> Option<u8> {
        self.0.take()
    }
}

impl<T: Copy> Default for Emitter<T> {
    fn default() -> Self {
        Emitter::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_at_one_hz_not_every_frame() {
        let mut s = SimSensors::new();
        s.cfg.hr_enabled = true;
        s.cfg.hr_bpm = 150;
        // The first feed emits promptly, because `emitted_ms` starts empty.
        s.feed(0, 0.0);
        assert_eq!(s.hr.poll(), Some(150), "first sample emits at once");
        // Frames a few milliseconds apart within the second emit nothing between ticks.
        for t in (8..1000).step_by(8) {
            s.feed(t, 0.0);
            assert_eq!(s.hr.poll(), None, "no value between 1 Hz ticks (t={t})");
        }
        // Past the cadence, a fresh sample is emitted.
        s.feed(1000, 0.0);
        assert_eq!(s.hr.poll(), Some(150), "a new sample at the next second");
    }

    #[test]
    fn disabling_mid_ride_goes_stale() {
        let mut s = SimSensors::new();
        s.cfg.power_enabled = true;
        s.cfg.power_w = 250;
        s.feed(0, 0.0);
        assert_eq!(s.power.poll(), Some(250));
        // Toggled off mid-ride, the source feeds `None` and emits nothing.
        s.cfg.power_enabled = false;
        s.feed(1000, 0.0);
        assert_eq!(s.power.poll(), None, "disabled → no sample (goes stale on the app's 5 s gate)");
        s.feed(2000, 0.0);
        assert_eq!(s.power.poll(), None, "stays quiet while disabled");
        // Re-enabled, it emits promptly again.
        s.cfg.power_enabled = true;
        s.feed(3000, 0.0);
        assert_eq!(s.power.poll(), Some(250), "re-enabling resumes the stream");
    }

    #[test]
    fn effort_follows_speed_drives_all_three() {
        let mut s = SimSensors::new();
        s.cfg.effort_follows_speed = true; // individual toggles ignored
        s.feed(0, 11.0); // ~40 km/h
        let hr = s.hr.poll().expect("hr synthesized from speed");
        let pw = s.power.poll().expect("power synthesized from speed");
        let cad = s.cadence.poll().expect("cadence synthesized from speed");
        assert!((40..=220).contains(&hr));
        assert!(pw <= 1000 && pw > 0, "moving → some power ({pw})");
        assert!(cad <= 130 && cad > 0, "moving → some cadence ({cad})");
    }

    #[test]
    fn seek_backward_re_arms() {
        let mut s = SimSensors::new();
        s.cfg.cadence_enabled = true;
        s.cfg.cadence_rpm = 90;
        s.feed(5000, 0.0);
        assert_eq!(s.cadence.poll(), Some(90));
        // A replay seek jumps the clock back, so the next feed emits at once.
        s.feed(1000, 0.0);
        assert_eq!(s.cadence.poll(), Some(90), "a backward clock jump re-arms immediately");
    }
}
