//! Temporary context-drawer prototype for the simulator UX review.
//!
//! This is a real `obc-app` overlay so its geometry, font and colours are constrained by the
//! 240x320 RGB222 device path. It is intentionally reached only through the App's debug hook while
//! the gesture grammar and contents are being evaluated.

use embedded_graphics::prelude::Point;
use obc_reader::{PoiCategory, PoiCategorySet};
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use crate::{input::Gesture, Msg};

use super::{palette, Ctx, Render, Screen, ScreenTick, Transition};

const OPEN_MS: u32 = 240;
const SLIDE_MS: u32 = 180;
const FRAME_MS: u32 = 16;
const HEADER_H: i32 = 48;
const ROW_H: i32 = 44;
const EDITOR_ROW_H: i32 = 36;
const BOTTOM_PAD: i32 = 8;

/// How the still-visible screen behind the temporary drawer is visually recessed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextBackdrop {
    None,
    /// Re-render the base through the device-64 dim LUT before drawing the sheet normally.
    DimLut,
    /// Draw one dark pixel per 2x2 cell over the exposed base, approximating a 25% scrim.
    Stipple,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextDrawerKind {
    Ride,
    UpAhead,
    RoutePlan,
    Weather,
}

impl ContextDrawerKind {
    pub(crate) fn for_screen(screen: &Screen) -> Option<Self> {
        match screen {
            Screen::Map(_) | Screen::Statistics(_) | Screen::Climb(_) | Screen::RideControl(_) => Some(Self::Ride),
            Screen::UpAhead(_) => Some(Self::UpAhead),
            Screen::NavConfirm(_) => Some(Self::RoutePlan),
            Screen::Weather(_) | Screen::WeatherHourly(_) | Screen::WeatherRainMap(_) => Some(Self::Weather),
            _ => None,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Ride => "RIDE ACTIONS",
            Self::UpAhead => "UP AHEAD",
            Self::RoutePlan => "ROUTE OPTIONS",
            Self::Weather => "WEATHER",
        }
    }

    fn rows(self) -> &'static [Row] {
        match self {
            Self::Ride => &RIDE_ROWS,
            Self::UpAhead => &UP_AHEAD_ROWS,
            Self::RoutePlan => &ROUTE_ROWS,
            Self::Weather => &WEATHER_ROWS,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Value {
    None,
    Everything,
    UpAheadSource,
    BikeProfile,
    WeatherRefresh,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Row {
    label: &'static str,
    value: Value,
}

const RIDE_ROWS: [Row; 5] = [
    Row { label: "Up ahead", value: Value::None },
    Row { label: "Detour", value: Value::None },
    Row { label: "POIs", value: Value::None },
    Row { label: "Routes", value: Value::None },
    Row { label: "Map display", value: Value::None },
];
const UP_AHEAD_ROWS: [Row; 2] =
    [Row { label: "Filter", value: Value::Everything }, Row { label: "Sources", value: Value::UpAheadSource }];
const ROUTE_ROWS: [Row; 2] =
    [Row { label: "Bike type", value: Value::BikeProfile }, Row { label: "Route options", value: Value::None }];
const WEATHER_ROWS: [Row; 2] =
    [Row { label: "Refresh now", value: Value::None }, Row { label: "Interval", value: Value::WeatherRefresh }];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Root,
    PoiFilter,
    BikeType,
}

#[derive(Clone, Copy, Debug)]
struct Slide {
    from: Page,
    to: Page,
    started_at_ms: u32,
}

#[derive(Debug)]
pub struct ContextDrawerScreen {
    kind: ContextDrawerKind,
    selected: usize,
    opened_at_ms: u32,
    fullscreen: bool,
    backdrop: ContextBackdrop,
    page: Page,
    slide: Option<Slide>,
    editor_selected: usize,
    filter_row: usize,
    filter_commit: Option<PoiCategorySet>,
}

impl ContextDrawerScreen {
    pub fn new(kind: ContextDrawerKind, opened_at_ms: u32, fullscreen: bool, backdrop: ContextBackdrop) -> Self {
        Self {
            kind,
            selected: 0,
            opened_at_ms,
            fullscreen,
            backdrop,
            page: Page::Root,
            slide: None,
            editor_selected: 0,
            filter_row: 0,
            filter_commit: None,
        }
    }

    pub fn backdrop(&self) -> ContextBackdrop {
        self.backdrop
    }

    pub(crate) fn with_filter(mut self, filter: PoiCategorySet) -> Self {
        self.filter_row = row_of_filter(filter);
        self
    }

    pub(crate) fn take_filter_commit(&mut self) -> Option<PoiCategorySet> {
        self.filter_commit.take()
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        self.finish_slide(cx.now_ms);
        if self.slide.is_some() {
            return Transition::None;
        }

        match self.page {
            Page::Root => match g {
                Gesture::Step(n) => {
                    self.selected = super::vocab::list::step_selection(self.selected, n, self.kind.rows().len());
                    Transition::None
                }
                Gesture::Press => {
                    match (self.kind, self.selected) {
                        (ContextDrawerKind::UpAhead, 0) => {
                            self.editor_selected = self.filter_row;
                            self.start_slide(Page::PoiFilter, cx.now_ms);
                        }
                        (ContextDrawerKind::RoutePlan, 0) => {
                            self.editor_selected = cx.nav_profiles.effective(cx.settings.bike_profile_idx) as usize;
                            self.start_slide(Page::BikeType, cx.now_ms);
                        }
                        _ => {}
                    }
                    Transition::None
                }
                Gesture::Back | Gesture::BackHold => Transition::Pop,
                Gesture::Hold => Transition::None,
            },
            Page::PoiFilter => match g {
                Gesture::Step(n) => {
                    self.editor_selected =
                        super::vocab::list::step_selection(self.editor_selected, n, 1 + PoiCategory::ALL.len());
                    Transition::None
                }
                Gesture::Press => {
                    self.filter_row = self.editor_selected;
                    self.filter_commit = Some(filter_of_row(self.filter_row));
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
            Page::BikeType => match g {
                Gesture::Step(n) => {
                    self.editor_selected =
                        super::vocab::list::step_selection(self.editor_selected, n, cx.nav_profiles.len().max(1));
                    Transition::None
                }
                Gesture::Press => {
                    if !cx.nav_profiles.is_empty() {
                        cx.settings.bike_profile_idx = self.editor_selected as u8;
                    }
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
        }
    }

    pub fn tick_timers(&mut self, now_ms: u32) -> ScreenTick {
        let slide_finished = self.finish_slide(now_ms);
        let open_remaining = OPEN_MS.saturating_sub(now_ms.wrapping_sub(self.opened_at_ms));
        let slide_remaining =
            self.slide.map(|slide| SLIDE_MS.saturating_sub(now_ms.wrapping_sub(slide.started_at_ms))).unwrap_or(0);
        let remaining = [open_remaining, slide_remaining].into_iter().filter(|r| *r > 0).min();
        match remaining {
            Some(remaining) => ScreenTick { changed: true, next_wake_ms: Some(FRAME_MS.min(remaining)), region: None },
            None if slide_finished => ScreenTick { changed: true, next_wake_ms: None, region: None },
            None => ScreenTick::idle(),
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;

        let target_h = self.panel_height(rx);
        let visible_h = self.visible_height(rx.now_ms, target_h);
        let top = rx.h - visible_h;
        if self.backdrop == ContextBackdrop::Stipple {
            draw_stipple(cv, rx.w, top);
        }
        if visible_h == 0 {
            return;
        }

        // The full-height panel moves as one object from below the display. Its part below `h` is
        // naturally clipped by the draw target, so text and rows rise with the sheet instead of
        // fading or popping into fixed positions.
        cv.round(rect(4, top, rx.w - 8, target_h + 8), 10, PARCHMENT);
        cv.round_outline(rect(4, top, rx.w - 8, target_h + 8), 10, WOOD_LIGHT);
        cv.round(rect(rx.w / 2 - 18, top + 7, 36, 4), 2, WOOD_LIGHT);
        cv.hline(12, top + HEADER_H - 1, rx.w - 24, RULE);

        if let Some(slide) = self.slide {
            let progress = self.slide_progress(rx.now_ms, slide);
            if slide.to == Page::Root {
                self.draw_page(cv, rx, top, target_h, slide.from, (progress * rx.w as f32) as i32);
                self.draw_page(cv, rx, top, target_h, slide.to, -((1.0 - progress) * rx.w as f32) as i32);
            } else {
                self.draw_page(cv, rx, top, target_h, slide.from, -(progress * rx.w as f32) as i32);
                self.draw_page(cv, rx, top, target_h, slide.to, ((1.0 - progress) * rx.w as f32) as i32);
            }
        } else {
            self.draw_page(cv, rx, top, target_h, self.page, 0);
        }
    }

    fn start_slide(&mut self, to: Page, now_ms: u32) {
        self.slide = Some(Slide { from: self.page, to, started_at_ms: now_ms });
        self.page = to;
    }

    fn finish_slide(&mut self, now_ms: u32) -> bool {
        let finished = self.slide.is_some_and(|s| now_ms.wrapping_sub(s.started_at_ms) >= SLIDE_MS);
        if finished {
            self.slide = None;
        }
        finished
    }

    fn slide_progress(&self, now_ms: u32, slide: Slide) -> f32 {
        let t = now_ms.wrapping_sub(slide.started_at_ms).min(SLIDE_MS) as f32 / SLIDE_MS as f32;
        t * t * (3.0 - 2.0 * t)
    }

    fn root_height(&self, screen_h: i32) -> i32 {
        if self.fullscreen {
            screen_h - 8
        } else {
            (HEADER_H + self.kind.rows().len() as i32 * ROW_H + BOTTOM_PAD).min(screen_h - 8)
        }
    }

    fn page_height(&self, page: Page, rx: &Render) -> i32 {
        if self.fullscreen {
            return rx.h - 8;
        }
        let rows = match page {
            Page::Root => return self.root_height(rx.h),
            Page::PoiFilter => 1 + PoiCategory::ALL.len(),
            Page::BikeType => rx.nav_profiles.len().max(1),
        };
        (HEADER_H + rows as i32 * EDITOR_ROW_H + BOTTOM_PAD).min(rx.h - 8)
    }

    fn panel_height(&self, rx: &Render) -> i32 {
        let Some(slide) = self.slide else { return self.page_height(self.page, rx) };
        let p = self.slide_progress(rx.now_ms, slide);
        let from = self.page_height(slide.from, rx) as f32;
        let to = self.page_height(slide.to, rx) as f32;
        (from + (to - from) * p + 0.5) as i32
    }

    fn visible_height(&self, now_ms: u32, target_h: i32) -> i32 {
        let elapsed = now_ms.wrapping_sub(self.opened_at_ms).min(OPEN_MS);
        let t = elapsed as f32 / OPEN_MS as f32;
        let eased = 1.0 - (1.0 - t) * (1.0 - t) * (1.0 - t);
        (target_h as f32 * eased + 0.5) as i32
    }

    fn draw_page(&self, cv: &mut impl Surface, rx: &Render, top: i32, panel_h: i32, page: Page, x: i32) {
        match page {
            Page::Root => self.draw_root(cv, rx, top, x),
            Page::PoiFilter => self.draw_filter_editor(cv, rx, top, panel_h, x),
            Page::BikeType => self.draw_bike_editor(cv, rx, top, panel_h, x),
        }
    }

    fn draw_root(&self, cv: &mut impl Surface, rx: &Render, top: i32, x: i32) {
        use palette::*;

        cv.text(self.kind.title(), Point::new(x + 14, top + 18), Font::Label, TextAlign::Left, WOOD);
        for (index, row) in self.kind.rows().iter().enumerate() {
            let y = top + HEADER_H + index as i32 * ROW_H;
            let area = rect(x + 8, y + 3, rx.w - 16, ROW_H - 6);
            let selected = index == self.selected;
            if selected {
                cv.round(area.intersection(&rect(8, y + 3, rx.w - 16, ROW_H - 6)), 6, AMBER);
            }
            self.draw_root_row(cv, rx, row, y, x);
        }
    }

    fn draw_root_row(&self, cv: &mut impl Surface, rx: &Render, row: &Row, y: i32, x: i32) {
        use palette::*;

        // No leading bullet: the right chevron already says "opens more", and one arrow per row
        // keeps the list quiet.
        cv.text_vcentered(row.label, x + 22, (y, ROW_H), Font::Label, TextAlign::Left, INK);

        let value = match row.value {
            Value::None => None,
            Value::Everything => Some(filter_short_name(self.filter_row)),
            Value::UpAheadSource => Some(rx.settings.up_ahead_source.name(rx.settings.language)),
            Value::WeatherRefresh => Some(rx.settings.weather_refresh.name(rx.settings.language)),
            Value::BikeProfile => None,
        };
        if let Some(value) = value {
            cv.text_vcentered(value, x + rx.w - 29, (y, ROW_H), Font::Label, TextAlign::Right, WOOD);
        } else if row.value == Value::BikeProfile {
            let profile = short_profile(rx, rx.settings.bike_profile_idx);
            cv.text_vcentered(&profile, x + rx.w - 29, (y, ROW_H), Font::Label, TextAlign::Right, WOOD);
        }
        draw_chevron(cv, x + rx.w - 17, y + ROW_H / 2, WOOD);
    }

    fn draw_filter_editor(&self, cv: &mut impl Surface, rx: &Render, top: i32, panel_h: i32, x: i32) {
        use palette::*;

        cv.text(rx.t(Msg::UpAheadFilterTitle), Point::new(x + 14, top + 18), Font::Label, TextAlign::Left, WOOD);
        let total = 1 + PoiCategory::ALL.len();
        self.draw_editor_rows(cv, rect(x, top, rx.w, panel_h), total, |cv, index, area, selected| {
            let mid = area.top_left.y + area.size.height as i32 / 2;
            let ink = if selected { INK } else { SUBTEXT };
            let bg = if selected { AMBER } else { PARCHMENT };
            let label = match index {
                0 => {
                    cv.triangle(
                        Point::new(area.top_left.x + 12, mid - 7),
                        Point::new(area.top_left.x + 12, mid + 7),
                        Point::new(area.top_left.x + 21, mid),
                        ink,
                    );
                    rx.t(Msg::UpAheadEverything)
                }
                row => {
                    let category = PoiCategory::ALL[row - 1];
                    super::poi_menu::draw_category_icon(cv, category, Point::new(area.top_left.x + 17, mid), ink, bg);
                    rx.t(super::poi_menu::category_msg(category))
                }
            };
            cv.text_vcentered(
                label,
                area.top_left.x + 35,
                (area.top_left.y, area.size.height as i32),
                Font::Label,
                TextAlign::Left,
                INK,
            );
            if index == self.filter_row {
                draw_check(cv, area.top_left.x + area.size.width as i32 - 17, mid, INK);
            }
        });
    }

    fn draw_bike_editor(&self, cv: &mut impl Surface, rx: &Render, top: i32, panel_h: i32, x: i32) {
        use palette::*;

        cv.text("BIKE TYPE", Point::new(x + 14, top + 18), Font::Label, TextAlign::Left, WOOD);
        let total = rx.nav_profiles.len().max(1);
        let committed = rx.nav_profiles.effective(rx.settings.bike_profile_idx) as usize;
        self.draw_editor_rows(cv, rect(x, top, rx.w, panel_h), total, |cv, index, area, _selected| {
            let mut profile: heapless::String<24> = heapless::String::new();
            rx.nav_profiles.write_label(index as u8, &mut profile);
            cv.text_vcentered(
                &profile,
                area.top_left.x + 18,
                (area.top_left.y, area.size.height as i32),
                Font::Label,
                TextAlign::Left,
                INK,
            );
            if index == committed {
                draw_check(
                    cv,
                    area.top_left.x + area.size.width as i32 - 17,
                    area.top_left.y + area.size.height as i32 / 2,
                    INK,
                );
            }
        });
    }

    fn draw_editor_rows<S: Surface>(
        &self,
        cv: &mut S,
        panel: embedded_graphics::primitives::Rectangle,
        total: usize,
        mut draw: impl FnMut(&mut S, usize, embedded_graphics::primitives::Rectangle, bool),
    ) {
        use palette::*;

        let top = panel.top_left.y;
        let panel_h = panel.size.height as i32;
        let x = panel.top_left.x;
        let width = panel.size.width as i32;
        let visible = ((panel_h - HEADER_H - BOTTOM_PAD) / EDITOR_ROW_H).max(1) as usize;
        let first = super::vocab::list::window_start(self.editor_selected, visible, total);
        for slot in 0..visible.min(total) {
            let index = first + slot;
            if index >= total {
                break;
            }
            let y = top + HEADER_H + slot as i32 * EDITOR_ROW_H;
            let area = rect(x + 8, y + 2, width - 16, EDITOR_ROW_H - 4);
            let selected = index == self.editor_selected;
            if selected {
                cv.round(area.intersection(&rect(8, y + 2, width - 16, EDITOR_ROW_H - 4)), 6, AMBER);
            }
            draw(cv, index, area, selected);
        }
    }
}

fn filter_of_row(row: usize) -> PoiCategorySet {
    match row {
        0 => PoiCategorySet::ALL,
        row => PoiCategorySet::only(PoiCategory::ALL[(row - 1).min(PoiCategory::ALL.len() - 1)]),
    }
}

fn row_of_filter(filter: PoiCategorySet) -> usize {
    PoiCategory::ALL.iter().position(|category| filter == PoiCategorySet::only(*category)).map_or(0, |index| index + 1)
}

fn filter_short_name(row: usize) -> &'static str {
    match row {
        0 => "All",
        1 => "Water",
        2 => "Camp",
        3 => "Lodging",
        4 => "Resupply",
        5 => "Pharmacy",
        _ => "Bikes",
    }
}

fn short_profile(rx: &Render, index: u8) -> heapless::String<8> {
    let mut full: heapless::String<24> = heapless::String::new();
    rx.nav_profiles.write_label(index, &mut full);
    let mut short = heapless::String::new();
    for ch in full.chars().take(7) {
        let _ = short.push(ch);
    }
    short
}

fn draw_chevron(cv: &mut impl Surface, x: i32, cy: i32, color: u16) {
    cv.line(Point::new(x - 3, cy - 5), Point::new(x + 2, cy), color);
    cv.line(Point::new(x + 2, cy), Point::new(x - 3, cy + 5), color);
}

fn draw_check(cv: &mut impl Surface, x: i32, cy: i32, color: u16) {
    cv.line(Point::new(x - 6, cy), Point::new(x - 2, cy + 4), color);
    cv.line(Point::new(x - 2, cy + 4), Point::new(x + 6, cy - 5), color);
}

/// A manually-tunable device-64 dim curve. Each RGB222 channel steps down by one level; feeding
/// all three through this four-entry curve is the complete 64 → 64 LUT without storing redundant
/// channel combinations. The output remains exactly on the device gamut.
const DIM_LEVEL: [u8; 4] = [0, 1, 1, 2];

pub(crate) fn dim_color(rgb565: u16) -> u16 {
    let r = DIM_LEVEL[((rgb565 >> 14) & 0x3) as usize];
    let g = DIM_LEVEL[((rgb565 >> 9) & 0x3) as usize];
    let b = DIM_LEVEL[((rgb565 >> 3) & 0x3) as usize];
    palette::rgb565(r * 85, g * 85, b * 85)
}

fn draw_stipple(cv: &mut impl Surface, w: i32, h: i32) {
    for y in (0..h).step_by(2) {
        for x in ((y & 2) / 2..w).step_by(2) {
            cv.fill(rect(x, y, 1, 1), palette::HUD);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{activity::Mode, screen::test_ctx, Activity, AppState, Settings};

    #[test]
    fn adaptive_height_exposes_the_content_size() {
        let ride = ContextDrawerScreen::new(ContextDrawerKind::Ride, 0, false, ContextBackdrop::None);
        let compact = ContextDrawerScreen::new(ContextDrawerKind::Weather, 0, false, ContextBackdrop::None);
        assert_eq!(ride.root_height(320), 276);
        assert_eq!(compact.root_height(320), 144);
        assert_eq!(
            ContextDrawerScreen::new(ContextDrawerKind::Weather, 0, true, ContextBackdrop::None).root_height(320),
            312
        );
    }

    #[test]
    fn opening_is_monotonic_and_lands_exactly() {
        let drawer = ContextDrawerScreen::new(ContextDrawerKind::Ride, 1_000, false, ContextBackdrop::None);
        let target = drawer.root_height(320);
        let frames = [0, 60, 120, 180, OPEN_MS].map(|dt| drawer.visible_height(1_000 + dt, target));
        assert_eq!(frames[0], 0);
        assert_eq!(frames[4], target);
        assert!(frames.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn category_filters_round_trip_through_drawer_rows() {
        assert_eq!(row_of_filter(PoiCategorySet::ALL), 0);
        for row in 1..=PoiCategory::ALL.len() {
            assert_eq!(row_of_filter(filter_of_row(row)), row);
        }
    }

    #[test]
    fn filter_browse_is_staged_and_back_cancels_it() {
        let mut state = AppState::new(0, 0, 1.0);
        let mut activity = Activity::new(Mode::Idle);
        let mut settings = Settings::default();
        let mut cx = test_ctx(&mut state, &mut activity, &mut settings);
        let mut drawer = ContextDrawerScreen::new(ContextDrawerKind::UpAhead, 0, false, ContextBackdrop::DimLut);

        drawer.handle(Gesture::Press, &mut cx);
        cx.now_ms = SLIDE_MS;
        drawer.handle(Gesture::Step(1), &mut cx);
        assert_eq!(drawer.filter_row, 0);
        assert_eq!(drawer.take_filter_commit(), None);
        drawer.handle(Gesture::Back, &mut cx);
        assert_eq!(drawer.filter_row, 0);
        assert_eq!(drawer.take_filter_commit(), None);
    }

    #[test]
    fn filter_press_commits_the_browsed_value() {
        let mut state = AppState::new(0, 0, 1.0);
        let mut activity = Activity::new(Mode::Idle);
        let mut settings = Settings::default();
        let mut cx = test_ctx(&mut state, &mut activity, &mut settings);
        let mut drawer = ContextDrawerScreen::new(ContextDrawerKind::UpAhead, 0, false, ContextBackdrop::DimLut);

        drawer.handle(Gesture::Press, &mut cx);
        cx.now_ms = SLIDE_MS;
        drawer.handle(Gesture::Step(1), &mut cx);
        drawer.handle(Gesture::Press, &mut cx);
        assert_eq!(drawer.take_filter_commit(), Some(PoiCategorySet::only(PoiCategory::Water)));
    }

    #[test]
    fn dim_lut_stays_on_gamut_and_never_brightens_a_channel() {
        for r in 0..4 {
            for g in 0..4 {
                for b in 0..4 {
                    let normal = palette::rgb565(r * 85, g * 85, b * 85);
                    let dim = dim_color(normal);
                    assert!(((dim >> 14) & 0x3) <= r as u16);
                    assert!(((dim >> 9) & 0x3) <= g as u16);
                    assert!(((dim >> 3) & 0x3) <= b as u16);
                }
            }
        }
        assert_eq!(dim_color(palette::PARCHMENT), palette::rgb565(170, 170, 170));
        assert_eq!(dim_color(palette::HUD), palette::rgb565(0, 0, 0));
    }
}
