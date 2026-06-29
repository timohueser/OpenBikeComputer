//! The Date & Time screen — the richest of the settings screens and the one the mock's input
//! flow is built around. A `Set from GPS` toggle switches between two row sets:
//!
//! - **Manual** (GPS off): `DATE` (year / month / day) and `TIME` (hour : minute) steppers, then
//!   `Save & exit`. Rotate moves the row cursor; press opens a value row's `▲▼` stepper on its
//!   first field; press steps field→field; back steps out.
//! - **GPS** (GPS on): the stamp is locked, so only `UTC offset` is editable. `GPS fix` and
//!   `Local time` are read-only info rows the cursor **skips** over.
//!
//! Edits are live (see the [module docs](super)); `Save & exit` is just a `Pop`.

use core::fmt::Write;

use embedded_graphics::{
    prelude::{DrawTarget, Point},
    primitives::Rectangle,
};
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Canvas, RenderStats,
};

use crate::input::Gesture;
use crate::screen::{palette, title_frame, Ctx, Render, Transition, LIST_TOP};
use crate::settings::{Settings, UTC_OFFSET_MAX, UTC_OFFSET_MIN, UTC_OFFSET_STEP};

/// One row of the Date & Time screen. The set in play depends on `Set from GPS`; the two info
/// rows are present only in GPS mode and are never the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowKind {
    /// `Set from GPS` — the mode toggle (always row 0).
    Toggle,
    /// `DATE` year/month/day steppers (manual only).
    Date,
    /// `TIME` hour:minute steppers (manual only).
    Time,
    /// Read-only GPS fix status (GPS only).
    GpsFix,
    /// Read-only derived local time (GPS only).
    LocalTime,
    /// `UTC offset` stepper (GPS only).
    Offset,
    /// `Save & exit` action.
    Save,
}

impl RowKind {
    /// Whether the cursor can land here — the info rows are display-only and skipped.
    fn selectable(self) -> bool {
        !matches!(self, RowKind::GpsFix | RowKind::LocalTime)
    }

    /// How many editable fields this row's stepper has (0 = a toggle / action / info row).
    fn fields(self) -> u8 {
        match self {
            RowKind::Date => 3,
            RowKind::Time => 2,
            RowKind::Offset => 1,
            _ => 0,
        }
    }

    /// Row height (px) — the steppers are taller to clear the `▲▼` arrows; the info rows are
    /// tall enough to stack a caption over its value.
    fn height(self) -> i32 {
        match self {
            RowKind::Date | RowKind::Time => 58,
            RowKind::Offset => 52,
            RowKind::GpsFix | RowKind::LocalTime => 48,
            RowKind::Toggle | RowKind::Save => 46,
        }
    }
}

const MANUAL_ROWS: [RowKind; 4] = [RowKind::Toggle, RowKind::Date, RowKind::Time, RowKind::Save];
const GPS_ROWS: [RowKind; 5] = [RowKind::Toggle, RowKind::GpsFix, RowKind::LocalTime, RowKind::Offset, RowKind::Save];

/// The row set in play for the current mode.
fn rows(gps_time: bool) -> &'static [RowKind] {
    if gps_time {
        &GPS_ROWS
    } else {
        &MANUAL_ROWS
    }
}

/// The Date & Time screen. `selected` indexes the current row set; `editing` is the open
/// field's index within the selected row, or `None` for row-level focus.
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
                // Save & exit: edits are already live, so this is just the climb out.
                RowKind::Save => Transition::Pop,
                RowKind::GpsFix | RowKind::LocalTime => Transition::None,
            },
            // Back steps out of an open field first, else climbs to the Settings list.
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

    pub fn draw<D, F>(&self, target: &mut D, rx: &mut Render, color_fn: &F) -> RenderStats
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        let (w, h) = (rx.w as i32, rx.h as i32);
        let s = rx.settings;
        let rows = rows(s.gps_time);
        let has_fix = rx.state.user_fix.is_some();
        let mut cv = Canvas::new(target, color_fn);
        // The mode is shown by the GPS-clock toggle row, so the title needs no MANUAL/GPS tag
        // (it would only collide with the long title on the 240 px bar).
        title_frame(&mut cv, w, h, "DATE & TIME", "");

        let mut y = LIST_TOP + 4;
        for (i, &kind) in rows.iter().enumerate() {
            let rh = kind.height();
            let area = super::row_rect(i as i32, y, w, rh);
            let selected = i == self.selected;
            let editing = if selected { self.editing } else { None };
            super::row_cursor(&mut cv, area, selected, editing.is_some());

            match kind {
                RowKind::Toggle => {
                    super::row_label(&mut cv, area, "GPS clock", None);
                    super::toggle_pill(&mut cv, area, s.gps_time);
                }
                RowKind::Date => draw_date(&mut cv, area, s, editing),
                RowKind::Time => draw_time(&mut cv, area, s, editing),
                RowKind::GpsFix => {
                    let mut v: heapless::String<24> = heapless::String::new();
                    if has_fix {
                        let _ = write!(v, "UTC {}", fmt_utc(s));
                    } else {
                        let _ = v.push_str("Searching for fix");
                    }
                    info_row(&mut cv, area, "GPS fix", &v);
                }
                RowKind::LocalTime => {
                    let mut v: heapless::String<24> = heapless::String::new();
                    let _ = write!(
                        v,
                        "{} {} {}  {:02}:{:02}",
                        s.clock.year,
                        s.clock.month_name(),
                        s.clock.day,
                        s.clock.hour,
                        s.clock.minute
                    );
                    info_row(&mut cv, area, "Local time", &v);
                }
                RowKind::Offset => {
                    super::row_label(&mut cv, area, "Offset", None);
                    let (cw, ch) = (72, 26);
                    let cell = rect(
                        area.top_left.x + area.size.width as i32 - cw - 8,
                        area.top_left.y + (area.size.height as i32 - ch) / 2,
                        cw,
                        ch,
                    );
                    super::stepper_field(&mut cv, cell, &fmt_offset(s.utc_offset_min), editing == Some(0));
                }
                RowKind::Save => {
                    cv.text(
                        "Save & exit",
                        Point::new(area.top_left.x + area.size.width as i32 / 2, area.top_left.y + 12),
                        Font::Body,
                        TextAlign::Center,
                        palette::INK,
                    );
                }
            }
            y += rh + 4;
        }
        RenderStats::default()
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

/// Draw the `DATE` row: a left `DATE` label + year / month / day stepper cells, right-aligned.
fn draw_date<D, F>(cv: &mut Canvas<D, F>, area: Rectangle, s: &Settings, editing: Option<u8>)
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    cv.text(
        "DATE",
        Point::new(area.top_left.x + 10, area.top_left.y + (area.size.height as i32 - 18) / 2),
        Font::Label,
        TextAlign::Left,
        palette::SUBTEXT,
    );
    let ch = 26;
    let cy = area.top_left.y + (area.size.height as i32 - ch) / 2;
    let right = area.top_left.x + area.size.width as i32 - 8;
    let (yw, mw, dw, gap) = (54, 42, 30, 5);
    let yx = right - yw - gap - mw - gap - dw;
    let (mut yr, mut mo, mut da) =
        (heapless::String::<8>::new(), heapless::String::<8>::new(), heapless::String::<8>::new());
    let _ = write!(yr, "{}", s.clock.year);
    let _ = mo.push_str(s.clock.month_name());
    let _ = write!(da, "{}", s.clock.day);
    super::stepper_field(cv, rect(yx, cy, yw, ch), &yr, editing == Some(0));
    super::stepper_field(cv, rect(yx + yw + gap, cy, mw, ch), &mo, editing == Some(1));
    super::stepper_field(cv, rect(yx + yw + gap + mw + gap, cy, dw, ch), &da, editing == Some(2));
}

/// Draw the `TIME` row: a left `TIME` label + hour : minute stepper cells, right-aligned.
fn draw_time<D, F>(cv: &mut Canvas<D, F>, area: Rectangle, s: &Settings, editing: Option<u8>)
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    cv.text(
        "TIME",
        Point::new(area.top_left.x + 10, area.top_left.y + (area.size.height as i32 - 18) / 2),
        Font::Label,
        TextAlign::Left,
        palette::SUBTEXT,
    );
    let ch = 26;
    let cy = area.top_left.y + (area.size.height as i32 - ch) / 2;
    let right = area.top_left.x + area.size.width as i32 - 8;
    let (cw, colon) = (44, 12);
    let hx = right - cw - colon - cw;
    let (mut hh, mut mm) = (heapless::String::<8>::new(), heapless::String::<8>::new());
    let _ = write!(hh, "{:02}", s.clock.hour);
    let _ = write!(mm, "{:02}", s.clock.minute);
    super::stepper_field(cv, rect(hx, cy, cw, ch), &hh, editing == Some(0));
    cv.text(":", Point::new(hx + cw + colon / 2, cy + 2), Font::Body, TextAlign::Center, palette::INK);
    super::stepper_field(cv, rect(hx + cw + colon, cy, cw, ch), &mm, editing == Some(1));
}

/// A read-only info row (no cursor): a muted caption stacked over its value, both left-aligned —
/// stacked rather than side-by-side because a long value (the date stamp, "Searching for fix")
/// would otherwise collide with the caption on the 240 px line.
fn info_row<D, F>(cv: &mut Canvas<D, F>, area: Rectangle, label: &str, value: &str)
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
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

/// The UTC time-of-day derived from the local clock and offset (`HH:MM`, wrapping within the
/// day — the date roll is elided in this compact status readout).
fn fmt_utc(s: &Settings) -> heapless::String<8> {
    let mut out = heapless::String::new();
    let local = s.clock.hour as i32 * 60 + s.clock.minute as i32;
    let utc = (local - s.utc_offset_min as i32).rem_euclid(24 * 60);
    let _ = write!(out, "{:02}:{:02}", utc / 60, utc % 60);
    out
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
        let mut cx = Ctx { state: &mut st, activity: &mut act, settings: s, routes: &[], now_ms: 0 };
        scr.handle(g, &mut cx)
    }

    /// Flipping `Set from GPS` swaps the row set; the cursor then skips the two read-only info
    /// rows, landing on `UTC offset` and then `Save` — never on `GPS fix` / `Local time`.
    #[test]
    fn gps_toggle_swaps_rows_and_cursor_skips_info_rows() {
        let mut s = Settings::default();
        let mut scr = DateTimeScreen::new();
        // Row 0 is the toggle in both modes; pressing it turns GPS time on.
        assert!(!s.gps_time);
        run(&mut scr, &mut s, Gesture::Press);
        assert!(s.gps_time, "press on the toggle row enabled GPS time");
        assert_eq!(scr.selected, 0, "still on the toggle row after the flip");
        // One detent down skips GpsFix (1) and LocalTime (2) to UTC offset (3)…
        run(&mut scr, &mut s, Gesture::Turn(1));
        assert_eq!(scr.selected, 3, "cursor skipped the info rows to UTC offset");
        // …and the next lands on Save (4).
        run(&mut scr, &mut s, Gesture::Turn(1));
        assert_eq!(scr.selected, 4, "and on to Save");
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

    /// Back steps out of an open field first (handled in place), then climbs to the Settings list.
    #[test]
    fn back_steps_out_then_climbs() {
        let mut s = Settings::default();
        let mut scr = DateTimeScreen::new();
        run(&mut scr, &mut s, Gesture::Turn(1)); // → DATE
        run(&mut scr, &mut s, Gesture::Press); // open year
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::None));
        assert_eq!(scr.editing, None, "back closed the field without leaving the screen");
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::Pop), "back again climbs out");
    }

    /// `Save & exit` is a plain `Pop` — edits were already applied live.
    #[test]
    fn save_pops() {
        let mut s = Settings::default(); // manual rows: [Toggle, Date, Time, Save]
        let mut scr = DateTimeScreen::new();
        for _ in 0..3 {
            run(&mut scr, &mut s, Gesture::Turn(1));
        }
        assert_eq!(scr.selected, 3, "on Save");
        assert!(matches!(run(&mut scr, &mut s, Gesture::Press), Transition::Pop));
    }
}
