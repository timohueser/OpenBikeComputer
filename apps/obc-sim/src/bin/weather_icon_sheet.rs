//! Deterministic WX17 proof-sheet renderer.
//!
//! This binary draws the production `obc_app::screen::weather_icons` masters through the ordinary
//! simulator framebuffer and RGB222 quantizer. With no arguments it writes the two committed
//! 240 x 320 fixtures under `apps/obc-sim/assets/weather-icons`; `--check` compares fresh pixels
//! against those fixtures without rewriting them.

#[allow(dead_code)]
#[path = "../framebuffer.rs"]
mod framebuffer;

use std::{env, fs, path::Path};

use embedded_graphics::pixelcolor::Rgb888;
use framebuffer::Framebuffer;
use obc_app::screen::{
    palette,
    weather_icons::{self, WeatherIcon, WeatherIconTheme},
};
use obc_reader::rgb565_to_device64;
use obc_render::{rect, Canvas, Surface};

const PANEL_W: u32 = 240;
const PANEL_H: u32 = 320;
const FAMILY_NAME: &str = "wx17-approved-diagonal-glints-family-24px.png";
const DASHBOARD_NAME: &str = "wx17-approved-diagonal-glints-dashboard-48px.png";

const ICONS: [(WeatherIcon, &str); 17] = [
    (WeatherIcon::ClearDay, "DAY"),
    (WeatherIcon::ClearNight, "NGT"),
    (WeatherIcon::MostlyClearDay, "M-D"),
    (WeatherIcon::MostlyClearNight, "M-N"),
    (WeatherIcon::PartlyCloudyDay, "P-D"),
    (WeatherIcon::PartlyCloudyNight, "P-N"),
    (WeatherIcon::Overcast, "CLD"),
    (WeatherIcon::Fog, "FOG"),
    (WeatherIcon::Drizzle, "DRZ"),
    (WeatherIcon::Rain, "RAIN"),
    (WeatherIcon::Sleet, "SLT"),
    (WeatherIcon::Snow, "SNW"),
    (WeatherIcon::Showers, "SHW"),
    (WeatherIcon::Thunderstorm, "TST"),
    (WeatherIcon::Hail, "HAIL"),
    (WeatherIcon::Wind, "WIND"),
    (WeatherIcon::Unavailable, "N/A"),
];

const DASHBOARD_ICONS: [(WeatherIcon, &str); 6] = [
    (WeatherIcon::ClearDay, "DAY"),
    (WeatherIcon::ClearNight, "NGT"),
    (WeatherIcon::PartlyCloudyDay, "PART"),
    (WeatherIcon::Rain, "RAIN"),
    (WeatherIcon::Snow, "SNOW"),
    (WeatherIcon::Thunderstorm, "TSTM"),
];

fn glyph(ch: char) -> [u8; 5] {
    match ch {
        'A' => [0b010, 0b101, 0b111, 0b101, 0b101],
        'B' => [0b110, 0b101, 0b110, 0b101, 0b110],
        'C' => [0b011, 0b100, 0b100, 0b100, 0b011],
        'D' => [0b110, 0b101, 0b101, 0b101, 0b110],
        'E' => [0b111, 0b100, 0b110, 0b100, 0b111],
        'F' => [0b111, 0b100, 0b110, 0b100, 0b100],
        'G' => [0b011, 0b100, 0b101, 0b101, 0b011],
        'H' => [0b101, 0b101, 0b111, 0b101, 0b101],
        'I' => [0b111, 0b010, 0b010, 0b010, 0b111],
        'J' => [0b001, 0b001, 0b001, 0b101, 0b010],
        'K' => [0b101, 0b101, 0b110, 0b101, 0b101],
        'L' => [0b100, 0b100, 0b100, 0b100, 0b111],
        'M' => [0b101, 0b111, 0b111, 0b101, 0b101],
        'N' => [0b101, 0b111, 0b111, 0b111, 0b101],
        'O' => [0b010, 0b101, 0b101, 0b101, 0b010],
        'P' => [0b110, 0b101, 0b110, 0b100, 0b100],
        'Q' => [0b010, 0b101, 0b101, 0b011, 0b001],
        'R' => [0b110, 0b101, 0b110, 0b101, 0b101],
        'S' => [0b011, 0b100, 0b010, 0b001, 0b110],
        'T' => [0b111, 0b010, 0b010, 0b010, 0b010],
        'U' => [0b101, 0b101, 0b101, 0b101, 0b111],
        'V' => [0b101, 0b101, 0b101, 0b101, 0b010],
        'W' => [0b101, 0b101, 0b111, 0b111, 0b101],
        'X' => [0b101, 0b101, 0b010, 0b101, 0b101],
        'Y' => [0b101, 0b101, 0b010, 0b010, 0b010],
        'Z' => [0b111, 0b001, 0b010, 0b100, 0b111],
        '/' => [0b001, 0b001, 0b010, 0b100, 0b100],
        '-' => [0, 0, 0b111, 0, 0],
        _ => [0; 5],
    }
}

fn tiny_text(cv: &mut impl Surface, text: &str, center_x: i32, y: i32, color: u16) {
    let width = text.chars().count() as i32 * 4 - 1;
    let mut x = center_x - width / 2;
    for ch in text.chars() {
        for (row, bits) in glyph(ch).iter().enumerate() {
            for col in 0..3 {
                if bits & (1 << (2 - col)) != 0 {
                    cv.fill(rect(x + col, y + row as i32, 1, 1), color);
                }
            }
        }
        x += 4;
    }
}

fn family_sheet() -> Framebuffer {
    let mut fb = Framebuffer::new(PANEL_W, PANEL_H);
    let quantized = |color| {
        let (r, g, b) = rgb565_to_device64(color);
        Rgb888::new(r, g, b)
    };
    {
        let mut cv = Canvas::new(&mut fb, &quantized);
        cv.clear(palette::HUD);
        cv.fill(rect(0, 0, 120, PANEL_H as i32), palette::PARCHMENT);
        cv.fill(rect(0, 0, 120, 18), palette::AMBER);
        cv.fill(rect(120, 0, 120, 18), palette::CONTOUR);
        tiny_text(&mut cv, "TRAIL LIGHT", 60, 6, palette::INK);
        tiny_text(&mut cv, "TRAIL DARK", 180, 6, palette::PARCHMENT);

        for (index, &(icon, label)) in ICONS.iter().enumerate() {
            let col = index % 3;
            let row = index / 3;
            for theme in [WeatherIconTheme::Parchment, WeatherIconTheme::Hud] {
                let x_offset = if theme == WeatherIconTheme::Hud { 120 } else { 0 };
                let center_x = x_offset + col as i32 * 40 + 20;
                let cell_y = 20 + row as i32 * 50;
                weather_icons::draw(&mut cv, icon, center_x, cell_y + 3, weather_icons::HOURLY_SCALE, theme);
                let ink = if theme == WeatherIconTheme::Hud { palette::PARCHMENT } else { palette::INK };
                tiny_text(&mut cv, label, center_x, cell_y + 39, ink);
            }
        }
        cv.vline(119, 0, PANEL_H as i32, 1, palette::AMBER);
    }
    fb
}

fn dashboard_sheet() -> Framebuffer {
    let mut fb = Framebuffer::new(PANEL_W, PANEL_H);
    let quantized = |color| {
        let (r, g, b) = rgb565_to_device64(color);
        Rgb888::new(r, g, b)
    };
    {
        let mut cv = Canvas::new(&mut fb, &quantized);
        cv.clear(palette::HUD);
        cv.fill(rect(0, 0, 120, PANEL_H as i32), palette::PARCHMENT);
        cv.fill(rect(0, 0, 120, 18), palette::AMBER);
        cv.fill(rect(120, 0, 120, 18), palette::CONTOUR);
        tiny_text(&mut cv, "DASH LIGHT", 60, 6, palette::INK);
        tiny_text(&mut cv, "DASH DARK", 180, 6, palette::PARCHMENT);

        for (index, &(icon, label)) in DASHBOARD_ICONS.iter().enumerate() {
            let col = index % 2;
            let row = index / 2;
            for theme in [WeatherIconTheme::Parchment, WeatherIconTheme::Hud] {
                let x_offset = if theme == WeatherIconTheme::Hud { 120 } else { 0 };
                let center_x = x_offset + col as i32 * 60 + 30;
                let cell_y = 23 + row as i32 * 98;
                weather_icons::draw(&mut cv, icon, center_x, cell_y, weather_icons::DASHBOARD_SCALE, theme);
                let ink = if theme == WeatherIconTheme::Hud { palette::PARCHMENT } else { palette::INK };
                tiny_text(&mut cv, label, center_x, cell_y + 57, ink);
            }
        }
        cv.vline(119, 0, PANEL_H as i32, 1, palette::AMBER);
    }
    fb
}

fn pixels(fb: &Framebuffer) -> Result<image::RgbImage, String> {
    let bytes = fb.as_rgb888();
    if bytes.iter().any(|value| ![0, 85, 170, 255].contains(value)) {
        return Err("weather sheet contains a channel outside RGB222".into());
    }
    image::RgbImage::from_raw(PANEL_W, PANEL_H, bytes.to_vec()).ok_or_else(|| "framebuffer size mismatch".into())
}

fn write_or_check(fb: &Framebuffer, path: &Path, check: bool) -> Result<(), String> {
    let fresh = pixels(fb)?;
    if check {
        let committed = image::open(path).map_err(|error| format!("{}: {error}", path.display()))?.into_rgb8();
        if committed.dimensions() != (PANEL_W, PANEL_H) || committed.as_raw() != fresh.as_raw() {
            return Err(format!("{} differs; rerun without --check and review the fixtures", path.display()));
        }
        Ok(())
    } else {
        fresh.save(path).map_err(|error| format!("{}: {error}", path.display()))
    }
}

fn main() -> Result<(), String> {
    let check = env::args().any(|arg| arg == "--check");
    let out = env::args()
        .skip(1)
        .find(|arg| !arg.starts_with('-'))
        .unwrap_or_else(|| "apps/obc-sim/assets/weather-icons".to_string());
    let out = Path::new(&out);
    if !check {
        fs::create_dir_all(out).map_err(|error| format!("{}: {error}", out.display()))?;
    }
    write_or_check(&family_sheet(), &out.join(FAMILY_NAME), check)?;
    write_or_check(&dashboard_sheet(), &out.join(DASHBOARD_NAME), check)?;
    println!("WX17 weather-icon fixtures {}", if check { "match" } else { "written" });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn committed_weather_icon_fixtures_match_the_production_renderer() {
        let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/weather-icons");
        write_or_check(&family_sheet(), &assets.join(FAMILY_NAME), true).unwrap();
        write_or_check(&dashboard_sheet(), &assets.join(DASHBOARD_NAME), true).unwrap();
    }
}
