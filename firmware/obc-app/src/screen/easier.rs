//! Map-first comparison of a frozen, measured candidate. App owns the sequential planner work.
use super::{palette::*, Ctx, RenderFrame, Transition};
use crate::{
    easier::{Phase, State},
    navigator::ReviewStatus,
    Gesture,
};
use core::fmt::Write;
use embedded_graphics::prelude::*;
use obc_map_scene::{BBox, MapScene};
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Canvas, Surface, Viewport,
};
use obc_route::easier::{Costs, Goal};
const NAMES: [&str; 3] = ["Less climbing", "Smoother surface", "Shorter ride"];

#[derive(Clone, Copy, PartialEq)]
pub struct EasierScreen {
    current: Costs,
    next: Option<Costs>,
    bounds: BBox,
    progress_m: u32,
    goal: u8,
    ordinal: u8,
    count: u8,
    ready: bool,
    review: bool,
    saving: bool,
    unavailable: bool,
    failure: Option<obc_route::NavError>,
}
impl EasierScreen {
    pub(crate) fn new() -> Self {
        let state = State::new();
        Self {
            current: state.current,
            next: None,
            bounds: state.bounds,
            progress_m: 0,
            goal: 0,
            ordinal: 0,
            count: 0,
            ready: false,
            review: false,
            saving: false,
            unavailable: false,
            failure: None,
        }
    }
    pub(crate) fn update(&mut self, state: &State, status: ReviewStatus) -> bool {
        let before = *self;
        self.current = state.current;
        self.next = state.choices[state.selected as usize].map(|r| r.choice.costs);
        self.bounds = state.bounds;
        self.progress_m = state.context.map_or(0, |c| c.progress_m);
        self.goal = state.selected;
        self.review = state.review;
        self.count = state.choices.iter().filter(|r| r.is_some()).count() as u8;
        self.ordinal = state.choices[..=state.selected as usize].iter().filter(|r| r.is_some()).count() as u8;
        self.ready = state.phase == Phase::Ready;
        self.saving = status == ReviewStatus::Saving;
        self.unavailable = state.phase == Phase::Unavailable;
        self.failure = state.failure;
        *self != before
    }
    pub fn handle(&mut self, _: Gesture, _: &mut Ctx) -> Transition {
        Transition::None
    }
    pub fn draw<D, F, S>(&self, cv: &mut Canvas<D, F>, rx: &mut RenderFrame<'_, S>)
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
        S: MapScene,
    {
        if let Some(next) = self.next.filter(|_| self.review) {
            cv.clear(PARCHMENT);
            header(cv, NAMES[self.goal as usize], rx.w, None);
            benefit(cv, self.current, next, self.goal as usize, rect(8, 46, rx.w - 16, 70), false);
            cv.text("Current", Point::new(150, 129), Font::Label, TextAlign::Right, SUBTEXT);
            cv.text("New", Point::new(226, 129), Font::Label, TextAlign::Right, SUBTEXT);
            cv.hline(12, 157, rx.w - 24, SUBTEXT);
            for (i, (name, old, new)) in [
                ("Ride", self.current.distance_m, next.distance_m),
                ("Climb", self.current.ascent_m, next.ascent_m),
                ("Rough", self.current.rough_m, next.rough_m),
            ]
            .iter()
            .enumerate()
            {
                let y = 163 + i as i32 * 36;
                cv.text(name, Point::new(12, y), Font::Body, TextAlign::Left, INK);
                let known = i != 1 || self.current.elevation_complete;
                let surface = i != 2 || (self.current.surface_attributed && self.current.unknown_m == 0);
                let a = distance(*old, i == 1);
                let b = distance(*new, i == 1);
                cv.text(
                    if known && surface { &a } else { "Unknown" },
                    Point::new(150, y),
                    Font::Body,
                    TextAlign::Right,
                    INK,
                );
                cv.text(
                    if i != 2 || next.unknown_m == 0 { &b } else { "Unknown" },
                    Point::new(226, y),
                    Font::Body,
                    TextAlign::Right,
                    INK,
                );
                if i < 2 {
                    cv.hline(12, y + 30, rx.w - 24, SUBTEXT);
                }
            }
            button(cv, if self.saving { "Saving…" } else { "Use this route" });
            return;
        }
        let vp = fit(self.bounds, rx.w, rx.h);
        let active = rx.route.take();
        super::map::draw_map_scene(cv, rx, &vp, None);
        rx.route = active;
        let progress = self.progress_m;
        let proposed = rx.nav_preview;
        if let Some(scratch) = rx.scratch.as_deref_mut() {
            let (target, color) = cv.split();
            if let Some(route) = active {
                route.visit_points_between(progress, route.total_distance_m, |points| {
                    scratch.stroke_path(target, &vp, points.iter().copied(), color(ROUTE), 4);
                });
            }
            if self.ready {
                scratch.stroke_path(target, &vp, proposed.iter().copied(), color(DETOUR), 4);
            }
        }
        if self.ready {
            for (p, finish) in [(rx.nav_preview.first(), false), (rx.nav_preview.last(), true)] {
                if let Some(&(lon, lat)) = p {
                    let (x, y) = vp.to_screen(lon, lat);
                    if finish {
                        cv.round(rect(x - 5, y - 5, 11, 11), 2, INK);
                        cv.fill(rect(x - 2, y - 2, 5, 5), PARCHMENT);
                    } else {
                        cv.disc(Point::new(x, y), 6, INK);
                        cv.disc(Point::new(x, y), 3, PARCHMENT);
                    }
                }
            }
        }
        header(cv, "Easier route", rx.w, self.ready.then_some((self.ordinal, self.count)));
        cv.fill(rect(0, 188, rx.w, rx.h - 188), PARCHMENT);
        if let Some(next) = self.next.filter(|_| self.ready) {
            benefit(cv, self.current, next, self.goal as usize, rect(8, 190, rx.w - 16, 86), true);
            button(cv, "Preview route");
        } else {
            cv.text(
                if self.unavailable {
                    match self.failure {
                        Some(obc_route::NavError::Exhausted) => "Search limit reached",
                        Some(obc_route::NavError::NoPath) => "No connecting route",
                        None if !self.current.elevation_complete => "Comparison unavailable",
                        None => "No useful route available",
                    }
                } else {
                    "Comparing routes…"
                },
                Point::new(rx.w / 2, 223),
                Font::Label,
                TextAlign::Center,
                INK,
            );
        }
    }
}
fn header(cv: &mut impl Surface, title: &str, w: i32, count: Option<(u8, u8)>) {
    cv.round(rect(4, 4, w - 8, 34), 6, INK);
    cv.text(title, Point::new(12, 9), Font::Label, TextAlign::Left, PARCHMENT);
    if let Some((index, count)) = count {
        let mut s = heapless::String::<8>::new();
        let _ = write!(s, "{index}/{count}");
        cv.text(&s, Point::new(w - 12, 9), Font::Label, TextAlign::Right, PARCHMENT);
    }
}
fn button(cv: &mut impl Surface, text: &str) {
    cv.round(rect(12, 280, 216, 32), 6, AMBER);
    cv.text(text, Point::new(120, 282), Font::Body, TextAlign::Center, INK);
}
fn fit(b: BBox, w: i32, h: i32) -> Viewport {
    let lat = b.min_lat + (b.max_lat - b.min_lat) / 2;
    let aspect = libm::cosf(lat as f32 * core::f32::consts::PI / 180_000_000.0);
    let zoom = ((w - 48) as f32 / ((b.max_lon - b.min_lon).max(1) as f32 * aspect))
        .min(104.0 / (b.max_lat - b.min_lat).max(1) as f32);
    Viewport::new(
        w as f32,
        h as f32,
        b.min_lon + (b.max_lon - b.min_lon) / 2,
        lat - ((h as f32 / 2.0 - 114.0) / zoom) as i32,
        zoom,
    )
}
fn distance(value: u32, ascent: bool) -> heapless::String<16> {
    let mut text = heapless::String::new();
    if ascent || value < 1000 {
        let _ = write!(text, "{value}m");
    } else if value.is_multiple_of(1000) {
        let _ = write!(text, "{}km", value / 1000);
    } else {
        let _ = write!(text, "{}.{:01}km", value / 1000, value % 1000 / 100);
    }
    text
}

fn benefit(
    cv: &mut impl Surface,
    current: Costs,
    next: Costs,
    goal: usize,
    area: embedded_graphics::primitives::Rectangle,
    heading: bool,
) {
    let title = heading.then_some(NAMES[goal]);
    let (old, new, label) = match goal {
        0 => (current.ascent_m, next.ascent_m, "ascent saved"),
        1 => (current.rough_m, next.rough_m, "rough avoided"),
        _ => (current.distance_m, next.distance_m, "distance saved"),
    };
    let text = distance(Goal::ALL[goal].saving(current, next), goal == 0);
    let center = area.top_left.x + area.size.width as i32 / 2;
    let label_height = Font::Label.cap_height() as i32;
    let number_height = Font::Display.cap_height() as i32;
    let heading_height = title.map_or(0, |_| label_height + 8);
    let content_height = heading_height + number_height + 8 + label_height;
    let mut top = area.top_left.y + (area.size.height as i32 - content_height) / 2;
    cv.round(area, 6, AMBER);
    if let Some(title) = title {
        cv.text_vcentered(title, center, (top, label_height), Font::Label, TextAlign::Center, INK);
        top += heading_height;
    }
    let row_width = 20 + 8 + obc_render::text::text_width(&text, Font::Display) as i32;
    let left = center - row_width / 2;
    cv.text_vcentered(&text, left + 28, (top, number_height), Font::Display, TextAlign::Left, INK);
    cv.text_vcentered(
        if new > old { "more than now" } else { label },
        center,
        (top + number_height + 8, label_height),
        Font::Label,
        TextAlign::Center,
        INK,
    );
    let x = left + 10;
    let y = top + number_height / 2;
    match goal {
        0 => cv.triangle(Point::new(x - 9, y + 7), Point::new(x, y - 9), Point::new(x + 9, y + 7), INK),
        1 => {
            cv.vline(x - 8, y - 9, 18, 2, INK);
            cv.vline(x + 7, y - 9, 18, 2, INK);
            for offset in [-7, 0, 7] {
                cv.vline(x, y + offset, 3, 1, INK);
            }
        }
        _ => {
            cv.hline(x - 8, y, 16, INK);
            cv.disc(Point::new(x - 8, y), 3, INK);
            cv.disc(Point::new(x + 8, y), 3, INK);
        }
    }
}
