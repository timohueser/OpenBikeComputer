//! The Bluetooth screen (epic #447, P8) — everything rider-facing about the radio: the on/off
//! switch (persisted as [`Settings::ble_enabled`]), a status line (Off / Advertising / Connected,
//! from the P1 seam), a "Paired: yes/no" row (deliberately no phone name), and the hold-guarded
//! **Forget phone** — the *only* re-pair path now that a stored bond rejects new pairings (the S0
//! §8 amendment).
//!
//! The toggle edits [`Settings`] in place like every settings screen — the host persists it and
//! carries the change to the radio plane. Forget is a **Pause-menu (ride_control) guarded row**
//! (owner review round 3 — the round-2 always-visible base + focus outline retired with the rest
//! of that idiom): a plain left-aligned Body label while unselected, the `PARCHMENT_SHADE` base
//! filling warning-red with the live hold while selected; the completed hold sets
//! [`AppState::ble_forget_pending`](crate::AppState) for the host to drain — no extra
//! confirmation popup, the guarded hold *is* the confirmation. With **no bond stored the row
//! isn't drawn at all** (the round-1 grammar: delete-class actions appear only when possible),
//! and Up/Down has nothing to select below the toggle.

use embedded_graphics::prelude::Point;
use obc_render::{
    text::{Font, TextAlign},
    Surface,
};

use crate::ble::BleLink;
use crate::input::Gesture;
use crate::screen::{confirm_row, palette, title_frame, Ctx, Render, Transition, LIST_TOP};
use crate::settings::{Language, Settings};
use crate::Msg;

/// Toggle-row height — matches the other settings screens' two-line rows.
const ROW_H: i32 = 58;
/// The Forget row's height + bottom anchor — the Route overview Delete row's geometry family
/// (38 px tall, the standard 10 px above the card bottom), so the two button faces match.
const FORGET_H: i32 = 38;

/// The two selectable rows: the radio toggle and the Forget action (the status/paired lines
/// between them are read-only). The Forget row exists only while a bond is stored — see
/// [`rows`].
const TOGGLE: usize = 0;
const FORGET: usize = 1;

/// How many rows the cursor can reach: just the toggle without a stored bond (the Forget row is
/// hidden entirely then — the round-1 only-when-possible grammar), both with one.
fn rows(paired: bool) -> usize {
    if paired {
        2
    } else {
        1
    }
}

/// The Bluetooth screen. State is just the highlighted row.
#[derive(Debug, Default)]
pub struct BluetoothScreen {
    selected: usize,
}

impl BluetoothScreen {
    pub fn new() -> Self {
        BluetoothScreen { selected: 0 }
    }

    /// True while the Forget row is selected **and actionable** (a bond is stored) — its hold fill
    /// would draw, so a charging hold is worth repainting
    /// ([`App::top_wants_hold_fill`](crate::App::top_wants_hold_fill)).
    pub(crate) fn selection_is_guarded(&self, paired: bool) -> bool {
        self.selected == FORGET && paired
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Step(n) => {
                // The bond vanishing from under the cursor (a completed forget) clamps it back
                // onto the toggle before the step, so a step never walks a hidden row.
                let len = rows(cx.state.ble_paired);
                self.selected = self.selected.min(len - 1);
                self.selected = crate::screen::list::step_selection(self.selected, n, len);
                Transition::None
            }
            // The toggle is a click-to-flip; the edit lands in `Settings` and the host's
            // change-detection save persists it + pushes it to the radio plane.
            Gesture::Press if self.selected == TOGGLE => {
                cx.settings.ble_enabled = !cx.settings.ble_enabled;
                Transition::None
            }
            // Forget phone: the guarded hold, live only while a bond is stored. Records the
            // one-shot request for the host ([`App::drain_host_commands`](crate::App::drain_host_commands)).
            Gesture::Hold if self.selected == FORGET && cx.state.ble_paired => {
                cx.state.ble_forget_pending = true;
                Transition::None
            }
            Gesture::Back => Transition::Pop,
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, rx.t(Msg::BluetoothTitle), "");

        // A stale below-the-end cursor (the bond was just forgotten) reads as the toggle row.
        let selected = self.selected.min(rows(rx.state.ble_paired) - 1);

        // Row 0 — the radio switch.
        let r0 = super::row_rect(LIST_TOP + 8, w, ROW_H);
        super::row_cursor(cv, r0, selected == TOGGLE, false);
        super::row_label(cv, r0, rx.t(Msg::BluetoothRadio), Some(rx.t(Msg::BluetoothRadioSub)));
        super::toggle_slider(cv, r0, rx.settings.ble_enabled);

        // The read-only lines — stacked caption-over-value pairs (a right-aligned "Advertising"
        // would collide with its caption on the 240 px panel): status (Off / Advertising /
        // Connected) and Paired (no phone name — deliberately not worth the protocol addition).
        let info_x = super::ROW_X + 10;
        let y0 = LIST_TOP + 8 + ROW_H + 16;
        cv.text(rx.t(Msg::BluetoothStatus), Point::new(info_x, y0), Font::Label, TextAlign::Left, SUBTEXT);
        cv.text(
            status_label(rx.settings, rx.state.ble_link, rx.settings.language),
            Point::new(info_x, y0 + 24),
            Font::Body,
            TextAlign::Left,
            INK,
        );
        let y1 = y0 + 62;
        cv.text(rx.t(Msg::BluetoothPaired), Point::new(info_x, y1), Font::Label, TextAlign::Left, SUBTEXT);
        let paired = if rx.state.ble_paired { rx.t(Msg::BluetoothYes) } else { rx.t(Msg::BluetoothNo) };
        cv.text(paired, Point::new(info_x, y1 + 24), Font::Body, TextAlign::Left, INK);

        // The Forget row — the Pause-menu guarded-row treatment (owner review round 3: the round-2
        // focus outline is retired everywhere): a plain left-aligned Body label while unselected,
        // the shaded base + warning-red hold fill only while the cursor is on it — exactly the
        // ride_control family's selected-guarded face, at the delete rows' bottom anchor. Drawn
        // only while a bond is stored — with nothing to forget the row simply isn't there (the
        // round-1 only-when-possible grammar).
        if rx.state.ble_paired {
            let fy = h - 10 - FORGET_H;
            let row = super::row_rect(fy, w, FORGET_H);
            confirm_row(cv, row, selected == FORGET, true, rx.hold_progress, WARNING, 6);
            cv.text_vcentered(
                rx.t(Msg::BluetoothForget),
                row.top_left.x + 12,
                (fy, FORGET_H),
                Font::Body,
                TextAlign::Left,
                INK,
            );
        }
    }
}

/// The status line's text: `Off` whenever the rider's switch is off (or the radio already reports
/// it), else the live link phase. Preferring the *setting* for Off means the line flips the moment
/// the toggle does, without waiting a pass for the radio to wind down.
fn status_label(settings: &Settings, link: BleLink, lang: Language) -> &'static str {
    if !settings.ble_enabled || link == BleLink::Off {
        crate::t(Msg::BluetoothOff, lang)
    } else if link == BleLink::Connected {
        crate::t(Msg::BluetoothConnected, lang)
    } else {
        crate::t(Msg::BluetoothAdvertising, lang)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Activity;
    use crate::{AppState, Mode};

    fn run(scr: &mut BluetoothScreen, st: &mut AppState, s: &mut Settings, g: Gesture) -> Transition {
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
            waypoints: &[],
            corridor: &[],
            sensor_scan_hits: &[],
            now_ms: 0,
        };
        scr.handle(g, &mut cx)
    }

    /// The toggle row flips the persisted radio switch; the change lands in `Settings` for the
    /// host's change-detection save.
    #[test]
    fn press_flips_the_radio_switch() {
        let mut st = AppState::new(0, 0, 1.0);
        let mut s = Settings::default();
        let mut scr = BluetoothScreen::new();
        assert!(s.ble_enabled, "on by default");
        run(&mut scr, &mut st, &mut s, Gesture::Press);
        assert!(!s.ble_enabled, "press flips it off");
        run(&mut scr, &mut st, &mut s, Gesture::Press);
        assert!(s.ble_enabled, "and back on");
    }

    /// Forget phone: the guarded hold records the one-shot request — but only while a bond is
    /// stored, and only on the Forget row. Without a bond the row is hidden and Up/Down can't even
    /// reach it (the round-1 only-when-possible grammar); a press there does nothing (hold is
    /// the action).
    #[test]
    fn forget_hold_is_guarded_and_paired_gated() {
        let mut st = AppState::new(0, 0, 1.0);
        let mut s = Settings::default();
        let mut scr = BluetoothScreen::new();

        // Unpaired: the Forget row doesn't exist — a step wraps within the one-row list.
        run(&mut scr, &mut st, &mut s, Gesture::Step(1));
        assert_eq!(scr.selected, TOGGLE, "unpaired: the cursor can't leave the toggle");
        assert!(!scr.selection_is_guarded(false), "unpaired: nothing armed");
        run(&mut scr, &mut st, &mut s, Gesture::Hold);
        assert!(!st.ble_forget_pending, "unpaired: a hold does nothing (nothing to forget)");

        st.ble_paired = true;
        run(&mut scr, &mut st, &mut s, Gesture::Step(1)); // → the (now present) Forget row
        assert_eq!(scr.selected, FORGET);
        assert!(scr.selection_is_guarded(true), "paired + selected: the hold fill is live");
        run(&mut scr, &mut st, &mut s, Gesture::Press);
        assert!(!st.ble_forget_pending, "a plain press never forgets");
        run(&mut scr, &mut st, &mut s, Gesture::Hold);
        assert!(st.ble_forget_pending, "the completed hold records the forget request");

        // A hold on the toggle row must not forget.
        st.ble_forget_pending = false;
        run(&mut scr, &mut st, &mut s, Gesture::Step(-1)); // back to the toggle
        run(&mut scr, &mut st, &mut s, Gesture::Hold);
        assert!(!st.ble_forget_pending, "a hold elsewhere doesn't forget");
    }

    /// The bond vanishing from under the cursor (the forget completing) clamps the stale Forget
    /// selection back onto the toggle on the next step — a hidden row is never walked.
    #[test]
    fn forget_completing_clamps_the_cursor() {
        let mut st = AppState::new(0, 0, 1.0);
        let mut s = Settings::default();
        let mut scr = BluetoothScreen::new();
        st.ble_paired = true;
        run(&mut scr, &mut st, &mut s, Gesture::Step(1));
        assert_eq!(scr.selected, FORGET);
        st.ble_paired = false; // the host drained the forget; the bond is gone
        assert!(!scr.selection_is_guarded(false), "no fill on a hidden row");
        run(&mut scr, &mut st, &mut s, Gesture::Step(1));
        assert_eq!(scr.selected, TOGGLE, "the step lands on the one remaining row");
    }

    /// The status line: the rider's switch wins (Off the moment the toggle flips), else the live
    /// link phase from the seam.
    #[test]
    fn status_line_prefers_the_switch_then_the_link() {
        let on = Settings::default();
        let off = Settings { ble_enabled: false, ..Settings::default() };
        let en = Language::En;
        assert_eq!(status_label(&off, BleLink::Connected, en), "Off", "the switch wins even mid-drop");
        assert_eq!(status_label(&on, BleLink::Off, en), "Off", "the radio's own Off reads Off too");
        assert_eq!(status_label(&on, BleLink::Advertising, en), "Advertising");
        assert_eq!(status_label(&on, BleLink::Connected, en), "Connected");
    }
}
