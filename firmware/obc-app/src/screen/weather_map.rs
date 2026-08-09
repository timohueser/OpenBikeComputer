//! The **Rain map** screen (WX11, epic #1185): the normal map scene — base map, route, rider,
//! chrome idiom — with the WX10 precipitation raster below the road band, plus 15-minute
//! time-step navigation over the bundle's *real* frame timestamps.
//!
//! Locked UX: **no** "Rain mm/h" title, **no** legend — the map chrome carries everything.
//! Honesty laws (from #1213's closeout): an out-of-regime overlay
//! ([`RenderStats::rain_out_of_regime`](obc_render::RenderStats)) shows an explicit
//! zoom-in state; expired / uncovered rain shows **WEATHER UPDATE NEEDED** or the explicit
//! hourly-only banner — a silent rain-free frame is never presented as dry.
//!
//! Bindings: `up/down` steps through the forecast frames (`NOW`, +15, +30, …), `hold` enters the
//! shared Inspect/pan mode (move / zoom, exactly the Map's), `back` returns to the dashboard.

use core::fmt::Write as _;

use embedded_graphics::{draw_target::DrawTarget, prelude::Point};
use obc_render::{
    text::{Font, TextAlign},
    Canvas, Surface,
};

use crate::input::Gesture;
use crate::settings::DateTime;
use crate::wall_clock::MinuteTicker;
use crate::weather::{local_hour_minute, rain_outlook, RainOutlook};
use crate::Msg;

use super::map::{draw_map_scene, handle_pan};
use super::{palette, wrapped, Ctx, RenderFrame, ScreenTick, Transition};

/// The time-step label's top anchor — the Map clock's slot (there is no clock overlay here; the
/// viewed frame's timestamp owns the top edge).
const LABEL_TOP: i32 = 10;

/// The rain map. Camera state is the shared [`AppState`](crate::AppState) (pan/zoom reuse the
/// Map's machinery); the selected time step lives there too (`rain_step`) so the host can lease
/// the matching frame. Screen-local state is only the minute ticker that re-derives freshness.
#[derive(Debug, Default)]
pub struct WeatherRainMapScreen {
    ticker: MinuteTicker,
}

impl WeatherRainMapScreen {
    pub fn new() -> Self {
        WeatherRainMapScreen::default()
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        // Inspect/pan is a sub-mode exactly as on the Map: while the shared camera holds a pan,
        // Select/Back drive panning (move/zoom) and Back-tap returns to the rain map's bindings.
        if cx.state.pan.is_some() {
            return handle_pan(g, cx);
        }
        match g {
            // Time-step through the forecast frames: clamped to the frames that actually exist
            // (`rain_steps_ahead`, host-refreshed against the snapshot each frame). No wrap — the
            // timeline's ends are ends.
            Gesture::Step(n) => {
                let step = cx.state.rain_step as i32 + n;
                cx.state.rain_step = step.clamp(0, cx.state.rain_steps_ahead as i32) as u8;
                Transition::None
            }
            Gesture::Hold => {
                cx.state.enter_pan(cx.activity.active_route.is_some(), cx.activity.progress_m);
                Transition::None
            }
            Gesture::Back => {
                cx.state.rain_step = 0;
                Transition::Pop
            }
            Gesture::Press | Gesture::BackHold => Transition::None,
        }
    }

    /// Minute tick: frame currency and the banner derivations move with the clock.
    pub fn tick_timers(&mut self, now: DateTime, ms_to_next_minute: u32) -> ScreenTick {
        ScreenTick { changed: self.ticker.changed(now), next_wake_ms: Some(ms_to_next_minute), region: None }
    }

    pub fn draw<D, F, S>(&self, cv: &mut Canvas<D, F>, rx: &mut RenderFrame<'_, S>)
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
        S: obc_map_scene::MapScene,
    {
        let vp = rx.state.viewport(rx.w as f32, rx.h as f32);
        // Whether the host leased a rain frame this call — consumed by the scene draw below, so
        // remember it before it's taken. `None` must never end as a silent dry-looking map.
        let had_lease = rx.rain.is_some();
        if draw_map_scene(cv, rx, &vp, None).is_none() {
            return;
        }
        let rx = &mut rx.render;
        let (w, h) = (rx.w, rx.h);
        let now = rx.now_utc as i64;
        let step = rx.state.rain_step;

        // The pan HUD owns the frame while panning; the rain raster stays under it, but the
        // time/banner chrome yields (the Map's own chip discipline).
        if rx.state.pan.is_some() {
            return;
        }

        // The viewed frame's REAL timestamp, top-centre (tier differences surface only through
        // real timestamps — locked): the current frame shows `HH:MM NOW`, a stepped one
        // `HH:MM +NN`.
        if let Some(snap) = rx.weather {
            if let Some(current) = snap.current_frame_index(now) {
                let index = (current + step as usize).min(snap.frames.len() - 1);
                let frame = &snap.frames[index];
                let (hh, mm) = local_hour_minute(frame.valid_at, rx.settings.utc_offset_min);
                let mut label: heapless::String<20> = heapless::String::new();
                if index == current {
                    let _ = write!(label, "{hh:02}:{mm:02} {}", rx.t(Msg::WeatherNow));
                } else {
                    let ahead_min = ((frame.valid_at - now).max(0) + 59) / 60;
                    let _ = write!(label, "{hh:02}:{mm:02} +{ahead_min}");
                }
                draw_halo_label(cv, w, &label);
            }
        }

        // The honest states, in precedence order: no renderable rain (stale / no product /
        // no snapshot) beats out-of-regime (nothing would draw at any zoom).
        if !had_lease {
            match rx.weather.map(|snap| rain_outlook(snap, now)) {
                Some(RainOutlook::HourlyOnly) => {
                    draw_banner(cv, w, h, rx.t(Msg::WeatherHourlyOnly), Some(rx.t(Msg::WeatherHourlyOnlySub)));
                }
                Some(RainOutlook::Dry) | Some(RainOutlook::RainIn { .. }) | Some(RainOutlook::StormIn { .. }) => {
                    // The outlook still answers but the overlay has nothing current to draw
                    // (e.g. a coverage seam): still never silently dry.
                    draw_banner(cv, w, h, rx.t(Msg::WeatherUpdateNeeded), None);
                }
                Some(RainOutlook::UpdateNeeded) => draw_banner(cv, w, h, rx.t(Msg::WeatherUpdateNeeded), None),
                None => draw_banner(cv, w, h, rx.t(Msg::WeatherNoData), Some(rx.t(Msg::WeatherNoDataSub))),
            }
        } else if rx.stats.rain_out_of_regime {
            draw_banner(cv, w, h, rx.t(Msg::WeatherZoomForRain), None);
        }
    }
}

/// The top-centre frame-time label — bare ink text with a 1-px parchment halo, the Map clock's
/// exact idiom (no pill, no title bar: the locked "no 'Rain mm/h' title" rule is structural).
fn draw_halo_label(cv: &mut impl Surface, w: i32, s: &str) {
    use palette::*;
    for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
        cv.text(s, Point::new(w / 2 + dx, LABEL_TOP + dy), Font::Body, TextAlign::Center, PARCHMENT);
    }
    cv.text(s, Point::new(w / 2, LABEL_TOP), Font::Body, TextAlign::Center, INK);
}

/// The bottom-centre honest-state banner: a parchment pill with a warning outline and one or two
/// centred [`Font::Label`] lines (wrapping handles the long locked copy at 240 px). The Map's
/// warning-chip slot, sized to its content.
fn draw_banner(cv: &mut impl Surface, w: i32, h: i32, text: &str, sub: Option<&str>) {
    use palette::*;
    // Measure the wrapped line count first (dry run against a no-op surface is overkill — the
    // greedy wrap is deterministic, so estimate from the same cell math `wrapped` uses).
    let budget_px = w - 44;
    let cell = Font::Label.char_width() as i32;
    let per_line = (budget_px / cell).max(1) as usize;
    let mut lines = 0usize;
    let mut line_len = 0usize;
    for word in text.split(' ') {
        let extra = if line_len == 0 { word.len() } else { line_len + 1 + word.len() };
        if extra > per_line && line_len != 0 {
            lines += 1;
            line_len = word.len();
        } else {
            line_len = extra;
        }
    }
    lines += 1;
    let line_h = Font::Label.cap_height() as i32 + 1;
    let body_h = lines as i32 * line_h + sub.map_or(0, |_| line_h + 2);
    let ph = body_h + 20;
    let py = h - ph - 12;
    let pw = w - 24;
    let px = (w - pw) / 2;
    cv.round(obc_render::rect(px, py, pw, ph), 9, PARCHMENT);
    cv.round_outline(obc_render::rect(px, py, pw, ph), 9, WARNING);
    let after = wrapped(cv, text, w / 2, py + 10, budget_px, Font::Label, WARNING);
    if let Some(sub) = sub {
        cv.text(sub, Point::new(w / 2, after + 2), Font::Label, TextAlign::Center, SUBTEXT);
    }
}
