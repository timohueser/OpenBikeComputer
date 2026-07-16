//! The Auto-delete screen (route auto-expiry + ride auto-delete, epic #638 S5). One stepper row —
//! **Synced rides**: how long a ride survives after it was verifiably synced to the phone before the
//! device deletes it (Never / 1 day / 1 week / 1 month, default 1 week). It reads and writes
//! [`Settings::ride_retention`](crate::Settings) live, exactly like the other single-setting picker
//! screens ([`Units`](super::UnitsScreen) / [`Language`](super::LanguageScreen)): there is no field
//! sub-mode, a *turn* walks the four values and a *press* cycles one forward, and leaving the screen
//! is the implicit save (the edit is already in [`Settings`], and
//! [`App::apply_gesture`](crate::App::apply_gesture) flags the host to persist it).
//!
//! Route retention is **not** here — it is per-object and app-controlled (locked in #638); the
//! device only surfaces it read-only in the Route overview's expiry row.

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::retention::RideRetention;
use crate::screen::{palette, title_frame, Ctx, Render, Transition, LIST_TOP};
use crate::Msg;

/// The Auto-delete screen. Stateless — the value lives in [`Settings::ride_retention`], and the one
/// stepper row is always the focused field (so the `▲▼` box always marks it, like the Units /
/// Language pickers are always their screen's cursor).
#[derive(Debug, Default)]
pub struct AutoDeleteScreen;

impl AutoDeleteScreen {
    pub fn new() -> Self {
        AutoDeleteScreen
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            // A short ring, so — like Units / Language — there's no separate edit mode: a turn walks
            // the four values in place and a press cycles one forward. `stepped` wraps at both ends.
            Gesture::Turn(n) => {
                cx.settings.ride_retention = cx.settings.ride_retention.stepped(n);
                Transition::None
            }
            Gesture::Press => {
                cx.settings.ride_retention = cx.settings.ride_retention.stepped(1);
                Transition::None
            }
            Gesture::Back => Transition::Pop,
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, rx.t(Msg::AutodeleteTitle), "");

        // A single centred setting: the "SYNCED RIDES" caption over the value's `▲▼` stepper cell.
        // The value strings are short (`1 month` is the widest), so a fixed 130 px cell holds every
        // language's value without crowding the arrows.
        let caption_y = LIST_TOP + 44;
        cv.text(
            rx.t(Msg::AutodeleteSyncedRides),
            Point::new(w / 2, caption_y),
            Font::Label,
            TextAlign::Center,
            palette::SUBTEXT,
        );

        let (cw, ch) = (130, 44);
        let cell = rect((w - cw) / 2, caption_y + 30, cw, ch);
        super::stepper_field(cv, cell, rx.t(retention_msg(rx.settings.ride_retention)), true, Font::Body);
    }
}

/// The catalog key for a [`RideRetention`] value — the stepper cell's displayed label.
fn retention_msg(r: RideRetention) -> Msg {
    match r {
        RideRetention::Never => Msg::AutodeleteNever,
        RideRetention::Day1 => Msg::AutodeleteDay1,
        RideRetention::Week1 => Msg::AutodeleteWeek1,
        RideRetention::Month1 => Msg::AutodeleteMonth1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Activity;
    use crate::{AppState, Mode, Settings};

    fn run(scr: &mut AutoDeleteScreen, s: &mut Settings, g: Gesture) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut act = Activity::new(Mode::Idle);
        let scratch = crate::screen::PoiScratch::new();
        let mut cx = Ctx {
            state: &mut st,
            activity: &mut act,
            settings: s,
            routes: &[],
            rides: &[],
            trips: &[],
            nav_profiles: &crate::NavProfiles::EMPTY,
            poi_scratch: &scratch,
            sensor_scan_hits: &[],
            now_ms: 0,
        };
        scr.handle(g, &mut cx)
    }

    /// A turn walks the four values in place and a press cycles one forward — both wrap through
    /// exactly Never → 1 day → 1 week → 1 month → Never, writing the choice straight into `Settings`.
    #[test]
    fn stepper_cycles_the_four_values_and_persists() {
        let mut s = Settings { ride_retention: RideRetention::Never, ..Settings::default() };
        let mut scr = AutoDeleteScreen::new();

        // A turn walks forward through the whole ring and wraps back to the start.
        for expect in [RideRetention::Day1, RideRetention::Week1, RideRetention::Month1, RideRetention::Never] {
            run(&mut scr, &mut s, Gesture::Turn(1));
            assert_eq!(s.ride_retention, expect, "a turn steps to the next value and persists it");
        }
        // A backward turn walks the ring the other way.
        run(&mut scr, &mut s, Gesture::Turn(-1));
        assert_eq!(s.ride_retention, RideRetention::Month1, "a backward turn wraps to the last value");

        // A press cycles one forward — from the last value it wraps to Never.
        run(&mut scr, &mut s, Gesture::Press);
        assert_eq!(s.ride_retention, RideRetention::Never, "a press cycles forward, wrapping past the end");

        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::Pop), "Back exits to the Settings list");
    }

    /// Every value maps to its own catalog key (no two share a label), so the stepper cell always
    /// reflects the live setting.
    #[test]
    fn every_value_has_a_distinct_label_key() {
        let keys = [
            retention_msg(RideRetention::Never) as usize,
            retention_msg(RideRetention::Day1) as usize,
            retention_msg(RideRetention::Week1) as usize,
            retention_msg(RideRetention::Month1) as usize,
        ];
        for (i, a) in keys.iter().enumerate() {
            for b in &keys[i + 1..] {
                assert_ne!(a, b, "each RideRetention value has a distinct label");
            }
        }
    }
}
