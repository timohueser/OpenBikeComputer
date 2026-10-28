//! The planner terrain decodes only the GLO-30 squares of a latitude band, and answers there as it
//! does with every square decoded, also where the shapes read GLO-30 across a degree line. Inside
//! the archive the shapes change nothing; in its feather band they change the height.

mod common;

use common::{write_archive, ArchiveTile, Scratch, SyntheticDem, PIXEL_IS_POINT};
use obc_dem::fetch::TileId;
use obc_dem::planner::{squares, tiles_in, Recent, Terrain};
use obc_dem::reference::TILE_LOG2;
use obc_dem::shape::{Line, Peak};
use obc_formats::obct::GRID_ORIGIN;

/// Near a degree line a height reads the squares on both sides of it: the last posts of one square
/// and the first of the next, and the square that owns the half post past them. A dome just north
/// of 47° N has a summit whose flanks read GLO-30 across the line, and a coastline runs east just
/// south of it.
#[test]
fn heights_across_a_degree_line_from_the_squares_of_their_band_equal_those_from_every_square() {
    let scratch = Scratch::new("planner-band");
    let step = 1.0 / 3600.0;
    let dome = |lat: f64, lon: f64| {
        let r = ((lat - 47.0006) * 111_320.0).hypot((lon - 8.003) * 75_900.0);
        (1000.0 + 40.0 * (1.0 - (r / 150.0).powi(2)).max(0.0)) as f32
    };
    // The northmost posts of N46E008, up to 47° N, and the southmost of N47E008, at the GLO-30 posting.
    for (lat, tie, rows) in [(46, 47.0, 37), (47, 47.0 + 36.0 * step, 36)] {
        let dem = SyntheticDem::build(tie, 8.0, step, rows, 24, PIXEL_IS_POINT, dome);
        dem.write(scratch.path(), &TileId { lat, lon: 8 }.stem());
    }
    let bounds = [8.0, 46.5, 8.001, 47.5];
    let files = tiles_in(scratch.path(), bounds);
    assert_eq!(files.len(), 2);
    let open = || {
        let features = (vec![vec![[46.9985, 7.99], [46.9985, 8.02]]], vec![Peak { at: [47.0006, 8.003], ele: 2000.0 }]);
        Terrain::open(&files, "GLO-30", None, bounds).unwrap().shaped(features, "OSM")
    };
    let mut every = open();
    every.load(i32::MIN..=i32::MAX).unwrap();
    let mut band = open();
    let (mut recent, mut band_recent, mut known, mut sea) = (Recent::default(), Recent::default(), 0, 0);
    // North to south, as the sweep of the terrain tiles goes.
    for lat in (46_990_000..=47_010_000).rev().step_by(37) {
        band.load(squares(lat)).unwrap();
        for lon in [8_001_000, 8_002_000, 8_003_000, 8_004_000] {
            let height = every.height(lat, lon, &mut recent).unwrap();
            assert_eq!(band.height(lat, lon, &mut band_recent).unwrap(), height, "at {lat}, {lon} µdeg");
            known += usize::from(height.is_some());
            sea += usize::from(height == Some(0.0));
        }
    }
    assert!(known > 1000, "the squares give heights on both sides of the line");
    assert!(sea > 10, "the sea beside the coastline is at sea level");
    let top = every.height(47_000_600, 8_003_000, &mut recent).unwrap().unwrap();
    assert!(top > f64::from(dome(47.0006, 8.003)) + 1.0, "the summit rises: {top} m");
}

/// The archive holds the ground at 500 m in a block of three by three tiles, and GLO-30 stands at
/// 520 m. Coastlines run through the middle of the block and 300 m inside its west edge. With
/// them, the sea is at sea level without the archive, the middle of the block keeps the archive's
/// height, and the feather band changes.
#[test]
fn the_shapes_change_the_feather_band_and_not_the_ground_that_the_archive_holds_alone() {
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
    let corner = |i: u32| (i64::from(GRID_ORIGIN) + i64::from(i) * tile) as i32;
    let (lat, middle) = (corner(ti) + tile as i32 / 2, corner(tj) + tile as i32 / 2);
    let feather = corner(tj - 1) + (300.0 / (0.11132 * 46.5f64.to_radians().cos())) as i32;
    let step = 1.0 / 3600.0;
    let west = f64::from(corner(tj - 1)) * 1e-6 - 0.01;
    let dem = SyntheticDem::build(f64::from(lat) * 1e-6 + 0.02, west, step, 144, 450, PIXEL_IS_POINT, |_, _| 520.0);
    dem.write(scratch.path(), &TileId { lat: 46, lon: 8 }.stem());
    let bounds = [8.3, 46.4, 8.6, 46.6];
    let files = tiles_in(scratch.path(), bounds);
    let coast = |lon: i32| vec![[46.4, f64::from(lon) * 1e-6], [46.6, f64::from(lon) * 1e-6]];
    let heights = |archive: Option<&std::path::Path>, lines: Vec<Line>| {
        let mut terrain = Terrain::open(&files, "GLO-30", archive, bounds).unwrap().shaped((lines, Vec::new()), "OSM");
        terrain.load(i32::MIN..=i32::MAX).unwrap();
        let mut recent = Recent::default();
        [middle + 100, feather + 100].map(|lon| terrain.height(lat, lon, &mut recent).unwrap().unwrap())
    };
    let coasts = || vec![coast(middle), coast(feather)];
    assert_eq!(heights(None, coasts()), [0.0, 0.0], "without the archive, the sea");
    let (plain, shaped) = (heights(Some(&archive), Vec::new()), heights(Some(&archive), coasts()));
    assert_eq!((plain[0], shaped[0]), (500.0, 500.0), "the archive alone");
    // GLO-30 takes the bias of the archive, −20 m, so the feather band without shapes is at 500 m.
    assert!((plain[1] - 500.0).abs() < 0.01, "the feather band: {} m", plain[1]);
    assert!(shaped[1] > 0.0 && shaped[1] < 400.0, "the feather band with the sea: {} m", shaped[1]);
}
