//! The Ride detail (epic #678 T2 / #680) — the recorded sibling of the
//! [Route overview](super::route_overview), opened by *press* on a Rides-list row. Top to bottom:
//! the `RIDE` title bar with the sync state in its right slot, the ride name, a `date · time`
//! line, the **content-paired pager** (owner review round 3 — the media band flips WITH its
//! stats): page A the recorded track's **shape preview** (the overview's aspect-fit sketch,
//! start disc + end diamond) over DISTANCE + RIDE TIME, page B the recorded **elevation band**
//! (the overview's composition — tan fill under an amber top stroke, max-elevation label at the
//! band's top-right) over AVG + CLIMBED — all stats from the
//! [`RideSummary`](crate::ride::RideSummary), no new figures — and the guarded **Delete ride**
//! row at the bottom.
//!
//! The band's profile **and** the track shape come from the host: entering the screen sets
//! [`Activity::viewed_ride`](crate::Activity::viewed_ride) (the Rides screen's press does), the
//! host drains [`App::ride_track_request`](crate::App::ride_track_request), streams
//! the ride's `RD{id}.ORD` into the app's resident ride-profile + ride-preview buffers
//! ([`App::set_ride_profile`](crate::App::set_ride_profile) /
//! [`App::set_ride_preview`](crate::App::set_ride_preview)), and Back/delete clears `viewed_ride`
//! so both invalidate on exit — filled on entry, one buffer each, never rebuilt per frame.
//!
//! **Delete** is the ride_control-pattern guarded row: the completed hold *is* the confirmation
//! (its fill the live feedback), the host deletes `RD{id}.ORD` + its synced-set entry, and the
//! screen pops back to the refreshed Rides list. While a ride is being recorded the row is
//! **hidden** (owner review round 1 — no greyed face): a live session holds `TRACK.OBT` open and
//! its `RD{id}.ORD` isn't written until Finish, so deleting is neither meaningful nor legal then,
//! and the `delete_enabled` guard keeps a hold a no-op regardless.
//!
//! Fit note (reworked in owner review round 2 — "very busy"): the four ledger rows don't fit
//! beside a readable band, so they auto-flip as the Route overview's **two-row pager** (5 s fixed
//! dwell, no page dots — the flip is the affordance) and the reclaimed vertical space goes back
//! into the media band (34 → 82 px, near the overview's 90 px reference look). Round 3 paired the
//! band with the stats: the track shape belongs to the distance/time page, the elevation band to
//! the climb page. The ledger *text* is untouched (the locked shrink order).

use core::fmt::Write;

use embedded_graphics::prelude::Point;
use obc_render::{
    text::{Font, TextAlign},
    Surface,
};

use crate::activity::Activity;
use crate::input::Gesture;
use crate::screen::ScreenTick;
use crate::stat_fields::fmt_hms;
use crate::Msg;

use super::{ledger_row, palette, title_frame, Ctx, MenuItem, Render, Transition, LIST_TOP};

/// The content-paired media band (track shape / elevation): the Route overview's composition,
/// regrown near its reference height by the pager rework (owner review round 2 — see the module
/// doc). Both pages draw in this same slot, so nothing jumps on the flip.
const BAND_TOP: i32 = 96;
const BAND_BOT: i32 = 178;
const SIDE_MARGIN: i32 = 12;

/// The stat half of the pager: two caption/value rows per page between the band and the delete row, at the
/// overview's row pitch (the compressed 33 px pitch retired with the four-row ledger).
const ROWS_TOP: i32 = 186;
const ROW_PITCH: i32 = 42;

/// The guarded Delete-ride row at the bottom — the overview's button band footprint.
const ROW_H: i32 = 34;

/// The stat pager's dwell — the Route overview's fixed 5 s flip (T3's constant, mirrored so the
/// two sibling pages read on the same rhythm).
const PAGE_FLIP_MS: u32 = 5_000;

/// The Ride detail. State is which catalog ride it shows plus the stat pager's flip state; the
/// delete row is the one selectable action, so there is no cursor.
#[derive(Debug, Default)]
pub struct RideDetailScreen {
    ride: usize,
    /// Which content-paired page is showing (0 = track shape + DISTANCE + RIDE TIME, 1 =
    /// elevation band + AVG + CLIMBED); auto-flipped by [`tick_timers`](Self::tick_timers).
    page: usize,
    /// Instant of the last page flip (wrap-safe). `None` until the first tick anchors it, so the
    /// first page gets a full dwell on entry — mirrors the Route overview's pager.
    last_flip_ms: Option<u32>,
}

impl RideDetailScreen {
    /// Open catalog ride `ride`'s detail. The caller (the Rides list's press) sets
    /// [`Activity::viewed_ride`](crate::Activity::viewed_ride) alongside, keying the host's
    /// track-profile fill.
    pub fn new(ride: usize) -> Self {
        RideDetailScreen { ride, page: 0, last_flip_ms: None }
    }

    /// Content-paired pager tick: flip the two pages every [`PAGE_FLIP_MS`], reporting the
    /// residual dwell as the next wake — exactly the Route overview's `tick_timers` (T3's
    /// Statistics-derived machinery), on the recorded sibling.
    pub fn tick_timers(&mut self, now_ms: u32) -> ScreenTick {
        let last = *self.last_flip_ms.get_or_insert(now_ms);
        let changed = now_ms.wrapping_sub(last) >= PAGE_FLIP_MS;
        if changed {
            self.page ^= 1; // two pages
            self.last_flip_ms = Some(now_ms);
        }
        let anchor = self.last_flip_ms.unwrap_or(now_ms);
        let next = PAGE_FLIP_MS.saturating_sub(now_ms.wrapping_sub(anchor)).max(1);
        ScreenTick { changed, next_wake_ms: Some(next), region: None }
    }

    /// Re-point the shown ride after a live catalog rescan. A vanished subject becomes an
    /// out-of-range index — the missing-ride path `draw`/`handle` already have (the empty state;
    /// a hold does nothing).
    pub(crate) fn remap_rides(&mut self, remap: &dyn Fn(usize) -> Option<usize>) {
        self.ride = remap(self.ride).unwrap_or(usize::MAX);
    }

    /// Whether the Delete-ride row is **live**: the ride exists and no tracking session is
    /// running (the old footer's "no delete while recording" rule — every delete stays legal).
    fn delete_enabled(&self, activity: &Activity, len: usize) -> bool {
        len > 0 && self.ride < len && !activity.is_tracking()
    }

    /// True while the delete row would fill for the current state — so
    /// [`App::top_wants_hold_fill`](crate::App::top_wants_hold_fill) repaints a charging hold here.
    pub(crate) fn selection_is_guarded(&self, activity: &Activity, rides_len: usize) -> bool {
        self.delete_enabled(activity, rides_len)
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            // A completed hold over the live Delete row requests the ride's deletion — the guarded
            // hold is the confirmation (no popup), the row's fill its live feedback. Records the
            // delete by index; the host resolves it to the durable object id, deletes `RD{id}.ORD`
            // + its synced-set entry, and the rescan re-feeds the catalog — while this pops back to
            // the Rides list (its remap keeps the highlight sane). A hold while recording (greyed
            // row) does nothing.
            Gesture::Hold if self.delete_enabled(cx.activity, cx.rides.len()) => {
                cx.activity.request_ride_delete(self.ride.min(cx.rides.len() - 1));
                cx.activity.viewed_ride = None; // leaving the page: the profile buffer invalidates
                Transition::Pop
            }
            Gesture::Back => {
                cx.activity.viewed_ride = None; // invalidate the resident ride profile on exit
                Transition::Pop
            }
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        let Some(ride) = rx.rides.get(self.ride) else {
            // The shown ride vanished in a rescan (deleted from the phone mid-view): the Rides
            // list's own empty-state copy, Back returns to the refreshed list.
            title_frame(cv, w, h, rx.t(Msg::RideStartTitle), "");
            super::empty_state(cv, w, h, rx.t(Msg::RidesNoRides), rx.t(Msg::RidesNoRidesSub));
            return;
        };
        let units = rx.settings.units;

        // Title bar: `RIDE` + the sync state in the right slot (Label, bar text colour).
        let sync = if ride.synced { rx.t(Msg::RideDetailSynced) } else { rx.t(Msg::RidesNotSynced) };
        title_frame(cv, w, h, rx.t(Msg::RideStartTitle), sync);

        // Ride name (Body, left inset, two-dot truncation at full card width).
        let name = super::route_menu::fit_name(&ride.name, ((w - 28) / Font::Body.char_width() as i32) as usize);
        cv.text(&name, Point::new(14, LIST_TOP + 2), Font::Body, TextAlign::Left, INK);

        // Date + start time on one olive Label line, e.g. `2025-07-02 · 14:12` — the list rows'
        // date helper plus the wall clock's `HH:MM` shape, no new formats.
        let d = crate::settings::DateTime::from_unix(ride.start_time);
        let mut when: heapless::String<20> = heapless::String::new();
        let _ = write!(when, "{} · {:02}:{:02}", super::rides::fmt_date(ride.start_time), d.hour, d.minute);
        cv.text(&when, Point::new(14, LIST_TOP + 28), Font::Label, TextAlign::Left, SUBTEXT);

        // The content-paired media band (owner review round 3): page A the recorded track's
        // shape preview (the overview's sketch — start disc, end diamond), page B its elevation
        // band (the overview's composition, tan fill under an amber top stroke) — both from the
        // host-filled residents, both in the same slot so nothing jumps on the flip.
        let chart_x = SIDE_MARGIN;
        let chart_w = w - 2 * SIDE_MARGIN;
        let page_b = self.page & 1 == 1;
        if !page_b {
            // Page A: an empty slice (the frame or two before the host's fill lands) just leaves
            // the slot blank, like the shape preview always has.
            super::route_overview::draw_route_preview(cv, w, BAND_TOP, BAND_BOT, rx.ride_preview);
        } else if let Some(profile) = rx.ride_profile {
            let win = profile.window(0.5, 1.0, chart_w.max(1) as u32);
            let span = (win.hi_frac - win.lo_frac).max(1e-6);
            let span_ele = (profile.max_ele_m - profile.min_ele_m).max(1) as f32;
            let ele_to_y = |e: i16| -> i32 {
                let t = ((e - profile.min_ele_m) as f32 / span_ele).clamp(0.0, 1.0);
                BAND_BOT - (t * (BAND_BOT - BAND_TOP) as f32) as i32
            };
            let mut prev_top: Option<i32> = None;
            for px in 0..chart_w {
                let f = win.lo_frac + span * (px as f32 / chart_w as f32);
                let top_y = ele_to_y(profile.sample(win.level, f).1);
                let x = chart_x + px;
                cv.vline(x, top_y, BAND_BOT - top_y + 1, 1, PARCHMENT_SHADE);
                // Amber top line, connected to the previous column so steep sections stay solid.
                let (y0, y1) = prev_top.map_or((top_y, top_y), |p| (p.min(top_y), p.max(top_y)));
                cv.vline(x, y0 - 1, (y1 - y0) + 2, 1, AMBER);
                prev_top = Some(top_y);
            }
            // Max-elevation label at the band's top-right corner.
            let mut peak: heapless::String<10> = heapless::String::new();
            let _ = write!(peak, "{} {}", units.elev(profile.peak_ele_m() as f32) as i32, units.elev_label());
            cv.text(&peak, Point::new(chart_x + chart_w - 2, BAND_TOP - 2), Font::Label, TextAlign::Right, SUBTEXT);
        } else {
            // Track still streaming in: keep the band's footprint so the page doesn't jump.
            cv.text(
                rx.t(Msg::RouteOverviewLoadingProfile),
                Point::new(w / 2, (BAND_TOP + BAND_BOT) / 2 - 9),
                Font::Label,
                TextAlign::Center,
                SUBTEXT,
            );
        }
        cv.hline(chart_x, BAND_BOT + 1, chart_w, RULE); // baseline marks the band slot on both pages

        // The stat half of the pager — everything from the RideSummary, no new stats. AVG is the
        // Statistics AVG tile's quotient (moving distance over moving time, here the stored
        // totals) and its caption (`AVG ` + the unit label); `--` before any moving time.
        let mut dist: heapless::String<8> = heapless::String::new();
        let _ = write!(dist, "{:.1}", units.dist(ride.distance_m as f32 / 1000.0));
        let dist_unit = if units.is_imperial() { "mi" } else { "km" };

        let time = fmt_hms(ride.moving_time_s as f32);

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

        // Two rows per page, auto-flipped every 5 s with the media band above (owner review
        // rounds 2 + 3: the Route overview's pager mechanics — the flip itself is the affordance,
        // no page dots), with the overview's hairline rule between a page's two rows. The stats
        // pair with their media: DISTANCE + RIDE TIME belong to the track shape, AVG + CLIMBED to
        // the elevation band.
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

        // The guarded Delete-ride row at the bottom (the ride_control pattern): its shaded base
        // fills warning-red with the live hold. While a ride is being recorded the row is simply
        // **not drawn** — no dim trash, no `Recording` cue (owner review round 1: the state can't
        // act, so it doesn't show) — and the `delete_enabled` guard keeps a hold a no-op regardless.
        if self.delete_enabled(rx.activity, rx.rides.len()) {
            let row_y = h - 10 - ROW_H;
            let geo = super::GuardedRowsGeometry {
                x: 14,
                w: w - 28,
                top: row_y,
                row_h: ROW_H,
                gap: 0,
                label_dx: 12,
                label_dy: 5,
            };
            let items = [MenuItem { label: rx.t(Msg::RideDetailDeleteRide), guard: true }];
            super::draw_guarded_rows(cv, &items, 0, rx.hold_progress, WARNING, geo);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Mode;
    use crate::ride::RideSummary;
    use crate::{AppState, Settings};

    fn summary(name: &str) -> RideSummary {
        RideSummary {
            name: heapless::String::try_from(name).unwrap(),
            start_time: 1_720_000_000,
            distance_m: 42_500,
            moving_time_s: 2 * 3600 + 31 * 60,
            climb_m: 640,
            synced: false,
        }
    }

    fn run(scr: &mut RideDetailScreen, act: &mut Activity, rides: &[RideSummary], g: Gesture) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut settings = Settings::default();
        let scratch = crate::screen::PoiScratch::new();
        let mut cx = Ctx {
            state: &mut st,
            activity: act,
            settings: &mut settings,
            routes: &[],
            rides,
            trips: &[],
            nav_profiles: &crate::NavProfiles::EMPTY,
            poi_scratch: &scratch,
            sensor_scan_hits: &[],
            now_ms: 0,
        };
        scr.handle(g, &mut cx)
    }

    /// A completed hold over the live Delete row records the delete, invalidates the profile key,
    /// and pops back to the Rides list.
    #[test]
    fn hold_deletes_and_returns_to_the_list() {
        let rides = [summary("A"), summary("B")];
        let mut act = Activity::new(Mode::Idle);
        act.viewed_ride = Some(1);
        let mut scr = RideDetailScreen::new(1);
        let t = run(&mut scr, &mut act, &rides, Gesture::Hold);
        assert!(matches!(t, Transition::Pop), "the delete returns to the list");
        assert_eq!(act.take_ride_delete(), Some(1), "the shown ride's index is requested");
        assert_eq!(act.viewed_ride, None, "leaving the page invalidates the profile buffer");
    }

    /// The row is disabled — a hold does nothing — while a ride is being recorded; ending the
    /// session re-arms it.
    #[test]
    fn hold_is_a_no_op_while_recording() {
        let rides = [summary("A")];
        let mut act = Activity::new(Mode::Riding);
        act.start_session(); // now tracking
        act.viewed_ride = Some(0);
        let mut scr = RideDetailScreen::new(0);
        assert!(!scr.selection_is_guarded(&act, rides.len()), "the row is hidden while recording");
        let t = run(&mut scr, &mut act, &rides, Gesture::Hold);
        assert!(matches!(t, Transition::None), "a hold while recording stays on the page");
        assert_eq!(act.take_ride_delete(), None, "and records nothing");
        assert_eq!(act.viewed_ride, Some(0), "the profile stays keyed to the open page");

        act.end_session();
        assert!(scr.selection_is_guarded(&act, rides.len()), "ending the ride re-arms the row");
    }

    /// Back pops and clears `viewed_ride`, so the resident ride profile invalidates on exit.
    #[test]
    fn back_pops_and_invalidates_the_profile_key() {
        let rides = [summary("A")];
        let mut act = Activity::new(Mode::Idle);
        act.viewed_ride = Some(0);
        let mut scr = RideDetailScreen::new(0);
        let t = run(&mut scr, &mut act, &rides, Gesture::Back);
        assert!(matches!(t, Transition::Pop));
        assert_eq!(act.viewed_ride, None);
    }

    /// The two-row stat pager flips exactly at the dwell deadline and only once, re-arming a fresh
    /// dwell — the Route overview's auto-flip contract, mirrored on the recorded sibling. The
    /// first poll anchors the dwell (page 0 gets a full one).
    #[test]
    fn stat_pager_flips_once_at_the_deadline() {
        let mut scr = RideDetailScreen::new(0);
        assert_eq!(scr.page, 0);
        assert!(!scr.tick_timers(0).changed, "the first poll only anchors the dwell");
        assert!(!scr.tick_timers(PAGE_FLIP_MS - 1).changed, "still dwelling just before the deadline");
        assert_eq!(scr.page, 0);
        assert!(scr.tick_timers(PAGE_FLIP_MS).changed, "flips exactly at the deadline");
        assert_eq!(scr.page, 1, "now on the AVG + CLIMBED page");
        assert!(!scr.tick_timers(PAGE_FLIP_MS + 1).changed, "and only once — a fresh dwell re-armed");
        assert!(scr.tick_timers(2 * PAGE_FLIP_MS).changed, "flips back at the next deadline");
        assert_eq!(scr.page, 0);
    }

    /// A vanished subject (out-of-range after a rescan) offers no delete.
    #[test]
    fn vanished_ride_has_no_delete() {
        let mut act = Activity::new(Mode::Idle);
        let mut scr = RideDetailScreen::new(0);
        scr.remap_rides(&|_| None);
        assert!(!scr.selection_is_guarded(&act, 1), "an out-of-range subject arms nothing");
        let t = run(&mut scr, &mut act, &[summary("A")], Gesture::Hold);
        assert!(matches!(t, Transition::None));
        assert_eq!(act.take_ride_delete(), None);
    }
}
