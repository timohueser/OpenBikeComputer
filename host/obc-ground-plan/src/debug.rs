//! The palette of the debug pictures: the debug tool and the viewer's debug draw read these colours
//! through `transport::vocabulary`, so both show one plan in the same colours.

use crate::graph::{CrossingKind, Family};

pub type Rgb = (u8, u8, u8);

/// The families in code order, with their colours.
pub const FAMILIES: [(Family, &str, Rgb); 4] = [
    (Family::Road, "road", (150, 100, 20)),
    (Family::Path, "path", (190, 40, 40)),
    (Family::Rail, "rail", (30, 30, 30)),
    (Family::Water, "water", (30, 90, 220)),
];
pub const LINK: Rgb = (230, 0, 0);
pub const SYNTHETIC_DECK: Rgb = (250, 140, 0);
pub const JUNCTION: Rgb = (20, 120, 40);
pub const GUEST_ARM: Rgb = (140, 40, 160);
pub const JOINT: Rgb = (0, 0, 0);
pub const END: Rgb = (0, 0, 0);
pub const CROSSING: Rgb = (255, 255, 255);

/// The crossing kinds in code order, with their letters.
pub const CROSSINGS: [(CrossingKind, char); 8] = [
    (CrossingKind::Deck, 'D'),
    (CrossingKind::Culvert, 'C'),
    (CrossingKind::Ford, 'F'),
    (CrossingKind::PipeInStones, 'P'),
    (CrossingKind::SteppingStones, 'S'),
    (CrossingKind::Footbridge, 'B'),
    (CrossingKind::LevelCrossing, 'L'),
    (CrossingKind::Stacked, 'X'),
];

pub fn family_colour(family: Family) -> Rgb {
    FAMILIES.iter().find(|f| f.0 == family).map_or((0, 0, 0), |f| f.2)
}

/// `#rrggbb`.
pub fn hex((r, g, b): Rgb) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}
