//! The **weather alert** card (WX11, epic #1185): RAIN AHEAD / STORM AHEAD with the timing line
//! and the two locked actions — **VIEW RAIN MAP** and **DISMISS**.
//!
//! Host-pushed ([`App::show_weather_alert`](crate::App::show_weather_alert)); *generating* alerts
//! — thresholds, deduplication, cooldown persistence — is WX12's, which will drive that seam.
//! Deliberately a modal card in the upload-popup family: idle-exempt, but freely dismissable,
//! and the cached weather screens underneath stay untouched.

use core::fmt::Write as _;

use embedded_graphics::prelude::Point;
use obc_render::{
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::Msg;

use super::weather_map::WeatherRainMapScreen;
use super::{
    card_triangle, draw_guarded_rows, palette, title_frame, wrapped, Ctx, GuardedRowsGeometry, MenuItem, Render,
    Screen, Transition,
};

/// What the alert is about — sets the title, body copy and glyph emphasis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeatherAlertKind {
    Rain,
    Storm,
}

/// The alert card. Carries its own copy inputs (kind + minutes) so it renders identically
/// whether WX12, the sim's injection flag, or a test pushed it.
#[derive(Debug)]
pub struct WeatherAlertScreen {
    kind: WeatherAlertKind,
    minutes: u16,
    selected: usize,
}

impl WeatherAlertScreen {
    pub fn new(kind: WeatherAlertKind, minutes: u16) -> Self {
        Self { kind, minutes, selected: 0 }
    }

    /// Refresh the copy in place — how a re-fired alert updates an already-open card instead of
    /// stacking a second one (see [`App::show_weather_alert`](crate::App::show_weather_alert)).
    pub fn update(&mut self, kind: WeatherAlertKind, minutes: u16) {
        self.kind = kind;
        self.minutes = minutes;
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Step(n) => super::list::on_step(&mut self.selected, n, 2),
            Gesture::Press => match self.selected {
                0 => {
                    // VIEW RAIN MAP replaces the card, so Back from the map returns to whatever
                    // the rider was doing — the alert is answered, not parked underneath.
                    cx.state.rain_step = 0;
                    Transition::Replace(Screen::WeatherRainMap(WeatherRainMapScreen::new()))
                }
                _ => Transition::Pop, // DISMISS
            },
            Gesture::Back => Transition::Pop,
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        let (title, body) = match self.kind {
            WeatherAlertKind::Rain => (Msg::WeatherAlertRain, Msg::WeatherAlertRainBody),
            WeatherAlertKind::Storm => (Msg::WeatherAlertStorm, Msg::WeatherAlertStormBody),
        };
        title_frame(cv, w, h, rx.t(title), "");

        card_triangle(cv, Point::new(w / 2, 82), 22);
        let after = wrapped(cv, rx.t(body), w / 2, 122, w - 48, Font::Body, INK);

        // The timing line: minutes-to-impact in the big Display face (`IN 28 MIN`), or the
        // localized NOW for an alert already on the rider.
        let mut timing: heapless::String<16> = heapless::String::new();
        if self.minutes == 0 {
            let _ = timing.push_str(rx.t(Msg::WeatherNow));
        } else {
            let _ = write!(timing, "{} {} {}", rx.t(Msg::WeatherIn), self.minutes, rx.t(Msg::WeatherMin));
        }
        cv.text(&timing, Point::new(w / 2, after + 14), Font::Display, TextAlign::Center, INK);

        let items =
            [MenuItem { label: rx.t(Msg::WeatherViewRainMap), guard: false }, MenuItem { label: rx.t(Msg::WeatherDismiss), guard: false }];
        let top = h - 2 * (46 + 8) - 12;
        draw_guarded_rows(cv, &items, self.selected, rx.hold_progress, WARNING, GuardedRowsGeometry::card(w, top));
    }
}
