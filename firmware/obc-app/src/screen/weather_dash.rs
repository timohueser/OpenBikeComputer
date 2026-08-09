//! The **Weather dashboard** (WX11, epic #1185) — locked concept C: a top decision card
//! (DRY FOR 2 HOURS / RAIN IN 35 MIN / STORM IN 28 MIN / the honest fallback states), the
//! two-hour precipitation strip below it, and the **HOURLY** / **RAIN MAP** actions.
//!
//! The card and the strip never duplicate each other: the card is the derived *claim*
//! ([`rain_outlook`]), the strip is the sampled per-frame evidence. Both derive from the
//! host-fed [`WeatherSnapshot`] plus this frame's `now`, so stale data degrades honestly the
//! moment the clock passes a freshness boundary — no host round-trip required. Cached content
//! stays fully visible during a refresh; the only refresh cue is the title bar's right slot.

use core::fmt::Write as _;

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::settings::DateTime;
use crate::wall_clock::MinuteTicker;
use crate::weather::{local_hour_minute, rain_outlook, RainOutlook, WeatherSnapshot, OUTLOOK_WINDOW_S};
use crate::Msg;

use super::weather_icons::{self, DayPhase, WeatherIcon, WeatherIconTheme};
use super::weather_map::WeatherRainMapScreen;
use super::{palette, title_frame, wrapped, Ctx, Render, Screen, ScreenTick, Transition, LIST_TOP};

/// The two action rows, in draw order.
const ACTIONS: usize = 2;

/// The decision card's frame: full row width, sitting right under the title bar.
const CARD_X: i32 = 14;
const CARD_Y: i32 = LIST_TOP;
const CARD_H: i32 = 88;

/// The two-hour strip: 8 fifteen-minute slots between `now` and `now + 2 h`.
pub(crate) const STRIP_SLOTS: usize = 8;
const STRIP_TOP: i32 = CARD_Y + CARD_H + 8;
/// Bar area height above the baseline (a full-intensity bar fills it exactly).
const STRIP_BAR_MAX: i32 = 38;
/// The baseline the bars stand on; slot labels hang below it.
const STRIP_BASE: i32 = STRIP_TOP + STRIP_BAR_MAX + 2;

/// Top of the two action rows — 8 px clear of the freshness line above (owner tuning round:
/// "Updated" sat too close to HOURLY), and the last row still 2 px inside the frame outline.
const ACTIONS_TOP: i32 = 232;
const ACTION_ROW_H: i32 = 38;
const ACTION_GAP: i32 = 6;

/// The Weather dashboard screen. State is the highlighted action row plus the minute ticker that
/// keeps the countdown / freshness copy honest as time passes.
#[derive(Debug, Default)]
pub struct WeatherScreen {
    selected: usize,
    ticker: MinuteTicker,
}

impl WeatherScreen {
    pub fn new() -> Self {
        WeatherScreen::default()
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Step(n) => super::list::on_step(&mut self.selected, n, ACTIONS),
            Gesture::Press => match self.selected {
                0 => Transition::Push(Screen::WeatherHourly(super::WeatherHourlyScreen::new())),
                _ => {
                    // The rain map always opens at the *current* frame — a previous visit's
                    // time-step must never leak into a fresh look at the sky — and inside the
                    // product's zoom regime (the zoom-out clamp, owner tuning round 2).
                    cx.state.rain_step = 0;
                    cx.state.clamp_rain_zoom();
                    Transition::Push(Screen::WeatherRainMap(WeatherRainMapScreen::new()))
                }
            },
            Gesture::Back => Transition::Pop,
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    /// Minute tick: the card's countdown ("RAIN IN 35 MIN") and the freshness derivation move
    /// with the wall clock, so the dashboard repaints once a minute while up.
    pub fn tick_timers(&mut self, now: DateTime, ms_to_next_minute: u32) -> ScreenTick {
        ScreenTick { changed: self.ticker.changed(now), next_wake_ms: Some(ms_to_next_minute), region: None }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let w = rx.w;
        // Title bar: the right slot is the one refresh cue — cached content below stays fully
        // visible while the phone fetches (locked UX).
        let right = if rx.weather_refreshing { rx.t(Msg::WeatherUpdating) } else { "" };
        title_frame(cv, w, rx.h, rx.t(Msg::WeatherTitle), right);

        let now = rx.now_utc as i64;
        let Some(snap) = rx.weather else {
            // No snapshot at all (never fetched / store empty): the explicit no-data state.
            draw_card_note(cv, w, rx.t(Msg::WeatherNoData), Some(rx.t(Msg::WeatherNoDataSub)));
            self.draw_actions(cv, w, rx);
            return;
        };

        // The "now" hourly record feeds the card's quiet extras: the current condition's icon
        // and the current temperature (owner tuning round: present, deliberately not prominent —
        // a Label readout under the icon).
        let now_record = snap.hourly_at(now).map(|(_, _, rec)| rec);
        let icon_now = now_record
            .map(|rec| hourly_icon(rec.condition, now, rx.settings.utc_offset_min))
            .unwrap_or(WeatherIcon::Unavailable);
        let temp = now_record.and_then(|rec| fmt_temp(rec.temperature_deci_c));
        let temp = temp.as_deref();

        let outlook = rain_outlook(snap, now);
        match outlook {
            RainOutlook::Dry => {
                // The current condition's icon keeps the dry card informative (sun/cloud/fog…)
                // without ever contradicting the claim — it comes from the same bundle.
                draw_card_lines(cv, w, rx.t(Msg::WeatherDry1), rx.t(Msg::WeatherDry2), INK, Some(icon_now), temp);
            }
            RainOutlook::RainIn { minutes } => {
                let mut value: heapless::String<12> = heapless::String::new();
                let (l1, l2) = if minutes == 0 {
                    (rx.t(Msg::WeatherRain), rx.t(Msg::WeatherNow))
                } else {
                    let _ = write!(value, "{minutes} {}", rx.t(Msg::WeatherMin));
                    (rx.t(Msg::WeatherRainIn), value.as_str())
                };
                draw_card_lines(cv, w, l1, l2, INK, Some(WeatherIcon::Rain), temp);
            }
            RainOutlook::StormIn { minutes } => {
                let mut value: heapless::String<12> = heapless::String::new();
                let (l1, l2) = if minutes == 0 {
                    (rx.t(Msg::WeatherStorm), rx.t(Msg::WeatherNow))
                } else {
                    let _ = write!(value, "{minutes} {}", rx.t(Msg::WeatherMin));
                    (rx.t(Msg::WeatherStormIn), value.as_str())
                };
                draw_card_lines(cv, w, l1, l2, WARNING, Some(WeatherIcon::Thunderstorm), temp);
            }
            RainOutlook::UpdateNeeded => {
                draw_card_note(cv, w, rx.t(Msg::WeatherUpdateNeeded), None);
            }
            RainOutlook::HourlyOnly => {
                // The hourly half of the bundle is fresh and valid here — only the rain product
                // is absent — so the card stays *calm*: the current condition + temperature with
                // the explicit hourly-only copy, no warning glyph (owner tuning round, 2b).
                draw_card_calm(cv, w, rx.t(Msg::WeatherHourlyOnly), rx.t(Msg::WeatherHourlyOnlySub), icon_now, temp);
            }
        }

        draw_strip(cv, w, snap, now, rx.t(Msg::WeatherNow));

        // The freshness line: the bundle's real production timestamp, local time. Absolute — a
        // real timestamp stays honest even on an untrusted device clock, where a derived "n min
        // ago" would fabricate an age (`clock_trusted` gates ages, not instants).
        let (hh, mm) = local_hour_minute(snap.generated_at, rx.settings.utc_offset_min);
        let mut fresh: heapless::String<24> = heapless::String::new();
        let _ = write!(fresh, "{} {:02}:{:02}", rx.t(Msg::WeatherUpdated), hh, mm);
        cv.text(&fresh, Point::new(w / 2, STRIP_BASE + 28), Font::Label, TextAlign::Center, SUBTEXT);

        self.draw_actions(cv, w, rx);
    }

    /// The HOURLY / RAIN MAP action rows — the settings-row cursor idiom (amber fill on the
    /// selected row), consistent spacing from the last content line.
    fn draw_actions(&self, cv: &mut impl Surface, w: i32, rx: &Render) {
        use palette::*;
        let labels = [rx.t(Msg::WeatherHourly), rx.t(Msg::WeatherRainMap)];
        for (i, label) in labels.iter().enumerate() {
            let y = ACTIONS_TOP + i as i32 * (ACTION_ROW_H + ACTION_GAP);
            let area = rect(CARD_X, y, w - 2 * CARD_X, ACTION_ROW_H);
            if i == self.selected {
                cv.round(area, 6, AMBER);
            }
            cv.text_vcentered(label, CARD_X + 14, (y, ACTION_ROW_H), Font::Body, TextAlign::Left, INK);
            // A forward cue on the far edge, like the settings rows that open a page.
            let cxr = CARD_X + area.size.width as i32 - 18;
            let mid = y + ACTION_ROW_H / 2;
            cv.triangle(Point::new(cxr, mid - 8), Point::new(cxr, mid + 8), Point::new(cxr + 10, mid), INK);
        }
    }
}

/// The condition's contextual icon for an instant, using the shared 6:00–19:59 daylight
/// heuristic (WX11 provisional: the device has no sunrise table; noted in the epic).
pub(super) fn hourly_icon(condition: u8, unix_utc: i64, utc_offset_min: i16) -> WeatherIcon {
    let (hour, _) = local_hour_minute(unix_utc, utc_offset_min);
    let phase = if (6..20).contains(&hour) { DayPhase::Day } else { DayPhase::Night };
    weather_icons::icon_for(condition, phase)
}

/// The decision card's pane + two text lines (Body caption over the big Display value) with an
/// optional 48-px icon on the right and the quiet current-temperature readout beneath it.
fn draw_card_lines(
    cv: &mut impl Surface,
    w: i32,
    line1: &str,
    line2: &str,
    value_color: u16,
    icon: Option<WeatherIcon>,
    temp: Option<&str>,
) {
    use palette::*;
    let area = rect(CARD_X, CARD_Y, w - 2 * CARD_X, CARD_H);
    cv.round(area, 6, PARCHMENT_SHADE);
    cv.text(line1, Point::new(CARD_X + 14, CARD_Y + 14), Font::Body, TextAlign::Left, INK);
    cv.text(line2, Point::new(CARD_X + 14, CARD_Y + 44), Font::Display, TextAlign::Left, value_color);
    let cx = CARD_X + area.size.width as i32 - 6 - 24;
    if let Some(icon) = icon {
        weather_icons::draw(cv, icon, cx, CARD_Y + 12, weather_icons::DASHBOARD_SCALE, WeatherIconTheme::Parchment);
    }
    if let Some(temp) = temp {
        cv.text(temp, Point::new(cx, CARD_Y + 64), Font::Label, TextAlign::Center, INK);
    }
}

/// The calm informational card (the hourly-only state): the explicit copy on the left — the
/// hourly forecast is fresh here, only the rain product is absent, so no warning glyph — with
/// the current condition + temperature on the right, exactly the lines-card anatomy.
fn draw_card_calm(cv: &mut impl Surface, w: i32, text: &str, sub: &str, icon: WeatherIcon, temp: Option<&str>) {
    use palette::*;
    let area = rect(CARD_X, CARD_Y, w - 2 * CARD_X, CARD_H);
    cv.round(area, 6, PARCHMENT_SHADE);
    // Text zone left of the icon column, wrapped at Label and vertically centred with its
    // sub-line, so the block can never spill the pane (the round-one overlap bug).
    let zone_w = area.size.width as i32 - 48 - 26;
    let zone_cx = CARD_X + 12 + zone_w / 2;
    let line_h = Font::Label.cap_height() as i32 + 1;
    let per_line = (zone_w / Font::Label.char_width() as i32).max(1) as usize;
    let lines = wrapped_line_count(text, per_line) as i32;
    let block_h = lines * line_h + 4 + line_h;
    let top = CARD_Y + ((CARD_H - block_h) / 2).max(8);
    let after = wrapped(cv, text, zone_cx, top, zone_w, Font::Label, INK);
    cv.text(sub, Point::new(zone_cx, after + 4), Font::Label, TextAlign::Center, SUBTEXT);
    let cx = CARD_X + area.size.width as i32 - 6 - 24;
    weather_icons::draw(cv, icon, cx, CARD_Y + 12, weather_icons::DASHBOARD_SCALE, WeatherIconTheme::Parchment);
    if let Some(temp) = temp {
        cv.text(temp, Point::new(cx, CARD_Y + 64), Font::Label, TextAlign::Center, INK);
    }
}

/// The greedy line count [`wrapped`](super::wrapped) will produce for `text` at `per_line`
/// characters — the pre-measure the centred card blocks need.
fn wrapped_line_count(text: &str, per_line: usize) -> usize {
    let mut lines = 0usize;
    let mut len = 0usize;
    for word in text.split(' ') {
        let extra = if len == 0 { word.len() } else { len + 1 + word.len() };
        if extra > per_line && len != 0 {
            lines += 1;
            len = word.len();
        } else {
            len = extra;
        }
    }
    lines + 1
}

/// The current temperature as a compact `14°` readout, or `None` on the wire sentinel — shared
/// by the dashboard card and the hourly rows so the two can never round differently.
pub(super) fn fmt_temp(deci_c: i16) -> Option<heapless::String<8>> {
    if deci_c == obc_formats::obcw::TEMP_UNAVAILABLE {
        return None;
    }
    let deg = ((deci_c as i32) + if deci_c >= 0 { 5 } else { -5 }) / 10;
    let mut s: heapless::String<8> = heapless::String::new();
    let _ = write!(s, "{}°", deg.clamp(-99, 99));
    Some(s)
}

/// The card's honest-fallback face: the shared warning triangle on the left with the wrapped
/// copy beside it (update-needed / no-data), or plain centred copy (hourly-only) — everything
/// stays inside the card's own pane.
/// The warning face of the card — the update-needed / no-data states only (the hourly-only
/// state is deliberately the *calm* [`draw_card_calm`]): the shared warning triangle on the
/// left, the wrapped copy + optional sub-line vertically centred beside it.
fn draw_card_note(cv: &mut impl Surface, w: i32, text: &str, sub: Option<&str>) {
    use palette::*;
    let area = rect(CARD_X, CARD_Y, w - 2 * CARD_X, CARD_H);
    cv.round(area, 6, PARCHMENT_SHADE);
    super::card_triangle(cv, Point::new(CARD_X + 30, CARD_Y + CARD_H / 2), 14);
    let zone_x = CARD_X + 58;
    let zone_w = area.size.width as i32 - 58 - 10;
    let line_h = Font::Label.cap_height() as i32 + 1;
    let per_line = (zone_w / Font::Label.char_width() as i32).max(1) as usize;
    let lines = wrapped_line_count(text, per_line) as i32;
    let block_h = lines * line_h + sub.map_or(0, |_| line_h + 4);
    let top = CARD_Y + ((CARD_H - block_h) / 2).max(8);
    let after = wrapped(cv, text, zone_x + zone_w / 2, top, zone_w, Font::Label, INK);
    if let Some(sub) = sub {
        cv.text(sub, Point::new(zone_x + zone_w / 2, after + 4), Font::Label, TextAlign::Center, SUBTEXT);
    }
}

/// The two-hour strip: 8 fifteen-minute slots from `now`, each a bar whose height and color come
/// straight from the sampled intensity (the firmware-owned [`rain_style`](obc_render::rain_style)
/// table — no separate legend, by design). A dry slot is a low tan stub; a slot no current frame
/// covers draws a small muted dot — unknown is unknown, never dry, but calm.
fn draw_strip(cv: &mut impl Surface, w: i32, snap: &WeatherSnapshot, now: i64, now_label: &str) {
    use palette::*;
    let inner_w = w - 2 * CARD_X;
    let gap = 4;
    let slot_w = (inner_w - gap * (STRIP_SLOTS as i32 - 1)) / STRIP_SLOTS as i32;
    let step_s = OUTLOOK_WINDOW_S / STRIP_SLOTS as i64;
    for i in 0..STRIP_SLOTS as i32 {
        let x = CARD_X + i * (slot_w + gap);
        let t = now + i as i64 * step_s;
        match snap.intensity_covering(t) {
            Some(0) => {
                // Measured dry: a low stub, so the timeline still reads as "answered".
                cv.fill(rect(x, STRIP_BASE - 4, slot_w, 4), RULE);
            }
            Some(intensity) => {
                let (color, _) = obc_render::rain_style(intensity);
                let h = 6 + (intensity as i32).min(12) * 32 / 12;
                cv.fill(rect(x, STRIP_BASE - h, slot_w, h), color);
            }
            None => {
                // Uncovered / no-data: a small muted dot — still visibly "no answer", never a
                // dry-looking stub, but calm (owner tuning round: eight question marks read as
                // alarming for what is a rare state once WX6's floor products cover the globe).
                cv.disc(Point::new(x + slot_w / 2, STRIP_BASE - 8), 2, CONTOUR);
            }
        }
    }
    // Baseline + the three time anchors: NOW on the left, +1h at the window's middle, +2h at the end.
    cv.fill(rect(CARD_X, STRIP_BASE, inner_w, 2), RULE);
    cv.text(now_label, Point::new(CARD_X, STRIP_BASE + 6), Font::Label, TextAlign::Left, SUBTEXT);
    cv.text("+1h", Point::new(CARD_X + inner_w / 2, STRIP_BASE + 6), Font::Label, TextAlign::Center, SUBTEXT);
    cv.text("+2h", Point::new(CARD_X + inner_w, STRIP_BASE + 6), Font::Label, TextAlign::Right, SUBTEXT);
}
