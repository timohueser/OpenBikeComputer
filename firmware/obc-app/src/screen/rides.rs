//! The list of stored rides, reached from the Rides station of the main menu. Each row shows the
//! ride name, a check mark when the ride is synced, and a `D MON · distance` metadata line.
//! Press opens the ride detail, which also holds the delete.
//!
//! The rides come from the app's ride catalog, which the host fills from the flat catalog.

use embedded_graphics::prelude::Point;
use obc_render::{
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::settings::{Language, Units};
use crate::Msg;

use super::vocab::chrome::empty_state;
use super::vocab::fmt::{write_date_short, write_distance_spaced};
use super::vocab::list::{self, ListGeometry, Separators};
use super::{palette, Ctx, Render, RideDetailScreen, Screen, Transition};

/// The height of a row pane, which holds the name line and the metadata line.
const ROW_H: i32 = 66;

/// Text inset from the edge of the row box. It is the Route menu inset, so the two list screens
/// keep the same gap between the cursor edge and the first character.
const TEXT_INSET: i32 = 12;

/// The half-width of the synced check mark, and its clearance from the right edge of the row box.
/// The clearance keeps the mark clear of the rounded corner.
const MARK_HALF: i32 = 5;
const MARK_RIGHT_GAP: i32 = 12;

#[derive(Debug, Default)]
pub struct RidesScreen {
    selected: usize,
}

impl RidesScreen {
    pub fn new() -> Self {
        RidesScreen { selected: 0 }
    }

    /// Re-point the highlight after a catalog rescan: the selection follows the identity of the
    /// highlighted ride to its new index, or clamps near its old position when the ride vanished.
    pub(crate) fn remap_rides(&mut self, remap: &dyn Fn(usize) -> Option<usize>, new_len: usize) {
        self.selected = remap(self.selected).unwrap_or_else(|| self.selected.min(new_len.saturating_sub(1)));
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        let len = cx.rides.len();
        match g {
            Gesture::Step(n) => list::on_step(&mut self.selected, n, len),
            // `viewed_ride` keys the host's track fill for the detail page.
            Gesture::Press if len > 0 => {
                let i = self.selected.min(len - 1);
                cx.activity.viewed_ride = Some(i);
                Transition::Push(Screen::RideDetail(RideDetailScreen::new(i)))
            }
            Gesture::Back => Transition::Pop,
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
            empty_state(cv, w, h, rx.t(Msg::RidesNoRides), rx.t(Msg::RidesNoRidesSub));
            return;
        }

        let sel = self.selected.min(total - 1);
        let first = list::window_start(sel, geo.visible, total) as i32;
        list::draw_rows(cv, geo, total, sel, first, |cv, row| {
            let ride = &rides[row.index].summary;
            let (bx, y) = (row.area.top_left.x, row.area.top_left.y);
            let accent = if row.selected { INK } else { SUBTEXT };

            // The name budget always keeps the mark slot, drawn or not, so the truncation does
            // not move when a ride syncs.
            let text_x = bx + TEXT_INSET;
            let mark_cx = bx + row.area.size.width as i32 - MARK_RIGHT_GAP - MARK_HALF;
            let name_px = (mark_cx - MARK_HALF - 8) - text_x; // mark's left edge − gap − name start
            let name_max = (name_px / Font::Body.char_width() as i32).max(6) as usize;
            let name = rx.marquee.fit(&ride.name, name_max, row.scroll());
            cv.text(&name, Point::new(text_x, y + 9), Font::Body, TextAlign::Left, INK);
            if ride.synced {
                let mark_c = Point::new(mark_cx, y + 9 + Font::Body.cap_mid() as i32);
                synced_mark(cv, mark_c, accent);
            }

            let meta_px = (w - geo.side_inset - 4) - text_x;
            let meta = meta_line(ride.start_time, ride.distance_m, units, rx.settings.language, meta_px);
            cv.text(&meta, Point::new(text_x, y + 35), Font::Label, TextAlign::Left, accent);
        });
    }
}

/// The synced check mark, centred at `c` on the cap of the name line: the shared two-stroke
/// check, at row-glyph scale.
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

/// Compose the metadata line of a row, for example `2 JUL · 42.5 km`. When the run is wider than
/// `budget_px`, the rightmost item drops whole: the date never yields and no gap shrinks. The
/// geometry is integer arithmetic over the monospace cell, so a drop is deterministic.
fn meta_line(start_time: u32, dist_m: u32, units: Units, lang: Language, budget_px: i32) -> heapless::String<32> {
    let cw = Font::Label.char_width() as i32;
    let mut dist: heapless::String<12> = heapless::String::new();
    write_distance_spaced(&mut dist, dist_m, units);

    let mut s: heapless::String<32> = heapless::String::new();
    write_date_short(&mut s, start_time, lang);
    for part in [dist.as_str()] {
        let want = s.chars().count() + 3 + part.chars().count(); // " · " + the item
        if want as i32 * cw > budget_px {
            break; // drop this item and all items right of it
        }
        let _ = s.push_str(" · ");
        let _ = s.push_str(part);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::{Activity, Mode};
    use crate::ride::{RideEntry, RideSummary};
    use crate::screen::test_ctx;
    use crate::{AppState, Settings};

    fn summary(name: &str, synced: bool) -> RideEntry {
        RideEntry {
            id: 1,
            summary: RideSummary {
                name: heapless::String::try_from(name).unwrap(),
                start_time: 1_720_000_000,
                distance_m: 42_500,
                moving_time_s: 2 * 3600 + 31 * 60,
                climb_m: 640,
                synced,
                synced_at_utc: 0,
            },
        }
    }

    fn run(scr: &mut RidesScreen, act: &mut Activity, rides: &[RideEntry], g: Gesture) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut settings = Settings::default();
        let mut cx = Ctx { rides, ..test_ctx(&mut st, act, &mut settings) };
        scr.handle(g, &mut cx)
    }

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

    #[test]
    fn press_on_an_empty_catalog_is_a_noop() {
        let mut act = Activity::new(Mode::Idle);
        let mut scr = RidesScreen::new();
        let t = run(&mut scr, &mut act, &[], Gesture::Press);
        assert!(matches!(t, Transition::None));
        assert_eq!(act.viewed_ride, None);
    }

    #[test]
    fn hold_records_no_delete_from_the_list() {
        let rides = [summary("A", true)];
        let mut act = Activity::new(Mode::Idle);
        let mut scr = RidesScreen::new();
        let t = run(&mut scr, &mut act, &rides, Gesture::Hold);
        assert!(matches!(t, Transition::None));
        assert_eq!(act.take_ride_delete(), None, "no delete request from the list");
    }

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

    #[test]
    fn meta_line_is_short_date_plus_distance() {
        let cw = Font::Label.char_width() as i32;
        // The line-2 budget of a 240 px panel. 1_720_000_000 = 2024-07-03 UTC.
        let pane = 200;
        assert_eq!(meta_line(1_720_000_000, 42_500, Units::Metric, Language::En, pane).as_str(), "3 JUL · 42.5 km");
        // A three-digit-km ride compacts to whole km. 1_735_257_600 = 2024-12-27.
        let worst = meta_line(1_735_257_600, 142_500, Units::Metric, Language::En, pane);
        assert_eq!(worst.as_str(), "27 DEC · 143 km", "tenths compact away past 100 km");
        let worst_fr = meta_line(1_719_100_800, 142_400, Units::Metric, Language::Fr, pane);
        assert_eq!(worst_fr.as_str(), "23 JUIN · 142 km");
        assert!(worst_fr.chars().count() as i32 * cw <= pane, "the worst run fits the budget");
        // The month table is per-language, and day-first everywhere. 1_709_596_800 = 2024-03-05.
        assert_eq!(meta_line(1_709_596_800, 8_000, Units::Metric, Language::De, pane).as_str(), "5 MÄR · 8.0 km");
        // On overflow the distance drops whole and the date stays.
        assert_eq!(meta_line(1_720_000_000, 42_500, Units::Metric, Language::En, 6 * cw).as_str(), "3 JUL");
    }
}
