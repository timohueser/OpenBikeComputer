//! The Firmware page (epic #615 S5, #620) — reached from the System menu's *Firmware update* row.
//! It carries the SD-sideload firmware update door plus the read-only device-info ledger.
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
use crate::screen::vocab::chrome::{title_frame, LIST_TOP};
use crate::screen::{palette, Ctx, DfuCheckScreen, Render, Screen, Transition};
use crate::Msg;

use crate::screen::vocab::rows::ROW_X;

/// The Firmware page. Stateless beyond being on the stack — the one row is always the selection.
#[derive(Debug, Default)]
pub struct FirmwareScreen {}

impl FirmwareScreen {
    pub fn new() -> Self {
        FirmwareScreen {}
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            // Install update from card: refused (greyed) while recording — the arm reboots the
            // device. Otherwise post the scan one-shot and open the "Checking card..." wait; the
            // board answers through `App::apply_event`, which swaps the wait for the
            // confirm screen or an error card.
            Gesture::Press if !cx.activity.is_tracking() => {
                cx.activity.request_dfu(crate::activity::DfuAction::Scan);
                Transition::Push(Screen::DfuCheck(DfuCheckScreen::new()))
            }
            Gesture::Back => Transition::Pop, // climb back to the System menu
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, rx.t(Msg::FirmwareTitle), "");

        let recording = rx.activity.is_tracking();
        let label = rx.t(Msg::FirmwareInstallUpdate);
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
            cv.text(rx.t(Msg::FirmwareRecording), Point::new(ROW_X + 10, y), Font::Label, TextAlign::Left, SUBTEXT);
            y += 26;
        }

        // A hairline rule sets the read-only device-info ledger apart from the action above.
        y += 6;
        cv.hline(20, y, w - 40, palette::RULE);

        // The device-info ledger as three **stacked caption-over-value blocks** (the Date & Time
        // info-row visual language) spread evenly down the rest of the panel — so the page reads
        // structured and fills the height rather than bunching under the button. A value keeps its
        // own full-width line, so a long firmware tag or map name never has to fight a caption for
        // room (the never-ellipsize rule). No serial numbers, no uptime.
        let fw = if rx.fw_version.is_empty() { "--" } else { rx.fw_version };
        let mut map_val: heapless::String<32> = heapless::String::new();
        if rx.map_name.is_empty() {
            let _ = map_val.push_str("--");
        } else {
            let _ = write!(map_val, "{} \u{00b7} v{}", rx.map_name, rx.map_obcm_version);
        }
        let mut free_val: heapless::String<16> = heapless::String::new();
        match rx.card_free_bytes {
            Some(bytes) => fmt_bytes(&mut free_val, bytes),
            None => {
                let _ = free_val.push_str("--");
            }
        }
        let blocks: [(&str, &str); 3] = [
            (rx.t(Msg::FirmwareVersion), fw),
            (rx.t(Msg::FirmwareMap), &map_val),
            (rx.t(Msg::FirmwareCardFree), &free_val),
        ];
        let info_top = y + 22;
        // Distribute the blocks so the last value lands near the panel bottom (gaps between the
        // n captions), clamped so a greyed "Recording" cue above can't crush them together.
        let pitch = ((h - 52 - info_top) / (blocks.len() as i32 - 1)).clamp(44, 68);
        for (i, (cap, val)) in blocks.iter().enumerate() {
            let by = info_top + i as i32 * pitch;
            cv.text(cap, Point::new(20, by), Font::Label, TextAlign::Left, SUBTEXT);
            cv.text(val, Point::new(20, by + 24), Font::Body, TextAlign::Left, INK);
        }
    }
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
    use crate::screen::test_ctx;
    use crate::settings::Settings;
    use crate::{AppState, Mode};

    fn run(scr: &mut FirmwareScreen, act: &mut Activity, g: Gesture) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut settings = Settings::default();
        let mut cx = test_ctx(&mut st, act, &mut settings);
        scr.handle(g, &mut cx)
    }

    /// Not recording: pressing Install posts the scan one-shot and opens the "Checking card..." wait.
    #[test]
    fn press_posts_scan_and_opens_the_check_wait() {
        let mut act = Activity::new(Mode::Idle);
        let mut scr = FirmwareScreen::new();
        let t = run(&mut scr, &mut act, Gesture::Press);
        assert!(matches!(t, Transition::Push(Screen::DfuCheck(_))), "opens the scan wait");
        assert_eq!(act.take_dfu_request(), Some(DfuAction::Scan), "and posts a Scan request");
    }

    /// Recording: the row is disabled — a press does nothing and posts nothing (the arm reboots).
    #[test]
    fn press_is_a_no_op_while_recording() {
        let mut act = Activity::new(Mode::Riding);
        act.start_session(); // is_tracking() ⇒ true
        let mut scr = FirmwareScreen::new();
        let t = run(&mut scr, &mut act, Gesture::Press);
        assert!(matches!(t, Transition::None), "disabled while recording");
        assert_eq!(act.take_dfu_request(), None, "and nothing is posted");
    }

    /// Back climbs to the System menu.
    #[test]
    fn back_pops_to_system() {
        let mut act = Activity::new(Mode::Idle);
        let mut scr = FirmwareScreen::new();
        assert!(matches!(run(&mut scr, &mut act, Gesture::Back), Transition::Pop));
    }
}
