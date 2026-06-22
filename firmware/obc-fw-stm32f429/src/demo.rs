//! On-glass font + palette bring-up demo — the device analog of the simulator's
//! `--text-demo` (`obc-sim/src/main.rs::render_text_demo`).
//!
//! Issue #33 listed "the existing font & palette demo" as a board-bring-up
//! deliverable, but the merged bring-up only landed the raw colour-bar test
//! pattern. This brings it onto glass: the Terminus font ladder + the "explorer's
//! field map" palette (`docs/bikepacking-computer-ui-spec.md`), drawn through the
//! SDRAM [`Framebuffer565`](obc_platform::Framebuffer565) with `obc_render::text`.
//! It verifies the text raster + the RGB565 colour path in isolation, before the
//! whole [`App::render_frame`](obc_app::App::render_frame) is pointed at the panel.
//!
//! Unlike the simulator — which previews the LS021B7DD02's device-64 (RGB222)
//! gamut via `rgb565_to_device64` — this draws the palette in **native RGB565**:
//! the ILI9341 is a true 5/6/5 panel, so the colours go on straight (no host-only
//! quantization). Reached via the `glass-demo` cargo feature.

use embedded_graphics::{pixelcolor::Rgb565, prelude::*, primitives::Rectangle};
use obc_render::text::{draw_text, Font, TextAlign};

/// Pack an 8-bit-per-channel colour into native RGB565 — so the palette below can
/// be written as the spec's hexes, exactly like the simulator's `rgb565()` helper.
fn c565(r: u8, g: u8, b: u8) -> Rgb565 {
    Rgb565::new(r >> 3, g >> 2, b >> 3)
}

/// Render the font ladder + palette onto `target`, mirroring the simulator's
/// `render_text_demo`: a HUD strip + title, the three font tiers over true-size
/// samples (annotated with their cap heights in mm), the named palette colours
/// each drawn in their own colour, and a left/center/right alignment row.
pub fn font_palette_demo<D>(target: &mut D) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    // The "explorer's field map" palette from docs/bikepacking-computer-ui-spec.md.
    let parchment = c565(0xEA, 0xDF, 0xC0);
    let hud = c565(0x2E, 0x25, 0x1A); // wood-dark HUD strip
    let ink = c565(0x2C, 0x21, 0x14);
    let amber = c565(0xE3, 0xA5, 0x2B);
    let forest = c565(0x4F, 0x6B, 0x43);
    let wood = c565(0x5B, 0x3F, 0x28);
    let warning = c565(0xC0, 0x49, 0x2E);

    let w = target.bounding_box().size.width as i32;

    target.clear(parchment)?;
    target.fill_solid(&Rectangle::new(Point::zero(), Size::new(w as u32, 28)), hud)?;
    draw_text(target, "TERMINUS FONT DEMO", Point::new(w / 2, 3), Font::Label, TextAlign::Center, parchment);

    // Font ladder: each tier's caption (in Label) over a true-size sample drawn in
    // that tier, annotated with its measured cap height in mm.
    let sample = "12.5 km/h";
    let mut y = 36;
    for (caption, font) in [
        ("Label  ter24  2.0mm", Font::Label),
        ("Body   ter28  2.4mm", Font::Body),
        ("Disply ter32  2.7mm", Font::Display),
    ] {
        draw_text(target, caption, Point::new(8, y), Font::Label, TextAlign::Left, wood);
        y += Font::Label.line_height() as i32 + 2;
        draw_text(target, sample, Point::new(8, y), font, TextAlign::Left, ink);
        y += font.line_height() as i32 + 8;
    }

    // Palette — each name drawn in its own colour, so the panel shows whether
    // amber, forest, wood and warning stay distinct and legible on glass.
    for (name, col) in [("amber", amber), ("forest", forest), ("wood", wood), ("warning", warning)] {
        draw_text(target, name, Point::new(8, y), Font::Label, TextAlign::Left, col);
        y += Font::Label.line_height() as i32 + 2;
    }

    // Alignment row, mirroring the menu counter / stat labels.
    y += 6;
    draw_text(target, "LEFT", Point::new(8, y), Font::Label, TextAlign::Left, ink);
    draw_text(target, "CENTER", Point::new(w / 2, y), Font::Label, TextAlign::Center, ink);
    draw_text(target, "RIGHT", Point::new(w - 8, y), Font::Label, TextAlign::Right, ink);
    Ok(())
}
