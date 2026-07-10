//! The SD-sideload firmware-update flow's screens (epic #615 S5, #620) — the user-facing half of
//! the DFU armer. Reached from **Settings → System → "Install update from card"**
//! ([`SystemScreen`](super::SystemScreen)); the scan/arm machinery runs board-side.
//!
//! Six static screens (no map plane), each a small typed state through the normal screen stack:
//!
//! - [`DfuCheckScreen`] — the brief "Checking card..." wait (spinner) after the scan is posted; the
//!   board's answer ([`App::notify_dfu_scan_result`](crate::App::notify_dfu_scan_result)) replaces
//!   it with the confirm screen or the error card. **Back** cancels the wait.
//! - [`DfuConfirmScreen`] — *installed → update* versions, the no-undo / same-version warnings, and
//!   the "the light blinks while the update installs" note. Encoder **Install** arms (posts
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

use super::{palette, title_frame, Ctx, MenuItem, Render, Screen, ScreenTick, Transition, TITLE_BAR_H};

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

// ── Multi-line centred body copy (author each catalog string on one line; wrap at draw time) ──

/// Draw `text` word-wrapped into centred [`Font::Label`] lines within `width_px`, the first line at
/// `top_y`, in `color`. Greedy over the monospace cell width; returns the `y` just past the last
/// line so a caller can stack more below it. A single word wider than the budget is left to clip
/// (versions and the like are short). Up to [`MAX_LINES`](Self) lines.
fn wrapped(cv: &mut impl Surface, text: &str, cx: i32, top_y: i32, width_px: i32, color: u16) -> i32 {
    const LH: i32 = 19; // Label line advance (cap ~18) + a hair of lead
    let char_w = Font::Label.char_width() as i32;
    let budget = (width_px / char_w).max(1) as usize;
    let mut y = top_y;
    let mut line: heapless::String<48> = heapless::String::new();
    for word in text.split(' ') {
        let extra = if line.is_empty() { word.len() } else { line.len() + 1 + word.len() };
        if extra > budget && !line.is_empty() {
            cv.text(&line, Point::new(cx, y), Font::Label, TextAlign::Center, color);
            y += LH;
            line.clear();
        }
        if !line.is_empty() {
            let _ = line.push(' ');
        }
        let _ = line.push_str(word);
    }
    if !line.is_empty() {
        cv.text(&line, Point::new(cx, y), Font::Label, TextAlign::Center, color);
        y += LH;
    }
    y
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

        // The two versions, stacked caption-over-value (a git-describe string is too wide to sit
        // beside its caption). Installed first, then the staged update — what "install" replaces
        // it with.
        let mut y = TITLE_BAR_H + 6;
        y = version_block(cv, w, y, rx.t(Msg::DfuInstalled), &self.report.installed);
        y = version_block(cv, w, y + 2, rx.t(Msg::DfuStaged), &self.report.staged);

        // Warnings (each a wrapped line): the same-version note, then the no-undo note when this is
        // a first install (no rollback snapshot exists — spec §2.4). Both warning-coloured.
        y += 6;
        if self.report.same_version() {
            y = wrapped(cv, rx.t(Msg::DfuSameVersion), w / 2, y, w - 24, WARNING);
        }
        if self.report.first_install {
            y = wrapped(cv, rx.t(Msg::DfuNoUndo), w / 2, y, w - 24, WARNING);
        }
        // The always-present note that the display goes dark + the LED blinks during the flash.
        wrapped(cv, rx.t(Msg::DfuLightBlinks), w / 2, y + 2, w - 24, SUBTEXT);

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

/// Draw one "caption over version" block centred at `top_y`: the olive caption (Label) above the
/// version string (Body, ink). Returns the `y` just past the block. Versions are never translated.
fn version_block(cv: &mut impl Surface, w: i32, top_y: i32, caption: &str, version: &Version) -> i32 {
    cv.text(caption, Point::new(w / 2, top_y), Font::Label, TextAlign::Center, palette::SUBTEXT);
    cv.text(version, Point::new(w / 2, top_y + 15), Font::Body, TextAlign::Center, palette::INK);
    top_y + 15 + Font::Body.cap_height() as i32
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
        draw_warning(cv, w / 2, TITLE_BAR_H + 46, 22);
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
        draw_check(cv, w / 2, TITLE_BAR_H + 56, 24);
        cv.text(rx.t(Msg::DfuUpdated), Point::new(w / 2, TITLE_BAR_H + 104), Font::Body, TextAlign::Center, INK);
        // The version, verbatim (never translated).
        cv.text(&self.version, Point::new(w / 2, TITLE_BAR_H + 134), Font::Body, TextAlign::Center, AMBER);
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
        draw_warning(cv, w / 2, TITLE_BAR_H + 46, 22);
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

// ── Shared icons (mirrors of the reset screen's warning + check glyphs) ──

/// An amber warning triangle with an ink exclamation, centred at `(cx, cy)`.
fn draw_warning(cv: &mut impl Surface, cx: i32, cy: i32, sz: i32) {
    use palette::*;
    cv.triangle(Point::new(cx, cy - sz), Point::new(cx - sz, cy + sz), Point::new(cx + sz, cy + sz), AMBER);
    cv.vline(cx, cy - sz / 4, sz / 2, 3, INK);
    cv.disc(Point::new(cx, cy + sz / 2 + 1), 2, INK);
}

/// A check mark in amber, centred near `(cx, cy)` — two strokes stepped out of discs.
fn draw_check(cv: &mut impl Surface, cx: i32, cy: i32, sz: i32) {
    fn seg(cv: &mut impl Surface, a: (i32, i32), b: (i32, i32)) {
        const N: i32 = 14;
        for k in 0..=N {
            let x = a.0 + (b.0 - a.0) * k / N;
            let y = a.1 + (b.1 - a.1) * k / N;
            cv.disc(Point::new(x, y), 3, palette::AMBER);
        }
    }
    seg(cv, (cx - sz, cy), (cx - sz / 3, cy + sz * 2 / 3));
    seg(cv, (cx - sz / 3, cy + sz * 2 / 3), (cx + sz, cy - sz * 2 / 3));
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
