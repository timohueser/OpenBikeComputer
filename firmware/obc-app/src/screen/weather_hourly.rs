//! The **Hourly forecast** screen (WX11, epic #1185): the OBCW bundle's 24 fixed hourly records
//! from the hour covering `now` onward, one evenly-spaced row each — time, the WX17 pixel icon,
//! temperature, precipitation and wind. **No separator bars** (locked UX): the columns carry the
//! grid.
//!
//! The wind arrow is drawn in the wind's *to*-direction on a north-up rose; the adjacent label is
//! the meteorological *from*-octant (`SW`) plus the speed. Route-relative coloring (green tail /
//! orange cross / red head) flows through [`wind_class`] against the WX12 travel direction
//! (`Render::travel_deg` — route tangent, else moving GPS course); without one the arrows stay
//! neutral ink, never a false head/tail.

use core::fmt::Write as _;

use embedded_graphics::prelude::Point;
use obc_render::{
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::settings::Units;
use crate::weather::{local_hour_minute, wind_class, wind_octant, WindClass};
use crate::Msg;

use super::weather_dash::hourly_icon;
use super::{empty_state, list, palette, stroke2, title_frame, Ctx, Render, Transition, LIST_TOP};

use obc_formats::obcw::{
    HOURLY_COUNT, HOURLY_INTERVAL_SECONDS, PRECIP_UNAVAILABLE, WIND_DIRECTION_UNAVAILABLE, WIND_SPEED_UNAVAILABLE,
};

/// Nominal row pitch — doubled from the round-one 34 (owner tuning round: the crammed rows were
/// the complaint); four rows fill the 320-px panel, with the leftover folded back into the pitch
/// so the last row lands flush.
const ROW_H: i32 = 64;

/// Fixed column geometry, left to right. Every box is bounded at its widest legal content, so no
/// two columns can collide at any string width in any catalog language: the time is two digits,
/// the icon 24 px, the temperature is clamped to `-99°` (4 cells of Display), the precipitation
/// to `99+mm` (5 cells of Label), and the wind text to octant (≤ 2 cells in every language:
/// N/NE/... , de N/O/SO/... , fr/es N/SO/O/...) + space + speed ≤ 2 digits = 5 Label cells,
/// right-anchored at [`WIND_TEXT_X`] so it can never reach back past [`WIND_ARROW_CX`]'s box.
const TIME_X: i32 = 14;
const ICON_CX: i32 = 62;
const TEMP_X: i32 = 84;
const PRECIP_X: i32 = 84;
const WIND_ARROW_CX: i32 = 150;
const WIND_TEXT_X: i32 = 228;

/// The eight compass-octant catalog keys, clockwise from north — indexed by
/// [`wind_octant`](crate::weather::wind_octant).
const OCTANTS: [Msg; 8] = [
    Msg::CompassN,
    Msg::CompassNe,
    Msg::CompassE,
    Msg::CompassSe,
    Msg::CompassS,
    Msg::CompassSw,
    Msg::CompassW,
    Msg::CompassNw,
];

/// The hourly list. State is the scroll window's first visible row (there is no row selection —
/// the rows are readouts, not actions).
#[derive(Debug, Default)]
pub struct WeatherHourlyScreen {
    first: usize,
}

impl WeatherHourlyScreen {
    pub fn new() -> Self {
        WeatherHourlyScreen::default()
    }

    pub fn handle(&mut self, g: Gesture, _cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Step(n) => {
                // Clamped scroll (no wrap): the list is a timeline, and a flick off the end
                // teleporting back to "now" would misread as new data.
                let first = self.first as i32 + n;
                self.first = first.clamp(0, HOURLY_COUNT as i32 - 1) as usize;
                Transition::None
            }
            Gesture::Back => Transition::Pop,
            Gesture::Press | Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        let (w, h) = (rx.w, rx.h);
        let now = rx.now_utc as i64;

        let Some(snap) = rx.weather else {
            title_frame(cv, w, h, rx.t(Msg::WeatherHourly), "");
            empty_state(cv, w, h, rx.t(Msg::WeatherNoData), rx.t(Msg::WeatherNoDataSub));
            return;
        };

        // Rows start at the record covering `now`; before the bundle they start at record 0 (the
        // future is still the future), and past the represented span the list is honestly empty.
        let start = match snap.hourly_at(now) {
            Some((index, _, _)) => index,
            None if now < snap.valid_from => 0,
            None => {
                title_frame(cv, w, h, rx.t(Msg::WeatherHourly), "");
                empty_state(cv, w, h, rx.t(Msg::WeatherUpdateNeeded), rx.t(Msg::WeatherNoDataSub));
                return;
            }
        };
        let total = HOURLY_COUNT - start;

        let avail = h - LIST_TOP - 6;
        let visible = ((avail / ROW_H).max(1)) as usize;
        let row_h = avail / visible as i32;
        let first = self.first.min(total.saturating_sub(visible));
        list::list_frame(cv, w, h, rx.t(Msg::WeatherHourly), first + 1, total, visible);

        for slot in 0..visible {
            let index = start + first + slot;
            if index >= HOURLY_COUNT {
                break;
            }
            let record = &snap.hourly[index];
            let valid_at = snap.valid_from + index as i64 * HOURLY_INTERVAL_SECONDS as i64;
            let y = LIST_TOP + slot as i32 * row_h;
            draw_row(cv, rx, w, y, row_h, valid_at, record);
        }
        list::scrollbar(cv, w - 8, LIST_TOP, visible as i32 * row_h, total, first, visible);
    }
}

/// One hourly row — two visual lines in a double-height slot (owner tuning round). Left to
/// right: the local hour and the WX17 icon in their own columns, then a stacked temperature
/// (Display, line 1) over the precipitation amount (Label, line 2), then the wind cluster —
/// the direction arrow in its own fixed box, the from-octant + speed right-anchored beside it.
/// Even spacing, no separators (locked); every column degrades to `--` on its wire sentinel
/// rather than inventing a value.
fn draw_row(
    cv: &mut impl Surface,
    rx: &Render,
    _w: i32,
    y: i32,
    row_h: i32,
    valid_at: i64,
    record: &obc_formats::obcw::HourlyRecord,
) {
    use palette::*;
    let mid = y + row_h / 2;
    let offset = rx.settings.utc_offset_min;
    // The two text baselines of the stacked columns: Display cap 26 on top, Label cap 18 below,
    // the pair centred in the row.
    let line1 = y + (row_h - 50) / 2;
    let line2 = line1 + 32;

    // Local hour, two digits, its own quiet column.
    let (hh, _) = local_hour_minute(valid_at, offset);
    let mut time: heapless::String<4> = heapless::String::new();
    let _ = write!(time, "{hh:02}");
    cv.text_vcentered(&time, TIME_X, (y, row_h), Font::Label, TextAlign::Left, SUBTEXT);

    // The WX17 icon, unchanged masters at hourly scale, centred in its column.
    let icon = hourly_icon(record.condition, valid_at, offset);
    super::weather_icons::draw(
        cv,
        icon,
        ICON_CX,
        y + (row_h - super::weather_icons::MASTER_EDGE) / 2,
        super::weather_icons::HOURLY_SCALE,
        super::weather_icons::WeatherIconTheme::Parchment,
    );

    // Temperature — the row's glanceable number, Display tier on the top line (clamped to two
    // digits, so its box is bounded at 4 cells).
    match super::weather_dash::fmt_temp(record.temperature_deci_c) {
        Some(temp) => cv.text(&temp, Point::new(TEMP_X, line1), Font::Display, TextAlign::Left, INK),
        None => cv.text("--", Point::new(TEMP_X, line1), Font::Display, TextAlign::Left, SUBTEXT),
    };

    // Precipitation over the hour, `N.Nmm`, on the second line; dry hours mute to olive so wet
    // hours pop; the unavailable sentinel is an honest `--`.
    let mut precip: heapless::String<8> = heapless::String::new();
    let precip_color = if record.precipitation_tenth_mm == PRECIP_UNAVAILABLE {
        let _ = precip.push_str("--");
        SUBTEXT
    } else if record.precipitation_tenth_mm == 0 {
        let _ = precip.push_str("0.0mm");
        SUBTEXT
    } else if record.precipitation_tenth_mm >= 990 {
        let _ = precip.push_str("99+mm");
        INK
    } else {
        let _ = write!(precip, "{}.{}mm", record.precipitation_tenth_mm / 10, record.precipitation_tenth_mm % 10);
        INK
    };
    cv.text(&precip, Point::new(PRECIP_X, line2), Font::Label, TextAlign::Left, precip_color);

    // Wind: the arrow in its own fixed box (the *to*-direction on a north-up rose;
    // route-relative color against the WX12 travel direction, neutral ink without one),
    // then the meteorological from-octant + speed right-anchored clear of it. The text is
    // bounded at 5 Label cells (see the column-geometry note), so the two can never touch.
    if record.wind_from_deg == WIND_DIRECTION_UNAVAILABLE || record.wind_speed_deci_ms == WIND_SPEED_UNAVAILABLE {
        cv.text_vcentered("--", WIND_TEXT_X, (y, row_h), Font::Label, TextAlign::Right, SUBTEXT);
        return;
    }
    let color = match wind_class(record.wind_from_deg, rx.travel_deg) {
        Some(WindClass::Tail) => ON,
        Some(WindClass::Cross) => AMBER,
        Some(WindClass::Head) => RED,
        None => INK,
    };
    wind_arrow(cv, Point::new(WIND_ARROW_CX, mid), 9, record.wind_from_deg, color);
    let speed = match rx.settings.units {
        Units::Metric => (record.wind_speed_deci_ms as u32 * 36 + 50) / 100, // deci-m/s → km/h
        Units::Imperial => (record.wind_speed_deci_ms as u32 * 2_237 + 5_000) / 10_000, // → mph
    };
    let mut wind: heapless::String<8> = heapless::String::new();
    let _ = write!(wind, "{} {}", rx.t(OCTANTS[wind_octant(record.wind_from_deg)]), speed.min(99));
    cv.text_vcentered(&wind, WIND_TEXT_X, (y, row_h), Font::Label, TextAlign::Right, INK);
}

/// A small full arrow (shaft + barbs) pointing in the wind's *to*-direction on a north-up rose —
/// the POI bearing arrow's stroke idiom at a continuous angle.
fn wind_arrow(cv: &mut impl Surface, c: Point, r: i32, wind_from_deg: u16, color: u16) {
    use core::f32::consts::PI;
    let theta = (wind_from_deg as f32 + 180.0).to_radians();
    let rf = r as f32;
    let end = |from: Point, ang: f32, len: f32| {
        Point::new(
            from.x + libm::roundf(libm::sinf(ang) * len) as i32,
            from.y - libm::roundf(libm::cosf(ang) * len) as i32,
        )
    };
    let tip = end(c, theta, rf);
    let tail = end(c, theta + PI, rf);
    stroke2(cv, tail, tip, color);
    for da in [3.0 * core::f32::consts::FRAC_PI_4, -3.0 * core::f32::consts::FRAC_PI_4] {
        stroke2(cv, tip, end(tip, theta + da, rf * 0.75), color);
    }
}
