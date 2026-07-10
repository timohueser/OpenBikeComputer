//! The route-less **start card** — the pre-ride launchpad opened by pressing the browse Map (the
//! Menu's Map station reached without a route). A small stack (T6, #684): the selected bike's hero
//! sprite + its profile name, a three-row pre-ride checklist (GPS / Battery / Card), then the two
//! option rows — **Start ride** begins a tracking session with no route attached, **Back** returns
//! to the browse map.
//!
//! Starting is a plain press, not a hold: it's reversible (the Paused page's Discard throws the
//! fresh session away), so it needs no guard. *Start ride* takes the shared
//! [`start_ride_routeless`](super::start_ride_routeless) path — the same session-begin the Route
//! overview's START RIDE runs, minus the route load — landing on the clean `[Home, Map]` stack.

use core::fmt::Write as _;

use embedded_graphics::prelude::Point;
use obc_render::{
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::Msg;

use super::settings::bike_icons;
use super::{list, palette, title_frame, Ctx, MenuItem, Render, Transition};

/// Top of the hero bike, just under the title bar.
const HERO_TOP: i32 = super::TITLE_BAR_H + 8;
/// Art-pixel scale for the hero bike. 2× (→ 100×60 device px), smaller than the Bike-type screen's
/// own 4× hero: that hero fills a screen with one row under it, but this card stacks five content
/// rows (name + three checklist + two options) below it, so it renders the same sprite at half scale
/// rather than cram the rows — the D5 mockup gate's "protect the rows/fonts over the hero" call.
const HERO_SCALE: i32 = 2;
/// Top of the profile name (olive Label, centred) just below the hero.
const NAME_TOP: i32 = 108;
/// Top of the first checklist row, the per-row pitch, and the label/value inset from each edge (the
/// values right-align to `w - CHECK_INSET_X`, the one shared value column).
const CHECK_TOP: i32 = 138;
const CHECK_ROW_H: i32 = 26;
const CHECK_INSET_X: i32 = 20;
/// Top of the Start ride / Back option block, anchored so the two rows clear the bottom edge.
const OPT_TOP: i32 = 222;

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

        // Opaque full-screen card (matching the Route-swap prompt's chrome): the title bar over the
        // hero + checklist + option rows.
        title_frame(cv, w, h, rx.t(Msg::RideStartTitle), "");

        // Hero: the selected profile's pixel bike — the same sprite + hinting colour the Bike-type
        // screen draws (matched by name) — with its profile name centred just below in olive Label.
        // A router-less / no-map device has no profiles: `for_name("")` falls back to the generic
        // bike in plain ink and the name draws empty.
        let marked = rx.nav_profiles.effective(rx.settings.bike_profile_idx);
        let name = rx.nav_profiles.name(marked).unwrap_or("");
        bike_icons::draw(cv, bike_icons::for_name(name), w / 2, HERO_TOP, HERO_SCALE, bike_icons::color_for(name));
        cv.text(name, Point::new(w / 2, NAME_TOP), Font::Label, TextAlign::Center, SUBTEXT);

        // Checklist: three plain rows, label (olive) at the left inset, value (ink) right-aligned at
        // the one shared value column. GPS reads its live fix state (the map chrome's `no_fix`);
        // Battery the same `NN%` Home shows; Card is static `OK` — the card is mounted (this screen is
        // unreachable without it), so it reassures without a live probe.
        let mut batt: heapless::String<8> = heapless::String::new();
        let _ = write!(batt, "{}%", rx.state.battery_pct);
        let gps = if rx.no_fix { rx.t(Msg::RideStartSearching) } else { rx.t(Msg::RideStartFix) };
        let rows = [
            (rx.t(Msg::RideStartGps), gps),
            (rx.t(Msg::RideStartBattery), batt.as_str()),
            (rx.t(Msg::RideStartCard), rx.t(Msg::RideStartCardOk)),
        ];
        for (i, (label, value)) in rows.into_iter().enumerate() {
            let top = CHECK_TOP + i as i32 * CHECK_ROW_H;
            cv.text_vcentered(label, CHECK_INSET_X, (top, CHECK_ROW_H), Font::Label, TextAlign::Left, SUBTEXT);
            cv.text_vcentered(value, w - CHECK_INSET_X, (top, CHECK_ROW_H), Font::Label, TextAlign::Right, INK);
        }

        // Options: Start ride (amber primary) / Back — unchanged behaviour, anchored at the bottom.
        let geo = super::GuardedRowsGeometry {
            x: 12,
            w: w - 24,
            top: OPT_TOP,
            row_h: 42,
            gap: 8,
            label_dx: 16,
            label_dy: 10,
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
