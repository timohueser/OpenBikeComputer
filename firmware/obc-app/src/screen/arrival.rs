//! The arrival view: the rider reached the end of the loaded route during a ride.
//!
//! Finish ride saves the ride on the same hold as Finish on the Paused page. Ride on loads the next
//! trip day and keeps recording; the ride keeps the day it started on. Keep riding, and Back, close
//! the view and leave the route loaded. Only the card scheduler opens it.

use core::fmt::Write;

use obc_render::Surface;

use super::vocab::card::{ActionRows, CardEvent};
use super::vocab::chrome::title_frame;
use super::vocab::rows::{draw_prompt, PromptOption};
use super::{Ctx, Render, Transition};
use crate::input::Gesture;
use crate::{Msg, RecorderIntent};

/// What the view offers, fixed when it opens. The card scheduler holds one, so it stays small.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ArrivalView {
    /// The loaded route's catalog index.
    pub(crate) route: u16,
    /// The loaded route's trip day, from 0.
    pub(crate) day: Option<u16>,
    /// The next trip day's catalog index. The rider stands at the end of the day, so it loads as
    /// it is.
    pub(crate) next: Option<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Finish,
    RideOn,
    KeepRiding,
}

#[derive(Debug)]
pub struct ArrivalScreen {
    view: ArrivalView,
    rows: ActionRows,
}

impl ArrivalScreen {
    pub(crate) fn new(view: ArrivalView) -> Self {
        ArrivalScreen { view, rows: ActionRows::new(0) }
    }

    /// Follow the routes through a catalog rescan. A next day that vanished takes its row away.
    pub(crate) fn remap_routes(&mut self, remap: &dyn Fn(usize) -> Option<usize>) {
        let remap = |i: u16| remap(usize::from(i)).and_then(|i| u16::try_from(i).ok());
        self.view.route = remap(self.view.route).unwrap_or(u16::MAX);
        self.view.next = self.view.next.and_then(remap);
    }

    fn rows(&self) -> heapless::Vec<Row, 3> {
        let mut rows = heapless::Vec::new();
        let _ = rows.push(Row::Finish);
        if self.view.next.is_some() {
            let _ = rows.push(Row::RideOn);
        }
        let _ = rows.push(Row::KeepRiding);
        rows
    }

    fn guards(rows: &[Row]) -> heapless::Vec<bool, 3> {
        rows.iter().map(|&row| row == Row::Finish).collect()
    }

    /// True when the highlighted row fills for a hold, which makes the app repaint that fill.
    pub fn selection_is_guarded(&self) -> bool {
        self.rows.selection_is_guarded(&Self::guards(&self.rows()))
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        let rows = self.rows();
        match self.rows.handle(g, &Self::guards(&rows)) {
            CardEvent::Activate(i) => match rows.get(i) {
                Some(Row::Finish) => super::ride_control::end_ride(cx, RecorderIntent::Save),
                Some(Row::RideOn) => {
                    if let Some(next) = self.view.next.map(usize::from).filter(|&i| i < cx.routes.len()) {
                        cx.navigator.load_route(next);
                    }
                    Transition::Pop
                }
                _ => Transition::Pop,
            },
            CardEvent::Dismiss => Transition::Pop,
            CardEvent::None => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, rx.t(Msg::ArrivalTitle), "");
        let name = |i: u16| rx.routes.get(usize::from(i)).map_or("", |r| r.name.as_str());
        let day_word = rx.t(Msg::RouteMenuDay);

        let here = name(self.view.route);
        let mut question: heapless::String<96> = heapless::String::new();
        match self.view.day.and_then(|day| Some((day + 1, day_place(here, day + 1)?))) {
            Some((number, place)) => {
                let _ = write!(question, "{place}{}{day_word} {number}.", rx.t(Msg::ArrivalEndOfDay));
            }
            None => {
                let _ = write!(question, "{}{here}.", rx.t(Msg::ArrivalEndOf));
            }
        }

        let mut ride_on: heapless::String<32> = heapless::String::new();
        let mut ride_on_hint: heapless::String<80> = heapless::String::new();
        if let (Some(day), Some(next)) = (self.view.day, self.view.next.and_then(|i| rx.routes.get(usize::from(i)))) {
            let number = day + 2;
            let _ = write!(ride_on, "{}{day_word} {number}", rx.t(Msg::ArrivalRideOn));
            let units = rx.settings.units;
            let unit = if units.is_imperial() { "mi" } else { "km" };
            let to = day_place(&next.name, number).unwrap_or(&next.name);
            let figure = (units.dist(next.distance_km as f32) + 0.5) as u32;
            let _ = write!(ride_on_hint, "{figure} {unit}{}{to}", rx.t(Msg::ArrivalTo));
        }

        let rows = self.rows();
        let mut options: heapless::Vec<PromptOption, 3> = heapless::Vec::new();
        for row in &rows {
            let option = match row {
                Row::Finish => PromptOption {
                    label: rx.t(Msg::ArrivalFinish),
                    hint: Some(rx.t(Msg::ArrivalFinishHint)),
                    guard: true,
                },
                Row::RideOn => PromptOption { label: &ride_on, hint: Some(&ride_on_hint), guard: false },
                Row::KeepRiding => PromptOption {
                    label: rx.t(Msg::ArrivalKeepRiding),
                    hint: Some(rx.t(Msg::ArrivalKeepRidingHint)),
                    guard: false,
                },
            };
            let _ = options.push(option);
        }
        draw_prompt(cv, w, &question, &options, self.rows.selected(), rx.hold_progress);
    }
}

/// The place in a day route's name: "Brig" in "Day 3 Brig", in any language, when the number is
/// `number`. The phone names day routes this way; any other name has no place.
fn day_place(name: &str, number: u16) -> Option<&str> {
    let mut words = name.splitn(3, ' ');
    let (_, n, place) = (words.next()?, words.next()?, words.next()?.trim());
    (n.parse::<u16>().ok()? == number && !place.is_empty()).then_some(place)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_day_route_name_gives_its_place() {
        assert_eq!(day_place("Day 3 Brig", 3), Some("Brig"));
        assert_eq!(day_place("Tag 2 Ulrichen ob Brig", 2), Some("Ulrichen ob Brig"));
        assert_eq!(day_place("Day 3 Brig", 2), None, "another day's number");
        assert_eq!(day_place("Grimsel climb", 1), None);
        assert_eq!(day_place("Day 3", 3), None);
    }
}
