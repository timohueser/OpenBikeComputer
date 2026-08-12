//! The spike, kept: a **real** Copernicus GLO-30 tile, decoded in pure Rust, baked, and read back
//! through the shared reader against surveyed elevations.
//!
//! `#[ignore]`d because it downloads ~40 MB from the AWS Open Data mirror. Run it deliberately:
//!
//! ```sh
//! cargo test -p obc-dem --test real_tile -- --ignored --nocapture
//! ```
//!
//! The tile is cached under `target/` (override with `OBC_DEM_TEST_SOURCES`), so a second run is
//! offline.
//!
//! ## What this proves that the synthetic fixtures cannot
//!
//! The synthetic GeoTIFF is uncompressed, single-strip and hand-written. A GLO-30 tile is none of
//! those: **float32 samples, Adobe DEFLATE, the floating-point horizontal predictor, 1024 × 1024
//! internal tiles, `PixelIsPoint`**. That combination — `Predictor = 3` in particular — is the one
//! the epic named as EL2's risk, and it is what the `tiff` crate had to handle for the fallback
//! (a bakery-side preconversion step) to stay unnecessary.
//!
//! ## Why these pins and not others
//!
//! Road passes: a surveyed summit sign is a number anyone can check, and a pass saddle is exactly
//! the terrain a 30 m DSM represents well. Deliberately **not** pinned:
//!
//! - **Sharp summits.** Finsteraarhorn (4274 m surveyed) reads 4086 m in this tile — the highest
//!   sample in the whole 1° square. A pyramidal snow-and-ice peak is not something a 30 m surface
//!   model resolves, and pinning one would be pinning the dataset's limit, not our arithmetic.
//! - **Tunnelled passes.** The Susten road crosses its summit in a tunnel; the DSM reads the
//!   mountain above it (2299 m against the road's 2224 m). Correctly.
//! - **Reservoir surfaces** as a *surveyed* number: a dam's crest elevation is not the water level
//!   on the day the radar flew. The lake check below asserts **flatness**, which is the property
//!   GLO-30's water flattening actually promises.

mod common;

use common::Scratch;
use obc_dem::bake::{bake_shard, BakeParams, V1_POSTING_LOG2};
use obc_dem::fetch::fetch_tiles;
use obc_dem::geotiff::{DemMosaic, DemTile};
use obc_dem::BboxUdeg;
use obc_elevation::{TerrainReader, TileCache, DEFAULT_TILE_SLOTS};
use obc_formats::io::SliceSource;

/// The exact box `fixtures/build-map-package.sh terrain` bakes the registered sidecar over, so this
/// test walks the same path that produced the file in the tree.
const GRIMSEL_BBOX: &str = "46.48261,8.15034,46.72070,8.46007";

/// Where the downloaded tiles are cached between runs.
fn sources_dir() -> std::path::PathBuf {
    match std::env::var_os("OBC_DEM_TEST_SOURCES") {
        Some(dir) => dir.into(),
        None => std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/obc-dem-sources"),
    }
}

#[test]
#[ignore = "downloads ~40 MB from the Copernicus AWS Open Data mirror"]
fn a_real_glo30_tile_decodes_bakes_and_reads_back_at_surveyed_elevations() {
    let bbox = BboxUdeg::parse(GRIMSEL_BBOX).unwrap();
    let dir = sources_dir();
    let paths =
        fetch_tiles(bbox, &dir, |tile, outcome| println!("  {} {outcome:?}", tile.stem())).expect("fetching GLO-30");
    assert_eq!(paths.len(), 1, "the Grimsel box lives inside one 1° tile");

    // --- the spike proper: pure-Rust decode of the real thing ---------------------------------
    // Built from the tiles the *box* needs, not from everything in the cache directory — the same
    // cache also holds the Teningen tile once `build-map-package.sh terrain` has run.
    let mut mosaic = DemMosaic::default();
    for path in &paths {
        mosaic.push(DemTile::open(path).expect("pure-Rust decode of a GLO-30 COG"));
    }
    assert_eq!(mosaic.len(), 1);

    // --- bake at the v1 posting, into one shard -------------------------------------------------
    let scratch = Scratch::new("real-tile");
    let shard = scratch.join("grimsel.obcd");
    let file = std::fs::File::create(&shard).unwrap();
    // A 2^16 cell keeps the fixture a few hundred KB while the *posting* — the thing that decides
    // the heights — stays the real v1 one.
    let params = BakeParams { posting_log2: V1_POSTING_LOG2, cell_log2: 16, bbox };
    let report = bake_shard(&mosaic, params, std::io::BufWriter::new(file), |_, _, _, _, _| {}).unwrap();
    println!("{report:?}");
    assert_eq!(report.samples_nodata, 0, "the Alps are not water and this tile has no voids");

    let bytes = std::fs::read(&shard).unwrap();
    let src = SliceSource(&bytes);
    let reader = TerrainReader::parse(&src).expect("the baked shard must parse as OBCT");
    let mut cache = TileCache::<DEFAULT_TILE_SLOTS>::new();
    let mut at = |lat: f64, lon: f64| {
        reader.sample(&mut cache, (lat * 1e6).round() as i32, (lon * 1e6).round() as i32).expect("covered")
    };

    // --- surveyed spot checks, through the shared reader ---------------------------------------
    // Tolerance is the source's, not ours: GLO-30's stated vertical accuracy is ~2–4 m RMSE and a
    // pass sign is a point on a road, not on the DSM's smoothed saddle.
    for (name, lat, lon, surveyed) in [
        ("Grimsel Pass", 46.5611, 8.3372, 2164i32),
        ("Furka Pass", 46.5722, 8.4153, 2429),
        ("Nufenen Pass", 46.4783, 8.3878, 2478),
    ] {
        let baked = i32::from(at(lat, lon));
        println!("  {name:<14} surveyed {surveyed:>5} m   baked {baked:>5} m   Δ {:>+4} m", baked - surveyed);
        assert!((baked - surveyed).abs() <= 10, "{name}: baked {baked} m vs surveyed {surveyed} m");
    }

    // --- water: flattened, and it stays flat through the resample -------------------------------
    // GLO-30 flattens water bodies, and this crate neither inpaints nor smooths, so a patch of
    // Grimselsee must come back as **one** number across several postings. The lake's own surveyed
    // full-supply level is 1909 m; the radar caught the reservoir drawn down to 1879.5 m, which is
    // precisely why the assertion here is flatness rather than a surveyed height.
    let (lake_lat, lake_lon) = (46.5695, 8.3211);
    let level = i32::from(at(lake_lat, lake_lon));
    assert_eq!(level, 1880, "the flattened surface quantises to one metre, half away from zero");
    for (dlat, dlon) in [(0.0004, 0.0), (-0.0004, 0.0), (0.0, 0.004), (0.0, -0.004)] {
        let nearby = i32::from(at(lake_lat + dlat, lake_lon + dlon));
        assert_eq!(nearby, level, "a flattened water surface must not develop relief");
    }
    println!("  Grimselsee     flattened surface at {level} m (surveyed full pool 1909 m — drawn down)");

    // --- the plausibility envelope --------------------------------------------------------------
    // The box spans the Grimsel and Furka roads and the Aare gorge below them.
    let valley = i32::from(at(46.6800, 8.2400));
    assert!((1000..=1600).contains(&valley), "the Haslital floor read {valley} m");
}
