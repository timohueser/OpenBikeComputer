use core::fmt::Write;

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{text_width, Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::screen::vocab::chrome::{title_frame, LIST_TOP};
use crate::screen::vocab::list::{scrollbar, window_start};
use crate::screen::vocab::rows::{row_cursor, row_rect, ROW_X};
use crate::screen::{Ctx, Render, Screen, Transition};
use crate::settings::{STAT_CYCLE_MAX, STAT_CYCLE_MIN};
use crate::Msg;

use super::StatFieldsScreen;

/// Row height. It fits a label and a sub-caption, with the value on the right.
const ROW_H: i32 = 58;
/// Row pitch: the height plus the gap.
const PITCH: i32 = ROW_H + 6;
const TOP: i32 = LIST_TOP + 8;
/// Rows the panel shows at once. The other rows scroll into view.
const VISIBLE: usize = 4;

/// Inset of the shared value column from the right edge. Every row right-aligns its value here.
const VAL_INSET: i32 = 12;
/// Gap between a press-to-cycle cue and the value to its right.
const CUE_GAP: i32 = 6;

const DATA_FIELDS: usize = 0;
const PAGE_CYCLE: usize = 1;
const CLIMB: usize = 2;
const WAYPOINTS: usize = 3;
const ROWS: usize = 4;

/// Step the page-cycle period by `n` seconds, clamped to the configured bounds.
fn step_cycle(v: u16, n: i32) -> u16 {
    (v as i32 + n).clamp(STAT_CYCLE_MIN as i32, STAT_CYCLE_MAX as i32) as u16
}
/// `editing_cycle` is set only while the page-cycle stepper is open. The other rows navigate, or
/// cycle a value in place.
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
                    self.selected = crate::screen::vocab::list::step_selection(self.selected, n, ROWS);
                }
                Transition::None
            }
            Gesture::Press => match self.selected {
                DATA_FIELDS => Transition::Push(Screen::StatFields(StatFieldsScreen::new())),
                PAGE_CYCLE => {
                    self.editing_cycle = !self.editing_cycle;
                    Transition::None
                }
                // A small choice cycles in place. There is no edit sub-mode.
                CLIMB => {
                    cx.settings.climb_mode = cx.settings.climb_mode.cycled();
                    Transition::None
                }
                WAYPOINTS => {
                    cx.settings.waypoint_mode = cx.settings.waypoint_mode.cycled();
                    Transition::None
                }
                _ => Transition::None,
            },
            Gesture::Back => super::back_out_of_field(self.editing_cycle, || self.editing_cycle = false),
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use crate::screen::palette::*;
        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, rx.t(Msg::RideTitle), "");

        // One value column: every row right-aligns its value here.
        let val_r = w - ROW_X - VAL_INSET;
        let first = window_start(self.selected, VISIBLE, ROWS);

        for slot in 0..VISIBLE {
            let idx = first + slot;
            if idx >= ROWS {
                break;
            }
            let y = TOP + slot as i32 * PITCH;
            let row = row_rect(y, w, ROW_H);
            let selected = idx == self.selected;

            match idx {
                DATA_FIELDS => {
                    row_cursor(cv, row, selected, false);
                    super::row_label(cv, row, rx.t(Msg::RideFields), Some(rx.t(Msg::RideFieldsSub)));
                    chevron(cv, val_r, &row);
                }
                PAGE_CYCLE => {
                    let editing = self.editing_cycle && selected;
                    row_cursor(cv, row, selected, editing);
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
                    row_cursor(cv, row, selected, false);
                    super::row_label(cv, row, rx.t(Msg::RideClimb), Some(rx.t(Msg::RideClimbSub)));
                    draw_subline_cycle_value(cv, &row, val_r, rx.settings.climb_mode.name(rx.settings.language));
                }
                WAYPOINTS => {
                    row_cursor(cv, row, selected, false);
                    super::row_label(cv, row, rx.t(Msg::RideWaypoints), Some(rx.t(Msg::RideWaypointsSub)));
                    draw_subline_cycle_value(cv, &row, val_r, rx.settings.waypoint_mode.name(rx.settings.language));
                }
                _ => {}
            }
        }

        scrollbar(cv, w - 8, TOP, VISIBLE as i32 * PITCH, ROWS, first, VISIBLE);
    }
}

/// A chevron on the value column. It shows the row opens a sub-screen.
fn chevron(cv: &mut impl Surface, val_r: i32, row: &embedded_graphics::primitives::Rectangle) {
    use crate::screen::palette::INK;
    let cx0 = val_r - 11;
    let midy = row.top_left.y + row.size.height as i32 / 2;
    cv.triangle(Point::new(cx0, midy - 9), Point::new(cx0, midy + 9), Point::new(cx0 + 11, midy), INK);
}

/// Draw a press-to-cycle row's `value` at the right of its sub-caption line, with the cue a fixed
/// [`CUE_GAP`] before the value. The Climb and Waypoints rows share this, so they cannot drift apart.
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
    let tmid = sub_y + Font::Label.cap_mid() as i32;
    cv.triangle(Point::new(ax, tmid - 6), Point::new(ax, tmid + 6), Point::new(ax - 8, tmid), INK);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Activity;
    use crate::screen::test_ctx;
    use crate::{AppState, Mode, Settings};

    fn run(scr: &mut RideScreen, s: &mut Settings, g: Gesture) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut act = Activity::new(Mode::Idle);
        let mut cx = test_ctx(&mut st, &mut act, s);
        scr.handle(g, &mut cx)
    }

    #[test]
    fn data_fields_opens_its_page_from_the_first_row() {
        let mut s = Settings::default();
        let mut scr = RideScreen::new();
        assert_eq!(scr.selected, DATA_FIELDS, "cursor starts on Data fields");
        assert!(matches!(run(&mut scr, &mut s, Gesture::Press), Transition::Push(Screen::StatFields(_))));
    }

    #[test]
    fn cycle_stepper_edits_live_and_clamps() {
        let mut s = Settings { stat_cycle_s: 5, ..Settings::default() };
        let mut scr = RideScreen::new();
        run(&mut scr, &mut s, Gesture::Step(1));
        assert_eq!(scr.selected, PAGE_CYCLE);
        run(&mut scr, &mut s, Gesture::Press);
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

    #[test]
    fn climb_row_cycles_the_mode() {
        use crate::settings::ClimbMode;
        let mut s = Settings { climb_mode: ClimbMode::Auto, ..Settings::default() };
        let mut scr = RideScreen::new();
        run(&mut scr, &mut s, Gesture::Step(2));
        assert_eq!(scr.selected, CLIMB);
        for expect in [ClimbMode::Off, ClimbMode::Manual, ClimbMode::Auto] {
            assert!(matches!(run(&mut scr, &mut s, Gesture::Press), Transition::None));
            assert_eq!(s.climb_mode, expect);
        }
    }

    #[test]
    fn waypoint_row_cycles_the_mode() {
        use crate::settings::WaypointMode;
        let mut s = Settings { waypoint_mode: WaypointMode::Approach, ..Settings::default() };
        let mut scr = RideScreen::new();
        run(&mut scr, &mut s, Gesture::Step(3));
        assert_eq!(scr.selected, WAYPOINTS);
        for expect in [WaypointMode::Always, WaypointMode::Off, WaypointMode::Approach] {
            assert!(matches!(run(&mut scr, &mut s, Gesture::Press), Transition::None));
            assert_eq!(s.waypoint_mode, expect);
        }
    }
    #[test]
    fn back_closes_stepper_first() {
        let mut s = Settings::default();
        let mut scr = RideScreen::new();
        run(&mut scr, &mut s, Gesture::Step(1));
        run(&mut scr, &mut s, Gesture::Press);
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::None));
        assert!(!scr.editing_cycle, "back closed the stepper, not the screen");
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::Pop), "back again exits");
    }

    #[test]
    fn every_row_is_reachable_inside_the_scrolling_window() {
        let mut s = Settings::default();
        let mut scr = RideScreen::new();
        for expect in 0..ROWS {
            assert_eq!(scr.selected, expect, "one step per row, no gaps");
            let first = window_start(scr.selected, VISIBLE, ROWS);
            assert!(
                (first..first + VISIBLE).contains(&scr.selected),
                "row {expect} must be inside the drawn window {first}..{}",
                first + VISIBLE
            );
            run(&mut scr, &mut s, Gesture::Step(1));
        }
        assert_eq!(scr.selected, 0, "and the cursor wraps over all five rows");
    }

    /// The sub-caption must clear the cue and value by 8 px in every language. This mirrors the
    /// draw math.
    #[test]
    fn cycle_row_value_clears_the_sub_caption() {
        use crate::i18n::t;
        use crate::settings::{ClimbMode, Language, WaypointMode};
        const W: i32 = 240;
        const MIN_CLEAR: i32 = 8;
        let val_r = W - ROW_X - VAL_INSET;
        let lw = |s: &str| text_width(s, Font::Label) as i32;
        for lang in [Language::En, Language::De, Language::Fr, Language::Es] {
            let rows: [(&str, &[&str]); 2] = [
                (
                    t(Msg::RideClimbSub, lang),
                    &[ClimbMode::Off.name(lang), ClimbMode::Manual.name(lang), ClimbMode::Auto.name(lang)],
                ),
                (
                    t(Msg::RideWaypointsSub, lang),
                    &[WaypointMode::Off.name(lang), WaypointMode::Approach.name(lang), WaypointMode::Always.name(lang)],
                ),
            ];
            for (sub, values) in rows {
                let sub_right = ROW_X + 10 + lw(sub);
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
