//! Emergency information remains readable without a map or a connected phone.

use super::{
    palette::*,
    vocab::{chrome::title_frame, marquee::fit},
    Ctx, Prepare, Render, Transition,
};
use crate::{Gesture, Msg};
use core::fmt::Write;
use embedded_graphics::prelude::Point;
use heapless::String;
use obc_map_scene::{cos_lat, delta_m, ground_dist_m_cl, BBox};
use obc_ports::Fix;
use obc_reader::Settlement;
use obc_render::{
    rect,
    text::{text_width, Font, TextAlign},
    Surface,
};

#[derive(Debug, Default)]
pub struct HelpScreen {
    pub(crate) signal: bool,
    position: Option<(i32, i32)>,
    age_s: Option<u32>,
    live: bool,
    elevation: Option<i32>,
    queried: Option<(i32, i32)>,
    nearest: Option<Settlement>,
}

impl HelpScreen {
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn refresh(&mut self, fix: Option<Fix>, age: Option<u32>, live: bool, elevation: Option<i32>) -> bool {
        let position = fix
            .filter(|f| (-90_000_000..=90_000_000).contains(&f.lat) && (-180_000_000..=180_000_000).contains(&f.lon))
            .map(|f| (f.lon, f.lat));
        let live = live && position.is_some();
        let changed = (self.position, self.age_s, self.live, self.elevation) != (position, age, live, elevation);
        self.position = position;
        self.age_s = age;
        self.live = live;
        self.elevation = elevation;
        changed
    }

    pub(crate) fn needs_reader(&self) -> bool {
        self.position.is_some_and(|(lon, lat)| self.queried != Some((lon / 1_000, lat / 1_000)))
    }

    pub(crate) fn prepare(&mut self, px: &mut Prepare) {
        if !self.needs_reader() {
            return;
        }
        let Some((lon, lat)) = self.position else { return };
        self.queried = Some((lon / 1_000, lat / 1_000));
        self.nearest = None;
        let Some(reader) = px.reader else { return };
        let cl = cos_lat(lat).max(0.01);
        let dx = (90_000.0 / cl) as i32;
        let bounds = BBox {
            min_lon: lon.saturating_sub(dx).max(-180_000_000),
            max_lon: lon.saturating_add(dx).min(180_000_000),
            min_lat: lat.saturating_sub(90_000).max(-90_000_000),
            max_lat: lat.saturating_add(90_000).min(90_000_000),
        };
        let mut best = 10_000.0;
        let result = reader.visit_settlements_in(&bounds, |place| {
            let distance = ground_dist_m_cl((lon, lat), (place.lon, place.lat), cl);
            if distance < best {
                best = distance;
                self.nearest = Some(place);
            }
        });
        if result.is_err() {
            self.nearest = None;
        }
    }

    pub fn handle(&mut self, gesture: Gesture, cx: &mut Ctx) -> Transition {
        match gesture {
            Gesture::Back => return Transition::Pop,
            Gesture::Press if cx.state.sound_available => self.signal = !self.signal,
            _ => {}
        }
        Transition::None
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        title_frame(cv, rx.w, rx.h, rx.t(Msg::HelpTitle), "");
        // The number block reserves two rows for 112 and a regional emergency number.
        let numbers = [("112", Msg::HelpEmergency)];
        for (row, (number, label)) in numbers.iter().enumerate() {
            let y = 44 + row as i32 * 32;
            cv.text(number, Point::new(14, y), Font::Display, TextAlign::Left, INK);
            cv.text(rx.t(*label), Point::new(76, y + 4), Font::Label, TextAlign::Left, INK);
        }
        if numbers.len() == 1 {
            cv.text(rx.t(Msg::HelpPhone), Point::new(14, 80), Font::Caption, TextAlign::Left, SUBTEXT);
        }
        cv.line(Point::new(14, 108), Point::new(rx.w - 14, 108), RULE);
        cv.text("WGS84 / DD", Point::new(14, 114), Font::Caption, TextAlign::Left, SUBTEXT);
        if let Some((lon, lat)) = self.position {
            let degrees = decimal_pair(lat, lon);
            let font =
                if text_width(&degrees, Font::Label) <= (rx.w - 28) as u32 { Font::Label } else { Font::Caption };
            cv.text(&degrees, Point::new(14, 136), font, TextAlign::Left, INK);
            cv.text("DMS", Point::new(14, 164), Font::Caption, TextAlign::Left, SUBTEXT);
            cv.text(&dms(lat, 'N', 'S'), Point::new(54, 164), Font::Caption, TextAlign::Left, INK);
            cv.text(&dms(lon, 'E', 'W'), Point::new(54, 184), Font::Caption, TextAlign::Left, INK);
            let status = if self.live { Msg::HelpCurrent } else { Msg::HelpLastKnown };
            let mut age = String::<48>::new();
            let _ = write!(age, "{}", rx.t(status));
            if let Some(seconds) = self.age_s {
                if seconds < 60 {
                    let _ = write!(age, " {}s", seconds);
                } else if seconds < 3_600 {
                    let _ = write!(age, " {}m", seconds / 60);
                } else {
                    let _ = write!(age, " {}h", seconds / 3_600);
                }
            }
            cv.text(&age, Point::new(14, 210), Font::Caption, TextAlign::Left, INK);
            if let Some(place) = &self.nearest {
                let (east, north) = delta_m((place.lon, place.lat), (lon, lat), cos_lat(lat));
                let distance = libm::sqrtf(east * east + north * north);
                let bearing = (libm::atan2f(east, north).to_degrees() + 360.0) % 360.0;
                let direction = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"][((bearing + 22.5) / 45.0) as usize % 8];
                let mut line = String::<64>::new();
                let _ = write!(line, "{:.1}km {} {}", distance / 1_000.0, direction, place.name);
                cv.text(
                    &fit(&line, rx.w - 28, Font::Caption),
                    Point::new(14, 234),
                    Font::Caption,
                    TextAlign::Left,
                    INK,
                );
            }
            if !self.live {
                cv.text(rx.t(Msg::HelpAcquiring), Point::new(14, 256), Font::Caption, TextAlign::Left, SUBTEXT);
            }
            if let Some(metres) = self.elevation.filter(|_| self.live) {
                let mut height = String::<20>::new();
                let _ = write!(height, "{} m", metres);
                cv.text(&height, Point::new(14, 256), Font::Caption, TextAlign::Left, INK);
            }
        } else {
            cv.text(rx.t(Msg::HelpAcquiring), Point::new(14, 140), Font::Label, TextAlign::Left, INK);
            cv.text(rx.t(Msg::HelpNoPosition), Point::new(14, 176), Font::Caption, TextAlign::Left, SUBTEXT);
        }
        let label = if !rx.state.sound_available {
            Msg::HelpNoSound
        } else if self.signal {
            Msg::HelpStop
        } else {
            Msg::HelpSignal
        };
        let area = rect(10, rx.h - 40, rx.w - 20, 32);
        cv.round(area, 6, if self.signal { WARNING } else { AMBER });
        let font = if text_width(rx.t(label), Font::Label) <= (rx.w - 28) as u32 { Font::Label } else { Font::Caption };
        cv.text(rx.t(label), Point::new(rx.w / 2, rx.h - 37), font, TextAlign::Center, ON_ACCENT);
    }
}

fn decimal_pair(lat: i32, lon: i32) -> String<32> {
    let mut out = String::new();
    for (index, coordinate) in [lat, lon].into_iter().enumerate() {
        if index != 0 {
            let _ = out.push_str(", ");
        }
        if coordinate < 0 {
            let _ = out.push('-');
        }
        let rounded = (coordinate.unsigned_abs() + 5) / 10;
        let _ = write!(out, "{}.{:05}", rounded / 100_000, rounded % 100_000);
    }
    out
}

fn dms(coordinate: i32, positive: char, negative: char) -> String<24> {
    let seconds = (u64::from(coordinate.unsigned_abs()) * 3_600 + 500_000) / 1_000_000;
    let mut out = String::new();
    let _ = write!(
        out,
        "{}°{:02}'{:02}\"{}",
        seconds / 3_600,
        seconds / 60 % 60,
        seconds % 60,
        if coordinate < 0 { negative } else { positive }
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_and_stale_positions_never_use_the_map_camera() {
        let mut screen = HelpScreen::new();
        screen.refresh(None, None, false, None);
        assert_eq!(screen.position, None);
        assert!(!screen.needs_reader());
        let fix = Fix::at(46_590_800, 8_327_300);
        assert!(screen.refresh(Some(fix), Some(0), true, Some(1764)));
        assert!(screen.live);
        assert!(screen.refresh(Some(fix), Some(30), false, Some(1764)));
        assert!(!screen.live);
        assert_eq!(screen.position, Some((fix.lon, fix.lat)));
        assert_eq!(screen.age_s, Some(30));
        screen.refresh(Some(Fix::at(91_000_000, 0)), Some(0), true, None);
        assert_eq!(screen.position, None);
        assert!(!screen.live);
    }

    #[test]
    fn readout_preserves_hemispheres_and_carries_rounded_seconds() {
        assert_eq!(decimal_pair(-1, -179_999_999).as_str(), "-0.00000, -180.00000");
        assert_eq!(decimal_pair(46_590_800, 8_327_300).as_str(), "46.59080, 8.32730");
        assert_eq!(dms(8_999_999, 'E', 'W').as_str(), "9°00'00\"E");
        assert_eq!(dms(-1, 'N', 'S').as_str(), "0°00'00\"S");
        assert_eq!(dms(-180_000_000, 'E', 'W').as_str(), "180°00'00\"W");
    }
}
