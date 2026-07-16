//! The Date & Time screen. GPS (and, after epic #638 S2, the phone over BLE) supplies UTC, so the
//! clock is never hand-set (#641 removed manual editing — a fat-fingered year must never reach the
//! auto-expiry sweep). Three rows, one of them editable:
//!
//! - `GPS fix` — read-only: the UTC anchor GPS supplies (or `Searching for fix`).
//! - `Local time` — read-only: local = UTC anchor + offset.
//! - `UTC offset` — the one stepper; turning it shifts the *displayed* local time only (expiry math
//!   is pure UTC). The two info rows are display-only and the cursor skips them.

use core::fmt::Write;

use embedded_graphics::{prelude::Point, primitives::Rectangle};
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::screen::{palette, title_frame, Ctx, Render, Transition, LIST_TOP};
use crate::settings::{Settings, UTC_OFFSET_MAX, UTC_OFFSET_MIN, UTC_OFFSET_STEP};
use crate::{t, Msg};

/// One row of the Date & Time screen. Only [`Offset`](RowKind::Offset) is selectable; the two info
/// rows are display-only and the cursor skips them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowKind {
    /// Read-only GPS fix status (the UTC anchor).
    GpsFix,
    /// Read-only local time = UTC + offset.
    LocalTime,
    /// `UTC offset` stepper (one field).
    Offset,
}

impl RowKind {
    /// Whether the cursor can land here — the info rows are display-only and skipped.
    fn selectable(self) -> bool {
        matches!(self, RowKind::Offset)
    }

    /// Row height (px). The stepper row is a touch taller for the arrow clearance around its cell.
    fn height(self) -> i32 {
        match self {
            RowKind::Offset => 56,
            RowKind::GpsFix | RowKind::LocalTime => 48,
        }
    }
}

/// The fixed row set: the two read-only clock-source rows, then the UTC-offset stepper.
const ROWS: [RowKind; 3] = [RowKind::GpsFix, RowKind::LocalTime, RowKind::Offset];

/// Index of the first selectable row — where the cursor parks (the info rows are skipped).
fn first_selectable() -> usize {
    ROWS.iter().position(|k| k.selectable()).expect("the row set always has a selectable row")
}

/// The Date & Time screen. `selected` indexes [`ROWS`] (always a selectable row); `editing` is the
/// open field's index within it, or `None` for row-level focus.
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
        // Park the cursor on the UTC-offset row — the only selectable one (the info rows are skipped).
        DateTimeScreen { selected: first_selectable(), editing: None }
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        let kind = ROWS[self.selected.min(ROWS.len() - 1)];
        match g {
            Gesture::Turn(n) => {
                match self.editing {
                    Some(_) => step_offset(cx.settings, n),
                    None => self.move_selection(n),
                }
                Transition::None
            }
            // The offset stepper has one field: enter it, then press again to step out. The info
            // rows are never selected, so a press on them can't happen — but stay put if it did.
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

    /// Move the row cursor by `n` detents, skipping the non-selectable info rows. With a single
    /// selectable row this keeps the cursor parked on it — kept general so adding a second stepper
    /// later just works.
    fn move_selection(&mut self, n: i32) {
        let len = ROWS.len() as i32;
        let dir = n.signum();
        let mut i = self.selected as i32;
        for _ in 0..n.unsigned_abs() {
            // Step at least one, then keep going until a selectable row (at most one lap).
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
            let area = super::row_rect(y, w, rh);
            let selected = i == self.selected;
            let editing = if selected { self.editing } else { None };
            super::row_cursor(cv, area, selected, editing.is_some());

            match kind {
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
            // A hairline separator with a wider gap (so it clears a selected row's amber bar) groups
            // the read-only clock source apart from the editable offset.
            let sep = matches!(kind, RowKind::LocalTime);
            if sep {
                cv.hline(20, y + rh + 7, w - 40, palette::RULE);
            }
            y += rh + if sep { 15 } else { 4 };
        }
    }
}

/// Apply a stepper turn to the UTC offset (live into [`Settings`], clamped to its range).
fn step_offset(s: &mut Settings, n: i32) {
    let v = s.utc_offset_min as i32 + n * UTC_OFFSET_STEP as i32;
    s.utc_offset_min = v.clamp(UTC_OFFSET_MIN as i32, UTC_OFFSET_MAX as i32) as i16;
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

    /// The cursor parks on the UTC-offset row (the only selectable one) and turning between rows
    /// keeps it there — the two read-only info rows are never the cursor.
    #[test]
    fn cursor_parks_on_the_offset_row() {
        let mut s = Settings::default();
        let mut scr = DateTimeScreen::new();
        assert_eq!(ROWS[scr.selected], RowKind::Offset, "starts on the offset row");
        run(&mut scr, &mut s, Gesture::Turn(1));
        assert_eq!(ROWS[scr.selected], RowKind::Offset, "a detent finds no other selectable row");
        run(&mut scr, &mut s, Gesture::Turn(-3));
        assert_eq!(ROWS[scr.selected], RowKind::Offset, "still parked after several detents");
    }

    /// Offset edit flow: press to open the single field, rotate to change it (live, `UTC_OFFSET_STEP`
    /// per detent), press again to step out.
    #[test]
    fn offset_field_edits_and_steps_out() {
        let mut s = Settings { utc_offset_min: 0, ..Settings::default() };
        let mut scr = DateTimeScreen::new();
        run(&mut scr, &mut s, Gesture::Press); // open the offset field
        assert_eq!(scr.editing, Some(0));
        run(&mut scr, &mut s, Gesture::Turn(2)); // +2 steps
        assert_eq!(s.utc_offset_min, 2 * UTC_OFFSET_STEP, "rotating the open field edits it live");
        run(&mut scr, &mut s, Gesture::Press); // step out (one field)
        assert_eq!(scr.editing, None);
    }

    /// The offset stepper clamps at the range ends rather than wrapping.
    #[test]
    fn offset_clamps_at_the_range_ends() {
        let mut s = Settings { utc_offset_min: UTC_OFFSET_MAX - UTC_OFFSET_STEP, ..Settings::default() };
        let mut scr = DateTimeScreen::new();
        run(&mut scr, &mut s, Gesture::Press);
        run(&mut scr, &mut s, Gesture::Turn(5)); // past the top
        assert_eq!(s.utc_offset_min, UTC_OFFSET_MAX, "clamps at the maximum offset");
    }

    /// Back steps out of an open field first (handled in place), then exits to the Settings list —
    /// there's no Save button; back is the implicit save.
    #[test]
    fn back_steps_out_then_exits() {
        let mut s = Settings::default();
        let mut scr = DateTimeScreen::new();
        run(&mut scr, &mut s, Gesture::Press); // open the offset field
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::None));
        assert_eq!(scr.editing, None, "back closed the field without leaving the screen");
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::Pop), "back again exits");
    }

    /// The UTC offset shifts the local time in the Local time row, not the UTC anchor.
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

    /// The Local time row's date rolls with the offset across midnight, not the raw UTC date beside
    /// an offset-shifted hour.
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
