//! The confirm dialog for a trip delete, reached by a long press on a trip folder row. The delete
//! cascades: it removes the trip and every route in it.
//!
//! The screen holds only the durable object id of the trip, so a catalog rescan that races the
//! confirm cannot retarget it. A completed hold on the Delete row records the request, the host
//! deletes the trip file and its member routes, and the menu regroups on the next scan.

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use obc_formats::obcr::NAME_CAP;

use crate::input::Gesture;
use crate::Msg;

use super::vocab::card::{ActionRows, CardEvent};
use super::vocab::chrome::{title_frame, wrapped, TITLE_BAR_H};
use super::vocab::rows::{GuardedRowsGeometry, MenuItem};
use super::{palette, Ctx, Render, Transition};

/// Per-row guard flags: only *Delete trip & routes* is destructive.
const GUARDS: [bool; 2] = [true, false];

const DELETE: usize = 0;
const CANCEL: usize = 1;

#[derive(Debug)]
pub struct TripDeleteScreen {
    /// The durable object id of the trip, which the host drains verbatim.
    trip_id: crate::CatalogObjectId,
    name: heapless::String<NAME_CAP>,
    actions: ActionRows,
}

impl TripDeleteScreen {
    /// A confirm for the trip with durable id `trip_id`. The cursor starts on Cancel, so a second
    /// hold on the way in cannot delete.
    pub fn new(trip_id: crate::CatalogObjectId, name: &str) -> Self {
        let mut n = heapless::String::new();
        let _ = n.push_str(fit_to_cap(name));
        TripDeleteScreen { trip_id, name: n, actions: ActionRows::new(CANCEL) }
    }

    /// True when the highlighted row fills for a hold, which makes the app repaint that fill.
    pub fn selection_is_guarded(&self) -> bool {
        self.actions.selection_is_guarded(&GUARDS)
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        // Cancel answers a hold as well as a press. Keep this screen-specific rule before the
        // shared controller; unguarded holds are otherwise inert.
        if matches!(g, Gesture::Hold) && !self.actions.selection_is_guarded(&GUARDS) {
            return Transition::Pop;
        }

        match self.actions.handle(g, &GUARDS) {
            CardEvent::Activate(DELETE) => {
                cx.activity.request_trip_delete(self.trip_id);
                Transition::Pop
            }
            CardEvent::Activate(CANCEL) | CardEvent::Dismiss => Transition::Pop,
            CardEvent::Activate(_) | CardEvent::None => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);

        title_frame(cv, w, h, rx.t(Msg::TripDeleteTitle), "");

        let max = (((w - 24) / Font::Body.char_width() as i32).max(6)) as usize;
        let name_row = rect(12, TITLE_BAR_H + 12, w - 24, Font::Body.line_height() as i32);
        let name = rx.marquee.fit(&self.name, max, Some(name_row));
        cv.text(&name, Point::new(w / 2, TITLE_BAR_H + 12), Font::Body, TextAlign::Center, INK);

        // The warning line wraps, so the longer translations do not clip. It returns the y below
        // the last line.
        let warn_end = wrapped(cv, rx.t(Msg::TripDeleteWarn), w / 2, TITLE_BAR_H + 40, w - 24, Font::Label, SUBTEXT);

        let geo = GuardedRowsGeometry::card(w, warn_end + 8);
        let items = [
            MenuItem { label: rx.t(Msg::TripDeleteConfirm), guard: GUARDS[0] },
            MenuItem { label: rx.t(Msg::TripDeleteCancel), guard: GUARDS[1] },
        ];
        self.actions.draw(cv, &items, rx.hold_progress, WARNING, geo);
    }
}

/// The longest prefix of `s` that fits [`NAME_CAP`] bytes and splits no multi-byte character.
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
    use crate::screen::test_ctx;
    use crate::{AppState, Settings};

    fn run(scr: &mut TripDeleteScreen, act: &mut Activity, g: Gesture) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut settings = Settings::default();
        let mut cx = test_ctx(&mut st, act, &mut settings);
        scr.handle(g, &mut cx)
    }

    #[test]
    fn entry_is_not_armed_on_delete() {
        let mut scr = TripDeleteScreen::new(7, "Alpen Traverse");
        assert!(!scr.selection_is_guarded(), "entry selects Cancel — nothing armed");
        let mut act = Activity::new(Mode::Idle);
        let t = run(&mut scr, &mut act, Gesture::Hold);
        assert!(matches!(t, Transition::Pop), "a hold on Cancel cancels (pops)");
        assert_eq!(act.take_trip_delete(), None);
    }

    #[test]
    fn hold_on_delete_records_the_trip_id_and_pops() {
        let mut scr = TripDeleteScreen::new(7, "Alpen Traverse");
        run(&mut scr, &mut Activity::new(Mode::Idle), Gesture::Step(1)); // Cancel → Delete
        assert!(scr.selection_is_guarded(), "the hold fill is live on the Delete row");
        let mut act = Activity::new(Mode::Idle);
        let t = run(&mut scr, &mut act, Gesture::Hold);
        assert!(matches!(t, Transition::Pop), "the delete pops back to the top level");
        assert_eq!(act.take_trip_delete(), Some(7), "records the trip's durable id verbatim");
    }

    #[test]
    fn cancel_pops_without_deleting() {
        let mut scr = TripDeleteScreen::new(7, "Alpen Traverse");
        let mut act = Activity::new(Mode::Idle);
        let t = run(&mut scr, &mut act, Gesture::Press);
        assert!(matches!(t, Transition::Pop));
        assert_eq!(act.take_trip_delete(), None, "Cancel never records a delete");
    }
}
