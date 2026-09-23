//! The detail page of one recorded ride, the recorded twin of the route detail. Two pages flip on
//! a dwell. Page 1 is the date line and the ridden track on the device map over one distance
//! and ride-time line. Page 2 is the profile with its climb total over AVG SPEED and one row for each sensor the
//! ride recorded. Both pages end with the guarded Delete-ride row, which is hidden while a ride
//! records.
//!
//! The track and the profile come from the host: entry sets `Activity::viewed_ride`, the host
//! fills the resident ride-preview and ride-profile buffers, and Back or delete clears the key,
//! so both buffers invalidate on exit.

use core::fmt::Write;

use embedded_graphics::{draw_target::DrawTarget, prelude::Point, primitives::Rectangle};
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Canvas, Surface,
};

use super::vocab::band::draw_profile;
use super::vocab::chrome::{empty_state, title_chrome, title_frame};
use super::vocab::fmt::{duration_hms, write_date_weekday};
use super::vocab::marquee::fit;
use super::vocab::pager::ContentPager;
use super::vocab::rows::{detail_totals, draw_guarded_rows, ledger_row, GuardedRowsGeometry, MenuItem};
use super::vocab::track_map::{draw_track_map, Track};
use crate::input::Gesture;
use crate::ride::RideSummary;
use crate::screen::ScreenTick;
use crate::settings::{DateTime, Language, Units};
use crate::{t, Msg};

use super::{palette, Ctx, RenderFrame, Transition};

/// Page 1's date line under the title bar, and the map band under it.
const DATE_Y: i32 = 40;
const MAP_X: i32 = 5;
const MAP_TOP: i32 = 62;
const MAP_BOT: i32 = 238;
const TOTALS_Y: i32 = 244;

/// Page 2's profile band. The peak label sits in the headroom above it.
const PROFILE_TOP: i32 = 62;

/// The ledger: rows at a fixed pitch, each over a rule, stacked up from the bottom rule, so the
/// band above takes whatever height the rows leave.
const LEDGER_BOT: i32 = 264;
const ROW_PITCH: i32 = 36;
const ROW_RULE: i32 = 40;

/// The Delete-ride row sits where the route detail's Delete-route row sits.
const DELETE_ROW_H: i32 = 38;

/// The most ledger rows a page holds: AVG SPEED and three sensors.
const MAX_ROWS: usize = 4;

#[derive(Debug, Default)]
pub struct RideDetailScreen {
    ride: usize,
    pager: ContentPager,
}

impl RideDetailScreen {
    /// The caller must also set `Activity::viewed_ride`, which keys the host's track fill.
    pub fn new(ride: usize) -> Self {
        RideDetailScreen { ride, pager: ContentPager::default() }
    }

    pub fn tick_timers(&mut self, now_ms: u32) -> ScreenTick {
        self.pager.tick(now_ms)
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
        let title = fit(&ride.name, w - 28, Font::Body);
        let units = rx.settings.units;

        let lang = rx.settings.language;
        let mut values = Rows::new();
        let profile_page = self.pager.on_second_page();
        if profile_page {
            page_two_rows(ride, units, lang, &mut values);
        }
        let top = rows_top(values.len());
        if profile_page {
            title_frame(cv, w, h, &title, "");
            let area = rect(12, PROFILE_TOP, w - 24, top - 24 - PROFILE_TOP + 1);
            let loading = rx.t(Msg::RouteOverviewLoadingProfile);
            draw_profile(cv, rx.ride_profile, area, units, loading);
            let climb = number((units.elev(ride.climb_m as f32) + 0.5) as u32);
            let mut total = heapless::String::<16>::new();
            let _ = write!(total, "{climb} {}", units.elev_label());
            super::route_overview::climb_arrow(cv, 16, top - 20, true, INK);
            cv.text(&total, Point::new(32, top - 20), Font::Label, TextAlign::Left, INK);
        } else {
            let track = Track { points: rx.ride_preview, color: TRAIL, end_dot: false };
            draw_track_map(cv, rx, rect(MAP_X, MAP_TOP, w - 2 * MAP_X, MAP_BOT - MAP_TOP), track, None);
            title_chrome(cv, w, h, &title);
            cv.hline(MAP_X, MAP_BOT, w - 2 * MAP_X, RULE);
            let mut when: heapless::String<24> = heapless::String::new();
            let d = DateTime::from_unix(ride.start_time);
            write_date_weekday(&mut when, &d, lang);
            let _ = write!(when, " · {:02}:{:02}", d.hour, d.minute);
            cv.text(&when, Point::new(14, DATE_Y), Font::Label, TextAlign::Left, SUBTEXT);
            let dist = number(format_args!("{:.1}", units.dist(ride.distance_m as f32 / 1000.0)));
            let dist_unit = if units.is_imperial() { "mi" } else { "km" };
            let time = duration_hms(ride.moving_time_s as f32);
            detail_totals(cv, w, TOTALS_Y, &dist, dist_unit, &time);
        }

        for (i, (caption, value, unit, arrow)) in values.iter().enumerate() {
            let y = top + i as i32 * ROW_PITCH;
            ledger_row(cv, w, y, caption, value, unit, *arrow);
            cv.hline(16, y + ROW_RULE, w - 32, RULE);
        }

        // A plain row, like Delete route under an unselected cursor; the shaded base and the fill
        // draw only while a hold charges.
        if self.delete_enabled(rx.recording, rx.rides.len()) {
            let items = [MenuItem { label: rx.t(Msg::RideDetailDeleteRide), guard: true }];
            let selected = if rx.hold_progress > 0.0 { 0 } else { usize::MAX };
            draw_guarded_rows(cv, &items, selected, rx.hold_progress, WARNING, delete_row(w, h));
        }
    }
}

/// A ledger row: the caption, the value, its unit, and the climb arrow.
type Rows = heapless::Vec<(heapless::String<16>, heapless::String<8>, &'static str, Option<bool>), MAX_ROWS>;

/// The top of the first of `n` ledger rows.
fn rows_top(n: usize) -> i32 {
    LEDGER_BOT - ROW_RULE - (n as i32 - 1) * ROW_PITCH
}

fn delete_row(w: i32, h: i32) -> GuardedRowsGeometry {
    GuardedRowsGeometry::panel(w, h - 10 - DELETE_ROW_H, DELETE_ROW_H, 0)
}

fn number(v: impl core::fmt::Display) -> heapless::String<8> {
    let mut s = heapless::String::new();
    let _ = write!(s, "{v}");
    s
}

/// A caption from catalog fragments, such as `AVG ` and `HR`.
fn caption(parts: &[&str]) -> heapless::String<16> {
    let mut s = heapless::String::new();
    for part in parts {
        let _ = s.push_str(part);
    }
    s
}

/// The average speed, then one row for each sensor the ride recorded. The captions are the riding
/// tiles' own, because "AVG SPEED" and a value with its unit overrun the row.
fn page_two_rows(ride: &RideSummary, units: Units, lang: Language, out: &mut Rows) {
    let avg = t(Msg::TileAvg, lang);
    // The stored distance over the stored moving time, and `--` before any moving time.
    let speed = if ride.moving_time_s > 0 {
        let kmh = ride.distance_m as f32 / 1000.0 / (ride.moving_time_s as f32 / 3600.0);
        number(format_args!("{:.1}", units.speed(kmh)))
    } else {
        number("--")
    };
    let _ = out.push((caption(&[avg, units.speed_label()]), speed, "", None));
    if let Some(hr) = ride.avg_hr {
        let _ = out.push((caption(&[avg, t(Msg::TileHr, lang)]), number(hr), "bpm", None));
    }
    if let Some(cadence) = ride.avg_cadence {
        let _ = out.push((caption(&[avg, t(Msg::TileRpm, lang)]), number(cadence), "", None));
    }
    if let Some(power) = ride.avg_power {
        let _ = out.push((caption(&[avg, t(Msg::TilePwr, lang)]), number(power), "W", None));
    }
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
    fn the_pager_drives_this_screen_s_paired_pages() {
        use super::super::vocab::pager::PAGE_FLIP_MS;
        let mut scr = RideDetailScreen::new(0);
        assert!(!scr.tick_timers(0).changed, "the first poll only anchors the dwell");
        assert!(!scr.pager.on_second_page(), "entry shows the track shape + DISTANCE page");
        assert!(scr.tick_timers(PAGE_FLIP_MS).changed, "the dwell flips the page");
        assert!(scr.pager.on_second_page(), "now on the AVG + CLIMBED page");
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
