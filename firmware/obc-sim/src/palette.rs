//! `--palette`: show the device's 64-color gamut and nothing else.
//!
//! A standalone preview mode — no map, no app, no control panel. It fills a
//! device-sized [`Framebuffer`] with every color the LS021B7DD02 can display
//! (RGB222: 4 levels each of red/green/blue → 64 colors) and either writes it to
//! a PNG (`--png`, headless) or shows it in a minimal window. The on-device
//! counterpart of the factory color-test screen.

use eframe::egui;
use embedded_graphics::{pixelcolor::Rgb888, prelude::*, primitives::Rectangle};

use crate::framebuffer::Framebuffer;

/// The four per-channel levels of the panel's RGB222 gamut. Each step is 85, so
/// these are exactly the values `obc_reader::rgb565_to_device64` quantizes to —
/// i.e. drawing them straight is already "what the device shows".
const LEVELS: [u8; 4] = [0, 85, 170, 255];

/// Fill `fb` with all 64 colors, laid out as a 2×2 grid of 4×4 blocks: one block
/// per red level (top-left 0 … bottom-right 255), each block sweeping green down
/// and blue across. The cells are computed by edge so they tile the whole buffer
/// with no gaps for any width/height.
pub fn draw_palette(fb: &mut Framebuffer) {
    let (w, h) = (fb.width() as i32, fb.height() as i32);
    for row in 0..8 {
        for col in 0..8 {
            // Red picks the block (row/col halves); green/blue sweep within it.
            let red = LEVELS[(row / 4 * 2 + col / 4) as usize];
            let green = LEVELS[(row % 4) as usize];
            let blue = LEVELS[(col % 4) as usize];
            let (x0, x1) = (col * w / 8, (col + 1) * w / 8);
            let (y0, y1) = (row * h / 8, (row + 1) * h / 8);
            let _ = fb.fill_solid(
                &Rectangle::new(Point::new(x0, y0), Size::new((x1 - x0) as u32, (y1 - y0) as u32)),
                Rgb888::new(red, green, blue),
            );
        }
    }
}

/// Launch a minimal window showing the palette at integer scale. Esc or Q closes it. The
/// framebuffer is drawn once up front (the mode is static).
pub fn run(width: u32, height: u32, scale: u32) -> Result<(), eframe::Error> {
    let win = [(width * scale) as f32, (height * scale) as f32];
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("OBC Palette — 64 colors").with_inner_size(win),
        ..Default::default()
    };
    let mut fb = Framebuffer::new(width, height);
    draw_palette(&mut fb);
    eframe::run_native(
        "OBC Palette",
        options,
        Box::new(move |_cc| Ok(Box::new(PaletteGui { fb, scale, texture: None }) as Box<dyn eframe::App>)),
    )
}

/// The minimal viewer: a pre-drawn framebuffer blitted to a texture each frame.
struct PaletteGui {
    fb: Framebuffer,
    scale: u32,
    texture: Option<egui::TextureHandle>,
}

impl eframe::App for PaletteGui {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::Q)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        // Upload the (static) framebuffer once.
        if self.texture.is_none() {
            let img =
                egui::ColorImage::from_rgb([self.fb.width() as usize, self.fb.height() as usize], self.fb.as_rgb888());
            self.texture = Some(ctx.load_texture("palette", img, egui::TextureOptions::NEAREST));
        }
        let tex = self.texture.as_ref().expect("uploaded just above");

        egui::CentralPanel::default().frame(egui::Frame::none()).show(ctx, |ui| {
            let (w, h) = (self.fb.width() as f32, self.fb.height() as f32);
            // Largest integer scale that fits, capped at `--scale`, at least 1×.
            let avail = ui.available_size();
            let fit = (avail.x / w).min(avail.y / h).floor().clamp(1.0, self.scale as f32);
            let size = egui::vec2(w * fit, h * fit);
            let rect = egui::Rect::from_center_size(ui.available_rect_before_wrap().center(), size);
            ui.put(
                rect,
                egui::Image::new(egui::load::SizedTexture::from_handle(tex))
                    .fit_to_exact_size(size)
                    .texture_options(egui::TextureOptions::NEAREST),
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(fb: &Framebuffer, x: u32, y: u32) -> (u8, u8, u8) {
        let i = ((y * fb.width() + x) * 3) as usize;
        let b = fb.as_rgb888();
        (b[i], b[i + 1], b[i + 2])
    }

    #[test]
    fn draws_all_64_distinct_device_colors() {
        let mut fb = Framebuffer::new(240, 320);
        draw_palette(&mut fb);

        // One sample per 30×40 cell → the 64 swatches; all must be distinct and
        // each channel must be one of the four RGB222 levels.
        let mut seen = std::collections::HashSet::new();
        for row in 0..8u32 {
            for col in 0..8u32 {
                let c = pixel(&fb, col * 30 + 15, row * 40 + 20);
                assert!([0, 85, 170, 255].contains(&c.0), "red {} not a device level", c.0);
                assert!([0, 85, 170, 255].contains(&c.1), "green {} not a device level", c.1);
                assert!([0, 85, 170, 255].contains(&c.2), "blue {} not a device level", c.2);
                seen.insert(c);
            }
        }
        assert_eq!(seen.len(), 64, "expected all 64 colors exactly once");
    }

    #[test]
    fn corners_anchor_the_layout() {
        let mut fb = Framebuffer::new(240, 320);
        draw_palette(&mut fb);
        // Top-left block is red=0 starting at green=blue=0; bottom-right ends at white.
        assert_eq!(pixel(&fb, 0, 0), (0, 0, 0));
        assert_eq!(pixel(&fb, 239, 319), (255, 255, 255));
    }

    /// The edge-tiled cells (`col * w / 8 .. (col+1) * w / 8`) only chain seamlessly if the
    /// edges have no gap or overlap — non-obvious when 8 doesn't divide w/h. On 37×53, check
    /// every pixel equals the color its cell predicts (a gap or overlap would mismatch).
    #[test]
    fn tiles_without_gaps_on_non_multiple_of_8_dimensions() {
        let (w, h) = (37u32, 53u32);
        let mut fb = Framebuffer::new(w, h);
        draw_palette(&mut fb);

        // Recompute the expected (red,green,blue) for the cell a given pixel falls in, by
        // inverting the same edge math draw_palette uses.
        let cell_color = |x: u32, y: u32| -> (u8, u8, u8) {
            // Find the col whose [col*w/8, (col+1)*w/8) span contains x (likewise row/y).
            let col = (0..8).find(|&c| x >= c * w / 8 && x < (c + 1) * w / 8).expect("x in some column");
            let row = (0..8).find(|&r| y >= r * h / 8 && y < (r + 1) * h / 8).expect("y in some row");
            let red = LEVELS[(row / 4 * 2 + col / 4) as usize];
            let green = LEVELS[(row % 4) as usize];
            let blue = LEVELS[(col % 4) as usize];
            (red, green, blue)
        };

        for y in 0..h {
            for x in 0..w {
                assert_eq!(pixel(&fb, x, y), cell_color(x, y), "gap/overlap at ({x},{y}) on {w}x{h}");
            }
        }
    }
}
