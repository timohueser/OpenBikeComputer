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

use super::vocab::card::{ActionRows, CardEvent};
use super::vocab::chrome::{card_triangle, title_frame, wrapped};
use super::vocab::rows::{GuardedRowsGeometry, MenuItem};
use super::weather_map::WeatherRainMapScreen;
use super::{palette, Ctx, Render, Screen, Transition};

const ACTION_GUARDS: [bool; 2] = [false; 2];
const VIEW: usize = 0;

/// What the alert is about — sets the title and body copy. The WX12 engine maps its classes onto
/// these faces: heavy rain (≥ 10 mm/h reaching the corridor) → [`Rain`](WeatherAlertKind::Rain),
/// thunderstorm/hail → [`Storm`](WeatherAlertKind::Storm), dangerous gusts →
/// [`Gust`](WeatherAlertKind::Gust).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeatherAlertKind {
    Rain,
    Storm,
    Gust,
}

/// The alert card. Carries its own copy inputs (kind + minutes) so it renders identically
/// whether WX12, the sim's injection flag, or a test pushed it.
#[derive(Debug)]
pub struct WeatherAlertScreen {
    kind: WeatherAlertKind,
    minutes: u16,
    actions: ActionRows,
    /// The screen this card was pushed over is already the rain map (recorded by
    /// [`App::show_weather_alert`](crate::App::show_weather_alert), which sees the stack) — VIEW
    /// RAIN MAP then simply pops back to it instead of stacking a second identical rain map
    /// (review F4).
    over_rain_map: bool,
}

impl WeatherAlertScreen {
    pub fn new(kind: WeatherAlertKind, minutes: u16, over_rain_map: bool) -> Self {
        Self { kind, minutes, actions: ActionRows::new(0), over_rain_map }
    }

    /// Refresh the copy in place — how a re-fired alert updates an already-open card instead of
    /// stacking a second one (see [`App::show_weather_alert`](crate::App::show_weather_alert)).
    /// Returns whether anything actually changed, so a per-tick refresh with identical copy
    /// (WX12's governor runs at fix cadence) doesn't dirty the frame.
    pub fn update(&mut self, kind: WeatherAlertKind, minutes: u16) -> bool {
        let changed = self.kind != kind || self.minutes != minutes;
        self.kind = kind;
        self.minutes = minutes;
        changed
    }

    /// The card's current face — the WX12 governor reads it to route same-event countdown
    /// refreshes to the open card.
    pub fn kind(&self) -> WeatherAlertKind {
        self.kind
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match self.actions.handle(g, &ACTION_GUARDS) {
            CardEvent::Activate(VIEW) => {
                // VIEW RAIN MAP answers the alert with the rain map, never parking the card
                // underneath: normally the card is *replaced*; when the rider was already on
                // the rain map the card simply pops back to it (review F4 — replacing would
                // stack two identical rain maps). Either way the view lands on the current
                // frame, inside the rain grid's zoom regime.
                cx.state.rain_step = 0;
                cx.state.clamp_rain_zoom(cx.weather.zoom_floor());
                if self.over_rain_map {
                    Transition::Pop
                } else {
                    Transition::Replace(Screen::WeatherRainMap(WeatherRainMapScreen::new()))
                }
            }
            CardEvent::Activate(_) | CardEvent::Dismiss => Transition::Pop,
            CardEvent::None => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        let (title, body) = match self.kind {
            WeatherAlertKind::Rain => (Msg::WeatherAlertRain, Msg::WeatherAlertRainBody),
            WeatherAlertKind::Storm => (Msg::WeatherAlertStorm, Msg::WeatherAlertStormBody),
            WeatherAlertKind::Gust => (Msg::WeatherAlertGust, Msg::WeatherAlertGustBody),
        };
        title_frame(cv, w, h, rx.t(title), "");

        card_triangle(cv, Point::new(w / 2, 78), 20);
        let after = wrapped(cv, rx.t(body), w / 2, 116, w - 44, Font::Label, INK);

        // The timing line: minutes-to-impact in the big Display face (`IN 28 MIN`), or the
        // localized NOW for an alert already on the rider.
        let mut timing: heapless::String<16> = heapless::String::new();
        if self.minutes == 0 {
            let _ = timing.push_str(rx.t(Msg::WeatherNow));
        } else {
            let _ = write!(timing, "{} {} {}", rx.t(Msg::WeatherIn), self.minutes, rx.t(Msg::WeatherMin));
        }
        cv.text(&timing, Point::new(w / 2, after + 10), Font::Display, TextAlign::Center, INK);

        let items = [
            MenuItem { label: rx.t(Msg::WeatherViewRainMap), guard: ACTION_GUARDS[0] },
            MenuItem { label: rx.t(Msg::WeatherDismiss), guard: ACTION_GUARDS[1] },
        ];
        let top = h - 2 * (46 + 8) - 12;
        self.actions.draw(cv, &items, rx.hold_progress, WARNING, GuardedRowsGeometry::card(w, top));
    }
}
