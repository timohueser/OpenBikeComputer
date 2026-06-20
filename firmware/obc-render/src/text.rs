//! On-screen text — the shared text primitive for the device UI.
//!
//! The map renderer draws only geometry; every non-map screen (menus, the
//! elevation stats, Ride control) needs text. This wires the converted **Terminus**
//! pixel font (a bold monospace bitmap face in the misc-fixed lineage of the old
//! embedded-graphics built-ins; see [`font_data`](crate::font_data)) in three size
//! tiers. Routing every screen's text through this one module means the font is a
//! single edit here — [`Font::mono`] — not a sweep across call sites.
//!
//! Like [`MapRenderer::draw_marker`](crate::MapRenderer::draw_marker), the color
//! is already resolved to the target's pixel type: the caller maps a style/palette
//! RGB565 through the host's `color_fn`, so text quantizes to the 64-color panel
//! exactly like the map does and stays true-color in the simulator. The
//! slice-1 check (`obc-render/tests/text.rs`, the `--text-demo` preview) confirms
//! a palette color drawn this way survives the device-64 quantization intact.

use embedded_graphics::{
    mono_font::{MonoFont, MonoTextStyle},
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
}

impl Font {
    /// The backing Terminus [`MonoFont`] — the single point the typeface is chosen.
    #[inline]
    fn mono(self) -> &'static MonoFont<'static> {
        match self {
            Font::Label => &font_data::TER_U24B,
            Font::Body => &font_data::TER_U28B,
            Font::Display => &font_data::TER_U32B,
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
}

/// Horizontal placement of a string relative to its anchor's x.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAlign {
    /// Anchor is the left edge — labels and list rows.
    Left,
    /// Anchor is the horizontal center — screen/section headers.
    Center,
    /// Anchor is the right edge — right-justified counters and values.
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

/// Pixel width `s` occupies in `font`. Exact for the monospace stand-ins, and a
/// good layout estimate once a proportional pixel font lands — for sizing a
/// selection highlight behind a row, or hand-justifying a value.
#[inline]
pub fn text_width(s: &str, font: Font) -> u32 {
    font.char_width() * s.chars().count() as u32
}

/// Draw `s` anchored at `anchor`, in `font`, aligned `align` about `anchor.x`,
/// in the already-resolved `color`. The text's **top** sits at `anchor.y` (top
/// baseline), so screen layout reads as "y = row top" rather than a font
/// baseline. Returns the position just past the string (next glyph's origin) for
/// chaining runs; a draw error — possible only on a real display, never on the
/// host's infallible targets — falls back to `anchor`.
pub fn draw_text<D>(
    target: &mut D,
    s: &str,
    anchor: Point,
    font: Font,
    align: TextAlign,
    color: D::Color,
) -> Point
where
    D: DrawTarget,
{
    let character_style = MonoTextStyle::new(font.mono(), color);
    let text_style =
        TextStyleBuilder::new().alignment(align.to_eg()).baseline(Baseline::Top).build();
    Text::with_text_style(s, anchor, character_style, text_style).draw(target).unwrap_or(anchor)
}
