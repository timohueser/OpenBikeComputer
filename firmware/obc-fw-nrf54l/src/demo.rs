//! On-glass font + palette bring-up demo — the nRF analog of the STM32's
//! [`demo.rs`](../../obc-fw-stm32f429/src/demo.rs) (issue #33) and the simulator's
//! `--text-demo` / `--palette` modes.
//!
//! It draws two things, in frame-absolute coordinates: the **Terminus font ladder** (the three
//! size tiers, `obc_render::text`) and the device's **64-colour gamut** (the LS021B7DD02's RGB222
//! — 4 levels each of R/G/B — laid out as the simulator's `palette.rs` 8×8 grid). That validates
//! the text raster, the RGB565 colour path, and band addressing in isolation, before the whole
//! [`App::render_frame`](obc_app::App::render_frame) is pointed at the panel (N6).
//!
//! Crucially this generator is **band-oblivious**: it sizes itself off `target.bounding_box()`
//! and draws the full 240×320 frame. The nRF host streams it through [`obc_platform::Band`], which
//! reports the full frame but clips each draw to one band — so the same code drives the banded
//! ST7789 push here and would drive a full-frame SDRAM plane unchanged. Behind the `glass-demo`
//! feature.
//!
//! Unlike the LS021B7DD02 it previews, the ST7789 stand-in is a true RGB565 panel, so the gamut
//! goes on straight (no host-only device-64 quantization): the 64 swatches are drawn from the
//! same four RGB222 levels `obc_reader::rgb565_to_device64` snaps to, i.e. "what the device shows".

use embedded_graphics::{pixelcolor::Rgb565, prelude::*, primitives::Rectangle};
use obc_render::text::{draw_text, Font, TextAlign};

/// The four per-channel levels of the panel's RGB222 gamut (step 85) — exactly the values
/// `obc_reader::rgb565_to_device64` quantizes to, so drawing them straight is the device's gamut.
const LEVELS: [u8; 4] = [0, 85, 170, 255];

/// Pack an 8-bit-per-channel colour into native RGB565, so the palette below reads as plain hexes
/// (like the simulator's `rgb565()` / the STM32 demo's `c565()`).
fn c565(r: u8, g: u8, b: u8) -> Rgb565 {
    Rgb565::new(r >> 3, g >> 2, b >> 3)
}

/// Render the font ladder + 64-colour gamut onto `target` (the full frame, via any
/// [`DrawTarget<Color = Rgb565>`]): a HUD title strip, the device's 8×8 colour gamut, then the
/// three Terminus tiers — two true-size samples and a left/centre/right alignment row.
pub fn font_palette_demo<D>(target: &mut D) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    // The "explorer's field map" palette from docs/bikepacking-computer-ui-spec.md.
    let parchment = c565(0xEA, 0xDF, 0xC0);
    let hud = c565(0x2E, 0x25, 0x1A); // wood-dark HUD strip
    let ink = c565(0x2C, 0x21, 0x14);
    let amber = c565(0xE3, 0xA5, 0x2B);
    let wood = c565(0x5B, 0x3F, 0x28);

    let w = target.bounding_box().size.width as i32;

    target.clear(parchment)?;
    target.fill_solid(&Rectangle::new(Point::zero(), Size::new(w as u32, 26)), hud)?;
    draw_text(target, "OBC 64-COLOUR + FONT", Point::new(w / 2, 3), Font::Label, TextAlign::Center, parchment);

    // Device gamut: the 8×8 grid of all 64 RGB222 colours (red picks the 4×4 block, green sweeps
    // down within it, blue across), tiled by edge over [grid_top, grid_bot) so it has no seams.
    let (grid_top, grid_bot) = (30, 210);
    for row in 0..8 {
        for col in 0..8 {
            let red = LEVELS[(row / 4 * 2 + col / 4) as usize];
            let green = LEVELS[(row % 4) as usize];
            let blue = LEVELS[(col % 4) as usize];
            let (x0, x1) = (col * w / 8, (col + 1) * w / 8);
            let (y0, y1) =
                (grid_top + row * (grid_bot - grid_top) / 8, grid_top + (row + 1) * (grid_bot - grid_top) / 8);
            target.fill_solid(
                &Rectangle::new(Point::new(x0, y0), Size::new((x1 - x0) as u32, (y1 - y0) as u32)),
                c565(red, green, blue),
            )?;
        }
    }

    // Font ladder below the grid: the Display + Body tiers over true-size samples, then a Label
    // alignment row — all three Terminus sizes and the three alignments on one screen.
    draw_text(target, "12.5 km/h", Point::new(8, 216), Font::Display, TextAlign::Left, ink);
    draw_text(target, "ride 042  18.3 km", Point::new(8, 252), Font::Body, TextAlign::Left, wood);
    let align_y = 288;
    draw_text(target, "LEFT", Point::new(8, align_y), Font::Label, TextAlign::Left, ink);
    draw_text(target, "CENTER", Point::new(w / 2, align_y), Font::Label, TextAlign::Center, amber);
    draw_text(target, "RIGHT", Point::new(w - 8, align_y), Font::Label, TextAlign::Right, ink);
    Ok(())
}
