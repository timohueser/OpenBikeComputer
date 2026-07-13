//! The trip **cascade-delete** confirm dialog (epic #526, TR3). Reached from the Route menu's top
//! level by long-pressing a trip folder row; confirming here deletes the trip **and every route
//! inside it** (locked: the on-device delete cascades — it's post-trip cleanup, not an ungroup).
//!
//! Modelled on the [`RouteSwapScreen`](super::RouteSwapScreen) guarded-action family: an opaque
//! full-frame card naming the trip, with a warning-red hold-guarded **Delete** row and a plain
//! **Cancel** row. The guarded hold is the exact idiom the Route overview's Delete-route row uses —
//! a completed [`Gesture::Hold`] with the Delete row selected records the request. The screen holds
//! only the trip's **durable object id** (its own device counter), so a catalog rescan racing the
//! confirm can't retarget it: the id is drained verbatim by
//! [`App::take_trip_delete`](crate::App::take_trip_delete) and the host cascade-deletes the
//! `TP{id}.OBT` + member route files, then rescans + re-feeds — the folder disappears and the menu
//! regroups. Back / Cancel pops to the top level, leaving the trip intact.

use embedded_graphics::prelude::Point;
use obc_render::{
    text::{Font, TextAlign},
    Surface,
};

use obc_route::NAME_CAP;

use crate::input::Gesture;
use crate::Msg;

use super::{list, palette, title_frame, Ctx, MenuItem, Render, Transition};

/// Per-row guard flags: only *Delete trip & routes* is destructive.
const GUARDS: [bool; 2] = [true, false];

const DELETE: usize = 0;
const CANCEL: usize = 1;

/// The confirm dialog. Carries the trip's durable object id (what the host deletes), its name (for
/// the card body), and the highlighted option.
#[derive(Debug)]
pub struct TripDeleteScreen {
    /// The trip's durable object id — drained verbatim by
    /// [`App::take_trip_delete`](crate::App::take_trip_delete).
    trip_id: u16,
    name: heapless::String<NAME_CAP>,
    selected: usize,
}

impl TripDeleteScreen {
    /// A confirm for the trip with durable id `trip_id` and display `name`. Entry selects the guarded
    /// Delete row's *neighbour* — the cursor starts on Cancel so an accidental double-hold on the way
    /// in can't delete; the rider turns onto Delete deliberately, then holds (mirrors the Route
    /// overview / Pause-menu idiom, where entry never lands armed on the destructive row).
    pub fn new(trip_id: u16, name: &str) -> Self {
        let mut n = heapless::String::new();
        let _ = n.push_str(fit_to_cap(name));
        TripDeleteScreen { trip_id, name: n, selected: CANCEL }
    }

    /// True while the highlighted option needs a hold: its row fills with the live hold progress in
    /// `draw`, so [`App::top_wants_hold_fill`](crate::App::top_wants_hold_fill) reports a charging
    /// hold as worth repainting here.
    pub fn selection_is_guarded(&self) -> bool {
        GUARDS[self.selected.min(GUARDS.len() - 1)]
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Turn(n) => list::on_turn(&mut self.selected, n, GUARDS.len()),
            // Cancel is a plain press; Delete is guarded (a press does nothing — it takes a hold).
            Gesture::Press if self.selected == CANCEL => Transition::Pop,
            Gesture::Hold if self.selected == DELETE => {
                // Record the cascade-delete against the trip's durable id and pop back to the top
                // level. The host drains it, deletes the `TP{id}.OBT` + member routes, rescans, and
                // re-feeds — the folder is gone and the menu regroups on the next draw.
                cx.activity.request_trip_delete(self.trip_id);
                Transition::Pop
            }
            Gesture::Back => Transition::Pop, // back = Cancel (keep the trip)
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);

        // Opaque full-frame prompt: the trip name under a DELETE TRIP title, a one-line warning, then
        // the two option rows.
        title_frame(cv, w, h, rx.t(Msg::TripDeleteTitle), "");

        // The trip name, centred, truncated with ".." to the card width (no ellipsis glyph).
        let max = (((w - 24) / Font::Body.char_width() as i32).max(6)) as usize;
        let name = super::route_menu::fit_name(&self.name, max);
        cv.text(&name, Point::new(w / 2, super::TITLE_BAR_H + 12), Font::Body, TextAlign::Center, INK);

        // The warning line — what the confirm actually does (deletes the routes too), word-wrapped in
        // the olive sub-text so the longer translations don't clip. Returns the y past the last line.
        let warn_end =
            super::wrapped(cv, rx.t(Msg::TripDeleteWarn), w / 2, super::TITLE_BAR_H + 40, w - 24, Font::Label, SUBTEXT);

        // The guarded Delete row fills warning-red (this IS destructive); Cancel is a plain amber row.
        let geo = super::GuardedRowsGeometry {
            x: 12,
            w: w - 24,
            top: warn_end + 8,
            row_h: 46,
            gap: 8,
            label_dx: 16,
            label_dy: 11,
        };
        let items = [
            MenuItem { label: rx.t(Msg::TripDeleteConfirm), guard: GUARDS[0] },
            MenuItem { label: rx.t(Msg::TripDeleteCancel), guard: GUARDS[1] },
        ];
        super::draw_guarded_rows(cv, &items, self.selected, rx.hold_progress, WARNING, geo);
    }
}

/// The longest prefix of `s` that fits [`NAME_CAP`] bytes without splitting a multi-byte char — the
/// name is copied into the screen's own buffer, so a longer scanned name can't overflow it.
fn fit_to_cap(s: &str) -> &str {
    let mut end = s.len().min(NAME_CAP);
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::{Activity, Mode};
    use crate::{AppState, Settings};

    fn run(scr: &mut TripDeleteScreen, act: &mut Activity, g: Gesture) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut settings = Settings::default();
        let scratch = crate::screen::PoiScratch::new();
        let mut cx = Ctx {
            state: &mut st,
            activity: act,
            settings: &mut settings,
            routes: &[],
            rides: &[],
            trips: &[],
            nav_profiles: &crate::NavProfiles::EMPTY,
            poi_scratch: &scratch,
            sensor_scan_hits: &[],
            now_ms: 0,
        };
        scr.handle(g, &mut cx)
    }

    /// Entry lands on Cancel (never armed on the destructive row); a hold there does nothing.
    #[test]
    fn entry_is_not_armed_on_delete() {
        let mut scr = TripDeleteScreen::new(7, "Alpen Traverse");
        assert!(!scr.selection_is_guarded(), "entry selects Cancel — nothing armed");
        let mut act = Activity::new(Mode::Idle);
        let t = run(&mut scr, &mut act, Gesture::Hold);
        assert!(matches!(t, Transition::None), "a hold on Cancel does nothing");
        assert_eq!(act.take_trip_delete(), None);
    }

    /// A completed hold with the cursor on the Delete row records the trip's durable id and pops.
    #[test]
    fn hold_on_delete_records_the_trip_id_and_pops() {
        let mut scr = TripDeleteScreen::new(7, "Alpen Traverse");
        run(&mut scr, &mut Activity::new(Mode::Idle), Gesture::Turn(1)); // Cancel → Delete
        assert!(scr.selection_is_guarded(), "the hold fill is live on the Delete row");
        let mut act = Activity::new(Mode::Idle);
        let t = run(&mut scr, &mut act, Gesture::Hold);
        assert!(matches!(t, Transition::Pop), "the delete pops back to the top level");
        assert_eq!(act.take_trip_delete(), Some(7), "records the trip's durable id verbatim");
    }

    /// A plain press on Cancel pops without recording anything.
    #[test]
    fn cancel_pops_without_deleting() {
        let mut scr = TripDeleteScreen::new(7, "Alpen Traverse");
        let mut act = Activity::new(Mode::Idle);
        let t = run(&mut scr, &mut act, Gesture::Press);
        assert!(matches!(t, Transition::Pop));
        assert_eq!(act.take_trip_delete(), None, "Cancel never records a delete");
    }
}
