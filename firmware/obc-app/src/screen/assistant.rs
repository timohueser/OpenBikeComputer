//! Shop-visit interaction study, drawn by the device renderer over its ordinary map scene.

mod whats_next;

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
    assistant_demo::{Demo, Phase, Stage},
    Gesture,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Questions,
    WhatsNext,
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
    ahead: whats_next::View,
}

const QUESTIONS: [&str; 8] = [
    "Find a place",
    "What's next",
    "Next town",
    "Easier option",
    "Road blocked",
    "Back on route",
    "Landmarks",
    "Worth a detour",
];
const CATEGORIES: [&str; 6] = ["Water", "Shop", "Pharmacy", "Bike repair", "Accommodation", "Train station"];

impl AssistantScreen {
    pub fn new(demo: Demo) -> Self {
        let page = match demo.phase {
            Phase::Riding => Page::Questions,
            Phase::ToStop | Phase::Returning => Page::Visit,
        };
        Self { page, selected: 0, ahead: whats_next::View::new(false) }
    }

    pub fn arrival() -> Self {
        Self { page: Page::Arrived, selected: 0, ahead: whats_next::View::new(false) }
    }

    pub(crate) fn for_stage(stage: Stage, selected: usize) -> Option<Self> {
        let page = match stage {
            Stage::Questions => Page::Questions,
            Stage::WhatsNext | Stage::ExploreAhead => Page::WhatsNext,
            Stage::Categories => Page::Categories,
            Stage::Choices => Page::Choices,
            Stage::Preview => Page::Preview,
            Stage::Visit => Page::Visit,
            Stage::Arrived => Page::Arrived,
            Stage::Skip => Page::Skip,
            _ => return None,
        };
        Some(Self {
            page,
            ahead: whats_next::View::new(stage == Stage::ExploreAhead),
            selected: match page {
                Page::Choices => selected,
                Page::Categories => 1,
                _ => 0,
            },
        })
    }

    fn go(&mut self, page: Page, selected: usize) -> Transition {
        self.page = page;
        self.selected = selected;
        Transition::None
    }

    pub(crate) fn has_ahead_context(&self) -> bool {
        self.page == Page::WhatsNext && self.ahead.is_timeline()
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        if self.page == Page::WhatsNext {
            return if self.ahead.handle(g, cx.up_ahead_scope()) {
                self.go(Page::Questions, 1)
            } else {
                Transition::None
            };
        }
        let Some(mut demo) = cx.state.assistant_demo else { return Transition::Pop };
        match g {
            Gesture::Step(n) => {
                let count = match self.page {
                    Page::Questions => QUESTIONS.len(),
                    Page::Categories => 6,
                    Page::Choices => demo.candidates.len as usize,
                    Page::Visit => {
                        if demo.phase == Phase::ToStop {
                            3
                        } else {
                            2
                        }
                    }
                    _ => 1,
                };
                self.selected = list::step_selection(self.selected, n, count.max(1));
                if self.page == Page::Choices {
                    demo.selected = self.selected as u8;
                    cx.state.assistant_demo = Some(demo);
                }
                Transition::None
            }
            Gesture::Back => match self.page {
                Page::Questions | Page::Arrived | Page::Visit => Transition::Pop,
                Page::WhatsNext => unreachable!(),
                Page::Categories => self.go(Page::Questions, 0),
                Page::Choices => self.go(Page::Categories, 1),
                Page::Preview => self.go(Page::Choices, demo.selected as usize),
                Page::Skip => self.go(Page::Visit, 1),
            },
            Gesture::Press => match self.page {
                Page::Questions if self.selected == 0 => self.go(Page::Categories, 1),
                Page::Questions if self.selected == 1 => {
                    cx.state.up_ahead_filter = obc_reader::PoiCategorySet::ALL;
                    self.ahead = whats_next::View::new(false);
                    self.go(Page::WhatsNext, 0)
                }
                Page::Categories if self.selected == 1 => {
                    demo.selected = 0;
                    cx.state.assistant_demo = Some(demo);
                    self.go(Page::Choices, 0)
                }
                Page::Choices if demo.candidates.len > 0 => {
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
                Page::Visit if demo.phase == Phase::ToStop && self.selected == 1 => self.go(Page::Skip, 0),
                Page::Visit => self.go(Page::Questions, 0),
                Page::Arrived => Transition::Pop,
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
        if self.page == Page::WhatsNext {
            self.ahead.draw(cv, rx.up_ahead_scope());
            return;
        }
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
                    if i == active || (self.page == Page::Questions && i == 1) { INK } else { SUBTEXT },
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
        let map_bottom = match self.page {
            Page::Choices => 208,
            Page::Preview => 192,
            Page::Visit => 174,
            Page::Arrived => 154,
            _ => 176,
        };
        if choices && demo.candidates.len == 0 {
            let vp = rx.state.viewport(rx.w as f32, rx.h as f32);
            let _ = super::map::draw_map_scene(cv, rx, &vp, None);
            cv.fill(rect(0, 0, rx.w, 40), PARCHMENT);
            cv.round(rect(4, 4, rx.w - 8, 34), 6, WOOD);
            cv.text("Shops", Point::new(14, 8), Font::Body, TextAlign::Left, PARCHMENT);
            cv.fill(rect(0, map_bottom, rx.w, rx.h - map_bottom), PARCHMENT);
            cv.text("No shops found", Point::new(14, 220), Font::Body, TextAlign::Left, INK);
            cv.text("Try another area", Point::new(14, 254), Font::Label, TextAlign::Left, SUBTEXT);
            return;
        }
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
        for (i, stop) in demo.stops().enumerate() {
            if choices || i == demo.selected as usize {
                let (x, y) = vp.to_screen(stop.position().0, stop.position().1);
                cv.round(rect(x - 13, y - 14, 27, 28), 4, INK);
                cv.round(rect(x - 11, y - 12, 23, 24), 3, if i == demo.selected as usize { AMBER } else { PARCHMENT });
                if choices {
                    cv.text(letter(i), Point::new(x, y - 12), Font::Label, TextAlign::Center, INK);
                } else {
                    super::poi_menu::draw_category_icon(
                        cv,
                        obc_reader::PoiCategory::Resupply,
                        Point::new(x, y),
                        INK,
                        AMBER,
                    );
                }
            }
        }
        cv.fill(rect(0, 0, rx.w, 40), PARCHMENT);
        cv.round(rect(4, 4, rx.w - 8, 34), 6, WOOD);
        let title = match self.page {
            Page::Choices => "Shops",
            Page::Preview => demo.stop().name,
            Page::Arrived => "Arrived",
            Page::Skip => "Skip shop?",
            Page::Visit if demo.phase == Phase::Returning => "Back to route",
            _ => "Shop stop",
        };
        cv.text(title, Point::new(14, 8), Font::Body, TextAlign::Left, PARCHMENT);
        cv.fill(rect(0, map_bottom, rx.w, rx.h - map_bottom), PARCHMENT);
        let stop = demo.stop();
        if choices {
            cv.round(rect(10, 210, rx.w - 20, 104), 6, AMBER);
            let role = if stop.on_way() {
                "On the way"
            } else if demo.stops().all(|other| stop.distance_m <= other.distance_m) {
                "Nearest"
            } else {
                "Detour"
            };
            let mut label = heapless::String::<24>::new();
            let _ = write!(label, "{} {role}", letter(self.selected));
            cv.text(&label, Point::new(18, 212), Font::Label, TextAlign::Left, SUBTEXT);
            let mut count = heapless::String::<8>::new();
            let _ = write!(count, "{}/{}", self.selected + 1, demo.candidates.len);
            cv.text(&count, Point::new(rx.w - 18, 212), Font::Label, TextAlign::Right, INK);
            cv.text(stop.name, Point::new(18, 236), Font::Body, TextAlign::Left, INK);
            figures(cv, stop.distance_m, stop.climb_m, 264, false);
            let mut line = heapless::String::<24>::new();
            super::vocab::fmt::write_distance_coarse(&mut line, "+", stop.extra_m, rx.settings.units);
            let _ = line.push_str(" extra");
            cv.text(&line, Point::new(18, 288), Font::Label, TextAlign::Left, INK);
        } else if self.page == Page::Preview {
            figures(cv, stop.distance_m, stop.climb_m, 196, false);
            cv.text("Extra incl. return", Point::new(14, 222), Font::Label, TextAlign::Left, SUBTEXT);
            figures(cv, stop.extra_m, stop.extra_climb_m, 248, true);
            button(cv, "Add stop", 280, true);
        } else if self.page == Page::Arrived {
            cv.text(stop.name, Point::new(14, 158), Font::Body, TextAlign::Left, INK);
            cv.text("Guidance continues", Point::new(14, 190), Font::Label, TextAlign::Left, INK);
            cv.text("Back to your route", Point::new(14, 214), Font::Label, TextAlign::Left, SUBTEXT);
            figures(cv, stop.return_m, stop.return_climb_m, 240, false);
            button(cv, "Dismiss", 280, true);
        } else if self.page == Page::Skip {
            cv.text(stop.name, Point::new(14, 180), Font::Body, TextAlign::Left, INK);
            cv.text("Use original route", Point::new(14, 215), Font::Label, TextAlign::Left, INK);
            cv.text("Rejoin when ready.", Point::new(14, 243), Font::Label, TextAlign::Left, SUBTEXT);
            button(cv, "Remove stop", 280, true);
        } else {
            cv.text(stop.name, Point::new(14, 178), Font::Body, TextAlign::Left, INK);
            button(cv, "Back to map", 208, self.selected == 0);
            if demo.phase == Phase::ToStop {
                button(cv, "Skip stop", 244, self.selected == 1);
            }
            button(cv, "Assistant", 280, self.selected == if demo.phase == Phase::ToStop { 2 } else { 1 });
        }
    }
}

fn viewport(demo: Demo, w: i32, h: i32, bottom: i32, choices: bool) -> Viewport {
    let mut min = if choices { demo.fixture.start } else { demo.stop().approach[0] };
    let mut max = min;
    for (i, stop) in demo.stops().enumerate() {
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

fn letter(index: usize) -> &'static str {
    ["A", "B", "C", "D"][index]
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
