//! The rider-started alpine signal: six evenly spaced beeps, then one minute quiet.

use crate::device_core::Sound;
use obc_ports::{Cue, Volume};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Distress {
    since: Option<u32>,
    slot: Option<u32>,
}

impl Distress {
    pub(crate) const fn new() -> Self {
        Self { since: None, slot: None }
    }

    pub(crate) fn update(&mut self, active: bool, now: u32) -> Option<Sound> {
        if !active {
            let stopped = self.since.take().is_some();
            self.slot = None;
            return stopped.then_some(Sound::Stop);
        }
        let since = *self.since.get_or_insert(now);
        let elapsed = now.wrapping_sub(since);
        let slot = elapsed / 10_000;
        // A late pass plays at most one beep. It never catches up with a burst.
        if slot % 12 < 6 && self.slot != Some(slot) {
            self.slot = Some(slot);
            Some(Sound::Play { cue: Cue::Distress, volume: Volume::Loud })
        } else {
            None
        }
    }

    pub(crate) fn wake_in(&self, now: u32) -> Option<u32> {
        let phase = now.wrapping_sub(self.since?) % 120_000;
        Some(if phase < 50_000 { 10_000 - phase % 10_000 } else { 120_000 - phase })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn six_beeps_then_a_quiet_minute_until_stopped() {
        for start in [0u32, u32::MAX - 25_000] {
            let mut signal = Distress::new();
            let mut heard = std::vec::Vec::new();
            for second in 0..=240 {
                if let Some(sound) = signal.update(true, start.wrapping_add(second * 1_000)) {
                    assert_eq!(sound, Sound::Play { cue: Cue::Distress, volume: Volume::Loud });
                    heard.push(second);
                }
            }
            assert_eq!(heard, [0, 10, 20, 30, 40, 50, 120, 130, 140, 150, 160, 170, 240]);
            assert_eq!(signal.update(false, start.wrapping_add(240_100)), Some(Sound::Stop));
            assert_eq!(signal.wake_in(start.wrapping_add(240_100)), None);
            assert_eq!(signal.update(false, 0), None);
        }
    }

    #[test]
    fn wakes_for_each_beep_and_never_replays_missed_beeps() {
        let mut signal = Distress::new();
        signal.update(true, 0);
        assert_eq!(signal.wake_in(1_000), Some(9_000));
        assert!(signal.update(true, 45_000).is_some());
        assert_eq!(signal.update(true, 45_001), None);
        assert_eq!(signal.wake_in(55_000), Some(65_000));
        assert_eq!(signal.update(true, 119_999), None);
        assert!(signal.update(true, 120_000).is_some());
    }
}
