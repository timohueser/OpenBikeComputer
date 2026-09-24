//! The cue-to-pattern table: the one table every [`Sounder`](obc_ports::Sounder) plays, on the
//! board, in the simulator and on the phone.
//!
//! Cues in the same [`Family`] share one pattern, because a rider learns about five sounds
//! reliably. Rhythm carries the meaning and stays audible in wind; pitch direction is the second
//! signal. The two pitches sit about a fourth apart inside the piezo's 2.7 to 4 kHz band.
//!
//! These values are a start. The owner tunes them by ear on the real part.

use obc_ports::{Cue, Family, Note};

const LOW: u16 = 3_000;
const HIGH: u16 = 4_000;

const fn tone(hz: u16, ms: u16) -> Note {
    Note { hz, ms }
}

const fn rest(ms: u16) -> Note {
    Note { hz: 0, ms }
}

/// One very short click: "got it".
const TICK: &[Note] = &[tone(HIGH, 12)];
/// Two short notes, rising: "look at the screen soon".
const HEADS_UP: &[Note] = &[tone(LOW, 100), rest(60), tone(HIGH, 100)];
/// Three quick notes, rising: "resolved".
const GOOD: &[Note] = &[tone(LOW, 70), rest(40), tone(LOW, 70), rest(40), tone(HIGH, 140)];
/// Two longer notes, falling: "something went wrong".
const PROBLEM: &[Note] = &[tone(HIGH, 250), rest(80), tone(LOW, 400)];
/// Three sharp beeps, played twice: "act now".
const URGENT: &[Note] = &[
    tone(HIGH, 80),
    rest(60),
    tone(HIGH, 80),
    rest(60),
    tone(HIGH, 80),
    rest(60),
    rest(400),
    tone(HIGH, 80),
    rest(60),
    tone(HIGH, 80),
    rest(60),
    tone(HIGH, 80),
    rest(60),
];

/// The notes `cue` plays.
pub fn pattern(cue: Cue) -> &'static [Note] {
    match cue.family() {
        Family::Tick => TICK,
        Family::HeadsUp => HEADS_UP,
        Family::Good => GOOD,
        Family::Problem => PROBLEM,
        Family::Urgent => URGENT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Cue; 14] = [
        Cue::KeyClick,
        Cue::HoldDone,
        Cue::ClimbStarts,
        Cue::BackOnRoute,
        Cue::SoundPreview,
        Cue::GpsBack,
        Cue::Arrived,
        Cue::OffRoute,
        Cue::GpsLost,
        Cue::SensorDropped,
        Cue::BatteryLow,
        Cue::RecordingError,
        Cue::StorageLost,
        Cue::BatteryCritical,
    ];

    /// Every cue sounds, no step has zero length, and a family is one pattern, not one per cue.
    #[test]
    fn every_cue_plays_its_familys_one_pattern() {
        for a in ALL {
            let notes = pattern(a);
            assert!(!notes.is_empty(), "{a:?} is silent");
            assert!(notes.iter().all(|n| n.ms > 0), "{a:?} has a zero-length step");
            for b in ALL.into_iter().filter(|b| b.family() == a.family()) {
                assert!(core::ptr::eq(notes, pattern(b)), "{a:?} and {b:?} share a family but not a pattern");
            }
        }
    }
}
