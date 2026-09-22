//! The detail page of one recorded ride. A pager flips the media band together with its stats:
//! the track shape over DISTANCE + RIDE TIME, then the elevation band over AVG + CLIMBED. A
//! guarded Delete ride row sits at the bottom, hidden while a ride is being recorded.
//!
//! The shape and the profile come from the host: entry sets `Activity::viewed_ride`, the host
//! fills the resident ride-preview and ride-profile buffers, and Back or delete clears the key,
//! so both buffers invalidate on exit.

use core::fmt::Write;

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use super::vocab::band::{ElevationBand, PeakLabel};
use super::vocab::chrome::{empty_state, title_frame, LIST_TOP};
use super::vocab::fmt::{date_iso, duration_hms};
use super::vocab::pager::ContentPager;
use super::vocab::rows::{draw_guarded_rows, ledger_row, GuardedRowsGeometry, MenuItem};
use crate::input::Gesture;
use crate::screen::ScreenTick;
use crate::Msg;

use super::{palette, Ctx, Render, Transition};

/// The media band slot. Both pages draw in it, so nothing moves on the flip.
const BAND_TOP: i32 = 96;
const BAND_BOT: i32 = 178;
const SIDE_MARGIN: i32 = 12;

/// The two stat rows of a page, between the band and the delete row.
const ROWS_TOP: i32 = 186;
const ROW_PITCH: i32 = 42;

/// The height of the guarded Delete-ride row.
const ROW_H: i32 = 34;

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

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        let Some(ride) = rx.rides.get(self.ride).map(|entry| &entry.summary) else {
            title_frame(cv, w, h, rx.t(Msg::RideStartTitle), "");
            empty_state(cv, w, h, rx.t(Msg::RidesNoRides), rx.t(Msg::RidesNoRidesSub));
            return;
        };
        let units = rx.settings.units;

        let sync = if ride.synced { rx.t(Msg::RideDetailSynced) } else { rx.t(Msg::RidesNotSynced) };
        title_frame(cv, w, h, rx.t(Msg::RideStartTitle), sync);

        let name_row = rect(14, LIST_TOP + 2, w - 28, Font::Body.line_height() as i32);
        let name = rx.marquee.fit(&ride.name, w - 28, Font::Body, Some(name_row));
        cv.text(&name, Point::new(14, LIST_TOP + 2), Font::Body, TextAlign::Left, INK);

        let d = crate::settings::DateTime::from_unix(ride.start_time);
        let mut when: heapless::String<20> = heapless::String::new();
        let _ = write!(when, "{} · {:02}:{:02}", date_iso(ride.start_time), d.hour, d.minute);
        cv.text(&when, Point::new(14, LIST_TOP + 28), Font::Label, TextAlign::Left, SUBTEXT);

        let chart_x = SIDE_MARGIN;
        let chart_w = w - 2 * SIDE_MARGIN;
        let page_b = self.pager.on_second_page();
        if !page_b {
            // An empty preview leaves the slot blank until the host fill lands.
            super::route_overview::draw_route_preview(cv, w, BAND_TOP, BAND_BOT, rx.ride_preview);
        } else if let Some(profile) = rx.ride_profile {
            // The label goes in the corner: the band is too short for a label over the apex.
            let band = ElevationBand::whole_route(profile, rect(chart_x, BAND_TOP, chart_w, BAND_BOT - BAND_TOP + 1));
            band.fill(cv, PARCHMENT_SHADE);
            band.stroke(cv, AMBER);
            band.peak_label(cv, units, PeakLabel::TopRight);
        } else {
            // The track still streams in. Keep the band footprint so the page does not jump.
            cv.text(
                rx.t(Msg::RouteOverviewLoadingProfile),
                Point::new(w / 2, (BAND_TOP + BAND_BOT) / 2 - 9),
                Font::Label,
                TextAlign::Center,
                SUBTEXT,
            );
        }
        cv.hline(chart_x, BAND_BOT + 1, chart_w, RULE);

        // AVG is the stored distance over the stored moving time, and `--` before any moving time.
        let mut dist: heapless::String<8> = heapless::String::new();
        let _ = write!(dist, "{:.1}", units.dist(ride.distance_m as f32 / 1000.0));
        let dist_unit = if units.is_imperial() { "mi" } else { "km" };

        let time = duration_hms(ride.moving_time_s as f32);

        let mut avg: heapless::String<8> = heapless::String::new();
        if ride.moving_time_s > 0 {
            let kmh = ride.distance_m as f32 / 1000.0 / (ride.moving_time_s as f32 / 3600.0);
            let _ = write!(avg, "{:.1}", units.speed(kmh));
        } else {
            let _ = avg.push_str("--");
        }
        let mut avg_cap: heapless::String<12> = heapless::String::new();
        let _ = avg_cap.push_str(rx.t(Msg::TileAvg));
        let _ = avg_cap.push_str(units.speed_label());

        let mut climb: heapless::String<8> = heapless::String::new();
        let _ = write!(climb, "{}", (units.elev(ride.climb_m as f32) + 0.5) as u32);

        let entries: [(&str, &str, &str, Option<bool>); 4] = [
            (rx.t(Msg::RideControlDistance), &dist, dist_unit, None),
            (rx.t(Msg::RideControlRideTime), &time, "", None),
            (&avg_cap, &avg, "", None),
            (rx.t(Msg::TileClimbed), &climb, units.elev_label(), Some(true)),
        ];
        let page_rows: [usize; 2] = if page_b { [2, 3] } else { [0, 1] };
        for (slot, &e) in page_rows.iter().enumerate() {
            let y = ROWS_TOP + slot as i32 * ROW_PITCH;
            let (caption, value, unit, arrow) = entries[e];
            ledger_row(cv, w, y, caption, value, unit, arrow);
            if slot + 1 < page_rows.len() {
                cv.hline(16, y + ROW_PITCH - 4, w - 32, RULE);
            }
        }

        if self.delete_enabled(rx.recording, rx.rides.len()) {
            let row_y = h - 10 - ROW_H;
            let geo = GuardedRowsGeometry::panel(w, row_y, ROW_H, 0);
            let items = [MenuItem { label: rx.t(Msg::RideDetailDeleteRide), guard: true }];
            draw_guarded_rows(cv, &items, 0, rx.hold_progress, WARNING, geo);
        }
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
                synced: false,
                synced_at_utc: 0,
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
