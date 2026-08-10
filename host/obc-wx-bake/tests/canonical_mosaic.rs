//! WXR3 (#1242): the canonical lattice, the priority mosaic, and what the mosaic publishes.
//!
//! Three properties, in the order they matter:
//!
//! 1. **No smoothing, still provably** — every published cell equals the quantized
//!    nearest-neighbour source cell *of whichever source won the mosaic there*, checked against
//!    independently decoded upstream bytes. This is `tests/cycle.rs`'s
//!    `published_cells_equal_quantized_nearest_neighbour_source_cells` carried across the rewrite:
//!    the mosaic changed *which* source answers a cell, never *how*.
//! 2. **The priority table decides, and nothing else does** — two overlapping sources, deliberately
//!    ranked against their cell sizes, so a mosaic that quietly preferred the finer lattice fails.
//! 3. **A floor outage is code 15, never dry** — the one distinction the no-provenance decision
//!    (#1242) keeps.
//!
//! Every test drives the production code path. What they shrink is the lattice *extent*: a
//! sub-window of the canonical lattice, same cell pitch and a lattice-aligned origin, so its cells
//! are canonical cells and a debug build can still encode them. The full 36,000 x 18,000 lattice
//! and its 24 shards are asserted arithmetically in `canonical`'s own unit tests, and measured end
//! to end by the WXR1 spike.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;

use obc_formats::obcg;
use obc_formats::precip4::{self, INTENSITY_DRY, INTENSITY_NODATA};
use obc_wx_bake::canonical::{
    emit_shard, run_canonical_cycle, CycleTimes, Lattice, Mosaic, MosaicLayer, CANONICAL, CELL_UDEG, CYCLE_FRAMES,
    LATTICE_CELL_SIZE_M,
};
use obc_wx_bake::fetch::FixtureUpstream;
use obc_wx_bake::geometry::GridGeometry;
use obc_wx_bake::grib::{decode_bzip2_field, ExpectedGrib, ICON_EU_GRID_DEFINITION_HEX};
use obc_wx_bake::manifest;
use obc_wx_bake::publish::DirStore;
use obc_wx_bake::source::{dwd_rv, gfs, icon_eu, Adapter, AdapterOutcome, Attribution, BakedFrame, BakedProduct};
use obc_wx_bake::stereo;

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
    std::fs::read(&path).unwrap_or_else(|error| panic!("fixture {name}: {error}"))
}

fn ts(text: &str) -> i64 {
    manifest::parse_rfc3339(text).expect("test timestamp")
}

/// A small window of the canonical lattice, driven through the identical production code.
///
/// Its cells **are** canonical cells: the same 0.01 degree pitch, and an origin a whole number of
/// canonical cells from the canonical origin. Only the extent is small enough that a debug build
/// can deflate it in a second rather than a minute.
fn sub_lattice(south_lat_udeg: i32, west_lon_udeg: i32, width: u32, height: u32) -> Lattice {
    let cell = i64::from(CELL_UDEG);
    assert_eq!((i64::from(south_lat_udeg) - i64::from(CANONICAL.south_lat_udeg)) % cell, 0, "origin off the lattice");
    assert_eq!((i64::from(west_lon_udeg) - i64::from(CANONICAL.west_lon_udeg)) % cell, 0, "origin off the lattice");
    Lattice {
        south_lat_udeg,
        west_lon_udeg,
        cell_udeg: CANONICAL.cell_udeg,
        width,
        height,
        shard_width: width.div_ceil(2),
        shard_height: height.div_ceil(2),
        tile_edge: 64,
        entries_per_page: CANONICAL.entries_per_page,
        cell_size_m: CANONICAL.cell_size_m,
    }
}

/// Decode one cell out of a published shard the way a corridor client would.
fn published_cell(bytes: &[u8], col: u32, row: u32) -> u8 {
    let header_bytes: &[u8; obcg::HEADER_LEN] = bytes[..obcg::HEADER_LEN].try_into().unwrap();
    let header = obcg::decode_header(header_bytes).unwrap();
    let (tile_col, tile_row) = header.tile_of_cell(col, row).unwrap();
    let tile_index = header.tile_index(tile_col, tile_row).unwrap();
    let page = header.page_of_entry(tile_index);
    let page_offset = header.page_offset(page).unwrap() as usize;
    let page_slice = &bytes[page_offset..page_offset + header.page_bytes() as usize];
    obcg::validate_page(&header, page_slice).unwrap();
    let within = (tile_index - page * u32::from(header.entries_per_page)) as usize;
    let entry = obcg::decode_entry(page_slice, within).unwrap();
    let payload = if entry.is_dry() {
        &[][..]
    } else {
        &bytes[entry.data_offset as usize..entry.data_offset as usize + usize::from(entry.encoded_len)]
    };
    let mut cells = vec![0u8; header.tile_cells()];
    obcg::decode_tile_cells(&header, &entry, payload, &mut cells).unwrap();
    cells[header.cell_index_in_tile(col, row).unwrap()]
}

// ---------------------------------------------------------------------------------------------
// 1. Every published cell equals the NN of whichever source won
// ---------------------------------------------------------------------------------------------

const DWD_RUN: &str = "2026-08-09T14:20:00Z";
const ICON_RUN: &str = "2026-08-09T06:00:00Z";
const RV_ETAG: &str = "\"6a788c2a-273800\"";
const DWD_GAIN: f64 = 0.000_999_999_931_780_621_3;

fn european_upstream() -> FixtureUpstream {
    let mut upstream = FixtureUpstream::default();
    upstream.insert(dwd_rv::LATEST_URL, fixture("composite_rv_20260809_1420.tar"), Some(RV_ETAG));
    for lead in 0..=12u32 {
        upstream.insert(
            icon_eu::lead_url(ts(ICON_RUN), lead),
            fixture(&format!("icon-eu-2026080906_{lead:03}.grib2.bz2")),
            None,
        );
    }
    upstream
}

fn bake(adapter: &dyn Adapter, upstream: &mut FixtureUpstream, now: i64) -> BakedProduct {
    let mut warnings = Vec::new();
    match adapter.bake(upstream, None, now, &mut warnings).expect("fixture bake") {
        AdapterOutcome::Baked(product) => *product,
        AdapterOutcome::Unchanged => panic!("{}: fixture bake reported Unchanged", adapter.id()),
    }
}

/// The raw DWD RV lead-0 member, straight out of the fixture tar — the same independent oracle
/// `tests/cycle.rs` uses, unchanged by the rewrite.
fn dwd_native_raster() -> Vec<u32> {
    let tar_bytes = fixture("composite_rv_20260809_1420.tar");
    let mut archive = tar::Archive::new(tar_bytes.as_slice());
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        if entry.path().unwrap().file_name().unwrap().to_str() == Some("composite_rv_20260809_1420_000-hd5") {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            let file = hdf5_pure::File::from_bytes(bytes).unwrap();
            return file.dataset("dataset1/data1/data").unwrap().read_u32().unwrap();
        }
    }
    panic!("lead-0 member in the fixture tar");
}

fn dwd_expected(lat: f64, lon: f64, raw: &[u32]) -> u8 {
    match stereo::native_index(lat, lon) {
        None => INTENSITY_NODATA,
        Some(index) => {
            let encoded = u64::from(raw[index]);
            if encoded == 4_294_967_295 {
                INTENSITY_NODATA
            } else if encoded == 0 {
                INTENSITY_DRY
            } else {
                let mm_5min = encoded as f64 * DWD_GAIN - DWD_GAIN;
                precip4::quantize_rate_mm_per_hour(mm_5min * 12.0)
            }
        }
    }
}

fn icon_expected_field() -> ExpectedGrib {
    ExpectedGrib {
        discipline: 0,
        category: 1,
        parameter: 52,
        grid_template: 0,
        expected_points: 904_689,
        expected_grid_definition_hex: ICON_EU_GRID_DEFINITION_HEX,
        product_template: 8,
        representation_templates: &[42],
        missing_sentinels: &[],
        allowed_messages: &[1],
        require_identical_messages: false,
    }
}

/// Nearest-neighbour index into a source window from a lattice cell centre, or `None` outside it.
fn source_index(window: &GridGeometry, lattice: &Lattice, col: u32, row: u32) -> Option<usize> {
    let lat = lattice.centre_lat_udeg(row);
    let lon = lattice.centre_lon_udeg(col);
    let column = (lon - i64::from(window.west_lon_udeg)).div_euclid(i64::from(window.cell_lon_udeg));
    let source_row = (lat - i64::from(window.south_lat_udeg)).div_euclid(i64::from(window.cell_lat_udeg));
    if !(0..i64::from(window.width)).contains(&column) || !(0..i64::from(window.height)).contains(&source_row) {
        return None;
    }
    Some(source_row as usize * window.width as usize + column as usize)
}

/// **The test the rewrite had to carry across.** Every published cell equals the quantized
/// nearest-neighbour of whichever source won the mosaic at that cell — checked against upstream
/// bytes this test decodes itself, and against an oracle that applies the priority rule (radar
/// first, model where the radar has no data, no-data where neither reaches) rather than asking
/// the mosaic what it did.
#[test]
fn every_published_cell_equals_the_quantized_nearest_neighbour_of_the_winning_source() {
    let mut upstream = european_upstream();
    // Anchor the cycle on the radar run so frame selection is exact rather than nearest: the
    // canonical f0 is valid at 14:20Z, which is the DWD observation itself and 20 minutes after
    // ICON's 14:00Z hourly frame.
    let now = ts(DWD_RUN);
    let dwd = bake(&dwd_rv::DwdRv, &mut upstream, now);
    let icon = bake(&icon_eu::IconEu, &mut upstream, now);
    let dwd_window = dwd.geometry;
    let icon_window = icon.geometry;
    let mosaic = Mosaic::from_products(vec![dwd, icon]).expect("both sources are in MOSAIC_PRIORITY");
    // Pinned rather than `anchored_at`, which would round 14:20Z down to the 14:15Z quarter-hour
    // and make f0 a *nearest* radar frame instead of the observation itself. The anchoring rule is
    // unit-tested in `canonical`; what this test needs is an exactly known frame pick.
    let times = CycleTimes { reference_time: now };

    // The independent oracles: the raw stereographic radar raster, and ICON's 13:00Z..14:00Z
    // hourly accumulation difference — the frame valid at 14:00Z, which is the one nearest the
    // canonical f0.
    let raw = dwd_native_raster();
    let expected_field = icon_expected_field();
    let f007 = decode_bzip2_field(&fixture("icon-eu-2026080906_007.grib2.bz2"), &expected_field).unwrap();
    let f008 = decode_bzip2_field(&fixture("icon-eu-2026080906_008.grib2.bz2"), &expected_field).unwrap();

    // A window over south-west Germany and its French/Swiss surroundings: partly inside the radar
    // trapezoid, partly outside it, so both branches of the priority rule are exercised.
    let lattice = sub_lattice(45_680_000, 1_460_000, 512, 384);
    let (mut radar_cells, mut model_cells, mut wet_cells, mut checked) = (0usize, 0usize, 0usize, 0usize);
    for shard in 0..lattice.shard_count() {
        let window = lattice.shard(shard).expect("shard on this lattice");
        let object = emit_shard(&lattice, &mosaic, times, 0, shard).expect("the shard emits and self-validates");
        for cell in (0..window.cells()).step_by(11) {
            let local_col = (cell as u32) % window.width;
            let local_row = (cell as u32) / window.width;
            let col = window.col0 + local_col;
            let row = window.row0 + local_row;
            let lat = lattice.centre_lat_udeg(row) as f64 / 1e6;
            let lon = lattice.centre_lon_udeg(col) as f64 / 1e6;

            // The oracle, in priority order: the radar answers unless it has no data there.
            let radar = source_index(&dwd_window, &lattice, col, row).map(|_| dwd_expected(lat, lon, &raw));
            let model = source_index(&icon_window, &lattice, col, row).map(|index| {
                precip4::quantize_rate_mm_per_hour(f64::from(f008.values[index]) - f64::from(f007.values[index]))
            });
            let (expected, winner) = match (radar, model) {
                (Some(value), _) if value != INTENSITY_NODATA => (value, Some(dwd_rv::ID)),
                (_, Some(value)) if value != INTENSITY_NODATA => (value, Some(icon_eu::ID)),
                _ => (INTENSITY_NODATA, None),
            };

            assert_eq!(published_cell(&object.bytes, local_col, local_row), expected, "cell ({col},{row})");
            assert_eq!(mosaic.winner_at(&lattice, times.valid_at(0), col, row), winner, "winner at ({col},{row})");
            match winner {
                Some(id) if id == dwd_rv::ID => radar_cells += 1,
                Some(_) => model_cells += 1,
                None => {}
            }
            if expected != INTENSITY_DRY && expected != INTENSITY_NODATA {
                wet_cells += 1;
            }
            checked += 1;
        }
    }
    eprintln!(
        "mosaic NN agreement: {checked} sampled cells, {radar_cells} radar, {model_cells} model, {wet_cells} wet"
    );
    assert!(checked > 10_000, "the sample must be big enough to mean something");
    assert!(radar_cells > 1_000, "the window must contain cells the radar answers");
    assert!(model_cells > 1_000, "the window must contain cells only the model answers");
    assert!(wet_cells > 0, "the captured run must contain rain for the agreement to mean anything");
}

// ---------------------------------------------------------------------------------------------
// 2. The priority table decides, and nothing else does
// ---------------------------------------------------------------------------------------------

/// A synthetic source on one window, one frame, filled from `value`.
fn synthetic(id: &'static str, window: GridGeometry, valid_at: i64, value: impl Fn(u32, u32) -> u8) -> BakedProduct {
    let cells = (0..window.height)
        .flat_map(|row| (0..window.width).map(move |col| (col, row)))
        .map(|(col, row)| value(col, row))
        .collect::<Vec<u8>>();
    BakedProduct {
        id,
        product_code: obcg::PRODUCT_MOSAIC,
        tier: obcg::TIER_RADAR,
        geometry: window,
        reference_time: valid_at,
        staleness_deadline: valid_at + 3_600,
        attribution: Attribution { text: "synthetic", url: "https://example.invalid" },
        upstream_etag: None,
        frames: vec![BakedFrame { offset_min: 0, valid_at, flags: obcg::FLAG_OBSERVED, source: None, cells }],
    }
}

fn window(south: i32, west: i32, cell: u32, width: u32, height: u32) -> GridGeometry {
    GridGeometry {
        south_lat_udeg: south,
        west_lon_udeg: west,
        cell_lat_udeg: cell,
        cell_lon_udeg: cell,
        width,
        height,
        cell_size_m: 1_000,
        tile_edge: 32,
        entries_per_page: 512,
    }
}

/// Two overlapping sources, ranked **against** their cell sizes: `icon-eu` (rank 2) is given a
/// deliberately coarse 0.04 degree window and `gfs` (rank 3) a fine 0.01 degree one. If the mosaic
/// preferred the finer lattice — the painter's-order shortcut the WXR1 spike used, and the easiest
/// thing to regress to — the floor would win the overlap and every assertion below would fail.
#[test]
fn the_priority_table_decides_the_overlap_not_the_cell_size() {
    let valid_at = ts("2026-08-09T14:00:00Z");
    let lattice = sub_lattice(45_680_000, 1_460_000, 128, 128);
    // Coarse but higher priority; every cell carries 3.
    let coarse = synthetic(icon_eu::ID, window(45_680_000, 1_460_000, 40_000, 16, 16), valid_at, |_, _| 3);
    // Fine but lower priority; carries 7 in the western half, no-data in the eastern half.
    let fine = synthetic(gfs::ID, window(45_680_000, 1_460_000, 10_000, 128, 128), valid_at, |col, _| {
        if col < 64 {
            7
        } else {
            INTENSITY_NODATA
        }
    });
    let mosaic = Mosaic::from_products(vec![fine, coarse]).expect("both ids are in MOSAIC_PRIORITY");
    let times = CycleTimes { reference_time: valid_at };
    let object = emit_shard(&lattice, &mosaic, times, 0, 0).expect("the shard emits");
    let shard = lattice.shard(0).expect("shard 0");
    assert_eq!((shard.width, shard.height), (64, 64));

    for (col, row) in [(0u32, 0u32), (10, 10), (63, 63)] {
        assert_eq!(published_cell(&object.bytes, col, row), 3, "higher priority wins ({col},{row})");
        assert_eq!(mosaic.winner_at(&lattice, valid_at, col, row), Some(icon_eu::ID));
    }

    // The other half of "first source *with data* wins": hole the higher-priority source and the
    // lower-priority one has to show through, at exactly the coarse cell boundary.
    let holed = synthetic(icon_eu::ID, window(45_680_000, 1_460_000, 40_000, 16, 16), valid_at, |col, _| {
        if col < 8 {
            INTENSITY_NODATA
        } else {
            3
        }
    });
    let fine = synthetic(gfs::ID, window(45_680_000, 1_460_000, 10_000, 128, 128), valid_at, |_, _| 7);
    let mosaic = Mosaic::from_products(vec![fine, holed]).expect("both ids are in MOSAIC_PRIORITY");
    let object = emit_shard(&lattice, &mosaic, times, 0, 0).expect("the shard emits");
    // Coarse column 0..8 is lattice columns 0..32: holed, so the floor shows through.
    assert_eq!(published_cell(&object.bytes, 5, 5), 7);
    assert_eq!(mosaic.winner_at(&lattice, valid_at, 5, 5), Some(gfs::ID));
    // Coarse column 8 starts at lattice column 32: the higher-priority source answers again.
    assert_eq!(published_cell(&object.bytes, 40, 5), 3);
    assert_eq!(mosaic.winner_at(&lattice, valid_at, 40, 5), Some(icon_eu::ID));
}

/// Cell replication is the nearest-neighbour rule `OBCG_Spec.md` §6 mandates, applied once: one
/// coarse source cell paints a whole block of identical lattice cells, and the block boundaries
/// land exactly where the coarse lattice says.
#[test]
fn a_coarse_source_cell_replicates_into_an_exact_block() {
    let valid_at = ts("2026-08-09T14:00:00Z");
    let lattice = sub_lattice(45_680_000, 1_460_000, 64, 64);
    // 0.04 degree cells = a 4 x 4 block of lattice cells each; value = the coarse column index.
    let coarse =
        synthetic(gfs::ID, window(45_680_000, 1_460_000, 40_000, 16, 16), valid_at, |col, _| (col % 8) as u8 + 1);
    let mosaic = Mosaic::from_products(vec![coarse]).expect("gfs is in MOSAIC_PRIORITY");
    let object = emit_shard(&lattice, &mosaic, CycleTimes { reference_time: valid_at }, 0, 0).expect("emits");
    for col in 0..32u32 {
        for row in 0..8u32 {
            let expected = (col / 4) % 8 + 1;
            assert_eq!(u32::from(published_cell(&object.bytes, col, row)), expected, "({col},{row})");
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 3. A floor outage is code 15, never dry
// ---------------------------------------------------------------------------------------------

/// The honesty rule, and the whole reason there is no coverage channel: with the global floor
/// present every cell carries a best-available value, so "no radar here" renders as model fill.
/// Take the floor away and the uncovered cells become intensity **15**, not 0 — "we do not know",
/// never "dry".
#[test]
fn a_floor_outage_publishes_code_fifteen_and_never_dry() {
    let valid_at = ts("2026-08-09T14:00:00Z");
    let lattice = sub_lattice(45_680_000, 1_460_000, 64, 64);
    let radar_window = window(45_680_000, 1_460_000, 10_000, 16, 16);
    let radar = || synthetic(dwd_rv::ID, radar_window, valid_at, |_, _| 4);
    let floor = || synthetic(gfs::ID, window(45_680_000, 1_460_000, 250_000, 4, 4), valid_at, |_, _| INTENSITY_DRY);
    let times = CycleTimes { reference_time: valid_at };

    // Healthy: the floor answers everywhere the radar does not, and it answers "dry".
    let healthy = Mosaic::from_products(vec![radar(), floor()]).expect("both ids are ranked");
    let object = emit_shard(&lattice, &healthy, times, 0, 0).expect("emits");
    assert_eq!(published_cell(&object.bytes, 4, 4), 4, "the radar still wins its own footprint");
    assert_eq!(published_cell(&object.bytes, 30, 30), INTENSITY_DRY, "the floor answers, and it says dry");
    assert_eq!(healthy.winner_at(&lattice, valid_at, 30, 30), Some(gfs::ID));

    // Outage: the floor source produced nothing this cycle. The cells it used to answer must read
    // as no-data, and must not silently read as dry.
    let degraded = Mosaic::from_products(vec![radar()]).expect("the radar is ranked");
    let object = emit_shard(&lattice, &degraded, times, 0, 0).expect("emits");
    assert_eq!(published_cell(&object.bytes, 4, 4), 4, "the radar is unaffected by the floor's outage");
    assert_eq!(published_cell(&object.bytes, 30, 30), INTENSITY_NODATA, "missing must never read as dry");
    assert_eq!(degraded.winner_at(&lattice, valid_at, 30, 30), None);

    // A source frame too far from the canonical frame's validity is the same kind of absence: it
    // is not sampled, and the cells fall through rather than carrying a stale value.
    let stale = Mosaic::from_products(vec![radar(), {
        let mut product = floor();
        product.frames[0].valid_at = valid_at + 4 * 3_600;
        product
    }])
    .expect("both ids are ranked");
    let object = emit_shard(&lattice, &stale, times, 0, 0).expect("emits");
    assert_eq!(published_cell(&object.bytes, 30, 30), INTENSITY_NODATA, "a four-hour-old floor is not sampled");
}

// ---------------------------------------------------------------------------------------------
// The published geometry states the lattice, not a source
// ---------------------------------------------------------------------------------------------

/// `cell_size_m` is pinned to the lattice (#1242): a frame whose German cells are 1 km radar and
/// whose Italian cells are 6.5 km model has no single source resolution to state, so it states the
/// lattice instead. The field is still there, at its fixed header offset, and the device still
/// reads it.
#[test]
fn every_published_frame_states_the_lattice_cell_size_and_pitch() {
    let valid_at = ts("2026-08-09T14:00:00Z");
    let lattice = sub_lattice(45_680_000, 1_460_000, 64, 64);
    let coarse = synthetic(gfs::ID, window(45_680_000, 1_460_000, 250_000, 4, 4), valid_at, |_, _| 2);
    let mosaic = Mosaic::from_products(vec![coarse]).expect("gfs is ranked");
    let object = emit_shard(&lattice, &mosaic, CycleTimes { reference_time: valid_at }, 0, 0).expect("emits");
    let mut scratch = vec![0u8; usize::from(lattice.tile_edge) * usize::from(lattice.tile_edge)];
    let header = obcg::validate(&object.bytes, &mut scratch).expect("valid object");
    assert_eq!(header.cell_size_m, LATTICE_CELL_SIZE_M, "the frame states the lattice, not the 27.75 km source");
    assert_eq!((header.cell_lat_udeg, header.cell_lon_udeg), (CELL_UDEG, CELL_UDEG));
    assert_eq!(header.product_id, obcg::PRODUCT_MOSAIC);
}

// ---------------------------------------------------------------------------------------------
// The whole cycle, end to end
// ---------------------------------------------------------------------------------------------

/// One canonical cycle against the fixture upstreams and a directory store: every shard of every
/// frame is published, every object self-validates, and the run is deterministic. The lattice is a
/// sub-window (the full 648 M-cell one takes 12 s in a release build and minutes in a debug one),
/// but the orchestration, the streaming shape, the fetchability proof and the placeholder manifest
/// are the production ones.
#[test]
fn a_canonical_cycle_publishes_every_shard_of_every_frame_and_repeats_itself() {
    let lattice = sub_lattice(45_680_000, 1_460_000, 256, 192);
    let now = ts("2026-08-09T14:30:00Z");
    let dwd = dwd_rv::DwdRv;
    let icon = icon_eu::IconEu;
    let adapters: [&dyn Adapter; 2] = [&dwd, &icon];

    let mut trees = Vec::new();
    for run in 0..2 {
        let dir = std::env::temp_dir().join(format!("obc-wx-canonical-{}-{run}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut store = DirStore::new(&dir);
        let mut upstream = european_upstream();
        let report = run_canonical_cycle(&lattice, &adapters, &mut upstream, &mut store, now, false)
            .expect("the canonical cycle publishes");
        let objects = usize::try_from(lattice.shard_count()).unwrap() * usize::try_from(CYCLE_FRAMES).unwrap();
        assert_eq!(report.published_objects, objects + 1, "every shard of every frame, plus the manifest");
        // The report names its sources in priority order, and only sources that have a row.
        assert_eq!(
            report.layers.iter().map(|(id, _, _)| id.as_str()).collect::<Vec<_>>(),
            vec![dwd_rv::ID, icon_eu::ID]
        );

        let mut tree = BTreeMap::new();
        let mut stack = vec![dir.clone()];
        while let Some(path) = stack.pop() {
            for entry in std::fs::read_dir(&path).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    let key = path.strip_prefix(&dir).unwrap().to_string_lossy().replace('\\', "/");
                    tree.insert(key, std::fs::read(&path).unwrap());
                }
            }
        }
        assert_eq!(tree.len(), objects + 1);
        // Every published object is a valid OBCG frame on the canonical lattice — read back from
        // disk, through the same fail-closed validator the phone uses.
        let mut scratch = vec![0u8; usize::from(lattice.tile_edge) * usize::from(lattice.tile_edge)];
        for (key, bytes) in &tree {
            if key.ends_with(".json") {
                let json: serde_json::Value = serde_json::from_slice(bytes).expect("the manifest is JSON");
                assert_eq!(json["version"], 2);
                assert_eq!(json["lattice"]["cell_size_m"], u64::from(LATTICE_CELL_SIZE_M));
                assert_eq!(json["objects"].as_array().expect("objects").len(), objects);
                continue;
            }
            let header = obcg::validate(bytes, &mut scratch).unwrap_or_else(|e| panic!("{key}: {e:?}"));
            assert_eq!(header.cell_size_m, LATTICE_CELL_SIZE_M);
            assert_eq!(header.product_id, obcg::PRODUCT_MOSAIC);
        }
        let _ = std::fs::remove_dir_all(&dir);
        trees.push(tree);
    }
    assert_eq!(trees[0], trees[1], "same upstream bytes, byte-identical published tree");
}

/// A layer whose source has no row in the priority table cannot be mosaicked — that is a bakery
/// configuration bug and it fails the cycle closed rather than silently dropping a source.
#[test]
fn an_unranked_source_refuses_to_join_the_mosaic() {
    let valid_at = ts("2026-08-09T14:00:00Z");
    let orphan = synthetic("not-a-source", window(45_680_000, 1_460_000, 10_000, 4, 4), valid_at, |_, _| 1);
    let error = MosaicLayer::from_product(orphan).expect_err("an unranked source must be refused");
    assert!(error.contains("MOSAIC_PRIORITY"), "{error}");
}
