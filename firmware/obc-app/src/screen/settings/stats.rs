//! The Stats screen — the riding [`Statistics`](crate::screen) page's configuration. **Page cycle**
//! (how fast the grid auto-flips) is a stepper here; **Fields** opens the
//! [`StatFields`](super::StatFieldsScreen) sub-screen for the panel selection + order; **Climb**
//! cycles the [`ClimbMode`](crate::settings::ClimbMode) that governs the Climb screen (epic #506);
//! **Waypoints** cycles the [`WaypointMode`](crate::settings::WaypointMode) that governs the Map
//! waypoint chip (epic #523). The cycle period is kept out of the field list deliberately — mixed
//! among the panels it read as just another draggable row.

use core::fmt::Write;

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{text_width, Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::screen::{title_frame, Ctx, Render, Screen, Transition, LIST_TOP};
use crate::settings::{STAT_CYCLE_MAX, STAT_CYCLE_MIN};
use crate::Msg;

use super::StatFieldsScreen;

/// Row height — fits a main label + sub-caption with the stepper / chevron on the right. The label
/// is kept short ("Pages") so the big Body glyphs clear the value box.
const ROW_H: i32 = 58;

/// Inset of the shared value column from the panel's right edge — every row right-aligns its value
/// here (T8 item 3).
const VAL_INSET: i32 = 12;
/// Fixed gap between a press-to-cycle `◄` cue and the value to its right (never glued to the word).
const CUE_GAP: i32 = 6;

const PAGE_CYCLE: usize = 0;
const FIELDS: usize = 1;
const CLIMB_PANEL: usize = 2;
const WAYPOINT_PANEL: usize = 3;
const ROWS: usize = 4;

/// Step the page-cycle period by `n` detents (1 s each), clamped to the configured bounds.
fn step_cycle(v: u16, n: i32) -> u16 {
    (v as i32 + n).clamp(STAT_CYCLE_MIN as i32, STAT_CYCLE_MAX as i32) as u16
}

/// The Stats screen. `selected` is the highlighted row; `editing_cycle` is set only while the
/// page-cycle stepper is open (the Fields + Climb rows have no edit sub-mode — Fields navigates,
/// Climb cycles its value in place).
#[derive(Debug, Default)]
pub struct StatsScreen {
    selected: usize,
    editing_cycle: bool,
}

impl StatsScreen {
    pub fn new() -> Self {
        StatsScreen::default()
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Turn(n) => {
                if self.editing_cycle {
                    cx.settings.stat_cycle_s = step_cycle(cx.settings.stat_cycle_s, n);
                } else {
                    self.selected = crate::screen::list::step_selection(self.selected, n, ROWS);
                }
                Transition::None
            }
            Gesture::Press => match self.selected {
                // The cycle row's single field: press toggles the stepper open/closed.
                PAGE_CYCLE => {
                    self.editing_cycle = !self.editing_cycle;
                    Transition::None
                }
                // The Climb row: press cycles Off → Manual → Auto in place (a small three-way
                // choice, so no edit sub-mode — like the Units screen's flip).
                CLIMB_PANEL => {
                    cx.settings.climb_mode = cx.settings.climb_mode.cycled();
                    Transition::None
                }
                // The Waypoints row: press cycles Off → Approach → Always in place, the same
                // cycle-in-place idiom as the Climb row above.
                WAYPOINT_PANEL => {
                    cx.settings.waypoint_mode = cx.settings.waypoint_mode.cycled();
                    Transition::None
                }
                // Fields → the panel manager.
                _ => Transition::Push(Screen::StatFields(StatFieldsScreen::new())),
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
        title_frame(cv, w, h, rx.t(Msg::SetStatsTitle), "");

        // ONE value column (T8 item 3): every row's value right-aligns at this inset from the panel
        // edge, and each press-to-cycle `◄` sits a fixed [`CUE_GAP`] px before its value — so the four
        // ragged rows ("5 s" floating mid-row, `◄Auto`/`◄Approach` glued flush-right) line up.
        let val_r = w - super::ROW_X - VAL_INSET;

        // Row 0 — Page cycle (single-line value row with a stepper on the right).
        let r0 = super::row_rect(LIST_TOP + 8, w, ROW_H);
        let editing = self.editing_cycle && self.selected == PAGE_CYCLE;
        super::row_cursor(cv, r0, self.selected == PAGE_CYCLE, editing);
        super::row_label(cv, r0, rx.t(Msg::SetStatsPages), Some(rx.t(Msg::SetStatsPagesSub)));
        let mut val: heapless::String<8> = heapless::String::new();
        let _ = write!(val, "{} s", rx.settings.stat_cycle_s);
        if editing {
            // The open stepper keeps its box + centred text (a transient state with visible chrome,
            // allowed to sit off the column — the ▲▼ box *is* the cursor then).
            let (cw, ch) = (76, 32);
            let cell = rect(val_r - cw, r0.top_left.y + (ROW_H - ch) / 2, cw, ch);
            super::stepper_field(cv, cell, &val, true, Font::Label);
        } else {
            // Idle: the value itself right-aligns on the shared column — an inactive stepper_field
            // would centre it in an invisible box, floating it left of the other rows' values.
            cv.text_vcentered(&val, val_r, (r0.top_left.y, ROW_H), Font::Label, TextAlign::Right, INK);
        }

        // Row 1 — Fields (opens the panel manager).
        let r1 = super::row_rect(LIST_TOP + 8 + ROW_H + 6, w, ROW_H);
        super::row_cursor(cv, r1, self.selected == FIELDS, false);
        super::row_label(cv, r1, rx.t(Msg::SetStatsFields), Some(rx.t(Msg::SetStatsFieldsSub)));
        // A right-pointing chevron says "enters a sub-screen" — its tip parked on the value column so
        // it shares the rows' right edge.
        let cx0 = val_r - 11;
        let midy = r1.top_left.y + r1.size.height as i32 / 2;
        cv.triangle(Point::new(cx0, midy - 9), Point::new(cx0, midy + 9), Point::new(cx0 + 11, midy), INK);

        // Row 2 — Climb panel (press cycles Off / Manual / Auto in place). The mode rides at the
        // right of the **sub-caption** line — exactly the Waypoints row's shape below, so the two
        // press-to-cycle rows read as twins (owner review round 1: the old Body value vcentered on
        // the row ran into the sub-caption — "Auto still clips the climb panel text"). The measured
        // clearance between the sub-caption and the ◄ cue is pinned for all four languages by
        // `cycle_row_value_clears_the_sub_caption` below.
        let r2 = super::row_rect(LIST_TOP + 8 + 2 * (ROW_H + 6), w, ROW_H);
        super::row_cursor(cv, r2, self.selected == CLIMB_PANEL, false);
        super::row_label(cv, r2, rx.t(Msg::SetStatsClimb), Some(rx.t(Msg::SetStatsClimbSub)));
        let climb_name = rx.settings.climb_mode.name(rx.settings.language);
        draw_subline_cycle_value(cv, &r2, val_r, climb_name);

        // Row 3 — Waypoints panel (press cycles Off / Approach / Always in place): the same
        // value-on-the-sub-caption-line shape as the Climb row above — compact Label at the shared
        // value column, the ◄ "press to change" cue a fixed CUE_GAP before it. (The caption is
        // "chip", not "map chip": at Label width the 8-char caption + ◄ + the 8-char "Approach"
        // value can't all clear the 240 px row, and the ◄ affordance wins.)
        let r3 = super::row_rect(LIST_TOP + 8 + 3 * (ROW_H + 6), w, ROW_H);
        super::row_cursor(cv, r3, self.selected == WAYPOINT_PANEL, false);
        super::row_label(cv, r3, rx.t(Msg::SetStatsWaypoints), Some(rx.t(Msg::SetStatsWaypointsSub)));
        let name = rx.settings.waypoint_mode.name(rx.settings.language);
        draw_subline_cycle_value(cv, &r3, val_r, name);
    }
}

/// Draw a press-to-cycle row's mode `value` at the right of its **sub-caption** line — Label tier,
/// ink, right-aligned on the shared `val_r` column — with the ◄ "press to change" cue a fixed
/// [`CUE_GAP`] before the value's left edge. The Climb and Waypoints rows share this drawer so the
/// two cycle rows can't drift apart (owner review round 1). The sub-caption itself is drawn by
/// `row_label` at the same `top + 30` line; `cycle_row_value_clears_the_sub_caption` pins the
/// measured clearance between the two in every language.
fn draw_subline_cycle_value(cv: &mut impl Surface, row: &embedded_graphics::primitives::Rectangle, val_r: i32, value: &str) {
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

    fn run(scr: &mut StatsScreen, s: &mut Settings, g: Gesture) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut act = Activity::new(Mode::Idle);
        let scratch = crate::screen::PoiScratch::new();
        let mut cx = Ctx {
            state: &mut st,
            activity: &mut act,
            settings: s,
            routes: &[],
            rides: &[],
            nav_profiles: &crate::NavProfiles::EMPTY,
            poi_scratch: &scratch,
            now_ms: 0,
        };
        scr.handle(g, &mut cx)
    }

    /// Press opens the cycle stepper; a turn edits the period live and clamps; press steps back out.
    #[test]
    fn cycle_stepper_edits_live_and_clamps() {
        let mut s = Settings { stat_cycle_s: 5, ..Settings::default() };
        let mut scr = StatsScreen::new();
        run(&mut scr, &mut s, Gesture::Press); // cursor starts on Page cycle → open stepper
        assert!(scr.editing_cycle);
        run(&mut scr, &mut s, Gesture::Turn(2));
        assert_eq!(s.stat_cycle_s, 7);
        run(&mut scr, &mut s, Gesture::Turn(100));
        assert_eq!(s.stat_cycle_s, STAT_CYCLE_MAX, "clamps to the max");
        run(&mut scr, &mut s, Gesture::Turn(-100));
        assert_eq!(s.stat_cycle_s, STAT_CYCLE_MIN, "and to the min");
        run(&mut scr, &mut s, Gesture::Press);
        assert!(!scr.editing_cycle, "press steps back out");
    }

    /// The Fields row pushes the panel manager; Back from the bare screen climbs to Settings.
    #[test]
    fn fields_row_pushes_manager_and_back_pops() {
        let mut s = Settings::default();
        let mut scr = StatsScreen::new();
        run(&mut scr, &mut s, Gesture::Turn(1)); // → Fields row
        assert_eq!(scr.selected, FIELDS);
        assert!(matches!(run(&mut scr, &mut s, Gesture::Press), Transition::Push(Screen::StatFields(_))));
        run(&mut scr, &mut s, Gesture::Turn(-1)); // back to Page cycle row
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::Pop));
    }

    /// The Climb row cycles Off → Manual → Auto → Off in place on each press (no edit sub-mode).
    #[test]
    fn climb_row_cycles_the_mode() {
        use crate::settings::ClimbMode;
        let mut s = Settings { climb_mode: ClimbMode::Auto, ..Settings::default() };
        let mut scr = StatsScreen::new();
        run(&mut scr, &mut s, Gesture::Turn(2)); // → Climb row (0 → 1 → 2)
        assert_eq!(scr.selected, CLIMB_PANEL);
        // Auto → Off → Manual → Auto, one press each — and no navigation transition.
        for expect in [ClimbMode::Off, ClimbMode::Manual, ClimbMode::Auto] {
            assert!(matches!(run(&mut scr, &mut s, Gesture::Press), Transition::None));
            assert_eq!(s.climb_mode, expect);
        }
    }

    /// The Waypoints row cycles Off → Approach → Always → Off in place on each press (no edit
    /// sub-mode), the fourth row under Climb.
    #[test]
    fn waypoint_row_cycles_the_mode() {
        use crate::settings::WaypointMode;
        let mut s = Settings { waypoint_mode: WaypointMode::Approach, ..Settings::default() };
        let mut scr = StatsScreen::new();
        run(&mut scr, &mut s, Gesture::Turn(3)); // → Waypoints row (0 → 1 → 2 → 3)
        assert_eq!(scr.selected, WAYPOINT_PANEL);
        // Approach → Always → Off → Approach, one press each — and no navigation transition.
        for expect in [WaypointMode::Always, WaypointMode::Off, WaypointMode::Approach] {
            assert!(matches!(run(&mut scr, &mut s, Gesture::Press), Transition::None));
            assert_eq!(s.waypoint_mode, expect);
        }
    }

    /// The two press-to-cycle rows' sub-caption lines clear their ◄value group by a **measured**
    /// ≥ 8 px in every language and every mode value (owner review round 1: "the Auto still clips
    /// the climb panel text"). Mirrors the draw math exactly: the sub-caption runs left-aligned
    /// from `ROW_X + 10`; the value right-aligns on the shared column `w - ROW_X - VAL_INSET`, its
    /// ◄ cue reaching `CUE_GAP + 8` px further left.
    #[test]
    fn cycle_row_value_clears_the_sub_caption() {
        use crate::i18n::t;
        use crate::settings::{ClimbMode, Language, WaypointMode};
        const W: i32 = 240; // the panel width every layout constant is tuned for
        const MIN_CLEAR: i32 = 8;
        let val_r = W - super::super::ROW_X - VAL_INSET;
        let lw = |s: &str| text_width(s, Font::Label) as i32;
        for lang in [Language::En, Language::De, Language::Fr, Language::Es] {
            let rows: [(&str, &[&str]); 2] = [
                (
                    t(Msg::SetStatsClimbSub, lang),
                    &[
                        ClimbMode::Off.name(lang),
                        ClimbMode::Manual.name(lang),
                        ClimbMode::Auto.name(lang),
                    ],
                ),
                (
                    t(Msg::SetStatsWaypointsSub, lang),
                    &[
                        WaypointMode::Off.name(lang),
                        WaypointMode::Approach.name(lang),
                        WaypointMode::Always.name(lang),
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

    /// Back closes an open stepper before it pops — the staged escape.
    #[test]
    fn back_closes_stepper_first() {
        let mut s = Settings::default();
        let mut scr = StatsScreen::new();
        run(&mut scr, &mut s, Gesture::Press); // open the stepper
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::None));
        assert!(!scr.editing_cycle, "back closed the stepper, not the screen");
    }
}
