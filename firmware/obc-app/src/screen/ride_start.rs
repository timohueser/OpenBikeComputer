//! The route-less **start card** — the small confirm opened by pressing the browse Map (the Menu's
//! Map station reached without a route). Two rows: **Start ride** begins a tracking session with no
//! route attached, **Back** returns to the browse map.
//!
//! Starting is a plain press, not a hold: it's reversible (the Paused page's Discard throws the
//! fresh session away), so it needs no guard. *Start ride* takes the shared
//! [`start_ride_routeless`](super::start_ride_routeless) path — the same session-begin the Route
//! overview's START RIDE runs, minus the route load — landing on the clean `[Home, Map]` stack.

use obc_render::Surface;

use crate::input::Gesture;
use crate::Msg;

use super::{list, palette, title_frame, Ctx, MenuItem, Render, Transition};

/// The two option rows (Start ride / Back), neither guarded — labels looked up per language at draw
/// time (see [`RideStartScreen::draw`]).
const N_ITEMS: usize = 2;

const START: usize = 0;

/// The start card. State is just the highlighted option.
#[derive(Debug, Default)]
pub struct RideStartScreen {
    selected: usize,
}

impl RideStartScreen {
    pub fn new() -> Self {
        RideStartScreen { selected: 0 }
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Turn(n) => list::on_turn(&mut self.selected, n, N_ITEMS),
            Gesture::Press => match self.selected {
                // Begin a route-less tracking session and root the stack to [Home, Map] — the same
                // clean landing the Route overview's START RIDE does, minus the route.
                START => super::start_ride_routeless(cx),
                _ => Transition::Pop, // Back
            },
            Gesture::Back => Transition::Pop, // back = Back (return to the browse map)
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);

        // Opaque full-screen card (matching the Route-swap prompt's chrome): the title bar, then two
        // option rows. No explainer line — the two labels say all there is to say.
        title_frame(cv, w, h, rx.t(Msg::RideStartTitle), "");

        let geo = super::GuardedRowsGeometry {
            x: 12,
            w: w - 24,
            top: super::TITLE_BAR_H + 46,
            row_h: 46,
            gap: 8,
            label_dx: 16,
            label_dy: 11,
        };
        let items = [
            MenuItem { label: rx.t(Msg::RideStartStartRide), guard: false },
            MenuItem { label: rx.t(Msg::RideStartBack), guard: false },
        ];
        super::draw_guarded_rows(cv, &items, self.selected, rx.hold_progress, AMBER, geo);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::{Activity, Mode};
    use crate::screen::Screen;
    use crate::{AppState, Settings};

    fn run(scr: &mut RideStartScreen, st: &mut AppState, act: &mut Activity, g: Gesture) -> Transition {
        let mut settings = Settings::default();
        let scratch = crate::screen::PoiScratch::new();
        let mut cx = Ctx {
            state: st,
            activity: act,
            settings: &mut settings,
            routes: &[],
            rides: &[],
            nav_profiles: &crate::NavProfiles::EMPTY,
            poi_scratch: &scratch,
            now_ms: 0,
        };
        scr.handle(g, &mut cx)
    }

    /// "Start ride" begins a **route-less** tracking session — riding mode, a fresh session, no
    /// `active_route` — and roots the stack to a clean `[Home, Map]` (the START RIDE precedent).
    #[test]
    fn start_begins_a_route_less_session_and_roots_the_stack() {
        let mut st = AppState::new(0, 0, 1.0);
        let mut act = Activity::new(Mode::Idle);
        let mut scr = RideStartScreen::new(); // selection starts on "Start ride"
        let t = run(&mut scr, &mut st, &mut act, Gesture::Press);
        assert!(matches!(t, Transition::Root(Screen::Map(_))), "start roots to [Home, Map]");
        assert_eq!(act.mode, Mode::Riding, "start enters riding mode");
        assert!(act.is_tracking(), "a fresh tracking session is open");
        assert_eq!(act.active_route, None, "no route is attached — this is a route-less ride");
    }

    /// The "Back" row (and the `back` gesture) pops to the browse map without starting anything.
    #[test]
    fn back_pops_without_starting() {
        let mut st = AppState::new(0, 0, 1.0);
        let mut act = Activity::new(Mode::Idle);
        let mut scr = RideStartScreen::new();
        run(&mut scr, &mut st, &mut act, Gesture::Turn(1)); // highlight "Back"
        let t = run(&mut scr, &mut st, &mut act, Gesture::Press);
        assert!(matches!(t, Transition::Pop), "Back pops");
        assert!(!act.is_tracking(), "nothing started");

        // The `back` gesture pops from any selection, too.
        let mut scr = RideStartScreen::new();
        let t = run(&mut scr, &mut st, &mut act, Gesture::Back);
        assert!(matches!(t, Transition::Pop));
        assert!(!act.is_tracking());
    }
}
