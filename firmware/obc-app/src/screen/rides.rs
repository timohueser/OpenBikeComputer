//! The list of stored rides, reached from the Rides station of the main menu. The rides of one trip
//! group into a trip folder, placed at its newest ride; a folder press pushes the trip's rides, a
//! second list scoped to that trip. A ride press opens the ride detail, which also holds the delete.
//! A synced ride has a tick at the right of its name.
//!
//! The rides come from the app's ride catalog, newest first, which the host fills from the flat
//! catalog. Folders form from those rides only.

use core::fmt::Write;

use embedded_graphics::prelude::Point;
use obc_render::{
    text::{text_width, Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::ride::{RideEntry, RideTrip};
use crate::settings::{DateTime, Units};
use crate::{CatalogObjectId, Msg, UI_RIDES_CAP};

use super::vocab::chrome::{empty_state, row_check, title_frame, ROW_CHECK_HALF};
use super::vocab::fmt::{write_date_short, write_date_weekday, write_distance_spaced};
use super::vocab::list;
use super::vocab::marquee::fit;
use super::vocab::two_line::{self, LINE2, LINE2_FONT};
use super::{palette, Ctx, Render, RideDetailScreen, Screen, Transition};

/// The synced tick's clearance from the right edge of the row box, clear of the rounded corner, and
/// the name's clearance from the tick. Both are tight, so "Day 1 Andermatt" fits whole.
const MARK_RIGHT_GAP: i32 = 10;
const MARK_NAME_GAP: i32 = 4;

/// The trip header: the totals line under the title bar, and the hairline between it and the list.
const HEADER_Y: i32 = 40;
const HEADER_RULE: i32 = 66;

/// What this list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// Trip folders and loose rides.
    TopLevel,
    /// The rides of one trip.
    Trip { key: u64 },
}

/// One row: a trip folder by the catalog index of its newest ride, or a ride by catalog index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Folder(usize),
    Ride(usize),
}

/// The identity of the highlighted row, so the highlight follows it across a catalog rescan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelId {
    Trip(u64),
    Ride(CatalogObjectId),
}

type Rows = heapless::Vec<Row, UI_RIDES_CAP>;

#[derive(Debug)]
pub struct RidesScreen {
    selected: usize,
    sel_id: Option<SelId>,
    scope: Scope,
}

impl Default for RidesScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl RidesScreen {
    pub fn new() -> Self {
        RidesScreen { selected: 0, sel_id: None, scope: Scope::TopLevel }
    }

    fn trip(key: u64) -> Self {
        RidesScreen { selected: 0, sel_id: None, scope: Scope::Trip { key } }
    }

    /// Re-point the highlight after a catalog rescan: find the pinned identity in the rebuilt list,
    /// or clamp near the old position when it vanished.
    pub(crate) fn remap_rides(&mut self, rides: &[RideEntry]) {
        let rows = rows(rides, self.scope);
        self.selected = self
            .sel_id
            .and_then(|id| rows.iter().position(|&r| identity(rides, r) == id))
            .unwrap_or_else(|| self.selected.min(rows.len().saturating_sub(1)));
        self.pin(rides, &rows);
    }

    fn pin(&mut self, rides: &[RideEntry], rows: &[Row]) {
        self.sel_id = rows.get(self.selected).map(|&r| identity(rides, r));
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        let rows = rows(cx.rides, self.scope);
        let len = rows.len();
        self.selected = self.selected.min(len.saturating_sub(1));
        let t = match g {
            Gesture::Step(n) => list::on_step(&mut self.selected, n, len),
            Gesture::Press if len > 0 => match rows[self.selected] {
                Row::Folder(i) => match cx.rides[i].summary.trip {
                    Some(trip) => Transition::Push(Screen::Rides(RidesScreen::trip(trip.key()))),
                    None => Transition::None,
                },
                // `viewed_ride` keys the host's track fill for the detail page.
                Row::Ride(i) => {
                    cx.activity.viewed_ride = Some(i);
                    Transition::Push(Screen::RideDetail(RideDetailScreen::new(i)))
                }
            },
            Gesture::Back => Transition::Pop,
            _ => Transition::None,
        };
        self.pin(cx.rides, &rows);
        t
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        let rides = rx.rides;
        let rows = rows(rides, self.scope);
        let total = rows.len();
        let units = rx.settings.units;
        let lang = rx.settings.language;

        let mut geo = two_line::geometry(w, h);
        if let Scope::Trip { .. } = self.scope {
            geo.top = HEADER_RULE + 4;
            geo.visible = ((h - geo.top - 6) / geo.row_h).max(1) as usize;
        }
        let pos = if total == 0 { 0 } else { self.selected.min(total - 1) + 1 };
        let counter = list::counter(pos, total, geo.visible);
        match self.scope {
            Scope::TopLevel => title_frame(cv, w, h, rx.t(Msg::RidesTitle), &counter),
            Scope::Trip { key } => {
                let trip = Folder::of(rides, rx.ride_trips, key);
                // The title and the counter both keep 14 px from the bar's ends, and 8 px apart.
                let counter_w = if counter.is_empty() { 0 } else { text_width(&counter, Font::Label) as i32 + 8 };
                let title = fit(trip.as_ref().map_or("", |t| t.name), w - 28 - counter_w, Font::Body);
                title_frame(cv, w, h, &title, &counter);
                if let Some(trip) = trip {
                    draw_header(cv, w, &trip, units);
                }
            }
        }

        if total == 0 {
            empty_state(cv, w, h, rx.t(Msg::RidesNoRides), rx.t(Msg::RidesNoRidesSub));
            return;
        }

        let sel = self.selected.min(total - 1);
        let first = list::window_start(sel, geo.visible, total) as i32;
        list::draw_rows(cv, geo, total, sel, first, |cv, row| {
            let x = two_line::text_x(&row);
            let mut line2: heapless::String<48> = heapless::String::new();
            match rows[row.index] {
                Row::Folder(i) => {
                    let Some(folder) = rides[i].summary.trip.and_then(|t| Folder::of(rides, rx.ride_trips, t.key()))
                    else {
                        return;
                    };
                    two_line::name_line(cv, &row, &rx.marquee, folder.name, (x, two_line::name_right(&row)), INK);
                    let days = rx.t(if folder.day_count == 1 { Msg::RouteMenuDayOne } else { Msg::RidesDays });
                    let _ = write!(line2, "{} {} {} {days}", folder.days_ridden, rx.t(Msg::RidesOf), folder.day_count);
                    push_item(&mut line2, &whole_distance(folder.distance_m, units), two_line::line2_right(&row) - x);
                }
                Row::Ride(i) => {
                    let ride = &rides[i].summary;
                    // The name budget keeps the tick's slot, drawn or not, so the cut does not move
                    // when a ride syncs.
                    let mark = two_line::right_mark(&row, ROW_CHECK_HALF, MARK_RIGHT_GAP);
                    let mut name: heapless::String<64> = heapless::String::new();
                    let _ = name.push_str(&ride.name);
                    if let Scope::Trip { .. } = self.scope {
                        let n = day_ordinal(rides, i);
                        if n > 1 {
                            let _ = write!(name, " ({n})");
                        }
                        write_date_weekday(&mut line2, &DateTime::from_unix(ride.start_time), lang);
                    } else {
                        write_date_short(&mut line2, ride.start_time, lang);
                    }
                    two_line::name_line(
                        cv,
                        &row,
                        &rx.marquee,
                        &name,
                        (x, mark.x - ROW_CHECK_HALF - MARK_NAME_GAP),
                        INK,
                    );
                    if ride.synced {
                        row_check(cv, mark, if row.selected { INK } else { LINE2 });
                    }
                    let mut dist: heapless::String<12> = heapless::String::new();
                    write_distance_spaced(&mut dist, ride.distance_m, units);
                    push_item(&mut line2, &dist, two_line::line2_right(&row) - x);
                }
            }
            cv.text(&line2, Point::new(x, two_line::line2_y(&row)), LINE2_FONT, TextAlign::Left, LINE2);
        });
    }
}

/// Build the rows of `scope` from the newest-first catalog.
fn rows(rides: &[RideEntry], scope: Scope) -> Rows {
    let mut out = Rows::new();
    for (i, ride) in rides.iter().enumerate() {
        let key = ride.summary.trip.map(|t| t.key());
        let row = match (scope, key) {
            (Scope::TopLevel, None) => Row::Ride(i),
            (Scope::TopLevel, Some(key)) if Folder::newest(rides, key) == Some(i) => Row::Folder(i),
            (Scope::Trip { key: scoped }, Some(key)) if key == scoped => Row::Ride(i),
            _ => continue,
        };
        let _ = out.push(row);
    }
    out
}

fn identity(rides: &[RideEntry], row: Row) -> SelId {
    match row {
        Row::Folder(i) => SelId::Trip(rides[i].summary.trip.map_or(0, |t| t.key())),
        Row::Ride(i) => SelId::Ride(rides[i].id),
    }
}

/// Which ride of its trip day the ride at `i` is, in the order of start: 1 for the first.
fn day_ordinal(rides: &[RideEntry], i: usize) -> usize {
    let (ride, id) = (&rides[i].summary, rides[i].id);
    let Some(trip) = ride.trip else { return 1 };
    let earlier = rides
        .iter()
        .filter(|r| r.summary.trip.is_some_and(|t| t.key() == trip.key() && t.day_index() == trip.day_index()))
        .filter(|r| (r.summary.start_time, r.id) < (ride.start_time, id))
        .count();
    earlier + 1
}

/// The facts of one trip folder, over the rides the catalog holds.
struct Folder<'a> {
    /// The trip's name, or the newest ride's name when the trip table has none.
    name: &'a str,
    days_ridden: u32,
    /// The day count the newest ride stored.
    day_count: u8,
    distance_m: u32,
    climb_m: u32,
}

impl<'a> Folder<'a> {
    /// The catalog index of the newest ride of trip `key`.
    fn newest(rides: &[RideEntry], key: u64) -> Option<usize> {
        rides.iter().position(|r| r.summary.trip.is_some_and(|t| t.key() == key))
    }

    fn of(rides: &'a [RideEntry], trips: &'a [RideTrip], key: u64) -> Option<Folder<'a>> {
        let newest = &rides[Self::newest(rides, key)?].summary;
        let name = trips.iter().find(|t| t.key == key).map_or(newest.name.as_str(), |t| t.name.as_str());
        let mut folder = Folder {
            name,
            days_ridden: 0,
            day_count: newest.trip.map_or(0, |t| t.day_count()),
            distance_m: 0,
            climb_m: 0,
        };
        let mut days = [0u64; 4];
        for ride in rides.iter().map(|r| &r.summary) {
            let Some(trip) = ride.trip.filter(|t| t.key() == key) else { continue };
            let day = usize::from(trip.day_index());
            days[day / 64] |= 1 << (day % 64);
            folder.distance_m = folder.distance_m.saturating_add(ride.distance_m);
            folder.climb_m += u32::from(ride.climb_m);
        }
        folder.days_ridden = days.iter().map(|d| d.count_ones()).sum();
        Some(folder)
    }
}

/// Append ` · <item>` to a row's line 2 when the whole run fits `budget_px`. Otherwise the item
/// drops whole and the text before it stays.
fn push_item(s: &mut heapless::String<48>, item: &str, budget_px: i32) {
    let chars = s.chars().count() + 3 + item.chars().count();
    if chars as i32 * LINE2_FONT.char_width() as i32 <= budget_px {
        let _ = write!(s, " · {item}");
    }
}

/// A trip's distance in whole kilometres or miles, as the route rows show a route's.
fn whole_distance(m: u32, units: Units) -> heapless::String<12> {
    let mut s = heapless::String::new();
    let unit = if units.is_imperial() { "mi" } else { "km" };
    let _ = write!(s, "{} {unit}", (units.dist(m as f32 / 1000.0) + 0.5) as u32);
    s
}

/// The trip's totals under the title bar, then the hairline over the list.
fn draw_header(cv: &mut impl Surface, w: i32, trip: &Folder, units: Units) {
    use palette::*;
    let dist = whole_distance(trip.distance_m, units);
    let x = 14;
    cv.text(&dist, Point::new(x, HEADER_Y), Font::Label, TextAlign::Left, SUBTEXT);
    let arrow_x = x + text_width(&dist, Font::Label) as i32 + 8;
    super::route_overview::climb_arrow(cv, arrow_x, HEADER_Y, true, SUBTEXT);
    let mut climb: heapless::String<12> = heapless::String::new();
    let _ = write!(climb, "{} {}", (units.elev(trip.climb_m as f32) + 0.5) as u32, units.elev_label());
    let climb_x = arrow_x + super::route_overview::ARROW_W + 2;
    cv.text(&climb, Point::new(climb_x, HEADER_Y), Font::Label, TextAlign::Left, SUBTEXT);
    cv.hline(5, HEADER_RULE, w - 10, RULE);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::{Activity, Mode};
    use crate::ride::RideSummary;
    use crate::screen::test_ctx;
    use crate::{AppState, Settings};
    use obc_formats::ride::TripRef;

    const ALPS: u64 = 0xA1;

    fn ride(id: u64, name: &str, start_time: u32, trip: Option<TripRef>) -> RideEntry {
        RideEntry {
            id,
            summary: RideSummary {
                name: heapless::String::try_from(name).unwrap(),
                start_time,
                distance_m: 40_000,
                climb_m: 500,
                trip,
                ..Default::default()
            },
        }
    }

    /// Five rides, newest first: a loose ride, the second ride of day 2 (index 1), a loose ride,
    /// the first ride of day 2, and day 1. The trip has three days.
    fn five() -> [RideEntry; 5] {
        let day = |d| TripRef::new(ALPS, d, 3);
        [
            ride(50, "Evening loop", 5_000, None),
            ride(40, "Day 2 Ulrichen", 4_000, day(1)),
            ride(30, "Commute", 3_000, None),
            ride(20, "Day 2 Ulrichen", 2_000, day(1)),
            ride(10, "Day 1 Andermatt", 1_000, day(0)),
        ]
    }

    fn run(scr: &mut RidesScreen, act: &mut Activity, rides: &[RideEntry], g: Gesture) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut settings = Settings::default();
        let mut cx = Ctx { rides, ..test_ctx(&mut st, act, &mut settings) };
        scr.handle(g, &mut cx)
    }

    #[test]
    fn a_trip_groups_into_one_folder_at_its_newest_ride() {
        let rides = five();
        assert_eq!(
            rows(&rides, Scope::TopLevel).as_slice(),
            [Row::Ride(0), Row::Folder(1), Row::Ride(2)],
            "the folder sorts by its newest ride among the loose rides"
        );
        assert_eq!(rows(&rides, Scope::Trip { key: ALPS }).as_slice(), [Row::Ride(1), Row::Ride(3), Row::Ride(4)]);

        let trips = [RideTrip { key: ALPS, name: heapless::String::try_from("Alps traverse").unwrap() }];
        let folder = Folder::of(&rides, &trips, ALPS).unwrap();
        assert_eq!((folder.name, folder.days_ridden, folder.day_count), ("Alps traverse", 2, 3), "2 of 3 days");
        assert_eq!((folder.distance_m, folder.climb_m), (120_000, 1_500), "the sums of the three rides");
        assert_eq!(Folder::of(&rides, &[], ALPS).unwrap().name, "Day 2 Ulrichen", "no trip name: the newest ride's");
    }

    #[test]
    fn a_second_ride_of_a_day_takes_its_order_of_start() {
        let rides = five();
        let ordinals: heapless::Vec<usize, 5> = (0..rides.len()).map(|i| day_ordinal(&rides, i)).collect();
        assert_eq!(ordinals.as_slice(), [1, 2, 1, 1, 1], "only the later ride of day 2 is \"(2)\"");
    }

    #[test]
    fn press_opens_a_folder_then_the_ride_detail() {
        let rides = five();
        let mut act = Activity::new(Mode::Idle);
        let mut top = RidesScreen::new();
        run(&mut top, &mut act, &rides, Gesture::Step(1));
        let Transition::Push(Screen::Rides(mut trip)) = run(&mut top, &mut act, &rides, Gesture::Press) else {
            panic!("a folder press pushes the trip's rides");
        };
        assert_eq!(act.viewed_ride, None, "a folder opens no detail");
        run(&mut trip, &mut act, &rides, Gesture::Step(1));
        let t = run(&mut trip, &mut act, &rides, Gesture::Press);
        assert!(matches!(t, Transition::Push(Screen::RideDetail(_))));
        assert_eq!(act.viewed_ride, Some(3), "the second trip row is catalog ride 3");
    }

    #[test]
    fn press_on_an_empty_catalog_is_a_noop() {
        let mut act = Activity::new(Mode::Idle);
        let mut scr = RidesScreen::new();
        let t = run(&mut scr, &mut act, &[], Gesture::Press);
        assert!(matches!(t, Transition::None));
        assert_eq!(act.viewed_ride, None);
    }

    #[test]
    fn remap_follows_identity_and_clamps_on_vanish() {
        let rides = five();
        let mut act = Activity::new(Mode::Idle);
        let mut scr = RidesScreen::new();
        run(&mut scr, &mut act, &rides, Gesture::Step(2)); // "Commute"
                                                           // The newest ride vanished: "Commute" moves up one row.
        scr.remap_rides(&rides[1..]);
        assert_eq!(scr.selected, 1);
        // "Commute" vanished too: the highlight clamps to the last row.
        scr.remap_rides(&rides[3..]);
        assert_eq!(scr.selected, 0, "a vanished highlight clamps to the last row");
    }

    #[test]
    fn line_2_drops_the_distance_whole() {
        let budget = 240 - 2 * two_line::SIDE_INSET - 4 - two_line::NAME_INSET;
        let line = |start: &str, item: &str| {
            let mut s: heapless::String<48> = heapless::String::try_from(start).unwrap();
            push_item(&mut s, item, budget);
            s
        };
        let folder = whole_distance(156_400, Units::Metric);
        assert_eq!(line("2 of 3 days", &folder).as_str(), "2 of 3 days · 156 km", "the folder line fits the row");
        assert_eq!(line("TUE 30 SEP", "74.3 km").as_str(), "TUE 30 SEP · 74.3 km");
        assert_eq!(line("MER 30 JUIL", "74.3 km").as_str(), "MER 30 JUIL", "the date never yields");
    }
}
