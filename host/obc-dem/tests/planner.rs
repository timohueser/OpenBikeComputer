//! The planner terrain decodes only the GLO-30 squares of a latitude band, and answers there as it
//! does with every square decoded, also where the shapes read GLO-30 around a point. Where the
//! archive holds the ground alone, the shapes change nothing.

mod common;

use common::{write_archive, ArchiveTile, Scratch, SyntheticDem, PIXEL_IS_POINT};
use obc_dem::fetch::TileId;
use obc_dem::planner::{squares, tiles_in, Recent, Terrain};
use obc_dem::reference::TILE_LOG2;
use obc_dem::shape::{Features, Line, Peak};
use obc_formats::obct::GRID_ORIGIN;

/// A coastline north along `lon` with the land to its west, a sea cliff 20 m inland of it, and a
/// summit, from 46.9° N to 47.1° N.
fn features(lon: f64) -> Features {
    let line = |lon: f64, coast| Line { points: vec![[46.9, lon], [47.1, lon]], coast, height: None };
    (vec![line(lon, true), line(lon - 0.00026, false)], vec![Peak { at: [47.0, lon - 0.001], ele: 2000.0 }])
}

/// Near a degree line a height reads the squares on both sides of it: the last posts of one square
/// and the first of the next, and the square that owns the half post past them.
#[test]
fn heights_across_a_degree_line_from_the_squares_of_their_band_equal_those_from_every_square() {
    let scratch = Scratch::new("planner-band");
    let step = 1.0 / 3600.0;
    // The northmost posts of N46E008, up to 47° N, and the southmost of N47E008, at the GLO-30 posting.
    for (lat, tie, rows) in [(46, 47.0, 37), (47, 47.0 + 36.0 * step, 36)] {
        let dem = SyntheticDem::build(tie, 8.0, step, rows, 24, PIXEL_IS_POINT, |lat, lon| {
            (1000.0 + 3e5 * (lat - 47.0) + 1e5 * (lon - 8.0)) as f32
        });
        dem.write(scratch.path(), &TileId { lat, lon: 8 }.stem());
    }
    let bounds = [8.0, 46.5, 8.001, 47.5];
    let files = tiles_in(scratch.path(), bounds);
    assert_eq!(files.len(), 2);
    let open = || Terrain::open(&files, "GLO-30", None, bounds).unwrap().shaped(features(8.003), "OSM");
    let mut every = open();
    every.load(i32::MIN..=i32::MAX).unwrap();
    let mut band = open();
    let (mut recent, mut band_recent, mut known, mut sea) = (Recent::default(), Recent::default(), 0, 0);
    // North to south, as the sweep of the terrain tiles goes.
    for lat in (46_990_000..=47_010_000).rev().step_by(37) {
        band.load(squares(lat)).unwrap();
        for lon in [8_000_100, 8_001_000, 8_001_900, 8_002_600, 8_003_300] {
            let height = every.height(lat, lon, &mut recent).unwrap();
            assert_eq!(band.height(lat, lon, &mut band_recent).unwrap(), height, "at {lat}, {lon} µdeg");
            known += usize::from(height.is_some());
            sea += usize::from(height == Some(0.0));
        }
    }
    assert!(known > 1000, "the squares give heights on both sides of the line");
    assert!(sea > 100, "the sea beside the coastline is at sea level");
}

/// The archive holds the ground at 500 m in a block of three by three tiles, GLO-30 stands at
/// 520 m, and a coastline runs through the middle tile. Inside the archive the height is the
/// archive's, with the shapes as without them.
#[test]
fn the_shapes_leave_the_ground_that_the_archive_holds_alone() {
    let scratch = Scratch::new("planner-archive");
    let tile = 1i64 << TILE_LOG2;
    let index = |degrees: f64| (((degrees * 1e6) as i64 - i64::from(GRID_ORIGIN)) / tile) as u32;
    let (ti, tj) = (index(46.5), index(8.5));
    let archive = scratch.join("archive");
    std::fs::create_dir_all(&archive).unwrap();
    let tiles: Vec<ArchiveTile> = (ti - 1..=ti + 1)
        .flat_map(|i| (tj - 1..=tj + 1).map(move |j| ArchiveTile::filled(i, j, |_, _| Some(500))))
        .collect();
    write_archive(&archive, "test", &tiles);
    let centre = |i: u32| (i64::from(GRID_ORIGIN) + i64::from(i) * tile + tile / 2) as i32;
    let (lat, lon) = (centre(ti), centre(tj));
    let step = 1.0 / 3600.0;
    let dem = SyntheticDem::build(
        f64::from(lat) * 1e-6 + 0.02,
        f64::from(lon) * 1e-6 - 0.02,
        step,
        144,
        144,
        PIXEL_IS_POINT,
        |_, _| 520.0,
    );
    dem.write(scratch.path(), &TileId { lat: 46, lon: 8 }.stem());
    let bounds = [8.4, 46.4, 8.6, 46.6];
    let files = tiles_in(scratch.path(), bounds);
    let coast = || Line {
        points: vec![[46.4, f64::from(lon) * 1e-6], [46.6, f64::from(lon) * 1e-6]],
        coast: true,
        height: None,
    };
    let heights = |archive: Option<&std::path::Path>, lines: Vec<Line>| {
        let mut terrain = Terrain::open(&files, "GLO-30", archive, bounds).unwrap().shaped((lines, Vec::new()), "OSM");
        terrain.load(i32::MIN..=i32::MAX).unwrap();
        let mut recent = Recent::default();
        [lon - 100, lon + 100].map(|lon| terrain.height(lat, lon, &mut recent).unwrap())
    };
    assert_eq!(heights(None, vec![coast()]), [Some(520.0), Some(0.0)], "without the archive, the sea");
    assert_eq!(heights(Some(&archive), Vec::new()), [Some(500.0); 2]);
    assert_eq!(heights(Some(&archive), vec![coast()]), [Some(500.0); 2], "the shapes change the ground of the archive");
}
