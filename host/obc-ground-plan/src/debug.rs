//! The palette of the debug pictures: the debug tool and the viewer's debug draw read these colours
//! through `transport::vocabulary`, so both show one plan in the same colours.

use crate::faces::{EdgeKind, Surface};
use crate::graph::{CrossingKind, Family};

pub type Rgb = (u8, u8, u8);
/// A colour with its alpha.
pub type Rgba = (u8, u8, u8, u8);

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
pub const CROSSINGS: [(CrossingKind, char); 9] = [
    (CrossingKind::Deck, 'D'),
    (CrossingKind::Culvert, 'C'),
    (CrossingKind::Ford, 'F'),
    (CrossingKind::PipeInStones, 'P'),
    (CrossingKind::SteppingStones, 'S'),
    (CrossingKind::Footbridge, 'B'),
    (CrossingKind::LevelCrossing, 'L'),
    (CrossingKind::Stacked, 'X'),
    (CrossingKind::Bore, 'T'),
];

pub fn family_colour(family: Family) -> Rgb {
    FAMILIES.iter().find(|f| f.0 == family).map_or((0, 0, 0), |f| f.2)
}

/// `#rrggbbaa`.
pub fn hex_alpha((r, g, b, a): Rgba) -> String {
    format!("{}{a:02x}", hex((r, g, b)))
}

/// `#rrggbb`.
pub fn hex((r, g, b): Rgb) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// The surfaces in code order, with their fills; a terrain face gets none.
pub const SURFACES: [(Surface, &str, Option<Rgba>); 9] = [
    (Surface::Terrain, "terrain", None),
    (Surface::Slope, "slope", Some((205, 190, 120, 110))),
    (Surface::Carriageway, "carriageway", Some((105, 105, 110, 255))),
    (Surface::Shoulder, "shoulder", Some((190, 182, 160, 255))),
    (Surface::Verge, "verge", Some((140, 185, 110, 255))),
    (Surface::CornerFill, "corner_fill", Some((230, 160, 50, 255))),
    (Surface::Crossing, "crossing", Some((210, 60, 60, 255))),
    (Surface::RailBed, "rail_bed", Some((125, 95, 75, 255))),
    (Surface::Platform, "platform", Some((200, 200, 205, 255))),
];
/// The fill of a junction's face where it differs from its surface's.
pub const JUNCTION_SURFACES: [(Surface, Rgba); 2] =
    [(Surface::Carriageway, (150, 90, 190, 255)), (Surface::RailBed, (165, 110, 80, 255))];

/// The edge kinds in code order, with their colours.
pub const EDGES: [(EdgeKind, &str, Rgb); 7] = [
    (EdgeKind::Continuous, "continuous", (20, 20, 20)),
    (EdgeKind::Kerb, "kerb", (255, 255, 255)),
    (EdgeKind::Wall, "wall", (220, 30, 30)),
    (EdgeKind::Headwall, "headwall", (255, 140, 0)),
    (EdgeKind::Portal, "portal", (160, 0, 200)),
    (EdgeKind::DeckEdge, "deck_edge", (0, 170, 220)),
    (EdgeKind::CellBorder, "cell_border", (0, 0, 0)),
];

/// The fill of a face, or None.
pub fn face_colour(surface: Surface, junction: bool) -> Option<Rgba> {
    let own = JUNCTION_SURFACES.iter().find(|j| junction && j.0 == surface).map(|j| j.1);
    own.or(SURFACES[surface as usize].2)
}

pub fn edge_colour(kind: EdgeKind) -> Rgb {
    EDGES[kind as usize].2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tables_are_in_code_order() {
        assert!(SURFACES.iter().enumerate().all(|(i, s)| s.0 as usize == i));
        assert!(EDGES.iter().enumerate().all(|(i, e)| e.0 as usize == i));
    }
}
