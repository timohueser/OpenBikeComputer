//! Pixel-art bike sprites for the [Bike type screen](super::bike_type), one per
//! routing profile, **matched by the profile's name** (routing-v2 follow-up). Each
//! sprite is authored as ASCII rows — a non-space cell is one ink pixel — so the art
//! is reviewable and tweakable right here in the source; [`draw`] blits it scaled with
//! run-length fills (one [`Surface::fill`](obc_render::Surface) per ink run, not per
//! pixel). The sprites are firmware-baked: the four shipped profiles
//! (Road / Gravel / MTB / Touring) each get their own silhouette, and a custom
//! web-builder profile falls back to a [`GENERIC`] bike.

use obc_render::{rect, Surface};

/// One sprite: ASCII rows, a non-space cell is an ink pixel. Rows need not be equal
/// length — [`draw`] centres on the longest row.
pub type Bike = &'static [&'static str];

/// Road bike: thin wheels, a diamond frame, drop handlebars. 33 × 18 art pixels; the
/// other silhouettes keep this frame and vary the bars, tyres, and load.
#[rustfmt::skip]
pub const ROAD: Bike = &[
    "                                 ",
    "                                 ",
    "                                 ",
    "          ####     #####         ",
    "            ########## #         ",
    "            #        # #         ",
    "           # #      # #          ",
    "          #  #      # #          ",
    "         #   #     #   #         ",
    "    ######    #   #    ######    ",
    "   #    ##    #  #     ##    #   ",
    "  #    #  #   #  #    #  #    #  ",
    "  #    #  #    ##     #  #    #  ",
    "  #   ##########      #   #   #  ",
    "  #       #    #      #       #  ",
    "  #       #    ##     #       #  ",
    "   #     #             #     #   ",
    "    #####               #####    ",
];

/// Gravel bike: drop bars (like the road) on **fat, knobby tyres** (2-px rings).
#[rustfmt::skip]
pub const GRAVEL: Bike = &[
    "                                 ",
    "                                 ",
    "                                 ",
    "          ####     #####         ",
    "            ########## #         ",
    "            #        # #         ",
    "           # #      # #          ",
    "          #  #      # #          ",
    "         #   #     #   #         ",
    "    #####     #   #    ######    ",
    "   #######    #  #     #######   ",
    "  ##     ##   #  #    ## #   ##  ",
    "  ##     ##    ##     ## #   ##  ",
    "  ##  ##########      ##  #  ##  ",
    "  ##     ##    #      ##     ##  ",
    "  ##     ##    ##     ##     ##  ",
    "   #######             #######   ",
    "    #####               #####    ",
];

/// Mountain bike: a **flat handlebar** and **fat, knobby tyres**.
#[rustfmt::skip]
pub const MTB: Bike = &[
    "                                 ",
    "                                 ",
    "                                 ",
    "          ####    #######        ",
    "            ##########           ",
    "            #        #           ",
    "           # #      # #          ",
    "          #  #      # #          ",
    "         #   #     #   #         ",
    "    #####     #   #    ######    ",
    "   #######    #  #     #######   ",
    "  ##     ##   #  #    ## #   ##  ",
    "  ##     ##    ##     ## #   ##  ",
    "  ##  ##########      ##  #  ##  ",
    "  ##     ##    #      ##     ##  ",
    "  ##     ##    ##     ##     ##  ",
    "   #######             #######   ",
    "    #####               #####    ",
];

/// Touring bike: road frame and drop bars, plus a **rear rack with a pannier** — the
/// load is the tell.
#[rustfmt::skip]
pub const TOURING: Bike = &[
    "                                 ",
    "                                 ",
    "                                 ",
    "  ######  ####     #####         ",
    "  #    #    ########## #         ",
    "  #    #    #        # #         ",
    "  ######   # #      # #          ",
    "   #  #   #  #      # #          ",
    "   #  #  #   #     #   #         ",
    "    ######    #   #    ######    ",
    "   #    ##    #  #     ##    #   ",
    "  #    #  #   #  #    #  #    #  ",
    "  #    #  #    ##     #  #    #  ",
    "  #   ##########      #   #   #  ",
    "  #       #    #      #       #  ",
    "  #       #    ##     #       #  ",
    "   #     #             #     #   ",
    "    #####               #####    ",
];

/// Generic bike for an unrecognised custom profile: a plain flat-bar frame on thin tyres.
#[rustfmt::skip]
pub const GENERIC: Bike = &[
    "                                 ",
    "                                 ",
    "                                 ",
    "          ####    #######        ",
    "            ##########           ",
    "            #        #           ",
    "           # #      # #          ",
    "          #  #      # #          ",
    "         #   #     #   #         ",
    "    ######    #   #    ######    ",
    "   #    ##    #  #     ##    #   ",
    "  #    #  #   #  #    #  #    #  ",
    "  #    #  #    ##     #  #    #  ",
    "  #   ##########      #   #   #  ",
    "  #       #    #      #       #  ",
    "  #       #    ##     #       #  ",
    "   #     #             #     #   ",
    "    #####               #####    ",
];

/// The sprite for a profile name, case-insensitive substring match; unrecognised ⇒
/// [`GENERIC`]. Keyed on the shipped default names and common synonyms so a custom
/// profile that *mentions* a bike type still gets the right art.
pub fn for_name(name: &str) -> Bike {
    let mut lower: heapless::String<20> = heapless::String::new();
    for c in name.chars().take(20) {
        for lc in c.to_lowercase() {
            let _ = lower.push(lc);
        }
    }
    let n = lower.as_str();
    if n.contains("mtb") || n.contains("mountain") {
        MTB
    } else if n.contains("gravel") || n.contains("cyclocross") || n.contains("cx") {
        GRAVEL
    } else if n.contains("tour") {
        TOURING
    } else if n.contains("road") || n.contains("race") {
        ROAD
    } else {
        GENERIC
    }
}

/// Blit `bike` centred horizontally on `center_x`, top edge at `top_y`, each art pixel
/// a `scale`×`scale` block of `color`. Contiguous ink cells in a row are filled as one
/// rectangle, so a sprite costs a handful of fills per row, not one per pixel.
pub fn draw(cv: &mut impl Surface, bike: Bike, center_x: i32, top_y: i32, scale: i32, color: u16) {
    let cols = bike.iter().map(|r| r.len()).max().unwrap_or(0) as i32;
    let x0 = center_x - cols * scale / 2;
    for (ry, row) in bike.iter().enumerate() {
        let y = top_y + ry as i32 * scale;
        let bytes = row.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] != b' ' {
                let start = i;
                while i < bytes.len() && bytes[i] != b' ' {
                    i += 1;
                }
                cv.fill(rect(x0 + start as i32 * scale, y, (i - start) as i32 * scale, scale), color);
            } else {
                i += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four shipped names (and common synonyms) map to their own sprite, case-insensitively; an
    /// unrecognised custom profile falls back to the generic bike.
    #[test]
    fn matches_shipped_names_case_insensitively() {
        // Compare by content — `const` slices have no stable address, so identify the sprite by value.
        assert_eq!(for_name("Road"), ROAD);
        assert_eq!(for_name("gravel"), GRAVEL);
        assert_eq!(for_name("MTB"), MTB);
        assert_eq!(for_name("Mountain"), MTB);
        assert_eq!(for_name("Touring"), TOURING);
        assert_eq!(for_name("Commuter"), GENERIC);
        assert_eq!(for_name(""), GENERIC);
    }

    /// Every sprite is a rectangular grid of the same dimensions — guards against a ragged ASCII row
    /// (a trimmed trailing space) that would misalign the run-length blit.
    #[test]
    fn sprites_are_uniform_rectangles() {
        let (rows, cols) = (ROAD.len(), ROAD[0].len());
        for bike in [ROAD, GRAVEL, MTB, TOURING, GENERIC] {
            assert_eq!(bike.len(), rows, "all sprites have the same row count");
            for row in bike {
                assert_eq!(row.len(), cols, "every row is the same width");
            }
        }
    }
}
