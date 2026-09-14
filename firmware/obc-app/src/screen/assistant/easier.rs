//! Map-first route comparison. Browsing never changes the accepted route.

use super::*;
use crate::assistant_demo::easier::Route;

const NAMES: [&str; 3] = ["Less climbing", "Smoother surface", "Shorter ride"];

#[derive(Debug, Default)]
pub(super) struct View {
    selected: usize,
    review: bool,
}

pub(super) enum Action {
    None,
    Back,
    Accept(usize),
}

impl View {
    pub fn new(selected: usize, review: bool) -> Self {
        Self { selected: selected.min(2), review }
    }

    pub fn handle(&mut self, gesture: Gesture, demo: &mut Demo) -> Action {
        match gesture {
            Gesture::Back if self.review => self.review = false,
            Gesture::Back => return Action::Back,
            Gesture::Step(n) if !self.review => self.selected = list::step_selection(self.selected, n, 3),
            Gesture::Press if demo.phase == Phase::Riding => {
                if let Some(routes) = demo.easier {
                    if self.review {
                        demo.easier_accepted = Some(self.selected as u8);
                        return Action::Accept(routes.alternatives[self.selected].route);
                    }
                    self.review = true;
                }
            }
            _ => {}
        }
        Action::None
    }

    pub fn draw<D, F, S>(&self, cv: &mut Canvas<D, F>, rx: &mut RenderFrame<'_, S>, demo: Demo)
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
        S: obc_map_scene::MapScene,
    {
        let Some(routes) = demo.easier.filter(|_| demo.phase == Phase::Riding) else {
            title_frame(cv, rx.w, rx.h, "Easier route", "");
            let text = if demo.phase == Phase::Riding { "No alternatives" } else { "Finish visit first" };
            cv.text(text, Point::new(12, 80), Font::Label, TextAlign::Left, INK);
            return;
        };
        let current = routes.current(demo.easier_accepted);
        let next = &routes.alternatives[self.selected];
        if self.review {
            cv.clear(PARCHMENT);
            header(cv, rx.w, NAMES[self.selected], None);
            cv.round(rect(8, 46, rx.w - 16, 70), 6, AMBER);
            benefit(cv, current, next, self.selected, 45);
            cv.text("Now", Point::new(150, 129), Font::Label, TextAlign::Right, SUBTEXT);
            cv.text("New", Point::new(226, 129), Font::Label, TextAlign::Right, DETOUR);
            cv.hline(12, 157, rx.w - 24, RULE);
            for (i, (label, old, new)) in [
                ("Ride", current.distance_m, next.distance_m),
                ("Climb", current.climb_m, next.climb_m),
                ("Rough", current.rough_m, next.rough_m),
            ]
            .into_iter()
            .enumerate()
            {
                let y = 163 + i as i32 * 36;
                cv.text(label, Point::new(12, y), Font::Label, TextAlign::Left, INK);
                let format = |value| distance(value, i == 1);
                cv.text(&format(old), Point::new(150, y), Font::Label, TextAlign::Right, SUBTEXT);
                cv.text(&format(new), Point::new(226, y), Font::Label, TextAlign::Right, DETOUR);
                if i < 2 {
                    cv.hline(12, y + 30, rx.w - 24, RULE);
                }
            }
            button(cv, "Use this route", 280, true);
            return;
        }
        let mut min = demo.fixture.start;
        let mut max = min;
        for &(lon, lat) in routes.current.path.iter().chain(routes.alternatives.iter().flat_map(|r| r.path)) {
            min = (min.0.min(lon), min.1.min(lat));
            max = (max.0.max(lon), max.1.max(lat));
        }
        let vp = fit(min, max, rx.w, rx.h, 188, 40);
        let active = rx.route.take();
        let _ = super::super::map::draw_map_scene(cv, rx, &vp, None);
        rx.route = active;
        if let Some(scratch) = rx.scratch.as_deref_mut() {
            let (target, color) = cv.split();
            scratch.stroke_path(target, &vp, current.path.iter().copied(), color(ROUTE), 4);
            scratch.stroke_path(target, &vp, next.path.iter().copied(), color(DETOUR), 4);
        }
        for (position, finish) in [(next.path.first(), false), (next.path.last(), true)] {
            if let Some(&(lon, lat)) = position {
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
        header(cv, rx.w, "Easier route", Some(self.selected + 1));
        cv.fill(rect(0, 188, rx.w, rx.h - 188), PARCHMENT);
        cv.round(rect(8, 190, rx.w - 16, 86), 6, AMBER);
        cv.text(NAMES[self.selected], Point::new(16, 190), Font::Label, TextAlign::Left, INK);
        benefit(cv, current, next, self.selected, 217);
        button(cv, "Preview route", 280, true);
    }
}

fn header(cv: &mut impl Surface, w: i32, title: &str, count: Option<usize>) {
    cv.fill(rect(0, 0, w, 40), PARCHMENT);
    cv.round(rect(4, 4, w - 8, 34), 6, WOOD);
    cv.text(title, Point::new(12, 9), Font::Label, TextAlign::Left, PARCHMENT);
    if let Some(count) = count {
        let mut label = heapless::String::<8>::new();
        let _ = write!(label, "{count}/3");
        cv.text(&label, Point::new(w - 12, 9), Font::Label, TextAlign::Right, PARCHMENT);
    }
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

fn benefit(cv: &mut impl Surface, current: &Route, next: &Route, goal: usize, y: i32) {
    let (old, new, label) = match goal {
        0 => (current.climb_m, next.climb_m, "ascent saved"),
        1 => (current.rough_m, next.rough_m, "rough avoided"),
        _ => (current.distance_m, next.distance_m, "distance saved"),
    };
    let text = distance(old.abs_diff(new), goal == 0);
    cv.text(&text, Point::new(48, y), Font::Display, TextAlign::Left, INK);
    cv.text(if new > old { "more than now" } else { label }, Point::new(48, y + 31), Font::Label, TextAlign::Left, INK);
    let x = 26;
    let y = y + 20;
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
