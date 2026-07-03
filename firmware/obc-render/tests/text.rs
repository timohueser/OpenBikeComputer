//! Smoke tests for the shared text primitive ([`obc_render::text`]). Draws into an in-memory
//! `DrawTarget` and asserts the glyphs land where they should, in the color the caller resolved —
//! including that a palette color quantized through the device-64 `color_fn` reaches the panel
//! intact (the same path the map styles take).

use embedded_graphics::{pixelcolor::Rgb888, prelude::*};
use obc_reader::{rgb565_to_device64, rgb565_to_rgb888};
use obc_render::text::{draw_text, text_width, Font, TextAlign};

mod common;
use common::Buf;

const RED: Rgb888 = Rgb888::new(255, 0, 0);

#[test]
fn draws_glyphs_inside_the_font_cell() {
    // A single "A" in the Body font, top-left at (2, 2), stays within its one-cell
    // box — proves something was drawn and that the top baseline puts the glyph
    // where the anchor says. The buffer fits the tallest tier's cell with margin.
    let mut b = Buf::new(64, 48);
    draw_text(&mut b, "A", Point::new(2, 2), Font::Body, TextAlign::Left, RED);
    let (minx, miny, maxx, maxy) = b.bbox(RED).expect("glyph should draw pixels");
    assert!(minx >= 2 && miny >= 2, "glyph starts at/after the anchor ({minx},{miny})");
    assert!(
        maxx < 2 + Font::Body.char_width() as i32 && maxy < 2 + Font::Body.line_height() as i32,
        "glyph fits in one cell ({maxx},{maxy})"
    );
}

#[test]
fn empty_string_draws_nothing() {
    let mut b = Buf::new(32, 16);
    draw_text(&mut b, "", Point::new(0, 0), Font::Label, TextAlign::Left, RED);
    assert_eq!(b.count(RED), 0);
}

#[test]
fn drawn_extent_fits_text_width() {
    // `text_width` is exact for the mono stand-in: the drawn ink never spills past
    // the advertised width, and the width scales linearly with the glyph count.
    let s = "MENU";
    let mut b = Buf::new(128, 16);
    draw_text(&mut b, s, Point::new(0, 0), Font::Label, TextAlign::Left, RED);
    let (_, _, maxx, _) = b.bbox(RED).expect("text should draw");
    assert!((maxx as u32) < text_width(s, Font::Label));
    assert_eq!(text_width("MMMM", Font::Label), 4 * Font::Label.char_width());
    assert!(text_width("MM", Font::Label) > text_width("M", Font::Label));
}

#[test]
fn quantized_palette_color_reaches_the_panel() {
    // Text drawn in a palette color resolved through the device-64 `color_fn` shows up in *that
    // quantized* color, not the true-color one — so text honors the 64-color gamut like map styles.
    let amber_565 = rgb565(0xE3, 0xA5, 0x2B); // accent amber #E3A52B
    let q = rgb565_to_device64(amber_565);
    let t = rgb565_to_rgb888(amber_565);
    assert_ne!(q, t, "test is only meaningful if quantization changes the color");

    let quantized = Rgb888::new(q.0, q.1, q.2);
    let mut b = Buf::new(32, 40); // fits the Display cell (16×32) at (2, 2)
    draw_text(&mut b, "8", Point::new(2, 2), Font::Display, TextAlign::Left, quantized);
    assert!(b.count(quantized) > 0, "the quantized color should be painted");
    assert_eq!(b.count(Rgb888::new(t.0, t.1, t.2)), 0, "the un-quantized true-color version must not appear");
}

#[test]
fn center_and_right_align_about_the_anchor() {
    let s = "OK";
    // Center: the glyphs straddle the anchor x.
    let mut c = Buf::new(64, 32);
    draw_text(&mut c, s, Point::new(32, 2), Font::Body, TextAlign::Center, RED);
    let (cminx, _, cmaxx, _) = c.bbox(RED).expect("centered text should draw");
    assert!(cminx < 32 && cmaxx > 32, "centered text straddles x=32 (got {cminx}..{cmaxx})");

    // Right: the glyphs end at/left of the anchor x.
    let mut r = Buf::new(64, 32);
    draw_text(&mut r, s, Point::new(40, 2), Font::Body, TextAlign::Right, RED);
    let (_, _, rmaxx, _) = r.bbox(RED).expect("right-aligned text should draw");
    assert!(rmaxx <= 40, "right-aligned text ends at x<=40 (got {rmaxx})");
}

/// Pack 8-bit RGB into RGB565 (the style/format color space the renderer quantizes from).
fn rgb565(r: u8, g: u8, b: u8) -> u16 {
    (((r as u16) >> 3) << 11) | (((g as u16) >> 2) << 5) | ((b as u16) >> 3)
}
