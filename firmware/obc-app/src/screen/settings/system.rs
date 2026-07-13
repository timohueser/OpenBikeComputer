//! The System settings screen (epic #615 S5, #620) — the door to the SD-sideload firmware update.
//!
//! One actionable row, "Install update from card": pressing it posts a
//! [`DfuAction::Scan`](crate::activity::DfuAction) and opens the "Checking card..." wait
//! ([`DfuCheckScreen`](crate::screen::DfuCheckScreen)); the board's answer replaces that with the
//! confirm screen or an error card. The row is **always visible** but **disabled while a ride is
//! recording** — the arm ends in a reboot, so a live ride would be lost (the same guard the board's
//! drain enforces). Disabled is the standard greyed treatment: dimmed label + a "Recording" cue,
//! and the press does nothing.
//!
//! The label is long (and the German/French/Spanish translations longer), so it word-wraps across
//! up to three [`Font::Label`] lines inside the amber row rather than truncating — the whole action
//! reads, in every language.

use core::fmt::Write;

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::screen::{palette, title_frame, Ctx, DfuCheckScreen, Render, Screen, Transition, LIST_TOP};
use crate::Msg;

use super::ROW_X;

/// The System settings screen. Stateless beyond being on the stack — the one row is always the
/// selection.
#[derive(Debug, Default)]
pub struct SystemScreen {}

impl SystemScreen {
    pub fn new() -> Self {
        SystemScreen {}
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            // Install update from card: refused (greyed) while recording — the arm reboots the
            // device. Otherwise post the scan one-shot and open the "Checking card..." wait; the
            // board answers through `App::notify_dfu_scan_result`, which swaps the wait for the
            // confirm screen or an error card.
            Gesture::Press if !cx.activity.is_tracking() => {
                cx.activity.request_dfu(crate::activity::DfuAction::Scan);
                Transition::Push(Screen::DfuCheck(DfuCheckScreen::new()))
            }
            Gesture::Back => Transition::Pop, // climb back to the Settings list
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, rx.t(Msg::SystemTitle), "");

        let recording = rx.activity.is_tracking();
        let label = rx.t(Msg::SystemInstallUpdate);
        let inner_w = w - 2 * ROW_X - 20;
        let lines = wrap_lines(label, inner_w);
        let row_h = (lines * 22 + 20).max(46);
        let row = rect(ROW_X, LIST_TOP, w - 2 * ROW_X, row_h);

        // Amber row cursor only when the action is live; a disabled row shows no box.
        if !recording {
            cv.round(row, 6, AMBER);
        }
        // The (possibly multi-line) label, wrapped left-aligned inside the row. Dimmed to olive
        // when disabled.
        let color = if recording { SUBTEXT } else { INK };
        draw_wrapped_label(cv, label, ROW_X + 10, LIST_TOP + 12, inner_w, color);

        // The disabled cue below the row, so the greyed state reads (the standard "Recording" cue).
        let mut y = LIST_TOP + row_h + 10;
        if recording {
            cv.text(rx.t(Msg::SystemRecording), Point::new(ROW_X + 10, y), Font::Label, TextAlign::Left, SUBTEXT);
            y += 28;
        }

        // The read-only device-info ledger (T8 item 6): three rows in the `ledger_row` visual
        // language (olive caption left, value right) but carrying text values — so a long firmware
        // tag wraps to a second line rather than ellipsizing (the never-ellipsize rule, family with
        // T9). No serial numbers, no uptime.
        y += 8;
        // Firmware — the running git-describe tag; `--` before the host feeds it.
        let fw = if rx.fw_version.is_empty() { "--" } else { rx.fw_version };
        y = ledger_info_row(cv, w, y, rx.t(Msg::SystemFirmware), fw);
        // Map — "name . vN" from the loaded map header; `--` with no map loaded.
        let mut map_val: heapless::String<32> = heapless::String::new();
        if rx.map_name.is_empty() {
            let _ = map_val.push_str("--");
        } else {
            let _ = write!(map_val, "{} \u{00b7} v{}", rx.map_name, rx.map_obcm_version);
        }
        y = ledger_info_row(cv, w, y, rx.t(Msg::SystemMap), &map_val);
        // Card free — human-readable SD free space; `--` until the on-entry FAT scan answers.
        let mut free_val: heapless::String<16> = heapless::String::new();
        match rx.card_free_bytes {
            Some(bytes) => fmt_bytes(&mut free_val, bytes),
            None => {
                let _ = free_val.push_str("--");
            }
        }
        ledger_info_row(cv, w, y, rx.t(Msg::SystemCardFree), &free_val);
    }
}

/// Draw one device-info ledger row (T8 item 6): the olive Label caption at the left inset and the
/// INK Body value right-aligned to `w - INSET`. A value that fits shares the caption's baseline; one
/// too wide to clear the caption wraps onto its own right-aligned line(s) below — char-wrapped on the
/// monospace cell budget (firmware tags have no spaces), **never ellipsized**. Returns the `y` just
/// past the row so the caller stacks the next.
fn ledger_info_row(cv: &mut impl Surface, w: i32, top_y: i32, caption: &str, value: &str) -> i32 {
    use palette::*;
    const INSET: i32 = 16;
    let char_w = Font::Body.char_width() as i32;
    cv.text(caption, Point::new(INSET, top_y + 4), Font::Label, TextAlign::Left, SUBTEXT);
    let cap_w = caption.chars().count() as i32 * Font::Label.char_width() as i32;
    let val_w = value.chars().count() as i32 * char_w;
    // Room for the value on the caption line: from just past the caption to the right inset.
    let same_line_budget = (w - INSET) - (INSET + cap_w + 8);
    if val_w <= same_line_budget {
        cv.text(value, Point::new(w - INSET, top_y + 2), Font::Body, TextAlign::Right, INK);
        return top_y + 32;
    }
    // Wrap the value onto right-aligned line(s) below the caption.
    let per_line = ((w - 2 * INSET) / char_w).max(1) as usize;
    let lh = Font::Body.cap_height() as i32 + 4;
    let mut y = top_y + 28;
    let mut line: heapless::String<48> = heapless::String::new();
    for ch in value.chars() {
        if line.chars().count() >= per_line {
            cv.text(&line, Point::new(w - INSET, y), Font::Body, TextAlign::Right, INK);
            y += lh;
            line.clear();
        }
        let _ = line.push(ch);
    }
    if !line.is_empty() {
        cv.text(&line, Point::new(w - INSET, y), Font::Body, TextAlign::Right, INK);
        y += lh;
    }
    y + 4
}

/// Format a byte count as a compact `N.N GB` / `NNN MB` / `NNN KB` string (T8 item 6) — GB with one
/// decimal at or above 1 GiB, whole MB / KB below (rounded).
fn fmt_bytes(s: &mut heapless::String<16>, bytes: u64) {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;
    if bytes >= GIB {
        let tenths = (bytes * 10 + GIB / 2) / GIB; // round to 0.1 GB
        let _ = write!(s, "{}.{} GB", tenths / 10, tenths % 10);
    } else if bytes >= MIB {
        let _ = write!(s, "{} MB", (bytes + MIB / 2) / MIB);
    } else {
        let _ = write!(s, "{} KB", (bytes + KIB / 2) / KIB);
    }
}

/// Number of [`Font::Label`] lines `text` word-wraps into within `width_px` (greedy over the
/// monospace cell width). At least 1.
fn wrap_lines(text: &str, width_px: i32) -> i32 {
    let budget = (width_px / Font::Label.char_width() as i32).max(1) as usize;
    let mut lines = 1;
    let mut used = 0usize;
    for word in text.split(' ') {
        let extra = if used == 0 { word.len() } else { used + 1 + word.len() };
        if extra > budget && used != 0 {
            lines += 1;
            used = word.len();
        } else {
            used = extra;
        }
    }
    lines
}

/// Draw `text` word-wrapped into left-aligned [`Font::Label`] lines from `(x, top_y)` within
/// `width_px`, in `color`.
fn draw_wrapped_label(cv: &mut impl Surface, text: &str, x: i32, top_y: i32, width_px: i32, color: u16) {
    let budget = (width_px / Font::Label.char_width() as i32).max(1) as usize;
    let mut y = top_y;
    let mut line: heapless::String<64> = heapless::String::new();
    for word in text.split(' ') {
        let extra = if line.is_empty() { word.len() } else { line.len() + 1 + word.len() };
        if extra > budget && !line.is_empty() {
            cv.text(&line, Point::new(x, y), Font::Label, TextAlign::Left, color);
            y += 22;
            line.clear();
        }
        if !line.is_empty() {
            let _ = line.push(' ');
        }
        let _ = line.push_str(word);
    }
    if !line.is_empty() {
        cv.text(&line, Point::new(x, y), Font::Label, TextAlign::Left, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::{Activity, DfuAction};
    use crate::screen::PoiScratch;
    use crate::settings::Settings;
    use crate::{AppState, Mode};

    fn run(scr: &mut SystemScreen, act: &mut Activity, g: Gesture) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut settings = Settings::default();
        let scratch = PoiScratch::new();
        let mut cx = Ctx {
            state: &mut st,
            activity: act,
            settings: &mut settings,
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

    /// Not recording: pressing Install posts the scan one-shot and opens the "Checking card..." wait.
    #[test]
    fn press_posts_scan_and_opens_the_check_wait() {
        let mut act = Activity::new(Mode::Idle);
        let mut scr = SystemScreen::new();
        let t = run(&mut scr, &mut act, Gesture::Press);
        assert!(matches!(t, Transition::Push(Screen::DfuCheck(_))), "opens the scan wait");
        assert_eq!(act.take_dfu_request(), Some(DfuAction::Scan), "and posts a Scan request");
    }

    /// Recording: the row is disabled — a press does nothing and posts nothing (the arm reboots).
    #[test]
    fn press_is_a_no_op_while_recording() {
        let mut act = Activity::new(Mode::Riding);
        act.start_session(); // is_tracking() ⇒ true
        let mut scr = SystemScreen::new();
        let t = run(&mut scr, &mut act, Gesture::Press);
        assert!(matches!(t, Transition::None), "disabled while recording");
        assert_eq!(act.take_dfu_request(), None, "and nothing is posted");
    }

    /// Back climbs to the Settings list.
    #[test]
    fn back_pops_to_settings() {
        let mut act = Activity::new(Mode::Idle);
        let mut scr = SystemScreen::new();
        assert!(matches!(run(&mut scr, &mut act, Gesture::Back), Transition::Pop));
    }
}
