//! Pixel-art bike sprites for the [ride-start card](crate::screen::RideStartScreen), one per
//! routing profile, matched by the profile name. Each sprite is a grid of ASCII rows, where a
//! non-space cell is one ink pixel. An unknown profile gets the [`GENERIC`] bike.

use obc_render::{rect, Surface};

use crate::screen::palette;

/// One sprite: ASCII rows, where a non-space cell is an ink pixel. All sprites are the same size.
pub type Bike = &'static [&'static str];

/// Road bike: thin tyres, a diamond frame, drop handlebars.
#[rustfmt::skip]
pub const ROAD: Bike = &[
    "                                                  ",
    "                                                  ",
    "                                                  ",
    "                               #####              ",
    "                 ######          # #              ",
    "                     #######     #  #             ",
    "                    ##      ######  #             ",
    "                    ##          ##                ",
    "                   #  #        #  #               ",
    "                  #   #        #  #               ",
    "          #####   #   #       #    ######         ",
    "        ##     ###    #      #    ##     ##       ",
    "      ##        ###    #    #   ## #       ##     ",
    "      ##       # ##    #    #   ##  #      ##     ",
    "     #  #      ##  #   #   #   #  # #     #  #    ",
    "     #   #    ##   #   #  #    #   # #   #   #    ",
    "    #     #  ##     #  ###    #     ##  #     #   ",
    "    #      ###      # # ###   #      ###      #   ",
    "    #      ############## #   #      ###      #   ",
    "    #      ###      # # # #   #      ###      #   ",
    "    #     #   #     #  ###    #     #   #     #   ",
    "     #   #     #   #   #       #   #     #   #    ",
    "     #  #       #  #  ##       #  #       #  #    ",
    "      ##         ##             ##         ##     ",
    "      ##         ##             ##         ##     ",
    "        ##     ##                 ##     ##       ",
    "          #####                     #####         ",
    "                                                  ",
    "                                                  ",
    "                                                  ",
];

/// Gravel bike: drop bars on fat, knobby tyres.
#[rustfmt::skip]
pub const GRAVEL: Bike = &[
    "                                                  ",
    "                                                  ",
    "                                                  ",
    "                               #####              ",
    "                 ######          # #              ",
    "                     #######     #  #             ",
    "                    ##      ######  #             ",
    "                    ##          ##                ",
    "                   #  #        #  #               ",
    "            #     #   #        #  #   #           ",
    "          #####   #   #       #    ######         ",
    "        ##########    #      #    #########       ",
    "      ####     ####    #    #   ####     ####     ",
    "      ##       # ##    #    #   ##  #      ##     ",
    "     ## #      ## ##   #   #   ## # #     # ##    ",
    "     ##  #    ##  ##   #  #    ##  # #   #  ##    ",
    "    ##    #  ##    ##  ###    ##    ##  #    ##   ",
    "    ##     ###     ## # ###   ##     ###     ##   ",
    "   ###     ############## #  ###     ###     ###  ",
    "    ##     ###     ## # # #   ##     ###     ##   ",
    "    ##    #   #    ##  ###    ##    #   #    ##   ",
    "     ##  #     #  ##   #       ##  #     #  ##    ",
    "     ## #       # ##  ##       ## #       # ##    ",
    "      ##         ##             ##         ##     ",
    "      ####     ####             ####     ####     ",
    "        #########                 #########       ",
    "          #####                     #####         ",
    "            #                         #           ",
    "                                                  ",
    "                                                  ",
];

/// Mountain bike: a flat handlebar and fat, knobby tyres.
#[rustfmt::skip]
pub const MTB: Bike = &[
    "                                                  ",
    "                                                  ",
    "                                                  ",
    "                             #########            ",
    "                 ######          #                ",
    "                     #######     #                ",
    "                    ##      ######                ",
    "                    ##          ##                ",
    "                   #  #        #  #               ",
    "            #     #   #        #  #   #           ",
    "          #####   #   #       #    ######         ",
    "        ##########    #      #    #########       ",
    "      ####     ####    #    #   ####     ####     ",
    "      ##       # ##    #    #   ##  #      ##     ",
    "     ## #      ## ##   #   #   ## # #     # ##    ",
    "     ##  #    ##  ##   #  #    ##  # #   #  ##    ",
    "    ##    #  ##    ##  ###    ##    ##  #    ##   ",
    "    ##     ###     ## # ###   ##     ###     ##   ",
    "   ###     ############## #  ###     ###     ###  ",
    "    ##     ###     ## # # #   ##     ###     ##   ",
    "    ##    #   #    ##  ###    ##    #   #    ##   ",
    "     ##  #     #  ##   #       ##  #     #  ##    ",
    "     ## #       # ##  ##       ## #       # ##    ",
    "      ##         ##             ##         ##     ",
    "      ####     ####             ####     ####     ",
    "        #########                 #########       ",
    "          #####                     #####         ",
    "            #                         #           ",
    "                                                  ",
    "                                                  ",
];

/// Touring bike: drop bars, a rear rack and a pannier.
#[rustfmt::skip]
pub const TOURING: Bike = &[
    "                                                  ",
    "                                                  ",
    "                                                  ",
    "                               #####              ",
    "                 ######          # #              ",
    "                     #######     #  #             ",
    "                    ##      ######  #             ",
    "                    ##          ##                ",
    "                  ##  #        #  #               ",
    "     ##############   #        #  #               ",
    "     ##########   #   #       #    ######         ",
    "     #  ###    ###    #      #    ##     ##       ",
    "     ###  #     ###    #    #   ## #       ##     ",
    "     ###  #    # ##    #    #   ##  #      ##     ",
    "     #  # #    ##  #   #   #   #  # #     #  #    ",
    "     ######   ##   #   #  #    #   # #   #   #    ",
    "    #     #  ##     #  ###    #     ##  #     #   ",
    "    #      ###      # # ###   #      ###      #   ",
    "    #      ############## #   #      ###      #   ",
    "    #      ###      # # # #   #      ###      #   ",
    "    #     #   #     #  ###    #     #   #     #   ",
    "     #   #     #   #   #       #   #     #   #    ",
    "     #  #       #  #  ##       #  #       #  #    ",
    "      ##         ##             ##         ##     ",
    "      ##         ##             ##         ##     ",
    "        ##     ##                 ##     ##       ",
    "          #####                     #####         ",
    "                                                  ",
    "                                                  ",
    "                                                  ",
];

/// Generic bike for an unrecognised custom profile: a plain flat-bar frame.
#[rustfmt::skip]
pub const GENERIC: Bike = &[
    "                                                  ",
    "                                                  ",
    "                                                  ",
    "                             #########            ",
    "                 ######          #                ",
    "                     #######     #                ",
    "                    ##      ######                ",
    "                    ##          ##                ",
    "                   #  #        #  #               ",
    "                  #   #        #  #               ",
    "          #####   #   #       #    ######         ",
    "        ##     ###    #      #    ##     ##       ",
    "      ##        ###    #    #   ## #       ##     ",
    "      ##       # ##    #    #   ##  #      ##     ",
    "     #  #      ##  #   #   #   #  # #     #  #    ",
    "     #   #    ##   #   #  #    #   # #   #   #    ",
    "    #     #  ##     #  ###    #     ##  #     #   ",
    "    #      ###      # # ###   #      ###      #   ",
    "    #      ############## #   #      ###      #   ",
    "    #      ###      # # # #   #      ###      #   ",
    "    #     #   #     #  ###    #     #   #     #   ",
    "     #   #     #   #   #       #   #     #   #    ",
    "     #  #       #  #  ##       #  #       #  #    ",
    "      ##         ##             ##         ##     ",
    "      ##         ##             ##         ##     ",
    "        ##     ##                 ##     ##       ",
    "          #####                     #####         ",
    "                                                  ",
    "                                                  ",
    "                                                  ",
];

/// The bike type a profile name gives. One classifier keeps the [sprite](for_name) and the
/// [colour](color_for) in agreement.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Road,
    Gravel,
    Mtb,
    Touring,
    Generic,
}

/// Classify a profile name by a case-insensitive substring match. It accepts the shipped names and
/// common synonyms, so a custom profile that mentions a bike type still resolves.
fn kind_for(name: &str) -> Kind {
    let mut lower: heapless::String<20> = heapless::String::new();
    for c in name.chars().take(20) {
        for lc in c.to_lowercase() {
            let _ = lower.push(lc);
        }
    }
    let n = lower.as_str();
    if n.contains("mtb") || n.contains("mountain") {
        Kind::Mtb
    } else if n.contains("gravel") || n.contains("cyclocross") || n.contains("cx") {
        Kind::Gravel
    } else if n.contains("tour") {
        Kind::Touring
    } else if n.contains("road") || n.contains("race") {
        Kind::Road
    } else {
        Kind::Generic
    }
}

pub fn for_name(name: &str) -> Bike {
    match kind_for(name) {
        Kind::Road => ROAD,
        Kind::Gravel => GRAVEL,
        Kind::Mtb => MTB,
        Kind::Touring => TOURING,
        Kind::Generic => GENERIC,
    }
}

/// The ink colour for a profile's bike. A custom profile stays plain ink. The colours land on
/// clean device-64 colours over the parchment background.
pub fn color_for(name: &str) -> u16 {
    use palette::rgb565;
    match kind_for(name) {
        Kind::Road => rgb565(200, 30, 30),    // red
        Kind::Gravel => rgb565(240, 90, 20),  // orange
        Kind::Mtb => rgb565(20, 130, 40),     // green
        Kind::Touring => rgb565(30, 70, 180), // blue
        Kind::Generic => palette::INK,
    }
}

/// Blit `bike` centred on `center_x` with its top edge at `top_y`, each art pixel a `scale` by
/// `scale` block. Each run of ink cells is one fill, so a sprite costs a few fills per row.
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

    #[test]
    fn matches_shipped_names_case_insensitively() {
        // `const` slices have no stable address, so compare the sprites by content.
        assert_eq!(for_name("Road"), ROAD);
        assert_eq!(for_name("gravel"), GRAVEL);
        assert_eq!(for_name("MTB"), MTB);
        assert_eq!(for_name("Mountain"), MTB);
        assert_eq!(for_name("Touring"), TOURING);
        assert_eq!(for_name("Commuter"), GENERIC);
        assert_eq!(for_name(""), GENERIC);
    }

    /// A ragged ASCII row, such as one with a trimmed trailing space, misaligns the blit.
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

    #[test]
    fn each_type_has_a_distinct_colour() {
        let cols = [color_for("Road"), color_for("Gravel"), color_for("MTB"), color_for("Touring")];
        for (i, a) in cols.iter().enumerate() {
            assert_ne!(*a, palette::INK, "a shipped type is coloured, not ink");
            for b in &cols[i + 1..] {
                assert_ne!(a, b, "the four bike colours must be distinct");
            }
        }
        assert_eq!(color_for("Commuter"), palette::INK, "a custom profile stays ink");
    }
}
