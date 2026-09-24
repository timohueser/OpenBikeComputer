//! Which cue the rider hears. Events that are edges already raise their cue directly. The levels
//! the app already has become edges here, after a settle time and with a cooldown, so a level that
//! flaps plays at most one loss cue and one recovery cue a minute.

use obc_ports::{Cue, Volume};

use crate::activity::Mode;
use crate::device_core::Sound;
use crate::device_status::LOW_BATTERY_PCT;
use crate::sensors::{SensorPhase, SensorStatus};
use crate::App;

/// After a loss cue, the same source raises no loss cue for this long. A loss that holds past the
/// cooldown plays when it ends.
const LOSS_COOLDOWN_MS: u32 = 60_000;
const BATTERY_CRITICAL_PCT: u8 = 5;
/// A battery threshold re-arms when the charge rises this many points above it.
const BATTERY_REARM_PCT: u8 = 5;

/// A level that is lost and comes back.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Source {
    OffRoute,
    Gps,
    /// Any sensor that connected during the ride and is not connected now.
    Sensor,
}

impl Source {
    const ALL: [Source; 3] = [Source::OffRoute, Source::Gps, Source::Sensor];

    fn loss(self) -> Cue {
        match self {
            Source::OffRoute => Cue::OffRoute,
            Source::Gps => Cue::GpsLost,
            Source::Sensor => Cue::SensorDropped,
        }
    }

    fn recovery(self) -> Option<Cue> {
        match self {
            Source::OffRoute => Some(Cue::BackOnRoute),
            Source::Gps => Some(Cue::GpsBack),
            Source::Sensor => None,
        }
    }

    /// How long a new level must hold before it counts. The live-fix window already waits at least
    /// 5 s, so `GpsLost` plays at least 15 s after the last fix.
    fn settle_ms(self, lost: bool) -> u32 {
        match (self, lost) {
            (Source::Gps | Source::Sensor, true) => 10_000,
            _ => 5_000,
        }
    }
}

/// A settled loss always played its cue, so a settled recovery may play its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Level {
    /// The level that last settled.
    lost: bool,
    /// When the reported level last moved off `lost`, while it stays off it.
    since: Option<u32>,
    /// When the last loss cue played.
    loss_at: Option<u32>,
}

impl Level {
    const IDLE: Level = Level { lost: false, since: None, loss_at: None };

    /// Millis until the reported level settles, or `None` when it is the settled one. A loss
    /// waits for its settle time and for the cooldown of the loss before it.
    fn due_in(&self, source: Source, now_ms: u32) -> Option<u32> {
        let settle = source.settle_ms(!self.lost).saturating_sub(now_ms.wrapping_sub(self.since?));
        let cooldown = match (self.lost, self.loss_at) {
            (false, Some(at)) => LOSS_COOLDOWN_MS.saturating_sub(now_ms.wrapping_sub(at)),
            _ => 0,
        };
        Some(settle.max(cooldown))
    }
}

/// The cue state. It lives in [`App`](crate::App), not in the pass state.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Cues {
    /// The cue to play at the next plan.
    raised: Option<Cue>,
    levels: [Level; 3],
    /// Whether the GPS had a fix since riding last started, so it has something to lose.
    gps_seen: bool,
    /// Slots whose sensor connected since riding last started.
    sensors_seen: u8,
    /// Whether `BatteryLow` and `BatteryCritical` may play.
    battery_armed: [bool; 2],
    arrived: bool,
}

impl Cues {
    pub(crate) const fn new() -> Self {
        Cues {
            raised: None,
            levels: [Level::IDLE; 3],
            gps_seen: false,
            sensors_seen: 0,
            battery_armed: [true; 2],
            arrived: false,
        }
    }

    /// Raise `cue` for the next plan. The higher family wins; in one family the first raised wins.
    pub(crate) fn raise(&mut self, cue: Cue) {
        if self.raised.is_none_or(|r| cue.family() > r.family()) {
            self.raised = Some(cue);
        }
    }

    /// Report a source's level: `Some(lost)` while its gate is open. `None` closes the gate and
    /// forgets the current loss, so no cue plays for it.
    pub(crate) fn level(&mut self, source: Source, lost: Option<bool>, now_ms: u32) {
        let l = &mut self.levels[source as usize];
        let Some(lost) = lost else {
            *l = Level { loss_at: l.loss_at, ..Level::IDLE };
            return;
        };
        if lost == l.lost {
            l.since = None;
            return;
        }
        l.since.get_or_insert(now_ms);
        if l.due_in(source, now_ms) != Some(0) {
            return;
        }
        l.lost = lost;
        l.since = None;
        let cue = if lost {
            l.loss_at = Some(now_ms);
            Some(source.loss())
        } else {
            source.recovery()
        };
        if let Some(cue) = cue {
            self.raise(cue);
        }
    }

    /// Report the GPS. Only a GPS that had a fix since riding last started can be lost.
    pub(crate) fn gps(&mut self, riding: bool, live_fix: bool, now_ms: u32) {
        self.gps_seen = riding && (self.gps_seen || live_fix);
        self.level(Source::Gps, self.gps_seen.then_some(!live_fix), now_ms);
    }

    /// Report the sensor slots. Only a sensor that connected while riding can drop.
    pub(crate) fn sensors(&mut self, slots: &[SensorStatus], riding: bool, now_ms: u32) {
        let mask = |keep: fn(&SensorStatus) -> bool| {
            slots.iter().enumerate().filter(|(_, s)| keep(s)).fold(0u8, |m, (i, _)| m | 1 << i)
        };
        let connected = mask(|s| s.phase == SensorPhase::Connected);
        self.sensors_seen = if riding { (self.sensors_seen | connected) & mask(SensorStatus::saved) } else { 0 };
        self.level(Source::Sensor, riding.then_some(self.sensors_seen & !connected != 0), now_ms);
    }

    /// Report the battery charge. Each threshold plays once on the way down.
    pub(crate) fn battery(&mut self, pct: u8) {
        let thresholds = [(LOW_BATTERY_PCT, Cue::BatteryLow), (BATTERY_CRITICAL_PCT, Cue::BatteryCritical)];
        for (i, (below, cue)) in thresholds.into_iter().enumerate() {
            if pct < below && self.battery_armed[i] {
                self.battery_armed[i] = false;
                self.raise(cue);
            } else if pct >= below + BATTERY_REARM_PCT {
                self.battery_armed[i] = true;
            }
        }
    }

    /// Report whether the rider has arrived, at the route end or at a visit's stop.
    pub(crate) fn arrived(&mut self, arrived: bool) {
        if arrived && !self.arrived {
            self.raise(Cue::Arrived);
        }
        self.arrived = arrived;
    }

    /// Millis until the soonest pending level settles, or `None` when nothing is pending.
    pub(crate) fn wake_in(&self, now_ms: u32) -> Option<u32> {
        (self.levels.iter().zip(Source::ALL)).filter_map(|(l, source)| l.due_in(source, now_ms)).min()
    }

    /// The cue to start now, at `volume`. The raised cue is consumed even when `volume` is `None`,
    /// so a cue never plays late when the rider turns sound on.
    pub(crate) fn take(&mut self, volume: Option<Volume>) -> Option<Sound> {
        let cue = self.raised.take()?;
        Some(Sound { cue, volume: volume? })
    }
}

impl App {
    /// Report every level source, then take the cue this pass plays. Call it once per pass, before
    /// the wake is planned, so a pending settle deadline is part of it.
    pub(crate) fn plan_sound(&mut self) -> Option<Sound> {
        let now = self.ui.now_ms;
        let riding = self.activity.mode == Mode::Riding;
        let route = self.navigator.route_state();
        let off_route = route.active_route.is_some().then_some(route.off_route);
        let arrived = route.arrival.arrived() || self.visit_arrival_pending();
        let live_fix = self.has_live_fix(now);
        let cues = &mut self.cues;
        cues.level(Source::OffRoute, off_route, now);
        cues.gps(riding, live_fix, now);
        cues.sensors(&self.ui.sensor_status, riding, now);
        cues.battery(self.state.device.battery_pct);
        cues.arrived(arrived);
        let volume = self.state.sound_available.then(|| self.settings().sound.volume()).flatten();
        self.cues.take(volume)
    }

    /// Millis until a cue level can change without an input: a pending level settles, or, while
    /// riding, the live fix goes stale and the `GpsLost` settle time starts.
    pub(crate) fn cue_wake_in(&self, now_ms: u32) -> Option<u32> {
        let fix_stale = (self.activity.mode == Mode::Riding).then(|| self.live_fix_left_ms(now_ms)).flatten();
        [self.cues.wake_in(now_ms), fix_stale].into_iter().flatten().min()
    }

    /// Raise the key click, when the rider turned key tones on.
    pub(crate) fn key_click(&mut self) {
        if self.settings().key_tones {
            self.cues.raise(Cue::KeyClick);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn played(cues: &mut Cues) -> Option<Cue> {
        cues.take(Some(Volume::Loud)).map(|s| s.cue)
    }

    /// Off route every 6 s for three minutes: one loss and one recovery cue in each minute.
    #[test]
    fn a_flapping_level_plays_one_loss_and_one_recovery_a_minute() {
        let mut cues = Cues::new();
        let mut heard = heapless::Vec::<(u32, Cue), 16>::new();
        for s in 0..180 {
            cues.level(Source::OffRoute, Some(s / 6 % 2 == 0), s * 1_000);
            if let Some(cue) = played(&mut cues) {
                heard.push((s, cue)).unwrap();
            }
        }
        let expected = [
            (5, Cue::OffRoute),
            (11, Cue::BackOnRoute),
            (65, Cue::OffRoute),
            (71, Cue::BackOnRoute),
            (125, Cue::OffRoute),
            (131, Cue::BackOnRoute),
        ];
        assert_eq!(heard.as_slice(), &expected);
    }

    #[test]
    fn a_loss_inside_the_cooldown_plays_when_the_cooldown_ends() {
        let mut cues = Cues::new();
        cues.level(Source::Gps, Some(true), 0);
        assert_eq!(cues.wake_in(4_000), Some(6_000), "the settle deadline wakes the pass");
        cues.level(Source::Gps, Some(true), 10_000);
        assert_eq!(played(&mut cues), Some(Cue::GpsLost));
        cues.level(Source::Gps, Some(false), 11_000);
        cues.level(Source::Gps, Some(false), 16_000);
        assert_eq!(played(&mut cues), Some(Cue::GpsBack));

        // Lost again inside the cooldown: a short loss plays nothing, a long one plays at its end.
        cues.level(Source::Gps, Some(true), 20_000);
        cues.level(Source::Gps, Some(true), 30_000);
        cues.level(Source::Gps, Some(false), 31_000);
        assert_eq!(played(&mut cues), None);
        cues.level(Source::Gps, Some(true), 40_000);
        assert_eq!(cues.wake_in(50_000), Some(20_000), "the cooldown end wakes the pass");
        cues.level(Source::Gps, Some(true), 69_999);
        assert_eq!(played(&mut cues), None);
        cues.level(Source::Gps, Some(true), 70_000);
        assert_eq!(played(&mut cues), Some(Cue::GpsLost));

        // A sensor drop has no recovery cue.
        cues.level(Source::Sensor, Some(true), 80_000);
        cues.level(Source::Sensor, Some(true), 90_000);
        assert_eq!(played(&mut cues), Some(Cue::SensorDropped));
        cues.level(Source::Sensor, Some(false), 91_000);
        cues.level(Source::Sensor, Some(false), 96_000);
        assert_eq!(played(&mut cues), None);
    }

    #[test]
    fn gps_lost_needs_a_fix_since_riding_started() {
        let mut cues = Cues::new();
        for s in 0..30 {
            cues.gps(s >= 10, false, s * 1_000);
        }
        assert_eq!(played(&mut cues), None, "no fix to lose, riding or not");
        assert_eq!(cues.wake_in(30_000), None, "a closed gate arms no wake");
        cues.gps(true, true, 30_000);
        cues.gps(true, false, 31_000);
        cues.gps(true, false, 41_000);
        assert_eq!(played(&mut cues), Some(Cue::GpsLost));

        cues.gps(false, false, 42_000);
        cues.gps(true, false, 43_000);
        cues.gps(true, false, 60_000);
        assert_eq!(played(&mut cues), None, "a pause forgets the fix");
    }

    #[test]
    fn a_higher_family_wins_the_pass_and_the_first_wins_in_a_family() {
        let mut cues = Cues::new();
        cues.raise(Cue::KeyClick);
        cues.raise(Cue::ClimbStarts);
        cues.battery(9);
        cues.raise(Cue::Arrived);
        assert_eq!(played(&mut cues), Some(Cue::BatteryLow));

        cues.raise(Cue::HoldDone);
        cues.raise(Cue::KeyClick);
        assert_eq!(played(&mut cues), Some(Cue::HoldDone));

        cues.battery(4);
        assert_eq!(played(&mut cues), Some(Cue::BatteryCritical));
        cues.battery(3);
        assert_eq!(played(&mut cues), None, "each threshold plays once");
        cues.battery(12);
        cues.battery(8);
        assert_eq!(played(&mut cues), None, "Low re-arms only at 15 %");
        cues.battery(15);
        cues.battery(9);
        assert_eq!(played(&mut cues), Some(Cue::BatteryLow));
    }

    #[test]
    fn with_sound_off_a_cue_is_consumed_and_never_plays_late() {
        let mut cues = Cues::new();
        cues.raise(Cue::RecordingError);
        assert_eq!(cues.take(None), None);
        assert_eq!(played(&mut cues), None);
    }
}
