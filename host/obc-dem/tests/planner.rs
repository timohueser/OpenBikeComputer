//! The planner terrain decodes only the GLO-30 squares of a latitude band, and answers there as it
//! does with every square decoded.

mod common;

use common::{Scratch, SyntheticDem, PIXEL_IS_POINT};
use obc_dem::fetch::TileId;
use obc_dem::planner::{squares, tiles_in, Recent, Terrain};

/// Near a degree line a height reads the squares on both sides of it: the last posts of one square
/// and the first of the next, and the square that owns the half post past them.
#[test]
fn heights_across_a_degree_line_from_the_squares_of_their_band_equal_those_from_every_square() {
    let scratch = Scratch::new("planner-band");
    let step = 1.0 / 3600.0;
    // The northmost posts of N46E008, up to 47° N, and the southmost of N47E008, at the GLO-30 posting.
    for (lat, tie, rows) in [(46, 47.0, 37), (47, 47.0 + 36.0 * step, 36)] {
        let dem = SyntheticDem::build(tie, 8.0, step, rows, 8, PIXEL_IS_POINT, |lat, lon| {
            (1000.0 + 3e5 * (lat - 47.0) + 1e5 * (lon - 8.0)) as f32
        });
        dem.write(scratch.path(), &TileId { lat, lon: 8 }.stem());
    }
    let bounds = [8.0, 46.5, 8.001, 47.5];
    let files = tiles_in(scratch.path(), bounds);
    assert_eq!(files.len(), 2);
    let mut every = Terrain::open(&files, "GLO-30", None, bounds).unwrap();
    every.load(i32::MIN..=i32::MAX).unwrap();
    let mut band = Terrain::open(&files, "GLO-30", None, bounds).unwrap();
    let (mut recent, mut band_recent, mut known) = (Recent::default(), Recent::default(), 0);
    // North to south, as the sweep of the terrain tiles goes.
    for lat in (46_990_000..=47_010_000).rev().step_by(37) {
        band.load(squares(lat)).unwrap();
        for lon in [8_000_100, 8_001_000, 8_001_900] {
            let height = every.height(lat, lon, &mut recent).unwrap();
            assert_eq!(band.height(lat, lon, &mut band_recent).unwrap(), height, "at {lat}, {lon} µdeg");
            known += usize::from(height.is_some());
        }
    }
    assert!(known > 1000, "the squares give heights on both sides of the line");
}
