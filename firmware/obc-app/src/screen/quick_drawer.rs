//! Temporary top quick-drawer prototype: four device-wide icon controls opened by the upper
//! button pair in the simulator. BLE uses the real persisted setting; brightness and shutdown
//! remain prototype-local until their platform ports exist.

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;

use super::vocab::chrome::stroke2;
use super::{palette, ContextBackdrop, Ctx, Render, Screen, ScreenTick, SettingsScreen, Transition};

const OPEN_MS: u32 = 220;
const SLIDE_MS: u32 = 180;
const FRAME_MS: u32 = 16;
const ROOT_H: i32 = 108;
const BRIGHTNESS_H: i32 = 136;
const POWER_H: i32 = 174;
const POWERING_OFF_H: i32 = 132;
const ITEM_COUNT: usize = 4;
const LIGHT: usize = 0;
const BLE: usize = 1;
const SETTINGS: usize = 2;
const POWER: usize = 3;
const BRIGHTNESS_LEVELS: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Root,
    Brightness,
    PowerConfirm,
    PoweringOff,
}

#[derive(Clone, Copy, Debug)]
struct Slide {
    from: Page,
    to: Page,
    started_at_ms: u32,
}

#[derive(Debug)]
pub struct QuickDrawerScreen {
    selected: usize,
    opened_at_ms: u32,
    backdrop: ContextBackdrop,
    page: Page,
    slide: Option<Slide>,
    brightness: u8,
    brightness_cursor: usize,
    brightness_commit: Option<u8>,
}

impl QuickDrawerScreen {
    pub fn new(opened_at_ms: u32, backdrop: ContextBackdrop, brightness: u8) -> Self {
        let brightness = brightness.min((BRIGHTNESS_LEVELS - 1) as u8);
        Self {
            selected: LIGHT,
            opened_at_ms,
            backdrop,
            page: Page::Root,
            slide: None,
            brightness,
            brightness_cursor: brightness as usize,
            brightness_commit: None,
        }
    }

    pub fn backdrop(&self) -> ContextBackdrop {
        self.backdrop
    }

    pub(crate) fn take_brightness_commit(&mut self) -> Option<u8> {
        self.brightness_commit.take()
    }

    pub(crate) fn selection_is_guarded(&self) -> bool {
        self.page == Page::PowerConfirm
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        self.finish_slide(cx.now_ms);
        if self.slide.is_some() {
            return Transition::None;
        }

        match self.page {
            Page::Root => match g {
                Gesture::Step(n) => {
                    self.selected = super::vocab::list::step_selection(self.selected, n, ITEM_COUNT);
                    Transition::None
                }
                Gesture::Press => match self.selected {
                    LIGHT => {
                        self.brightness_cursor = self.brightness as usize;
                        self.start_slide(Page::Brightness, cx.now_ms);
                        Transition::None
                    }
                    BLE => {
                        cx.settings.ble_enabled = !cx.settings.ble_enabled;
                        Transition::None
                    }
                    SETTINGS => Transition::Replace(Screen::Settings(SettingsScreen::new())),
                    POWER => {
                        self.start_slide(Page::PowerConfirm, cx.now_ms);
                        Transition::None
                    }
                    _ => Transition::None,
                },
                Gesture::Back | Gesture::BackHold => Transition::Pop,
                Gesture::Hold => Transition::None,
            },
            Page::Brightness => match g {
                Gesture::Step(n) => {
                    self.brightness_cursor =
                        super::vocab::list::step_selection(self.brightness_cursor, n, BRIGHTNESS_LEVELS);
                    Transition::None
                }
                Gesture::Press => {
                    self.brightness = self.brightness_cursor as u8;
                    self.brightness_commit = Some(self.brightness);
                    self.start_slide(Page::Root, cx.now_ms);
                    Transition::None
                }
                Gesture::Back => {
                    self.start_slide(Page::Root, cx.now_ms);
                    Transition::None
                }
                Gesture::BackHold => Transition::Pop,
                Gesture::Hold => Transition::None,
            },
            Page::PowerConfirm => match g {
                Gesture::Hold => {
                    self.start_slide(Page::PoweringOff, cx.now_ms);
                    Transition::None
                }
                Gesture::Back | Gesture::Press => {
                    self.start_slide(Page::Root, cx.now_ms);
                    Transition::None
                }
                Gesture::Step(_) | Gesture::BackHold => Transition::None,
            },
            Page::PoweringOff => match g {
                // Prototype escape hatch: real hardware would never return from its power-off port.
                Gesture::Back | Gesture::BackHold => {
                    self.start_slide(Page::Root, cx.now_ms);
                    Transition::None
                }
                Gesture::Step(_) | Gesture::Press | Gesture::Hold => Transition::None,
            },
        }
    }

    pub fn tick_timers(&mut self, now_ms: u32) -> ScreenTick {
        let slide_finished = self.finish_slide(now_ms);
        let open_remaining = OPEN_MS.saturating_sub(now_ms.wrapping_sub(self.opened_at_ms));
        let slide_remaining =
            self.slide.map(|slide| SLIDE_MS.saturating_sub(now_ms.wrapping_sub(slide.started_at_ms))).unwrap_or(0);
        let remaining = [open_remaining, slide_remaining].into_iter().filter(|remaining| *remaining > 0).min();
        match remaining {
            Some(remaining) => ScreenTick { changed: true, next_wake_ms: Some(FRAME_MS.min(remaining)), region: None },
            None if slide_finished => ScreenTick { changed: true, next_wake_ms: None, region: None },
            None => ScreenTick::idle(),
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;

        let panel_h = self.panel_height(rx.now_ms);
        let visible_h = self.visible_height(rx.now_ms, panel_h);
        let top = visible_h - panel_h;
        if self.backdrop == ContextBackdrop::Stipple {
            draw_stipple(cv, rx.w, visible_h, rx.h);
        }
        if visible_h == 0 {
            return;
        }

        cv.round(rect(4, top - 8, rx.w - 8, panel_h + 8), 10, PARCHMENT);
        cv.round_outline(rect(4, top - 8, rx.w - 8, panel_h + 8), 10, WOOD_LIGHT);
        cv.round(rect(rx.w / 2 - 18, top + panel_h - 11, 36, 4), 2, WOOD_LIGHT);

        if let Some(slide) = self.slide {
            let progress = slide_progress(rx.now_ms, slide);
            if slide.to == Page::Root {
                self.draw_page(cv, rx, top, panel_h, slide.from, (progress * rx.w as f32) as i32);
                self.draw_page(cv, rx, top, panel_h, slide.to, -((1.0 - progress) * rx.w as f32) as i32);
            } else {
                self.draw_page(cv, rx, top, panel_h, slide.from, -(progress * rx.w as f32) as i32);
                self.draw_page(cv, rx, top, panel_h, slide.to, ((1.0 - progress) * rx.w as f32) as i32);
            }
        } else {
            self.draw_page(cv, rx, top, panel_h, self.page, 0);
        }
    }

    fn start_slide(&mut self, to: Page, now_ms: u32) {
        self.slide = Some(Slide { from: self.page, to, started_at_ms: now_ms });
        self.page = to;
    }

    fn finish_slide(&mut self, now_ms: u32) -> bool {
        let finished = self.slide.is_some_and(|slide| now_ms.wrapping_sub(slide.started_at_ms) >= SLIDE_MS);
        if finished {
            self.slide = None;
        }
        finished
    }

    fn page_height(page: Page) -> i32 {
        match page {
            Page::Root => ROOT_H,
            Page::Brightness => BRIGHTNESS_H,
            Page::PowerConfirm => POWER_H,
            Page::PoweringOff => POWERING_OFF_H,
        }
    }

    fn panel_height(&self, now_ms: u32) -> i32 {
        let Some(slide) = self.slide else { return Self::page_height(self.page) };
        let progress = slide_progress(now_ms, slide);
        let from = Self::page_height(slide.from) as f32;
        let to = Self::page_height(slide.to) as f32;
        (from + (to - from) * progress + 0.5) as i32
    }

    fn visible_height(&self, now_ms: u32, panel_h: i32) -> i32 {
        let elapsed = now_ms.wrapping_sub(self.opened_at_ms).min(OPEN_MS);
        let t = elapsed as f32 / OPEN_MS as f32;
        let eased = 1.0 - (1.0 - t) * (1.0 - t) * (1.0 - t);
        (panel_h as f32 * eased + 0.5) as i32
    }

    fn draw_page(&self, cv: &mut impl Surface, rx: &Render, top: i32, panel_h: i32, page: Page, x: i32) {
        match page {
            Page::Root => self.draw_root(cv, rx, top, x),
            Page::Brightness => self.draw_brightness(cv, rx, top, x),
            Page::PowerConfirm => self.draw_power_confirm(cv, rx, top, x),
            Page::PoweringOff => self.draw_powering_off(cv, top, panel_h, x, rx.w),
        }
    }

    fn draw_root(&self, cv: &mut impl Surface, rx: &Render, top: i32, x: i32) {
        use palette::*;

        const CELL: i32 = 46;
        const GAP: i32 = 10;
        let first_x = (rx.w - (CELL * ITEM_COUNT as i32 + GAP * (ITEM_COUNT as i32 - 1))) / 2;
        for index in 0..ITEM_COUNT {
            let left = x + first_x + index as i32 * (CELL + GAP);
            let area = rect(left, top + 10, CELL, CELL);
            let selected = index == self.selected;
            if selected {
                cv.round(area.intersection(&rect(8, top + 10, rx.w - 16, CELL)), 10, AMBER);
            }
            // One ink for the whole row — state lives in the badge and the caption, never in a
            // per-icon hue (the dial's single-ink glyph language).
            let ink = if selected { INK } else { WOOD };
            let bg = if selected { AMBER } else { PARCHMENT };
            let center = Point::new(left + CELL / 2, top + 10 + CELL / 2);
            match index {
                LIGHT => draw_sun(cv, center, ink),
                BLE => {
                    draw_ble_rune(cv, center, ink);
                    draw_state_badge(cv, Point::new(center.x + 13, center.y + 12), rx.settings.ble_enabled, bg);
                }
                SETTINGS => super::menu::icon_sliders(cv, center, 0.9, ink),
                POWER => draw_power(cv, center, ink, bg),
                _ => {}
            }
        }

        // The selected item's name — the row stays icon-only while every station is still
        // discoverable by browsing (same voice as the context drawer's WOOD header).
        let caption = match self.selected {
            LIGHT => "BRIGHTNESS",
            BLE => {
                if rx.settings.ble_enabled {
                    "BLUETOOTH ON"
                } else {
                    "BLUETOOTH OFF"
                }
            }
            SETTINGS => "SETTINGS",
            _ => "POWER OFF",
        };
        cv.text(caption, Point::new(x + rx.w / 2, top + 68), Font::Label, TextAlign::Center, WOOD);
    }

    fn draw_brightness(&self, cv: &mut impl Surface, rx: &Render, top: i32, x: i32) {
        use palette::*;

        cv.text("BRIGHTNESS", Point::new(x + 14, top + 18), Font::Label, TextAlign::Left, WOOD);
        cv.hline(x + 12, top + 47, rx.w - 24, RULE);
        draw_sun(cv, Point::new(x + 26, top + 82), WOOD);

        let x0 = x + 52;
        let x1 = x + rx.w - 24;
        let y = top + 82;
        cv.round(rect(x0, y - 2, x1 - x0, 5), 2, PARCHMENT_SHADE);
        for index in 0..BRIGHTNESS_LEVELS {
            let px = slider_x(x0, x1, index);
            cv.vline(px, y - 6, 13, 1, SUBTEXT);
            if index == self.brightness as usize {
                draw_check(cv, px, y + 22, WOOD);
            }
        }
        let knob_x = slider_x(x0, x1, self.brightness_cursor);
        cv.disc(Point::new(knob_x, y), 8, AMBER);
        cv.disc(Point::new(knob_x, y), 3, INK);
    }

    fn draw_power_confirm(&self, cv: &mut impl Surface, rx: &Render, top: i32, x: i32) {
        use palette::*;

        cv.text("POWER OFF?", Point::new(x + 14, top + 18), Font::Label, TextAlign::Left, WARNING);
        cv.hline(x + 12, top + 47, rx.w - 24, RULE);
        draw_power(cv, Point::new(x + rx.w / 2, top + 77), WARNING, PARCHMENT);
        cv.text("HOLD SELECT", Point::new(x + rx.w / 2, top + 105), Font::Label, TextAlign::Center, INK);

        let button = rect(x + 20, top + 125, rx.w - 40, 32);
        cv.round(button, 6, PARCHMENT_SHADE);
        let fill_w = ((button.size.width as f32 * rx.hold_progress.clamp(0.0, 1.0)) + 0.5) as i32;
        if fill_w > 0 {
            cv.round(rect(button.top_left.x, button.top_left.y, fill_w, button.size.height as i32), 6, WARNING);
        }
        cv.round_outline(button, 6, WARNING);
    }

    fn draw_powering_off(&self, cv: &mut impl Surface, top: i32, _panel_h: i32, x: i32, width: i32) {
        use palette::*;

        draw_power(cv, Point::new(x + width / 2, top + 43), WARNING, PARCHMENT);
        cv.text("POWERING OFF...", Point::new(x + width / 2, top + 75), Font::Body, TextAlign::Center, INK);
    }
}

fn slide_progress(now_ms: u32, slide: Slide) -> f32 {
    let t = now_ms.wrapping_sub(slide.started_at_ms).min(SLIDE_MS) as f32 / SLIDE_MS as f32;
    t * t * (3.0 - 2.0 * t)
}

fn slider_x(x0: i32, x1: i32, index: usize) -> i32 {
    x0 + (x1 - x0) * index as i32 / (BRIGHTNESS_LEVELS as i32 - 1)
}

/// Brightness sun: a filled core with four cardinal bars and four diagonal ray dots — the same
/// filled-geometry sun the Weather station glyph established.
fn draw_sun(cv: &mut impl Surface, center: Point, color: u16) {
    cv.disc(center, 6, color);
    cv.vline(center.x - 1, center.y - 13, 5, 2, color);
    cv.vline(center.x - 1, center.y + 9, 5, 2, color);
    cv.fill(rect(center.x - 13, center.y - 1, 5, 2), color);
    cv.fill(rect(center.x + 9, center.y - 1, 5, 2), color);
    for (dx, dy) in [(-9, -9), (9, -9), (9, 9), (-9, 9)] {
        cv.disc(Point::new(center.x + dx, center.y + dy), 2, color);
    }
}

/// The Bluetooth bind-rune at drawer scale: the title-bar rune's geometry, doubled to the panel's
/// 2 px stroke idiom so it carries the same visual mass as the filled glyphs beside it.
fn draw_ble_rune(cv: &mut impl Surface, center: Point, color: u16) {
    let half = 11;
    let quarter = 5;
    let stem_x = center.x - 2;
    let (top, mid, bot) =
        (Point::new(stem_x, center.y - half), Point::new(stem_x, center.y), Point::new(stem_x, center.y + half));
    let up_tip = Point::new(center.x + 6, center.y - half + quarter);
    let lo_tip = Point::new(center.x + 6, center.y + half - quarter);
    let up_left = Point::new(center.x - 8, center.y - half + quarter);
    let lo_left = Point::new(center.x - 8, center.y + half - quarter);
    stroke2(cv, top, bot, color);
    stroke2(cv, top, up_tip, color);
    stroke2(cv, up_tip, mid, color);
    stroke2(cv, bot, lo_tip, color);
    stroke2(cv, lo_tip, mid, color);
    stroke2(cv, up_tip, lo_left, color);
    stroke2(cv, lo_tip, up_left, color);
}

/// On/off badge in the cell corner: a filled green dot for on, a hollow grey ring for off — the
/// settings toggle-pill colour vocabulary, anchored to the icon box instead of dangling.
fn draw_state_badge(cv: &mut impl Surface, at: Point, on: bool, bg: u16) {
    use palette::*;
    cv.disc(at, 6, bg);
    if on {
        cv.disc(at, 4, ON);
    } else {
        cv.disc(at, 4, CONTOUR);
        cv.disc(at, 2, bg);
    }
}

/// Power symbol as filled geometry: a 3 px ring with its top arc broken, and a heavy stem through
/// the gap.
fn draw_power(cv: &mut impl Surface, center: Point, color: u16, bg: u16) {
    cv.disc(center, 10, color);
    cv.disc(center, 7, bg);
    cv.fill(rect(center.x - 3, center.y - 12, 7, 8), bg);
    cv.vline(center.x - 1, center.y - 14, 11, 3, color);
}

fn draw_check(cv: &mut impl Surface, x: i32, cy: i32, color: u16) {
    cv.line(Point::new(x - 5, cy), Point::new(x - 1, cy + 4), color);
    cv.line(Point::new(x - 1, cy + 4), Point::new(x + 6, cy - 4), color);
}

fn draw_stipple(cv: &mut impl Surface, w: i32, y0: i32, h: i32) {
    for y in (y0..h).step_by(2) {
        for x in ((y & 2) / 2..w).step_by(2) {
            cv.fill(rect(x, y, 1, 1), palette::HUD);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{activity::Mode, screen::test_ctx, Activity, AppState, Settings};

    fn context<'a>(
        state: &'a mut AppState,
        activity: &'a mut Activity,
        settings: &'a mut Settings,
        now_ms: u32,
    ) -> Ctx<'a> {
        Ctx { now_ms, ..test_ctx(state, activity, settings) }
    }

    #[test]
    fn ble_icon_toggles_the_real_setting() {
        let mut state = AppState::new(0, 0, 1.0);
        let mut activity = Activity::new(Mode::Idle);
        let mut settings = Settings::default();
        let mut drawer = QuickDrawerScreen::new(0, ContextBackdrop::DimLut, 3);
        let mut cx = context(&mut state, &mut activity, &mut settings, 0);

        drawer.handle(Gesture::Step(1), &mut cx);
        drawer.handle(Gesture::Press, &mut cx);
        assert!(!cx.settings.ble_enabled);
        drawer.handle(Gesture::Press, &mut cx);
        assert!(cx.settings.ble_enabled);
    }

    #[test]
    fn brightness_is_staged_until_select_and_back_cancels() {
        let mut state = AppState::new(0, 0, 1.0);
        let mut activity = Activity::new(Mode::Idle);
        let mut settings = Settings::default();
        let mut drawer = QuickDrawerScreen::new(0, ContextBackdrop::DimLut, 2);
        let mut cx = context(&mut state, &mut activity, &mut settings, 0);

        drawer.handle(Gesture::Press, &mut cx);
        cx.now_ms = SLIDE_MS;
        drawer.handle(Gesture::Step(1), &mut cx);
        assert_eq!(drawer.take_brightness_commit(), None);
        drawer.handle(Gesture::Back, &mut cx);
        assert_eq!(drawer.brightness, 2);

        cx.now_ms += SLIDE_MS;
        drawer.handle(Gesture::Press, &mut cx);
        cx.now_ms += SLIDE_MS;
        drawer.handle(Gesture::Step(1), &mut cx);
        drawer.handle(Gesture::Press, &mut cx);
        assert_eq!(drawer.take_brightness_commit(), Some(3));
    }

    #[test]
    fn power_confirmation_requires_a_hold() {
        let mut state = AppState::new(0, 0, 1.0);
        let mut activity = Activity::new(Mode::Idle);
        let mut settings = Settings::default();
        let mut drawer = QuickDrawerScreen::new(0, ContextBackdrop::DimLut, 3);
        let mut cx = context(&mut state, &mut activity, &mut settings, 0);

        drawer.handle(Gesture::Step(3), &mut cx);
        drawer.handle(Gesture::Press, &mut cx);
        cx.now_ms = SLIDE_MS;
        drawer.handle(Gesture::Press, &mut cx);
        assert_eq!(drawer.page, Page::Root, "a tap cancels the confirmation");

        cx.now_ms += SLIDE_MS;
        drawer.handle(Gesture::Press, &mut cx);
        cx.now_ms += SLIDE_MS;
        drawer.handle(Gesture::Hold, &mut cx);
        assert_eq!(drawer.page, Page::PoweringOff);
    }
}
