//! Pixel-art bike sprites for the [ride-start card](crate::screen::RideStartScreen), one per
//! [`BikeType`]. Each sprite is a grid of ASCII rows, where a non-space cell is one ink pixel.

use obc_render::{rect, Surface};

use crate::screen::palette;
use crate::settings::BikeType;

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

pub const fn sprite(bike: BikeType) -> Bike {
    match bike {
        BikeType::Road => ROAD,
        BikeType::Gravel => GRAVEL,
        BikeType::Mtb => MTB,
        BikeType::Touring => TOURING,
    }
}

/// The ink colour for a bike. The colours land on clean device-64 colours over the parchment
/// background.
pub fn color(bike: BikeType) -> u16 {
    use palette::rgb565;
    match bike {
        BikeType::Road => rgb565(200, 30, 30),    // red
        BikeType::Gravel => rgb565(240, 90, 20),  // orange
        BikeType::Mtb => rgb565(20, 130, 40),     // green
        BikeType::Touring => rgb565(30, 70, 180), // blue
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

    /// A ragged ASCII row, such as one with a trimmed trailing space, misaligns the blit.
    #[test]
    fn sprites_are_uniform_rectangles() {
        let (rows, cols) = (ROAD.len(), ROAD[0].len());
        for bike in BikeType::ALL.map(sprite) {
            assert_eq!(bike.len(), rows, "all sprites have the same row count");
            for row in bike {
                assert_eq!(row.len(), cols, "every row is the same width");
            }
        }
    }

    #[test]
    fn each_type_has_a_distinct_colour() {
        let cols = BikeType::ALL.map(color);
        for (i, a) in cols.iter().enumerate() {
            for b in &cols[i + 1..] {
                assert_ne!(a, b, "the four bike colours must be distinct");
            }
        }
    }
}
