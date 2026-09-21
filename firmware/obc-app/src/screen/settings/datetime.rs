//! Date and time from GPS or BLE, with an editable local UTC offset.

use core::fmt::Write;

use embedded_graphics::{prelude::Point, primitives::Rectangle};
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::screen::vocab::chrome::{title_frame, LIST_TOP};
use crate::screen::vocab::fmt::utc_offset;
use crate::screen::vocab::rows::{row_cursor, row_rect};
use crate::screen::{palette, Ctx, Render, Transition};
use crate::settings::{Settings, UTC_OFFSET_MAX, UTC_OFFSET_MIN, UTC_OFFSET_STEP};
use crate::{t, Msg};

/// One row of the Date & Time screen. Only [`Offset`](RowKind::Offset) is selectable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowKind {
    /// Read-only GPS fix status, which is the UTC anchor.
    GpsFix,
    /// Read-only local time, which is UTC plus the offset.
    LocalTime,
    /// The UTC offset stepper.
    Offset,
}

impl RowKind {
    fn selectable(self) -> bool {
        matches!(self, RowKind::Offset)
    }

    /// Row height in pixels. The stepper row is taller, for the arrows around its cell.
    fn height(self) -> i32 {
        match self {
            RowKind::Offset => 56,
            RowKind::GpsFix | RowKind::LocalTime => 48,
        }
    }
}

const ROWS: [RowKind; 3] = [RowKind::GpsFix, RowKind::LocalTime, RowKind::Offset];

fn first_selectable() -> usize {
    ROWS.iter().position(|k| k.selectable()).expect("the row set always has a selectable row")
}

/// `selected` indexes [`ROWS`] and is always a selectable row. `editing` holds the open field
/// index, or `None` for row-level focus.
#[derive(Debug)]
pub struct DateTimeScreen {
    selected: usize,
    editing: Option<u8>,
}

impl Default for DateTimeScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl DateTimeScreen {
    pub fn new() -> Self {
        DateTimeScreen { selected: first_selectable(), editing: None }
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        let kind = ROWS[self.selected.min(ROWS.len() - 1)];
        match g {
            Gesture::Step(n) => {
                match self.editing {
                    Some(_) => step_offset(cx.settings, n),
                    None => self.move_selection(n),
                }
                Transition::None
            }
            Gesture::Press => match kind {
                RowKind::Offset => {
                    self.editing = match self.editing {
                        None => Some(0),
                        Some(_) => None,
                    };
                    Transition::None
                }
                RowKind::GpsFix | RowKind::LocalTime => Transition::None,
            },
            Gesture::Back => super::back_out_of_field(self.editing.is_some(), || self.editing = None),
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    /// Move the row cursor by `n` steps and skip the read-only rows.
    fn move_selection(&mut self, n: i32) {
        let len = ROWS.len() as i32;
        let dir = n.signum();
        let mut i = self.selected as i32;
        for _ in 0..n.unsigned_abs() {
            // Step at least one row, then continue to the next selectable row, at most one lap.
            for _ in 0..len {
                i = (i + dir).rem_euclid(len);
                if ROWS[i as usize].selectable() {
                    break;
                }
            }
        }
        self.selected = i as usize;
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        let (w, h) = (rx.w, rx.h);
        let s = rx.settings;
        let has_fix = rx.state.user_fix.is_some();
        title_frame(cv, w, h, rx.t(Msg::DatetimeTitle), "");
        let lang = s.language;

        let mut y = LIST_TOP + 4;
        for (i, &kind) in ROWS.iter().enumerate() {
            let rh = kind.height();
            let area = row_rect(y, w, rh);
            let selected = i == self.selected;
            let editing = if selected { self.editing } else { None };
            row_cursor(cv, area, selected, editing.is_some());

            match kind {
                RowKind::GpsFix => {
                    let mut v: heapless::String<24> = heapless::String::new();
                    if has_fix {
                        let _ = write!(v, "{}{:02}:{:02}", t(Msg::DatetimeUtc, lang), s.clock.hour, s.clock.minute);
                    } else {
                        let _ = v.push_str(t(Msg::DatetimeSearching, lang));
                    }
                    info_row(cv, area, t(Msg::DatetimeGpsFix, lang), &v);
                }
                RowKind::LocalTime => {
                    // The offset can cross midnight, so take the date and the time from `local_clock`.
                    let local = s.local_clock();
                    let mut v: heapless::String<24> = heapless::String::new();
                    let _ = write!(
                        v,
                        "{} {} {} {:02}:{:02}",
                        local.year,
                        crate::settings::month_name(local, lang),
                        local.day,
                        local.hour,
                        local.minute
                    );
                    info_row(cv, area, t(Msg::DatetimeLocalTime, lang), &v);
                }
                RowKind::Offset => {
                    super::row_label(cv, area, t(Msg::DatetimeOffset, lang), None);
                    let (cw, ch) = (84, 32);
                    let cell = rect(
                        area.top_left.x + area.size.width as i32 - cw - 6,
                        area.top_left.y + (area.size.height as i32 - ch) / 2,
                        cw,
                        ch,
                    );
                    super::stepper_field(cv, cell, &utc_offset(s.utc_offset_min), editing == Some(0), Font::Label);
                }
            }
            // The separator groups the read-only clock source apart from the editable offset.
            let sep = matches!(kind, RowKind::LocalTime);
            if sep {
                cv.hline(20, y + rh + 7, w - 40, palette::RULE);
            }
            y += rh + if sep { 15 } else { 4 };
        }
    }
}

/// Apply a stepper step to the UTC offset, clamped to its range.
fn step_offset(s: &mut Settings, n: i32) {
    let v = s.utc_offset_min as i32 + n * UTC_OFFSET_STEP as i32;
    s.local_offset_known = true;
    s.utc_offset_min = v.clamp(UTC_OFFSET_MIN as i32, UTC_OFFSET_MAX as i32) as i16;
}

/// A read-only row: a muted caption over its value.
fn info_row(cv: &mut impl Surface, area: Rectangle, label: &str, value: &str) {
    let x = area.top_left.x + 10;
    cv.text(label, Point::new(x, area.top_left.y + 2), Font::Label, TextAlign::Left, palette::SUBTEXT);
    cv.text(value, Point::new(x, area.top_left.y + 24), Font::Label, TextAlign::Left, palette::INK);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Activity;
    use crate::screen::test_ctx;
    use crate::{AppState, Mode, Settings};

    fn run(scr: &mut DateTimeScreen, s: &mut Settings, g: Gesture) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut act = Activity::new(Mode::Idle);
        let mut cx = test_ctx(&mut st, &mut act, s);
        scr.handle(g, &mut cx)
    }

    #[test]
    fn cursor_parks_on_the_offset_row() {
        let mut s = Settings::default();
        let mut scr = DateTimeScreen::new();
        assert_eq!(ROWS[scr.selected], RowKind::Offset, "starts on the offset row");
        run(&mut scr, &mut s, Gesture::Step(1));
        assert_eq!(ROWS[scr.selected], RowKind::Offset, "a step finds no other selectable row");
        run(&mut scr, &mut s, Gesture::Step(-3));
        assert_eq!(ROWS[scr.selected], RowKind::Offset, "still parked after several steps");
    }

    #[test]
    fn offset_field_edits_and_steps_out() {
        let mut s = Settings { utc_offset_min: 0, ..Settings::default() };
        let mut scr = DateTimeScreen::new();
        run(&mut scr, &mut s, Gesture::Press);
        assert_eq!(scr.editing, Some(0));
        run(&mut scr, &mut s, Gesture::Step(2));
        assert_eq!(s.utc_offset_min, 2 * UTC_OFFSET_STEP, "rotating the open field edits it live");
        run(&mut scr, &mut s, Gesture::Press);
        assert_eq!(scr.editing, None);
    }

    #[test]
    fn offset_clamps_at_the_range_ends() {
        let mut s = Settings { utc_offset_min: UTC_OFFSET_MAX - UTC_OFFSET_STEP, ..Settings::default() };
        let mut scr = DateTimeScreen::new();
        run(&mut scr, &mut s, Gesture::Press);
        run(&mut scr, &mut s, Gesture::Step(5));
        assert_eq!(s.utc_offset_min, UTC_OFFSET_MAX, "clamps at the maximum offset");
    }

    #[test]
    fn back_steps_out_then_exits() {
        let mut s = Settings::default();
        let mut scr = DateTimeScreen::new();
        run(&mut scr, &mut s, Gesture::Press);
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::None));
        assert_eq!(scr.editing, None, "back closed the field without leaving the screen");
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::Pop), "back again exits");
    }

    #[test]
    fn offset_shifts_local_time_not_utc() {
        let mut s = Settings::default();
        s.clock.hour = 12;
        s.clock.minute = 0;
        s.utc_offset_min = 0;
        let local = s.local_clock();
        assert_eq!((local.hour, local.minute), (12, 0), "at +00:00 local matches the UTC anchor");
        s.utc_offset_min = 120; // +02:00
        let local = s.local_clock();
        assert_eq!((local.hour, local.minute), (14, 0), "offset moves local forward");
        assert_eq!((s.clock.hour, s.clock.minute), (12, 0), "the stored UTC anchor did not move");
    }

    #[test]
    fn local_time_date_rolls_across_midnight() {
        let mut s = Settings::default();
        s.clock.year = 2025;
        s.clock.month = 6;
        s.clock.day = 29;
        s.clock.hour = 23;
        s.clock.minute = 0;
        s.utc_offset_min = 120; // 23:00 UTC +02:00 → Jun 30 01:00 local
        let local = s.local_clock();
        assert_eq!((local.month, local.day, local.hour), (6, 30, 1), "forward offset advances the local date");
        s.clock.day = 29;
        s.clock.hour = 1;
        s.utc_offset_min = -120; // 01:00 UTC −02:00 → Jun 28 23:00 local
        let local = s.local_clock();
        assert_eq!((local.month, local.day, local.hour), (6, 28, 23), "backward offset rolls the local date back");
    }
}
