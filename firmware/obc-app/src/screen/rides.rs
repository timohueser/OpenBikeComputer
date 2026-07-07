//! The Rides screen (epic #447, P7 / #454) — a **see-and-delete** list of stored rides, reached
//! from the main Menu's Rides station. It exists to let a rider reclaim SD space; it is *not* a ride
//! browser (no detail screen, no track preview — out of scope). Each row is two lines — the ride
//! name with its date, then a compact distance / time / climb stats line — in the established
//! two-line list style (the Route menu / POI list are the models).
//!
//! Rides come from the app's ride catalog ([`Render::rides`]/[`Ctx::rides`]), populated by the host
//! from `/tracks/RD{id}.ORD` headers — the same source the BLE `rideList` serves. Each summary
//! carries a `synced` flag (whether the phone has downloaded the ride at least once, persisted in the
//! `/tracks` synced-set sidecar): an **unsynced** ride's delete footer renders warning-red with a
//! "not synced" cue — still deletable, just informed.
//!
//! **Delete** is the P6 hold-to-delete idiom (the guarded hold *is* the confirmation, no popup). The
//! footer is greyed while a ride is being recorded — a live tracking session holds `TRACK.OBT` open
//! and its `RD{id}.ORD` isn't written until Finish, so deleting is neither meaningful nor legal then
//! (embedded-sdmmc refuses to delete an open handle). The completed hold records a delete by index;
//! [`App::take_ride_delete`](crate::App::take_ride_delete) resolves it to the ride's durable object
//! id and the host deletes through `ObjectStore` (revision bump + `storeChanged`), so the phone's
//! device-rides reconcile — its own library copy is untouched, and ids never reuse.

use core::fmt::Write;

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use crate::activity::Activity;
use crate::input::Gesture;
use crate::settings::{DateTime, Units};

use super::list::{self, ListGeometry, Separators};
use super::route_menu::fit_name;
use super::{palette, Ctx, Render, Transition};

/// Height of the hold-to-delete footer reserved below the list — matches the Route menu's footer band
/// so the two screens' delete idiom reads identically.
const FOOTER_H: i32 = 34;

/// Per-ride pane height (two lines: name/date + stats), sized to fill the list above the footer.
const ROW_H: i32 = 66;

/// Left/right inset of the hold-to-delete footer's rule + contents.
const FOOTER_X: i32 = 12;

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

    /// Whether the highlighted ride's hold-to-delete footer is **live**: there is a ride to delete and
    /// no tracking session is running. The footer greys out entirely while recording — the in-progress
    /// ride's log (`TRACK.OBT`) and its not-yet-written `RD{id}.ORD` are the exact open handles
    /// embedded-sdmmc would refuse to delete, and the recording ride isn't even in the list until it's
    /// saved at Finish. So a single "no delete while recording" rule keeps every delete legal.
    fn delete_enabled(&self, activity: &Activity, len: usize) -> bool {
        len > 0 && self.selected < len && !activity.is_tracking()
    }

    /// True while the hold-to-delete footer would fill for the current highlight — so
    /// [`App::top_wants_hold_fill`](crate::App::top_wants_hold_fill) repaints a charging hold here.
    pub(crate) fn selection_is_deletable(&self, activity: &Activity, rides_len: usize) -> bool {
        self.delete_enabled(activity, rides_len)
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        let len = cx.rides.len();
        match g {
            Gesture::Turn(n) => list::on_turn(&mut self.selected, n, len),
            // A completed hold over a *deletable* highlighted ride requests its deletion — the guarded
            // hold is the confirmation (no popup), the footer bar its live feedback. Records the delete
            // by index; the host resolves it to the durable object id, deletes the object through the
            // store, and the store-changed rescan re-feeds the catalog (the remap keeps the highlight
            // sane). A hold while recording (greyed footer) does nothing.
            Gesture::Hold if self.delete_enabled(cx.activity, len) => {
                cx.activity.request_ride_delete(self.selected.min(len - 1));
                Transition::None
            }
            Gesture::Back => Transition::Pop, // return to the Menu
            // No `press` action: the Rides screen is see-and-delete, not a browser (locked). A tap on
            // a ride does nothing.
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        let rides = rx.rides;
        let total = rides.len();
        let units = rx.settings.units;
        // Reserve the footer band so a ride pane never draws under the hold-to-delete bar.
        let geo = ListGeometry::below_title(w, h - FOOTER_H, ROW_H, 8, 12, Separators::Unselected);

        let pos = if total == 0 { 0 } else { self.selected.min(total - 1) + 1 };
        list::list_frame(cv, w, h, "RIDES", pos, total, geo.visible);

        if total == 0 {
            super::empty_state(cv, w, h, "No rides yet", "Record a ride to see it here");
            return;
        }

        let sel = self.selected.min(total - 1);
        let first = list::window_start(sel, geo.visible, total) as i32;
        list::draw_rows(cv, geo, total, sel, first, |cv, row| {
            let ride = &rides[row.index];
            let y = row.area.top_left.y;
            let accent = if row.selected { INK } else { SUBTEXT };

            // Line one: the ride name (truncated with ".." — no ellipsis glyph), then its date pushed
            // to the right of the pane so name and date share the line.
            let date = fmt_date(ride.start_time);
            let date_w = date.chars().count() as i32 * Font::Label.char_width() as i32;
            let name_px = (w - 12) - date_w - 12 - 12; // pane right edge − date − gap − left inset
            let name_max = (name_px / Font::Body.char_width() as i32).max(6) as usize;
            let name = fit_name(&ride.name, name_max);
            cv.text(&name, Point::new(12, y + 9), Font::Body, TextAlign::Left, INK);
            cv.text(&date, Point::new(w - 12, y + 12), Font::Label, TextAlign::Right, accent);

            // Line two: distance + moving time on the left, climb right-aligned to the pane edge —
            // right-aligning the climb keeps a 5-digit metre figure from overrunning the pane. The
            // three columns are laid at fixed x's (not a single string) so they never collide the way
            // a packed left-run did on the narrow pane.
            let sy = y + 35;
            let mut dist: heapless::String<12> = heapless::String::new();
            write_distance(&mut dist, ride.distance_m, units);
            cv.text(&dist, Point::new(12, sy), Font::Label, TextAlign::Left, accent);
            let mut hms: heapless::String<10> = heapless::String::new();
            write_hms(&mut hms, ride.moving_time_s);
            cv.text(&hms, Point::new(w / 2 + 6, sy), Font::Label, TextAlign::Center, accent);
            let mut climb: heapless::String<16> = heapless::String::new();
            write_climb(&mut climb, ride.climb_m, units);
            cv.text(&climb, Point::new(w - 12, sy), Font::Label, TextAlign::Right, accent);
        });

        // The hold-to-delete footer over the highlighted ride: greyed while recording, warning-red
        // with a "not synced" cue for an unsynced ride, the standard footer otherwise.
        let synced = rides.get(sel).map(|r| r.synced).unwrap_or(true);
        delete_footer(cv, w, h, self.delete_enabled(rx.activity, total), synced, rx.hold_progress);
    }
}

/// Draw the hold-to-delete footer. `enabled` greys the whole footer (a dim trash + "Recording" hint)
/// while a ride is being recorded. When live, an **unsynced** ride adds a warning-red "not synced"
/// cue before the progress bar; a synced ride shows the plain trash + bar. The bar fills with the
/// live encoder hold. The delete itself fires from `handle`'s `Hold` arm.
fn delete_footer(cv: &mut impl Surface, w: i32, h: i32, enabled: bool, synced: bool, hold: f32) {
    use palette::*;
    let fy = h - FOOTER_H;
    cv.hline(FOOTER_X, fy, w - 2 * FOOTER_X, RULE);
    let midy = fy + FOOTER_H / 2;
    if !enabled {
        // Greyed while recording: a dim trash + a "Recording" hint so the disabled state reads
        // deliberately (the recording ride isn't listed and can't be deleted mid-session).
        draw_trash(cv, FOOTER_X + 16, midy, RULE);
        cv.text_vcentered("Recording", FOOTER_X + 36, (fy, FOOTER_H), Font::Label, TextAlign::Left, SUBTEXT);
        return;
    }
    let p = hold.clamp(0.0, 1.0);
    let mut bx = FOOTER_X + 36;
    if synced {
        draw_trash(cv, FOOTER_X + 16, midy, WARNING);
    } else {
        // Unsynced: a warning-red trash + a compact "not synced" cue, then the bar is pushed right of
        // it so the rider is told the ride isn't backed up before they hold to delete it.
        draw_trash(cv, FOOTER_X + 16, midy, WARNING);
        let cue = "not synced";
        cv.text_vcentered(cue, bx, (fy, FOOTER_H), Font::Label, TextAlign::Left, WARNING);
        bx += cue.chars().count() as i32 * Font::Label.char_width() as i32 + 8;
    }
    let bh = 12;
    let by = midy - bh / 2;
    let bw = w - FOOTER_X - 4 - bx;
    if bw > 8 {
        cv.round(rect(bx, by, bw, bh), 6, PARCHMENT_SHADE);
        let fill = (bw as f32 * p) as i32;
        if fill > 0 {
            cv.round(rect(bx, by, fill, bh), 6, WARNING);
        }
    }
}

/// Draw a small trash-can glyph centred at `(cx, cy)`. The Route menu's twin — kept local so the two
/// footers stay independent.
fn draw_trash(cv: &mut impl Surface, cx: i32, cy: i32, color: u16) {
    let (bw, bh) = (11, 12);
    let (bx, by) = (cx - bw / 2, cy - bh / 2 + 1);
    cv.round_outline(rect(bx, by, bw, bh), 2, color); // can body
    cv.hline(bx - 2, by - 2, bw + 4, color); // lid
    cv.hline(cx - 2, by - 4, 5, color); // handle
    cv.vline(cx - 2, by + 3, bh - 5, 1, color); // ribs
    cv.vline(cx + 2, by + 3, bh - 5, 1, color);
}

/// Format a ride's unix `start_time` as a compact `YYYY-MM-DD` (UTC). Kept short so name + date
/// share the first line. (Local-time formatting would need the app's UTC offset threaded in; the
/// date rarely differs and the extra plumbing isn't worth it for a see-and-delete list.)
fn fmt_date(start_time: u32) -> heapless::String<12> {
    let d = DateTime::from_unix(start_time);
    let mut s = heapless::String::new();
    let _ = write!(s, "{:04}-{:02}-{:02}", d.year, d.month, d.day);
    s
}

/// Append a compact distance in the rider's units: `NN.N km` / `NN.N mi`.
fn write_distance<const N: usize>(s: &mut heapless::String<N>, dist_m: u32, units: Units) {
    if units.is_imperial() {
        use crate::settings::{FT_PER_M, FT_PER_MI};
        let mi10 = (dist_m as f32 * FT_PER_M / FT_PER_MI as f32 * 10.0) as u32;
        let _ = write!(s, "{}.{} mi", mi10 / 10, mi10 % 10);
    } else {
        let km10 = (dist_m + 50) / 100; // tenths of a km
        let _ = write!(s, "{}.{} km", km10 / 10, km10 % 10);
    }
}

/// Append total ascent in the rider's units: `NNNN m` / `NNNN ft`, prefixed by an up-arrow the panel
/// font lacks, so a plain `^`.
fn write_climb<const N: usize>(s: &mut heapless::String<N>, climb_m: u16, units: Units) {
    if units.is_imperial() {
        use crate::settings::FT_PER_M;
        let ft = (climb_m as f32 * FT_PER_M) as u32;
        let _ = write!(s, "^{ft} ft");
    } else {
        let _ = write!(s, "^{climb_m} m");
    }
}

/// Append a moving time as `H:MM` (or `M:SS` under an hour) — compact for the stats line.
fn write_hms<const N: usize>(s: &mut heapless::String<N>, secs: u32) {
    let (h, m, sec) = (secs / 3600, secs % 3600 / 60, secs % 60);
    if h > 0 {
        let _ = write!(s, "{h}:{m:02}");
    } else {
        let _ = write!(s, "{m}:{sec:02}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Mode;
    use crate::ride::RideSummary;
    use crate::{AppState, Settings};

    fn summary(name: &str, synced: bool) -> RideSummary {
        RideSummary {
            name: heapless::String::try_from(name).unwrap(),
            start_time: 1_720_000_000,
            distance_m: 42_500,
            moving_time_s: 3 * 3600 + 12 * 60,
            climb_m: 640,
            synced,
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
            nav_profiles: &crate::NavProfiles::EMPTY,
            poi_scratch: &scratch,
            now_ms: 0,
        };
        scr.handle(g, &mut cx)
    }

    /// A completed hold over the highlighted ride records a delete request for **that** row's index —
    /// the durable id resolution is `App::take_ride_delete`'s job.
    #[test]
    fn hold_records_delete_for_the_highlighted_ride() {
        let rides = [summary("A", true), summary("B", false), summary("C", true)];
        let mut act = Activity::new(Mode::Idle);
        let mut scr = RidesScreen::new();
        run(&mut scr, &mut act, &rides, Gesture::Turn(1)); // highlight row 1 ("B")
        assert_eq!(scr.selected, 1);
        let t = run(&mut scr, &mut act, &rides, Gesture::Hold);
        assert!(matches!(t, Transition::None), "the hold stays on the screen");
        assert_eq!(act.take_ride_delete(), Some(1), "the highlighted ride's index is requested");
    }

    /// The footer is greyed — and a hold does nothing — while a ride is being recorded (a tracking
    /// session is live). Ending the session re-enables the footer.
    #[test]
    fn hold_is_a_no_op_while_recording() {
        let rides = [summary("A", true), summary("B", true)];
        let mut act = Activity::new(Mode::Riding);
        act.start_session(); // now tracking
        let mut scr = RidesScreen::new();

        assert!(!scr.selection_is_deletable(&act, rides.len()), "the footer is greyed while recording");
        run(&mut scr, &mut act, &rides, Gesture::Hold);
        assert_eq!(act.take_ride_delete(), None, "a hold while recording records nothing");

        act.end_session();
        assert!(scr.selection_is_deletable(&act, rides.len()), "ending the ride re-enables the footer");
        run(&mut scr, &mut act, &rides, Gesture::Hold);
        assert_eq!(act.take_ride_delete(), Some(0));
    }

    /// An empty catalog offers no delete.
    #[test]
    fn empty_catalog_has_no_delete() {
        let mut act = Activity::new(Mode::Idle);
        let mut scr = RidesScreen::new();
        assert!(!scr.selection_is_deletable(&act, 0));
        run(&mut scr, &mut act, &[], Gesture::Hold);
        assert_eq!(act.take_ride_delete(), None);
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
}
