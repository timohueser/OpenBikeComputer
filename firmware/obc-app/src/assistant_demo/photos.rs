//! Fixed photo samples for the opt-in assistant and panel studies. Pixels are 00_RR_GG_BB.

use crate::screen::palette::INK;
use embedded_graphics::{
    draw_target::DrawTarget,
    pixelcolor::{IntoStorage, Rgb565, Rgb888},
    prelude::Point,
};
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Canvas, Surface,
};

#[derive(Debug, PartialEq)]
pub struct Photo {
    pub pixels: &'static [u8; 160 * 120],
    pub large_pixels: &'static [u8; 216 * 240],
    pub credit: &'static str,
    pub source: &'static str,
    pub licence: &'static str,
}

pub static AARE: Photo = Photo {
    pixels: include_bytes!("../../assets/landmarks/aare.rgb222"),
    large_pixels: include_bytes!("../../assets/landmarks/aare-large.rgb222"),
    credit: "Pazit Polak. Resized and ordered dither. CC BY-SA 2.0.",
    source: "https://commons.wikimedia.org/wiki/File:Aareschlucht_166_7.jpg",
    licence: "https://creativecommons.org/licenses/by-sa/2.0/",
};

pub static FALLS: Photo = Photo {
    pixels: include_bytes!("../../assets/landmarks/falls.rgb222"),
    large_pixels: include_bytes!("../../assets/landmarks/falls-large.rgb222"),
    credit: "Paul Hermans. Resized and ordered dither. CC BY-SA 4.0.",
    source: "https://commons.wikimedia.org/wiki/File:Schattenhalb_Reichenbachfall_7-05-2024_10-56-28.jpg",
    licence: "https://creativecommons.org/licenses/by-sa/4.0/",
};

pub static DUNLOUGH: Photo = Photo {
    pixels: include_bytes!("../../assets/landmarks/dunlough.rgb222"),
    large_pixels: include_bytes!("../../assets/landmarks/dunlough-large.rgb222"),
    credit: "Superbass / Wikimedia Commons. Resized and ordered dither. CC BY-SA 4.0.",
    source: "https://commons.wikimedia.org/wiki/File:2019-07-30-Dunlough_Castle-0819.jpg",
    licence: "https://creativecommons.org/licenses/by-sa/4.0/",
};

pub fn draw<D, F>(cv: &mut Canvas<D, F>, photo: &Photo, at: Point, large: bool)
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    let (target, color) = cv.split();
    let palette: [D::Color; 64] = core::array::from_fn(|i| {
        let rgb = Rgb888::new(((i >> 4) & 3) as u8 * 85, ((i >> 2) & 3) as u8 * 85, (i & 3) as u8 * 85);
        color(Rgb565::from(rgb).into_storage())
    });
    let (width, height, pixels): (_, _, &[u8]) =
        if large { (216, 240, photo.large_pixels) } else { (160, 120, photo.pixels) };
    let _ = target.fill_contiguous(&rect(at.x, at.y, width, height), pixels.iter().map(|&i| palette[i as usize]));
}

pub fn paragraph(cv: &mut impl Surface, text: &str, mut y: i32) {
    let mut line = heapless::String::<18>::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.len() + 1 + word.len() > line.capacity() {
            cv.text(&line, Point::new(12, y), Font::Label, TextAlign::Left, INK);
            y += 24;
            line.clear();
        }
        if !line.is_empty() {
            let _ = line.push(' ');
        }
        for chunk in word.as_bytes().chunks(18) {
            if !line.is_empty() && line.len() + chunk.len() > 18 {
                cv.text(&line, Point::new(12, y), Font::Label, TextAlign::Left, INK);
                y += 24;
                line.clear();
            }
            let _ = line.push_str(core::str::from_utf8(chunk).unwrap_or("?"));
        }
    }
    cv.text(&line, Point::new(12, y), Font::Label, TextAlign::Left, INK);
}
