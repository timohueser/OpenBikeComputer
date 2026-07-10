//! The SD-sideload firmware-update flow's screens (epic #615 S5, #620) — the user-facing half of
//! the DFU armer. Reached from **Settings → System → "Install update from card"**
//! ([`SystemScreen`](super::SystemScreen)); the scan/arm machinery runs board-side.
//!
//! Six static screens (no map plane), each a small typed state through the normal screen stack:
//!
//! - [`DfuCheckScreen`] — the brief "Checking card..." wait (spinner) after the scan is posted; the
//!   board's answer ([`App::notify_dfu_scan_result`](crate::App::notify_dfu_scan_result)) replaces
//!   it with the confirm screen or the error card. **Back** cancels the wait.
//! - [`DfuConfirmScreen`] — the *installed → update* version table and the no-undo / same-version
//!   warnings. Encoder **Install** arms (posts
//!   [`DfuAction::Install`](crate::activity::DfuAction) and swaps to the progress screen); **Back**
//!   / **Cancel** returns to the System menu. The standard two-row confirm chrome, like
//!   [`NavConfirmScreen`](super::NavConfirmScreen).
//! - [`DfuProgressScreen`] — "Preparing update..." (spinner) while the drain runs the CRC pass +
//!   rollback snapshot + arm, then the board reboots into the bootloader (its LED takes over — the
//!   display is off during the flash). Ignores input; the arm is irreversible.
//! - [`DfuErrorScreen`] — a [`DfuScanError`](crate::dfu::DfuScanError) as a plain sentence; **Back**
//!   dismisses (like [`NavFailScreen`](super::NavFailScreen)).
//! - [`DfuUpdatedScreen`] — the one-time "Updated to vX" toast the first healthy boot after an
//!   update shows (host-pushed via [`App::notify_update_confirmed`](crate::App::notify_update_confirmed));
//!   any press/Back dismisses.
//! - [`DfuFailedScreen`] — the one-time "UPDATE FAILED" card the first boot after a failed update
//!   shows (host-pushed via [`App::notify_update_failed`](crate::App::notify_update_failed) from
//!   the board's boot-outcome reconcile): a typed [`DfuFailure`](crate::dfu::DfuFailure) verdict —
//!   never started vs reverted — plus the staged version; any press/Back dismisses.

use embedded_graphics::{
    prelude::{Point, Size},
    primitives::Rectangle,
};
use obc_render::{
    text::{Font, TextAlign},
    Surface,
};

use crate::dfu::{DfuFailure, DfuScanError, DfuScanReport, Version};
use crate::input::Gesture;
use crate::Msg;

use super::{
    card_check, card_triangle, palette, title_frame, Ctx, MenuItem, Render, Screen, ScreenTick, Transition, TITLE_BAR_H,
};

// ── The shared indeterminate spinner (the Menu dial's needle, like the nav planner's #499) ──

/// Degrees per second the wait spinner sweeps — a calm, steady rotation, matching the nav planner.
const SPIN_DPS: f32 = 240.0;
/// Frame cadence the spinner repaints at *and* asks the host to wake for — smooth enough for a
/// needle, cheap enough that the wait isn't dominated by full-chrome repaints.
const SPIN_FRAME_MS: u32 = 66;
/// The spinner needle's sweep radius (px).
const NEEDLE_R: f32 = 42.0;
/// Half-extent (px) of the reported dirty square: the sweep plus a rasterizer margin.
const NEEDLE_CLIP_HALF: i32 = NEEDLE_R as i32 + 2;

/// The square the spinning needle repaints inside, centred on `(w/2, h/2)` — everything else on a
/// wait screen (title bar, caption) is static, so the host can clip the repaint to this disc.
fn needle_region(w: i32, h: i32) -> Rectangle {
    let (cx, cy) = (w / 2, h / 2);
    Rectangle::new(
        Point::new(cx - NEEDLE_CLIP_HALF, cy - NEEDLE_CLIP_HALF),
        Size::new(2 * NEEDLE_CLIP_HALF as u32 + 1, 2 * NEEDLE_CLIP_HALF as u32 + 1),
    )
}

/// A free-spinning compass needle over static chrome — the "working..." indicator shared by
/// [`DfuCheckScreen`] and [`DfuProgressScreen`]. Advanced by real elapsed millis (so the speed
/// reads the same at any host frame rate) and throttled to [`SPIN_FRAME_MS`] like the nav planner.
#[derive(Debug, Default)]
struct Spinner {
    /// Current angle (0° = N, clockwise), advanced in [`tick`](Spinner::tick).
    needle_deg: f32,
    /// Clock of the previous tick, for the per-frame `dt`; `None` before the first.
    last_ms: Option<u32>,
    /// Clock of the last tick that claimed a repaint — the throttle's anchor.
    last_paint_ms: Option<u32>,
}

impl Spinner {
    /// Spin by real elapsed time and keep the host's frame cadence armed. The claim carries the
    /// [`needle_region`] as its dirty region (the chrome never changes), so the host repaints only
    /// the disc. `w`/`h` of 0 (no frame rendered yet) abstains (`None` = full repaint).
    fn tick(&mut self, now_ms: u32, w: i32, h: i32) -> ScreenTick {
        let dt = self.last_ms.map_or(0.0, |last| now_ms.wrapping_sub(last) as f32 / 1000.0);
        self.last_ms = Some(now_ms);
        self.needle_deg = (self.needle_deg + SPIN_DPS * dt.min(0.25)) % 360.0;
        let due = self.last_paint_ms.is_none_or(|last| now_ms.wrapping_sub(last) >= SPIN_FRAME_MS);
        if due {
            self.last_paint_ms = Some(now_ms);
        }
        let region = (w > 0 && h > 0).then(|| needle_region(w, h));
        ScreenTick { changed: due && dt > 0.0, next_wake_ms: Some(SPIN_FRAME_MS), region }
    }

    /// Draw the needle centred on the panel, over a title bar + a caption line.
    fn draw(&self, cv: &mut impl Surface, rx: &Render, title: &str, caption: &str) {
        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, title, "");
        super::menu::draw_needle(cv, Point::new(w / 2, h / 2), self.needle_deg, NEEDLE_R, 10.0);
        cv.text(caption, Point::new(w / 2, h * 72 / 100), Font::Label, TextAlign::Center, palette::INK);
    }
}

// ── Multi-line centred body copy: the shared `super::wrapped` (author each catalog string on one
// line; wrap at draw time), always at `Font::Label` on these cards. ──

fn wrapped(cv: &mut impl Surface, text: &str, cx: i32, top_y: i32, width_px: i32, color: u16) -> i32 {
    super::wrapped(cv, text, cx, top_y, width_px, Font::Label, color)
}

// ── DfuCheck: the "Checking card..." wait ──

/// The scan wait: up from the System menu's press until the board answers. Shows the spinner over
/// "Checking card...". **Back** cancels (the drained scan's answer, if it later arrives, is dropped
/// by [`notify_dfu_scan_result`](crate::App::notify_dfu_scan_result) — a scan costs nothing).
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
        self.spin.draw(cv, rx, rx.t(Msg::DfuTitle), rx.t(Msg::DfuChecking));
    }
}

// ── DfuConfirm: installed → update, the warnings, and Install / Cancel ──

/// The two confirm rows (Install / Cancel), neither guarded.
const N_CONFIRM_ITEMS: usize = 2;
const INSTALL: usize = 0;

/// Side inset (px) the confirm card's version table and notes keep from the panel edges — a version
/// string is right-aligned to `w - INSET`, never edge-to-edge (spec §1).
const INSET: i32 = 12;

/// The install confirm. Carries the scan report (the versions + no-undo fact) and the highlighted
/// option. Encoder **Install** posts [`DfuAction::Install`](crate::activity::DfuAction) and swaps to
/// the progress screen; **Back** / **Cancel** returns to the System menu.
#[derive(Debug)]
pub struct DfuConfirmScreen {
    report: DfuScanReport,
    selected: usize,
}

impl DfuConfirmScreen {
    pub fn new(report: DfuScanReport) -> Self {
        DfuConfirmScreen { report, selected: 0 }
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Turn(n) => super::list::on_turn(&mut self.selected, n, N_CONFIRM_ITEMS),
            Gesture::Press if self.selected == INSTALL => {
                // Arm: post the install one-shot (the board snapshots the rollback + arms + reboots)
                // and swap to the progress spinner. The confirm was pushed over the System menu.
                cx.activity.request_dfu(crate::activity::DfuAction::Install);
                Transition::Replace(Screen::DfuProgress(DfuProgressScreen::new()))
            }
            Gesture::Press => Transition::Pop, // Cancel
            Gesture::Back => Transition::Pop,  // Back = Cancel
            _ => Transition::None,
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
        let geo = super::GuardedRowsGeometry {
            x: 12,
            w: w - 24,
            top: h - 2 * 42 - 6 - 12,
            row_h: 42,
            gap: 6,
            label_dx: 16,
            label_dy: 9,
        };
        let items = [
            MenuItem { label: rx.t(Msg::DfuInstall), guard: false },
            MenuItem { label: rx.t(Msg::DfuCancel), guard: false },
        ];
        super::draw_guarded_rows(cv, &items, self.selected, rx.hold_progress, AMBER, geo);
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

// ── DfuProgress: "Preparing update..." until the board reboots ──

/// The arming-in-progress screen: the spinner over "Preparing update..." while the drain runs the
/// CRC pass + rollback snapshot + arm. The board reboots into the bootloader when the arm lands (no
/// further app frame — the LED takes over during the flash), so this screen ignores all input: an
/// arm can't be cancelled.
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
        self.spin.draw(cv, rx, rx.t(Msg::DfuTitle), rx.t(Msg::DfuPreparing));
    }
}

// ── DfuError: a typed scan error as a plain sentence ──

/// The scan-error card (issue #620 §2): a [`DfuScanError`] mapped to plain copy. Info-only — any
/// **Back** dismisses (like the nav failure card), returning to the System menu.
#[derive(Debug)]
pub struct DfuErrorScreen {
    error: DfuScanError,
}

impl DfuErrorScreen {
    pub fn new(error: DfuScanError) -> Self {
        DfuErrorScreen { error }
    }

    /// The scan error this card shows — lets the seam tests pin the error→card mapping.
    pub fn error(&self) -> DfuScanError {
        self.error
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
        let msg = match self.error {
            DfuScanError::NotFound => rx.t(Msg::DfuNotFound),
            DfuScanError::Unreadable => rx.t(Msg::DfuUnreadable),
            DfuScanError::Damaged => rx.t(Msg::DfuDamaged),
            DfuScanError::TooLarge => rx.t(Msg::DfuTooLarge),
            DfuScanError::TooFragmented => rx.t(Msg::DfuFragmented),
        };
        wrapped(cv, msg, w / 2, TITLE_BAR_H + 84, w - 32, INK);
    }
}

// ── DfuUpdated: the one-time post-update toast ──

/// The one-time "Updated to vX" toast the first healthy boot after an update shows (host-pushed via
/// [`App::notify_update_confirmed`](crate::App::notify_update_confirmed)). Info-only; any press/Back
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
/// [`App::notify_update_failed`](crate::App::notify_update_failed) from the board's boot-outcome
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
    use crate::screen::PoiScratch;
    use crate::settings::Settings;
    use crate::{AppState, Mode};

    fn report(installed: &str, staged: &str, first_install: bool) -> DfuScanReport {
        DfuScanReport::new(installed, staged, first_install)
    }

    /// Build a throwaway `Ctx`, run a gesture, and hand back the transition + the drained DFU
    /// one-shot the screen posted (if any).
    fn run(scr: &mut impl FnMut(&mut Ctx) -> Transition) -> (Transition, Option<DfuAction>) {
        let mut st = AppState::new(0, 0, 1.0);
        let mut act = Activity::new(Mode::Idle);
        let mut settings = Settings::default();
        let scratch = PoiScratch::new();
        let t = {
            let mut cx = Ctx {
                state: &mut st,
                activity: &mut act,
                settings: &mut settings,
                routes: &[],
                rides: &[],
                nav_profiles: &crate::NavProfiles::EMPTY,
                poi_scratch: &scratch,
                now_ms: 0,
            };
            scr(&mut cx)
        };
        (t, act.take_dfu_request())
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
        // Turn to the Cancel row, then press.
        let (_, _) = run(&mut |cx| scr.handle(Gesture::Turn(1), cx));
        let (t, posted) = run(&mut |cx| scr.handle(Gesture::Press, cx));
        assert!(matches!(t, Transition::Pop), "Cancel pops");
        assert_eq!(posted, None, "and arms nothing");

        let mut scr = DfuConfirmScreen::new(report("v1", "v2", false));
        let (t, posted) = run(&mut |cx| scr.handle(Gesture::Back, cx));
        assert!(matches!(t, Transition::Pop), "Back cancels");
        assert_eq!(posted, None);
    }

    /// The error card carries its variant and dismisses on Back/press.
    #[test]
    fn error_card_dismisses() {
        let mut scr = DfuErrorScreen::new(DfuScanError::TooFragmented);
        assert_eq!(scr.error(), DfuScanError::TooFragmented);
        let (t, _) = run(&mut |cx| scr.handle(Gesture::Back, cx));
        assert!(matches!(t, Transition::Pop));
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
