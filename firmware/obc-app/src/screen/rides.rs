//! The Rides screen (epic #447 P7 / #454; rows redesigned by epic #678 T2 / #680) — the list of
//! stored rides, reached from the main Menu's Rides station. Each row is two lines: the ride
//! **name** with width priority (two-dot truncation only when actually needed) and — on a synced
//! ride only — a small right-aligned **check mark** (owner review round 2: the solid/hollow disc
//! pair was unreadable; the check is the device's established success idiom, and an unsynced ride
//! simply shows nothing — the Ride detail's title-bar "not synced" text carries the explicit
//! state) — then an olive metadata line, `D MON · distance` (the C1 re-cut: a short day-first
//! date the 240 px pane can actually hold beside the distance; the full date lives on the
//! detail's date·time line, the duration in its ledger), with the anti-cram rule: when the line
//! would collide at the pane width the **rightmost** item drops (the distance, in a pathological
//! overflow) rather than any gap shrinking — the date never yields.
//!
//! Rides come from the app's ride catalog ([`Render::rides`]/[`Ctx::rides`]), populated by the host
//! from `/tracks/RD{id}.ORD` headers — the same source the BLE `rideList` serves. Each summary
//! carries a `synced` flag (whether the phone has downloaded the ride at least once, persisted in
//! the `/tracks` synced-set sidecar) — the glyph's fact.
//!
//! **Press opens the [Ride detail](super::ride_detail)** (#680) — the recorded sibling of the
//! Route overview: elevation band, stat ledger, and the guarded *Delete ride* row. Deleting moved
//! there with it; this screen carries no hold-to-delete footer anymore, and the reclaimed band
//! returns to list rows.

use core::fmt::Write;

use embedded_graphics::prelude::Point;
use obc_render::{
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::settings::{DateTime, Language, Units};
use crate::{t, Msg};

use super::list::{self, ListGeometry, Separators};
use super::route_menu::fit_name;
use super::{palette, Ctx, Render, RideDetailScreen, Screen, Transition};

/// Per-ride pane height (two lines: name + metadata), sized to fill the full list area (the old
/// delete footer's band returned to rows).
const ROW_H: i32 = 66;

/// Text inset from the row box's edge — the Route menu rows' `NAME_INSET`, so the two list screens
/// keep the same gap between the amber cursor's edge and their first character (owner review round
/// 2: the name used to start on the box edge itself).
const TEXT_INSET: i32 = 12;

/// The synced check mark's half-width (~10 px ink) and its clearance from the row box's right
/// edge — pulled left far enough that the mark never touches the box's rounded corner.
const MARK_HALF: i32 = 5;
const MARK_RIGHT_GAP: i32 = 12;

/// The Rides list. State is the highlighted ride.
#[derive(Debug, Default)]
pub struct RidesScreen {
    selected: usize,
}

impl RidesScreen {
    pub fn new() -> Self {
        RidesScreen { selected: 0 }
    }

    /// Re-point the highlight after a live ride-catalog rescan (#454): the selection follows the
    /// previously-highlighted ride's *identity* to its new index; if that ride vanished (deleted here
    /// or from the phone) it clamps near its old position. Mirrors the Route menu's `remap_routes`.
    pub(crate) fn remap_rides(&mut self, remap: &dyn Fn(usize) -> Option<usize>, new_len: usize) {
        self.selected = remap(self.selected).unwrap_or_else(|| self.selected.min(new_len.saturating_sub(1)));
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        let len = cx.rides.len();
        match g {
            Gesture::Step(n) => list::on_step(&mut self.selected, n, len),
            // Press opens the highlighted ride's detail (#680). `viewed_ride` keys the host's
            // track-profile fill — the detail's elevation band streams the ride's `RD{id}.ORD`
            // once while the page is up (the Route overview's `active_route` idiom).
            Gesture::Press if len > 0 => {
                let i = self.selected.min(len - 1);
                cx.activity.viewed_ride = Some(i);
                Transition::Push(Screen::RideDetail(RideDetailScreen::new(i)))
            }
            Gesture::Back => Transition::Pop, // return to the Menu
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        let rides = rx.rides;
        let total = rides.len();
        let units = rx.settings.units;
        let geo = ListGeometry::below_title(w, h, ROW_H, 8, 12, Separators::Unselected);

        let pos = if total == 0 { 0 } else { self.selected.min(total - 1) + 1 };
        list::list_frame(cv, w, h, rx.t(Msg::RidesTitle), pos, total, geo.visible);

        if total == 0 {
            super::empty_state(cv, w, h, rx.t(Msg::RidesNoRides), rx.t(Msg::RidesNoRidesSub));
            return;
        }

        let sel = self.selected.min(total - 1);
        let first = list::window_start(sel, geo.visible, total) as i32;
        list::draw_rows(cv, geo, total, sel, first, |cv, row| {
            let ride = &rides[row.index];
            let (bx, y) = (row.area.top_left.x, row.area.top_left.y);
            let accent = if row.selected { INK } else { SUBTEXT };

            // Line one: the ride name with width priority — its budget is the pane minus the
            // sync-mark slot + standard gap (reserved whether or not the mark draws, so the
            // truncation never jumps when a ride syncs), truncated with ".." only when it
            // actually overruns. A **synced** ride carries a small check mark right-aligned on
            // the same line, vertically centred on the name's cap and pulled clear of the box's
            // rounded corner; an unsynced ride shows nothing there (owner review round 2).
            let text_x = bx + TEXT_INSET;
            let mark_cx = bx + row.area.size.width as i32 - MARK_RIGHT_GAP - MARK_HALF;
            let name_px = (mark_cx - MARK_HALF - 8) - text_x; // mark's left edge − gap − name start
            let name_max = (name_px / Font::Body.char_width() as i32).max(6) as usize;
            let name = fit_name(&ride.name, name_max);
            cv.text(&name, Point::new(text_x, y + 9), Font::Body, TextAlign::Left, INK);
            if ride.synced {
                let mark_c = Point::new(mark_cx, y + 9 + Font::Body.cap_height() as i32 / 2);
                synced_mark(cv, mark_c, accent);
            }

            // Line two: `D MON · distance`, one olive Label run with the anti-cram
            // drop-rightmost rule (never a squeezed gap). Budget: the new text inset to a 4 px
            // guard inside the box's right edge.
            let meta_px = (w - geo.side_inset - 4) - text_x;
            let meta = meta_line(ride.start_time, ride.distance_m, units, rx.settings.language, meta_px);
            cv.text(&meta, Point::new(text_x, y + 35), Font::Label, TextAlign::Left, accent);
        });
    }
}

/// The synced check mark — the [`card_check`](super::card_check) two-stroke check shrunk to the
/// row-glyph scale (~10 px ink, 1 px-radius disc strokes so it stays legible without going bold):
/// down-stroke to the low point, then up-stroke to the top-right. `c` is the mark's centre on the
/// name line's cap; drawn in the row's accent (olive on a resting row, ink on the amber cursor).
fn synced_mark(cv: &mut impl Surface, c: Point, color: u16) {
    fn seg(cv: &mut impl Surface, a: (i32, i32), b: (i32, i32), color: u16) {
        const N: i32 = 8;
        for s in 0..=N {
            let x = a.0 + (b.0 - a.0) * s / N;
            let y = a.1 + (b.1 - a.1) * s / N;
            cv.disc(Point::new(x, y), 1, color);
        }
    }
    let k = MARK_HALF;
    seg(cv, (c.x - k, c.y), (c.x - k / 3, c.y + k * 2 / 3), color);
    seg(cv, (c.x - k / 3, c.y + k * 2 / 3), (c.x + k, c.y - k * 2 / 3), color);
}

/// Compose a row's metadata line — `D MON · distance` (e.g. `2 JUL · 42.5 km`) — dropping the
/// **rightmost** item when the run would overflow `budget_px` (the C1 anti-cram guard: the
/// distance drops whole in a pathological overflow; the date never does, and gaps never shrink).
/// The worst legitimate run — a two-digit day, a four-char month (fr `JUIL`), a three-digit-km
/// distance (whole km at that magnitude — [`write_distance`] compacts past 100) — is 16 Label
/// cells, inside the inset row's ~16.6-cell budget, so the guard normally never fires. Pure
/// integer geometry over the monospace Label cell, so any drop is deterministic.
fn meta_line(start_time: u32, dist_m: u32, units: Units, lang: Language, budget_px: i32) -> heapless::String<32> {
    let cw = Font::Label.char_width() as i32;
    let mut dist: heapless::String<12> = heapless::String::new();
    write_distance(&mut dist, dist_m, units);

    let mut s: heapless::String<32> = heapless::String::new();
    write_short_date(&mut s, start_time, lang);
    for part in [dist.as_str()] {
        let want = s.chars().count() + 3 + part.chars().count(); // " · " + the item
        if want as i32 * cw > budget_px {
            break; // drop this item and everything right of it
        }
        let _ = s.push_str(" · ");
        let _ = s.push_str(part);
    }
    s
}

/// The 12 uppercase month-abbreviation keys (the `[date]` catalog section) in calendar order —
/// the short-date table the rides rows draw from (and the Home date line reuses, T5 / #683).
/// Distinct from the Date & Time stepper's mixed-case `[month]` table.
const DATE_MONTHS: [Msg; 12] = [
    Msg::DateJan,
    Msg::DateFeb,
    Msg::DateMar,
    Msg::DateApr,
    Msg::DateMay,
    Msg::DateJun,
    Msg::DateJul,
    Msg::DateAug,
    Msg::DateSep,
    Msg::DateOct,
    Msg::DateNov,
    Msg::DateDec,
];

/// Append a ride's unix `start_time` as the short day-first date `D MON` (UTC) — no leading zero,
/// the month from the per-language uppercase table. Day-first in all four languages (the locked
/// shared shape).
fn write_short_date<const N: usize>(s: &mut heapless::String<N>, start_time: u32, lang: Language) {
    let d = DateTime::from_unix(start_time);
    let _ = write!(s, "{} {}", d.day, t(DATE_MONTHS[(d.month.clamp(1, 12) - 1) as usize], lang));
}

/// Format a ride's unix `start_time` as a compact `YYYY-MM-DD` (UTC) — the list's and the Ride
/// detail's shared date shape. (Local-time formatting would need the app's UTC offset threaded in;
/// the date rarely differs and the extra plumbing isn't worth it.)
pub(crate) fn fmt_date(start_time: u32) -> heapless::String<12> {
    let d = DateTime::from_unix(start_time);
    let mut s = heapless::String::new();
    let _ = write!(s, "{:04}-{:02}-{:02}", d.year, d.month, d.day);
    s
}

/// Append a compact distance in the rider's units: `NN.N km` / `NN.N mi`, compacting to a whole
/// unit (`142 km`) from 100 up — the tenths stop meaning anything at that magnitude, and the whole
/// figure keeps the worst legitimate metadata run inside the inset row's budget (the same
/// compact-past-the-crossover idiom as [`write_off_route`](super::write_off_route)).
fn write_distance<const N: usize>(s: &mut heapless::String<N>, dist_m: u32, units: Units) {
    if units.is_imperial() {
        use crate::settings::{FT_PER_M, FT_PER_MI};
        let mi10 = (dist_m as f32 * FT_PER_M / FT_PER_MI as f32 * 10.0) as u32;
        if mi10 >= 1000 {
            let _ = write!(s, "{} mi", (mi10 + 5) / 10);
        } else {
            let _ = write!(s, "{}.{} mi", mi10 / 10, mi10 % 10);
        }
    } else {
        let km10 = (dist_m + 50) / 100; // tenths of a km
        if km10 >= 1000 {
            let _ = write!(s, "{} km", (dist_m + 500) / 1000);
        } else {
            let _ = write!(s, "{}.{} km", km10 / 10, km10 % 10);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::{Activity, Mode};
    use crate::ride::RideSummary;
    use crate::{AppState, Settings};

    fn summary(name: &str, synced: bool) -> RideSummary {
        RideSummary {
            name: heapless::String::try_from(name).unwrap(),
            start_time: 1_720_000_000,
            distance_m: 42_500,
            moving_time_s: 2 * 3600 + 31 * 60,
            climb_m: 640,
            synced,
            synced_at_utc: 0,
        }
    }

    fn run(scr: &mut RidesScreen, act: &mut Activity, rides: &[RideSummary], g: Gesture) -> Transition {
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
            waypoints: &[],
            corridor: &[],
            sensor_scan_hits: &[],
            now_ms: 0,
        };
        scr.handle(g, &mut cx)
    }

    /// Press opens the highlighted ride's detail and keys the host's track-profile fill on it
    /// (`viewed_ride`) — the row is a door now, not a no-op (#680).
    #[test]
    fn press_opens_the_highlighted_rides_detail() {
        let rides = [summary("A", true), summary("B", false)];
        let mut act = Activity::new(Mode::Idle);
        let mut scr = RidesScreen::new();
        run(&mut scr, &mut act, &rides, Gesture::Step(1)); // highlight row 1 ("B")
        let t = run(&mut scr, &mut act, &rides, Gesture::Press);
        assert!(matches!(t, Transition::Push(Screen::RideDetail(_))), "press pushes the Ride detail");
        assert_eq!(act.viewed_ride, Some(1), "the detail's track request is keyed on the pressed row");
    }

    /// An empty catalog's press does nothing (no detail to open).
    #[test]
    fn press_on_an_empty_catalog_is_a_noop() {
        let mut act = Activity::new(Mode::Idle);
        let mut scr = RidesScreen::new();
        let t = run(&mut scr, &mut act, &[], Gesture::Press);
        assert!(matches!(t, Transition::None));
        assert_eq!(act.viewed_ride, None);
    }

    /// The list carries no hold-to-delete anymore — a hold records nothing (deletes live on the
    /// Ride detail's guarded row now, #680).
    #[test]
    fn hold_records_no_delete_from_the_list() {
        let rides = [summary("A", true)];
        let mut act = Activity::new(Mode::Idle);
        let mut scr = RidesScreen::new();
        let t = run(&mut scr, &mut act, &rides, Gesture::Hold);
        assert!(matches!(t, Transition::None));
        assert_eq!(act.take_ride_delete(), None, "no delete request from the list");
    }

    /// `remap_rides` follows the highlighted ride's identity across a rescan, and clamps when it
    /// vanishes.
    #[test]
    fn remap_follows_identity_and_clamps_on_vanish() {
        let mut scr = RidesScreen::new();
        scr.selected = 2;
        // Row 2 moved to row 0.
        scr.remap_rides(&|i| if i == 2 { Some(0) } else { None }, 3);
        assert_eq!(scr.selected, 0);
        // Row (now 0) vanished; a shorter list clamps to the last row.
        scr.selected = 5;
        scr.remap_rides(&|_| None, 2);
        assert_eq!(scr.selected, 1, "a vanished highlight clamps to the last row");
    }

    /// The metadata line (the C1 re-cut): `D MON · distance` fits the inset row's line-2 budget
    /// whole — including the worst legitimate run (two-digit day, four-char fr month, a
    /// three-digit-km distance compacted to whole km) — and the drop-rightmost guard still sheds
    /// the distance (never the date) in a pathological overflow. Months come from the
    /// per-language `[date]` table.
    #[test]
    fn meta_line_is_short_date_plus_distance() {
        let cw = Font::Label.char_width() as i32;
        let pane = 200; // the 240 px panel's line-2 budget: (w − 12 − 4) − (12 + 12)
                        // 1_720_000_000 = 2024-07-03 UTC.
        assert_eq!(meta_line(1_720_000_000, 42_500, Units::Metric, Language::En, pane).as_str(), "3 JUL · 42.5 km");
        // A three-digit-km ride compacts to whole km, so the worst legitimate run — a two-digit
        // day + the 4-char fr JUIL — is 16 cells, still whole. 1_735_257_600 = 2024-12-27.
        let worst = meta_line(1_735_257_600, 142_500, Units::Metric, Language::En, pane);
        assert_eq!(worst.as_str(), "27 DEC · 143 km", "tenths compact away past 100 km");
        let worst_fr = meta_line(1_719_100_800, 142_400, Units::Metric, Language::Fr, pane);
        assert_eq!(worst_fr.as_str(), "23 JUIN · 142 km");
        assert!(worst_fr.chars().count() as i32 * cw <= pane, "the worst run fits the budget");
        // The month table is per-language (day-first everywhere): 1_709_596_800 = 2024-03-05.
        assert_eq!(meta_line(1_709_596_800, 8_000, Units::Metric, Language::De, pane).as_str(), "5 MÄR · 8.0 km");
        // Pathological overflow: the distance drops whole; the date never yields.
        assert_eq!(meta_line(1_720_000_000, 42_500, Units::Metric, Language::En, 6 * cw).as_str(), "3 JUL");
    }
}
