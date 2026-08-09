//! Canonical 24 x 24 weather icon masters (WX17, issue #1204).
//!
//! This module is the one weather-art authority shared by firmware and simulator. Every glyph is
//! drawn from deterministic integer pixel primitives into [`Surface`]; a dashboard icon is the
//! same master at [`DASHBOARD_SCALE`], never a separately rasterized asset. The approved grammar is
//! the restrained "Trail" outline family with solid celestial bodies and detached diagonal sun
//! glints. There are no screen-layout, weather-fetch, storage, BLE, or server concerns here.

use obc_formats::obcw;
use obc_render::{rect, Surface};

use super::palette;

/// Edge length of every master, in art pixels.
pub const MASTER_EDGE: i32 = 24;
/// Hourly rows render one device pixel per art pixel.
pub const HOURLY_SCALE: i32 = 1;
/// Dashboard cards render the identical master as 2 x 2 device-pixel blocks.
pub const DASHBOARD_SCALE: i32 = 2;

const SKY: u16 = palette::rgb565(0, 110, 230); // -> RGB222 (0,85,255)
const ICE: u16 = palette::rgb565(90, 220, 255); // -> RGB222 (85,255,255)
const CLOUD: u16 = palette::rgb565(170, 170, 170); // -> RGB222 (170,170,170)
const CLOUD_DARK: u16 = palette::rgb565(85, 85, 85); // -> RGB222 (85,85,85)
const MOON: u16 = palette::rgb565(170, 170, 255); // -> RGB222 (170,170,255)

/// Whether a condition is shown in its daylight or night-time context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DayPhase {
    Day,
    Night,
}

/// The two backgrounds the approved family is contrast-tuned against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeatherIconTheme {
    Parchment,
    Hud,
}

impl WeatherIconTheme {
    pub const fn background(self) -> u16 {
        match self {
            Self::Parchment => palette::PARCHMENT,
            Self::Hud => palette::HUD,
        }
    }
}

/// Every distinct contextual glyph in the approved family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeatherIcon {
    ClearDay,
    ClearNight,
    MostlyClearDay,
    MostlyClearNight,
    PartlyCloudyDay,
    PartlyCloudyNight,
    Overcast,
    Fog,
    Drizzle,
    Rain,
    Sleet,
    Snow,
    Showers,
    Thunderstorm,
    Hail,
    Wind,
    Unavailable,
}

impl WeatherIcon {
    /// Stable review and test order: contextual clear states first, then WX2 wire order.
    pub const ALL: [Self; 17] = [
        Self::ClearDay,
        Self::ClearNight,
        Self::MostlyClearDay,
        Self::MostlyClearNight,
        Self::PartlyCloudyDay,
        Self::PartlyCloudyNight,
        Self::Overcast,
        Self::Fog,
        Self::Drizzle,
        Self::Rain,
        Self::Sleet,
        Self::Snow,
        Self::Showers,
        Self::Thunderstorm,
        Self::Hail,
        Self::Wind,
        Self::Unavailable,
    ];
}

/// One row of the canonical WX2 condition -> icon mapping table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConditionIconMapping {
    pub condition: u8,
    pub day: WeatherIcon,
    pub night: WeatherIcon,
}

/// Wire-code order, followed by the `0xFF` unavailable sentinel.
///
/// Only clear, mostly-clear, and partly-cloudy have contextual day/night art. Conditions that do
/// not encode a celestial state deliberately map to the same glyph in both columns.
pub const CONDITION_ICON_MAP: [ConditionIconMapping; 14] = [
    ConditionIconMapping {
        condition: obcw::CONDITION_CLEAR,
        day: WeatherIcon::ClearDay,
        night: WeatherIcon::ClearNight,
    },
    ConditionIconMapping {
        condition: obcw::CONDITION_MOSTLY_CLEAR,
        day: WeatherIcon::MostlyClearDay,
        night: WeatherIcon::MostlyClearNight,
    },
    ConditionIconMapping {
        condition: obcw::CONDITION_PARTLY_CLOUDY,
        day: WeatherIcon::PartlyCloudyDay,
        night: WeatherIcon::PartlyCloudyNight,
    },
    same(obcw::CONDITION_OVERCAST, WeatherIcon::Overcast),
    same(obcw::CONDITION_FOG, WeatherIcon::Fog),
    same(obcw::CONDITION_DRIZZLE, WeatherIcon::Drizzle),
    same(obcw::CONDITION_RAIN, WeatherIcon::Rain),
    same(obcw::CONDITION_SLEET, WeatherIcon::Sleet),
    same(obcw::CONDITION_SNOW, WeatherIcon::Snow),
    same(obcw::CONDITION_SHOWERS, WeatherIcon::Showers),
    same(obcw::CONDITION_THUNDERSTORM, WeatherIcon::Thunderstorm),
    same(obcw::CONDITION_HAIL, WeatherIcon::Hail),
    same(obcw::CONDITION_WIND, WeatherIcon::Wind),
    same(obcw::CONDITION_UNAVAILABLE, WeatherIcon::Unavailable),
];

const fn same(condition: u8, icon: WeatherIcon) -> ConditionIconMapping {
    ConditionIconMapping { condition, day: icon, night: icon }
}

/// Resolve a WX2 wire condition and daylight context. Unknown future codes fail visibly as the
/// unavailable glyph rather than borrowing a misleading known condition.
pub fn icon_for(condition: u8, phase: DayPhase) -> WeatherIcon {
    let unavailable = &CONDITION_ICON_MAP[CONDITION_ICON_MAP.len() - 1];
    let mapping = if condition <= obcw::CONDITION_WIND { &CONDITION_ICON_MAP[condition as usize] } else { unavailable };
    match phase {
        DayPhase::Day => mapping.day,
        DayPhase::Night => mapping.night,
    }
}

#[derive(Clone, Copy)]
struct Theme {
    bg: u16,
    ink: u16,
    cloud: u16,
}

fn colors(theme: WeatherIconTheme) -> Theme {
    match theme {
        WeatherIconTheme::Parchment => Theme { bg: palette::PARCHMENT, ink: palette::INK, cloud: CLOUD },
        WeatherIconTheme::Hud => Theme { bg: palette::HUD, ink: palette::PARCHMENT, cloud: CLOUD_DARK },
    }
}

struct Pixels<'a, S: Surface> {
    cv: &'a mut S,
    ox: i32,
    oy: i32,
    scale: i32,
}

impl<'a, S: Surface> Pixels<'a, S> {
    fn new(cv: &'a mut S, center_x: i32, top_y: i32, scale: i32) -> Self {
        Self { cv, ox: center_x - MASTER_EDGE * scale / 2, oy: top_y, scale }
    }

    fn px(&mut self, x: i32, y: i32, color: u16) {
        self.cv.fill(rect(self.ox + x * self.scale, self.oy + y * self.scale, self.scale, self.scale), color);
    }

    fn run(&mut self, x: i32, y: i32, len: i32, color: u16) {
        self.cv.fill(rect(self.ox + x * self.scale, self.oy + y * self.scale, len * self.scale, self.scale), color);
    }

    fn block(&mut self, x: i32, y: i32, width: i32, height: i32, color: u16) {
        self.cv.fill(
            rect(self.ox + x * self.scale, self.oy + y * self.scale, width * self.scale, height * self.scale),
            color,
        );
    }

    fn disc(&mut self, cx: i32, cy: i32, radius: i32, color: u16) {
        let limit = radius * radius + radius / 2;
        for y in -radius..=radius {
            let mut start = None;
            let mut end = 0;
            for x in -radius..=radius {
                if x * x + y * y <= limit {
                    start.get_or_insert(x);
                    end = x;
                }
            }
            if let Some(start) = start {
                self.run(cx + start, cy + y, end - start + 1, color);
            }
        }
    }

    fn ring(&mut self, cx: i32, cy: i32, radius: i32, thickness: i32, color: u16) {
        let outer = radius * radius + radius / 2;
        let inner_radius = radius - thickness;
        let inner = inner_radius * inner_radius - inner_radius / 2;
        for y in -radius..=radius {
            for x in -radius..=radius {
                let distance = x * x + y * y;
                if distance <= outer && distance >= inner {
                    self.px(cx + x, cy + y, color);
                }
            }
        }
    }

    fn line(&mut self, mut x0: i32, mut y0: i32, x1: i32, y1: i32, color: u16) {
        let dx = (x1 - x0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let dy = -(y1 - y0).abs();
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut error = dx + dy;
        loop {
            self.px(x0, y0, color);
            if x0 == x1 && y0 == y1 {
                break;
            }
            let twice = 2 * error;
            if twice >= dy {
                error += dy;
                x0 += sx;
            }
            if twice <= dx {
                error += dx;
                y0 += sy;
            }
        }
    }

    fn line2(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: u16) {
        self.line(x0, y0, x1, y1, color);
        if (x1 - x0).abs() >= (y1 - y0).abs() {
            self.line(x0, y0 + 1, x1, y1 + 1, color);
        } else {
            self.line(x0 + 1, y0, x1 + 1, y1, color);
        }
    }
}

fn sun(p: &mut Pixels<'_, impl Surface>, cx: i32, cy: i32, small: bool, theme: Theme) {
    let radius = if small { 3 } else { 5 };
    p.disc(cx, cy, radius + 1, theme.ink);
    p.disc(cx, cy, radius, palette::AMBER);

    // Owner-approved compact halo: four detached, staggered diagonal two-pixel glints. Keeping
    // these off the cardinal axes avoids a crosshair/registration-mark silhouette.
    let halo = radius + 1;
    for (dx, dy) in [(1, -1), (1, 1), (-1, 1), (-1, -1)] {
        p.px(cx + dx * halo, cy + dy * (halo + 1), theme.ink);
        p.px(cx + dx * (halo + 1), cy + dy * halo, theme.ink);
    }
}

fn moon(p: &mut Pixels<'_, impl Surface>, cx: i32, cy: i32, small: bool, theme: Theme) {
    let radius = if small { 4 } else { 6 };
    let carve = if small { 2 } else { 4 };
    let carve_x = if small { 2 } else { 3 };
    p.disc(cx, cy, radius, theme.ink);
    p.disc(cx, cy, radius - 1, MOON);
    p.disc(cx + carve_x, cy - 2, carve, theme.bg);
}

fn cloud(p: &mut Pixels<'_, impl Surface>, x: i32, y: i32, small: bool, theme: Theme) {
    let outline = if small {
        &[
            ((1, 8), (1, 6)),
            ((1, 6), (4, 4)),
            ((4, 4), (7, 4)),
            ((7, 4), (9, 1)),
            ((9, 1), (12, 1)),
            ((12, 1), (15, 5)),
            ((15, 5), (18, 5)),
            ((18, 5), (19, 8)),
            ((19, 8), (18, 10)),
            ((18, 10), (3, 10)),
            ((3, 10), (1, 8)),
        ][..]
    } else {
        &[
            ((1, 11), (1, 8)),
            ((1, 8), (4, 5)),
            ((4, 5), (8, 5)),
            ((8, 5), (10, 1)),
            ((10, 1), (14, 1)),
            ((14, 1), (18, 7)),
            ((18, 7), (20, 7)),
            ((20, 7), (22, 10)),
            ((22, 10), (20, 13)),
            ((20, 13), (4, 13)),
            ((4, 13), (1, 11)),
        ][..]
    };
    for &(from, to) in outline {
        p.line2(x + from.0, y + from.1, x + to.0, y + to.1, theme.ink);
    }
    if small {
        p.run(x + 4, y + 8, 13, theme.cloud);
    } else {
        p.run(x + 5, y + 10, 14, theme.cloud);
    }
}

fn snowflake(p: &mut Pixels<'_, impl Surface>, x: i32, y: i32, color: u16) {
    p.run(x - 2, y, 5, color);
    for yy in y - 2..=y + 2 {
        p.px(x, yy, color);
    }
}

fn precipitation(p: &mut Pixels<'_, impl Surface>, icon: WeatherIcon, theme: Theme) {
    match icon {
        WeatherIcon::Drizzle => {
            for x in [5, 11, 17] {
                p.block(x, 17 + (x / 6) % 2, 2, 2, SKY);
            }
        }
        WeatherIcon::Rain | WeatherIcon::Showers => {
            for x in [5, 12, 19] {
                p.line2(x, 16, x - 2, 22, SKY);
            }
        }
        WeatherIcon::Sleet => {
            p.line2(5, 16, 3, 22, SKY);
            snowflake(p, 12, 20, theme.ink);
            p.line2(20, 16, 18, 22, SKY);
        }
        WeatherIcon::Snow => {
            for (x, y) in [(5, 19), (12, 21), (19, 19)] {
                snowflake(p, x, y, theme.ink);
            }
        }
        WeatherIcon::Hail => {
            for (x, y) in [(5, 19), (12, 21), (19, 19)] {
                p.ring(x, y, 2, 1, ICE);
            }
        }
        _ => {}
    }
}

/// Draw one approved icon centred on `center_x`, with its master top at `top_y`.
///
/// `scale` must be a positive integer. [`HOURLY_SCALE`] and [`DASHBOARD_SCALE`] are the locked
/// product sizes. The caller owns the background fill and must pass the matching `theme` so the
/// moon cutout and contrast ink agree with it.
pub fn draw(cv: &mut impl Surface, icon: WeatherIcon, center_x: i32, top_y: i32, scale: i32, theme: WeatherIconTheme) {
    debug_assert!(scale > 0, "weather icon scale must be positive");
    let theme = colors(theme);
    let mut p = Pixels::new(cv, center_x, top_y, scale);
    match icon {
        WeatherIcon::ClearDay => sun(&mut p, 12, 11, false, theme),
        WeatherIcon::ClearNight => moon(&mut p, 10, 12, false, theme),
        WeatherIcon::MostlyClearDay => {
            sun(&mut p, 6, 6, true, theme);
            cloud(&mut p, 3, 8, true, theme);
        }
        WeatherIcon::MostlyClearNight => {
            moon(&mut p, 6, 7, true, theme);
            cloud(&mut p, 3, 8, true, theme);
        }
        WeatherIcon::PartlyCloudyDay => {
            sun(&mut p, 6, 6, true, theme);
            cloud(&mut p, 0, 7, false, theme);
        }
        WeatherIcon::PartlyCloudyNight => {
            moon(&mut p, 5, 6, true, theme);
            cloud(&mut p, 0, 7, false, theme);
        }
        WeatherIcon::Overcast => {
            cloud(&mut p, 2, 1, true, theme);
            cloud(&mut p, 0, 9, true, theme);
            p.run(5, 21, 14, theme.cloud);
        }
        WeatherIcon::Fog => {
            for (x, y, width) in [(4, 5, 16), (1, 10, 18), (6, 15, 17), (2, 20, 18)] {
                p.run(x, y, width, theme.ink);
                p.run(x + 3, y + 2, width - 5, theme.cloud);
            }
        }
        WeatherIcon::Drizzle | WeatherIcon::Rain | WeatherIcon::Sleet | WeatherIcon::Snow | WeatherIcon::Hail => {
            cloud(&mut p, 0, 0, false, theme);
            precipitation(&mut p, icon, theme);
        }
        WeatherIcon::Showers => {
            sun(&mut p, 6, 6, true, theme);
            cloud(&mut p, 0, 1, false, theme);
            precipitation(&mut p, icon, theme);
        }
        WeatherIcon::Thunderstorm => {
            cloud(&mut p, 0, 0, false, theme);
            p.line2(12, 14, 9, 19, palette::YELLOW);
            p.line2(9, 19, 14, 19, palette::YELLOW);
            p.line2(14, 19, 11, 23, palette::YELLOW);
            p.line(4, 17, 3, 22, SKY);
            p.line(21, 17, 19, 22, SKY);
        }
        WeatherIcon::Wind => {
            p.ring(8, 8, 4, 2, theme.ink);
            p.block(8, 6, 5, 6, theme.bg);
            for (x0, y, x1) in [(2, 8, 18), (6, 13, 22), (2, 18, 17)] {
                p.line2(x0, y, x1, y, SKY);
            }
            p.line2(18, 8, 21, 10, SKY);
            p.line2(17, 18, 20, 20, SKY);
        }
        WeatherIcon::Unavailable => {
            p.block(4, 4, 16, 2, theme.ink);
            p.block(4, 18, 16, 2, theme.ink);
            p.block(4, 6, 2, 12, theme.ink);
            p.block(18, 6, 2, 12, theme.ink);
            p.line2(8, 8, 16, 16, palette::WARNING);
            p.line2(16, 8, 8, 16, palette::WARNING);
        }
    }
}

#[cfg(test)]
mod tests {
    use embedded_graphics::{prelude::Point, primitives::Rectangle};
    use obc_render::text::{Font, TextAlign};

    use super::*;

    struct Capture<const EDGE: usize> {
        pixels: [[u16; EDGE]; EDGE],
    }

    impl<const EDGE: usize> Capture<EDGE> {
        fn new(background: u16) -> Self {
            Self { pixels: [[background; EDGE]; EDGE] }
        }
    }

    impl<const EDGE: usize> Surface for Capture<EDGE> {
        fn clear(&mut self, color: u16) {
            self.pixels = [[color; EDGE]; EDGE];
        }

        fn fill(&mut self, area: Rectangle, color: u16) {
            if area.size.width == 0 || area.size.height == 0 {
                return;
            }
            let bottom_right = area.bottom_right().expect("non-empty rectangle");
            assert!(area.top_left.x >= 0 && area.top_left.y >= 0, "weather master underflow: {area:?}");
            assert!(
                bottom_right.x < EDGE as i32 && bottom_right.y < EDGE as i32,
                "weather master overflow: {area:?} outside {EDGE} x {EDGE}"
            );
            for y in area.top_left.y..=bottom_right.y {
                for x in area.top_left.x..=bottom_right.x {
                    self.pixels[y as usize][x as usize] = color;
                }
            }
        }

        fn round(&mut self, _: Rectangle, _: u32, _: u16) {
            unreachable!("weather icons use only rectangular art-pixel fills")
        }
        fn round_outline(&mut self, _: Rectangle, _: u32, _: u16) {
            unreachable!("weather icons use only rectangular art-pixel fills")
        }
        fn line(&mut self, _: Point, _: Point, _: u16) {
            unreachable!("weather icons use their deterministic art-pixel line")
        }
        fn triangle(&mut self, _: Point, _: Point, _: Point, _: u16) {
            unreachable!("weather icons use only rectangular art-pixel fills")
        }
        fn disc(&mut self, _: Point, _: u32, _: u16) {
            unreachable!("weather icons use their deterministic art-pixel disc")
        }
        fn text(&mut self, _: &str, _: Point, _: Font, _: TextAlign, _: u16) -> Point {
            unreachable!("weather icons contain no text")
        }
    }

    #[test]
    fn wx2_mapping_table_is_complete_and_contextual_only_where_intended() {
        let expected = [
            (obcw::CONDITION_CLEAR, WeatherIcon::ClearDay, WeatherIcon::ClearNight),
            (obcw::CONDITION_MOSTLY_CLEAR, WeatherIcon::MostlyClearDay, WeatherIcon::MostlyClearNight),
            (obcw::CONDITION_PARTLY_CLOUDY, WeatherIcon::PartlyCloudyDay, WeatherIcon::PartlyCloudyNight),
            (obcw::CONDITION_OVERCAST, WeatherIcon::Overcast, WeatherIcon::Overcast),
            (obcw::CONDITION_FOG, WeatherIcon::Fog, WeatherIcon::Fog),
            (obcw::CONDITION_DRIZZLE, WeatherIcon::Drizzle, WeatherIcon::Drizzle),
            (obcw::CONDITION_RAIN, WeatherIcon::Rain, WeatherIcon::Rain),
            (obcw::CONDITION_SLEET, WeatherIcon::Sleet, WeatherIcon::Sleet),
            (obcw::CONDITION_SNOW, WeatherIcon::Snow, WeatherIcon::Snow),
            (obcw::CONDITION_SHOWERS, WeatherIcon::Showers, WeatherIcon::Showers),
            (obcw::CONDITION_THUNDERSTORM, WeatherIcon::Thunderstorm, WeatherIcon::Thunderstorm),
            (obcw::CONDITION_HAIL, WeatherIcon::Hail, WeatherIcon::Hail),
            (obcw::CONDITION_WIND, WeatherIcon::Wind, WeatherIcon::Wind),
            (obcw::CONDITION_UNAVAILABLE, WeatherIcon::Unavailable, WeatherIcon::Unavailable),
        ];
        for (mapping, &(condition, day, night)) in CONDITION_ICON_MAP.iter().zip(&expected) {
            assert_eq!((mapping.condition, mapping.day, mapping.night), (condition, day, night));
            assert_eq!(icon_for(condition, DayPhase::Day), day);
            assert_eq!(icon_for(condition, DayPhase::Night), night);
        }
        for future in [13, 42, 254] {
            assert_eq!(icon_for(future, DayPhase::Day), WeatherIcon::Unavailable);
            assert_eq!(icon_for(future, DayPhase::Night), WeatherIcon::Unavailable);
        }
    }

    #[test]
    fn every_master_stays_in_24_pixels_and_doubles_exactly() {
        assert_eq!(MASTER_EDGE, 24);
        for theme in [WeatherIconTheme::Parchment, WeatherIconTheme::Hud] {
            for icon in WeatherIcon::ALL {
                let mut hourly = Capture::<24>::new(theme.background());
                draw(&mut hourly, icon, 12, 0, HOURLY_SCALE, theme);
                assert!(hourly.pixels.iter().flatten().any(|&color| color != theme.background()));

                let mut dashboard = Capture::<48>::new(theme.background());
                draw(&mut dashboard, icon, 24, 0, DASHBOARD_SCALE, theme);
                for y in 0..24 {
                    for x in 0..24 {
                        let expected = hourly.pixels[y][x];
                        assert_eq!(dashboard.pixels[y * 2][x * 2], expected);
                        assert_eq!(dashboard.pixels[y * 2][x * 2 + 1], expected);
                        assert_eq!(dashboard.pixels[y * 2 + 1][x * 2], expected);
                        assert_eq!(dashboard.pixels[y * 2 + 1][x * 2 + 1], expected);
                    }
                }
            }
        }
    }

    #[test]
    fn approved_suns_have_only_the_eight_detached_diagonal_glint_pixels_outside_the_disk() {
        for (small, radius) in [(false, 5), (true, 3)] {
            let mut capture = Capture::<24>::new(palette::PARCHMENT);
            let mut pixels = Pixels::new(&mut capture, 12, 0, 1);
            sun(&mut pixels, 12, 12, small, colors(WeatherIconTheme::Parchment));

            let outer = radius + 1;
            let disc_limit = outer * outer + outer / 2;
            let halo = radius + 1;
            let expected = [
                (12 + halo, 12 - halo - 1),
                (12 + halo + 1, 12 - halo),
                (12 + halo, 12 + halo + 1),
                (12 + halo + 1, 12 + halo),
                (12 - halo, 12 + halo + 1),
                (12 - halo - 1, 12 + halo),
                (12 - halo, 12 - halo - 1),
                (12 - halo - 1, 12 - halo),
            ];
            let mut outside_count = 0;
            for y in 0..24 {
                for x in 0..24 {
                    let dx = x as i32 - 12;
                    let dy = y as i32 - 12;
                    if dx * dx + dy * dy > disc_limit && capture.pixels[y][x] != palette::PARCHMENT {
                        outside_count += 1;
                        assert!(expected.contains(&(x as i32, y as i32)), "unexpected ray/glint at ({x},{y})");
                    }
                }
            }
            assert_eq!(outside_count, expected.len());
        }
    }

    #[test]
    fn every_rendered_color_quantizes_to_rgb222() {
        const LEVELS: [u8; 4] = [0, 85, 170, 255];
        for theme in [WeatherIconTheme::Parchment, WeatherIconTheme::Hud] {
            for icon in WeatherIcon::ALL {
                let mut capture = Capture::<24>::new(theme.background());
                draw(&mut capture, icon, 12, 0, HOURLY_SCALE, theme);
                for color in capture.pixels.iter().flatten() {
                    let (r, g, b) = obc_reader::rgb565_to_device64(*color);
                    assert!(LEVELS.contains(&r) && LEVELS.contains(&g) && LEVELS.contains(&b));
                }
            }
        }
    }
}
