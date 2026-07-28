//! The Ride screen — everything you tune for a ride, in one group. Two rows open their own rich
//! pages (**Bike type**, the routing-profile hero picker; **Data fields**, the
//! [`StatFields`](super::StatFieldsScreen) grid editor); the rest are simple controls edited in
//! place: **Page cycle** (a stepper for how fast the [`Statistics`](crate::screen) grid auto-flips),
//! **Climb** / **Waypoints** (press-to-cycle mode rows), and **Auto-delete** (the synced-ride
//! retention ring — Never / 1 day / 1 week / 1 month, moved here from its old standalone page).
//!
//! Six rows overrun the ~4-row panel, so this is the one settings screen that **scrolls**: the row
//! cursor drives the window ([`window_start`](crate::screen::list::window_start)) exactly like the
//! nav lists, and a scrollbar tracks the position. The two-level Select model is otherwise the
//! shared one — a step moves the cursor (or edits an open stepper), a press opens a page / flips a
//! cycle / toggles the Page-cycle stepper.

use core::fmt::Write;

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{text_width, Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::retention::RideRetention;
use crate::screen::list::{scrollbar, window_start};
use crate::screen::{title_frame, BikeTypeScreen, Ctx, Render, Screen, Transition, LIST_TOP};
use crate::settings::{STAT_CYCLE_MAX, STAT_CYCLE_MIN};
use crate::Msg;

use super::StatFieldsScreen;

/// Row height — fits a main label + sub-caption with the stepper / chevron / cycle value on the
/// right. The rows keep the Stats screen's family so the grid editor opens from a matching face.
const ROW_H: i32 = 58;
/// Row pitch (height + gap) — the window slots step by this.
const PITCH: i32 = ROW_H + 6;
/// Top of the first row slot.
const TOP: i32 = LIST_TOP + 8;
/// Rows the panel shows at once (the other rows scroll into view).
const VISIBLE: usize = 4;

/// Inset of the shared value column from the panel's right edge — every row right-aligns its value
/// here (T8 item 3).
const VAL_INSET: i32 = 12;
/// Fixed gap between a press-to-cycle `◄` cue and the value to its right (never glued to the word).
const CUE_GAP: i32 = 6;

const BIKE_TYPE: usize = 0;
const DATA_FIELDS: usize = 1;
const PAGE_CYCLE: usize = 2;
const CLIMB: usize = 3;
const WAYPOINTS: usize = 4;
const AUTODELETE: usize = 5;
const ROWS: usize = 6;

/// Step the page-cycle period by `n` steps (1 s each), clamped to the configured bounds.
fn step_cycle(v: u16, n: i32) -> u16 {
    (v as i32 + n).clamp(STAT_CYCLE_MIN as i32, STAT_CYCLE_MAX as i32) as u16
}

/// The catalog key for a [`RideRetention`] value — the Auto-delete row's cycle label.
fn retention_msg(r: RideRetention) -> Msg {
    match r {
        RideRetention::Never => Msg::AutodeleteNever,
        RideRetention::Day1 => Msg::AutodeleteDay1,
        RideRetention::Week1 => Msg::AutodeleteWeek1,
        RideRetention::Month1 => Msg::AutodeleteMonth1,
    }
}

/// The Ride screen. `selected` is the highlighted row; `editing_cycle` is set only while the
/// page-cycle stepper is open (every other row either navigates or cycles a value in place).
#[derive(Debug, Default)]
pub struct RideScreen {
    selected: usize,
    editing_cycle: bool,
}

impl RideScreen {
    pub fn new() -> Self {
        RideScreen::default()
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Step(n) => {
                if self.editing_cycle {
                    cx.settings.stat_cycle_s = step_cycle(cx.settings.stat_cycle_s, n);
                } else {
                    self.selected = crate::screen::list::step_selection(self.selected, n, ROWS);
                }
                Transition::None
            }
            Gesture::Press => match self.selected {
                // Bike type → the routing-profile hero picker (its own page).
                BIKE_TYPE => Transition::Push(Screen::BikeType(BikeTypeScreen::new())),
                // Data fields → the panel-grid editor (its own page).
                DATA_FIELDS => Transition::Push(Screen::StatFields(StatFieldsScreen::new())),
                // The cycle row's single field: press toggles the stepper open/closed.
                PAGE_CYCLE => {
                    self.editing_cycle = !self.editing_cycle;
                    Transition::None
                }
                // Press cycles Off → Manual → Auto in place (a small choice, no edit sub-mode).
                CLIMB => {
                    cx.settings.climb_mode = cx.settings.climb_mode.cycled();
                    Transition::None
                }
                // Press cycles Off → Approach → Always in place, the twin of the Climb row.
                WAYPOINTS => {
                    cx.settings.waypoint_mode = cx.settings.waypoint_mode.cycled();
                    Transition::None
                }
                // Press cycles the synced-ride retention ring one forward (wraps at both ends), the
                // same in-place idiom the old Auto-delete page used.
                AUTODELETE => {
                    cx.settings.ride_retention = cx.settings.ride_retention.stepped(1);
                    Transition::None
                }
                _ => Transition::None,
            },
            // Back steps out of an open field first, else climbs to the Settings list.
            Gesture::Back => {
                if self.editing_cycle {
                    self.editing_cycle = false;
                    Transition::None
                } else {
                    Transition::Pop
                }
            }
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use crate::screen::palette::*;
        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, rx.t(Msg::RideTitle), "");

        // ONE value column (T8 item 3): every row's value right-aligns at this inset.
        let val_r = w - super::ROW_X - VAL_INSET;
        // The scrolling window: the cursor stays on screen, the window follows it.
        let first = window_start(self.selected, VISIBLE, ROWS);

        for slot in 0..VISIBLE {
            let idx = first + slot;
            if idx >= ROWS {
                break;
            }
            let y = TOP + slot as i32 * PITCH;
            let row = super::row_rect(y, w, ROW_H);
            let selected = idx == self.selected;

            match idx {
                BIKE_TYPE => {
                    super::row_cursor(cv, row, selected, false);
                    super::row_label(cv, row, rx.t(Msg::RideBikeType), None);
                    chevron(cv, val_r, &row);
                }
                DATA_FIELDS => {
                    super::row_cursor(cv, row, selected, false);
                    super::row_label(cv, row, rx.t(Msg::RideFields), Some(rx.t(Msg::RideFieldsSub)));
                    chevron(cv, val_r, &row);
                }
                PAGE_CYCLE => {
                    let editing = self.editing_cycle && selected;
                    super::row_cursor(cv, row, selected, editing);
                    super::row_label(cv, row, rx.t(Msg::RidePages), Some(rx.t(Msg::RidePagesSub)));
                    let mut val: heapless::String<8> = heapless::String::new();
                    let _ = write!(val, "{} s", rx.settings.stat_cycle_s);
                    if editing {
                        let (cw, ch) = (76, 32);
                        let cell = rect(val_r - cw, row.top_left.y + (ROW_H - ch) / 2, cw, ch);
                        super::stepper_field(cv, cell, &val, true, Font::Label);
                    } else {
                        cv.text_vcentered(&val, val_r, (row.top_left.y, ROW_H), Font::Label, TextAlign::Right, INK);
                    }
                }
                CLIMB => {
                    super::row_cursor(cv, row, selected, false);
                    super::row_label(cv, row, rx.t(Msg::RideClimb), Some(rx.t(Msg::RideClimbSub)));
                    draw_subline_cycle_value(cv, &row, val_r, rx.settings.climb_mode.name(rx.settings.language));
                }
                WAYPOINTS => {
                    super::row_cursor(cv, row, selected, false);
                    super::row_label(cv, row, rx.t(Msg::RideWaypoints), Some(rx.t(Msg::RideWaypointsSub)));
                    draw_subline_cycle_value(cv, &row, val_r, rx.settings.waypoint_mode.name(rx.settings.language));
                }
                AUTODELETE => {
                    // Auto-delete: the retention value rides on the sub-caption line with the same
                    // ◄ "press to cycle" cue as the Climb / Waypoints rows above.
                    super::row_cursor(cv, row, selected, false);
                    super::row_label(cv, row, rx.t(Msg::RideAutodelete), Some(rx.t(Msg::RideAutodeleteSub)));
                    draw_subline_cycle_value(cv, &row, val_r, rx.t(retention_msg(rx.settings.ride_retention)));
                }
                _ => {}
            }
        }

        scrollbar(cv, w - 8, TOP, VISIBLE as i32 * PITCH, ROWS, first, VISIBLE);
    }
}

/// A right-pointing chevron parked on the value column — the "enters a sub-screen" cue for the
/// Bike type and Data fields rows.
fn chevron(cv: &mut impl Surface, val_r: i32, row: &embedded_graphics::primitives::Rectangle) {
    use crate::screen::palette::INK;
    let cx0 = val_r - 11;
    let midy = row.top_left.y + row.size.height as i32 / 2;
    cv.triangle(Point::new(cx0, midy - 9), Point::new(cx0, midy + 9), Point::new(cx0 + 11, midy), INK);
}

/// Draw a press-to-cycle row's mode `value` at the right of its **sub-caption** line — Label tier,
/// ink, right-aligned on the shared `val_r` column — with the ◄ "press to change" cue a fixed
/// [`CUE_GAP`] before the value's left edge. The Climb and Waypoints rows share this so the two
/// cycle rows can't drift apart; `cycle_row_value_clears_the_sub_caption` pins the clearance.
fn draw_subline_cycle_value(
    cv: &mut impl Surface,
    row: &embedded_graphics::primitives::Rectangle,
    val_r: i32,
    value: &str,
) {
    use crate::screen::palette::INK;
    let sub_y = row.top_left.y + 30;
    cv.text(value, Point::new(val_r, sub_y), Font::Label, TextAlign::Right, INK);
    let ax = val_r - text_width(value, Font::Label) as i32 - CUE_GAP;
    let tmid = sub_y + Font::Label.cap_height() as i32 / 2;
    cv.triangle(Point::new(ax, tmid - 6), Point::new(ax, tmid + 6), Point::new(ax - 8, tmid), INK);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Activity;
    use crate::{AppState, Mode, Settings};

    fn run(scr: &mut RideScreen, s: &mut Settings, g: Gesture) -> Transition {
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
            waypoints: &[],
            corridor: &[],
            sensor_scan_hits: &[],
            now_ms: 0,
        };
        scr.handle(g, &mut cx)
    }

    /// Bike type and Data fields open their own pages; the cursor starts on Bike type.
    #[test]
    fn bike_type_and_fields_open_pages() {
        let mut s = Settings::default();
        let mut scr = RideScreen::new();
        assert_eq!(scr.selected, BIKE_TYPE, "cursor starts on Bike type");
        assert!(matches!(run(&mut scr, &mut s, Gesture::Press), Transition::Push(Screen::BikeType(_))));
        run(&mut scr, &mut s, Gesture::Step(1)); // → Data fields
        assert_eq!(scr.selected, DATA_FIELDS);
        assert!(matches!(run(&mut scr, &mut s, Gesture::Press), Transition::Push(Screen::StatFields(_))));
    }

    /// Press opens the page-cycle stepper; a step edits the period live and clamps; press steps out.
    #[test]
    fn cycle_stepper_edits_live_and_clamps() {
        let mut s = Settings { stat_cycle_s: 5, ..Settings::default() };
        let mut scr = RideScreen::new();
        run(&mut scr, &mut s, Gesture::Step(2)); // → Page cycle row
        assert_eq!(scr.selected, PAGE_CYCLE);
        run(&mut scr, &mut s, Gesture::Press); // open stepper
        assert!(scr.editing_cycle);
        run(&mut scr, &mut s, Gesture::Step(2));
        assert_eq!(s.stat_cycle_s, 7);
        run(&mut scr, &mut s, Gesture::Step(100));
        assert_eq!(s.stat_cycle_s, STAT_CYCLE_MAX, "clamps to the max");
        run(&mut scr, &mut s, Gesture::Step(-100));
        assert_eq!(s.stat_cycle_s, STAT_CYCLE_MIN, "and to the min");
        run(&mut scr, &mut s, Gesture::Press);
        assert!(!scr.editing_cycle, "press steps back out");
    }

    /// The Climb row cycles Off → Manual → Auto in place on each press (no edit sub-mode).
    #[test]
    fn climb_row_cycles_the_mode() {
        use crate::settings::ClimbMode;
        let mut s = Settings { climb_mode: ClimbMode::Auto, ..Settings::default() };
        let mut scr = RideScreen::new();
        run(&mut scr, &mut s, Gesture::Step(3)); // → Climb row
        assert_eq!(scr.selected, CLIMB);
        for expect in [ClimbMode::Off, ClimbMode::Manual, ClimbMode::Auto] {
            assert!(matches!(run(&mut scr, &mut s, Gesture::Press), Transition::None));
            assert_eq!(s.climb_mode, expect);
        }
    }

    /// The Waypoints row cycles Off → Approach → Always in place on each press.
    #[test]
    fn waypoint_row_cycles_the_mode() {
        use crate::settings::WaypointMode;
        let mut s = Settings { waypoint_mode: WaypointMode::Approach, ..Settings::default() };
        let mut scr = RideScreen::new();
        run(&mut scr, &mut s, Gesture::Step(4)); // → Waypoints row
        assert_eq!(scr.selected, WAYPOINTS);
        for expect in [WaypointMode::Always, WaypointMode::Off, WaypointMode::Approach] {
            assert!(matches!(run(&mut scr, &mut s, Gesture::Press), Transition::None));
            assert_eq!(s.waypoint_mode, expect);
        }
    }

    /// The Auto-delete row cycles the retention ring one forward per press (wrapping), writing the
    /// choice straight into `Settings` — the old standalone page's behaviour, now a row here.
    #[test]
    fn autodelete_row_cycles_retention() {
        let mut s = Settings { ride_retention: RideRetention::Never, ..Settings::default() };
        let mut scr = RideScreen::new();
        run(&mut scr, &mut s, Gesture::Step(5)); // → Auto-delete row (last)
        assert_eq!(scr.selected, AUTODELETE);
        for expect in [RideRetention::Day1, RideRetention::Week1, RideRetention::Month1, RideRetention::Never] {
            assert!(matches!(run(&mut scr, &mut s, Gesture::Press), Transition::None));
            assert_eq!(s.ride_retention, expect, "a press cycles to the next value and persists it");
        }
    }

    /// Back closes an open stepper before it pops — the staged escape.
    #[test]
    fn back_closes_stepper_first() {
        let mut s = Settings::default();
        let mut scr = RideScreen::new();
        run(&mut scr, &mut s, Gesture::Step(2)); // → Page cycle
        run(&mut scr, &mut s, Gesture::Press); // open the stepper
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::None));
        assert!(!scr.editing_cycle, "back closed the stepper, not the screen");
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::Pop), "back again exits");
    }

    /// The two press-to-cycle rows' sub-caption lines clear their ◄value group by a **measured**
    /// ≥ 8 px in every language and mode value (owner review round 1). Mirrors the draw math.
    #[test]
    fn cycle_row_value_clears_the_sub_caption() {
        use crate::i18n::t;
        use crate::settings::{ClimbMode, Language, WaypointMode};
        const W: i32 = 240;
        const MIN_CLEAR: i32 = 8;
        let val_r = W - super::super::ROW_X - VAL_INSET;
        let lw = |s: &str| text_width(s, Font::Label) as i32;
        for lang in [Language::En, Language::De, Language::Fr, Language::Es] {
            let rows: [(&str, &[&str]); 3] = [
                (
                    t(Msg::RideClimbSub, lang),
                    &[ClimbMode::Off.name(lang), ClimbMode::Manual.name(lang), ClimbMode::Auto.name(lang)],
                ),
                (
                    t(Msg::RideWaypointsSub, lang),
                    &[WaypointMode::Off.name(lang), WaypointMode::Approach.name(lang), WaypointMode::Always.name(lang)],
                ),
                (
                    t(Msg::RideAutodeleteSub, lang),
                    &[
                        t(Msg::AutodeleteNever, lang),
                        t(Msg::AutodeleteDay1, lang),
                        t(Msg::AutodeleteWeek1, lang),
                        t(Msg::AutodeleteMonth1, lang),
                    ],
                ),
            ];
            for (sub, values) in rows {
                let sub_right = super::super::ROW_X + 10 + lw(sub);
                for value in values {
                    let cue_tip = val_r - lw(value) - CUE_GAP - 8;
                    assert!(
                        cue_tip - sub_right >= MIN_CLEAR,
                        "{lang:?}: sub-caption {sub:?} (ends {sub_right}) too close to \
                         cue+value {value:?} (cue tip {cue_tip}) — clearance {} < {MIN_CLEAR}",
                        cue_tip - sub_right
                    );
                }
            }
        }
    }
}
