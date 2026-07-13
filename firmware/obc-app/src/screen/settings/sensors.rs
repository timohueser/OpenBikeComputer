//! The Sensors screen (BLE sensors epic #707, SE7) — pair, view and forget the HR / power / cadence
//! sensors, placed next to Bluetooth in Settings.
//!
//! Two screens live here:
//!
//! - [`SensorsScreen`]: three rows — `Heart rate` / `Power` / `Cadence` — each a kind label over a
//!   live status line (`Not set` · `Searching` · `Connecting` · `Connected · 78%`, battery only when
//!   known). **Press** a row → open its scan list; on a **saved** row a **hold** forgets it (the
//!   Bluetooth Forget-family guarded footer: a plain prompt while unselected, the shaded base filling
//!   warning-red with the live hold while the row is selected).
//! - [`SensorScanScreen`]: the scan list for one quantity — the discovered sensors of that kind
//!   (name, or address when unnamed, + RSSI), a turn to move, a press to **save + connect** (writes
//!   the settings slot → the host reconciles it to the radio) and pop back to the row now `Connecting`.
//!   Empty while scanning shows `Searching…`.
//!
//! Saving/forgetting is a plain [`Settings`](crate::Settings) edit — the host's per-pass reconcile
//! (the `set_radio_enabled` shape) carries the change to the board's central manager and persists it,
//! so there is one durable path (no separate one-shot to race the settings write). Scan mode is a
//! level ([`Activity::request_sensor_scan`]) the host polls to keep a discovery scan running.

use core::fmt::Write;

use embedded_graphics::prelude::Point;
use obc_render::{
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::screen::{confirm_row, palette, title_frame, Ctx, Render, Screen, Transition, LIST_TOP};
use crate::sensors::{SensorPhase, SensorStatus};
use crate::settings::{Language, SavedSensor, SENSOR_SLOTS};
use crate::Msg;

/// Two-line row height — matches the Bluetooth toggle row's family.
const ROW_H: i32 = 58;
/// The Forget footer's height + bottom anchor — the Route overview / Bluetooth Delete-row geometry
/// (38 px tall, 10 px above the card bottom) so the button faces all match.
const FORGET_H: i32 = 38;

/// The i18n key for a slot's kind label (`Heart rate` / `Power` / `Cadence`). Slot index = kind.
fn kind_msg(slot: usize) -> Msg {
    match slot {
        0 => Msg::SensorsHeartRate,
        1 => Msg::SensorsPower,
        _ => Msg::SensorsCadence,
    }
}

/// The Sensors screen — the three kind rows. State is the highlighted row.
#[derive(Debug, Default)]
pub struct SensorsScreen {
    selected: usize,
}

impl SensorsScreen {
    pub fn new() -> Self {
        SensorsScreen { selected: 0 }
    }

    /// True while a hold would charge the Forget footer — the selected row has a saved sensor
    /// ([`App::top_wants_hold_fill`](crate::App::top_wants_hold_fill) repaints a charging hold).
    pub(crate) fn selection_is_guarded(&self, settings: &crate::Settings) -> bool {
        settings.saved_sensors.get(self.selected).is_some_and(|s| s.present)
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Turn(n) => {
                self.selected = crate::screen::list::step_selection(self.selected, n, SENSOR_SLOTS);
                Transition::None
            }
            // Enter the row's scan list — raise scan mode so the host runs a discovery scan and feeds
            // the hits back; the scan screen lowers it on exit.
            Gesture::Press => {
                cx.activity.request_sensor_scan(true);
                Transition::Push(Screen::SensorScan(SensorScanScreen::new(self.selected as u8)))
            }
            // Forget: the guarded hold, live only while the selected slot holds a saved sensor. A plain
            // settings edit — the host reconcile drops the link and persists the cleared slot (the same
            // path a factory reset takes). No confirmation popup; the guarded hold *is* the confirm.
            Gesture::Hold if self.selection_is_guarded(cx.settings) => {
                cx.settings.saved_sensors[self.selected] = SavedSensor::EMPTY;
                Transition::None
            }
            Gesture::Back => Transition::Pop,
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, rx.t(Msg::SensorsTitle), "");

        for slot in 0..SENSOR_SLOTS {
            let y = LIST_TOP + 8 + slot as i32 * ROW_H;
            let row = super::row_rect(y, w, ROW_H);
            super::row_cursor(cv, row, slot == self.selected, false);
            let present = rx.settings.saved_sensors[slot].present;
            let status = rx.sensor_status.get(slot).copied().unwrap_or_default();
            let mut sub = heapless::String::<24>::new();
            status_line(&mut sub, present, status, rx.settings.language);
            super::row_label(cv, row, rx.t(kind_msg(slot)), Some(&sub));
        }

        // The Forget footer — the Bluetooth/Route Delete-row treatment: a plain left-aligned label
        // while the selected row has no saved sensor, the shaded base + warning hold-fill while it
        // does. Drawn only when the selected slot is saved (the only-when-possible grammar).
        if self.selection_is_guarded(rx.settings) {
            let fy = h - 10 - FORGET_H;
            let row = super::row_rect(fy, w, FORGET_H);
            confirm_row(cv, row, true, true, rx.hold_progress, WARNING, 6);
            cv.text_vcentered(
                rx.t(Msg::SensorsForget),
                row.top_left.x + 12,
                (fy, FORGET_H),
                Font::Body,
                TextAlign::Left,
                INK,
            );
        }
    }
}

/// Compose one row's status line into `buf`: `Not set` when no sensor is saved, else the live phase
/// (`Searching` / `Connecting` / `Connected`, with `· NN%` when the battery is known). A saved slot
/// whose status snapshot hasn't caught up (`NotSet`) still reads `Searching` — it will connect — so
/// the line never contradicts the saved-and-forgettable footer.
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
                let _ = write!(buf, " \u{00b7} {pct}%");
            }
        }
        // NotSet (snapshot not caught up) or Searching both read as searching for the saved sensor.
        _ => {
            let _ = buf.push_str(crate::t(Msg::SensorsSearching, lang));
        }
    }
}

/// The scan list for one sensor quantity (SE7). State is the target slot + the highlighted hit.
#[derive(Debug)]
pub struct SensorScanScreen {
    /// The quantity being paired (0 HR · 1 Power · 2 Cadence) — filters the scan hits.
    slot: u8,
    selected: usize,
}

impl SensorScanScreen {
    pub fn new(slot: u8) -> Self {
        SensorScanScreen { slot, selected: 0 }
    }

    /// The hits of this screen's quantity, in feed order — the visible rows.
    fn hits<'a>(
        &self,
        all: &'a [crate::sensors::SensorScanHit],
    ) -> impl Iterator<Item = &'a crate::sensors::SensorScanHit> {
        let slot = self.slot;
        all.iter().filter(move |h| h.slot == slot)
    }

    /// How many hits this quantity has right now — bounds the cursor.
    fn count(&self, all: &[crate::sensors::SensorScanHit]) -> usize {
        self.hits(all).count()
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Turn(n) => {
                let len = self.count(cx.sensor_scan_hits);
                if len > 0 {
                    self.selected = crate::screen::list::step_selection(self.selected.min(len - 1), n, len);
                }
                Transition::None
            }
            // Save + connect the highlighted sensor: write the settings slot (the host reconcile
            // carries it to the radio and persists it), leave scan mode, and pop back to the row —
            // which now reads `Connecting` from the pushed status.
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
            // Back cancels the scan (lowers scan mode) and returns to the row list unchanged.
            Gesture::Back => {
                cx.activity.request_sensor_scan(false);
                Transition::Pop
            }
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        // The kind being paired titles the scan list, so the three lists read distinctly; the body
        // (a sensor list, or the `Searching...` note) makes it clear this is the pairing screen.
        title_frame(cv, w, h, rx.t(kind_msg(self.slot as usize)), "");

        let len = self.count(rx.sensor_scan_hits);
        if len == 0 {
            // Empty while scanning: the calm searching note (no list yet).
            super::empty_state(cv, w, h, rx.t(Msg::SensorsScanning), "");
            return;
        }

        let selected = self.selected.min(len - 1);
        for (i, hit) in self.hits(rx.sensor_scan_hits).enumerate() {
            let y = LIST_TOP + 8 + i as i32 * ROW_H;
            let row = super::row_rect(y, w, ROW_H);
            super::row_cursor(cv, row, i == selected, false);
            // Name, or the address when the advert carried none.
            let x = row.top_left.x + 10;
            if hit.name.is_empty() {
                let mut addr = heapless::String::<24>::new();
                fmt_addr(&mut addr, &hit.addr);
                cv.text(&addr, Point::new(x, row.top_left.y + 5), Font::Body, TextAlign::Left, INK);
            } else {
                cv.text(&hit.name, Point::new(x, row.top_left.y + 5), Font::Body, TextAlign::Left, INK);
            }
            // RSSI (dBm) under the name — the signal cue.
            let mut rssi = heapless::String::<12>::new();
            let _ = write!(rssi, "{} dBm", hit.rssi);
            cv.text(&rssi, Point::new(x, row.top_left.y + 30), Font::Label, TextAlign::Left, SUBTEXT);
        }
    }
}

/// Format a BLE address big-endian (`AA:BB:…`), the conventional display order (the stored bytes are
/// little-endian, as the wire carries them).
fn fmt_addr(buf: &mut heapless::String<24>, addr: &[u8; 6]) {
    for (i, b) in addr.iter().rev().enumerate() {
        if i > 0 {
            let _ = buf.push(':');
        }
        let _ = write!(buf, "{b:02X}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Activity;
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
        let scratch = crate::screen::PoiScratch::new();
        let mut cx = Ctx {
            state: st,
            activity: &mut act,
            settings: s,
            routes: &[],
            rides: &[],
            trips: &[],
            nav_profiles: &crate::NavProfiles::EMPTY,
            poi_scratch: &scratch,
            sensor_scan_hits: hits,
            now_ms: 0,
        };
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
        let scratch = crate::screen::PoiScratch::new();
        let mut cx = Ctx {
            state: st,
            activity: act,
            settings: s,
            routes: &[],
            rides: &[],
            trips: &[],
            nav_profiles: &crate::NavProfiles::EMPTY,
            poi_scratch: &scratch,
            sensor_scan_hits: hits,
            now_ms: 0,
        };
        scr.handle(g, &mut cx)
    }

    /// Entering a row pushes its scan list and raises scan mode; the slot travels with the screen.
    #[test]
    fn press_opens_scan_for_the_selected_kind() {
        let mut st = AppState::new(0, 0, 1.0);
        let mut s = Settings::default();
        let mut scr = SensorsScreen::new();
        run(&mut scr, &mut st, &mut s, &[], Gesture::Turn(1)); // → Power (slot 1)
        let t = {
            let mut act = Activity::new(Mode::Idle);
            let scratch = crate::screen::PoiScratch::new();
            let mut cx = Ctx {
                state: &mut st,
                activity: &mut act,
                settings: &mut s,
                routes: &[],
                rides: &[],
                trips: &[],
                nav_profiles: &crate::NavProfiles::EMPTY,
                poi_scratch: &scratch,
                sensor_scan_hits: &[],
                now_ms: 0,
            };
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

    /// Forget is guarded: live only on a saved row, and it clears the settings slot (the host reconcile
    /// drops the link + persists). A hold on an empty row does nothing.
    #[test]
    fn forget_hold_is_guarded_and_clears_the_slot() {
        let mut st = AppState::new(0, 0, 1.0);
        let mut s = Settings::default();
        let mut scr = SensorsScreen::new();

        // Empty HR row: the footer isn't armed, a hold does nothing.
        assert!(!scr.selection_is_guarded(&s));
        run(&mut scr, &mut st, &mut s, &[], Gesture::Hold);
        assert!(!s.saved_sensors[0].present, "nothing to forget on an empty row");

        // Save HR, then the hold clears it.
        s.saved_sensors[0] = SavedSensor::saved(1, [9, 9, 9, 9, 9, 9]);
        assert!(scr.selection_is_guarded(&s), "a saved row arms the footer");
        run(&mut scr, &mut st, &mut s, &[], Gesture::Hold);
        assert_eq!(s.saved_sensors[0], SavedSensor::EMPTY, "the hold forgets the sensor");
    }

    /// Picking a scan hit saves its address to the slot, leaves scan mode, and pops back.
    #[test]
    fn picking_a_hit_saves_and_pops() {
        let mut st = AppState::new(0, 0, 1.0);
        let mut s = Settings::default();
        let mut act = Activity::new(Mode::Idle);
        act.request_sensor_scan(true);
        let mut scr = SensorScanScreen::new(0); // HR
        let hits = [hit(1, "PWR", -50), hit(0, "HRM", -60), hit(0, "Watch", -72)];

        // The cursor + press select the *first HR* hit (the power hit is filtered out).
        let t = run_scan(&mut scr, &mut st, &mut s, &mut act, &hits, Gesture::Press);
        assert!(matches!(t, Transition::Pop), "a pick pops back to the row list");
        assert!(s.saved_sensors[0].present, "the HR slot now holds a saved sensor");
        assert_eq!(s.saved_sensors[0].addr, [1, 2, 3, 4, 5, 6]);
        assert!(!act.sensor_scan_active(), "picking leaves scan mode");
    }

    /// Turn walks only this quantity's hits; a press with no hits does nothing (no panic on empty).
    #[test]
    fn scan_cursor_bounded_to_kind_and_empty_is_safe() {
        let mut st = AppState::new(0, 0, 1.0);
        let mut s = Settings::default();
        let mut act = Activity::new(Mode::Idle);
        let mut scr = SensorScanScreen::new(2); // Cadence — no hits below
        let hits = [hit(0, "HRM", -60), hit(1, "PWR", -50)];

        run_scan(&mut scr, &mut st, &mut s, &mut act, &hits, Gesture::Turn(1));
        assert_eq!(scr.selected, 0, "no cadence hits → the cursor can't move");
        let t = run_scan(&mut scr, &mut st, &mut s, &mut act, &hits, Gesture::Press);
        assert!(matches!(t, Transition::None), "a press with no hit does nothing");
        assert!(!s.saved_sensors[2].present, "and saves nothing");
    }

    /// Back lowers scan mode and pops.
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

    /// The status line: `Not set` when empty, else the phase, with the battery only when known.
    #[test]
    fn status_line_reads_the_phase() {
        let en = Language::En;
        let mut b = heapless::String::<24>::new();

        status_line(&mut b, false, SensorStatus::default(), en);
        assert_eq!(b.as_str(), "Not set");

        b.clear();
        status_line(&mut b, true, SensorStatus { phase: SensorPhase::Searching, ..Default::default() }, en);
        assert_eq!(b.as_str(), "Searching");

        // A saved slot whose snapshot hasn't caught up still reads Searching (never contradicts the
        // armed Forget footer).
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
        assert_eq!(b.as_str(), "Connected \u{00b7} 78%");

        b.clear();
        status_line(&mut b, true, SensorStatus { phase: SensorPhase::Connected, battery: None, last_value_ms: 0 }, en);
        assert_eq!(b.as_str(), "Connected", "no battery → no percent tail");
    }

    /// Addresses render big-endian with colons, unnamed hits fall back to the address.
    #[test]
    fn address_formatting() {
        let mut b = heapless::String::<24>::new();
        fmt_addr(&mut b, &[0x11, 0x22, 0x33, 0x44, 0x55, 0x66]);
        assert_eq!(b.as_str(), "66:55:44:33:22:11");
    }
}
