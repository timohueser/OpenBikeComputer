//! The Firmware page: the update door for a card sideload, and the read-only device-info ledger.
//!
//! The one action posts a scan request and opens the check wait. The row is disabled while a ride
//! records, because the install ends in a reboot. The label word-wraps, because the translations
//! are too long for one line.

use core::fmt::Write;

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::screen::vocab::chrome::{title_frame, wrapped_aligned, wrapped_line_pitch, wrapped_lines, LIST_TOP};
use crate::screen::vocab::fmt::write_bytes_short;
use crate::screen::{palette, Ctx, DfuCheckScreen, Render, Screen, Transition};
use crate::Msg;

use crate::screen::vocab::rows::ROW_X;

#[derive(Debug, Default)]
pub struct FirmwareScreen {}

impl FirmwareScreen {
    pub fn new() -> Self {
        FirmwareScreen {}
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            // The install is refused while recording, because it reboots the device.
            Gesture::Press if !cx.recorder.recording() => {
                cx.dfu.admit_intent(crate::dfu::DfuIntent::ScanRequested);
                Transition::Push(Screen::DfuCheck(DfuCheckScreen::new()))
            }
            Gesture::Back => Transition::Pop,
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, rx.t(Msg::FirmwareTitle), "");

        let recording = rx.recording;
        let label = rx.t(Msg::FirmwareInstallUpdate);
        let inner_w = w - 2 * ROW_X - 20;
        let lines = wrapped_lines(label, inner_w, Font::Label);
        let row_h = (lines * wrapped_line_pitch(Font::Label) + 20).max(46);
        let row = rect(ROW_X, LIST_TOP, w - 2 * ROW_X, row_h);

        if !recording {
            cv.round(row, 6, AMBER);
        }
        let color = if recording { SUBTEXT } else { INK };
        wrapped_aligned(cv, label, ROW_X + 10, LIST_TOP + 12, inner_w, Font::Label, TextAlign::Left, color);

        let mut y = LIST_TOP + row_h + 10;
        if recording {
            cv.text(rx.t(Msg::FirmwareRecording), Point::new(ROW_X + 10, y), Font::Label, TextAlign::Left, SUBTEXT);
            y += 26;
        }

        // The rule sets the read-only ledger apart from the action above.
        y += 6;
        cv.hline(20, y, w - 40, palette::RULE);

        // Each value keeps its own full-width line, so a long firmware tag or map name still fits.
        let fw = if rx.fw_version.is_empty() { "--" } else { rx.fw_version };
        let mut map_val: heapless::String<32> = heapless::String::new();
        if rx.map_name.is_empty() {
            let _ = map_val.push_str("--");
        } else {
            let _ = write!(map_val, "{} \u{00b7} v{}", rx.map_name, rx.map_obcm_version);
        }
        let mut free_val: heapless::String<16> = heapless::String::new();
        match rx.card_free_bytes {
            Some(bytes) => write_bytes_short(&mut free_val, bytes),
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
        // Spread the blocks down the panel, but keep a minimum pitch so a Recording cue above
        // cannot crush them together.
        let pitch = ((h - 52 - info_top) / (blocks.len() as i32 - 1)).clamp(44, 68);
        for (i, (cap, val)) in blocks.iter().enumerate() {
            let by = info_top + i as i32 * pitch;
            cv.text(cap, Point::new(20, by), Font::Label, TextAlign::Left, SUBTEXT);
            cv.text(val, Point::new(20, by + 24), Font::Body, TextAlign::Left, INK);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::{Activity, DfuAction};
    use crate::screen::test_ctx;
    use crate::settings::Settings;
    use crate::{AppState, Mode};

    fn run(
        scr: &mut FirmwareScreen,
        act: &mut Activity,
        rec: &mut crate::RecorderMachine,
        dfu: &mut crate::dfu::DfuState,
        g: Gesture,
    ) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut settings = Settings::default();
        let mut cx = Ctx { dfu, recorder: rec, ..test_ctx(&mut st, act, &mut settings) };
        scr.handle(g, &mut cx)
    }

    fn drained_dfu(dfu: &mut crate::dfu::DfuState) -> Option<DfuAction> {
        dfu.next_effect().map(|effect| match effect {
            crate::dfu::DfuEffect::Scan { .. } => DfuAction::Scan,
            crate::dfu::DfuEffect::ArmInstall { .. } => DfuAction::Install,
        })
    }

    #[test]
    fn press_posts_scan_and_opens_the_check_wait() {
        let mut rec = crate::RecorderMachine::new();
        let mut act = Activity::new(Mode::Idle);
        let mut scr = FirmwareScreen::new();
        let mut dfu = crate::dfu::DfuState::new();
        let t = run(&mut scr, &mut act, &mut rec, &mut dfu, Gesture::Press);
        assert!(matches!(t, Transition::Push(Screen::DfuCheck(_))), "opens the scan wait");
        assert_eq!(drained_dfu(&mut dfu), Some(DfuAction::Scan), "and posts a Scan request");
    }

    #[test]
    fn press_is_a_no_op_while_recording() {
        let mut rec = crate::RecorderMachine::new();
        let mut act = Activity::new(Mode::Riding);
        rec.test_open();
        let mut scr = FirmwareScreen::new();
        let mut dfu = crate::dfu::DfuState::new();
        let t = run(&mut scr, &mut act, &mut rec, &mut dfu, Gesture::Press);
        assert!(matches!(t, Transition::None), "disabled while recording");
        assert_eq!(drained_dfu(&mut dfu), None, "and nothing is posted");
    }

    #[test]
    fn back_pops_to_system() {
        let mut act = Activity::new(Mode::Idle);
        let mut rec = crate::RecorderMachine::new();
        let mut scr = FirmwareScreen::new();
        assert!(matches!(
            run(&mut scr, &mut act, &mut rec, &mut crate::dfu::DfuState::new(), Gesture::Back),
            Transition::Pop
        ));
    }
}
