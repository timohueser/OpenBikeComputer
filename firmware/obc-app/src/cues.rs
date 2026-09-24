//! Which cue the rider hears. Events that are edges already raise their cue directly. The levels
//! the app already has become edges here, after a settle time and with a cooldown, so a level that
//! flaps plays at most one loss cue a minute, and one recovery cue for each loss.

use obc_ports::{Cue, Volume};

use crate::activity::Mode;
use crate::device_core::Sound;
use crate::device_status::LOW_BATTERY_PCT;
use crate::sensors::{SensorPhase, SensorStatus};
use crate::settings::SENSOR_SLOTS;
use crate::{Alert, Alerts, App};

/// After a loss cue, the same source raises no loss cue for this long. A loss that holds past the
/// cooldown plays when it ends. A failure cue is silent for this long after it plays.
const LOSS_COOLDOWN_MS: u32 = 60_000;
/// The alerts that play a cue. The others are shown on the warning card only.
const FAILURES: [(Alert, Cue); 2] =
    [(Alert::RecordingFailed, Cue::RecordingError), (Alert::StorageLost, Cue::StorageLost)];
const BATTERY_CRITICAL_PCT: u8 = 5;
/// A battery threshold re-arms when the charge rises this many points above it.
const BATTERY_REARM_PCT: u8 = 5;

/// A level that is lost and comes back, with one level in [`Cues`].
#[derive(Debug, Clone, Copy)]
pub(crate) enum Source {
    OffRoute,
    Gps,
}

impl Source {
    const ALL: [Source; 2] = [Source::OffRoute, Source::Gps];

    const fn rule(self) -> Rule {
        match self {
            Source::OffRoute => {
                Rule { loss: Cue::OffRoute, recovery: Some(Cue::BackOnRoute), lost_ms: 5_000, back_ms: 5_000 }
            }
            // The live-fix window already waits at least 5 s, so `GpsLost` plays at least 15 s after
            // the last fix.
            Source::Gps => Rule { loss: Cue::GpsLost, recovery: Some(Cue::GpsBack), lost_ms: 10_000, back_ms: 5_000 },
        }
    }
}

/// The cues of a level, and how long a new level must hold before it counts.
#[derive(Debug, Clone, Copy)]
struct Rule {
    loss: Cue,
    recovery: Option<Cue>,
    lost_ms: u32,
    back_ms: u32,
}

/// One sensor slot that connected during the ride and is not connected now. Each slot has its own
/// level, and one cooldown covers them all. A slot that connects again is forgotten at once.
const SENSOR: Rule = Rule { loss: Cue::SensorDropped, recovery: None, lost_ms: 10_000, back_ms: 0 };

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
    fn due_in(&self, rule: Rule, now_ms: u32) -> Option<u32> {
        let settle = if self.lost { rule.back_ms } else { rule.lost_ms };
        let settle = settle.saturating_sub(now_ms.wrapping_sub(self.since?));
        let cooldown = match (self.lost, self.loss_at) {
            (false, Some(at)) => LOSS_COOLDOWN_MS.saturating_sub(now_ms.wrapping_sub(at)),
            _ => 0,
        };
        Some(settle.max(cooldown))
    }

    /// Report the level: `Some(lost)` while its gate is open. `None` closes the gate and forgets
    /// the current loss, so no cue plays for it. Returns the cue to raise when the level settles.
    fn report(&mut self, rule: Rule, lost: Option<bool>, now_ms: u32) -> Option<Cue> {
        let Some(lost) = lost else {
            *self = Level { loss_at: self.loss_at, ..Level::IDLE };
            return None;
        };
        if lost == self.lost {
            self.since = None;
            return None;
        }
        self.since.get_or_insert(now_ms);
        if self.due_in(rule, now_ms) != Some(0) {
            return None;
        }
        self.lost = lost;
        self.since = None;
        if lost {
            self.loss_at = Some(now_ms);
            Some(rule.loss)
        } else {
            rule.recovery
        }
    }
}

/// The cue state. It lives in [`App`](crate::App), not in the pass state.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Cues {
    /// The cue to play at the next plan.
    raised: Option<Cue>,
    levels: [Level; Source::ALL.len()],
    sensors: [Level; SENSOR_SLOTS],
    /// Whether the GPS had a fix since riding last started, so it has something to lose.
    gps_seen: bool,
    /// Slots whose sensor connected since riding last started.
    sensors_seen: u8,
    /// Whether `BatteryLow` and `BatteryCritical` may play.
    battery_armed: [bool; 2],
    arrived: bool,
    /// When each of the [`FAILURES`] last played its cue.
    failure_at: [Option<u32>; 2],
}

impl Cues {
    pub(crate) const fn new() -> Self {
        Cues {
            raised: None,
            levels: [Level::IDLE; Source::ALL.len()],
            sensors: [Level::IDLE; SENSOR_SLOTS],
            gps_seen: false,
            sensors_seen: 0,
            battery_armed: [true; 2],
            arrived: false,
            failure_at: [None; 2],
        }
    }

    /// Raise `cue` for the next plan. The higher family wins; in one family the first raised wins.
    pub(crate) fn raise(&mut self, cue: Cue) {
        if self.raised.is_none_or(|r| cue.family() > r.family()) {
            self.raised = Some(cue);
        }
    }

    /// Report the level of `source`, as [`Level::report`] takes it.
    pub(crate) fn level(&mut self, source: Source, lost: Option<bool>, now_ms: u32) {
        if let Some(cue) = self.levels[source as usize].report(source.rule(), lost, now_ms) {
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
        let mut dropped = false;
        for (i, level) in self.sensors.iter_mut().enumerate() {
            let lost = (self.sensors_seen & 1 << i != 0).then_some(connected & 1 << i == 0);
            dropped |= level.report(SENSOR, lost, now_ms).is_some();
        }
        if dropped {
            self.sensors.iter_mut().for_each(|l| l.loss_at = Some(now_ms));
            self.raise(Cue::SensorDropped);
        }
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

    /// Report the alerts of a pass. A failure plays its cue on every raise outside its cooldown.
    pub(crate) fn alerts(&mut self, alerts: Alerts, now_ms: u32) {
        for (i, (alert, cue)) in FAILURES.into_iter().enumerate() {
            let cooled = self.failure_at[i].is_none_or(|at| now_ms.wrapping_sub(at) >= LOSS_COOLDOWN_MS);
            if alerts.contains(alert) && cooled {
                self.failure_at[i] = Some(now_ms);
                self.raise(cue);
            }
        }
    }

    /// Millis until the soonest pending level settles, or `None` when nothing is pending.
    pub(crate) fn wake_in(&self, now_ms: u32) -> Option<u32> {
        let sensors = self.sensors.iter().map(|l| (l, SENSOR));
        let levels = self.levels.iter().zip(Source::ALL.map(Source::rule));
        levels.chain(sensors).filter_map(|(l, rule)| l.due_in(rule, now_ms)).min()
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
    }

    /// Ride for `secs` with the slots that `connected(s)` gives at second `s`, and list the seconds
    /// at which `SensorDropped` plays.
    fn sensor_drops(secs: u32, connected: impl Fn(u32) -> [bool; SENSOR_SLOTS]) -> heapless::Vec<u32, 8> {
        let mut cues = Cues::new();
        let mut heard = heapless::Vec::new();
        for s in 0..secs {
            let slots = connected(s).map(|c| SensorStatus {
                phase: if c { SensorPhase::Connected } else { SensorPhase::Searching },
                ..SensorStatus::default()
            });
            cues.sensors(&slots, true, s * 1_000);
            if let Some(cue) = played(&mut cues) {
                assert_eq!(cue, Cue::SensorDropped);
                heard.push(s).unwrap();
            }
        }
        heard
    }

    #[test]
    fn each_sensor_that_drops_plays_its_own_cue() {
        let apart = sensor_drops(150, |s| [s < 10, s < 100, true]);
        assert_eq!(apart.as_slice(), &[20, 110], "two drops at different times");

        // B is due at 40 s, inside the cooldown of A's cue, and plays when the cooldown ends.
        let overlap = sensor_drops(150, |s| [!(10..35).contains(&s), s < 30, true]);
        assert_eq!(overlap.as_slice(), &[20, 80], "A drops, B drops, A comes back");
    }

    #[test]
    fn a_flapping_sensor_plays_at_most_one_cue_a_minute() {
        // Up for 3 s, down for 12 s, for three minutes.
        let heard = sensor_drops(180, |s| [s % 15 < 3, true, true]);
        assert_eq!(heard.as_slice(), &[13, 73, 133]);
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

    /// A persistent failure is raised on every pass. Its cue plays at most once a minute.
    #[test]
    fn a_failure_raised_every_pass_plays_once_a_minute() {
        let mut cues = Cues::new();
        let mut heard = heapless::Vec::<u32, 4>::new();
        for s in 0..180 {
            cues.alerts(Alert::RecordingFailed.into(), s * 1_000);
            if played(&mut cues) == Some(Cue::RecordingError) {
                heard.push(s).unwrap();
            }
        }
        assert_eq!(heard.as_slice(), &[0, 60, 120]);

        cues.raise(Cue::BatteryLow);
        cues.alerts(Alert::StorageLost.into(), 180_000);
        assert_eq!(played(&mut cues), Some(Cue::StorageLost), "storage lost outranks a Problem cue");
        assert_eq!(Cue::StorageLost.family(), obc_ports::Family::Urgent);
    }
}
