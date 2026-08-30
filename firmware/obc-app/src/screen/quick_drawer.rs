//! The **universal quick drawer** (#1515 D2): the top sheet the Up+Select chord opens from
//! anywhere.
//!
//! Four device-wide controls, unlabelled by design — the symbols are established enough that
//! captions would only add noise, so the sheet names the *selected* one in a single line under the
//! row instead of writing four labels the rider already knows. Two of them open a nested page that
//! slides in horizontally while the sheet adapts its height: the brightness editor and the guarded
//! power confirmation.
//!
//! **The screen owns no device state.** Brightness is a persisted [`Settings`](crate::Settings)
//! row and the BLE switch is `Settings::ble_enabled`, both edited in place through
//! [`Ctx`](super::Ctx) so the App's one `==` diff arms the save. The only value that lives here is
//! the brightness the editor has *staged but not committed* — which is exactly what makes
//! Back-cancels-and-reverts free: the moment the editor closes, every reader falls back to the
//! committed row again.

use core::fmt::Write as _;

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::Msg;

use super::vocab::chrome::stroke2;
use super::vocab::sheet;
use super::{palette, Ctx, Render, Screen, ScreenTick, SettingsScreen, Transition};

/// How long the sheet takes to slide down from the top edge on open (ms).
///
/// **The one knob this sheet's open feel hangs on** (#1559). 220 ms landed in about four visible
/// steps on the panel and read as lag rather than as motion; this is the owner's "start at about
/// twice that" and it is a default to iterate on glass, not a measurement.
pub(crate) const OPEN_MS: u32 = 440;
/// How long a nested page takes to slide in, and the sheet to grow into its height (ms).
const SLIDE_MS: u32 = 180;
/// How long one step of the open costs the panel, and therefore the cadence the sheet asks to be
/// woken at (ms).
///
/// Measured on the LS021B7DD02, release build (#1559 bench rounds 1 and 2): a present costs
/// **8.4 ms** of whole-frame row hash plus **0.137 ms per pushed row**, and drawing the sheet
/// itself costs about **12 ms**. With the base frozen, this sheet's deepest step pushes its 104 px
/// root and lip — 8.4 + 15.3 + 12 ≈ 36 ms. So that is the step: one the panel can actually finish.
/// The 16 ms token it replaces asked for two steps in the time one takes, and the host missed every
/// other one. [`OPEN_MS`] divided by this is the step count: 440 / 36 ≈ 12.
const STEP_MS: u32 = 36;

/// Sheet height per page, in device pixels. Adaptive: the sheet uses only what its page needs.
const ROOT_H: i32 = 104;
const BRIGHTNESS_H: i32 = 136;
const POWER_H: i32 = 150;
const POWERING_OFF_H: i32 = 132;

/// One device-wide control on the sheet's root row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Control {
    Brightness,
    Ble,
    Settings,
    Power,
}

/// The controls this device actually has, in row order.
///
/// **A deviation from #1515, and a deliberate one.** The issue names *exactly four* icons. On a
/// platform whose panel has no light (see [`Backlight::available`](obc_ports::Backlight)), the
/// brightness row is dropped and the rider gets three. A control the hardware cannot honour is not
/// a smaller lie than a port that returns `Ok(())` — it is the same lie one layer up, with a slider
/// that moves, a check-mark that relocates and a setting that persists, over zero photons. The row
/// comes back the moment the hardware does — as it did on the board, whose `PanelBacklight` drives
/// a real PWM since #1558 — and nothing else about the sheet changes.
fn controls(backlight: bool) -> &'static [Control] {
    const WITH_LIGHT: [Control; 4] = [Control::Brightness, Control::Ble, Control::Settings, Control::Power];
    const NO_LIGHT: [Control; 3] = [Control::Ble, Control::Settings, Control::Power];
    if backlight {
        &WITH_LIGHT
    } else {
        &NO_LIGHT
    }
}

/// How many discrete backlight steps the brightness editor offers.
///
/// **There is no "off".** Level 0 is the dimmest *lit* step, not a dark panel: a rider who cannot
/// see the screen cannot find the control that turns it back on (owner ruling, #1515).
pub const BRIGHTNESS_LEVELS: u8 = obc_ports::BACKLIGHT_LEVELS;
/// The brightest level — the factory default, and the `range` ceiling of the settings row.
pub const BRIGHTNESS_MAX: u8 = BRIGHTNESS_LEVELS - 1;

/// The percentage a level is **captioned** at: 20 % … 100 %.
///
/// It is the rider's vocabulary for "one of five steps", not a duty cycle. What a host drives is
/// its own business — the board's PWM ladder is square-law and reaches 4 / 16 / 36 / 64 / 100 %
/// (`obc_platform::backlight`), because evenly spaced duty is not evenly spaced *perceived*
/// brightness. Evenly spaced captions are the honest label for evenly spaced steps.
pub(crate) fn brightness_percent(level: u8) -> u16 {
    (level.min(BRIGHTNESS_MAX) as u16 + 1) * (100 / BRIGHTNESS_LEVELS as u16)
}

/// The sheet's pages. `Root` is the icon row; the other three are nested pages that slide in
/// horizontally over it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Page {
    Root,
    Brightness,
    PowerConfirm,
    /// The terminal frame: the rider completed the guarded hold and the host is about to call the
    /// power-off port. Nothing dismisses it.
    PoweringOff,
}

impl Page {
    fn height(self) -> i32 {
        match self {
            Page::Root => ROOT_H,
            Page::Brightness => BRIGHTNESS_H,
            Page::PowerConfirm => POWER_H,
            Page::PoweringOff => POWERING_OFF_H,
        }
    }
}

/// A horizontal page transition in flight: where it came from, and when it started.
#[derive(Clone, Copy, Debug)]
struct Slide {
    from: Page,
    started_ms: u32,
}

/// The quick drawer's whole state: which page, which icon, and the level the editor has staged.
pub struct QuickDrawerScreen {
    /// When the open slide started — the clock of the **first frame that could draw the sheet**,
    /// and `None` until one has.
    ///
    /// The sheet is not handed a clock when it is built, and that is the fix for #1569. A chord is
    /// resolved *above* the pass, before the pass sets its `now_ms`, so a sheet stamped at
    /// construction carries the clock of the pass **before** the squeeze. On a host whose frames
    /// gap — the board's Map sleeps until something happens — that is seconds old, the first frame
    /// computes an elapsed far past [`OPEN_MS`], and the sheet is drawn already landed: the open
    /// cuts. Starting the clock on the first tick makes the open begin where it can first be seen,
    /// on every host and with no host having to say anything.
    opened_ms: Option<u32>,
    slide: Option<Slide>,
    page: Page,
    selected: u8,
    /// How much of the sheet the last reported tick put on the panel, in device pixels; `-1` before
    /// the first one.
    ///
    /// It is what makes the open **motion** rather than a cut (#1559). A step that would redraw the
    /// sheet where it already stands is not reported at all — the bench measured whole renders
    /// pushing zero rows — and the frame the sheet lands on is reported exactly when it moves the
    /// sheet, which is what the old `landed` edge was approximating.
    shown_h: i16,
    /// The draw of the screen below that this sheet **owes** — see [`needs_base`](Self::needs_base).
    needs_base: bool,
    /// The brightness level the editor is previewing. Meaningful only on [`Page::Brightness`] —
    /// off that page every reader falls back to the committed settings row, which is why Back
    /// reverts the live preview without storing anything to undo.
    staged: u8,
}

impl QuickDrawerScreen {
    /// A drawer that has begun to open, with the first control selected. Its slide starts on the
    /// first frame that ticks it — see [`opened_ms`](Self::opened_ms).
    pub fn opening() -> Self {
        QuickDrawerScreen {
            opened_ms: None,
            slide: None,
            page: Page::Root,
            selected: 0,
            staged: 0,
            shown_h: -1,
            needs_base: false,
        }
    }

    /// The brightness the panel should show **right now**: the editor's staged preview while it is
    /// open, and nothing (→ the committed row) everywhere else.
    pub(crate) fn staged_brightness(&self) -> Option<u8> {
        (self.page == Page::Brightness).then_some(self.staged)
    }

    /// Whether the guarded power confirmation is up — the page that draws the hold bar.
    pub(crate) fn selection_is_guarded(&self) -> bool {
        self.page == Page::PowerConfirm
    }

    /// Whether the rider completed the guarded hold: the host renders this frame and then calls
    /// the power-off port. Idempotent — the state is the page, so a host may poll it every frame.
    pub(crate) fn powering_off(&self) -> bool {
        self.page == Page::PoweringOff
    }

    /// The exact facts this drawer draws, for the pass's render key: the page, the selected icon,
    /// and the staged value. The committed value is the settings row, which the key names beside
    /// this; the sheet's animation is reported through [`ScreenTick`], not through the key.
    pub(crate) fn key(&self) -> (u8, u8, u8) {
        (self.page as u8, self.selected, self.staged)
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        // A page transition owns the input while it runs: acting on a half-drawn page would let a
        // fast double-press land on a row the rider cannot see yet. Asked, never retired — see
        // [`slide_running`](Self::slide_running).
        if self.slide_running(cx.now_ms) {
            return Transition::None;
        }
        match self.page {
            Page::Root => self.handle_root(g, cx),
            Page::Brightness => self.handle_brightness(g, cx),
            Page::PowerConfirm => self.handle_power(g, cx.now_ms),
            // The device is going away; nothing here has a meaning any more.
            Page::PoweringOff => Transition::None,
        }
    }

    /// Whether a page slide is still in flight at `now_ms` — the input gate's question, asked
    /// without answering the tick's (#1515 D5).
    ///
    /// Retiring a slide is [`settle`](Self::settle)'s edge, and that edge is what
    /// [`tick_timers`](Self::tick_timers) reads to arm the base draw the settling frame owes. Input
    /// runs first in a pass, so a gesture landing at or after the slide's end used to retire the
    /// slide silently and leave the tick nothing to read: the sheet kept its two pages' ink in the
    /// margin either side of it — and the 32 rows the editor gives back going 136 → 104 — or, with
    /// no render key moved, asked for no repaint at all and stayed half-slid. The gate is a pure
    /// read, so the gesture is accepted exactly as before.
    fn slide_running(&self, now_ms: u32) -> bool {
        self.slide.is_some_and(|s| now_ms.wrapping_sub(s.started_ms) < SLIDE_MS)
    }

    fn handle_root(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        let row = controls(cx.backlight);
        match g {
            Gesture::Step(n) => {
                self.selected = super::vocab::list::step_selection(self.selected as usize, n, row.len()) as u8;
                Transition::None
            }
            Gesture::Press => match row.get(self.selected as usize) {
                Some(Control::Brightness) => {
                    self.staged = cx.settings.brightness.min(BRIGHTNESS_MAX);
                    self.slide_to(Page::Brightness, cx.now_ms);
                    Transition::None
                }
                // The radio switch is the persisted setting itself: the App's before/after `==`
                // arms the save, and the board re-reads the row it already watches.
                Some(Control::Ble) => {
                    cx.settings.ble_enabled = !cx.settings.ble_enabled;
                    Transition::None
                }
                // Central settings **replace** the sheet, so Back out of settings lands on the
                // base screen rather than on a drawer the rider has finished with.
                Some(Control::Settings) => Transition::Replace(Screen::Settings(SettingsScreen::new())),
                Some(Control::Power) => {
                    self.slide_to(Page::PowerConfirm, cx.now_ms);
                    Transition::None
                }
                // Unreachable: the selection is stepped within `row`. A drawer opened on a platform
                // with a light and redrawn without one would land here rather than on a wrong row.
                None => Transition::None,
            },
            Gesture::Back => Transition::Pop,
            // Back-hold is the global escape, resolved above screen dispatch; it never arrives.
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    fn handle_brightness(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            // A value axis, not a ring: the ends clamp, so a rider holding Up cannot wrap from the
            // brightest step round to the dimmest.
            Gesture::Step(n) => {
                self.staged = (self.staged as i32 + n).clamp(0, BRIGHTNESS_MAX as i32) as u8;
                Transition::None
            }
            Gesture::Press => {
                cx.settings.brightness = self.staged;
                self.slide_to(Page::Root, cx.now_ms);
                Transition::None
            }
            // Cancel: the staged value is simply abandoned, so the live preview reverts to the
            // committed row on the very next frame.
            Gesture::Back => {
                self.slide_to(Page::Root, cx.now_ms);
                Transition::None
            }
            // Back-hold is the global escape, resolved above screen dispatch; it never arrives.
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    fn handle_power(&mut self, g: Gesture, now_ms: u32) -> Transition {
        match g {
            // Only a completed hold shuts the device down.
            Gesture::Hold => {
                self.slide_to(Page::PoweringOff, now_ms);
                Transition::None
            }
            // A tap never shuts down — it cancels, exactly like Back.
            Gesture::Press | Gesture::Back => {
                self.slide_to(Page::Root, now_ms);
                Transition::None
            }
            Gesture::Step(_) | Gesture::BackHold => Transition::None,
        }
    }

    /// The sheet's animation: the open slide, then any page slide, at the panel's step cadence.
    pub fn tick_timers(&mut self, now_ms: u32) -> ScreenTick {
        // This frame is the open's origin if no frame has been one yet (#1569).
        let opened_ms = *self.opened_ms.get_or_insert(now_ms);
        let settled = self.settle(now_ms);
        let sheet_h = self.sheet_height(now_ms);
        let visible = self.visible_height(now_ms, sheet_h);
        // The open is over when the sheet has **arrived**, not when its clock runs out: the
        // ease-out's last few per cent move no pixel, and the steps they would ask for are whole
        // renders that push nothing.
        let opening =
            if sheet_h > 0 && visible >= sheet_h { 0 } else { OPEN_MS.saturating_sub(now_ms.wrapping_sub(opened_ms)) };
        let sliding = self.slide.map_or(0, |s| SLIDE_MS.saturating_sub(now_ms.wrapping_sub(s.started_ms)));
        let moved = visible != self.shown_h as i32;
        // The base draw this sheet owes is a **debt**, so this adds to it and never clears it: a
        // pass may tick and then draw no frame at all, and only a frame that drew the base ends the
        // obligation ([`needs_base`](Self::needs_base)).
        self.needs_base |= sliding > 0 || settled;
        self.shown_h = visible as i16;
        // The wake is the time to the **next step boundary**, not a whole step from wherever this
        // poll happened to land: the sheet advances on those boundaries, so asking for a full step
        // off one carries the offset to the end and finishes the open a step late.
        let to_step = STEP_MS - now_ms.wrapping_sub(opened_ms) % STEP_MS;
        match [opening, sliding].into_iter().filter(|r| *r > 0).min() {
            // A page slide moves its two pages across a sheet that may not change height at all, so
            // it is a change whether or not the sheet grew.
            Some(remaining) => {
                ScreenTick { changed: sliding > 0 || moved, next_wake_ms: Some(to_step.min(remaining)), region: None }
            }
            // The frame a slide ends on still differs from the one before it; the frame the open
            // ends on differs only if it moved the sheet, and reporting one that did not is a whole
            // render spent on nothing.
            None if settled || moved => ScreenTick { changed: true, next_wake_ms: None, region: None },
            None => ScreenTick::idle(),
        }
    }

    /// Whether this sheet still **owes** the screen below a draw (#1559, #1515 D5).
    ///
    /// A **page slide**, and only that on this sheet: its two pages travel through the inset margin
    /// either side of the sheet, where the base shows, so every frame of one — including the frame
    /// it settles on, which is the last that can leave ink there — needs the base under it. A slide
    /// between pages of different heights also *shrinks* the sheet, and the rows it gives back are
    /// put back by the same draw. Everywhere else the frozen base's rows stand and the sheet is all
    /// that is drawn.
    ///
    /// It is a **debt, not a flag**: nothing but
    /// [`clear_base_debt`](Self::clear_base_debt) — called by the frame that actually drew the base
    /// — ends it. A tick that decided it per frame could have the obligation stolen from under it
    /// by a pass that ticked and drew nothing, or by input running first and retiring the slide
    /// before the tick could see the edge.
    pub(crate) fn needs_base(&self) -> bool {
        self.needs_base
    }

    /// Discharge the debt: the frame that drew the base has put back everything this sheet was not
    /// covering. Called at the frame boundary, which is the only place that answer exists.
    pub(crate) fn clear_base_debt(&mut self) {
        self.needs_base = false;
    }

    /// Begin a horizontal transition to `to`, which becomes the live page at once (so `handle`
    /// and the render key already speak about the destination) while the slide draws both.
    fn slide_to(&mut self, to: Page, now_ms: u32) {
        self.slide = Some(Slide { from: self.page, started_ms: now_ms });
        self.page = to;
        // From this frame on the two pages travel outside the sheet's own footprint, so the base
        // has to be under them — armed here rather than at the next tick, which would be one frame
        // late.
        self.needs_base = true;
    }

    /// Retire a finished slide. Returns whether this call is the one that retired it. The **tick**
    /// calls this and nothing else does: the edge it returns is what arms the settling frame's
    /// base draw (see [`slide_running`](Self::slide_running)).
    fn settle(&mut self, now_ms: u32) -> bool {
        let done = self.slide.is_some_and(|s| now_ms.wrapping_sub(s.started_ms) >= SLIDE_MS);
        if done {
            self.slide = None;
        }
        done
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        let sheet_h = self.sheet_height(rx.now_ms);
        let visible = self.visible_height(rx.now_ms, sheet_h);
        if visible == 0 {
            return;
        }
        // The sheet stays attached to the top edge: it slides down by drawing its full height with
        // its top off-screen, so the rounded bottom lip is what the rider sees arriving.
        let top = visible - sheet_h;
        cv.round(rect(4, top - 8, rx.w - 8, sheet_h + 8), 10, palette::PARCHMENT);
        cv.round_outline(rect(4, top - 8, rx.w - 8, sheet_h + 8), 10, palette::WOOD_LIGHT);
        // The grab lip, so the sheet reads as pulled down from the top rather than as a card.
        cv.round(rect(rx.w / 2 - 18, top + sheet_h - 11, 36, 4), 2, palette::WOOD_LIGHT);

        match self.slide {
            Some(slide) => {
                let t = sheet::slid(rx.now_ms, slide.started_ms, SLIDE_MS);
                // Going deeper pushes the old page left; returning to the root pulls it right.
                let back = self.page == Page::Root;
                let (out, incoming) = if back {
                    ((t * rx.w as f32) as i32, -((1.0 - t) * rx.w as f32) as i32)
                } else {
                    (-((t * rx.w as f32) as i32), ((1.0 - t) * rx.w as f32) as i32)
                };
                self.draw_page(cv, rx, slide.from, top, out);
                self.draw_page(cv, rx, self.page, top, incoming);
            }
            None => self.draw_page(cv, rx, self.page, top, 0),
        }
    }

    /// The sheet height this frame: the page's own, or the interpolation between two pages'
    /// while a slide runs — which is how the sheet grows and shrinks with its content.
    fn sheet_height(&self, now_ms: u32) -> i32 {
        let Some(slide) = self.slide else { return self.page.height() };
        let t = sheet::slid(now_ms, slide.started_ms, SLIDE_MS);
        let (from, to) = (slide.from.height() as f32, self.page.height() as f32);
        (from + (to - from) * t + 0.5) as i32
    }

    /// How much of the sheet has arrived from the top edge, on the open animation's ease-out —
    /// advanced in whole [`STEP_MS`] steps.
    ///
    /// The quantising is the pacing (#1559). A device wakes on more than its own timers, and a
    /// sheet that answered the raw clock would give a busy host a hundred one-pixel steps of a
    /// 104 px sheet, each one a whole frame the panel cannot finish. Reading the step boundary
    /// instead means the sheet moves exactly as often as it asked to be woken, and a wake between
    /// two steps draws the frame that is already there — which the tick then does not ask for.
    fn visible_height(&self, now_ms: u32, sheet_h: i32) -> i32 {
        // Before the first tick the open has not started, so a host that draws a sheet it has not
        // ticked draws no sheet — which is the frame the open begins from anyway.
        let Some(opened_ms) = self.opened_ms else { return 0 };
        let elapsed = now_ms.wrapping_sub(opened_ms);
        // The frame the sheet opens on is its **first step**, not a frame that draws nothing: the
        // chord costs the host a repaint whatever this returns, and #1559's rule is that no frame
        // of the open is spent on nothing. So a step boundary is counted from one step in.
        let stepped = (elapsed / STEP_MS + 1) * STEP_MS;
        (sheet_h as f32 * sheet::arrived(stepped, 0, OPEN_MS) + 0.5) as i32
    }

    fn draw_page(&self, cv: &mut impl Surface, rx: &Render, page: Page, top: i32, x: i32) {
        match page {
            Page::Root => self.draw_root(cv, rx, top, x),
            Page::Brightness => self.draw_brightness(cv, rx, top, x),
            Page::PowerConfirm => draw_power_confirm(cv, rx, top, x),
            Page::PoweringOff => draw_powering_off(cv, rx, top, x),
        }
    }

    /// The unlabelled icons, plus one line naming the selected one.
    fn draw_root(&self, cv: &mut impl Surface, rx: &Render, top: i32, x: i32) {
        const STEP: i32 = 57;
        let row = controls(rx.backlight);
        let first = x + (rx.w - STEP * (row.len() as i32 - 1)) / 2;
        for (i, control) in row.iter().enumerate() {
            let c = Point::new(first + i as i32 * STEP, top + 32);
            let selected = i as u8 == self.selected;
            // "On" is the amber disc; "off" is the recessive grey one. Only the two stateful
            // controls have an off state — settings and power are always simply available.
            let on = match control {
                Control::Brightness => true,
                Control::Ble => rx.settings.ble_enabled,
                Control::Settings | Control::Power => false,
            };
            let (fill, ink) = if on { (palette::AMBER, palette::INK) } else { (palette::CONTOUR, palette::PARCHMENT) };
            if selected {
                cv.disc(c, 24, palette::INK);
                cv.disc(c, 22, palette::PARCHMENT);
            }
            cv.disc(c, if selected { 19 } else { 20 }, fill);
            match control {
                Control::Brightness => draw_bulb(cv, c, ink, fill),
                Control::Ble => draw_ble_rune(cv, c, ink),
                Control::Settings => draw_gear(cv, c, ink, fill),
                Control::Power => draw_power(cv, c, ink, fill),
            }
        }

        let caption = match row.get(self.selected as usize) {
            Some(Control::Brightness) => rx.t(Msg::QuickBrightness),
            Some(Control::Ble) => {
                rx.t(if rx.settings.ble_enabled { Msg::QuickBluetoothOn } else { Msg::QuickBluetoothOff })
            }
            Some(Control::Settings) => rx.t(Msg::QuickSettings),
            Some(Control::Power) => rx.t(Msg::QuickPower),
            None => "",
        };
        cv.text(caption, Point::new(x + rx.w / 2, top + 67), Font::Label, TextAlign::Center, palette::WOOD);
    }

    /// The nested value editor: the staged percentage as a title, and a five-notch slider whose
    /// tick marks the level already committed.
    fn draw_brightness(&self, cv: &mut impl Surface, rx: &Render, top: i32, x: i32) {
        let mut title: heapless::String<24> = heapless::String::new();
        let _ = write!(title, "{} {}%", rx.t(Msg::QuickBrightness), brightness_percent(self.staged));
        cv.text(&title, Point::new(x + 14, top + 18), Font::Label, TextAlign::Left, palette::WOOD);
        cv.hline(x + 12, top + 47, rx.w - 24, palette::RULE);
        draw_bulb(cv, Point::new(x + 26, top + 82), palette::WOOD, palette::PARCHMENT);

        let (x0, x1, y) = (x + 52, x + rx.w - 24, top + 82);
        cv.round(rect(x0, y - 2, x1 - x0, 5), 2, palette::PARCHMENT_SHADE);
        for level in 0..BRIGHTNESS_LEVELS {
            let px = sheet::notch_x(x0, x1, level, BRIGHTNESS_LEVELS);
            cv.vline(px, y - 6, 13, 1, palette::SUBTEXT);
            if level == rx.settings.brightness.min(BRIGHTNESS_MAX) {
                sheet::committed_tick(cv, px, y + 22, palette::WOOD);
            }
        }
        let knob = sheet::notch_x(x0, x1, self.staged, BRIGHTNESS_LEVELS);
        cv.disc(Point::new(knob, y), 8, palette::AMBER);
        cv.disc(Point::new(knob, y), 3, palette::INK);
    }
}

/// The guarded confirmation: a warning-red power glyph, the question, and the established
/// segmented hold bar filling from the live Select progress.
fn draw_power_confirm(cv: &mut impl Surface, rx: &Render, top: i32, x: i32) {
    let icon = Point::new(x + rx.w / 2, top + 32);
    cv.disc(icon, 21, palette::WARNING);
    draw_power(cv, icon, palette::PARCHMENT, palette::WARNING);
    cv.text(
        rx.t(Msg::QuickPowerConfirm),
        Point::new(x + rx.w / 2, top + 59),
        Font::Label,
        TextAlign::Center,
        palette::INK,
    );
    cv.text(
        rx.t(Msg::QuickPowerHold),
        Point::new(x + rx.w / 2, top + 83),
        Font::Label,
        TextAlign::Center,
        palette::WOOD,
    );

    const SEGMENTS: i32 = 5;
    const SEG_W: i32 = 32;
    const GAP: i32 = 5;
    let x0 = x + (rx.w - (SEGMENTS * SEG_W + (SEGMENTS - 1) * GAP)) / 2;
    let filled = rx.hold_progress.clamp(0.0, 1.0) * SEGMENTS as f32;
    for i in 0..SEGMENTS {
        let area = rect(x0 + i * (SEG_W + GAP), top + 111, SEG_W, 10);
        cv.round(area, 3, if filled > i as f32 { palette::WARNING } else { palette::PARCHMENT_SHADE });
        cv.round_outline(area, 3, palette::WARNING);
    }
}

/// The last frame the panel holds while the host calls the power-off port.
fn draw_powering_off(cv: &mut impl Surface, rx: &Render, top: i32, x: i32) {
    draw_power(cv, Point::new(x + rx.w / 2, top + 43), palette::WARNING, palette::PARCHMENT);
    cv.text(
        rx.t(Msg::QuickPoweringOff),
        Point::new(x + rx.w / 2, top + 75),
        Font::Body,
        TextAlign::Center,
        palette::INK,
    );
}

/// A compact outline bulb with a filament and a screw base, at the sheet's 22 px icon scale.
fn draw_bulb(cv: &mut impl Surface, c: Point, color: u16, bg: u16) {
    let dome = Point::new(c.x, c.y - 4);
    cv.disc(dome, 9, color);
    cv.disc(dome, 6, bg);
    cv.fill(rect(c.x - 10, c.y + 1, 21, 12), bg);
    stroke2(cv, Point::new(c.x - 8, c.y - 1), Point::new(c.x - 4, c.y + 5), color);
    stroke2(cv, Point::new(c.x + 8, c.y - 1), Point::new(c.x + 4, c.y + 5), color);
    cv.line(Point::new(c.x - 3, c.y - 2), Point::new(c.x, c.y + 2), color);
    cv.line(Point::new(c.x + 3, c.y - 2), Point::new(c.x, c.y + 2), color);
    cv.hline(c.x - 4, c.y + 6, 9, color);
    cv.hline(c.x - 4, c.y + 9, 9, color);
    cv.hline(c.x - 2, c.y + 12, 5, color);
}

/// The Bluetooth bind-rune at sheet scale: the title bar's geometry on the panel's 2 px stroke, so
/// it carries the same visual mass as the filled glyphs beside it.
fn draw_ble_rune(cv: &mut impl Surface, c: Point, color: u16) {
    let (half, quarter) = (11, 5);
    let stem = c.x - 2;
    let (top, mid, bot) = (Point::new(stem, c.y - half), Point::new(stem, c.y), Point::new(stem, c.y + half));
    let up_tip = Point::new(c.x + 6, c.y - half + quarter);
    let lo_tip = Point::new(c.x + 6, c.y + half - quarter);
    let up_left = Point::new(c.x - 8, c.y - half + quarter);
    let lo_left = Point::new(c.x - 8, c.y + half - quarter);
    stroke2(cv, top, bot, color);
    stroke2(cv, top, up_tip, color);
    stroke2(cv, up_tip, mid, color);
    stroke2(cv, bot, lo_tip, color);
    stroke2(cv, lo_tip, mid, color);
    stroke2(cv, up_tip, lo_left, color);
    stroke2(cv, lo_tip, up_left, color);
}

/// A filled pixel gear: eight square teeth around a punched hub.
fn draw_gear(cv: &mut impl Surface, c: Point, color: u16, bg: u16) {
    const TOOTH: i32 = 5;
    const HALF: i32 = TOOTH / 2;
    for (dx, dy) in [(0, -9), (9, 0), (0, 9), (-9, 0), (-7, -7), (7, -7), (7, 7), (-7, 7)] {
        cv.fill(rect(c.x + dx - HALF, c.y + dy - HALF, TOOTH, TOOTH), color);
    }
    cv.disc(c, 9, color);
    cv.disc(c, 4, bg);
}

/// The universal power symbol: a ring with its top arc broken, and a heavy stem through the gap.
fn draw_power(cv: &mut impl Surface, c: Point, color: u16, bg: u16) {
    cv.disc(c, 10, color);
    cv.disc(c, 7, bg);
    cv.fill(rect(c.x - 3, c.y - 12, 7, 8), bg);
    cv.vline(c.x - 1, c.y - 14, 11, 3, color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Mode;
    use crate::screen::test_ctx;
    use crate::{Activity, AppState, Settings};

    /// A drawer with its open + slide animations already finished, so `handle` acts immediately.
    /// The first tick is the open's origin, so it is taken a whole [`OPEN_MS`] before `now_ms`.
    fn settled(now_ms: u32) -> QuickDrawerScreen {
        let mut d = QuickDrawerScreen::opening();
        d.tick_timers(now_ms.saturating_sub(OPEN_MS));
        d
    }

    struct World {
        state: AppState,
        activity: Activity,
        settings: Settings,
        now_ms: u32,
    }

    impl World {
        fn new() -> Self {
            World {
                state: AppState::new(0, 0, 1.0),
                activity: Activity::new(Mode::Idle),
                settings: Settings::default(),
                now_ms: 1_000,
            }
        }

        /// Apply one gesture, then step the clock past any slide it started.
        fn press(&mut self, d: &mut QuickDrawerScreen, g: Gesture) -> Transition {
            let now_ms = self.now_ms;
            let t =
                d.handle(g, &mut Ctx { now_ms, ..test_ctx(&mut self.state, &mut self.activity, &mut self.settings) });
            self.now_ms += SLIDE_MS;
            t
        }
    }

    /// The percentages the five levels are captioned at — and the absence of a zero.
    #[test]
    fn the_five_levels_span_twenty_to_a_hundred_percent() {
        let percents: heapless::Vec<u16, 8> = (0..BRIGHTNESS_LEVELS).map(brightness_percent).collect();
        assert_eq!(percents.as_slice(), [20, 40, 60, 80, 100]);
        assert!(percents.iter().all(|p| *p > 0), "no level turns the panel off");
    }

    /// The BLE icon flips the persisted radio row in place — the App's `==` diff is what turns that
    /// into a save, so the screen writes the field and nothing else.
    #[test]
    fn the_ble_icon_toggles_the_persisted_radio_row() {
        let mut w = World::new();
        let mut d = settled(w.now_ms);
        assert!(w.settings.ble_enabled, "the radio starts on");

        w.press(&mut d, Gesture::Step(1)); // LIGHT -> BLE
        w.press(&mut d, Gesture::Press);
        assert!(!w.settings.ble_enabled, "the press switched the radio off");
        w.press(&mut d, Gesture::Press);
        assert!(w.settings.ble_enabled, "and back on");
    }

    /// Brightness stages, previews, commits — and Back both cancels the commit **and** takes the
    /// live preview away with it.
    #[test]
    fn brightness_stages_a_live_preview_that_select_commits_and_back_reverts() {
        let mut w = World::new();
        w.settings.brightness = 2;
        let mut d = settled(w.now_ms);

        w.press(&mut d, Gesture::Press); // open the editor on the committed level
        assert_eq!(d.staged_brightness(), Some(2), "the editor opens on what is committed");
        w.press(&mut d, Gesture::Step(1));
        assert_eq!(d.staged_brightness(), Some(3), "Up/Down previews live");
        assert_eq!(w.settings.brightness, 2, "…but commits nothing yet");

        w.press(&mut d, Gesture::Back);
        assert_eq!(d.staged_brightness(), None, "cancelled: the panel falls back to the committed row");
        assert_eq!(w.settings.brightness, 2, "Back cancels the edit");

        w.press(&mut d, Gesture::Press);
        w.press(&mut d, Gesture::Step(2));
        w.press(&mut d, Gesture::Press);
        assert_eq!(w.settings.brightness, 4, "Select commits the staged level");
        assert_eq!(d.staged_brightness(), None, "and the editor is closed");
    }

    /// The value axis clamps at both ends instead of wrapping, and never reaches an "off".
    #[test]
    fn the_brightness_axis_clamps_at_both_ends() {
        let mut w = World::new();
        w.settings.brightness = 0;
        let mut d = settled(w.now_ms);
        w.press(&mut d, Gesture::Press);
        w.press(&mut d, Gesture::Step(-3));
        assert_eq!(d.staged_brightness(), Some(0), "the dimmest lit level is the floor");
        w.press(&mut d, Gesture::Step(9));
        assert_eq!(d.staged_brightness(), Some(BRIGHTNESS_MAX), "…and the brightest is the ceiling");
    }

    /// Power needs a completed hold: a tap on the confirmation cancels it, Back cancels it, and
    /// only `Hold` reaches the terminal page the host powers off from.
    #[test]
    fn power_requires_a_completed_hold_and_a_tap_cancels() {
        let mut w = World::new();
        let mut d = settled(w.now_ms);
        w.press(&mut d, Gesture::Step(3)); // -> POWER
        w.press(&mut d, Gesture::Press);
        assert!(d.selection_is_guarded(), "the confirmation is up and draws its hold bar");
        assert!(!d.powering_off());

        w.press(&mut d, Gesture::Press);
        assert_eq!(d.page, Page::Root, "a tap never shuts down");

        w.press(&mut d, Gesture::Press); // -> confirm again
        w.press(&mut d, Gesture::Back);
        assert_eq!(d.page, Page::Root, "and Back cancels too");

        w.press(&mut d, Gesture::Press);
        w.press(&mut d, Gesture::Hold);
        assert!(d.powering_off(), "only a completed hold gets there");
    }

    /// The settings icon **replaces** the sheet, so Back out of central settings lands on the base
    /// screen rather than back inside a drawer.
    #[test]
    fn the_settings_icon_replaces_the_sheet() {
        let mut w = World::new();
        let mut d = settled(w.now_ms);
        w.press(&mut d, Gesture::Step(2)); // -> SETTINGS
        let t = w.press(&mut d, Gesture::Press);
        assert!(matches!(t, Transition::Replace(Screen::Settings(_))));
    }

    /// The wake asks for the **next step boundary**, not a whole step from wherever the poll landed
    /// — otherwise a device that wakes off-boundary carries the offset to the end and finishes the
    /// open a step late. The mutant is `STEP_MS.min(remaining)`.
    #[test]
    fn the_wake_lands_on_the_next_step_boundary() {
        let mut d = QuickDrawerScreen::opening();
        d.tick_timers(0); // the frame the open starts on
        assert_eq!(d.tick_timers(STEP_MS + 5).next_wake_ms, Some(STEP_MS - 5), "five into a step, ask for the rest");
        assert_eq!(d.tick_timers(STEP_MS * 2).next_wake_ms, Some(STEP_MS), "on a boundary, ask for a whole step");
    }

    /// A page transition owns the input while it runs: a press landing mid-slide changes nothing.
    #[test]
    fn a_press_during_a_slide_is_ignored() {
        let mut w = World::new();
        let mut d = settled(w.now_ms);
        let now_ms = w.now_ms;
        d.handle(Gesture::Press, &mut Ctx { now_ms, ..test_ctx(&mut w.state, &mut w.activity, &mut w.settings) });
        // Still sliding into the editor: a step must not move the staged value.
        let staged = d.staged_brightness();
        d.handle(
            Gesture::Step(1),
            &mut Ctx { now_ms: now_ms + SLIDE_MS / 2, ..test_ctx(&mut w.state, &mut w.activity, &mut w.settings) },
        );
        assert_eq!(d.staged_brightness(), staged, "a mid-slide press acts on nothing");
    }

    /// The sheet arrives monotonically and lands exactly on its page height.
    #[test]
    fn the_sheet_slides_in_monotonically_and_lands_exactly() {
        let mut d = QuickDrawerScreen::opening();
        d.tick_timers(1_000); // the frame the open starts on
        let target = ROOT_H;
        let quarter = OPEN_MS / 4;
        let frames: heapless::Vec<i32, 8> = [0, quarter, quarter * 2, quarter * 3, OPEN_MS]
            .iter()
            .map(|dt| d.visible_height(1_000 + dt, target))
            .collect();
        assert!(frames[0] > 0, "the sheet's first step is on the frame it opens on, not one step later");
        assert_eq!(frames[4], target, "and the sheet lands exactly on its height");
        assert!(frames.windows(2).all(|p| p[0] < p[1]), "monotonic: {frames:?}");
    }

    /// **The open is paced by the two constants and nothing else** (#1559): the sheet asks to be
    /// woken every [`STEP_MS`], it asks for as many steps as [`OPEN_MS`] pays for, and each one
    /// moves the sheet. Changing either constant changes the motion — which is the whole point of
    /// their being the tunables an on-glass round turns.
    #[test]
    fn the_open_takes_open_ms_in_steps_of_step_ms_and_every_step_moves_the_sheet() {
        let mut d = QuickDrawerScreen::opening();
        let (mut ms, mut heights) = (0u32, heapless::Vec::<i32, 32>::new());
        // Poll at 1 ms, the finest any host could: what the sheet asks for is what it gets, and a
        // poll between two steps must cost nothing.
        while ms < OPEN_MS * 2 {
            let tick = d.tick_timers(ms);
            if tick.changed {
                let _ = heights.push(d.visible_height(ms, ROOT_H));
            }
            ms += 1;
        }
        assert!(heights.windows(2).all(|p| p[0] < p[1]), "no step redraws the sheet where it stands: {heights:?}");
        assert_eq!(heights.last(), Some(&ROOT_H), "the last step is the sheet landed");
        // Real motion, and inside the panel's budget: about `OPEN_MS / STEP_MS` steps, allowing for
        // the ease-out finishing a shade early (the last few per cent move no pixel).
        let steps = heights.len() as u32;
        assert!(
            (OPEN_MS / STEP_MS / 2..=OPEN_MS / STEP_MS + 1).contains(&steps),
            "{steps} steps for a {OPEN_MS} ms open at a {STEP_MS} ms cadence"
        );
        assert!(steps >= 8, "an open that reads as motion is many steps, not the four the panel used to show");
    }

    /// **The open starts on the frame that can first draw it** (#1569), whatever the host was
    /// doing before the squeeze. A chord is resolved above the pass, so the sheet is built with no
    /// clock at all — and this is what that buys: a board whose Map slept for seconds still gets
    /// the whole slide.
    ///
    /// The mutant is stamping the open at construction from the host's clock: with the first frame
    /// eight seconds after the pass in front of it, the sheet is drawn landed on frame one and the
    /// open is a cut.
    #[test]
    fn the_open_starts_on_the_first_frame_and_not_on_a_clock_from_before_the_squeeze() {
        // The board's idle Map: the pass in front of the squeeze is seconds back, and the first
        // frame of the open is the pass the chord woke.
        let first_ms = 8_000;
        let mut d = QuickDrawerScreen::opening();
        let (mut ms, mut heights) = (first_ms, heapless::Vec::<i32, 32>::new());
        while ms < first_ms + OPEN_MS * 2 {
            if d.tick_timers(ms).changed {
                let _ = heights.push(d.visible_height(ms, ROOT_H));
            }
            ms += 1;
        }
        let first = *heights.first().expect("the open reported at least one step");
        assert!(first > 0 && first < ROOT_H, "the frame the squeeze woke draws the first step, not the landed sheet");
        assert_eq!(heights.last(), Some(&ROOT_H), "and lands on its height");
        assert!(heights.len() >= 8, "a sparsely woken host still gets the whole slide: {heights:?}");
    }

    /// A settled sheet is **silent**: it asks for no wake and no repaint, however often it is
    /// polled. The frozen base under it depends on that.
    #[test]
    fn a_settled_sheet_asks_for_nothing() {
        let mut d = QuickDrawerScreen::opening();
        for ms in 0..OPEN_MS * 2 {
            d.tick_timers(ms);
        }
        for ms in OPEN_MS * 2..OPEN_MS * 2 + 500 {
            assert_eq!(d.tick_timers(ms), ScreenTick::idle(), "a landed sheet is quiet at {ms} ms");
        }
        assert!(!d.needs_base(), "…and asks for nothing under it either");
    }

    /// A page slide **asks for the base under it**, both ways: its two pages travel through the
    /// margin either side of the sheet, and coming back out of a taller page gives rows back.
    ///
    /// What stops it asking is the **draw**, not the next tick (#1515 D5): a tick puts no pixel
    /// back, so the debt outlives every one of them until
    /// [`clear_base_debt`](QuickDrawerScreen::clear_base_debt), and no tick after that re-arms it.
    #[test]
    fn a_page_slide_asks_for_the_base_and_a_settled_page_stops_asking() {
        let mut w = World::new();
        let mut d = settled(w.now_ms);
        for ms in w.now_ms..w.now_ms + 10 {
            d.tick_timers(ms);
        }
        assert!(!d.needs_base(), "the landed root page covers what it covers");

        let now_ms = w.now_ms;
        d.handle(Gesture::Press, &mut Ctx { now_ms, ..test_ctx(&mut w.state, &mut w.activity, &mut w.settings) });
        assert!(d.needs_base(), "the slide into the taller editor is already outside the sheet");
        for ms in now_ms..now_ms + SLIDE_MS + 1 {
            d.tick_timers(ms);
        }
        assert!(d.needs_base(), "the frame the slide settles on is still the slide");
        d.tick_timers(now_ms + SLIDE_MS + 2);
        assert!(d.needs_base(), "…and a tick that drew nothing still owes it");
        d.clear_base_debt();
        d.tick_timers(now_ms + SLIDE_MS + 3);
        assert!(!d.needs_base(), "…and the settled editor covers what it covers again");

        // Back out: the same, and this slide also shrinks the sheet 136 -> 104, so the 32 rows it
        // gives back are put back by the draw the slide was already asking for.
        let now_ms = now_ms + SLIDE_MS + 3;
        d.handle(Gesture::Back, &mut Ctx { now_ms, ..test_ctx(&mut w.state, &mut w.activity, &mut w.settings) });
        for ms in now_ms..now_ms + SLIDE_MS {
            d.tick_timers(ms);
            assert!(d.needs_base(), "a slide that gives rows back needs the base at {ms} ms");
        }
        d.tick_timers(now_ms + SLIDE_MS);
        assert!(d.needs_base(), "…including the frame it settles on, the last that can leave ink in the margin");
        d.clear_base_debt();
        d.tick_timers(now_ms + SLIDE_MS + 1);
        assert!(!d.needs_base(), "and the shorter root page covers what it covers");
    }

    /// **A press landing as the slide lands does not take the base draw with it** (#1515 D5).
    ///
    /// Input runs before the tick in one pass, so a gesture at or after `slide start + SLIDE_MS` —
    /// well inside an ordinary double-tap — used to retire the slide itself, through the `settle`
    /// call `handle` opened with. The tick then found no edge and *assigned* `needs_base = false`,
    /// so the settling frame lost the base draw it owed: the outgoing page's ink stayed in the 4 px
    /// margin either side of the sheet, and the 32 rows the editor gives back going 136 → 104 stayed
    /// parchment.
    ///
    /// Every frame of the slide is modelled as it really runs — it draws the base, so it discharges
    /// the debt, and the next tick has to arm it again. The mutant is `self.settle(cx.now_ms)` back
    /// at the top of `handle`: both halves below fail.
    #[test]
    fn a_press_as_the_slide_lands_does_not_spend_the_base_draw_it_owes() {
        let mut w = World::new();
        w.settings.brightness = 2; // room to step in both directions
        let mut d = settled(w.now_ms);
        let start = w.now_ms;
        d.handle(
            Gesture::Press,
            &mut Ctx { now_ms: start, ..test_ctx(&mut w.state, &mut w.activity, &mut w.settings) },
        ); // → the brightness editor, 104 -> 136

        // The slide's own frames. Each draws the base and therefore pays what it owed.
        for ms in start..start + SLIDE_MS {
            assert!(d.tick_timers(ms).changed, "a frame of the slide is a frame the host renders");
            assert!(d.needs_base(), "…and it is drawn over the base, at {ms} ms");
            d.clear_base_debt();
        }

        // The settling frame, with a gesture landing on exactly it.
        let landed = start + SLIDE_MS;
        d.handle(
            Gesture::Step(1),
            &mut Ctx { now_ms: landed, ..test_ctx(&mut w.state, &mut w.activity, &mut w.settings) },
        );
        let tick = d.tick_timers(landed);
        assert!(d.needs_base(), "the settling frame still owes the margin the two pages travelled through");
        assert!(tick.changed, "…and is still asked for, so the pages do not stay half-slid");
        assert_eq!(d.staged_brightness(), Some(3), "the gesture itself is accepted exactly as before");
    }

    /// **Every string this sheet draws fits it, in all four languages** (#1515 D5).
    ///
    /// The context sheet's copy has been measured since D4a; this one's has been measured by nobody,
    /// and it is the sheet with the least room — its captions are centred on a 232 px card, so an
    /// overrun is clipped at both ends rather than running into a control.
    ///
    /// Two constraints have almost nothing left, and they are pinned by name below rather than left
    /// in PR prose: `es "BLUETOOTH INACTIVO"` is **exactly** the 216 px budget, and the terminal
    /// power line is 210 px of it in the wider `Font::Body`. A longer translation of any of the
    /// eight fails here instead of on the panel.
    #[test]
    fn every_quick_drawer_string_fits_the_sheet_in_every_language() {
        use crate::i18n::t;
        use crate::settings::Language;
        use obc_render::text::text_width;

        const W: i32 = 240;
        const MIN_CLEAR: i32 = 8;
        // `draw` lays the sheet out as `rect(4, .., w - 8, ..)`: a 232 px card inset 4 px each side.
        let card_w = W - 8;
        // A centred line has to clear both edges, so it loses the clearance twice.
        let centred_room = card_w - MIN_CLEAR * 2;
        // `draw_brightness` writes its title left-aligned at x + 14, and the card ends at 236.
        let title_room = W - 4 - 14 - MIN_CLEAR;
        assert_eq!((centred_room, title_room), (216, 214), "the sheet's two budgets, pinned");

        let (mut worst_label, mut worst_body) = (0, 0);
        for lang in [Language::En, Language::De, Language::Fr, Language::Es] {
            // The root row's caption line, both BLE states, and the two lines of the guarded
            // confirmation — every one of them centred in `Font::Label`.
            for msg in [
                Msg::QuickBrightness,
                Msg::QuickBluetoothOn,
                Msg::QuickBluetoothOff,
                Msg::QuickSettings,
                Msg::QuickPower,
                Msg::QuickPowerConfirm,
                Msg::QuickPowerHold,
            ] {
                let s = t(msg, lang);
                let px = text_width(s, Font::Label) as i32;
                assert!(px <= centred_room, "{lang:?}: {s:?} ({px} px) overruns the {centred_room} px sheet");
                worst_label = worst_label.max(px);
            }
            // The terminal frame's one line is the sheet's only `Font::Body` string, and Body is the
            // wider tier — so it is measured on its own budget rather than assumed to be safer.
            let off = t(Msg::QuickPoweringOff, lang);
            let px = text_width(off, Font::Body) as i32;
            assert!(px <= centred_room, "{lang:?}: {off:?} ({px} px in Body) overruns the {centred_room} px sheet");
            worst_body = worst_body.max(px);
            // The brightness editor's title is that caption again plus the ` 100%` the `write!`
            // glues on, left-aligned at the page's own inset.
            let title =
                text_width(t(Msg::QuickBrightness, lang), Font::Label) as i32 + text_width(" 100%", Font::Label) as i32;
            assert!(title <= title_room, "{lang:?}: the brightness title ({title} px) overruns {title_room} px");
        }

        assert_eq!(worst_label, 216, "es \"BLUETOOTH INACTIVO\" in Label, pinned");
        assert_eq!(centred_room - worst_label, 0, "…with nothing left: one more glyph does not fit the sheet");
        assert_eq!(worst_body, 210, "en \"POWERING OFF...\" / de \"SCHALTET AUS...\" in Body, pinned");
        assert_eq!(centred_room - worst_body, 6, "…with 6 px to spare");
    }
}
