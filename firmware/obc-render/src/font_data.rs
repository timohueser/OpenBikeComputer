//! Generated Terminus pixel-font data, the typeface behind [`Font`](crate::text::Font).
//!
//! Terminus (SIL OFL, see `fonts/terminus/LICENSE`) is a bold monospace bitmap font. Each tier is
//! one Terminus BDF converted to embedded-graphics' `MonoFont` strip layout by
//! `fonts/convert_bdf.py`.
//!
//! The four text tiers ship the `latin` charset, ASCII plus Latin-1 Supplement plus Latin
//! Extended-A, so European route, ride and POI names render their accents instead of `?`. `Huge`
//! is the clock only, so it stays ASCII.
//!
//! Sizes target physical cap heights on the 240 px panel: `Caption` 1.76 mm, `Label` 2.03 mm,
//! `Body` 2.44 mm and `Display` 2.71 mm.
//!
//! Regenerate from the Terminus BDFs with `fonts/convert_bdf.py` per text cut, then
//! `fonts/double_strip.py` for the doubled clock strip.

use embedded_graphics::{
    geometry::Size,
    image::ImageRaw,
    mono_font::{
        mapping::{GlyphMapping, StrGlyphMapping, ASCII},
        DecorationDimensions, MonoFont,
    },
    pixelcolor::BinaryColor,
};

/// Glyph mapping for the text tiers, in the exact index order the converter lays the strip out in,
/// so a slot is a mapping index. Unmapped chars fall back to `?` at its ASCII index.
static LATIN: StrGlyphMapping =
    StrGlyphMapping::new("\0\u{20}\u{7f}\0\u{a0}\u{ff}\0\u{100}\u{17f}", '?' as usize - ' ' as usize);

/// Build a `MonoFont` from a converted strip. `cell` is the glyph cell, `ascent` the BDF ascent
/// and `mapping` the glyph order the strip was laid out in; the strip is `16 * w` px wide.
const fn mono(
    data: &'static [u8],
    cell: (u32, u32),
    ascent: u32,
    mapping: &'static dyn GlyphMapping,
) -> MonoFont<'static> {
    let baseline = ascent - 1;
    MonoFont {
        image: ImageRaw::<BinaryColor>::new(data, 16 * cell.0),
        glyph_mapping: mapping,
        character_size: Size::new(cell.0, cell.1),
        character_spacing: 0,
        baseline,
        underline: DecorationDimensions::new(baseline + 2, 1),
        strikethrough: DecorationDimensions::new(cell.1 / 2, 1),
    }
}

/// Terminus 10×20 bold, the `Caption` tier. A native Terminus cut rather than a reduction of a
/// larger one, so the strokes are hinted for this cell.
pub static TER_U20B: MonoFont = mono(include_bytes!("../fonts/terminus/ter_u20b.raw"), (10, 20), 16, &LATIN);

/// Terminus 12×24 bold — cap 15 px (≈ 2.0 mm). The `Label` tier.
pub static TER_U24B: MonoFont = mono(include_bytes!("../fonts/terminus/ter_u24b.raw"), (12, 24), 19, &LATIN);

/// Terminus 14×28 bold — cap 18 px (≈ 2.44 mm). The `Body` tier.
pub static TER_U28B: MonoFont = mono(include_bytes!("../fonts/terminus/ter_u28b.raw"), (14, 28), 22, &LATIN);

/// Terminus 16×32 bold, the `Display` tier and the largest native Terminus cut.
pub static TER_U32B: MonoFont = mono(include_bytes!("../fonts/terminus/ter_u32b.raw"), (16, 32), 26, &LATIN);

/// Terminus 16×32 bold integer-doubled to 32×64, the `Huge` tier for the Home-screen clock. 2× is
/// past Terminus' largest native cut, so this strip is pixel-doubled rather than rendered from the
/// BDF, and the chunky doubled edges read as deliberate at clock size. Digits and colon only.
pub static TER_U64B: MonoFont = mono(include_bytes!("../fonts/terminus/ter_u64b.raw"), (32, 64), 52, &ASCII);
