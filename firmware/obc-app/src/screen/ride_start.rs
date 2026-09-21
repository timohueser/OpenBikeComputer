//! The route-less start card, opened by a press on the browse Map. It stacks the hero sprite of
//! the selected bike, its profile name, a GPS and battery checklist, and the two option rows.
//! Start ride begins a tracking session with no route attached, and Back returns to the map.
//!
//! Starting is a press, not a hold, because it is reversible: Discard on the Paused page throws
//! the new session away.

use core::fmt::Write as _;

use embedded_graphics::prelude::Point;
use obc_render::{
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::Msg;

use super::settings::bike_icons;
use super::vocab::card::{ActionRows, CardEvent};
use super::vocab::chrome::{title_frame, TITLE_BAR_H};
use super::vocab::rows::{GuardedRowsGeometry, MenuItem};
use super::{palette, Ctx, Render, Transition};

/// Top of the hero bike, just under the title bar.
const HERO_TOP: i32 = TITLE_BAR_H + 8;
/// The art-pixel scale of the hero bike. It stays small, because the rows below it come first.
const HERO_SCALE: i32 = 2;
/// Top of the profile name (olive Label, centred) just below the hero.
const NAME_TOP: i32 = 108;
/// Top of the first checklist row, the row pitch, and the inset of the label and the value. The
/// values right-align to `w - CHECK_INSET_X`, the one shared value column.
const CHECK_TOP: i32 = 164;
const CHECK_ROW_H: i32 = 26;
const CHECK_INSET_X: i32 = 20;
/// Top of the Start ride / Back option block, anchored so the two rows clear the bottom edge.
const OPT_TOP: i32 = 222;

/// The two option rows are not guarded.
const ACTION_GUARDS: [bool; 2] = [false; 2];

const START: usize = 0;

#[derive(Debug, Default)]
pub struct RideStartScreen {
    actions: ActionRows,
}

impl RideStartScreen {
    pub fn new() -> Self {
        RideStartScreen { actions: ActionRows::new(0) }
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match self.actions.handle(g, &ACTION_GUARDS) {
            CardEvent::Activate(START) => super::start_ride_routeless(cx),
            CardEvent::Activate(_) | CardEvent::Dismiss => Transition::Pop,
            CardEvent::None => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);

        title_frame(cv, w, h, rx.t(Msg::RideStartTitle), "");

        // A device without a map has no profiles. `for_name("")` then gives the generic bike in
        // plain ink, and the name draws empty.
        let marked = rx.nav_profiles.effective(rx.settings.bike_profile_idx);
        let name = rx.nav_profiles.name(marked).unwrap_or("");
        bike_icons::draw(cv, bike_icons::for_name(name), w / 2, HERO_TOP, HERO_SCALE, bike_icons::color_for(name));
        cv.text(name, Point::new(w / 2, NAME_TOP), Font::Label, TextAlign::Center, SUBTEXT);

        let mut batt: heapless::String<8> = heapless::String::new();
        let _ = write!(batt, "{}%", rx.state.device.battery_pct);
        let gps = if rx.no_fix { rx.t(Msg::RideStartSearching) } else { rx.t(Msg::RideStartFix) };
        let rows = [(rx.t(Msg::RideStartGps), gps), (rx.t(Msg::RideStartBattery), batt.as_str())];
        for (i, (label, value)) in rows.into_iter().enumerate() {
            let top = CHECK_TOP + i as i32 * CHECK_ROW_H;
            cv.text_vcentered(label, CHECK_INSET_X, (top, CHECK_ROW_H), Font::Label, TextAlign::Left, SUBTEXT);
            cv.text_vcentered(value, w - CHECK_INSET_X, (top, CHECK_ROW_H), Font::Label, TextAlign::Right, INK);
        }

        let geo = GuardedRowsGeometry { x: 12, w: w - 24, top: OPT_TOP, row_h: 42, gap: 8, label_dx: 16, label_dy: 10 };
        let items = [
            MenuItem { label: rx.t(Msg::RideStartStartRide), guard: ACTION_GUARDS[0] },
            MenuItem { label: rx.t(Msg::RideStartBack), guard: ACTION_GUARDS[1] },
        ];
        self.actions.draw(cv, &items, rx.hold_progress, AMBER, geo);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::{Activity, Mode};
    use crate::screen::test_ctx;
    use crate::screen::Screen;
    use crate::{AppState, Settings};

    fn run(
        scr: &mut RideStartScreen,
        st: &mut AppState,
        act: &mut Activity,
        rec: &mut crate::RecorderMachine,
        navigator: &mut crate::navigator::NavigatorMachine,
        g: Gesture,
    ) -> Transition {
        let mut settings = Settings::default();
        let mut cx = Ctx { recorder: rec, navigator, ..test_ctx(st, act, &mut settings) };
        scr.handle(g, &mut cx)
    }

    #[test]
    fn start_begins_a_route_less_session_and_roots_the_stack() {
        let mut rec = crate::RecorderMachine::new();
        let mut st = AppState::new(0, 0, 1.0);
        let mut act = Activity::new(Mode::Idle);
        let mut navigator = crate::navigator::NavigatorMachine::new();
        let mut scr = RideStartScreen::new(); // the selection starts on "Start ride"
        let t = run(&mut scr, &mut st, &mut act, &mut rec, &mut navigator, Gesture::Press);
        assert!(matches!(t, Transition::Root(Screen::Map(_))), "start roots to [Home, Map]");
        assert_eq!(act.mode, Mode::Riding, "start enters riding mode");
        assert_eq!(rec.test_take_intent(), Some(crate::RecorderIntent::Start), "the ride is named to Recorder");
        assert_eq!(navigator.route_state().active_route, None, "no route is attached — this is a route-less ride");
    }

    #[test]
    fn back_pops_without_starting() {
        let mut rec = crate::RecorderMachine::new();
        let mut st = AppState::new(0, 0, 1.0);
        let mut act = Activity::new(Mode::Idle);
        let mut navigator = crate::navigator::NavigatorMachine::new();
        let mut scr = RideStartScreen::new();
        run(&mut scr, &mut st, &mut act, &mut rec, &mut navigator, Gesture::Step(1)); // highlight "Back"
        let t = run(&mut scr, &mut st, &mut act, &mut rec, &mut navigator, Gesture::Press);
        assert!(matches!(t, Transition::Pop), "Back pops");
        assert!(!rec.recording(), "nothing started");

        // The back gesture pops from any selection, too.
        let mut scr = RideStartScreen::new();
        let t = run(&mut scr, &mut st, &mut act, &mut rec, &mut navigator, Gesture::Back);
        assert!(matches!(t, Transition::Pop));
        assert!(!rec.recording());
    }
}
