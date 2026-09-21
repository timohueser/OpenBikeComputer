//! The screens of the SD-sideload firmware-update flow, reached from Settings → System. Each is a
//! static card through the normal screen stack: the two waits, the install confirm, the terminal
//! "Installing update" card, the error card, and the two one-time boot-outcome cards.

use embedded_graphics::prelude::Point;
use obc_render::{
    text::{Font, TextAlign},
    Surface,
};

use crate::dfu::{DfuFailure, DfuInstallError, DfuScanError, DfuScanReport, Version};
use crate::input::Gesture;
use crate::Msg;

use super::vocab::card::{ActionRows, CardEvent};
use super::vocab::chrome::{self, card_check, card_triangle, title_frame, TITLE_BAR_H};
use super::vocab::rows::{GuardedRowsGeometry, MenuItem};
use super::vocab::spinner::Spinner;
use super::{palette, Ctx, Render, Screen, ScreenTick, Transition};

/// Draw a DFU wait screen: the title bar, the spinner needle, and the caption that names what the
/// board is doing.
fn wait_card(cv: &mut impl Surface, rx: &Render, spin: &Spinner, title: &str, caption: &str) {
    let (w, h) = (rx.w, rx.h);
    title_frame(cv, w, h, title, "");
    spin.draw_needle(cv, w, h);
    cv.text(caption, Point::new(w / 2, h * 72 / 100), Font::Label, TextAlign::Center, palette::INK);
}

/// Centred body copy at `Font::Label`. Each catalog string is authored on one line and wrapped
/// here, at draw time.
fn wrapped(cv: &mut impl Surface, text: &str, cx: i32, top_y: i32, width_px: i32, color: u16) -> i32 {
    chrome::wrapped(cv, text, cx, top_y, width_px, Font::Label, color)
}

/// The scan wait: the spinner over "Checking card..." until the board answers. Back cancels; a
/// later answer is then dropped.
#[derive(Debug, Default)]
pub struct DfuCheckScreen {
    spin: Spinner,
}

impl DfuCheckScreen {
    pub fn new() -> Self {
        DfuCheckScreen { spin: Spinner::default() }
    }

    pub fn handle(&mut self, g: Gesture, _cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Back => Transition::Pop,
            _ => Transition::None,
        }
    }

    pub fn tick_timers(&mut self, now_ms: u32, w: i32, h: i32) -> ScreenTick {
        self.spin.tick(now_ms, w, h)
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        wait_card(cv, rx, &self.spin, rx.t(Msg::DfuTitle), rx.t(Msg::DfuChecking));
    }
}

/// The two confirm rows (Install / Cancel); neither is guarded.
const CONFIRM_GUARDS: [bool; 2] = [false; 2];
const INSTALL: usize = 0;

/// Side inset (px) the cards keep from the panel edges.
const INSET: i32 = 12;

/// The install confirm: the version table and the Install / Cancel rows.
#[derive(Debug)]
pub struct DfuConfirmScreen {
    report: DfuScanReport,
    actions: ActionRows,
}

impl DfuConfirmScreen {
    pub fn new(report: DfuScanReport) -> Self {
        DfuConfirmScreen { report, actions: ActionRows::new(0) }
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match self.actions.handle(g, &CONFIRM_GUARDS) {
            CardEvent::Activate(INSTALL) => {
                // Post the install one-shot; the board snapshots the rollback, arms, and reboots.
                cx.dfu.admit_intent(crate::dfu::DfuIntent::InstallRequested);
                Transition::Replace(Screen::DfuProgress(DfuProgressScreen::new()))
            }
            CardEvent::Activate(_) | CardEvent::Dismiss => Transition::Pop, // Cancel
            CardEvent::None => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, rx.t(Msg::DfuConfirmTitle), "");

        let mut y = TITLE_BAR_H + 12;
        y = version_row(cv, w, y, rx.t(Msg::DfuInstalled), &self.report.installed);
        y = version_row(cv, w, y + 4, rx.t(Msg::DfuStaged), &self.report.staged);

        let note_y = y + chrome::wrapped_line_pitch(Font::Label) + 1;
        if self.report.same_version() {
            let after = wrapped(cv, rx.t(Msg::DfuSameVersion), w / 2, note_y, w - 2 * INSET, WARNING);
            if self.report.first_install {
                wrapped(cv, rx.t(Msg::DfuNoUndo), w / 2, after, w - 2 * INSET, WARNING);
            }
        } else if self.report.first_install {
            wrapped(cv, rx.t(Msg::DfuNoUndo), w / 2, note_y, w - 2 * INSET, WARNING);
        }

        let geo = GuardedRowsGeometry {
            x: 12,
            w: w - 24,
            top: h - 2 * 42 - 6 - 12,
            row_h: 42,
            gap: 6,
            label_dx: 16,
            label_dy: 9,
        };
        let items = [
            MenuItem { label: rx.t(Msg::DfuInstall), guard: CONFIRM_GUARDS[0] },
            MenuItem { label: rx.t(Msg::DfuCancel), guard: CONFIRM_GUARDS[1] },
        ];
        self.actions.draw(cv, &items, rx.hold_progress, AMBER, geo);
    }
}

/// Draw one version-table row: the caption at the left inset, the version right-aligned. A version
/// too wide to share the baseline wraps onto its own lines below. Returns the `y` past the row.
fn version_row(cv: &mut impl Surface, w: i32, top_y: i32, caption: &str, version: &Version) -> i32 {
    use palette::*;
    let lh = chrome::wrapped_line_pitch(Font::Label) + 1;
    let char_w = Font::Label.char_width() as i32;
    let cap_w = caption.chars().count() as i32 * char_w;
    let ver_w = version.chars().count() as i32 * char_w;
    cv.text(caption, Point::new(INSET, top_y), Font::Label, TextAlign::Left, SUBTEXT);
    if ver_w <= w - 2 * INSET - cap_w - 10 {
        cv.text(version, Point::new(w - INSET, top_y), Font::Label, TextAlign::Right, INK);
        top_y + lh
    } else {
        version_lines(cv, version, w - INSET, top_y + lh, w - 2 * INSET, Font::Label, TextAlign::Right, INK)
    }
}

/// Draw a version string char-wrapped to `width_px`, from `top_y`. A git-describe tag has no
/// spaces, so the word-wrapping [`wrapped`] cannot break it. Returns the `y` past the last line.
#[allow(clippy::too_many_arguments)]
fn version_lines(
    cv: &mut impl Surface,
    version: &str,
    x: i32,
    top_y: i32,
    width_px: i32,
    font: Font,
    align: TextAlign,
    color: u16,
) -> i32 {
    let budget = (width_px / font.char_width() as i32).max(1) as usize;
    let lh = chrome::wrapped_line_pitch(font) + 1;
    let mut y = top_y;
    let mut line: heapless::String<48> = heapless::String::new();
    for ch in version.chars() {
        if line.chars().count() >= budget {
            cv.text(&line, Point::new(x, y), font, align, color);
            y += lh;
            line.clear();
        }
        let _ = line.push(ch);
    }
    if !line.is_empty() {
        cv.text(&line, Point::new(x, y), font, align, color);
        y += lh;
    }
    y
}

/// The arming wait: the spinner over "Preparing update..." while the board drains the install
/// one-shot. It ignores all input, because an arm cannot be cancelled.
#[derive(Debug, Default)]
pub struct DfuProgressScreen {
    spin: Spinner,
}

impl DfuProgressScreen {
    pub fn new() -> Self {
        DfuProgressScreen { spin: Spinner::default() }
    }

    pub fn handle(&mut self, _g: Gesture, _cx: &mut Ctx) -> Transition {
        Transition::None // the arm is irreversible; nothing to do but wait for the reboot
    }

    pub fn tick_timers(&mut self, now_ms: u32, w: i32, h: i32) -> ScreenTick {
        self.spin.tick(now_ms, w, h)
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        wait_card(cv, rx, &self.spin, rx.t(Msg::DfuTitle), rx.t(Msg::DfuPreparing));
    }
}

/// The last frame the app paints before the arm's warm reset. The bootloader does not draw, but it
/// keeps the COM wave alive, so the Memory-in-Pixel panel holds this frame through the whole flash.
/// Everything on the card is static: a spinner would freeze at the reset and read as a wedge.
#[derive(Debug, Default)]
pub struct DfuInstallingScreen;

impl DfuInstallingScreen {
    pub fn new() -> Self {
        DfuInstallingScreen
    }

    pub fn handle(&mut self, _g: Gesture, _cx: &mut Ctx) -> Transition {
        Transition::None // terminal: the reset is already on its way
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, rx.t(Msg::DfuTitle), "");
        let after_head =
            chrome::wrapped(cv, rx.t(Msg::DfuInstalling), w / 2, TITLE_BAR_H + 40, w - 2 * INSET, Font::Body, INK);
        let after_body = wrapped(cv, rx.t(Msg::DfuInstallingBody), w / 2, after_head + 16, w - 2 * INSET, INK);
        wrapped(cv, rx.t(Msg::DfuInstallingPower), w / 2, after_body + 12, w - 2 * INSET, WARNING);
    }
}

/// Which half of the flow the error card reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DfuErrorReason {
    /// The staging scan rejected `UPDATE.BIN`.
    Scan(DfuScanError),
    /// The install drain refused to arm, or the arm itself failed.
    Install(DfuInstallError),
}

/// The error card: a [`DfuErrorReason`] as plain copy. Any Back dismisses it.
#[derive(Debug)]
pub struct DfuErrorScreen {
    reason: DfuErrorReason,
}

impl DfuErrorScreen {
    /// A scan rejection card.
    pub fn new(error: DfuScanError) -> Self {
        DfuErrorScreen { reason: DfuErrorReason::Scan(error) }
    }

    /// An install-drain failure card. A re-scan bucket normalises to a scan reason, so both paths
    /// share the scan copy.
    pub fn new_install(error: DfuInstallError) -> Self {
        let reason = match error {
            DfuInstallError::Scan(e) => DfuErrorReason::Scan(e),
            other => DfuErrorReason::Install(other),
        };
        DfuErrorScreen { reason }
    }

    pub fn reason(&self) -> DfuErrorReason {
        self.reason
    }

    pub fn handle(&mut self, g: Gesture, _cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Press | Gesture::Back => Transition::Pop,
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, rx.t(Msg::DfuTitle), "");
        card_triangle(cv, Point::new(w / 2, TITLE_BAR_H + 46), 22);
        let scan_msg = |e: DfuScanError| match e {
            DfuScanError::NotFound => Msg::DfuNotFound,
            DfuScanError::Unreadable => Msg::DfuUnreadable,
            DfuScanError::Damaged => Msg::DfuDamaged,
            DfuScanError::TooLarge => Msg::DfuTooLarge,
            DfuScanError::TooFragmented => Msg::DfuFragmented,
            DfuScanError::Untrusted => Msg::DfuUntrusted,
        };
        let key = match self.reason {
            DfuErrorReason::Scan(e) => scan_msg(e),
            DfuErrorReason::Install(DfuInstallError::Scan(e)) => scan_msg(e),
            DfuErrorReason::Install(DfuInstallError::Recording) => Msg::DfuInstallRecording,
            DfuErrorReason::Install(DfuInstallError::NoCard) => Msg::DfuInstallNoCard,
            DfuErrorReason::Install(DfuInstallError::SnapshotFailed) => Msg::DfuInstallSnapshotFailed,
            DfuErrorReason::Install(DfuInstallError::StateWriteFailed) => Msg::DfuInstallStateWrite,
        };
        wrapped(cv, rx.t(key), w / 2, TITLE_BAR_H + 84, w - 32, INK);
    }
}

/// The one-time "Updated to vX" toast the first healthy boot after an update shows. Any press or
/// Back dismisses it.
#[derive(Debug)]
pub struct DfuUpdatedScreen {
    version: Version,
}

impl DfuUpdatedScreen {
    pub fn new(version: &str) -> Self {
        let mut v = Version::new();
        for ch in version.chars() {
            if v.push(ch).is_err() {
                break;
            }
        }
        DfuUpdatedScreen { version: v }
    }

    pub fn handle(&mut self, g: Gesture, _cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Press | Gesture::Back => Transition::Pop,
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, rx.t(Msg::DfuUpdatedTitle), "");
        card_check(cv, Point::new(w / 2, TITLE_BAR_H + 56), 24);
        cv.text(rx.t(Msg::DfuUpdated), Point::new(w / 2, TITLE_BAR_H + 104), Font::Body, TextAlign::Center, INK);
        version_lines(cv, &self.version, w / 2, TITLE_BAR_H + 134, w - 2 * INSET, Font::Body, TextAlign::Center, AMBER);
    }
}

/// The one-time "UPDATE FAILED" card the first boot after a failed update shows. It carries the
/// [`DfuFailure`] verdict and, when the arm marker survived, the staged version. Any press or Back
/// dismisses it.
#[derive(Debug)]
pub struct DfuFailedScreen {
    why: DfuFailure,
    staged: Option<Version>,
}

impl DfuFailedScreen {
    pub fn new(why: DfuFailure, staged: Option<&str>) -> Self {
        DfuFailedScreen { why, staged: staged.map(crate::dfu::clamp) }
    }

    pub fn why(&self) -> DfuFailure {
        self.why
    }

    pub fn handle(&mut self, g: Gesture, _cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Press | Gesture::Back => Transition::Pop,
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, rx.t(Msg::DfuFailedTitle), "");
        card_triangle(cv, Point::new(w / 2, TITLE_BAR_H + 46), 22);
        let msg = match self.why {
            DfuFailure::NotStarted => rx.t(Msg::DfuFailedNotStarted),
            DfuFailure::Reverted => rx.t(Msg::DfuFailedReverted),
        };
        let bottom = wrapped(cv, msg, w / 2, TITLE_BAR_H + 84, w - 32, INK);
        if let Some(v) = &self.staged {
            version_lines(cv, v, w / 2, bottom + 22, w - 2 * INSET, Font::Body, TextAlign::Center, AMBER);
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

    fn report(installed: &str, staged: &str, first_install: bool) -> DfuScanReport {
        DfuScanReport::new(installed, staged, first_install)
    }

    /// Run a gesture and hand back the transition and the action the screen posted to the DFU domain.
    fn run(scr: &mut impl FnMut(&mut Ctx) -> Transition) -> (Transition, Option<DfuAction>) {
        let mut st = AppState::new(0, 0, 1.0);
        let mut act = Activity::new(Mode::Idle);
        let mut settings = Settings::default();
        let mut dfu = crate::dfu::DfuState::new();
        let t = {
            let mut cx = Ctx { dfu: &mut dfu, ..test_ctx(&mut st, &mut act, &mut settings) };
            scr(&mut cx)
        };
        (t, drained_dfu(&mut dfu))
    }

    /// The action the DFU domain is holding.
    fn drained_dfu(dfu: &mut crate::dfu::DfuState) -> Option<DfuAction> {
        dfu.next_effect().map(|effect| match effect {
            crate::dfu::DfuEffect::Scan { .. } => DfuAction::Scan,
            crate::dfu::DfuEffect::ArmInstall { .. } => DfuAction::Install,
        })
    }

    #[test]
    fn same_version_is_exact_equality() {
        assert!(report("v1.0.0-0-gabc", "v1.0.0-0-gabc", false).same_version());
        assert!(!report("v1.0.0-0-gabc", "v1.1.0-0-gdef", false).same_version());
    }

    #[test]
    fn confirm_install_posts_and_shows_progress() {
        let mut scr = DfuConfirmScreen::new(report("v1", "v2", false));
        let (t, posted) = run(&mut |cx| scr.handle(Gesture::Press, cx));
        assert!(matches!(t, Transition::Replace(Screen::DfuProgress(_))), "Install swaps to progress");
        assert_eq!(posted, Some(DfuAction::Install), "and arms via the install one-shot");
    }

    #[test]
    fn confirm_cancel_and_back_pop_without_arming() {
        let mut scr = DfuConfirmScreen::new(report("v1", "v2", false));
        let (_, _) = run(&mut |cx| scr.handle(Gesture::Step(1), cx));
        let (t, posted) = run(&mut |cx| scr.handle(Gesture::Press, cx));
        assert!(matches!(t, Transition::Pop), "Cancel pops");
        assert_eq!(posted, None, "and arms nothing");

        let mut scr = DfuConfirmScreen::new(report("v1", "v2", false));
        let (t, posted) = run(&mut |cx| scr.handle(Gesture::Back, cx));
        assert!(matches!(t, Transition::Pop), "Back cancels");
        assert_eq!(posted, None);
    }

    #[test]
    fn error_card_dismisses() {
        let mut scr = DfuErrorScreen::new(DfuScanError::TooFragmented);
        assert_eq!(scr.reason(), DfuErrorReason::Scan(DfuScanError::TooFragmented));
        let (t, _) = run(&mut |cx| scr.handle(Gesture::Back, cx));
        assert!(matches!(t, Transition::Pop));

        let scr = DfuErrorScreen::new_install(DfuInstallError::Recording);
        assert_eq!(scr.reason(), DfuErrorReason::Install(DfuInstallError::Recording));

        let scr = DfuErrorScreen::new_install(DfuInstallError::Scan(DfuScanError::Damaged));
        assert_eq!(scr.reason(), DfuErrorReason::Scan(DfuScanError::Damaged));
    }

    #[test]
    fn toast_dismisses() {
        let mut scr = DfuUpdatedScreen::new("v2.0.0-0-gccc");
        let (t, _) = run(&mut |cx| scr.handle(Gesture::Press, cx));
        assert!(matches!(t, Transition::Pop));
    }

    #[test]
    fn check_wait_cancels_on_back() {
        let mut scr = DfuCheckScreen::new();
        let (t, _) = run(&mut |cx| scr.handle(Gesture::Back, cx));
        assert!(matches!(t, Transition::Pop));
    }
}
