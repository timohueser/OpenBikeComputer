//! The detail page of one recorded ride, the recorded twin of the route detail. Up and Down turn
//! its pages, and the title bar counts them. Page 1 is the date line and the ridden track on the
//! device map over the distance, ride-time and average-speed totals. Page 2 is the elevation
//! profile in page 1's map band, with the climb and the descent on page 1's first totals line.
//! Page 3 shows only for a ride with sensor data: the HR and power graphs over the whole ride, then
//! AVG RPM and KJ. Every page ends with the guarded Delete-ride row, selected, which is hidden
//! while a ride records.
//!
//! The track, the profile, the descent and the sensor series come from the host: entry sets
//! `Activity::viewed_ride`, the host fills the resident ride-track buffers, and Back or delete
//! clears the key, so the buffers invalidate on exit.

use core::fmt::Write;

use embedded_graphics::{draw_target::DrawTarget, prelude::Point, primitives::Rectangle};
use obc_render::{
    rect,
    text::{text_width, Font, TextAlign},
    Canvas, Surface,
};

use super::route_overview::{climb_arrow, ARROW_W};
use super::vocab::band::draw_profile;
use super::vocab::chrome::{empty_state, title_chrome, title_frame};
use super::vocab::fmt::{duration_hms, write_date_weekday};
use super::vocab::rows::{detail_totals, draw_guarded_rows, ledger_row, GuardedRowsGeometry, MenuItem};
use super::vocab::tiles::{graph_field, GraphBlock};
use super::vocab::track_map::{draw_track_map, Track};
use crate::effort::Metric;
use crate::input::Gesture;
use crate::ride::RideSummary;
use crate::settings::DateTime;
use crate::Msg;

use super::{palette, Ctx, RenderFrame, Transition};

/// Page 1's date line under the title bar, and the map band under it. Page 2's profile takes the
/// same band.
const DATE_Y: i32 = 40;
const MAP_X: i32 = 5;
const MAP_TOP: i32 = 62;
const MAP_BOT: i32 = 212;
/// The two totals lines under the band.
const TOTALS_Y: i32 = 218;
const SPEED_Y: i32 = 244;

/// Page 3's graph fields, stacked under the title bar.
const GRAPH_TOP: i32 = 44;
const GRAPH_H: i32 = 58;
const GRAPH_GAP: i32 = 6;

/// The ledger rows under the graphs: rows at a fixed pitch, each over a rule.
const ROW_PITCH: i32 = 36;
const ROW_RULE: i32 = 40;

/// The Delete-ride row sits where the route detail's Delete-route row sits.
const DELETE_ROW_H: i32 = 38;

#[derive(Debug, Default)]
pub struct RideDetailScreen {
    ride: usize,
    /// The shown page, from 0. A remap to a ride with fewer pages can leave it past the end, so
    /// every read clamps it.
    page: usize,
}

impl RideDetailScreen {
    /// The caller must also set `Activity::viewed_ride`, which keys the host's track fill.
    pub fn new(ride: usize) -> Self {
        RideDetailScreen { ride, page: 0 }
    }

    /// Re-point the shown ride after a catalog rescan. A ride that vanished becomes an
    /// out-of-range index, which `draw` and `handle` show as the empty state.
    pub(crate) fn remap_rides(&mut self, remap: &dyn Fn(usize) -> Option<usize>) {
        self.ride = remap(self.ride).unwrap_or(usize::MAX);
    }

    /// The row is live only when the ride exists and no session records.
    fn delete_enabled(&self, recording: bool, len: usize) -> bool {
        len > 0 && self.ride < len && !recording
    }

    /// True while the delete row fills for a hold, which makes the app repaint the charging hold.
    pub(crate) fn selection_is_guarded(&self, recording: bool, rides_len: usize) -> bool {
        self.delete_enabled(recording, rides_len)
    }

    /// The Delete row, the one rectangle a hold step repaints. It lies under the map band, so a
    /// clipped repaint of it renders no map.
    pub(crate) fn hold_fill_region(w: i32, h: i32) -> Rectangle {
        delete_row(w, h).row(0)
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Step(n) => {
                if let Some(entry) = cx.rides.get(self.ride) {
                    let pages = page_count(&entry.summary) as i32;
                    self.page = (self.page(&entry.summary) as i32 + n).rem_euclid(pages) as usize;
                }
                Transition::None
            }
            // The completed hold is the confirmation. The host resolves the index to the
            // catalog object and removes it; the list refreshes on its rescan.
            Gesture::Hold if self.delete_enabled(cx.recorder.recording(), cx.rides.len()) => {
                cx.activity.request_ride_delete(self.ride.min(cx.rides.len() - 1));
                cx.activity.viewed_ride = None;
                Transition::Pop
            }
            Gesture::Back => {
                cx.activity.viewed_ride = None;
                Transition::Pop
            }
            _ => Transition::None,
        }
    }

    fn page(&self, ride: &RideSummary) -> usize {
        self.page.min(page_count(ride) - 1)
    }

    /// The title bar's page counter, such as `1/3`.
    fn counter(&self, ride: &RideSummary) -> heapless::String<8> {
        let mut s = heapless::String::new();
        let _ = write!(s, "{}/{}", self.page(ride) + 1, page_count(ride));
        s
    }

    pub fn draw<D, F>(&self, cv: &mut Canvas<D, F>, rx: &mut RenderFrame<'_, '_>)
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        let Some(ride) = rx.rides.get(self.ride).map(|entry| &entry.summary) else {
            title_frame(cv, w, h, rx.t(Msg::RideStartTitle), "");
            empty_state(cv, w, h, rx.t(Msg::RidesNoRides), rx.t(Msg::RidesNoRidesSub));
            return;
        };
        // The title does not scroll: a scroll step on a map base would render the map again.
        let counter = self.counter(ride);
        match self.page(ride) {
            0 => {
                totals_page(cv, rx, ride);
                title_chrome(cv, w, h, &ride.name, &counter);
            }
            1 => {
                title_frame(cv, w, h, &ride.name, &counter);
                elevation_page(cv, rx, ride);
            }
            _ => {
                title_frame(cv, w, h, &ride.name, &counter);
                sensor_page(cv, rx, ride);
            }
        }

        if self.delete_enabled(rx.recording, rx.rides.len()) {
            let items = [MenuItem { label: rx.t(Msg::RideDetailDeleteRide), guard: true }];
            draw_guarded_rows(cv, &items, 0, rx.hold_progress, WARNING, delete_row(w, h));
        }
    }
}

/// Two pages, and a third for a ride with any sensor data.
fn page_count(ride: &RideSummary) -> usize {
    if ride.avg_hr.is_some() || ride.avg_cadence.is_some() || ride.avg_power.is_some() {
        3
    } else {
        2
    }
}

/// The date, the map with the track, then distance and time over the average speed.
fn totals_page<D, F>(cv: &mut Canvas<D, F>, rx: &mut RenderFrame<'_, '_>, ride: &RideSummary)
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    use palette::*;
    let w = rx.w;
    let units = rx.settings.units;
    let track = Track { points: rx.ride_preview, color: TRAIL, end_dot: false };
    draw_track_map(cv, rx, rect(MAP_X, MAP_TOP, w - 2 * MAP_X, MAP_BOT - MAP_TOP), track, None);
    cv.hline(MAP_X, MAP_BOT, w - 2 * MAP_X, RULE);
    let mut when: heapless::String<24> = heapless::String::new();
    let d = DateTime::from_unix(ride.start_time);
    write_date_weekday(&mut when, &d, rx.settings.language);
    let _ = write!(when, " · {:02}:{:02}", d.hour, d.minute);
    cv.text(&when, Point::new(14, DATE_Y), Font::Label, TextAlign::Left, SUBTEXT);
    let dist = number(format_args!("{:.1}", units.dist(ride.distance_m as f32 / 1000.0)));
    let (dist_unit, speed_unit) = if units.is_imperial() { ("mi", "mph") } else { ("km", "km/h") };
    detail_totals(cv, w, TOTALS_Y, &dist, dist_unit, &duration_hms(ride.moving_time_s as f32));
    // The stored distance over the stored moving time, and `--` before any moving time.
    let mut speed = heapless::String::<16>::new();
    if ride.moving_time_s > 0 {
        let kmh = ride.distance_m as f32 / 1000.0 / (ride.moving_time_s as f32 / 3600.0);
        let _ = write!(speed, "{:.1} {speed_unit}", units.speed(kmh));
    } else {
        let _ = write!(speed, "-- {speed_unit}");
    }
    cv.text(&speed, Point::new(16, SPEED_Y), Font::Label, TextAlign::Left, INK);
}

/// The profile in page 1's map band, with the climb and the descent on page 1's first totals line.
/// The descent comes from the opened ride, so it shows `--` until the fill lands.
fn elevation_page(cv: &mut impl Surface, rx: &RenderFrame<'_, '_>, ride: &RideSummary) {
    use palette::*;
    let w = rx.w;
    let units = rx.settings.units;
    draw_profile(cv, rx.ride_profile, map_band(w), units, rx.t(Msg::RouteOverviewLoadingProfile));
    let elevation = |m: Option<u16>| {
        let mut s = heapless::String::<12>::new();
        let _ = match m {
            Some(m) => write!(s, "{} {}", (units.elev(m as f32) + 0.5) as u32, units.elev_label()),
            None => write!(s, "-- {}", units.elev_label()),
        };
        s
    };
    let up = elevation(Some(ride.climb_m));
    let down = elevation(rx.ride_facts.map(|f| f.descent_m));
    climb_arrow(cv, 16, TOTALS_Y, true, INK);
    cv.text(&up, Point::new(16 + ARROW_W + 4, TOTALS_Y), Font::Label, TextAlign::Left, INK);
    cv.text(&down, Point::new(w - 16, TOTALS_Y), Font::Label, TextAlign::Right, INK);
    climb_arrow(cv, w - 16 - text_width(&down, Font::Label) as i32 - ARROW_W - 4, TOTALS_Y, false, INK);
}

/// The profile's band: page 1's map band, inset like the route detail's profile.
fn map_band(w: i32) -> Rectangle {
    rect(12, MAP_TOP, w - 24, MAP_BOT - MAP_TOP)
}

/// The HR and power graphs over the whole ride, then the AVG RPM and KJ rows. Each shows only when
/// the ride recorded it.
fn sensor_page(cv: &mut impl Surface, rx: &RenderFrame<'_, '_>, ride: &RideSummary) {
    use palette::*;
    let w = rx.w;
    let limits = rx.settings.effort_limits();
    let avg = rx.t(Msg::TileAvg);
    let facts = rx.ride_facts;
    let mut y = GRAPH_TOP;
    let mut area = || {
        let area = rect(MAP_X, y, w - 2 * MAP_X, GRAPH_H);
        y += GRAPH_H + GRAPH_GAP;
        area
    };
    if let Some(hr) = ride.avg_hr {
        let series = facts.map_or(&[][..], |f| f.hr()).iter().map(|&v| u16::from(v));
        let caption = caption(avg, rx.t(Msg::TileHr));
        ride_graph(cv, area(), &caption, hr.into(), series, Metric::Hr, limits.of(Metric::Hr));
    }
    if let Some(power) = ride.avg_power {
        let series = facts.map_or(&[][..], |f| f.power()).iter().copied();
        let caption = caption(avg, rx.t(Msg::TilePwrShort));
        ride_graph(cv, area(), &caption, power, series, Metric::Power, limits.of(Metric::Power));
    }
    let mut row = y + 2;
    let mut ledger = |caption: &str, value: heapless::String<8>| {
        ledger_row(cv, w, row, caption, &value, "", None);
        cv.hline(16, row + ROW_RULE, w - 32, RULE);
        row += ROW_PITCH;
    };
    if let Some(cadence) = ride.avg_cadence {
        ledger(&caption(avg, rx.t(Msg::TileRpm)), number(cadence));
    }
    if let Some(kj) = ride.energy_kj {
        ledger(rx.t(Msg::TileKj), number(kj));
    }
}

/// One whole-ride graph field, tinted in the zone of the ride's average.
fn ride_graph(
    cv: &mut impl Surface,
    area: Rectangle,
    caption: &str,
    avg: u16,
    series: impl ExactSizeIterator<Item = u16> + Clone,
    m: Metric,
    limit: Option<u32>,
) {
    let zone = limit.map(|l| m.zone_of(avg.into(), l));
    graph_field(cv, area, GraphBlock::Ride, caption, &number(avg), zone, series, m, limit);
}

fn delete_row(w: i32, h: i32) -> GuardedRowsGeometry {
    GuardedRowsGeometry::panel(w, h - 10 - DELETE_ROW_H, DELETE_ROW_H, 0)
}

fn number(v: impl core::fmt::Display) -> heapless::String<8> {
    let mut s = heapless::String::new();
    let _ = write!(s, "{v}");
    s
}

/// A caption from two catalog fragments, such as `AVG ` and `HR`.
fn caption(avg: &str, metric: &str) -> heapless::String<16> {
    let mut s = heapless::String::new();
    let _ = s.push_str(avg);
    let _ = s.push_str(metric);
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Activity;
    use crate::activity::Mode;
    use crate::ride::{RideEntry, RideSummary};
    use crate::screen::test_ctx;
    use crate::{AppState, Settings};

    fn summary(name: &str) -> RideEntry {
        RideEntry {
            id: 1,
            summary: RideSummary {
                name: heapless::String::try_from(name).unwrap(),
                start_time: 1_720_000_000,
                distance_m: 42_500,
                moving_time_s: 2 * 3600 + 31 * 60,
                climb_m: 640,
                ..Default::default()
            },
        }
    }

    fn run(
        scr: &mut RideDetailScreen,
        act: &mut Activity,
        rec: &mut crate::RecorderMachine,
        rides: &[RideEntry],
        g: Gesture,
    ) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut settings = Settings::default();
        let mut cx = Ctx { rides, recorder: rec, ..test_ctx(&mut st, act, &mut settings) };
        scr.handle(g, &mut cx)
    }

    #[test]
    fn hold_deletes_and_returns_to_the_list() {
        let mut rec = crate::RecorderMachine::new();
        let rides = [summary("A"), summary("B")];
        let mut act = Activity::new(Mode::Idle);
        act.viewed_ride = Some(1);
        let mut scr = RideDetailScreen::new(1);
        let t = run(&mut scr, &mut act, &mut rec, &rides, Gesture::Hold);
        assert!(matches!(t, Transition::Pop), "the delete returns to the list");
        assert_eq!(act.take_ride_delete(), Some(1), "the shown ride's index is requested");
        assert_eq!(act.viewed_ride, None, "leaving the page invalidates the profile buffer");
    }

    #[test]
    fn hold_is_a_no_op_while_recording() {
        let mut rec = crate::RecorderMachine::new();
        let rides = [summary("A")];
        let mut act = Activity::new(Mode::Riding);
        rec.test_open();
        act.viewed_ride = Some(0);
        let mut scr = RideDetailScreen::new(0);
        assert!(!scr.selection_is_guarded(rec.recording(), rides.len()), "the row is hidden while recording");
        let t = run(&mut scr, &mut act, &mut rec, &rides, Gesture::Hold);
        assert!(matches!(t, Transition::None), "a hold while recording stays on the page");
        assert_eq!(act.take_ride_delete(), None, "and records nothing");
        assert_eq!(act.viewed_ride, Some(0), "the profile stays keyed to the open page");

        rec.test_close();
        assert!(scr.selection_is_guarded(rec.recording(), rides.len()), "ending the ride re-arms the row");
    }

    #[test]
    fn back_pops_and_invalidates_the_profile_key() {
        let mut rec = crate::RecorderMachine::new();
        let rides = [summary("A")];
        let mut act = Activity::new(Mode::Idle);
        act.viewed_ride = Some(0);
        let mut scr = RideDetailScreen::new(0);
        let t = run(&mut scr, &mut act, &mut rec, &rides, Gesture::Back);
        assert!(matches!(t, Transition::Pop));
        assert_eq!(act.viewed_ride, None);
    }

    #[test]
    fn up_and_down_turn_the_pages_and_the_title_counts_them() {
        let mut rec = crate::RecorderMachine::new();
        let mut act = Activity::new(Mode::Idle);
        let plain = [summary("A")];
        let mut hr = summary("B");
        hr.summary.avg_hr = Some(128);
        let hr = [hr];
        let mut cadence = summary("C");
        cadence.summary.avg_cadence = Some(85);

        let mut scr = RideDetailScreen::new(0);
        let mut turn = |rides: &[RideEntry], n| {
            let t = run(&mut scr, &mut act, &mut rec, rides, Gesture::Step(n));
            assert!(matches!(t, Transition::None), "a page turn stays on the screen");
            scr.counter(&rides[0].summary)
        };
        assert_eq!(turn(&plain, 0), "1/2", "a ride without sensor data has two pages");
        assert_eq!(turn(&plain, 1), "2/2");
        assert_eq!(turn(&plain, 1), "1/2", "Down past the last page comes back to the first");
        assert_eq!(turn(&plain, -1), "2/2", "and Up before the first goes to the last");
        assert_eq!(turn(&hr, 1), "3/3", "any sensor adds the third page");
        assert_eq!(turn(&hr, -2), "1/3");
        assert_eq!(page_count(&cadence.summary), 3, "cadence alone also counts");

        let mut scr = RideDetailScreen { ride: 0, page: 2 };
        assert_eq!(scr.counter(&plain[0].summary), "2/2", "a page past a remapped ride's end clamps");
        run(&mut scr, &mut act, &mut rec, &plain, Gesture::Step(1));
        assert_eq!(scr.counter(&plain[0].summary), "1/2", "and turns on from the clamped page");
    }

    #[test]
    fn vanished_ride_has_no_delete() {
        let mut rec = crate::RecorderMachine::new();
        let mut act = Activity::new(Mode::Idle);
        let mut scr = RideDetailScreen::new(0);
        scr.remap_rides(&|_| None);
        assert!(!scr.selection_is_guarded(rec.recording(), 1), "an out-of-range subject arms nothing");
        let t = run(&mut scr, &mut act, &mut rec, &[summary("A")], Gesture::Hold);
        assert!(matches!(t, Transition::None));
        assert_eq!(act.take_ride_delete(), None);
    }
}
