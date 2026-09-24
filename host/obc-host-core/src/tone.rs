//! A sound pattern as PCM samples, for the hosts that play cues through a sound card.
//!
//! Mono square wave, so a host hears what the piezo plays. `Quiet` is 6 dB below `Loud`, the same
//! step as the board's one-pin against two-pin drive.

use obc_ports::{Note, Volume};

/// The attack and the release of each note, so a note edge does not click.
const RAMP_MS: u32 = 2;

/// `notes` at `volume`, `sample_rate` samples per second, in `-1.0..=1.0`.
pub fn render(notes: &[Note], volume: Volume, sample_rate: u32) -> Vec<f32> {
    let amplitude = match volume {
        Volume::Loud => 0.5,
        Volume::Quiet => 0.25,
    };
    let ramp = (RAMP_MS * sample_rate / 1000).max(1) as f32;
    let mut out = Vec::new();
    for note in notes {
        let len = u32::from(note.ms) * sample_rate / 1000;
        out.extend((0..len).map(|i| {
            if note.hz == 0 {
                return 0.0;
            }
            // Which half period sample `i` falls in: even halves are high, odd halves low.
            let half = u64::from(i) * u64::from(note.hz) * 2 / u64::from(sample_rate);
            let square = if half % 2 == 0 { 1.0 } else { -1.0 };
            let envelope = (i as f32 / ramp).min((len - 1 - i) as f32 / ramp).min(1.0);
            amplitude * envelope * square
        }));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pattern_renders_its_duration_with_silent_rests_soft_edges_and_a_6_db_step() {
        let rate = 48_000;
        let notes = [Note { hz: 3_000, ms: 20 }, Note { hz: 0, ms: 10 }, Note { hz: 4_000, ms: 20 }];
        let loud = render(&notes, Volume::Loud, rate);
        let quiet = render(&notes, Volume::Quiet, rate);

        let per_ms = rate as usize / 1000;
        assert_eq!(loud.len(), 50 * per_ms, "one sample per tick of the pattern's duration");

        let (first, rest, last) = (0..20 * per_ms, 20 * per_ms..30 * per_ms, 30 * per_ms..50 * per_ms);
        assert!(loud[rest].iter().all(|&s| s == 0.0), "a rest is silent");
        for note in [first, last] {
            assert!(loud[note.start].abs() < 0.01, "a note starts near zero");
            assert!(loud[note.end - 1].abs() < 0.01, "a note ends near zero");
        }

        let peak = |samples: &[f32]| samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert_eq!(peak(&loud), 0.5);
        assert_eq!(peak(&quiet), peak(&loud) / 2.0, "Quiet is half the amplitude of Loud");
    }
}
