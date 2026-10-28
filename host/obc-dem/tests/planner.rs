//! The planner terrain decodes only the GLO-30 squares of a latitude band, and answers there as it
//! does with every square decoded, also where the shapes read GLO-30 across a degree line. Inside
//! the archive the shapes change nothing; in its feather band they change the height. GLO-30 takes
//! the bias that the index states, and a height does not depend on the bounds of the bake.

mod common;

use std::collections::BTreeMap;

use common::{set_bias, write_archive, ArchiveTile, Scratch, SyntheticDem, PIXEL_IS_POINT};
use obc_dem::fetch::TileId;
use obc_dem::planner::{biases, squares, tiles_in, Recent, Terrain, MIN_SAMPLES};
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
    set_bias(&archive, -20.0);
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
    // GLO-30 takes the bias of the index, −20 m, so the feather band without shapes is at 500 m.
    assert!((plain[1] - 500.0).abs() < 0.01, "the feather band: {} m", plain[1]);
    assert!(shaped[1] > 0.0 && shaped[1] < 400.0, "the feather band with the sea: {} m", shaped[1]);
}

/// GLO-30 takes the bias that the index states for the best model of a tile, and the seam reads
/// the whole tiles that the bounds meet and the ring of tiles around them, so a height does not
/// depend on the bounds of the bake. The archive is a block of three by three tiles, at 500 m in
/// its west column and 480 m in the two others; GLO-30 stands at 520 m, and the index states
/// −20 m. The small bounds meet two tiles of the middle row. Its points: one in the feather band
/// of the west edge of the archive, one 100 m east of the line between two columns, and one 300 m
/// south of the north edge of the tiles that the small bounds meet, where only the ring shows that
/// the archive goes on.
#[test]
fn a_height_does_not_depend_on_the_bounds_of_the_bake() {
    let scratch = Scratch::new("planner-bounds");
    let tile = 1i64 << TILE_LOG2;
    let index = |degrees: f64| (((degrees * 1e6) as i64 - i64::from(GRID_ORIGIN)) / tile) as u32;
    let (ti, tj) = (index(46.5), index(8.5));
    let archive = scratch.join("archive");
    std::fs::create_dir_all(&archive).unwrap();
    let tiles: Vec<ArchiveTile> = (ti - 1..=ti + 1)
        .flat_map(|i| {
            (tj - 1..=tj + 1).map(move |j| ArchiveTile::filled(i, j, move |_, _| Some(if j < tj { 500 } else { 480 })))
        })
        .collect();
    write_archive(&archive, "test", &tiles);
    set_bias(&archive, -20.0);
    let corner = |i: u32| (i64::from(GRID_ORIGIN) + i64::from(i) * tile) as i32;
    let metres = |m: f64| (m / (0.11132 * 46.5f64.to_radians().cos())) as i32;
    let lat = corner(ti) + tile as i32 / 2;
    let north = corner(ti + 1) - (300.0 / 0.11132) as i32;
    let points = [
        (lat, corner(tj - 1) + metres(400.0)),
        (lat, corner(tj) + metres(100.0)),
        (north, corner(tj) + metres(2500.0)),
    ];
    let dem = SyntheticDem::build(
        f64::from(lat) * 1e-6 + 0.05,
        f64::from(corner(tj - 1)) * 1e-6 - 0.01,
        1.0 / 3600.0,
        360,
        450,
        PIXEL_IS_POINT,
        |_, _| 520.0,
    );
    dem.write(scratch.path(), &TileId { lat: 46, lon: 8 }.stem());
    let heights = |bounds: [f64; 4]| {
        let mut terrain = Terrain::open(&tiles_in(scratch.path(), bounds), "GLO-30", Some(&archive), bounds).unwrap();
        terrain.load(i32::MIN..=i32::MAX).unwrap();
        let mut recent = Recent::default();
        points.map(|(lat, lon)| terrain.height(lat, lon, &mut recent).unwrap().unwrap())
    };
    let degrees = |udeg: i32| f64::from(udeg) * 1e-6;
    let small = [degrees(points[0].1), degrees(lat), degrees(points[2].1), degrees(north)];
    let small = heights([small[0] - 0.001, small[1] - 0.001, small[2] + 0.001, small[3] + 0.001]);
    assert_eq!(small, heights([8.3, 46.4, 8.6, 46.6]), "the same heights in a small and a large bake");
    assert!((small[0] - 500.0).abs() < 0.01, "GLO-30 with the bias of the index: {} m", small[0]);
    assert_eq!(small[1..], [480.0, 480.0], "the archive alone, also next to the edge of the small bounds");
}

/// The bias of a model is the median of model − GLO-30 over its samples. A model with fewer than
/// `MIN_SAMPLES` with GLO-30 has none, and a sample in a square without GLO-30 is an error.
#[test]
fn the_bias_of_a_model_is_the_median_of_its_samples_less_glo30() {
    let scratch = Scratch::new("planner-biases");
    let dem = SyntheticDem::build(46.6, 8.0, 1.0 / 3600.0, 360, 360, PIXEL_IS_POINT, |_, _| 520.0);
    dem.write(scratch.path(), &TileId { lat: 46, lon: 8 }.stem());
    let glo30 = tiles_in(scratch.path(), [8.0, 46.0, 8.1, 46.6]);
    let at = |metres: f64, n: usize| vec![(46_550_000, 8_050_000, metres); n];
    let a = [at(500.0, 512), at(497.5, 300), at(510.0, 300)].concat();
    let samples = BTreeMap::from([("a".to_string(), a), ("b".to_string(), at(500.0, MIN_SAMPLES - 1))]);
    assert_eq!(biases(&samples, &glo30).unwrap(), BTreeMap::from([("a".to_string(), -20.0)]));
    let north = BTreeMap::from([("a".to_string(), vec![(47_550_000, 8_050_000, 500.0)])]);
    let error = biases(&north, &glo30).unwrap_err();
    assert!(error.contains("Copernicus_DSM_COG_10_N47_00_E008_00_DEM"), "{error}");
}
