//! The universal quick drawer: the top sheet the Up+Select chord opens from anywhere.
//!
//! The device-wide controls are unlabelled, because the symbols are established enough that four
//! captions would only add noise; the sheet names the selected one in a single line under the row
//! instead. Two of them open a nested page that slides in horizontally while the sheet adapts its
//! height: the brightness editor and the guarded power confirmation.
//!
//! The screen owns no device state. Brightness is a persisted [`Settings`](crate::Settings) row,
//! edited in place through [`Ctx`](super::Ctx) so the App's one `==` diff arms the save. The only
//! value that lives here is the brightness the editor has staged but not committed, which is what
//! makes a Back that cancels and reverts free.

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
use super::vocab::sheet::{self, Edge, SheetMotion, SheetTiming};
use super::{palette, Ctx, Render, Screen, ScreenTick, SettingsPage, Transition};

/// How long the sheet takes to slide down from the top edge on open (ms). A default to iterate on
/// glass, not a measurement.
pub(crate) const OPEN_MS: u32 = 440;
/// How long a nested page takes to slide in, and the sheet to grow into its height (ms).
const SLIDE_MS: u32 = 180;
/// How long one step of the open costs the panel, and therefore the cadence the sheet asks to be
/// woken at (ms). A step the panel can finish, so the host does not miss every other one.
const STEP_MS: u32 = 36;
/// The motion the shared sheet engine runs this sheet on.
pub(crate) const MOTION: SheetTiming = SheetTiming { open_ms: OPEN_MS, slide_ms: SLIDE_MS, step_ms: STEP_MS };

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

/// The controls this device actually has, in row order. On a platform whose panel has no light the
/// brightness row is dropped, because a slider that moves over zero photons is a lie. The row comes
/// back the moment the hardware does, and nothing else about the sheet changes.
fn controls(backlight: bool) -> &'static [Control] {
    const WITH_LIGHT: [Control; 4] = [Control::Brightness, Control::Ble, Control::Settings, Control::Power];
    const NO_LIGHT: [Control; 3] = [Control::Ble, Control::Settings, Control::Power];
    if backlight {
        &WITH_LIGHT
    } else {
        &NO_LIGHT
    }
}

/// How many discrete backlight steps the brightness editor offers. There is no "off": level 0 is
/// the dimmest lit step, because a rider who cannot see the screen cannot find the control that
/// turns it back on.
pub const BRIGHTNESS_LEVELS: u8 = obc_ports::BACKLIGHT_LEVELS;
/// The brightest level: the factory default, and the ceiling of the settings row.
pub const BRIGHTNESS_MAX: u8 = BRIGHTNESS_LEVELS - 1;

/// The percentage a level is captioned at. It is the rider's vocabulary for one of five steps, not
/// a duty cycle: a host's own PWM ladder is not linear, because evenly spaced duty is not evenly
/// spaced perceived brightness.
pub(crate) fn brightness_percent(level: u8) -> u16 {
    (level.min(BRIGHTNESS_MAX) as u16 + 1) * (100 / BRIGHTNESS_LEVELS as u16)
}

/// The sheet's pages. `Root` is the icon row; the others slide in horizontally over it.
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

/// The quick drawer's whole state: which page, which icon, and the level the editor has staged.
pub struct QuickDrawerScreen {
    /// The open, the page slide and the base-draw debt, on this sheet's own [`MOTION`].
    pub(crate) motion: SheetMotion,
    page: Page,
    /// The page a running slide travels out of. Meaningful only while [`SheetMotion`] reports one.
    slide_from: Page,
    selected: u8,
    /// The brightness level the editor is previewing. Meaningful only on [`Page::Brightness`]: off
    /// that page every reader falls back to the committed settings row, which is why Back reverts
    /// the live preview without storing anything to undo.
    staged: u8,
}

impl QuickDrawerScreen {
    /// A drawer that has begun to open, with the first control selected. Its slide starts on the
    /// first frame that ticks it, not on the pass the chord was resolved in.
    pub fn opening() -> Self {
        QuickDrawerScreen {
            motion: SheetMotion::opening(),
            page: Page::Root,
            slide_from: Page::Root,
            selected: 0,
            staged: 0,
        }
    }

    /// The brightness the panel should show now: the editor's staged preview while it is open, and
    /// nothing, which means the committed row, everywhere else.
    pub(crate) fn staged_brightness(&self) -> Option<u8> {
        (self.page == Page::Brightness).then_some(self.staged)
    }

    /// Whether the guarded power confirmation is up, which is the page that draws the hold bar.
    pub(crate) fn selection_is_guarded(&self) -> bool {
        self.page == Page::PowerConfirm
    }

    /// Whether the rider completed the guarded hold: the host renders this frame and then calls the
    /// power-off port. The state is the page, so a host may poll it every frame.
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
        // fast double-press land on a row the rider cannot see yet.
        if self.motion.sliding(cx.now_ms, MOTION) {
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
                Some(Control::Ble) => {
                    cx.settings.ble_enabled = !cx.settings.ble_enabled;
                    Transition::None
                }
                // Central settings start from the root, and the descent the squeeze came from
                // goes: the sheet reaches this row from inside settings too, so a row that kept
                // that descent would lay one way down on another. Back therefore leaves settings
                // for the view the rider rides on, not for a drawer they have finished with.
                Some(Control::Settings) => Transition::OverRoot(Screen::Settings(SettingsPage::hub())),
                Some(Control::Power) => {
                    self.slide_to(Page::PowerConfirm, cx.now_ms);
                    Transition::None
                }
                // Unreachable: the selection is stepped within `row`. A drawer opened with a light
                // and redrawn without one lands here rather than on a wrong row.
                None => Transition::None,
            },
            Gesture::Back => Transition::Pop,
            // Back-hold is the global escape, resolved above screen dispatch, so it never arrives.
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
            // Cancel: the staged value is abandoned, so the live preview reverts to the committed
            // row on the next frame.
            Gesture::Back => {
                self.slide_to(Page::Root, cx.now_ms);
                Transition::None
            }
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
            // A tap never shuts down: it cancels, exactly like Back.
            Gesture::Press | Gesture::Back => {
                self.slide_to(Page::Root, now_ms);
                Transition::None
            }
            Gesture::Step(_) | Gesture::BackHold => Transition::None,
        }
    }

    /// The sheet's animation: the open slide, then any page slide, at the panel's step cadence.
    pub fn tick_timers(&mut self, now_ms: u32) -> ScreenTick {
        let sheet_h = self.sheet_height(now_ms);
        self.motion.tick(now_ms, MOTION, sheet_h)
    }

    /// Begin a horizontal transition to `to`, which becomes the live page at once (so `handle`
    /// and the render key already speak about the destination) while the slide draws both.
    fn slide_to(&mut self, to: Page, now_ms: u32) {
        self.slide_from = self.page;
        self.page = to;
        self.motion.slide_to(now_ms);
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        let sheet_h = self.sheet_height(rx.now_ms);
        let visible = self.motion.visible_height(rx.now_ms, MOTION, sheet_h);
        if visible == 0 {
            return;
        }
        // The sheet hangs from the top edge: it slides down by drawing its full height with its
        // top off-screen.
        let top = visible - sheet_h;
        sheet::frame(cv, rx.w, top, sheet_h, Edge::Top);
        match self.motion.page_offsets(rx.now_ms, MOTION, rx.w, self.page == Page::Root) {
            Some((out, incoming)) => {
                self.draw_page(cv, rx, self.slide_from, top, out);
                self.draw_page(cv, rx, self.page, top, incoming);
            }
            None => self.draw_page(cv, rx, self.page, top, 0),
        }
    }

    /// The sheet height this frame: the page's own, or the interpolation between two pages' while a
    /// slide runs, which is how the sheet grows and shrinks with its content.
    fn sheet_height(&self, now_ms: u32) -> i32 {
        self.motion.height(now_ms, MOTION, self.slide_from.height(), self.page.height())
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
            // "On" is the amber disc and "off" is the recessive grey one. Only the two stateful
            // controls have an off state; settings and power are always available.
            let on = match control {
                Control::Brightness => true,
                Control::Ble => rx.settings.ble_enabled,
                Control::Settings | Control::Power => false,
            };
            let (fill, ink) =
                if on { (palette::AMBER, palette::ON_ACCENT) } else { (palette::CONTOUR, palette::BAR_TEXT) };
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
        cv.disc(Point::new(knob, y), 3, palette::ON_ACCENT);
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

/// Bluetooth rune.
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

    /// A drawer with its animations already finished, so `handle` acts immediately. The first tick
    /// is the open's origin, so it is taken a whole [`OPEN_MS`] before `now_ms`.
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

    /// The percentages the five levels are captioned at, and the absence of a zero.
    #[test]
    fn the_five_levels_span_twenty_to_a_hundred_percent() {
        let percents: heapless::Vec<u16, 8> = (0..BRIGHTNESS_LEVELS).map(brightness_percent).collect();
        assert_eq!(percents.as_slice(), [20, 40, 60, 80, 100]);
        assert!(percents.iter().all(|p| *p > 0), "no level turns the panel off");
    }

    #[test]
    fn bluetooth_toggles_in_place() {
        let mut w = World::new();
        let mut d = settled(w.now_ms);
        w.press(&mut d, Gesture::Step(1));
        assert!(matches!(w.press(&mut d, Gesture::Press), Transition::None));
        assert!(!w.settings.ble_enabled);
        assert!(matches!(w.press(&mut d, Gesture::Press), Transition::None));
        assert!(w.settings.ble_enabled);
    }

    /// Brightness stages, previews and commits, and Back cancels the commit and takes the live
    /// preview away with it.
    #[test]
    fn brightness_stages_a_live_preview_that_select_commits_and_back_reverts() {
        let mut w = World::new();
        w.settings.brightness = 2;
        let mut d = settled(w.now_ms);

        w.press(&mut d, Gesture::Press); // the editor opens on the committed level
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

    /// The value axis clamps at both ends instead of wrapping, and never reaches an "off" step.
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

    /// Only `Hold` reaches the terminal page the host powers off from.
    #[test]
    fn power_requires_a_completed_hold_and_a_tap_cancels() {
        let mut w = World::new();
        let mut d = settled(w.now_ms);
        w.press(&mut d, Gesture::Step(3)); // the power control
        w.press(&mut d, Gesture::Press);
        assert!(d.selection_is_guarded(), "the confirmation is up and draws its hold bar");
        assert!(!d.powering_off());

        w.press(&mut d, Gesture::Press);
        assert_eq!(d.page, Page::Root, "a tap never shuts down");

        w.press(&mut d, Gesture::Press); // confirm again
        w.press(&mut d, Gesture::Back);
        assert_eq!(d.page, Page::Root, "and Back cancels too");

        w.press(&mut d, Gesture::Press);
        w.press(&mut d, Gesture::Hold);
        assert!(d.powering_off(), "only a completed hold gets there");
    }

    /// The settings icon asks for central settings on the root, so the descent the sheet opened
    /// over goes with the sheet.
    #[test]
    fn the_settings_icon_asks_for_settings_on_the_root() {
        let mut w = World::new();
        let mut d = settled(w.now_ms);
        w.press(&mut d, Gesture::Step(2)); // the settings control
        let t = w.press(&mut d, Gesture::Press);
        assert!(matches!(t, Transition::OverRoot(Screen::Settings(_))));
    }

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
}
