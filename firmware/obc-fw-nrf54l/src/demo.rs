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

/// A **line / box diagnostic card** (issue #155 bench) — to tell apart "the font is drawn wrong"
/// from "the panel's area-gradation cell structure is just visible on solid strokes".
///
/// The `Display` font draws as a clean 1:1 Terminus 16×32 bitmap, so any fine-line texture in it
/// must come from the panel, not the renderer. This card isolates that: it draws **full-level black**
/// (device `0,0,0` — whole cell off) bars and a box at the **same stroke widths as the font**, beside
/// the actual `Display` digits. Two outcomes:
///   - the black bars/box show the **same** striations as the font → it's the panel's per-pixel area
///     blocks (the 2/3-area MSB + 1/3-area LSB sub-cells), inherent to how it makes 64 colours;
///   - the black bars/box are **clean** while the font is striped → a real pixel/pack bug, dig in.
///
/// The right-hand column is the explicit gradation reference — solid boxes at device levels 3/2/1:
/// level 3 lights the whole cell, level 2 only the 2/3 (MSB) area, level 1 only the 1/3 (LSB) area,
/// so 2 and 1 **should** look textured/dim by design while 0 and 3 are whole-cell. The 1-px combs at
/// the bottom expose any odd/even column interleave or row-drop error (they'd break the regular
/// alternation). Same `DrawTarget` contract as [`font_palette_demo`], so it drives any backend.
// Wired into the FLPR bring-up bin's BTN0 cycle (`ls021-flpr`); the ST7789 `glass-demo` build pulls
// in this shared module but only draws `font_palette_demo`, so it's unused there.
#[cfg_attr(feature = "glass-demo", allow(dead_code))]
pub fn line_test_card<D>(target: &mut D) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    /// Fill one axis-aligned box — a free helper so it borrows `target` only per call (a closure
    /// capturing `target` would hold the borrow across the `draw_text` calls below).
    fn bar<D: DrawTarget<Color = Rgb565>>(
        t: &mut D,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        c: Rgb565,
    ) -> Result<(), D::Error> {
        t.fill_solid(&Rectangle::new(Point::new(x, y), Size::new(w, h)), c)
    }

    let white = c565(255, 255, 255); // device (3,3,3) — whole cell on
    let black = c565(0, 0, 0); // device (0,0,0) — whole cell off (== the `ink` text colour)
    let gray2 = c565(170, 170, 170); // device (2,2,2) — only the 2/3-area (MSB) block
    let gray1 = c565(85, 85, 85); // device (1,1,1) — only the 1/3-area (LSB) block
    let w = target.bounding_box().size.width as i32;

    target.clear(white)?;
    draw_text(target, "LINE / BOX TEST", Point::new(w / 2, 2), Font::Label, TextAlign::Center, black);

    // Full-level BLACK vertical bars, widths 1..8 px (the font strokes are ~3-4 px).
    let mut x = 6;
    for bw in [1u32, 2, 3, 4, 5, 6, 8] {
        bar(target, x, 22, bw, 52, black)?;
        x += bw as i32 + 14;
    }

    // Full-level BLACK horizontal bars, widths 1..8 px.
    let mut y = 84;
    for bw in [1u32, 2, 3, 4, 5, 6, 8] {
        bar(target, 6, y, 150, bw, black)?;
        y += bw as i32 + 9;
    }

    // Gradation reference column: solid boxes at device levels 0 / 2 / 1 (whole / 2-3 / 1-3 area).
    bar(target, 184, 22, 44, 40, black)?; // level 0 — whole cell off
    bar(target, 184, 70, 44, 40, gray2)?; // level 2 — 2/3-area only (should look textured)
    bar(target, 184, 118, 44, 40, gray1)?; // level 1 — 1/3-area only (should look textured/dimmer)

    // Direct A/B: a solid black box the height of the Display cap, beside the actual Display digits.
    bar(target, 6, 200, 40, 32, black)?;
    draw_text(target, "12.5", Point::new(54, 196), Font::Display, TextAlign::Left, black);

    // 1-px combs: alternating columns then alternating rows. A clean regular pattern means columns
    // and rows are addressed right; a broken/solid/doubled patch means an interleave or row-drop bug.
    let cy = 248;
    for gx in (6..150).step_by(2) {
        bar(target, gx, cy, 1, 22, black)?; // vertical comb (every other column)
    }
    for gry in (cy + 30..cy + 52).step_by(2) {
        bar(target, 6, gry, 150, 1, black)?; // horizontal comb (every other row)
    }
    Ok(())
}
