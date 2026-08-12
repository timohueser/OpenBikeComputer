//! **The spike (#1070), kept as a test.** Can pure Rust decode a real Copernicus GLO-30 tile?
//!
//! The epic named this as EL2's headline risk, with a documented fallback — a bakery-side
//! preconversion step — if the answer had been no, because the one thing that was never on the
//! table is a native dependency in the packer tree (#907). The answer is **yes**: the `tiff` crate
//! reads the mirror's tiles as they are shipped, and this test is what says so on every run rather
//! than only in a PR description.
//!
//! ```sh
//! cargo test -p obc-dem --test decode -- --ignored --nocapture
//! ```
//!
//! `#[ignore]`d: it downloads ~40 MB from the AWS Open Data mirror, cached under `target/`
//! (override with `OBC_DEM_TEST_SOURCES`). What it pins is the *source's* shape — the layer below
//! any OBCT byte; `tests/real_tile.rs` carries the same tile through a bake and back.

mod common;

use obc_dem::fetch::{fetch_tiles, TileId};
use obc_dem::geotiff::{DemMosaic, DemTile};
use obc_dem::BboxUdeg;

/// The tile the spike ran against: 46–47 °N, 8–9 °E — the Bernese/Urner Alps, and the square the
/// simulator's Grimsel map sits in.
const GRIMSEL_BBOX: &str = "46.48261,8.15034,46.72070,8.46007";

fn sources_dir() -> std::path::PathBuf {
    match std::env::var_os("OBC_DEM_TEST_SOURCES") {
        Some(dir) => dir.into(),
        None => std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/obc-dem-sources"),
    }
}

#[test]
#[ignore = "downloads ~40 MB from the Copernicus AWS Open Data mirror"]
fn the_tiff_crate_decodes_a_real_glo30_tile() {
    let bbox = BboxUdeg::parse(GRIMSEL_BBOX).unwrap();
    let dir = sources_dir();
    let paths =
        fetch_tiles(bbox, &dir, |tile, outcome| println!("  {} {outcome:?}", tile.stem())).expect("fetching GLO-30");
    assert_eq!(paths, vec![dir.join(TileId { lat: 46, lon: 8 }.file_name())]);

    // The decode itself — float32 samples, Adobe DEFLATE, the floating-point horizontal predictor
    // (`Predictor = 3`), 1024 × 1024 internal tiles. No GDAL, no C, no shelling out.
    let tile = DemTile::open(&paths[0]).expect("pure-Rust decode of a GLO-30 COG");

    // --- geometry ------------------------------------------------------------------------------
    let (rows, cols) = tile.shape();
    assert_eq!((rows, cols), (3600, 3600), "a 1° tile at 1 arcsecond, in this latitude band");
    let (step_lat, step_lon) = tile.step_deg();
    assert!((step_lat - 1.0 / 3600.0).abs() < 1e-15 && (step_lon - 1.0 / 3600.0).abs() < 1e-15);

    // `PixelIsPoint`, pinned by where post (0, 0) lands. The file's tie point is (8.0, 47.0); with
    // `PixelIsPoint` that is the *centre* of the north-west post, so the south-west post — post
    // (0, 0) after the ingest flip — is exactly 3599 steps below it. Under `PixelIsArea` it would
    // sit half a step inside on both axes instead, and every sample in the bake would be ~15 m out.
    let (south, west) = tile.south_west_deg();
    assert!((west - 8.0).abs() < 1e-12, "west edge {west}");
    assert!((south - (47.0 - 3599.0 / 3600.0)).abs() < 1e-9, "south edge {south}");
    println!("  {rows}×{cols} posts, south-west post at ({south:.6}, {west:.6})");

    // Column count varies with latitude band on this mirror (3600 up to 50 °N, 1800 above it), so
    // nothing downstream may assume one global post lattice — hence the per-tile geotransform.

    // --- values --------------------------------------------------------------------------------
    // One tile, not `open_dir`: the cache directory also holds the Teningen tile once
    // `build-map-package.sh terrain` has run, and this test is about *this* square.
    let mut mosaic = DemMosaic::default();
    mosaic.push(DemTile::open(&paths[0]).expect("decode"));
    for (name, lat, lon, surveyed, tolerance) in [
        ("Grimsel Pass", 46.5611, 8.3372, 2164.0, 10.0),
        ("Furka Pass", 46.5722, 8.4153, 2429.0, 10.0),
        ("Nufenen Pass", 46.4783, 8.3878, 2478.0, 10.0),
    ] {
        let height = mosaic.height(lat, lon).expect("covered");
        println!("  {name:<14} surveyed {surveyed:>7.0} m   source {height:>8.2} m");
        assert!((height - surveyed).abs() <= tolerance, "{name}: {height} m against a surveyed {surveyed} m");
    }

    // **Orientation and water flattening, in one number.** Lago Maggiore sits in the far south-east
    // of this square and is regulated at ≈ 193 m; GLO-30 flattens it, so the tile reads *exactly*
    // 193 over the whole lake. A raster ingested without the §2 row flip would answer the height at
    // 46.87 °N instead — the Schächental, well over a kilometre up — so this single equality pins
    // the flip, the geotransform and the flattening at once.
    let maggiore = mosaic.height(46.13, 8.72).expect("covered");
    println!("  Lago Maggiore  surveyed     193 m   source {maggiore:>8.2} m (flattened)");
    assert_eq!(maggiore, 193.0, "a flattened lake surface must arrive exactly, not nearly");

    // --- voids ---------------------------------------------------------------------------------
    // This square is entirely land and lake, so the tile has none. Water being *flattened* rather
    // than voided is exactly why the bake has no inpainting to do: a lake arrives as a constant.
    assert!(mosaic.height(46.5695, 8.3211).is_some(), "Grimselsee is flattened water, not a void");
}
