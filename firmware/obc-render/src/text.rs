//! On-screen text: the shared text primitive for the device UI.
//!
//! It wires the converted Terminus bitmap face in size tiers, so the font is a single edit here.
//! The colour is already resolved to the target's pixel type, so text quantizes to the 64-colour
//! panel exactly like the map does and stays true-colour in the simulator.

use embedded_graphics::{
    image::GetPixel,
    mono_font::{MonoFont, MonoTextStyle},
    pixelcolor::BinaryColor,
    prelude::*,
    text::{Alignment, Baseline, Text, TextStyleBuilder},
};

use crate::font_data;

/// A text size, one of five Terminus tiers. The names describe intent, not pixel sizes, so screen
/// code reads the same whichever Terminus cut each maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Font {
    /// Terminus 10×20 (cap about 1.76 mm): annotation over other content, such as settlement names.
    Caption,
    /// Terminus 12×24 (cap ≈ 2.0 mm) — dense labels, list captions, the HUD strip title.
    Label,
    /// Terminus 14×28 (cap ≈ 2.44 mm) — list / menu rows and body text.
    Body,
    /// Terminus 16×32 (cap ≈ 2.71 mm) — glanceable numbers (speed, the big stat tiles).
    Display,
    /// Terminus 32×64 (cap about 5.4 mm): the one oversized readout, the Home-screen clock,
    /// pixel-doubled from `Display`.
    Huge,
}

impl Font {
    /// The backing Terminus [`MonoFont`]: the single point the typeface is chosen.
    #[inline]
    pub(crate) fn mono(self) -> &'static MonoFont<'static> {
        match self {
            Font::Caption => &font_data::TER_U20B,
            Font::Label => &font_data::TER_U24B,
            Font::Body => &font_data::TER_U28B,
            Font::Display => &font_data::TER_U32B,
            Font::Huge => &font_data::TER_U64B,
        }
    }

    /// Glyph cell width in pixels; the face is monospace.
    #[inline]
    pub fn char_width(self) -> u32 {
        self.mono().character_size.width
    }

    /// Glyph cell height in pixels, the per-row advance for stacking lines.
    #[inline]
    pub fn line_height(self) -> u32 {
        self.mono().character_size.height
    }

    /// Height of unaccented flat capitals and digits, excluding top bearing and descenders.
    #[inline]
    pub const fn cap_height(self) -> u32 {
        match self {
            Font::Caption => 13,
            Font::Label => 15,
            Font::Body => 18,
            Font::Display => 20,
            Font::Huge => 40,
        }
    }

    /// Capital ink top relative to the cell-top anchor used by [`draw_text`].
    #[inline]
    pub fn cap_top(self) -> u32 {
        self.cap_bottom() - self.cap_height()
    }

    /// Exclusive capital ink bottom relative to the cell-top anchor.
    #[inline]
    pub fn cap_bottom(self) -> u32 {
        self.mono().baseline + 1
    }

    /// Capital ink centre relative to the cell-top anchor, for adjacent icons.
    #[inline]
    pub fn cap_mid(self) -> u32 {
        self.cap_top() + self.cap_height() / 2
    }
}

/// Horizontal placement of a string relative to its anchor's x.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAlign {
    /// Anchor is the left edge.
    Left,
    /// Anchor is the horizontal center.
    Center,
    /// Anchor is the right edge.
    Right,
}

impl TextAlign {
    #[inline]
    fn to_eg(self) -> Alignment {
        match self {
            TextAlign::Left => Alignment::Left,
            TextAlign::Center => Alignment::Center,
            TextAlign::Right => Alignment::Right,
        }
    }
}

/// Pixel width `s` occupies in `font`. Exact for the monospace face.
#[inline]
pub fn text_width(s: &str, font: Font) -> u32 {
    font.char_width() * s.chars().count() as u32
}

/// Visible vertical ink bounds of one line, relative to its cell-top anchor; blank text has none.
/// It scans only rows outside the bounds already found, so it needs no glyph table.
pub fn text_ink_bounds(s: &str, font: Font) -> Option<core::ops::Range<i32>> {
    let mono = font.mono();
    let (w, h) = (mono.character_size.width, mono.character_size.height);
    let per_row = mono.image.size().width / w;
    let (mut top, mut bottom) = (h, 0);
    for c in s.chars() {
        let glyph = mono.glyph_mapping.index(c) as u32;
        let (gx, gy) = (glyph % per_row * w, glyph / per_row * h);
        let row_has_ink =
            |y| (0..w).any(|x| mono.image.pixel(Point::new((gx + x) as i32, (gy + y) as i32)) == Some(BinaryColor::On));
        if let Some(y) = (0..top).find(|&y| row_has_ink(y)) {
            top = y;
        }
        if let Some(y) = (bottom..h).rev().find(|&y| row_has_ink(y)) {
            bottom = y + 1;
        }
    }
    (top < bottom).then_some(top as i32..bottom as i32)
}

/// Whether the text tiers can render `c` as a real glyph rather than the silent `?` fallback.
///
/// The four text tiers share one `LATIN` glyph strip, and any other char maps to `?`'s slot. This
/// reads that mapping off the actual font, so callers are pinned to the real coverage rather than
/// a hand-copied range. The ASCII-only `Huge` clock tier carries no user-facing copy.
#[inline]
pub fn glyph_supported(c: char) -> bool {
    // Any text tier shares the `LATIN` mapping, so the Body cut stands in for all of them. An
    // unmapped char resolves to `?`'s fallback slot, so `c` is covered if it lands on a different
    // slot, except `?` itself, which legitimately owns that slot.
    let mapping = Font::Body.mono().glyph_mapping;
    c == '?' || mapping.index(c) != mapping.index('?')
}

/// Draw `s` anchored at `anchor`, aligned about `anchor.x`, in the already-resolved `color`. The
/// glyph cell top sits at `anchor.y`. Returns the position just past the string for chaining runs;
/// a draw error falls back to `anchor`.
pub fn draw_text<D>(target: &mut D, s: &str, anchor: Point, font: Font, align: TextAlign, color: D::Color) -> Point
where
    D: DrawTarget,
{
    let character_style = MonoTextStyle::new(font.mono(), color);
    let text_style = TextStyleBuilder::new().alignment(align.to_eg()).baseline(Baseline::Top).build();
    Text::with_text_style(s, anchor, character_style, text_style).draw(target).unwrap_or(anchor)
}

/// Draw a text run counter-clockwise from `bottom_left`, optionally downsampling the source bitmap
/// by an integer `divisor`. Each output pixel is on when any source pixel in its `divisor` square
/// is on, so the thin Terminus strokes stay legible after reduction.
pub fn draw_text_ccw<D>(target: &mut D, s: &str, bottom_left: Point, font: Font, divisor: u32, color: D::Color)
where
    D: DrawTarget,
{
    let divisor = divisor.max(1);
    let mono = font.mono();
    let cell_w = mono.character_size.width;
    let cell_h = mono.character_size.height;
    let out_w = cell_w.div_ceil(divisor);
    let out_h = cell_h.div_ceil(divisor);
    let glyphs_per_row = mono.image.size().width / cell_w;
    let image = mono.image;

    let pixels = s.chars().enumerate().flat_map(move |(char_i, c)| {
        let glyph = mono.glyph_mapping.index(c) as u32;
        let glyph_x = glyph % glyphs_per_row * cell_w;
        let glyph_y = glyph / glyphs_per_row * cell_h;
        (0..out_w).flat_map(move |out_x| {
            (0..out_h).filter_map(move |out_y| {
                let on = (0..divisor).any(|dx| {
                    let source_x = out_x * divisor + dx;
                    source_x < cell_w
                        && (0..divisor).any(|dy| {
                            let source_y = out_y * divisor + dy;
                            source_y < cell_h
                                && image.pixel(Point::new((glyph_x + source_x) as i32, (glyph_y + source_y) as i32))
                                    == Some(BinaryColor::On)
                        })
                });
                on.then(|| {
                    let run_x = char_i as i32 * out_w as i32 + out_x as i32;
                    Pixel(Point::new(bottom_left.x + out_y as i32, bottom_left.y - 1 - run_x), color)
                })
            })
        })
    });
    let _ = target.draw_iter(pixels);
}
