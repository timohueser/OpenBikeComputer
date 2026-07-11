//! "Effort follows speed" — a plausible HR / power / cadence synth from ground speed.
//!
//! A shared host helper so the two feeders — the in-process simulator ([`obc-sim`]) and the USB
//! feeder ([`obc-usb-host`]) — synthesize the **same** curves from a replayed GPX's derived speed.
//! It is deliberately not physical: the numbers only have to look like a real ride so a recorded
//! replay lays down lifelike sensor tracks without anyone babysitting three sliders. Light,
//! deterministic wobble keyed on a per-sample `phase` counter keeps the curves from reading as flat
//! synthetic lines.
//!
//! [`obc-sim`]: https://docs.rs/obc-sim
//! [`obc-usb-host`]: https://docs.rs/obc-usb-host

/// Below this ground speed (m/s) the rider is treated as stopped: cadence and power fall to `0`
/// (coasting / at a light), HR eases toward its resting floor. Matches the `GpxPlayer`'s own
/// moving threshold so "stopped" means the same thing to the synth as to the fix.
const STOPPED_MPS: f32 = 0.5;

/// The synthesized sensor triple for one sample: heart rate (bpm), power (W), cadence (rpm) — the
/// exact types the `HeartRateSource` / `PowerSource` / `CadenceSource` traits (and the `H`/`P`/`R`
/// debug-link lines) carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Effort {
    pub hr_bpm: u16,
    pub power_w: u16,
    pub cadence_rpm: u8,
}

/// Synthesize a plausible [`Effort`] from ground speed (m/s), with light deterministic wobble keyed
/// on `phase` (increment it once per emitted sample). Curves:
/// - **HR** eases from a ~95 bpm easy-spin floor up with speed (≈ `95 + 2.2·km/h`), clamped to a
///   sane 40–195 bpm band; a stop relaxes it toward the floor rather than to zero.
/// - **Power** rises roughly with the square of speed (aero-dominated, `≈ 0.28·(km/h)²`), clamped
///   0–1000 W; zero when stopped (freewheeling).
/// - **Cadence** sits in a steady spin band while moving (`≈ 78 + 0.4·km/h`, clamped ≤ 120 rpm);
///   zero when stopped (feet still), which the app records as a fresh coasting `0`, distinct from
///   "no sensor".
pub fn effort_from_speed(speed_mps: f32, phase: u32) -> Effort {
    let kmh = (speed_mps.max(0.0)) * 3.6;
    let moving = speed_mps >= STOPPED_MPS;

    // Small, bounded, deterministic wobble in `-1.0..=1.0` from a cheap integer hash of `phase`, so
    // successive samples breathe a little instead of tracing a ruler-straight line.
    let w = wobble(phase);

    let hr = (95.0 + 2.2 * kmh + 3.0 * w).round().clamp(40.0, 195.0) as u16;
    let power = if moving { (0.28 * kmh * kmh + 8.0 * w).round().clamp(0.0, 1000.0) as u16 } else { 0 };
    let cadence = if moving { (78.0 + 0.4 * kmh + 2.0 * w).round().clamp(0.0, 120.0) as u8 } else { 0 };

    Effort { hr_bpm: hr, power_w: power, cadence_rpm: cadence }
}

/// A deterministic pseudo-random wobble in `-1.0..=1.0` from a sample counter — a small integer
/// hash (a Weyl-style multiply + xorshift) folded to a float. Cheap, `no_std`-friendly, and
/// repeatable so a headless replay is byte-stable.
fn wobble(phase: u32) -> f32 {
    let mut x = phase.wrapping_mul(2_654_435_761);
    x ^= x >> 15;
    x = x.wrapping_mul(2_246_822_519);
    x ^= x >> 13;
    // Top 16 bits → [0, 1) → [-1, 1).
    (x >> 16) as f32 / 32_768.0 - 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stopped_zeroes_power_and_cadence() {
        let e = effort_from_speed(0.0, 0);
        assert_eq!(e.power_w, 0, "no power when freewheeling / stopped");
        assert_eq!(e.cadence_rpm, 0, "feet still → cadence 0");
        assert!((40..=110).contains(&e.hr_bpm), "HR relaxes toward the floor, not to zero: {}", e.hr_bpm);
    }

    #[test]
    fn faster_means_more_effort() {
        // Compare well-separated speeds at the same wobble phase so the trend, not the jitter, shows.
        let slow = effort_from_speed(3.0, 0); // ~11 km/h
        let fast = effort_from_speed(11.0, 0); // ~40 km/h
        assert!(fast.hr_bpm > slow.hr_bpm, "HR rises with speed");
        assert!(fast.power_w > slow.power_w, "power rises with speed");
        assert!(fast.cadence_rpm >= slow.cadence_rpm, "cadence rises (or holds) with speed");
    }

    #[test]
    fn values_stay_in_sensor_ranges() {
        // Sweep speed + phase; every triple must stay inside the trait/slider ranges.
        for phase in 0..64 {
            for kmh in 0..=80 {
                let e = effort_from_speed(kmh as f32 / 3.6, phase);
                assert!((40..=220).contains(&e.hr_bpm), "hr {} out of range", e.hr_bpm);
                assert!(e.power_w <= 1000, "power {} out of range", e.power_w);
                assert!(e.cadence_rpm <= 130, "cadence {} out of range", e.cadence_rpm);
            }
        }
    }

    #[test]
    fn wobble_is_deterministic_and_bounded() {
        for phase in 0..1000 {
            let w = wobble(phase);
            assert!((-1.0..=1.0).contains(&w), "wobble {w} out of [-1,1] at phase {phase}");
            assert_eq!(w, wobble(phase), "wobble is a pure function of phase");
        }
    }
}
