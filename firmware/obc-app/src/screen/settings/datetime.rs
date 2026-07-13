//! The Date & Time screen. A `GPS clock` slider switches between two row sets:
//!
//! - **Manual** (GPS off): a `DATE` row (year / month / day) and a `TIME` row (hour : minute).
//! - **GPS** (GPS on): GPS supplies UTC, so the stamp is locked and only the `UTC offset` is yours —
//!   turning it shifts the *local* time (UTC + offset). `GPS fix` and `Local time` are read-only info
//!   rows the cursor skips.

use core::fmt::Write;

use embedded_graphics::{prelude::Point, primitives::Rectangle};
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::screen::{palette, title_frame, Ctx, Render, Transition, LIST_TOP};
use crate::settings::{DateTimeEditorExt, Language, Settings, UTC_OFFSET_MAX, UTC_OFFSET_MIN, UTC_OFFSET_STEP};
use crate::{t, Msg};

/// One row of the Date & Time screen. The set in play depends on `GPS clock`; the two info rows
/// are present only in GPS mode and are never the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowKind {
    /// `GPS clock` — the mode toggle (always row 0).
    Toggle,
    /// `DATE` year/month/day steppers (manual only).
    Date,
    /// `TIME` hour:minute steppers (manual only).
    Time,
    /// Read-only GPS fix status (GPS only).
    GpsFix,
    /// Read-only local time = UTC + offset (GPS only).
    LocalTime,
    /// `UTC offset` stepper (GPS only).
    Offset,
}

impl RowKind {
    /// Whether the cursor can land here — the info rows are display-only and skipped.
    fn selectable(self) -> bool {
        !matches!(self, RowKind::GpsFix | RowKind::LocalTime)
    }

    /// How many editable fields this row's stepper has (0 = a toggle / info row).
    fn fields(self) -> u8 {
        match self {
            RowKind::Date => 3,
            RowKind::Time => 2,
            RowKind::Offset => 1,
            _ => 0,
        }
    }

    /// Row height (px). The Date/Time rows are tall (a caption over big steppers with arrow clearance).
    fn height(self) -> i32 {
        match self {
            RowKind::Date | RowKind::Time => 78,
            RowKind::Offset => 56,
            RowKind::GpsFix | RowKind::LocalTime => 48,
            RowKind::Toggle => 46,
        }
    }
}

const MANUAL_ROWS: [RowKind; 3] = [RowKind::Toggle, RowKind::Date, RowKind::Time];
const GPS_ROWS: [RowKind; 4] = [RowKind::Toggle, RowKind::GpsFix, RowKind::LocalTime, RowKind::Offset];

/// The row set in play for the current mode.
fn rows(gps_time: bool) -> &'static [RowKind] {
    if gps_time {
        &GPS_ROWS
    } else {
        &MANUAL_ROWS
    }
}

/// The Date & Time screen. `selected` indexes the current row set; `editing` is the open field's
/// index within the selected row, or `None` for row-level focus.
#[derive(Debug, Default)]
pub struct DateTimeScreen {
    selected: usize,
    editing: Option<u8>,
}

impl DateTimeScreen {
    pub fn new() -> Self {
        DateTimeScreen { selected: 0, editing: None }
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        let rows = rows(cx.settings.gps_time);
        let kind = rows[self.selected.min(rows.len() - 1)];
        match g {
            Gesture::Turn(n) => {
                match self.editing {
                    Some(f) => step_field(cx.settings, kind, f, n),
                    None => self.move_selection(n, rows),
                }
                Transition::None
            }
            Gesture::Press => match kind {
                // The toggle flips the mode (and so the row set); stay on it, drop any edit.
                RowKind::Toggle => {
                    cx.settings.gps_time = !cx.settings.gps_time;
                    self.selected = 0;
                    self.editing = None;
                    Transition::None
                }
                // A value row: enter its stepper, then step field→field, then off the end → out.
                RowKind::Date | RowKind::Time | RowKind::Offset => {
                    self.editing = match self.editing {
                        None => Some(0),
                        Some(f) if (f + 1) < kind.fields() => Some(f + 1),
                        Some(_) => None,
                    };
                    Transition::None
                }
                RowKind::GpsFix | RowKind::LocalTime => Transition::None,
            },
            // Back steps out of an open field first, else exits to the Settings list (edits are
            // already live, so this is the implicit save).
            Gesture::Back => {
                if self.editing.is_some() {
                    self.editing = None;
                    Transition::None
                } else {
                    Transition::Pop
                }
            }
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    /// Move the row cursor by `n` detents, skipping the non-selectable info rows.
    fn move_selection(&mut self, n: i32, rows: &[RowKind]) {
        let len = rows.len() as i32;
        let dir = n.signum();
        let mut i = self.selected as i32;
        for _ in 0..n.unsigned_abs() {
            // Step at least one, then keep going until a selectable row (at most one lap).
            for _ in 0..len {
                i = (i + dir).rem_euclid(len);
                if rows[i as usize].selectable() {
                    break;
                }
            }
        }
        self.selected = i as usize;
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        let (w, h) = (rx.w, rx.h);
        let s = rx.settings;
        let rows = rows(s.gps_time);
        let has_fix = rx.state.user_fix.is_some();
        title_frame(cv, w, h, rx.t(Msg::DatetimeTitle), "");
        let lang = s.language;

        let mut y = LIST_TOP + 4;
        for (i, &kind) in rows.iter().enumerate() {
            let rh = kind.height();
            let area = super::row_rect(y, w, rh);
            let selected = i == self.selected;
            let editing = if selected { self.editing } else { None };
            super::row_cursor(cv, area, selected, editing.is_some());

            match kind {
                RowKind::Toggle => {
                    super::row_label(cv, area, t(Msg::DatetimeGpsClock, lang), None);
                    super::toggle_slider(cv, area, s.gps_time);
                }
                RowKind::Date => draw_date(cv, area, s, editing, lang),
                RowKind::Time => draw_time(cv, area, s, editing, lang),
                RowKind::GpsFix => {
                    // The UTC anchor GPS supplies — fixed, independent of the offset.
                    let mut v: heapless::String<24> = heapless::String::new();
                    if has_fix {
                        let _ = write!(v, "{}{:02}:{:02}", t(Msg::DatetimeUtc, lang), s.clock.hour, s.clock.minute);
                    } else {
                        let _ = v.push_str(t(Msg::DatetimeSearching, lang));
                    }
                    info_row(cv, area, t(Msg::DatetimeGpsFix, lang), &v);
                }
                RowKind::LocalTime => {
                    // The offset can carry across midnight, so take the whole local stamp (date and
                    // time) from `local_clock`, not the raw UTC date beside an offset-shifted hour.
                    let local = s.local_clock();
                    let mut v: heapless::String<24> = heapless::String::new();
                    let _ = write!(
                        v,
                        "{} {} {}  {:02}:{:02}",
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
                    super::stepper_field(cv, cell, &fmt_offset(s.utc_offset_min), editing == Some(0), Font::Label);
                }
            }
            // Hairline separators with a wider gap so they clear a selected row's amber bar,
            // grouping the clock source apart from the editable values.
            let sep = matches!(kind, RowKind::Toggle | RowKind::Date | RowKind::LocalTime);
            if sep {
                cv.hline(20, y + rh + 7, w - 40, palette::RULE);
            }
            y += rh + if sep { 15 } else { 4 };
        }
    }
}

/// Apply a stepper turn to the `field`-th field of `kind` (live into [`Settings`]).
fn step_field(s: &mut Settings, kind: RowKind, field: u8, n: i32) {
    match (kind, field) {
        (RowKind::Date, 0) => s.clock.step_year(n),
        (RowKind::Date, 1) => s.clock.step_month(n),
        (RowKind::Date, 2) => s.clock.step_day(n),
        (RowKind::Time, 0) => s.clock.step_hour(n),
        (RowKind::Time, 1) => s.clock.step_minute(n),
        (RowKind::Offset, 0) => {
            let v = s.utc_offset_min as i32 + n * UTC_OFFSET_STEP as i32;
            s.utc_offset_min = v.clamp(UTC_OFFSET_MIN as i32, UTC_OFFSET_MAX as i32) as i16;
        }
        _ => {}
    }
}

/// Draw a Date/Time row: a left caption above a centred group of big (Body) stepper cells. The
/// `cells` are `(text, width)` pairs laid out left→right with `gap` between; `active` is the open
/// field's index (or `None`). Shared by [`draw_date`] and [`draw_time`].
fn draw_stepper_row(
    cv: &mut impl Surface,
    area: Rectangle,
    label: &str,
    cells: &[(&str, i32)],
    gap: i32,
    active: Option<u8>,
) {
    let top = area.top_left.y;
    cv.text(label, Point::new(area.top_left.x + 12, top + 2), Font::Label, TextAlign::Left, palette::SUBTEXT);
    let ch = 32;
    let cy = top + 34;
    let total: i32 = cells.iter().map(|c| c.1).sum::<i32>() + gap * (cells.len() as i32 - 1);
    let mut x = area.top_left.x + (area.size.width as i32 - total) / 2;
    for (idx, &(text, cw)) in cells.iter().enumerate() {
        super::stepper_field(cv, rect(x, cy, cw, ch), text, active == Some(idx as u8), Font::Body);
        x += cw + gap;
    }
}

/// Draw the `DATE` row: `DATE` over year / month / day Body steppers, captioned in `lang`.
fn draw_date(cv: &mut impl Surface, area: Rectangle, s: &Settings, editing: Option<u8>, lang: Language) {
    let (mut yr, mut mo, mut da) =
        (heapless::String::<8>::new(), heapless::String::<8>::new(), heapless::String::<8>::new());
    let _ = write!(yr, "{}", s.clock.year);
    let _ = mo.push_str(crate::settings::month_name(s.clock, lang));
    let _ = write!(da, "{}", s.clock.day);
    // The month cell is 70 px (5 glyphs at Font::Body's 14 px), not 56: the four-char French months
    // (`août`, `sept`, `févr`) exactly fill 56 px, sitting flush against the active cell's amber
    // border. 70 px keeps a one-glyph margin either side, matching the year cell (#614).
    draw_stepper_row(cv, area, t(Msg::DatetimeDate, lang), &[(&yr, 70), (&mo, 70), (&da, 44)], 8, editing);
}

/// Draw the `TIME` row: `TIME` over hour : minute Body steppers, captioned in `lang`.
fn draw_time(cv: &mut impl Surface, area: Rectangle, s: &Settings, editing: Option<u8>, lang: Language) {
    let (mut hh, mut mm) = (heapless::String::<8>::new(), heapless::String::<8>::new());
    let _ = write!(hh, "{:02}", s.clock.hour);
    let _ = write!(mm, "{:02}", s.clock.minute);
    // The colon is drawn as a cell so the layout centres the whole "HH : MM" group.
    draw_stepper_row(cv, area, t(Msg::DatetimeTime, lang), &[(&hh, 58), (":", 16), (&mm, 58)], 4, time_active(editing));
}

/// Map the Time row's two editable fields (hour, minute) onto the three-cell layout (hour, colon,
/// minute) so the colon cell is never the active one.
fn time_active(editing: Option<u8>) -> Option<u8> {
    match editing {
        Some(0) => Some(0), // hour
        Some(1) => Some(2), // minute (skip the colon cell at index 1)
        _ => None,
    }
}

/// A read-only info row (no cursor): a muted caption stacked over its value, both left-aligned.
fn info_row(cv: &mut impl Surface, area: Rectangle, label: &str, value: &str) {
    let x = area.top_left.x + 10;
    cv.text(label, Point::new(x, area.top_left.y + 2), Font::Label, TextAlign::Left, palette::SUBTEXT);
    cv.text(value, Point::new(x, area.top_left.y + 24), Font::Label, TextAlign::Left, palette::INK);
}

/// Format a UTC offset as `±HH:MM`.
fn fmt_offset(min: i16) -> heapless::String<8> {
    let mut s = heapless::String::new();
    let sign = if min < 0 { '-' } else { '+' };
    let a = min.unsigned_abs();
    let _ = write!(s, "{sign}{:02}:{:02}", a / 60, a % 60);
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Activity;
    use crate::{AppState, Mode, Settings};

    /// Drive one gesture through the screen against a real `Settings`, returning the transition.
    fn run(scr: &mut DateTimeScreen, s: &mut Settings, g: Gesture) -> Transition {
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

    /// Flipping `GPS clock` swaps the row set; the cursor then skips the two read-only info rows,
    /// landing on `UTC offset`, and wraps back to the toggle (offset is the last selectable row).
    #[test]
    fn gps_toggle_swaps_rows_and_cursor_skips_info_rows() {
        let mut s = Settings::default();
        let mut scr = DateTimeScreen::new();
        assert!(!s.gps_time);
        run(&mut scr, &mut s, Gesture::Press);
        assert!(s.gps_time, "press on the toggle row enabled GPS time");
        assert_eq!(scr.selected, 0, "still on the toggle row after the flip");
        // GPS rows = [Toggle, GpsFix, LocalTime, Offset]; one detent skips the info rows to Offset.
        run(&mut scr, &mut s, Gesture::Turn(1));
        assert_eq!(scr.selected, 3, "cursor skipped the info rows to UTC offset");
        // Offset is the last selectable row, so the next detent wraps back to the toggle.
        run(&mut scr, &mut s, Gesture::Turn(1));
        assert_eq!(scr.selected, 0, "wraps past the end back to the toggle");
    }

    /// Manual edit flow: rotate to DATE, press to open the year field, rotate to change it, press
    /// to advance field→field, and press off the last field to step out.
    #[test]
    fn manual_field_edit_advances_and_steps_out() {
        let mut s = Settings::default();
        let y0 = s.clock.year;
        let mut scr = DateTimeScreen::new();
        run(&mut scr, &mut s, Gesture::Turn(1)); // toggle → DATE
        assert_eq!(scr.selected, 1);
        run(&mut scr, &mut s, Gesture::Press); // open the year field
        assert_eq!(scr.editing, Some(0));
        run(&mut scr, &mut s, Gesture::Turn(1)); // bump the year
        assert_eq!(s.clock.year, y0 + 1, "rotating the open field edits it live");
        run(&mut scr, &mut s, Gesture::Press); // → month
        assert_eq!(scr.editing, Some(1));
        run(&mut scr, &mut s, Gesture::Press); // → day
        assert_eq!(scr.editing, Some(2));
        run(&mut scr, &mut s, Gesture::Press); // off the last field → row focus
        assert_eq!(scr.editing, None);
    }

    /// Back steps out of an open field first (handled in place), then exits to the Settings list —
    /// there's no Save button; back is the implicit save.
    #[test]
    fn back_steps_out_then_exits() {
        let mut s = Settings::default();
        let mut scr = DateTimeScreen::new();
        run(&mut scr, &mut s, Gesture::Turn(1)); // → DATE
        run(&mut scr, &mut s, Gesture::Press); // open year
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::None));
        assert_eq!(scr.editing, None, "back closed the field without leaving the screen");
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::Pop), "back again exits");
    }

    /// The UTC offset shifts the local time in the Local time row, not the UTC anchor.
    #[test]
    fn offset_shifts_local_time_not_utc() {
        let mut s = Settings { gps_time: true, ..Settings::default() };
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

    /// The Local time row's date rolls with the offset across midnight, not the raw UTC date beside
    /// an offset-shifted hour.
    #[test]
    fn local_time_date_rolls_across_midnight() {
        let mut s = Settings { gps_time: true, ..Settings::default() };
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
