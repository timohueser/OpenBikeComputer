//! Shop-visit interaction study, drawn by the device renderer over its ordinary map scene.

use core::fmt::Write;
use embedded_graphics::{draw_target::DrawTarget, prelude::Point};
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Canvas, Surface, Viewport,
};

use super::{
    palette::*,
    vocab::{chrome::title_frame, list},
    Ctx, MapScreen, RenderFrame, Screen, Transition,
};
use crate::{
    assistant_demo::{Demo, Phase},
    Gesture,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Questions,
    Categories,
    Choices,
    Preview,
    Visit,
    Arrived,
    Skip,
}

#[derive(Debug)]
pub struct AssistantScreen {
    page: Page,
    selected: usize,
}

const QUESTIONS: [&str; 7] =
    ["Find a place", "Next town", "Easier option", "Road blocked", "Back on route", "Landmarks", "Worth a detour"];
const CATEGORIES: [&str; 6] = ["Water", "Shop", "Pharmacy", "Bike repair", "Accommodation", "Train station"];

impl AssistantScreen {
    pub fn new(demo: Demo) -> Self {
        let page = match demo.phase {
            Phase::Riding => Page::Questions,
            Phase::Arrived => Page::Arrived,
            _ => Page::Visit,
        };
        Self { page, selected: 0 }
    }

    pub fn arrival() -> Self {
        Self { page: Page::Arrived, selected: 0 }
    }

    fn go(&mut self, page: Page, selected: usize) -> Transition {
        self.page = page;
        self.selected = selected;
        Transition::None
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        let Some(mut demo) = cx.state.assistant_demo else { return Transition::Pop };
        match g {
            Gesture::Step(n) => {
                let count = match self.page {
                    Page::Questions => 7,
                    Page::Categories => 6,
                    Page::Choices => 2,
                    Page::Visit if demo.phase == Phase::ToStop => 2,
                    _ => 1,
                };
                self.selected = list::step_selection(self.selected, n, count);
                Transition::None
            }
            Gesture::Back => match self.page {
                Page::Questions | Page::Arrived | Page::Visit => Transition::Pop,
                Page::Categories => self.go(Page::Questions, 0),
                Page::Choices => self.go(Page::Categories, 1),
                Page::Preview => self.go(Page::Choices, demo.selected as usize),
                Page::Skip => self.go(Page::Visit, 1),
            },
            Gesture::Press => match self.page {
                Page::Questions if self.selected == 0 => self.go(Page::Categories, 1),
                Page::Categories if self.selected == 1 => self.go(Page::Choices, 0),
                Page::Choices => {
                    demo.selected = self.selected as u8;
                    cx.state.assistant_demo = Some(demo);
                    self.go(Page::Preview, 0)
                }
                Page::Preview => {
                    demo.phase = Phase::ToStop;
                    cx.navigator.set_active_route(Some(demo.stop().outbound));
                    cx.state.assistant_demo = Some(demo);
                    Transition::Root(Screen::Map(MapScreen::new()))
                }
                Page::Visit if self.selected == 0 => Transition::Pop,
                Page::Visit if demo.phase == Phase::ToStop => self.go(Page::Skip, 0),
                Page::Arrived => {
                    demo.phase = Phase::Returning;
                    cx.navigator.set_active_route(Some(demo.stop().continuation));
                    cx.state.assistant_demo = Some(demo);
                    Transition::Root(Screen::Map(MapScreen::new()))
                }
                Page::Skip => {
                    demo.phase = Phase::Riding;
                    cx.navigator.set_active_route(Some(demo.fixture.original));
                    cx.state.assistant_demo = Some(demo);
                    Transition::Root(Screen::Map(MapScreen::new()))
                }
                _ => Transition::None,
            },
            _ => Transition::None,
        }
    }

    pub fn draw<D, F, S>(&self, cv: &mut Canvas<D, F>, rx: &mut RenderFrame<'_, S>)
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
        S: obc_map_scene::MapScene,
    {
        let Some(mut demo) = rx.state.assistant_demo else { return };
        if self.page == Page::Questions || self.page == Page::Categories {
            let (title, rows, active): (&str, &[&str], usize) = if self.page == Page::Questions {
                ("Assistant", &QUESTIONS, 0)
            } else {
                ("Find a place", &CATEGORIES, 1)
            };
            title_frame(cv, rx.w, rx.h, title, "");
            let first = list::window_start(self.selected, 6, rows.len());
            for (slot, &label) in rows.iter().skip(first).take(6).enumerate() {
                let i = first + slot;
                let y = 43 + slot as i32 * 44;
                if i == self.selected {
                    cv.round(rect(10, y, rx.w - 20, 40), 6, AMBER);
                }
                cv.text(
                    label,
                    Point::new(18, y + 5),
                    Font::Body,
                    TextAlign::Left,
                    if i == active { INK } else { SUBTEXT },
                );
            }
            if rows.len() > 6 {
                cv.vline(rx.w - 7, 44, 261, 2, RULE);
                cv.vline(rx.w - 7, 44 + first as i32 * 36, 225, 2, WOOD);
            }
            return;
        }
        if self.page == Page::Choices {
            demo.selected = self.selected as u8;
        }
        let choices = self.page == Page::Choices;
        let map_bottom = if matches!(self.page, Page::Choices | Page::Preview) { 143 } else { 176 };
        let vp = viewport(demo, rx.w, rx.h, map_bottom, choices);
        let _ = super::map::draw_map_scene(cv, rx, &vp, None);
        if matches!(self.page, Page::Choices | Page::Preview) {
            if let Some(scratch) = rx.scratch.as_deref_mut() {
                let (target, color) = cv.split();
                scratch.stroke_path(
                    target,
                    &vp,
                    demo.stop().approach.iter().copied(),
                    color(DETOUR),
                    super::ROUTE_WEIGHT,
                );
            }
        }
        for (i, stop) in demo.fixture.stops.iter().enumerate() {
            if choices || i == demo.selected as usize {
                let (x, y) = vp.to_screen(stop.position().0, stop.position().1);
                cv.round(rect(x - 13, y - 14, 27, 28), 4, INK);
                cv.round(rect(x - 11, y - 12, 23, 24), 3, if i == demo.selected as usize { AMBER } else { PARCHMENT });
                cv.text(if i == 0 { "A" } else { "B" }, Point::new(x, y - 12), Font::Label, TextAlign::Center, INK);
            }
        }
        cv.fill(rect(0, 0, rx.w, 40), PARCHMENT);
        cv.round(rect(4, 4, rx.w - 8, 34), 6, WOOD);
        let title = match self.page {
            Page::Choices => "Shops",
            Page::Preview => "Shop visit",
            Page::Arrived => "Arrived",
            Page::Skip => "Skip shop?",
            _ => "Shop stop",
        };
        cv.text(title, Point::new(14, 8), Font::Body, TextAlign::Left, PARCHMENT);
        cv.fill(rect(0, map_bottom, rx.w, rx.h - map_bottom), PARCHMENT);
        let stop = demo.stop();
        if choices {
            for (i, stop) in demo.fixture.stops.iter().enumerate() {
                let y = 146 + i as i32 * 83;
                if i == self.selected {
                    cv.round(rect(10, y, rx.w - 20, 80), 6, AMBER);
                }
                cv.text(
                    if i == 0 { "A On the way" } else { "B Nearest" },
                    Point::new(18, y),
                    Font::Label,
                    TextAlign::Left,
                    SUBTEXT,
                );
                cv.text(stop.name, Point::new(18, y + 20), Font::Label, TextAlign::Left, INK);
                figures(cv, stop.distance_m, stop.climb_m, y + 40, false);
                let mut line = heapless::String::<24>::new();
                super::vocab::fmt::write_distance_coarse(&mut line, "+", stop.extra_m, rx.settings.units);
                let _ = line.push_str(" extra");
                cv.text(&line, Point::new(18, y + 59), Font::Label, TextAlign::Left, INK);
            }
        } else if self.page == Page::Preview {
            cv.text(stop.name, Point::new(14, 146), Font::Body, TextAlign::Left, INK);
            figures(cv, stop.distance_m, stop.climb_m, 176, false);
            cv.text("Extra incl. return", Point::new(14, 202), Font::Label, TextAlign::Left, SUBTEXT);
            figures(cv, stop.extra_m, stop.extra_climb_m, 226, true);
            then_destination(cv, demo, 250);
            button(cv, "Add stop", 280, true);
        } else if self.page == Page::Arrived {
            cv.text(stop.name, Point::new(14, 180), Font::Body, TextAlign::Left, INK);
            cv.text("Back to your route", Point::new(14, 211), Font::Label, TextAlign::Left, SUBTEXT);
            figures(cv, stop.return_m, stop.return_climb_m, 239, false);
            button(cv, "Continue ride", 280, true);
        } else if self.page == Page::Skip {
            cv.text(stop.name, Point::new(14, 180), Font::Body, TextAlign::Left, INK);
            cv.text("Use original route", Point::new(14, 215), Font::Label, TextAlign::Left, INK);
            cv.text("Rejoin when ready.", Point::new(14, 243), Font::Label, TextAlign::Left, SUBTEXT);
            button(cv, "Remove stop", 280, true);
        } else {
            cv.text(stop.name, Point::new(14, 180), Font::Body, TextAlign::Left, INK);
            then_destination(cv, demo, 211);
            button(cv, "Back to map", 244, self.selected == 0);
            if demo.phase == Phase::ToStop {
                button(cv, "Skip stop", 280, self.selected == 1);
            }
        }
    }
}

fn viewport(demo: Demo, w: i32, h: i32, bottom: i32, choices: bool) -> Viewport {
    let mut min = if choices { demo.fixture.start } else { demo.stop().approach[0] };
    let mut max = min;
    for (i, stop) in demo.fixture.stops.iter().enumerate() {
        if choices || i == demo.selected as usize {
            for &(lon, lat) in stop.approach {
                min.0 = min.0.min(lon);
                min.1 = min.1.min(lat);
                max.0 = max.0.max(lon);
                max.1 = max.1.max(lat);
            }
        }
    }
    let lat = min.1 + (max.1 - min.1) / 2;
    let aspect = libm::cosf(lat as f32 * core::f32::consts::PI / 180_000_000.0);
    let zoom = ((w - 48) as f32 / ((max.0 - min.0).max(1) as f32 * aspect))
        .min((bottom - 84) as f32 / (max.1 - min.1).max(1) as f32);
    let desired_y = (40 + bottom) as f32 / 2.0;
    Viewport::new(
        w as f32,
        h as f32,
        min.0 + (max.0 - min.0) / 2,
        lat - ((h as f32 / 2.0 - desired_y) / zoom) as i32,
        zoom,
    )
}

fn figures(cv: &mut impl Surface, distance: u32, climb: u32, y: i32, extra: bool) {
    let mut d = heapless::String::<16>::new();
    if extra {
        let _ = d.push('+');
    }
    if distance < 1000 {
        let _ = write!(d, "{distance}m");
    } else {
        let _ = write!(d, "{}.{:01}km", distance / 1000, distance % 1000 / 100);
    }
    cv.text(&d, Point::new(18, y), Font::Label, TextAlign::Left, INK);
    cv.triangle(Point::new(125, y + 18), Point::new(132, y + 6), Point::new(139, y + 18), INK);
    let mut c = heapless::String::<16>::new();
    let _ = write!(c, "{climb}m");
    cv.text(&c, Point::new(146, y), Font::Label, TextAlign::Left, INK);
}

fn button(cv: &mut impl Surface, label: &str, y: i32, selected: bool) {
    if selected {
        cv.round(rect(12, y, 216, 32), 6, AMBER);
    }
    cv.text(label, Point::new(120, y + 2), Font::Body, TextAlign::Center, INK);
}

fn then_destination(cv: &mut impl Surface, demo: Demo, y: i32) {
    let mut line = heapless::String::<32>::new();
    let _ = write!(line, "Then {}", demo.fixture.destination);
    cv.text(&line, Point::new(14, y), Font::Label, TextAlign::Left, SUBTEXT);
}
