//! Nearby landmark study: stable map/list selection, short text pages, and the shared visit preview.

use super::*;
use crate::assistant_demo::{photos::paragraph, Fixture, Stop};

#[derive(Debug, Default)]
pub(super) struct View {
    nearby: heapless::Vec<u8, 4>,
    selected: usize,
    page: Option<usize>,
    origin: (i32, i32),
}

pub(super) enum Action {
    None,
    Back,
    Preview(u8),
}

impl View {
    pub fn new(fixture: &Fixture, origin: (i32, i32)) -> Self {
        let mut view = Self { origin, ..Self::default() };
        for (i, stop) in fixture.stops.iter().enumerate() {
            if stop.landmark.is_none() || stop.open_now == Some(false) {
                continue;
            }
            let Ok(i) = u8::try_from(i) else { continue };
            let distance = direct_distance(origin, stop);
            let at = view.nearby.partition_point(|&i| direct_distance(origin, &fixture.stops[i as usize]) <= distance);
            if at < view.nearby.capacity() {
                if view.nearby.is_full() {
                    view.nearby.pop();
                }
                let _ = view.nearby.insert(at, i);
            }
        }
        view
    }

    pub fn handle(&mut self, g: Gesture, fixture: &Fixture) -> Action {
        match g {
            Gesture::Back if self.page.is_some() => self.page = None,
            Gesture::Back => return Action::Back,
            Gesture::Step(n) => {
                if let Some(page) = self.page {
                    let stop = &fixture.stops[self.nearby[self.selected] as usize];
                    let landmark = stop.landmark.unwrap();
                    self.page = Some(list::step_selection(
                        page,
                        n,
                        landmark.pages.len() + 2 * usize::from(landmark.photo.is_some()),
                    ));
                } else {
                    self.selected = list::step_selection(self.selected, n, self.nearby.len().max(1));
                }
            }
            Gesture::Press if !self.nearby.is_empty() => {
                if self.page.is_some() {
                    return Action::Preview(self.nearby[self.selected]);
                }
                self.page = Some(0);
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
        if let Some(page) = self.page {
            let index = self.nearby[self.selected];
            let stop = &demo.fixture.stops[index as usize];
            let landmark = stop.landmark.unwrap();
            cv.fill(rect(0, 0, rx.w, rx.h), PARCHMENT);
            cv.round(rect(4, 4, rx.w - 8, 34), 6, WOOD);
            cv.text(stop.name, Point::new(12, 9), Font::Label, TextAlign::Left, PARCHMENT);
            if page == landmark.pages.len() + 1 {
                if let Some(photo) = landmark.photo {
                    crate::assistant_demo::photos::draw(cv, photo, Point::new(12, 40), true);
                    let mut label = heapless::String::<24>::new();
                    let _ = write!(label, "Visit  {0}/{0}", landmark.pages.len() + 2);
                    button(cv, &label, 280, true);
                    return;
                }
            }
            subtitle(cv, stop, direct_distance(self.origin, stop), 48);
            if let Some(text) = landmark.pages.get(page) {
                paragraph(cv, text, 82);
            } else if let Some(photo) = landmark.photo {
                crate::assistant_demo::photos::draw(cv, photo, Point::new(40, 86), false);
                cv.text("160 x 120", Point::new(120, 218), Font::Label, TextAlign::Center, SUBTEXT);
            }
            let mut pages = heapless::String::<24>::new();
            let _ = write!(
                pages,
                "Up/down  {}/{}",
                page + 1,
                landmark.pages.len() + 2 * usize::from(landmark.photo.is_some())
            );
            cv.text(&pages, Point::new(120, 252), Font::Label, TextAlign::Center, SUBTEXT);
            button(cv, "Visit", 280, true);
            return;
        }
        let mut min = self.origin;
        let mut max = min;
        for &i in &self.nearby {
            let (lon, lat) = demo.fixture.stops[i as usize].position();
            min = (min.0.min(lon), min.1.min(lat));
            max = (max.0.max(lon), max.1.max(lat));
        }
        let vp = fit(min, max, rx.w, rx.h, 208, 40);
        let _ = super::super::map::draw_map_scene(cv, rx, &vp, None);
        let (x, y) = vp.to_screen(self.origin.0, self.origin.1);
        cv.disc(Point::new(x, y), 7, INK);
        cv.disc(Point::new(x, y), 5, PARCHMENT);
        cv.disc(Point::new(x, y), 2, INK);
        for (row, &i) in self.nearby.iter().enumerate() {
            let pos = demo.fixture.stops[i as usize].position();
            let (x, y) = vp.to_screen(pos.0, pos.1);
            marker(cv, x, y, row, row == self.selected);
        }
        cv.fill(rect(0, 0, rx.w, 40), PARCHMENT);
        cv.round(rect(4, 4, rx.w - 8, 34), 6, WOOD);
        cv.text("Landmarks", Point::new(14, 8), Font::Body, TextAlign::Left, PARCHMENT);
        cv.fill(rect(0, 208, rx.w, rx.h - 208), PARCHMENT);
        if let Some(&i) = self.nearby.get(self.selected) {
            let stop = &demo.fixture.stops[i as usize];
            cv.round(rect(6, 210, rx.w - 12, 104), 6, AMBER);
            let mut count = heapless::String::<16>::new();
            let _ = write!(count, "{}  {}/{}", letter(self.selected), self.selected + 1, self.nearby.len());
            cv.text(&count, Point::new(12, 212), Font::Label, TextAlign::Left, INK);
            cv.text(stop.landmark.unwrap().kind, Point::new(rx.w - 12, 212), Font::Label, TextAlign::Right, SUBTEXT);
            cv.text(stop.name, Point::new(12, 238), Font::Label, TextAlign::Left, INK);
            let mut label = heapless::String::<24>::new();
            super::super::vocab::fmt::write_distance_coarse(
                &mut label,
                "",
                direct_distance(self.origin, stop),
                crate::Units::Metric,
            );
            let _ = label.push_str(" straight line");
            cv.text(&label, Point::new(12, 262), Font::Label, TextAlign::Left, INK);
            cv.text("Select to read", Point::new(120, 288), Font::Label, TextAlign::Center, SUBTEXT);
        } else {
            cv.text("None found nearby", Point::new(12, 220), Font::Label, TextAlign::Left, INK);
        }
    }
}

fn direct_distance(origin: (i32, i32), stop: &Stop) -> u32 {
    obc_map_scene::ground_dist_m(origin, stop.position()) as u32
}

fn subtitle(cv: &mut impl Surface, stop: &Stop, distance: u32, y: i32) {
    let mut text = heapless::String::<32>::new();
    let _ = write!(text, "{}  ", stop.landmark.unwrap().kind);
    super::super::vocab::fmt::write_distance_coarse(&mut text, "", distance, crate::Units::Metric);
    cv.text(&text, Point::new(30, y), Font::Label, TextAlign::Left, SUBTEXT);
}

pub(super) fn marker(cv: &mut impl Surface, x: i32, y: i32, row: usize, selected: bool) {
    cv.round(rect(x - 13, y - 14, 27, 28), 4, INK);
    cv.round(rect(x - 11, y - 12, 23, 24), 3, if selected { AMBER } else { PARCHMENT });
    cv.text(letter(row), Point::new(x, y - 12), Font::Label, TextAlign::Center, INK);
}

pub(super) fn sources(cv: &mut impl Surface, demo: Demo, page: usize) {
    let landmarks = || demo.fixture.stops.iter().filter_map(|stop| stop.landmark);
    let (title, text) = match page {
        0 => ("Text sources", "Wikipedia contributors. Shortened and reworded. CC BY-SA 4.0. No warranties."),
        1 => ("Text licence", "https://creativecommons.org/licenses/by-sa/4.0/"),
        _ if page < landmarks().count() + 2 => ("Article source", landmarks().nth(page - 2).unwrap().article),
        _ => {
            let offset = page - landmarks().count() - 2;
            let photo = landmarks().filter_map(|l| l.photo).nth(offset / 3).unwrap();
            match offset % 3 {
                0 => ("Photo credit", photo.credit),
                1 => ("Photo source", photo.source),
                _ => ("Photo licence", photo.licence),
            }
        }
    };
    cv.fill(rect(0, 0, 240, 320), PARCHMENT);
    cv.round(rect(4, 4, 232, 34), 6, WOOD);
    cv.text(title, Point::new(12, 8), Font::Body, TextAlign::Left, PARCHMENT);
    let mut url = heapless::String::<128>::new();
    if page >= 2 && page < landmarks().count() + 2 {
        let _ = url.push_str("https://en.wikipedia.org/wiki/");
    }
    let _ = url.push_str(text);
    paragraph(cv, &url, 66);
    let mut pages = heapless::String::<24>::new();
    let _ = write!(pages, "Up/down  {}/{}", page + 1, source_pages(demo));
    cv.text(&pages, Point::new(120, 252), Font::Label, TextAlign::Center, SUBTEXT);
    button(cv, "Back", 280, true);
}

pub(super) fn source_pages(demo: Demo) -> usize {
    2 + demo
        .fixture
        .stops
        .iter()
        .filter_map(|stop| stop.landmark)
        .map(|l| 1 + usize::from(l.photo.is_some()) * 3)
        .sum::<usize>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assistant_demo::Landmark;

    #[test]
    fn nearby_orders_by_direct_distance_and_keeps_unknown_hours() {
        static INFO: Landmark = Landmark { kind: "Gorge", article: "Aare_Gorge", photo: None, pages: &["A gorge."] };
        const STOP: Stop = Stop {
            name: "Landmark",
            landmark: Some(&INFO),
            open_now: None,
            approach: &[(100, 100)],
            distance_m: 1,
            climb_m: 0,
            extra_m: 0,
            extra_climb_m: 0,
            return_m: 0,
            return_climb_m: 0,
            outbound: 1,
            continuation: 2,
        };
        static FIXTURE: Fixture = Fixture {
            original: 0,
            start: (0, 0),
            stops: &[
                Stop { approach: &[(500, 500)], ..STOP },
                Stop { open_now: Some(false), ..STOP },
                Stop { landmark: None, ..STOP },
                Stop { distance_m: 9_000, ..STOP },
                Stop { approach: &[(300, 300)], open_now: Some(true), ..STOP },
                Stop { approach: &[(900, 900)], ..STOP },
            ],
        };
        let mut view = View::new(&FIXTURE, FIXTURE.start);
        assert_eq!(view.nearby.as_slice(), [3, 4, 0, 5]);
        view.handle(Gesture::Step(1), &FIXTURE);
        view.handle(Gesture::Press, &FIXTURE);
        view.handle(Gesture::Step(1), &FIXTURE);
        assert!(matches!(view.handle(Gesture::Press, &FIXTURE), Action::Preview(4)));
        view.handle(Gesture::Back, &FIXTURE);
        assert_eq!(view.selected, 1);
        assert_eq!(view.page, None);
        let elsewhere = View::new(&FIXTURE, (1_000, 1_000));
        assert_eq!(elsewhere.nearby[0], 5, "distance is measured from the rider");
    }
}
