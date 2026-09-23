//! The card after Finish on a trip day. DAY N DONE shows today's ledger and tomorrow's day, and
//! flips to TOMORROW: tomorrow's profile over its distance and time estimate. TRIP DONE shows
//! today's ledger and the trip's totals, and has no page 2. OK returns Home.
//!
//! [`App::land_day_done`](crate::App) opens it when the store confirms the save of a ride on a trip
//! day, so every Finish that names [`Save`](crate::RecorderIntent::Save) leads here. Tomorrow's
//! profile is a derived read into the ride profile's buffer: the rest of the day before and the day,
//! joined, without building the route that joins them.

use core::fmt::Write;

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{text_width, Font, TextAlign},
    Surface,
};

use super::route_menu::climb_group;
use super::vocab::band::ElevationBand;
use super::vocab::chrome::{title_frame, LIST_TOP};
use super::vocab::fmt::{distance_figure, duration_hms};
use super::vocab::marquee::fit;
use super::vocab::pager::ContentPager;
use super::vocab::rows::{draw_guarded_rows, ledger_row, GuardedRowsGeometry, MenuItem};
use super::{palette, Ctx, Render, ScreenTick, Transition};
use crate::input::Gesture;
use crate::route::RouteSummary;
use crate::settings::Units;
use crate::trip::{DayLoad, TripSummary};
use crate::{CatalogObjectId, Msg};

/// Today's ledger: three rows under the title bar, each over a rule.
const ROWS_TOP: i32 = 38;
const ROW_PITCH: i32 = 36;
const ROW_RULE: i32 = 40;

/// The block under the ledger: an olive caption, a name, and one or two olive lines.
const TEXT_X: i32 = 16;
const BLOCK_TOP: i32 = 160;
const NAME_DY: i32 = 22;
const LINE_DY: i32 = 48;
const LINE2_DY: i32 = 70;

/// Page 2: the name, the profile band, and the two ledger rows under it.
const NAME_TOP: i32 = LIST_TOP + 2;
const PROFILE_X: i32 = 12;
const PROFILE_TOP: i32 = 72;
const PROFILE_H: i32 = 111;
const PAGE2_ROWS_TOP: i32 = 186;

/// The OK row, at the bottom of the frame.
const OK_ROW_H: i32 = 38;

/// A finished ride's numbers, as the Paused page showed them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RideTotals {
    pub distance_m: u32,
    pub moving_s: u32,
    pub climb_m: u32,
}

#[derive(Debug)]
pub struct DayDoneScreen {
    today: RideTotals,
    /// The saved ride. The trip totals add today's numbers in its place, so they do not wait for
    /// the catalog read that lists it.
    ride: CatalogObjectId,
    trip: u64,
    /// The finished day, from 0.
    day: u16,
    /// Tomorrow, from 0. `None` when the finished day was the last one: TRIP DONE.
    next: Option<u16>,
    pager: ContentPager,
}

impl DayDoneScreen {
    pub(crate) fn new(today: RideTotals, ride: CatalogObjectId, trip: u64, day: u16, next: Option<u16>) -> Self {
        DayDoneScreen { today, ride, trip, day, next, pager: ContentPager::default() }
    }

    /// Tomorrow: the trip's key and the day. `None` on TRIP DONE.
    pub(crate) fn tomorrow(&self) -> Option<(u64, u16)> {
        Some((self.trip, self.next?))
    }

    #[cfg(test)]
    pub(crate) fn trip_done(&self) -> bool {
        self.next.is_none()
    }

    /// Only DAY N DONE has a page 2.
    pub fn tick_timers(&mut self, now_ms: u32) -> ScreenTick {
        if self.next.is_none() {
            return ScreenTick::idle();
        }
        self.pager.tick(now_ms)
    }

    pub fn handle(&mut self, g: Gesture, _cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Press | Gesture::Back => Transition::Home,
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        let trip = rx.trips.iter().find(|t| t.key == self.trip);
        let tomorrow = self.next.zip(trip).and_then(|(day, trip)| Tomorrow::of(trip, day, rx));
        match tomorrow {
            Some(tomorrow) if self.pager.on_second_page() => self.draw_tomorrow(cv, rx, &tomorrow),
            _ => self.draw_today(cv, rx, trip, tomorrow.as_ref()),
        }
        let geo = GuardedRowsGeometry::panel(rx.w, rx.h - 10 - OK_ROW_H, OK_ROW_H, 0);
        draw_guarded_rows(cv, &[MenuItem { label: rx.t(Msg::DayDoneOk), guard: false }], 0, 0.0, 0, geo);
    }

    fn draw_today(&self, cv: &mut impl Surface, rx: &Render, trip: Option<&TripSummary>, tomorrow: Option<&Tomorrow>) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        let units = rx.settings.units;
        let mut title: heapless::String<32> = heapless::String::new();
        if self.next.is_some() {
            let _ = write!(title, "{} {} {}", rx.t(Msg::DayDoneDay), self.day + 1, rx.t(Msg::DayDoneDone));
        } else {
            let _ = title.push_str(rx.t(Msg::DayDoneTripDone));
        }
        title_frame(cv, w, h, &title, "");

        let dist = distance_figure(units.dist(self.today.distance_m as f32 / 1000.0));
        let time = duration_hms(self.today.moving_s as f32);
        let climb = whole(units.elev(self.today.climb_m as f32));
        let rows: [(&str, &str, &str, Option<bool>); 3] = [
            (rx.t(Msg::RideControlDistance), &dist, dist_unit(units), None),
            (rx.t(Msg::RideControlRideTime), &time, "h", None),
            (rx.t(Msg::TileClimbed), &climb, units.elev_label(), Some(true)),
        ];
        for (i, (caption, value, unit, arrow)) in rows.into_iter().enumerate() {
            let y = ROWS_TOP + i as i32 * ROW_PITCH;
            ledger_row(cv, w, y, caption, value, unit, arrow);
            cv.hline(16, y + ROW_RULE, w - 32, RULE);
        }

        let at = |dy: i32| Point::new(TEXT_X, BLOCK_TOP + dy);
        match (self.next, trip) {
            (Some(_), _) => {
                let Some(t) = tomorrow else { return };
                cv.text(rx.t(Msg::DayDoneTomorrow), at(0), Font::Caption, TextAlign::Left, SUBTEXT);
                let region = rect(TEXT_X, BLOCK_TOP + NAME_DY, w - 2 * TEXT_X, Font::Label.line_height() as i32);
                let name = rx.marquee.fit(&t.route.name, w - 2 * TEXT_X, Font::Label, Some(region));
                cv.text(&name, at(NAME_DY), Font::Label, TextAlign::Left, INK);
                stats_line(cv, at(LINE_DY), units, t.distance_m, t.climb_m);
                if let Some(time) = t.est(rx) {
                    let mut est: heapless::String<24> = heapless::String::new();
                    let _ = write!(est, "~{time} h {}", rx.t(Msg::DayDoneEst));
                    cv.text(&est, at(LINE2_DY), Font::Caption, TextAlign::Left, SUBTEXT);
                }
            }
            (None, Some(trip)) => {
                let mut name: heapless::String<{ 2 * obc_formats::obcr::NAME_CAP }> = heapless::String::new();
                for c in trip.name.chars().flat_map(char::to_uppercase) {
                    let _ = name.push(c);
                }
                let caption = fit(&name, w - 2 * TEXT_X, Font::Caption);
                cv.text(&caption, at(0), Font::Caption, TextAlign::Left, SUBTEXT);
                let days = trip.stage_ids.len();
                let mut count: heapless::String<16> = heapless::String::new();
                let word = if days == 1 { Msg::RouteMenuDayOne } else { Msg::RouteMenuDays };
                let _ = write!(count, "{days} {}", rx.t(word));
                cv.text(&count, at(NAME_DY), Font::Label, TextAlign::Left, INK);
                let (distance_m, climb_m) = self.trip_totals(rx);
                stats_line(cv, at(LINE_DY), units, distance_m, Some(climb_m));
            }
            (None, None) => {}
        }
    }

    fn draw_tomorrow(&self, cv: &mut impl Surface, rx: &Render, t: &Tomorrow) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, rx.t(Msg::DayDoneTomorrow), "");
        let region = rect(TEXT_X, NAME_TOP, w - 2 * TEXT_X, Font::Label.line_height() as i32);
        let name = rx.marquee.fit(&t.route.name, w - 2 * TEXT_X, Font::Label, Some(region));
        cv.text(&name, Point::new(TEXT_X, NAME_TOP), Font::Label, TextAlign::Left, INK);

        match rx.day_profile {
            Some(profile) => {
                let band =
                    ElevationBand::whole_route(profile, rect(PROFILE_X, PROFILE_TOP, w - 2 * PROFILE_X, PROFILE_H));
                band.fill(cv, PARCHMENT_SHADE);
                band.stroke(cv, AMBER);
            }
            None => {
                let at = Point::new(w / 2, PROFILE_TOP + PROFILE_H / 2 - 9);
                cv.text(rx.t(Msg::RouteOverviewLoadingProfile), at, Font::Label, TextAlign::Center, SUBTEXT);
            }
        }

        let units = rx.settings.units;
        let dist = whole(units.dist(t.distance_m as f32 / 1000.0));
        let est = t.est(rx).unwrap_or_else(|| heapless::String::try_from("--").unwrap_or_default());
        let rows: [(&str, &str, &str); 2] =
            [(rx.t(Msg::RouteOverviewDistance), &dist, dist_unit(units)), (rx.t(Msg::RouteOverviewEstTime), &est, "h")];
        for (i, (caption, value, unit)) in rows.into_iter().enumerate() {
            let y = PAGE2_ROWS_TOP + i as i32 * ROW_PITCH;
            ledger_row(cv, w, y, caption, value, unit, None);
            cv.hline(16, y + ROW_RULE, w - 32, RULE);
        }
    }

    /// The trip's rides that the device still lists, with today's ride in place of its own row.
    fn trip_totals(&self, rx: &Render) -> (u32, u32) {
        rx.rides
            .iter()
            .filter(|r| r.id != self.ride && r.summary.trip.is_some_and(|t| t.key() == self.trip))
            .fold((self.today.distance_m, self.today.climb_m), |(d, c), r| {
                (d.saturating_add(r.summary.distance_m), c.saturating_add(u32::from(r.summary.climb_m)))
            })
    }
}

/// Tomorrow's day as [`load_day`](TripSummary::load_day) loads it, with the start card's length.
/// Its climb is the route header's for the day as it is. After the rest of the day before, it is
/// the host-filled day profile's, so it is `None` until that arrives.
struct Tomorrow<'a> {
    route: &'a RouteSummary,
    distance_m: u32,
    climb_m: Option<u32>,
}

impl<'a> Tomorrow<'a> {
    fn of(trip: &TripSummary, day: u16, rx: &Render<'a>) -> Option<Self> {
        let (_, index) = trip.days().find(|&(d, _)| d == day)?;
        let route = rx.routes.get(usize::from(index))?;
        let load = trip.load_day(day, trip.progress_in(rx.trip_progress), rx.day_join.as_ref());
        let climb_m = match load {
            DayLoad::AsIs => Some(route.climb_m),
            DayLoad::Rest { .. } => rx.day_profile.map(|p| p.ascent_to(1.0)),
        };
        Some(Tomorrow { route, distance_m: load.distance_km(route.distance_km) * 1000, climb_m })
    }

    /// The time estimate, `H:MM`, for the current bike type.
    fn est(&self, rx: &Render) -> Option<heapless::String<8>> {
        let climb_m = self.climb_m?;
        Some(duration_hms(rx.settings.bike_type.ride_time_s(self.distance_m, climb_m) as f32))
    }
}

/// The olive stats line: the distance, then the climb group once the climb is known.
fn stats_line(cv: &mut impl Surface, at: Point, units: Units, distance_m: u32, climb_m: Option<u32>) {
    let mut dist: heapless::String<16> = heapless::String::new();
    let _ = write!(dist, "{} {}", whole(units.dist(distance_m as f32 / 1000.0)), dist_unit(units));
    cv.text(&dist, at, Font::Caption, TextAlign::Left, palette::SUBTEXT);
    let Some(climb_m) = climb_m else { return };
    let mut climb: heapless::String<16> = heapless::String::new();
    let _ = write!(climb, "{} {}", whole(units.elev(climb_m as f32)), units.elev_label());
    let x = at.x + text_width(&dist, Font::Caption) as i32 + 8;
    climb_group(cv, x, at.y, &climb, palette::SUBTEXT);
}

fn whole(value: f32) -> heapless::String<8> {
    let mut s = heapless::String::new();
    let _ = write!(s, "{}", (value + 0.5) as u32);
    s
}

fn dist_unit(units: Units) -> &'static str {
    if units.is_imperial() {
        "mi"
    } else {
        "km"
    }
}
