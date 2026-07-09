//! Generated Terminus pixel-font data — the real typeface behind [`Font`](crate::text::Font).
//!
//! Terminus (<https://terminus-font.sourceforge.net/>, SIL OFL — see `fonts/terminus/LICENSE`) is a
//! bold monospace bitmap font. Each tier is one Terminus BDF converted to embedded-graphics'
//! `MonoFont` strip layout (16 glyphs/row, 1bpp MSB-first) by `fonts/convert_bdf.py`.
//!
//! The three text tiers ship the **`latin` charset** — ASCII `0x20..=0x7F` + Latin-1 Supplement
//! `0xA0..=0xFF` + Latin Extended-A `0x100..=0x17F` (320 glyphs, 20 rows) — so European route,
//! ride and POI names render their umlauts and accents (ä ö ü ß é è à č š ž ł ő ű …) instead of
//! `?` (issue #489). Their glyph order matches the [`LATIN`] mapping below. `Huge` is the clock
//! only (digits + colon), so it stays ASCII-only via eg's [`mapping::ASCII`] — no wasted glyphs.
//!
//! Sizes target physical cap heights on the 240 px / 32.46 mm panel (7.39 px/mm):
//!
//! | Tier      | source       | cap px | ≈ mm | charset |
//! |-----------|--------------|--------|------|---------|
//! | `Label`   | ter-u24 bold |   15   | 2.03 | latin   |
//! | `Body`    | ter-u28 bold |   18   | 2.44 | latin   |
//! | `Display` | ter-u32 bold |   20   | 2.71 | latin   |
//!
//! Generated with `--deslash-zero` (the `0` uses the slash-free capital-`O` ring). `baseline` is
//! `ascent - 1` (eg convention); the UI draws `Baseline::Top`, so it only positions the unused
//! underline/strikethrough decorations.
//!
//! Regenerate (from the Terminus BDFs): for each text cut,
//! `python3 fonts/convert_bdf.py ter-uNNb.bdf fonts/terminus/ter_uNNb.raw --charset latin --deslash-zero`;
//! then `python3 fonts/double_strip.py fonts/terminus/ter_u32b.raw fonts/terminus/ter_u64b.raw 16 32`
//! (doubles just the ASCII rows for the clock).

use embedded_graphics::{
    geometry::Size,
    image::ImageRaw,
    mono_font::{
        mapping::{GlyphMapping, StrGlyphMapping, ASCII},
        DecorationDimensions, MonoFont,
    },
    pixelcolor::BinaryColor,
};

/// Glyph mapping for the text tiers: ASCII + Latin-1 Supplement + Latin Extended-A, in the exact
/// index order `fonts/convert_bdf.py --charset latin` lays the strip out (so slot == mapping index).
/// This is eg's built-in `ISO_8859_1` string extended with the Latin Extended-A range. Unmapped
/// chars fall back to `?` at its ASCII index, just like eg's built-in mappings.
static LATIN: StrGlyphMapping =
    StrGlyphMapping::new("\0\u{20}\u{7f}\0\u{a0}\u{ff}\0\u{100}\u{17f}", '?' as usize - ' ' as usize);

/// Build a `MonoFont` from a converted strip. `cell` is the glyph cell `(w, h)`, `ascent` the
/// BDF ascent (already ×scale for scaled cuts) and `mapping` the glyph order the strip was laid
/// out in; the strip is `16 * w` px wide (the eg 16-glyphs-per-row layout). Keeps the consts below
/// to one line each.
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

/// Terminus 12×24 bold — cap 15 px (≈ 2.0 mm). The `Label` tier.
pub static TER_U24B: MonoFont = mono(include_bytes!("../fonts/terminus/ter_u24b.raw"), (12, 24), 19, &LATIN);

/// Terminus 14×28 bold — cap 18 px (≈ 2.44 mm). The `Body` tier.
pub static TER_U28B: MonoFont = mono(include_bytes!("../fonts/terminus/ter_u28b.raw"), (14, 28), 22, &LATIN);

/// Terminus 16×32 bold — cap 20 px (≈ 2.71 mm). The `Display` tier (big numbers); the
/// largest native Terminus cut.
pub static TER_U32B: MonoFont = mono(include_bytes!("../fonts/terminus/ter_u32b.raw"), (16, 32), 26, &LATIN);

/// Terminus 16×32 bold, integer-doubled to 32×64 — cap 40 px (≈ 5.4 mm). The `Huge` tier,
/// the one oversized readout (the Home-screen clock). 2× is past Terminus' largest native cut,
/// so this strip is pixel-doubled from `ter_u32b.raw`'s ASCII rows (`fonts/double_strip.py`,
/// nearest-neighbour 2×2 blocks) rather than rendered from the BDF; the chunky doubled edges read
/// as deliberate at clock size. Digits + colon only, so it keeps eg's [`mapping::ASCII`].
/// `ascent`/`cell` are 2× the `Display` cut's.
pub static TER_U64B: MonoFont = mono(include_bytes!("../fonts/terminus/ter_u64b.raw"), (32, 64), 52, &ASCII);
