//! The Bike type screen — picks the routing profile the on-device planner weights edges by
//! (routing-v2 N5, epic #533). The choice is a bare **index** into the loaded map's §8.6 profile
//! table ([`Settings::bike_profile_idx`](crate::Settings)); this screen cycles it through the map's
//! profile **names** ([`NavProfiles`](crate::NavProfiles)), so a custom web-builder profile shows up
//! automatically — no hardcoded list. A single value row that a press or a step walks in place, like
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

use super::bike_icons;
use crate::input::Gesture;
use crate::screen::{palette, title_frame, Ctx, Render, Transition, TITLE_BAR_H};
use crate::Msg;

/// Art-pixel scale for the hero bike sprite (50 × 30 art px → 200 × 120 device px).
const BIKE_SCALE: i32 = 4;
/// Top of the hero bike, just under the title bar.
const BIKE_TOP_Y: i32 = TITLE_BAR_H + 8;
/// Top of the profile-name selector row, below the hero bike.
const SELECTOR_ROW_Y: i32 = 198;

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
            // A step browses the loaded map's profile names, wrapping at both ends. A no-op when the
            // map carries zero or one profile — nothing to cycle to — so a router-less / no-map
            // device just leaves the index at 0.
            Gesture::Step(n) => {
                let count = cx.nav_profiles.len();
                if count > 1 {
                    let cur = cx.settings.bike_profile_idx as i32;
                    cx.settings.bike_profile_idx = (cur + n).rem_euclid(count as i32) as u8;
                }
                Transition::None
            }
            // Select confirms the browsed choice and closes the screen — the edits are live, so this
            // just pops, exactly like Back (it no longer advances the selection).
            Gesture::Press | Gesture::Back => Transition::Pop,
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, rx.t(Msg::BikeTypeTitle), "");

        let idx = rx.settings.bike_profile_idx;
        let count = rx.nav_profiles.len();

        // No map loaded (or a router-less `ble` build): nothing to pick from, so say so instead of
        // drawing an empty picker.
        if count == 0 {
            super::empty_state(cv, w, h, rx.t(Msg::BikeTypeNoProfiles), rx.t(Msg::BikeTypeNoProfilesSub));
            return;
        }

        // The *effective* profile: profile 0 when the stored index is out of range on a smaller map
        // (the router's fallback, N3) — so the hero bike and the selector name agree.
        let marked = rx.nav_profiles.effective(idx);
        let eff_name = rx.nav_profiles.name(marked).unwrap_or("");

        // Hero: the pixel-art bike for the effective profile, matched by name and filling the space
        // under the title bar. A custom profile the matcher doesn't recognise gets the generic bike.
        // Each type is drawn in its own hinting colour (road red, gravel brown, MTB green, …).
        let bike = bike_icons::for_name(eff_name);
        bike_icons::draw(cv, bike, w / 2, BIKE_TOP_Y, BIKE_SCALE, bike_icons::color_for(eff_name));

        // The one selector row — the current profile name centred in the amber cursor, flanked by
        // left/right arrows so it reads as "rotate to switch". `write_label` shows profile 0's name
        // for an out-of-range stored index (never a name the map doesn't have). The bike above *is*
        // the "which type" cue, so there's no separate list to repeat the names.
        let area = super::row_rect(SELECTOR_ROW_Y, w, 46);
        super::row_cursor(cv, area, true, false);
        let midy = area.top_left.y + area.size.height as i32 / 2;
        let mut label: heapless::String<20> = heapless::String::new();
        rx.nav_profiles.write_label(idx, &mut label);
        cv.text_vcentered(&label, w / 2, (area.top_left.y, 46), Font::Body, TextAlign::Center, INK);
        if count > 1 {
            let ax = area.top_left.x + 18;
            cv.triangle(Point::new(ax, midy - 9), Point::new(ax, midy + 9), Point::new(ax - 11, midy), INK);
            let bx = area.top_left.x + area.size.width as i32 - 18;
            cv.triangle(Point::new(bx, midy - 9), Point::new(bx, midy + 9), Point::new(bx + 11, midy), INK);
        }

        // The centred olive teaching line under the picker — the screen otherwise never says what the
        // choice *does*; this names it (the router weights edges by this profile — N5, epic #533). T8
        // item 2. Authored as one line, but "Routing uses this profile" (and its de/fr/es
        // translations) overruns a single 240 px Label line, so it centre-wraps to two rather than
        // clip — the shared card body wrap, so every language stays inside the panel.
        let sub_y = area.top_left.y + area.size.height as i32 + 14;
        crate::screen::wrapped(cv, rx.t(Msg::BikeTypeRoutingUses), w / 2, sub_y, w - 24, Font::Label, SUBTEXT);
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
            trips: &[],
            nav_profiles: profs,
            poi_scratch: &scratch,
            waypoints: &[],
            corridor: &[],
            sensor_scan_hits: &[],
            now_ms: 0,
        };
        scr.handle(g, &mut cx)
    }

    /// A step cycles the index through the map's profiles, wrapping at both ends; Select and Back
    /// both close the screen and a Select does **not** advance the selection (the browse-then-confirm
    /// model — the edits are already live).
    #[test]
    fn turn_cycles_select_and_back_close() {
        let profs = profiles(&["Road", "Gravel", "MTB", "Touring"]);
        let mut s = Settings::default();
        let mut scr = BikeTypeScreen::new();
        assert_eq!(s.bike_profile_idx, 0, "defaults to profile 0");
        run(&mut scr, &mut s, &profs, Gesture::Step(1));
        assert_eq!(s.bike_profile_idx, 1, "a step moves forward one profile");
        run(&mut scr, &mut s, &profs, Gesture::Step(2));
        assert_eq!(s.bike_profile_idx, 3, "a multi-step move walks by its count");
        run(&mut scr, &mut s, &profs, Gesture::Step(1));
        assert_eq!(s.bike_profile_idx, 0, "wraps past the last profile");
        run(&mut scr, &mut s, &profs, Gesture::Step(-1));
        assert_eq!(s.bike_profile_idx, 3, "and back past the first");

        // Select closes the screen without changing the browsed selection.
        let t = run(&mut scr, &mut s, &profs, Gesture::Press);
        assert!(matches!(t, Transition::Pop), "Select closes the screen");
        assert_eq!(s.bike_profile_idx, 3, "Select confirms — it does not advance the selection");
        // Back closes it too.
        assert!(matches!(run(&mut scr, &mut s, &profs, Gesture::Back), Transition::Pop));
    }

    /// With zero or one profile there is nothing to cycle to — a step is a no-op (the inert
    /// no-map / single-profile case; "no behavior change when the map has exactly one profile").
    #[test]
    fn single_or_no_profile_is_inert() {
        let mut scr = BikeTypeScreen::new();

        let none = profiles(&[]);
        let mut s = Settings::default();
        run(&mut scr, &mut s, &none, Gesture::Step(3));
        assert_eq!(s.bike_profile_idx, 0, "no profiles → a step is a no-op");

        let one = profiles(&["Road"]);
        let mut s = Settings::default();
        run(&mut scr, &mut s, &one, Gesture::Step(5));
        assert_eq!(s.bike_profile_idx, 0, "a single profile has nowhere to step");
    }
}
