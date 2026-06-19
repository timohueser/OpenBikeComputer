//! Generated Terminus pixel-font data — the real typeface behind [`Font`](crate::text::Font).
//!
//! Terminus (<https://terminus-font.sourceforge.net/>, SIL OFL — see `fonts/terminus/LICENSE`)
//! is a bold, highly legible monospace bitmap font in the misc-fixed lineage of the old
//! embedded-graphics built-ins, but with much larger cuts. Each tier here is one Terminus
//! BDF converted to embedded-graphics' `MonoFont` strip layout (printable ASCII
//! `0x20..=0x7F`, 16 glyphs per row over 6 rows, 1bpp MSB-first) by `fonts/convert_bdf.py`,
//! so we reuse eg's public [`mapping::ASCII`] and ship only the bitmap (~17 KB total —
//! trivial on the nRF54L flash).
//!
//! Sizes target physical cap heights on the 240 px / 32.46 mm panel (7.39 px/mm):
//!
//! | Tier      | source       | cap px | ≈ mm |
//! |-----------|--------------|--------|------|
//! | `Label`   | ter-u24 bold |   15   | 2.03 |
//! | `Body`    | ter-u28 bold |   18   | 2.44 |
//! | `Display` | ter-u32 bold |   20   | 2.71 |
//!
//! All three are native Terminus cuts (16×32 is its largest), kept crisp at 1× — chosen
//! over scaling up for sharper edges and narrower glyphs. They are generated with
//! `--deslash-zero` (the `0` uses the slash-free capital-`O` ring). `baseline` is
//! `ascent - 1` (the eg convention); the UI draws `Baseline::Top`, so it only positions the
//! unused underline/strikethrough decorations. The converter's `--scale` path
//! (`fonts/convert_bdf.py`) stays available if a larger tier is ever wanted.
//!
//! Regenerate (from the Terminus BDFs): for each cut,
//! `python3 fonts/convert_bdf.py ter-uNNb.bdf fonts/terminus/ter_uNNb.raw --deslash-zero`.

use embedded_graphics::{
    geometry::Size,
    image::ImageRaw,
    mono_font::{mapping::ASCII, DecorationDimensions, MonoFont},
    pixelcolor::BinaryColor,
};

/// Build a `MonoFont` from a converted strip. `cell` is the glyph cell `(w, h)` and
/// `ascent` the BDF ascent (already ×scale for scaled cuts); the strip is `16 * w` px wide
/// (the eg 16-glyphs-per-row layout). Keeps the three consts below to one line each.
const fn mono(data: &'static [u8], cell: (u32, u32), ascent: u32) -> MonoFont<'static> {
    let baseline = ascent - 1;
    MonoFont {
        image: ImageRaw::<BinaryColor>::new(data, 16 * cell.0),
        glyph_mapping: &ASCII,
        character_size: Size::new(cell.0, cell.1),
        character_spacing: 0,
        baseline,
        underline: DecorationDimensions::new(baseline + 2, 1),
        strikethrough: DecorationDimensions::new(cell.1 / 2, 1),
    }
}

/// Terminus 12×24 bold — cap 15 px (≈ 2.0 mm). The `Label` tier.
pub static TER_U24B: MonoFont =
    mono(include_bytes!("../fonts/terminus/ter_u24b.raw"), (12, 24), 19);

/// Terminus 14×28 bold — cap 18 px (≈ 2.44 mm). The `Body` tier.
pub static TER_U28B: MonoFont =
    mono(include_bytes!("../fonts/terminus/ter_u28b.raw"), (14, 28), 22);

/// Terminus 16×32 bold — cap 20 px (≈ 2.71 mm). The `Display` tier (big numbers); the
/// largest native Terminus cut.
pub static TER_U32B: MonoFont =
    mono(include_bytes!("../fonts/terminus/ter_u32b.raw"), (16, 32), 26);
