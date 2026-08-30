//! On-screen text — the shared text primitive for the device UI.
//!
//! Wires the converted **Terminus** pixel font (a bold monospace bitmap face; see
//! [`font_data`](crate::font_data)) in size tiers. Routing every screen's text through this module
//! makes the font a single edit here ([`Font::mono`]).
//!
//! The color is already resolved to the target's pixel type: the caller maps a palette RGB565
//! through the host's `color_fn`, so text quantizes to the 64-color panel exactly like the map does
//! and stays true-color in the simulator.

use embedded_graphics::{
    image::GetPixel,
    mono_font::{MonoFont, MonoTextStyle},
    pixelcolor::BinaryColor,
    prelude::*,
    text::{Alignment, Baseline, Text, TextStyleBuilder},
};

use crate::font_data;

/// A text size — one of three Terminus tiers. The names describe intent
/// (`Label` / `Body` / `Display`), not pixel sizes, so screen code reads the same
/// regardless of which Terminus cut each maps to (see [`Font::mono`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Font {
    /// Terminus 12×24 (cap ≈ 2.0 mm) — dense labels, list captions, the HUD strip title.
    Label,
    /// Terminus 14×28 (cap ≈ 2.44 mm) — list / menu rows and body text.
    Body,
    /// Terminus 16×32 (cap ≈ 2.71 mm) — glanceable numbers (speed, the big stat tiles).
    Display,
    /// Terminus 32×64 (cap ≈ 5.4 mm) — the one oversized readout: the Home-screen clock.
    /// Pixel-doubled from `Display` (see [`font_data::TER_U64B`](crate::font_data)).
    Huge,
}

impl Font {
    /// The backing Terminus [`MonoFont`] — the single point the typeface is chosen.
    #[inline]
    pub(crate) fn mono(self) -> &'static MonoFont<'static> {
        match self {
            Font::Label => &font_data::TER_U24B,
            Font::Body => &font_data::TER_U28B,
            Font::Display => &font_data::TER_U32B,
            Font::Huge => &font_data::TER_U64B,
        }
    }

    /// Glyph cell width in pixels (monospace — every glyph is this wide).
    #[inline]
    pub fn char_width(self) -> u32 {
        self.mono().character_size.width
    }

    /// Glyph cell height in pixels — the per-row advance for stacking lines.
    #[inline]
    pub fn line_height(self) -> u32 {
        self.mono().character_size.height
    }

    /// Cap height in pixels — the vertical span the glyphs actually occupy (≤ the cell
    /// [`line_height`](Self::line_height)), for centring text in a cell. Approximate but stable.
    #[inline]
    pub fn cap_height(self) -> u32 {
        match self {
            Font::Label => 18,
            Font::Body => 22,
            Font::Display => 26,
            Font::Huge => 52, // 2× Display; the Home clock only
        }
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

/// Whether the text tiers can render `c` as a real glyph rather than the silent `?` fallback.
///
/// The `Label` / `Body` / `Display` tiers share the one `LATIN` glyph strip added in #489/#601
/// (ASCII `0x20..=0x7f` + Latin-1 Supplement `0xa0..=0xff` + Latin Extended-A `0x100..=0x17f`);
/// any other char maps to `?`'s slot and paints as `?`. This reads that mapping off the **actual
/// font**, so callers (e.g. the i18n repertoire test) are pinned to the real coverage, not a
/// hand-copied range that could drift from the strip. The ASCII-only `Huge` clock tier is not
/// consulted — it carries no user-facing copy.
#[inline]
pub fn glyph_supported(c: char) -> bool {
    // Any text tier shares the `LATIN` mapping; the Body cut stands in for all three. An
    // unmapped char resolves to `?`'s fallback slot — so `c` is covered iff it lands on a
    // different slot, except `?` itself, which legitimately owns that slot. (`index` resolves
    // on the `&dyn GlyphMapping` field, so its trait needs no import here.)
    let mapping = Font::Body.mono().glyph_mapping;
    c == '?' || mapping.index(c) != mapping.index('?')
}

/// Draw `s` anchored at `anchor`, in `font`, aligned `align` about `anchor.x`, in the
/// already-resolved `color`. The text's **top** sits at `anchor.y` (top baseline), so layout reads
/// as "y = row top". Returns the position just past the string for chaining runs; a draw error
/// falls back to `anchor`.
pub fn draw_text<D>(target: &mut D, s: &str, anchor: Point, font: Font, align: TextAlign, color: D::Color) -> Point
where
    D: DrawTarget,
{
    let character_style = MonoTextStyle::new(font.mono(), color);
    let text_style = TextStyleBuilder::new().alignment(align.to_eg()).baseline(Baseline::Top).build();
    Text::with_text_style(s, anchor, character_style, text_style).draw(target).unwrap_or(anchor)
}

/// Draw a text run counter-clockwise from `bottom_left`, optionally downsampling the source bitmap
/// by an integer `divisor`. Peak View uses the Label face at 2:1, yielding a compact 6x12 label
/// without shipping a fourth font strip. Each output pixel is on when any source pixel in its
/// `divisor` square is on, so the thin Terminus strokes remain legible after reduction.
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
