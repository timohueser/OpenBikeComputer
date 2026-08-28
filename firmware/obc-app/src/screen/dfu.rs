//! The SD-sideload firmware-update flow's screens (epic #615 S5, #620) — the user-facing half of
//! the DFU armer. Reached from **Settings → System → "Install update from card"**
//! ([`SystemScreen`](super::SystemScreen)); the scan/arm machinery runs board-side.
//!
//! Seven static screens (no map plane), each a small typed state through the normal screen stack:
//!
//! - [`DfuCheckScreen`] — the brief "Checking card..." wait (spinner) after the scan is posted; the
//!   board's answer (the pass's fact stage) replaces
//!   it with the confirm screen or the error card. **Back** cancels the wait.
//! - [`DfuConfirmScreen`] — the *installed → update* version table and the no-undo / same-version
//!   warnings. Select **Install** arms (posts
//!   [`DfuAction::Install`](crate::activity::DfuAction) and swaps to the progress screen); **Back**
//!   / **Cancel** returns to the System menu. The standard two-row confirm chrome, like
//!   [`NavConfirmScreen`](super::NavConfirmScreen).
//! - [`DfuProgressScreen`] — "Preparing update..." (spinner) while the drain runs the CRC pass +
//!   rollback snapshot + arm. Ignores input; the arm is irreversible.
//! - [`DfuInstallingScreen`] — the static, terminal "Installing update" card the board swaps in
//!   (the pass's fact stage) and paints as its **last
//!   frame** before the arm's warm reset: the bootloader never draws (LED codes only), but it
//!   parks the panel pins and keeps the COM wave alive, so the Memory-in-Pixel glass *holds this
//!   frame* through the whole flash.
//! - [`DfuErrorScreen`] — a [`DfuScanError`](crate::dfu::DfuScanError) *or*
//!   [`DfuInstallError`](crate::dfu::DfuInstallError) as a plain sentence (a scan rejection or an
//!   install-drain refusal / arm failure, #755); **Back** dismisses (like
//!   [`NavFailScreen`](super::NavFailScreen)).
//! - [`DfuUpdatedScreen`] — the one-time "Updated to vX" toast the first healthy boot after an
//!   update shows (host-pushed via the pass's fact stage);
//!   any press/Back dismisses.
//! - [`DfuFailedScreen`] — the one-time "UPDATE FAILED" card the first boot after a failed update
//!   shows (host-pushed via the pass's fact stage from
//!   the board's boot-outcome reconcile): a typed [`DfuFailure`](crate::dfu::DfuFailure) verdict —
//!   never started vs reverted — plus the staged version; any press/Back dismisses.

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

// ── The wait card: the shared spinner over this flow's title + caption ──

/// Draw a DFU wait screen: the title bar, the shared spinner's needle centred on the panel, and
/// the caption naming what the board is doing. The two waits ([`DfuCheckScreen`] and
/// [`DfuProgressScreen`]) differ only in that copy.
fn wait_card(cv: &mut impl Surface, rx: &Render, spin: &Spinner, title: &str, caption: &str) {
    let (w, h) = (rx.w, rx.h);
    title_frame(cv, w, h, title, "");
    spin.draw_needle(cv, w, h);
    cv.text(caption, Point::new(w / 2, h * 72 / 100), Font::Label, TextAlign::Center, palette::INK);
}

// ── Multi-line centred body copy: the shared `wrapped` (author each catalog string on one
// line; wrap at draw time), always at `Font::Label` on these cards. ──

fn wrapped(cv: &mut impl Surface, text: &str, cx: i32, top_y: i32, width_px: i32, color: u16) -> i32 {
    chrome::wrapped(cv, text, cx, top_y, width_px, Font::Label, color)
}

// ── DfuCheck: the "Checking card..." wait ──

/// The scan wait: up from the System menu's press until the board answers. Shows the spinner over
/// "Checking card...". **Back** cancels (the drained scan's answer, if it later arrives, is dropped
/// by the update domain's answer — a scan costs nothing).
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

    /// [`Screen::tick_timers`] arm: spin the needle while the board scans.
    pub fn tick_timers(&mut self, now_ms: u32, w: i32, h: i32) -> ScreenTick {
        self.spin.tick(now_ms, w, h)
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        wait_card(cv, rx, &self.spin, rx.t(Msg::DfuTitle), rx.t(Msg::DfuChecking));
    }
}

// ── DfuConfirm: installed → update, the warnings, and Install / Cancel ──

/// The two confirm rows (Install / Cancel), neither guarded.
const CONFIRM_GUARDS: [bool; 2] = [false; 2];
const INSTALL: usize = 0;

/// Side inset (px) the confirm card's version table and notes keep from the panel edges — a version
/// string is right-aligned to `w - INSET`, never edge-to-edge (spec §1).
const INSET: i32 = 12;

/// The install confirm. Carries the scan report (the versions + no-undo fact) and the highlighted
/// option. Select **Install** posts [`DfuAction::Install`](crate::activity::DfuAction) and swaps to
/// the progress screen; **Back** / **Cancel** returns to the System menu.
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
                // Arm: post the install one-shot (the board snapshots the rollback + arms + reboots)
                // and swap to the progress spinner. The confirm was pushed over the System menu.
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

        // The version table: two rows, an olive caption at the left inset labelling the right-
        // aligned INK version. Installed first, then the staged update — what "install" replaces it
        // with. A git-describe string too wide for its row wraps below the caption (never clipped).
        let mut y = TITLE_BAR_H + 12;
        y = version_row(cv, w, y, rx.t(Msg::DfuInstalled), &self.report.installed);
        y = version_row(cv, w, y + 4, rx.t(Msg::DfuStaged), &self.report.staged);

        // The conditional red note — the same-version note, then the no-undo note on a first install
        // (no rollback snapshot exists — spec §2.4) — one type-step down (Label), with a blank line
        // above it and the clearance to the Install row below. Both warning-coloured.
        let note_y = y + Font::Label.cap_height() as i32 + 2;
        if self.report.same_version() {
            let after = wrapped(cv, rx.t(Msg::DfuSameVersion), w / 2, note_y, w - 2 * INSET, WARNING);
            if self.report.first_install {
                wrapped(cv, rx.t(Msg::DfuNoUndo), w / 2, after, w - 2 * INSET, WARNING);
            }
        } else if self.report.first_install {
            wrapped(cv, rx.t(Msg::DfuNoUndo), w / 2, note_y, w - 2 * INSET, WARNING);
        }

        // The Install / Cancel rows, the standard confirm chrome (like the create-route confirm).
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

/// Draw one version-table row: the olive caption (Label) at the left inset labelling the INK version
/// right-aligned to `w - INSET`. A version that fits beside its caption shares the baseline; one too
/// wide wraps onto its own line(s) below (right-aligned, `INSET`-px side insets — never ellipsized,
/// never edge-to-edge). Returns the `y` just past the row. Versions are never translated.
fn version_row(cv: &mut impl Surface, w: i32, top_y: i32, caption: &str, version: &Version) -> i32 {
    use palette::*;
    let lh = Font::Label.cap_height() as i32 + 2;
    let char_w = Font::Label.char_width() as i32;
    let cap_w = caption.chars().count() as i32 * char_w;
    let ver_w = version.chars().count() as i32 * char_w;
    cv.text(caption, Point::new(INSET, top_y), Font::Label, TextAlign::Left, SUBTEXT);
    if ver_w <= w - 2 * INSET - cap_w - 10 {
        // Fits beside the caption: right-aligned on the same baseline.
        cv.text(version, Point::new(w - INSET, top_y), Font::Label, TextAlign::Right, INK);
        top_y + lh
    } else {
        // Too wide: wrap onto its own line(s) below the caption, right-aligned within the insets.
        version_lines(cv, version, w - INSET, top_y + lh, w - 2 * INSET, Font::Label, TextAlign::Right, INK)
    }
}

/// Draw a version string char-wrapped to `width_px`, `align`ed at anchor `x`, stacking `font` lines
/// from `top_y`. A git-describe tag is a single space-less token the word-wrapping [`wrapped`] can't
/// break, so this splits it on the monospace cell budget — the versions never ellipsize. Returns the
/// `y` just past the last line. Shared by the confirm table (right-aligned) and the "UPDATED" toast
/// (centred).
#[allow(clippy::too_many_arguments)] // a plain draw helper: surface + string + full text geometry
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
    let lh = font.cap_height() as i32 + 2;
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

// ── DfuProgress: "Preparing update..." until the board commits to the arm ──

/// The arming-in-progress screen: the spinner over "Preparing update..." while the install
/// one-shot waits for the board's drain. Once the drain's guards pass, the board swaps this for
/// the terminal [`DfuInstallingScreen`] (its last painted frame before the reboot into the
/// bootloader). This screen ignores all input: an arm can't be cancelled.
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

    /// [`Screen::tick_timers`] arm: spin the needle while the board arms.
    pub fn tick_timers(&mut self, now_ms: u32, w: i32, h: i32) -> ScreenTick {
        self.spin.tick(now_ms, w, h)
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        wait_card(cv, rx, &self.spin, rx.t(Msg::DfuTitle), rx.t(Msg::DfuPreparing));
    }
}

// ── DfuInstalling: the terminal pre-reset frame the panel holds through the install ──

/// The static **"Installing update"** card — the last frame the app paints before the arm's warm
/// reset into the bootloader. The bootloader never draws (its LED codes are the liveness signal);
/// it parks the panel pins and keeps the COM wave alternating (`obc-boot/src/com.rs`), so the
/// Memory-in-Pixel panel *holds this exact frame* for the whole multi-ten-second flash.
/// Board-pushed (the pass's fact stage) right before the
/// rollback snapshot + arm — deliberately **everything on it is static**: a spinner would freeze
/// mid-sweep the moment the reset lands and read as a wedge, so the copy names the LED as the
/// "still working" signal instead. Input is ignored; the arm is already irreversible.
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
        // The headline wraps at Body size (the French copy is two lines at 240 px), the
        // explanation below at Label, and the one imperative — keep power on — in warning red.
        let after_head =
            chrome::wrapped(cv, rx.t(Msg::DfuInstalling), w / 2, TITLE_BAR_H + 40, w - 2 * INSET, Font::Body, INK);
        let after_body = wrapped(cv, rx.t(Msg::DfuInstallingBody), w / 2, after_head + 16, w - 2 * INSET, INK);
        wrapped(cv, rx.t(Msg::DfuInstallingPower), w / 2, after_body + 12, w - 2 * INSET, WARNING);
    }
}

// ── DfuError: a typed scan- or install-drain failure as a plain sentence ──

/// Which half of the flow the error card is reporting (issue #620 §2, #755). A scan rejection
/// ([`DfuScanError`], from the "Checking card..." step) or an install-drain refusal / arm failure
/// ([`DfuInstallError`], from the "Preparing update..." step). One card, one reason type — the draw
/// picks the copy. Keeping both under a single screen keeps the i18n catalog to one add-per-reason
/// (the install re-scan bucket reuses the scan copy).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DfuErrorReason {
    /// The staging scan rejected `UPDATE.BIN`.
    Scan(DfuScanError),
    /// The install drain refused to arm, or the arm itself failed.
    Install(DfuInstallError),
}

/// The error card (issue #620 §2, #755): a [`DfuErrorReason`] mapped to plain copy. Info-only — any
/// **Back** dismisses (like the nav failure card), returning to the System menu. Reached either from
/// the scan's answer (`notify_dfu_scan_result`) or the install drain's
/// (`notify_dfu_install_failed`).
#[derive(Debug)]
pub struct DfuErrorScreen {
    reason: DfuErrorReason,
}

impl DfuErrorScreen {
    /// A scan rejection card (the "Checking card..." step's `Err`).
    pub fn new(error: DfuScanError) -> Self {
        DfuErrorScreen { reason: DfuErrorReason::Scan(error) }
    }

    /// An install-drain failure card (the "Preparing update..." step's non-reboot outcome). A
    /// re-scan bucket is normalised to a plain scan reason so both paths share the scan copy.
    pub fn new_install(error: DfuInstallError) -> Self {
        let reason = match error {
            DfuInstallError::Scan(e) => DfuErrorReason::Scan(e),
            other => DfuErrorReason::Install(other),
        };
        DfuErrorScreen { reason }
    }

    /// The reason this card shows — lets the seam tests pin the error→card mapping.
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

// ── DfuUpdated: the one-time post-update toast ──

/// The one-time "Updated to vX" toast the first healthy boot after an update shows (host-pushed via
/// the pass's fact stage). Info-only; any press/Back
/// dismisses, like the "ROUTE UPDATED" card.
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
        // The version, verbatim (never translated) — a long tag wraps to a second centred line
        // (`version_lines`), never running off the card's edge.
        version_lines(cv, &self.version, w / 2, TITLE_BAR_H + 134, w - 2 * INSET, Font::Body, TextAlign::Center, AMBER);
    }
}

// ── DfuFailed: the one-time boot-outcome failure card ──

/// The one-time "UPDATE FAILED" card the first boot after a failed update shows (host-pushed via
/// the pass's fact stage from the board's boot-outcome
/// reconcile — the failure twin of [`DfuUpdatedScreen`]). Carries the typed [`DfuFailure`] verdict
/// and, when the arm marker survived, the staged version that failed. Info-only; any press/Back
/// dismisses.
#[derive(Debug)]
pub struct DfuFailedScreen {
    why: DfuFailure,
    staged: Option<Version>,
}

impl DfuFailedScreen {
    pub fn new(why: DfuFailure, staged: Option<&str>) -> Self {
        DfuFailedScreen { why, staged: staged.map(crate::dfu::clamp) }
    }

    /// The verdict this card shows — lets the seam tests pin the failure→card mapping.
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
        // The staged version that failed, verbatim (never translated) — when the marker survived.
        if let Some(v) = &self.staged {
            cv.text(v, Point::new(w / 2, bottom + 22), Font::Body, TextAlign::Center, AMBER);
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

    /// Build a throwaway `Ctx`, run a gesture, and hand back the transition + the update phase the
    /// screen posted to the DFU domain (if any), taken as an executor would.
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

    /// The phase the DFU domain is holding, in the legacy vocabulary the tests already speak.
    fn drained_dfu(dfu: &mut crate::dfu::DfuState) -> Option<DfuAction> {
        dfu.next_effect().map(|effect| match effect {
            crate::dfu::DfuEffect::Scan { .. } => DfuAction::Scan,
            crate::dfu::DfuEffect::ArmInstall { .. } => DfuAction::Install,
        })
    }

    /// The same-version warning fires only on a byte-for-byte match.
    #[test]
    fn same_version_is_exact_equality() {
        assert!(report("v1.0.0-0-gabc", "v1.0.0-0-gabc", false).same_version());
        assert!(!report("v1.0.0-0-gabc", "v1.1.0-0-gdef", false).same_version());
    }

    /// Confirm → Install posts the install one-shot and swaps to the progress spinner.
    #[test]
    fn confirm_install_posts_and_shows_progress() {
        let mut scr = DfuConfirmScreen::new(report("v1", "v2", false));
        let (t, posted) = run(&mut |cx| scr.handle(Gesture::Press, cx));
        assert!(matches!(t, Transition::Replace(Screen::DfuProgress(_))), "Install swaps to progress");
        assert_eq!(posted, Some(DfuAction::Install), "and arms via the install one-shot");
    }

    /// Confirm → Cancel (and Back) pops without arming anything.
    #[test]
    fn confirm_cancel_and_back_pop_without_arming() {
        let mut scr = DfuConfirmScreen::new(report("v1", "v2", false));
        // Step down to the Cancel row, then press.
        let (_, _) = run(&mut |cx| scr.handle(Gesture::Step(1), cx));
        let (t, posted) = run(&mut |cx| scr.handle(Gesture::Press, cx));
        assert!(matches!(t, Transition::Pop), "Cancel pops");
        assert_eq!(posted, None, "and arms nothing");

        let mut scr = DfuConfirmScreen::new(report("v1", "v2", false));
        let (t, posted) = run(&mut |cx| scr.handle(Gesture::Back, cx));
        assert!(matches!(t, Transition::Pop), "Back cancels");
        assert_eq!(posted, None);
    }

    /// The error card carries its variant and dismisses on Back/press — for both a scan rejection
    /// and an install-drain failure, and the install re-scan bucket normalises to a scan reason so
    /// it shares the scan copy.
    #[test]
    fn error_card_dismisses() {
        let mut scr = DfuErrorScreen::new(DfuScanError::TooFragmented);
        assert_eq!(scr.reason(), DfuErrorReason::Scan(DfuScanError::TooFragmented));
        let (t, _) = run(&mut |cx| scr.handle(Gesture::Back, cx));
        assert!(matches!(t, Transition::Pop));

        let scr = DfuErrorScreen::new_install(DfuInstallError::Recording);
        assert_eq!(scr.reason(), DfuErrorReason::Install(DfuInstallError::Recording));

        // The re-scan bucket folds to a plain scan reason (shared copy, no duplicate catalog key).
        let scr = DfuErrorScreen::new_install(DfuInstallError::Scan(DfuScanError::Damaged));
        assert_eq!(scr.reason(), DfuErrorReason::Scan(DfuScanError::Damaged));
    }

    /// The post-update toast dismisses on any press/Back.
    #[test]
    fn toast_dismisses() {
        let mut scr = DfuUpdatedScreen::new("v2.0.0-0-gccc");
        let (t, _) = run(&mut |cx| scr.handle(Gesture::Press, cx));
        assert!(matches!(t, Transition::Pop));
    }

    /// The check wait cancels on Back (the drained scan's answer is then dropped).
    #[test]
    fn check_wait_cancels_on_back() {
        let mut scr = DfuCheckScreen::new();
        let (t, _) = run(&mut |cx| scr.handle(Gesture::Back, cx));
        assert!(matches!(t, Transition::Pop));
    }
}
