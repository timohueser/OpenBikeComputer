//! On-screen text — the shared text primitive for the device UI.
//!
//! The map renderer draws only geometry; every non-map screen (menus, the
//! elevation stats, Ride control) needs text. This wires `embedded-graphics`'
//! built-in monospace fonts as **stand-ins** for the converted pixel font
//! (m5x7 / m3x6 / Pixellari) the device UI will ultimately ship — see
//! `docs/ui_framework_brief.md`. Routing every screen's text through this one
//! module means swapping the stand-ins for the real pixel font is a single edit
//! here, not a sweep across call sites.
//!
//! Like [`MapRenderer::draw_marker`](crate::MapRenderer::draw_marker), the color
//! is already resolved to the target's pixel type: the caller maps a style/palette
//! RGB565 through the host's `color_fn`, so text quantizes to the 64-color panel
//! exactly like the map does and stays true-color in the simulator. The
//! slice-1 check (`obcm-render/tests/text.rs`, the `--text-demo` preview) confirms
//! a palette color drawn this way survives the device-64 quantization intact.

use embedded_graphics::{
    mono_font::{ascii, MonoFont, MonoTextStyle},
    prelude::*,
    text::{Alignment, Baseline, Text, TextStyleBuilder},
};

/// A text size. Three monospace stand-ins until the converted pixel font lands;
/// the names describe intent (`Label` / `Body` / `Display`), not pixel sizes, so
/// screen code keeps reading right after the font swap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Font {
    /// Smallest — dense labels, list captions, the HUD strip title.
    Label,
    /// Mid — list rows and body text.
    Body,
    /// Largest — glanceable numbers (speed, time, the big stat tiles).
    Display,
}

impl Font {
    /// The backing embedded-graphics mono font (stand-in for the pixel font).
    #[inline]
    fn mono(self) -> &'static MonoFont<'static> {
        match self {
            Font::Label => &ascii::FONT_6X10,
            Font::Body => &ascii::FONT_9X15,
            Font::Display => &ascii::FONT_10X20,
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
    let text_style = TextStyleBuilder::new()
        .alignment(align.to_eg())
        .baseline(Baseline::Top)
        .build();
    Text::with_text_style(s, anchor, character_style, text_style)
        .draw(target)
        .unwrap_or(anchor)
}
