//! Pair, view and forget the heart rate, power and cadence sensors. [`SensorsScreen`] lists the
//! three kinds, and [`SensorScanScreen`] is the scan list for one kind.
//!
//! A save or a forget is a plain [`Settings`](crate::Settings) edit. The host reconcile carries the
//! change to the radio and persists it, so there is one durable path. Scan mode is a level
//! ([`Activity::request_sensor_scan`]) the host polls to keep a discovery scan running.

use core::fmt::Write;

use obc_render::Surface;

use crate::input::Gesture;
use crate::screen::vocab::chrome::{empty_state, title_frame, LIST_TOP};
use crate::screen::vocab::fmt::write_ble_address;
use crate::screen::vocab::rows::{self, row_rect, Line2, ROW_GAP, ROW_TWO};
use crate::screen::{Ctx, Render, Screen, Transition};
use crate::sensors::{SensorPhase, SensorStatus};
use crate::settings::{Language, SavedSensor, SENSOR_SLOTS};
use crate::Msg;

/// The label key for a slot. The slot index is the sensor kind.
fn kind_msg(slot: usize) -> Msg {
    match slot {
        0 => Msg::SensorsHeartRate,
        1 => Msg::SensorsPower,
        _ => Msg::SensorsCadence,
    }
}

#[derive(Debug, Default)]
pub struct SensorsScreen {
    selected: usize,
}

impl SensorsScreen {
    pub fn new() -> Self {
        SensorsScreen { selected: 0 }
    }

    /// True while a hold would charge the Forget footer, that is, the selected row has a sensor.
    pub(crate) fn selection_is_guarded(&self, settings: &crate::Settings) -> bool {
        settings.saved_sensors.get(self.selected).is_some_and(|s| s.present)
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Step(n) => {
                self.selected = crate::screen::vocab::list::step_selection(self.selected, n, SENSOR_SLOTS);
                Transition::None
            }
            // Scan mode makes the host run a discovery scan. The scan screen lowers it on exit.
            Gesture::Press => {
                cx.activity.request_sensor_scan(true);
                Transition::Push(Screen::SensorScan(SensorScanScreen::new(self.selected as u8)))
            }
            // The guarded hold is the confirmation. There is no popup.
            Gesture::Hold if self.selection_is_guarded(cx.settings) => {
                cx.settings.saved_sensors[self.selected] = SavedSensor::EMPTY;
                Transition::None
            }
            Gesture::Back => Transition::Pop,
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, rx.t(Msg::SensorsTitle), "");

        for slot in 0..SENSOR_SLOTS {
            let y = LIST_TOP + slot as i32 * (ROW_TWO + ROW_GAP);
            let row = row_rect(y, w, ROW_TWO);
            let present = rx.settings.saved_sensors[slot].present;
            let status = rx.sensor_status.get(slot).copied().unwrap_or_default();
            let mut sub = heapless::String::<24>::new();
            status_line(&mut sub, present, status, rx.settings.language);
            rows::nav_row(cv, row, rx.t(kind_msg(slot)), Some(Line2::text(&sub)), slot == self.selected, true, true);
        }

        if self.selection_is_guarded(rx.settings) {
            super::forget_footer(cv, w, h, rx.t(Msg::SensorsForget), rx.hold_progress);
        }
    }
}

/// Compose one row's status line into `buf`. A saved slot whose status snapshot is not yet current
/// reads `Searching`, so the line does not contradict the armed Forget footer.
fn status_line(buf: &mut heapless::String<24>, present: bool, status: SensorStatus, lang: Language) {
    if !present {
        let _ = buf.push_str(crate::t(Msg::SensorsNotSet, lang));
        return;
    }
    match status.phase {
        SensorPhase::Connecting => {
            let _ = buf.push_str(crate::t(Msg::SensorsConnecting, lang));
        }
        SensorPhase::Connected => {
            let _ = buf.push_str(crate::t(Msg::SensorsConnected, lang));
            if let Some(pct) = status.battery {
                let _ = write!(buf, " {pct}%");
            }
        }
        // A stale snapshot (`NotSet`) reads the same as `Searching`.
        _ => {
            let _ = buf.push_str(crate::t(Msg::SensorsSearching, lang));
        }
    }
}

#[derive(Debug)]
pub struct SensorScanScreen {
    /// The kind being paired: 0 heart rate, 1 power, 2 cadence. It filters the scan hits.
    slot: u8,
    selected: usize,
}

impl SensorScanScreen {
    pub fn new(slot: u8) -> Self {
        SensorScanScreen { slot, selected: 0 }
    }

    fn hits<'a>(
        &self,
        all: &'a [crate::sensors::SensorScanHit],
    ) -> impl Iterator<Item = &'a crate::sensors::SensorScanHit> {
        let slot = self.slot;
        all.iter().filter(move |h| h.slot == slot)
    }

    fn count(&self, all: &[crate::sensors::SensorScanHit]) -> usize {
        self.hits(all).count()
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Step(n) => {
                let len = self.count(cx.sensor_scan_hits);
                if len > 0 {
                    self.selected = crate::screen::vocab::list::step_selection(self.selected.min(len - 1), n, len);
                }
                Transition::None
            }
            // The settings write is the save. The host reconcile connects the sensor.
            Gesture::Press => {
                let picked = self.hits(cx.sensor_scan_hits).nth(self.selected).map(|h| (h.addr_kind, h.addr));
                if let Some((addr_kind, addr)) = picked {
                    cx.settings.saved_sensors[self.slot as usize] = SavedSensor::saved(addr_kind, addr);
                    cx.activity.request_sensor_scan(false);
                    Transition::Pop
                } else {
                    Transition::None
                }
            }
            Gesture::Back => {
                cx.activity.request_sensor_scan(false);
                Transition::Pop
            }
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, rx.t(kind_msg(self.slot as usize)), "");

        let len = self.count(rx.sensor_scan_hits);
        if len == 0 {
            empty_state(cv, w, h, rx.t(Msg::SensorsScanning), "");
            return;
        }

        let selected = self.selected.min(len - 1);
        for (i, hit) in self.hits(rx.sensor_scan_hits).enumerate() {
            let y = LIST_TOP + i as i32 * (ROW_TWO + ROW_GAP);
            let row = row_rect(y, w, ROW_TWO);
            let mut addr = heapless::String::<24>::new();
            if hit.name.is_empty() {
                write_ble_address(&mut addr, &hit.addr);
            }
            let name = if hit.name.is_empty() { addr.as_str() } else { hit.name.as_str() };
            let mut rssi = heapless::String::<12>::new();
            let _ = write!(rssi, "{} dBm", hit.rssi);
            rows::nav_row(cv, row, name, Some(Line2::text(&rssi)), i == selected, true, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Activity;
    use crate::screen::test_ctx;
    use crate::sensors::SensorScanHit;
    use crate::settings::Settings;
    use crate::{AppState, Mode};

    fn hit(slot: u8, name: &str, rssi: i8) -> SensorScanHit {
        let mut n = heapless::String::new();
        let _ = n.push_str(name);
        SensorScanHit { slot, addr_kind: 1, addr: [1, 2, 3, 4, 5, 6], name: n, rssi }
    }

    fn run(
        scr: &mut SensorsScreen,
        st: &mut AppState,
        s: &mut Settings,
        hits: &[SensorScanHit],
        g: Gesture,
    ) -> Transition {
        let mut act = Activity::new(Mode::Idle);
        let mut cx = Ctx { sensor_scan_hits: hits, ..test_ctx(st, &mut act, s) };
        scr.handle(g, &mut cx)
    }

    fn run_scan(
        scr: &mut SensorScanScreen,
        st: &mut AppState,
        s: &mut Settings,
        act: &mut Activity,
        hits: &[SensorScanHit],
        g: Gesture,
    ) -> Transition {
        let mut cx = Ctx { sensor_scan_hits: hits, ..test_ctx(st, act, s) };
        scr.handle(g, &mut cx)
    }

    #[test]
    fn press_opens_scan_for_the_selected_kind() {
        let mut st = AppState::new(0, 0, 1.0);
        let mut s = Settings::default();
        let mut scr = SensorsScreen::new();
        run(&mut scr, &mut st, &mut s, &[], Gesture::Step(1));
        let t = {
            let mut act = Activity::new(Mode::Idle);
            let mut cx = test_ctx(&mut st, &mut act, &mut s);
            let t = scr.handle(Gesture::Press, &mut cx);
            assert!(act.sensor_scan_active(), "entering a row raises scan mode");
            t
        };
        match t {
            Transition::Push(Screen::SensorScan(scan)) => {
                assert_eq!(scan.slot, 1, "the Power slot travels with the scan")
            }
            _ => panic!("press should push the scan list"),
        }
    }

    #[test]
    fn forget_hold_is_guarded_and_clears_the_slot() {
        let mut st = AppState::new(0, 0, 1.0);
        let mut s = Settings::default();
        let mut scr = SensorsScreen::new();

        assert!(!scr.selection_is_guarded(&s));
        run(&mut scr, &mut st, &mut s, &[], Gesture::Hold);
        assert!(!s.saved_sensors[0].present, "nothing to forget on an empty row");

        s.saved_sensors[0] = SavedSensor::saved(1, [9, 9, 9, 9, 9, 9]);
        assert!(scr.selection_is_guarded(&s), "a saved row arms the footer");
        run(&mut scr, &mut st, &mut s, &[], Gesture::Hold);
        assert_eq!(s.saved_sensors[0], SavedSensor::EMPTY, "the hold forgets the sensor");
    }

    #[test]
    fn picking_a_hit_saves_and_pops() {
        let mut st = AppState::new(0, 0, 1.0);
        let mut s = Settings::default();
        let mut act = Activity::new(Mode::Idle);
        act.request_sensor_scan(true);
        let mut scr = SensorScanScreen::new(0);
        let hits = [hit(1, "PWR", -50), hit(0, "HRM", -60), hit(0, "Watch", -72)];

        // The press selects the first heart rate hit. The power hit is filtered out.
        let t = run_scan(&mut scr, &mut st, &mut s, &mut act, &hits, Gesture::Press);
        assert!(matches!(t, Transition::Pop), "a pick pops back to the row list");
        assert!(s.saved_sensors[0].present, "the HR slot now holds a saved sensor");
        assert_eq!(s.saved_sensors[0].addr, [1, 2, 3, 4, 5, 6]);
        assert!(!act.sensor_scan_active(), "picking leaves scan mode");
    }

    #[test]
    fn scan_cursor_bounded_to_kind_and_empty_is_safe() {
        let mut st = AppState::new(0, 0, 1.0);
        let mut s = Settings::default();
        let mut act = Activity::new(Mode::Idle);
        let mut scr = SensorScanScreen::new(2);
        let hits = [hit(0, "HRM", -60), hit(1, "PWR", -50)];

        run_scan(&mut scr, &mut st, &mut s, &mut act, &hits, Gesture::Step(1));
        assert_eq!(scr.selected, 0, "no cadence hits → the cursor can't move");
        let t = run_scan(&mut scr, &mut st, &mut s, &mut act, &hits, Gesture::Press);
        assert!(matches!(t, Transition::None), "a press with no hit does nothing");
        assert!(!s.saved_sensors[2].present, "and saves nothing");
    }

    #[test]
    fn back_cancels_scan() {
        let mut st = AppState::new(0, 0, 1.0);
        let mut s = Settings::default();
        let mut act = Activity::new(Mode::Idle);
        act.request_sensor_scan(true);
        let mut scr = SensorScanScreen::new(0);
        let t = run_scan(&mut scr, &mut st, &mut s, &mut act, &[], Gesture::Back);
        assert!(matches!(t, Transition::Pop));
        assert!(!act.sensor_scan_active(), "Back leaves scan mode");
    }

    #[test]
    fn status_line_reads_the_phase() {
        let en = Language::En;
        let mut b = heapless::String::<24>::new();

        status_line(&mut b, false, SensorStatus::default(), en);
        assert_eq!(b.as_str(), "Not set");

        b.clear();
        status_line(&mut b, true, SensorStatus { phase: SensorPhase::Searching, ..Default::default() }, en);
        assert_eq!(b.as_str(), "Searching");

        b.clear();
        status_line(&mut b, true, SensorStatus { phase: SensorPhase::NotSet, ..Default::default() }, en);
        assert_eq!(b.as_str(), "Searching");

        b.clear();
        status_line(&mut b, true, SensorStatus { phase: SensorPhase::Connecting, ..Default::default() }, en);
        assert_eq!(b.as_str(), "Connecting");

        b.clear();
        status_line(
            &mut b,
            true,
            SensorStatus { phase: SensorPhase::Connected, battery: Some(78), last_value_ms: 0 },
            en,
        );
        assert_eq!(b.as_str(), "Connected 78%");

        b.clear();
        status_line(&mut b, true, SensorStatus { phase: SensorPhase::Connected, battery: None, last_value_ms: 0 }, en);
        assert_eq!(b.as_str(), "Connected", "no battery → no percent tail");
    }
}
