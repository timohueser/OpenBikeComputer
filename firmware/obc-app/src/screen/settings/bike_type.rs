//! The Bike type screen — picks the routing profile the on-device planner weights edges by
//! (routing-v2 N5, epic #533). The choice is a bare **index** into the loaded map's §8.6 profile
//! table ([`Settings::bike_profile_idx`](crate::Settings)); this screen cycles it through the map's
//! profile **names** ([`NavProfiles`](crate::NavProfiles)), so a custom web-builder profile shows up
//! automatically — no hardcoded list. A single value row that a press or a turn steps in place, like
//! the Units screen, only over N names instead of two.
//!
//! Inert without a map: an `ble` image ships without the router, and a fresh boot has no map loaded,
//! so [`NavProfiles`] is empty — the row then renders the `Profile N` fallback and cycling is a
//! no-op (there is nothing to cycle to). The setting still persists; it just does nothing until a
//! map with profiles is present, mirroring how the other nav-only UI degrades there.

use embedded_graphics::prelude::Point;
use obc_render::{
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::screen::{palette, title_frame, Ctx, Render, Transition, LIST_TOP};

/// The Bike type screen. Stateless — the value lives in [`Settings`](crate::Settings) and the name
/// list in the App's [`NavProfiles`](crate::NavProfiles); the one row is always the cursor.
#[derive(Debug, Default)]
pub struct BikeTypeScreen;

impl BikeTypeScreen {
    pub fn new() -> Self {
        BikeTypeScreen
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            // Press or a turn steps the selection (press = one step forward), wrapping through the
            // loaded map's profile names. A no-op when the map carries zero or one profile — nothing
            // to cycle to — so a router-less / no-map device just leaves the index at 0.
            Gesture::Press | Gesture::Turn(_) => {
                let count = cx.nav_profiles.len();
                if count > 1 {
                    let step = if let Gesture::Turn(n) = g { n } else { 1 };
                    let cur = cx.settings.bike_profile_idx as i32;
                    cx.settings.bike_profile_idx = (cur + step).rem_euclid(count as i32) as u8;
                }
                Transition::None
            }
            Gesture::Back => Transition::Pop,
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, "BIKE TYPE", "");

        let idx = rx.settings.bike_profile_idx;
        let count = rx.nav_profiles.len();

        // The single value row — the current profile name centred, flanked by left/right arrows so it
        // reads as "rotate to switch" (the Units screen's affordance). The name resolves against the
        // loaded map; an out-of-range index (a stale setting on a smaller map) shows the honest
        // `Profile N` fallback, since routing will fall back to profile 0.
        let area = super::row_rect(LIST_TOP + 8, w, 50);
        super::row_cursor(cv, area, true, false);
        let midy = area.top_left.y + area.size.height as i32 / 2;
        let mut label: heapless::String<20> = heapless::String::new();
        rx.nav_profiles.write_label(idx, &mut label);
        cv.text_vcentered(&label, w / 2, (area.top_left.y, 50), Font::Body, TextAlign::Center, INK);
        // ◄ and ► as filled triangles, inset from the row edges — drawn only when there's more than
        // one profile to move between (a single- or no-profile map has nowhere to go).
        if count > 1 {
            let ax = area.top_left.x + 18;
            cv.triangle(Point::new(ax, midy - 9), Point::new(ax, midy + 9), Point::new(ax - 11, midy), INK);
            let bx = area.top_left.x + area.size.width as i32 - 18;
            cv.triangle(Point::new(bx, midy - 9), Point::new(bx, midy + 9), Point::new(bx + 11, midy), INK);
        }

        // The full profile list below, current row highlighted — context for what the map offers, and
        // the "N of M" sense without a separate counter. When no map profiles are resident the row is
        // inert, so say so instead of listing nothing.
        if count == 0 {
            super::empty_state(cv, w, h, "No map profiles", "Load a map to pick a bike type");
            return;
        }
        let mut ry = LIST_TOP + 96;
        for i in 0..count {
            let name = rx.nav_profiles.name(i as u8).unwrap_or("");
            let selected = i as u8 == idx;
            let color = if selected { INK } else { SUBTEXT };
            let x = area.top_left.x + 12;
            if selected {
                // A small right-pointing wedge marks the active profile (the panel font has no
                // bullet glyph; the nav-list rows use the same drawn-marker idiom).
                let my = ry + 9;
                cv.triangle(Point::new(x, my - 7), Point::new(x, my + 7), Point::new(x + 9, my), INK);
            }
            cv.text(name, Point::new(x + 18, ry), Font::Body, TextAlign::Left, color);
            ry += 34;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Activity;
    use crate::nav_profiles::NavProfiles;
    use crate::{AppState, Mode, Settings};

    /// A [`NavProfiles`] with the given names, for a handle `Ctx` (the multiplier arrays the router
    /// reads are the map's, not the UI's — the screen only needs the names).
    fn profiles(names: &[&str]) -> NavProfiles {
        NavProfiles::from_names(names)
    }

    fn run(scr: &mut BikeTypeScreen, s: &mut Settings, profs: &NavProfiles, g: Gesture) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut act = Activity::new(Mode::Idle);
        let scratch = crate::screen::PoiScratch::new();
        let mut cx = Ctx {
            state: &mut st,
            activity: &mut act,
            settings: s,
            routes: &[],
            rides: &[],
            nav_profiles: profs,
            poi_scratch: &scratch,
            now_ms: 0,
        };
        scr.handle(g, &mut cx)
    }

    /// A turn/press cycles the index through the map's profiles, wrapping at both ends.
    #[test]
    fn cycles_through_map_profiles() {
        let profs = profiles(&["Road", "Gravel", "MTB", "Touring"]);
        let mut s = Settings::default();
        let mut scr = BikeTypeScreen::new();
        assert_eq!(s.bike_profile_idx, 0, "defaults to profile 0");
        run(&mut scr, &mut s, &profs, Gesture::Press);
        assert_eq!(s.bike_profile_idx, 1, "press steps forward one");
        run(&mut scr, &mut s, &profs, Gesture::Turn(2));
        assert_eq!(s.bike_profile_idx, 3, "a turn walks by its detents");
        run(&mut scr, &mut s, &profs, Gesture::Turn(1));
        assert_eq!(s.bike_profile_idx, 0, "wraps past the last profile");
        run(&mut scr, &mut s, &profs, Gesture::Turn(-1));
        assert_eq!(s.bike_profile_idx, 3, "and back past the first");
    }

    /// With zero or one profile there is nothing to cycle to — the index stays put (the inert
    /// no-map / single-profile case; "no behavior change when the map has exactly one profile").
    #[test]
    fn single_or_no_profile_is_inert() {
        let mut scr = BikeTypeScreen::new();

        let none = profiles(&[]);
        let mut s = Settings::default();
        run(&mut scr, &mut s, &none, Gesture::Press);
        run(&mut scr, &mut s, &none, Gesture::Turn(3));
        assert_eq!(s.bike_profile_idx, 0, "no profiles → cycling is a no-op");

        let one = profiles(&["Road"]);
        let mut s = Settings::default();
        run(&mut scr, &mut s, &one, Gesture::Press);
        run(&mut scr, &mut s, &one, Gesture::Turn(5));
        assert_eq!(s.bike_profile_idx, 0, "a single profile has nowhere to step");
    }

    /// Back pops the screen; the value edits are live (no save button), so nothing else to do.
    #[test]
    fn back_pops() {
        let profs = profiles(&["Road", "MTB"]);
        let mut s = Settings::default();
        let mut scr = BikeTypeScreen::new();
        assert!(matches!(run(&mut scr, &mut s, &profs, Gesture::Back), Transition::Pop));
    }
}
