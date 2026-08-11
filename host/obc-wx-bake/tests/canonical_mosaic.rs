//! WXR3 (#1242): the canonical lattice, the priority mosaic, and what the mosaic publishes.
//!
//! Three properties, in the order they matter:
//!
//! 1. **No smoothing, still provably** — every published cell equals the quantized
//!    nearest-neighbour source cell *of whichever source won the mosaic there*, checked against
//!    independently decoded upstream bytes. This is `tests/cycle.rs`'s
//!    `published_cells_equal_quantized_nearest_neighbour_source_cells` carried across the rewrite
//!    (that file is gone with the multi-product path, #1246): the mosaic changed *which* source
//!    answers a cell, never *how*.
//! 2. **The priority table decides, and nothing else does** — two overlapping sources, deliberately
//!    ranked against their cell sizes, so a mosaic that quietly preferred the finer lattice fails.
//! 3. **A floor outage is code 15, never dry** — the one distinction the no-provenance decision
//!    (#1242) keeps.
//!
//! Every test drives the production code path. What they shrink is the lattice *extent*: a
//! sub-window of the canonical lattice, same cell pitch and a lattice-aligned origin, so its cells
//! are canonical cells and a debug build can still encode them. The full 36,000 x 18,000 lattice
//! and its 24 shards are asserted arithmetically in `canonical`'s own unit tests, and were measured
//! end to end by the WXR1 spike (#1240, whose numbers are recorded in #1254 and whose harness
//! #1246 deleted).

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::PathBuf;

use obc_formats::obcg;
use obc_formats::precip4::{self, INTENSITY_DRY, INTENSITY_NODATA};
use obc_wx_bake::canonical::{
    bake_cycle, emit_shard, run_cycle, source_column, source_reaches, CycleTimes, Lattice, Mosaic, MosaicLayer,
    BAKE_THREADS, CANONICAL, CELL_UDEG, CYCLE_FRAMES, LATTICE_CELL_SIZE_M,
};
use obc_wx_bake::fetch::FixtureUpstream;
use obc_wx_bake::geometry::GridGeometry;
use obc_wx_bake::grib::{decode_bzip2_field, ExpectedGrib, ICON_EU_GRID_DEFINITION_HEX};
use obc_wx_bake::publish::DirStore;
use obc_wx_bake::source::{dwd_rv, gfs, hrrr, icon_eu, mrms, Adapter, Attribution, BakedFrame, BakedSource};
use obc_wx_bake::stereo;
use obc_wx_bake::{manifest_v2, timefmt};

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
    std::fs::read(&path).unwrap_or_else(|error| panic!("fixture {name}: {error}"))
}

fn ts(text: &str) -> i64 {
    timefmt::parse_rfc3339(text).expect("test timestamp")
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

fn bake(adapter: &dyn Adapter, upstream: &mut FixtureUpstream, now: i64) -> BakedSource {
    let mut warnings = Vec::new();
    adapter.bake(upstream, now, &mut warnings).expect("fixture bake")
}

/// The raw DWD RV lead-0 member, straight out of the fixture tar — the independent oracle the
/// no-smoothing proof rests on, decoded here rather than through any bakery code.
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
///
/// **How independent this oracle actually is**, so a later reader does not over-trust the sample
/// count. This function is a deliberate re-derivation of the window→index arithmetic that
/// `Mosaic::fill` also does, so the *selection* half of the oracle is a second opinion rather than
/// an independent one — it would agree with a shared sign error. What is genuinely independent is
/// the *value* half: the DWD branch reads the raw ODIM raster out of the tar and re-implements the
/// gain/offset/quantize chain, and it reaches the raster through `stereo::native_index` from the
/// source cell's own centre, which pins the DWD window's alignment against the projection rather
/// than against the mosaic. The ICON branch differences two independently decoded GRIB fields.
/// The wrap and edge rules that `fill` alone owns are pinned separately, by
/// `the_floor_covers_every_column_once_the_antimeridian_wraps` and
/// `a_lattice_centre_on_a_source_outer_edge_is_outside_the_window`.
fn source_index(window: &GridGeometry, lattice: &Lattice, col: u32, row: u32) -> Option<(usize, u32, u32)> {
    let lat = lattice.centre_lat_udeg(row);
    let lon = lattice.centre_lon_udeg(col);
    let column = (lon - i64::from(window.west_lon_udeg)).div_euclid(i64::from(window.cell_lon_udeg));
    let source_row = (lat - i64::from(window.south_lat_udeg)).div_euclid(i64::from(window.cell_lat_udeg));
    if !(0..i64::from(window.width)).contains(&column) || !(0..i64::from(window.height)).contains(&source_row) {
        return None;
    }
    Some((source_row as usize * window.width as usize + column as usize, column as u32, source_row as u32))
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
    let mosaic = Mosaic::from_sources(vec![dwd, icon]).expect("both sources are in MOSAIC_PRIORITY");
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

            // The oracle, in priority order: the radar answers unless it has no data there. The
            // DWD window is *not* on the canonical lattice (it is the shipped 9,000 x 14,000 µdeg
            // one, frozen until the cutover — see `dwd_rv::GEOMETRY`), so the published cell is the
            // stereographic sample taken at the **source cell's** centre, not at the lattice
            // cell's. Getting that wrong is exactly the double-hop this test exists to describe.
            let radar = source_index(&dwd_window, &lattice, col, row).map(|(_, column, source_row)| {
                dwd_expected(dwd_window.center_lat_deg(source_row), dwd_window.center_lon_deg(column), &raw)
            });
            let model = source_index(&icon_window, &lattice, col, row).map(|(index, _, _)| {
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

/// A synthetic source on one window, one **forecast** frame, filled from `value`.
fn synthetic(id: &'static str, window: GridGeometry, valid_at: i64, value: impl Fn(u32, u32) -> u8) -> BakedSource {
    synthetic_frames(id, window, &[(valid_at, obcg::FLAG_FORECAST)], value)
}

/// A synthetic source with an explicit frame list: `(valid_at, flags)` each carrying the same
/// field, which is enough for every frame-selection and provenance question.
fn synthetic_frames(
    id: &'static str,
    window: GridGeometry,
    frames: &[(i64, u16)],
    value: impl Fn(u32, u32) -> u8,
) -> BakedSource {
    let cells = (0..window.height)
        .flat_map(|row| (0..window.width).map(move |col| (col, row)))
        .map(|(col, row)| value(col, row))
        .collect::<Vec<u8>>();
    let reference_time = frames.first().map_or(0, |(valid_at, _)| *valid_at);
    BakedSource {
        id,
        geometry: window,
        reference_time,
        attribution: Attribution { text: "synthetic", url: "https://example.invalid" },
        frames: frames
            .iter()
            .map(|(valid_at, flags)| BakedFrame {
                offset_min: ((valid_at - reference_time) / 60) as u32,
                valid_at: *valid_at,
                flags: *flags,
                cells: cells.clone(),
            })
            .collect(),
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
    let mosaic = Mosaic::from_sources(vec![fine, coarse]).expect("both ids are in MOSAIC_PRIORITY");
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
    let mosaic = Mosaic::from_sources(vec![fine, holed]).expect("both ids are in MOSAIC_PRIORITY");
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
    let mosaic = Mosaic::from_sources(vec![coarse]).expect("gfs is in MOSAIC_PRIORITY");
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
    let healthy = Mosaic::from_sources(vec![radar(), floor()]).expect("both ids are ranked");
    let object = emit_shard(&lattice, &healthy, times, 0, 0).expect("emits");
    assert_eq!(published_cell(&object.bytes, 4, 4), 4, "the radar still wins its own footprint");
    assert_eq!(published_cell(&object.bytes, 30, 30), INTENSITY_DRY, "the floor answers, and it says dry");
    assert_eq!(healthy.winner_at(&lattice, valid_at, 30, 30), Some(gfs::ID));

    // Outage: the floor source produced nothing this cycle. The cells it used to answer must read
    // as no-data, and must not silently read as dry.
    let degraded = Mosaic::from_sources(vec![radar()]).expect("the radar is ranked");
    let object = emit_shard(&lattice, &degraded, times, 0, 0).expect("emits");
    assert_eq!(published_cell(&object.bytes, 4, 4), 4, "the radar is unaffected by the floor's outage");
    assert_eq!(published_cell(&object.bytes, 30, 30), INTENSITY_NODATA, "missing must never read as dry");
    assert_eq!(degraded.winner_at(&lattice, valid_at, 30, 30), None);

    // A source frame too far from the canonical frame's validity is the same kind of absence: it
    // is not sampled, and the cells fall through rather than carrying a stale value.
    let stale = Mosaic::from_sources(vec![radar(), {
        let mut product = floor();
        product.frames[0].valid_at = valid_at + 4 * 3_600;
        product
    }])
    .expect("both ids are ranked");
    let object = emit_shard(&lattice, &stale, times, 0, 0).expect("emits");
    assert_eq!(published_cell(&object.bytes, 30, 30), INTENSITY_NODATA, "a four-hour-old floor is not sampled");
}

// ---------------------------------------------------------------------------------------------
// The covered domain — what "every cell always carries a best-available value" actually means
// ---------------------------------------------------------------------------------------------

/// **The claim the whole no-provenance decision rests on, finally checked.** The floor source's
/// window drops the antimeridian column, so before the wrap there was a permanent no-data stripe
/// through Fiji in every frame of every cycle. This walks all 36,000 canonical columns.
#[test]
fn the_floor_covers_every_column_once_the_antimeridian_wraps() {
    let floor = gfs::GEOMETRY;
    let interior_row = CANONICAL.covered_rows().start;
    let uncovered: Vec<u32> =
        (0..CANONICAL.width).filter(|&col| !source_reaches(&floor, &CANONICAL, col, interior_row)).collect();
    assert!(uncovered.is_empty(), "the floor leaves {} columns unpainted: {uncovered:?}", uncovered.len());

    // …and the wrap is nearest-neighbour on the circle, not a modulo that shifts by a cell. The
    // westmost lattice column (centre 179.995 W) is nearer to the floor's first grid point
    // (179.75 W) than to its last (179.75 E); the eastmost is the mirror image.
    assert_eq!(source_column(&floor, CANONICAL.centre_lon_udeg(0)), Some(0));
    assert_eq!(source_column(&floor, CANONICAL.centre_lon_udeg(CANONICAL.width - 1)), Some(floor.width - 1));
    // A regional source is not periodic and must not wrap: nothing off its east edge comes back
    // around onto its west edge.
    assert_eq!(source_column(&icon_eu::GEOMETRY, CANONICAL.centre_lon_udeg(0)), None);
}

/// The polar band is a genuine hole and is named as one. No source we ingest reaches beyond
/// ±89.875°, so those rows publish intensity 15 forever — honest, but not something to discover
/// from a rendered frame.
#[test]
fn the_covered_domain_is_exactly_what_the_floor_reaches() {
    let rows = CANONICAL.covered_rows();
    assert_eq!(rows, 12..17_987, "the covered domain moved; the module docs and spec state this range");
    let sources = [gfs::GEOMETRY, icon_eu::GEOMETRY, dwd_rv::GEOMETRY, mrms::GEOMETRY, hrrr::GEOMETRY];
    // Inside: the floor reaches every one of them, so no cell is unsourced.
    for row in [rows.start, rows.start + 1, CANONICAL.height / 2, rows.end - 2, rows.end - 1] {
        assert!(source_reaches(&gfs::GEOMETRY, &CANONICAL, 18_000, row), "row {row} is inside the covered domain");
    }
    // Outside: *nothing* reaches, not just the floor — 25 rows of 18,000, both poles.
    let outside: Vec<u32> = (0..CANONICAL.height).filter(|row| !rows.contains(row)).collect();
    assert_eq!(outside.len(), 25);
    for row in outside {
        for source in &sources {
            for col in [0u32, 12_345, CANONICAL.width - 1] {
                assert!(!source_reaches(source, &CANONICAL, col, row), "row {row} col {col} is claimed by a source");
            }
        }
    }
}

/// The half-open-window rule, pinned because it is the one place nearest-neighbour quietly drops a
/// cell and because WXR6's windows have to be checked against it. A lattice centre landing on an
/// *interior* source boundary takes the eastern/northern cell; one landing on the window's *outer*
/// edge is outside the window and gets no data, rather than being snapped back inside a source
/// that does not cover it.
#[test]
fn a_lattice_centre_on_a_source_outer_edge_is_outside_the_window() {
    let lattice = sub_lattice(45_680_000, 1_460_000, 8, 8);
    // 0.02° cells whose edges land exactly on lattice cell *centres*: west edge at 1.455°, so cell
    // boundaries are 1.455, 1.475, … and lattice centres are 1.465, 1.475, ….
    let source = window(45_675_000, 1_455_000, 20_000, 4, 4);
    let columns: Vec<Option<u32>> = (0..8).map(|col| source_column(&source, lattice.centre_lon_udeg(col))).collect();
    assert_eq!(
        columns,
        vec![Some(0), Some(1), Some(1), Some(2), Some(2), Some(3), Some(3), None],
        "interior boundaries take the eastern cell; the outer edge is outside"
    );
}

// ---------------------------------------------------------------------------------------------
// Frame selection and provenance
// ---------------------------------------------------------------------------------------------

/// **An observation must not paint a forward frame that a real forecast is offering.** The `us`
/// layer is exactly this shape — a 1 km MRMS observation at f0, 3 km HRRR forecasts ahead of it —
/// and a nearest-only tie-break lets the frozen radar field win a forward frame it has nothing to
/// say about. Re-using an observation as a forecast is a persistence nowcast; that is WXR9 #1251's
/// job to do deliberately, not the frame picker's to stumble into.
#[test]
fn a_forecast_beats_an_equally_close_observation_on_a_forward_frame() {
    let t0 = ts("2026-08-09T14:00:00Z");
    let lattice = sub_lattice(45_680_000, 1_460_000, 64, 64);
    let source = window(45_680_000, 1_460_000, 10_000, 64, 64);
    // One layer, two frames the same distance from the target: an observation 15 min behind and a
    // forecast 15 min ahead. The forecast is about the future; the observation is frozen weather.
    let composed = synthetic_frames(
        dwd_rv::ID,
        source,
        &[(t0 - 900, obcg::FLAG_OBSERVED), (t0 + 900, obcg::FLAG_FORECAST)],
        |_, _| 5,
    );
    let mosaic = Mosaic::from_sources(vec![composed]).expect("ranked");
    let object = emit_shard(&lattice, &mosaic, CycleTimes { reference_time: t0 }, 0, 0).expect("emits");
    let mut scratch = vec![0u8; usize::from(lattice.tile_edge) * usize::from(lattice.tile_edge)];
    let header = obcg::validate(&object.bytes, &mut scratch).expect("valid");
    assert_eq!(header.flags, obcg::FLAG_FORECAST, "the forecast frame won, so the object is not Observed");

    // The observation still wins when it is genuinely the nearest — including exactly on target,
    // where it is the observation *of* that instant.
    let exact =
        synthetic_frames(dwd_rv::ID, source, &[(t0, obcg::FLAG_OBSERVED), (t0 + 900, obcg::FLAG_FORECAST)], |_, _| 5);
    let mosaic = Mosaic::from_sources(vec![exact]).expect("ranked");
    let object = emit_shard(&lattice, &mosaic, CycleTimes { reference_time: t0 }, 0, 0).expect("emits");
    let header = obcg::validate(&object.bytes, &mut scratch).expect("valid");
    assert_eq!(header.flags, obcg::FLAG_OBSERVED, "an observation valid at the target instant is the answer");
}

/// `FLAG_OBSERVED` is the last provenance channel the device sees, so it is measured rather than
/// assumed. A shard painted entirely by radar observation says Observed; the same frame's shard
/// over model fill does not — which is the ~85 % of the planet an unconditional "f0 is Observed"
/// would have lied about.
#[test]
fn the_observed_flag_follows_what_actually_painted_the_shard() {
    let t0 = ts("2026-08-09T14:00:00Z");
    // Two shards side by side: the radar covers only the western one.
    let lattice = sub_lattice(45_680_000, 1_460_000, 128, 64);
    let radar = synthetic_frames(
        dwd_rv::ID,
        window(45_680_000, 1_460_000, 10_000, 64, 64),
        &[(t0, obcg::FLAG_OBSERVED)],
        |_, _| 6,
    );
    let floor = synthetic_frames(
        gfs::ID,
        window(45_680_000, 1_460_000, 250_000, 8, 8),
        &[(t0, obcg::FLAG_FORECAST)],
        |_, _| INTENSITY_DRY,
    );
    let mosaic = Mosaic::from_sources(vec![radar, floor]).expect("ranked");
    let times = CycleTimes { reference_time: t0 };
    let mut scratch = vec![0u8; usize::from(lattice.tile_edge) * usize::from(lattice.tile_edge)];

    let west = emit_shard(&lattice, &mosaic, times, 0, 0).expect("emits");
    let east = emit_shard(&lattice, &mosaic, times, 0, 1).expect("emits");
    assert_eq!(obcg::validate(&west.bytes, &mut scratch).expect("valid").flags, obcg::FLAG_OBSERVED);
    assert_eq!(obcg::validate(&east.bytes, &mut scratch).expect("valid").flags, obcg::FLAG_FORECAST);
    assert!(west.observed && !east.observed);
    assert!(west.fill.all_observed && east.fill.painted && !east.fill.all_observed);

    // **And the second half of the rule.** The radar layer holds one frame, valid at the anchor, so
    // `nearest()` hands it to f+15 and f+30 as well — inside `MAX_FRAME_SKEW_S`, painting the same
    // cells from the same field. Those frames are about instants no observation of exists at bake
    // time, so they are forecasts by persistence and must say so, however observed their cells'
    // provenance is. Without this the western shard published three objects with three different
    // `valid_at`s, one field between them, and Observed on all three.
    for offset_min in [15, 30] {
        let ahead = emit_shard(&lattice, &mosaic, times, offset_min, 0).expect("emits");
        assert!(
            ahead.fill.all_observed,
            "f+{offset_min}: the frozen radar frame is still what painted it — that is the premise"
        );
        assert!(!ahead.observed, "f+{offset_min}: a frame ahead of the anchor is never observed");
        assert_eq!(
            obcg::validate(&ahead.bytes, &mut scratch).expect("valid").flags,
            obcg::FLAG_FORECAST,
            "f+{offset_min}"
        );
    }
    // Past the skew window the frozen frame is refused outright and the shard is no-data, which is
    // the other honest answer and also a forecast.
    let far = emit_shard(&lattice, &mosaic, times, 45, 0).expect("emits");
    assert!(!far.fill.painted && !far.observed);
}

/// **The failure the epic actually forbids**: a source that has fallen out of the timeline must
/// hand its cells to the next-priority source, not to no-data — and certainly not to dry. The
/// floor-outage test proves stale → 15 with nothing beneath; this proves stale → the next layer.
#[test]
fn a_stale_source_falls_through_to_the_next_priority_layer() {
    let t0 = ts("2026-08-09T14:00:00Z");
    let lattice = sub_lattice(45_680_000, 1_460_000, 64, 64);
    let germany = window(45_680_000, 1_460_000, 10_000, 64, 64);
    // The radar run is four hours behind the anchor: far outside MAX_FRAME_SKEW_S.
    let stale_radar = synthetic_frames(dwd_rv::ID, germany, &[(t0 - 4 * 3_600, obcg::FLAG_OBSERVED)], |_, _| 9);
    let model = synthetic_frames(icon_eu::ID, germany, &[(t0, obcg::FLAG_FORECAST)], |_, _| 3);
    let floor =
        synthetic_frames(gfs::ID, window(45_680_000, 1_460_000, 250_000, 8, 8), &[(t0, obcg::FLAG_FORECAST)], |_, _| 1);
    let mosaic = Mosaic::from_sources(vec![stale_radar, model, floor]).expect("all three are ranked");
    let object = emit_shard(&lattice, &mosaic, CycleTimes { reference_time: t0 }, 0, 0).expect("emits");
    for (col, row) in [(0u32, 0u32), (15, 15), (31, 31)] {
        let value = published_cell(&object.bytes, col, row);
        assert_ne!(value, 9, "the stale radar must not paint ({col},{row})");
        assert_ne!(value, INTENSITY_NODATA, "a stale source must not become a hole when a fresh one covers");
        assert_ne!(value, INTENSITY_DRY, "and it must never become dry");
        assert_eq!(value, 3, "the model answers Germany");
    }
    assert_eq!(mosaic.winner_at(&lattice, t0, 31, 31), Some(icon_eu::ID));
}

/// A cycle in which no source reached the lattice at all is 216 objects of "we do not know" about
/// the whole planet. Publishing it would swap in a manifest claiming the service is current, so
/// the baker fails closed and the previous generation stands.
#[test]
fn a_cycle_that_paints_nothing_refuses_to_publish() {
    let t0 = ts("2026-08-09T14:00:00Z");
    let lattice = sub_lattice(45_680_000, 1_460_000, 32, 32);
    // Everything the one layer has is four hours out of skew.
    let stale = synthetic_frames(
        gfs::ID,
        window(45_680_000, 1_460_000, 250_000, 8, 8),
        &[(t0 - 4 * 3_600, obcg::FLAG_FORECAST)],
        |_, _| 2,
    );
    let mosaic = Mosaic::from_sources(vec![stale]).expect("ranked");
    let times = CycleTimes { reference_time: t0 };
    let mut painted = 0usize;
    let mut total = 0usize;
    bake_cycle(&lattice, &mosaic, times, 2, &mut |object| {
        total += 1;
        painted += usize::from(object.fill.painted);
        Ok(())
    })
    .expect("baking itself succeeds; it is publishing that must refuse");
    assert!(total > 0 && painted == 0, "the premise: every object is entirely no-data");
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
    let mosaic = Mosaic::from_sources(vec![coarse]).expect("gfs is ranked");
    let object = emit_shard(&lattice, &mosaic, CycleTimes { reference_time: valid_at }, 0, 0).expect("emits");
    let mut scratch = vec![0u8; usize::from(lattice.tile_edge) * usize::from(lattice.tile_edge)];
    let header = obcg::validate(&object.bytes, &mut scratch).expect("valid object");
    assert_eq!(header.cell_size_m, LATTICE_CELL_SIZE_M, "the frame states the lattice, not the 27.75 km source");
    assert_eq!((header.cell_lat_udeg, header.cell_lon_udeg), (CELL_UDEG, CELL_UDEG));
}

// ---------------------------------------------------------------------------------------------
// The whole cycle, end to end
// ---------------------------------------------------------------------------------------------

/// Read a published directory store back as `key -> bytes`.
fn published_tree(dir: &std::path::Path) -> BTreeMap<String, Vec<u8>> {
    let mut tree = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(path) = stack.pop() {
        for entry in std::fs::read_dir(&path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let key = path.strip_prefix(dir).unwrap().to_string_lossy().replace('\\', "/");
                tree.insert(key, std::fs::read(&path).unwrap());
            }
        }
    }
    tree
}

/// One canonical cycle against the fixture upstreams and a directory store: the run is
/// deterministic, every object self-validates, and — the WXR4 (#1243) property — **the manifest and
/// the tree are the same statement**. Every present bit names an object that is there, at the key
/// the client computes and at the length and CRC the manifest promised; every clear bit names one
/// that is not, and nothing exists that the manifest did not name.
///
/// The lattice is a sub-window (the full 648 M-cell one takes 12 s in a release build and minutes
/// in a debug one), but the orchestration, the streaming shape, the fetchability proof and the
/// manifest are the production ones.
#[test]
fn a_canonical_cycle_publishes_exactly_what_its_manifest_says_and_repeats_itself() {
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
        let report = run_cycle(&lattice, &adapters, &mut upstream, &mut store, now, BAKE_THREADS, false)
            .expect("the canonical cycle publishes");
        let baked = usize::try_from(lattice.shard_count()).unwrap() * usize::try_from(CYCLE_FRAMES).unwrap();
        assert_eq!(
            report.published_objects + report.dry_shards,
            baked + 1,
            "every shard of every frame is either published or accounted for as dry, plus the manifest"
        );
        // The report names its sources in priority order, and only sources that have a row.
        assert_eq!(
            report.layers.iter().map(|(id, _, _)| id.as_str()).collect::<Vec<_>>(),
            vec![dwd_rv::ID, icon_eu::ID]
        );

        let tree = published_tree(&dir);
        assert_eq!(tree.len(), report.published_objects);
        let document = manifest_v2::from_json(tree.get(manifest_v2::MANIFEST_KEY).expect("the manifest")).expect("v2");
        assert_eq!(document.version, 2);
        assert_eq!(document.generation, "20260809T1430Z");
        assert_eq!(document.lattice.cell_size_m, LATTICE_CELL_SIZE_M);
        assert_eq!(document.frames.len(), usize::try_from(CYCLE_FRAMES).unwrap());
        assert_eq!(
            document.attribution.iter().map(|entry| entry.source_id.as_str()).collect::<Vec<_>>(),
            vec![dwd_rv::ID, icon_eu::ID],
            "every source that may have painted a cell, in priority order"
        );

        // The manifest is the tree, and the tree is the manifest.
        let mut named = std::collections::BTreeSet::from([manifest_v2::MANIFEST_KEY.to_string()]);
        let mut scratch = vec![0u8; usize::from(lattice.tile_edge) * usize::from(lattice.tile_edge)];
        for frame in &document.frames {
            for shard_row in 0..lattice.shard_rows() {
                for shard_col in 0..lattice.shard_cols() {
                    let key = manifest_v2::shard_key(
                        &document.key_prefix,
                        &document.generation,
                        frame.offset_min,
                        shard_col,
                        shard_row,
                    );
                    let listed = frame.shards.iter().find(|s| (s.col, s.row) == (shard_col, shard_row));
                    let bit = shard_row * lattice.shard_cols() + shard_col;
                    let byte = u8::from_str_radix(&frame.present[(bit as usize / 8) * 2..][..2], 16).unwrap();
                    let present = byte & (1 << (bit % 8)) != 0;
                    assert_eq!(present, listed.is_some(), "{key}: bitmap and shards[] disagree");
                    let Some(listed) = listed else {
                        assert!(!tree.contains_key(&key), "{key}: bitmap says dry, an object exists");
                        continue;
                    };
                    let bytes = tree.get(&key).unwrap_or_else(|| panic!("{key}: the manifest names it, it is missing"));
                    assert_eq!(bytes.len() as u64, listed.bytes, "{key}: length");
                    let header = obcg::validate(bytes, &mut scratch).unwrap_or_else(|e| panic!("{key}: {e:?}"));
                    assert_eq!(format!("0x{:08X}", header.object_crc32), listed.object_crc32, "{key}: CRC");
                    assert_eq!(header.cell_size_m, LATTICE_CELL_SIZE_M);
                    assert_eq!(listed.observed, header.flags & obcg::FLAG_OBSERVED != 0, "{key}: observed");
                    named.insert(key);
                }
            }
        }
        assert_eq!(named.into_iter().collect::<Vec<_>>(), tree.keys().cloned().collect::<Vec<_>>());

        let _ = std::fs::remove_dir_all(&dir);
        trees.push(tree);
    }
    assert_eq!(trees[0], trees[1], "same upstream bytes, byte-identical published tree");
}

/// **Missing is not dry, and dry is not missing** (#1243). A source that covers the western half of
/// the lattice with dry cells and nothing at all in the east must produce two different answers:
/// the western shards are *omitted* with a clear presence bit, and the eastern ones are *published*
/// full of intensity 15. Conflating them is the exact failure the epic forbids, and it is the one a
/// "skip anything with nothing in it" shortcut walks straight into.
#[test]
fn a_dry_shard_is_omitted_and_a_no_data_shard_is_published() {
    let t0 = ts("2026-08-09T14:00:00Z");
    let lattice = sub_lattice(45_680_000, 1_460_000, 32, 32);
    let frames: Vec<(i64, u16)> =
        (0..CYCLE_FRAMES).map(|frame| (t0 + i64::from(frame) * 900, obcg::FLAG_FORECAST)).collect();
    // The western 16 columns, entirely dry. The eastern 16 have no source at all.
    let west =
        synthetic_frames(gfs::ID, window(45_680_000, 1_460_000, CELL_UDEG, 16, 32), &frames, |_, _| INTENSITY_DRY);
    let mosaic = Mosaic::from_sources(vec![west]).expect("ranked");
    let times = CycleTimes { reference_time: t0 };
    let mut scratch = vec![0u8; usize::from(lattice.tile_edge) * usize::from(lattice.tile_edge)];
    let mut seen = 0usize;
    // The manifest is built here too, so the outage travels the whole way to a **set** presence bit
    // rather than being proved at the bake layer and asserted at the document layer separately.
    let mut document = manifest_v2::Builder::new(&lattice, times, t0, Vec::new(), Vec::new());
    bake_cycle(&lattice, &mosaic, times, 2, &mut |object| {
        seen += 1;
        if !object.fill.all_dry {
            document.record(
                object.offset_min,
                object.col,
                object.row,
                object.bytes.len() as u64,
                object.object_crc32,
                object.observed,
            );
        }
        if object.col == 0 {
            assert!(object.fill.all_dry, "s0-{}: dry everywhere, so it is omitted", object.row);
            assert!(object.fill.painted, "the dry source did paint it");
        } else {
            assert!(!object.fill.all_dry, "s1-{}: no source reaches it — that is no-data, not dry", object.row);
            assert!(!object.fill.painted);
            let header = obcg::validate(&object.bytes, &mut scratch).expect("a valid no-data object");
            assert_eq!(header.width, 16);
            assert_eq!(
                published_cell(&object.bytes, 0, 0),
                INTENSITY_NODATA,
                "an unreachable shard publishes 'we do not know', it does not vanish"
            );
        }
        Ok(())
    })
    .expect("bakes");
    assert_eq!(seen, usize::try_from(lattice.shard_count()).unwrap() * usize::try_from(CYCLE_FRAMES).unwrap());

    // Every frame: the two dry western shards are bit-clear and unlisted; the two no-data eastern
    // ones are bit-set and listed. A source outage is a published object the client must fetch —
    // it can never reach a rider as "no rain".
    let manifest = document.finish();
    assert_eq!(manifest.frames.len(), usize::try_from(CYCLE_FRAMES).unwrap());
    for frame in &manifest.frames {
        assert_eq!(frame.present, "0a", "bits 1 and 3 — the (1,0) and (1,1) shards — and nothing else");
        assert_eq!(
            frame.shards.iter().map(|shard| (shard.col, shard.row)).collect::<Vec<_>>(),
            vec![(1, 0), (1, 1)],
            "f{}: only the no-data shards are published",
            frame.offset_min
        );
        assert!(frame.shards.iter().all(|shard| !shard.observed));
    }
}

/// **A corrupt upstream publishes nothing, and the previous generation stands byte for byte.**
///
/// Carried over from the multi-product suite #1246 deleted, and it says something simpler than it
/// used to. That version had to prove *isolation*: one broken adapter must not cost the other
/// products their publication, because the manifest listed four independently selectable products
/// and a per-adapter systemd timer published one of them. There is one dataset now and the mosaic
/// needs every source's cells, so isolation is neither available nor wanted — a cycle that cannot
/// bake a source cannot bake a complete dataset, and publishing a partial one would swap in a
/// manifest claiming the service is current over a hole.
///
/// So what survives is the half that always mattered: **fail-closed**. A truncated tar, a flipped
/// byte inside an HDF5 member, a short model lead — each fails the cycle, moves no object, and
/// leaves the previously published generation and its manifest exactly as they were. One cycle of
/// freshness is the whole cost, and the next tick recovers.
#[test]
fn a_corrupt_upstream_publishes_nothing_and_leaves_the_previous_generation_standing() {
    let lattice = sub_lattice(45_680_000, 1_460_000, 64, 48);
    let now = ts("2026-08-09T14:30:00Z");
    let dwd = dwd_rv::DwdRv;
    let icon = icon_eu::IconEu;
    let adapters: [&dyn Adapter; 2] = [&dwd, &icon];
    let dir = std::env::temp_dir().join(format!("obc-wx-corrupt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut store = DirStore::new(&dir);

    // A good generation to protect.
    run_cycle(&lattice, &adapters, &mut european_upstream(), &mut store, now, 2, false).expect("the good cycle");
    let published = published_tree(&dir);
    assert!(published.len() > 1, "the good cycle published objects and a manifest");

    let truncated_tar = || {
        let mut tar = fixture("composite_rv_20260809_1420.tar");
        tar.truncate(tar.len() / 2);
        tar
    };
    let flipped_tar = || {
        let mut tar = fixture("composite_rv_20260809_1420.tar");
        let middle = tar.len() / 2;
        tar[middle] ^= 0x40;
        tar
    };
    let short_lead = || {
        let mut lead = fixture("icon-eu-2026080906_005.grib2.bz2");
        lead.truncate(lead.len() - 100);
        lead
    };

    for (name, broken) in [
        ("a truncated RV tar", Box::new(truncated_tar) as Box<dyn Fn() -> Vec<u8>>),
        ("a flipped byte inside an HDF5 member", Box::new(flipped_tar)),
    ] {
        let mut upstream = european_upstream();
        upstream.insert(dwd_rv::LATEST_URL, broken(), Some("\"changed\""));
        let error = run_cycle(&lattice, &adapters, &mut upstream, &mut store, now + 900, 2, false)
            .expect_err("a corrupt source must fail the cycle");
        eprintln!("{name}: {error}");
        assert_eq!(published_tree(&dir), published, "{name}: the previous generation must be untouched");
    }

    // The same for the other side of the mosaic: a model lead that stops mid-stream.
    let mut upstream = european_upstream();
    upstream.insert(icon_eu::lead_url(ts(ICON_RUN), 5), short_lead(), None);
    let error = run_cycle(&lattice, &adapters, &mut upstream, &mut store, now + 1_800, 2, false)
        .expect_err("a short model lead must fail the cycle");
    eprintln!("a truncated ICON lead: {error}");
    assert_eq!(published_tree(&dir), published, "a broken model must not move an object either");

    let _ = std::fs::remove_dir_all(&dir);
}

/// The retention contract WXR8's sweep derives its delete set from: a generation names the two
/// before it, newest first, and nothing older. The baker keeps no state, so this is read back out
/// of the manifest it published last time and nowhere else.
#[test]
fn each_generation_names_the_two_before_it() {
    let lattice = sub_lattice(45_680_000, 1_460_000, 64, 48);
    let dwd = dwd_rv::DwdRv;
    let icon = icon_eu::IconEu;
    let adapters: [&dyn Adapter; 2] = [&dwd, &icon];
    let dir = std::env::temp_dir().join(format!("obc-wx-generations-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut store = DirStore::new(&dir);

    let mut chains = Vec::new();
    for step in 0..4 {
        let now = ts("2026-08-09T14:30:00Z") + step * 900;
        let mut upstream = european_upstream();
        run_cycle(&lattice, &adapters, &mut upstream, &mut store, now, 2, false).expect("publishes");
        let raw = std::fs::read(dir.join(manifest_v2::MANIFEST_KEY)).expect("the manifest");
        let document = manifest_v2::from_json(&raw).expect("v2");
        chains.push((document.generation, document.previous_generations));
    }
    assert_eq!(
        chains,
        vec![
            ("20260809T1430Z".to_string(), vec![]),
            ("20260809T1445Z".to_string(), vec!["20260809T1430Z".to_string()]),
            ("20260809T1500Z".to_string(), vec!["20260809T1445Z".to_string(), "20260809T1430Z".to_string()]),
            ("20260809T1515Z".to_string(), vec!["20260809T1500Z".to_string(), "20260809T1445Z".to_string()]),
        ],
        "current plus exactly two, newest first"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// **Publish, then sweep, then check what is left** (WXR8 #1247) — the end-to-end half of the
/// retention story, over the real `run_cycle` and a real store rather than a recording double.
///
/// The property is one sentence: *after any cycle, the generations that exist in the tree are
/// exactly the generations the published manifest names.* That is stronger than "the sweep deleted
/// N-3", because it also fails if the sweep ever deleted something still on the chain — the
/// failure mode this feature introduced and the reason it is gated this way rather than by counting
/// deletions.
#[test]
fn the_tree_holds_exactly_the_generations_the_published_manifest_names() {
    let lattice = sub_lattice(45_680_000, 1_460_000, 64, 48);
    let dwd = dwd_rv::DwdRv;
    let icon = icon_eu::IconEu;
    let adapters: [&dyn Adapter; 2] = [&dwd, &icon];
    let dir = std::env::temp_dir().join(format!("obc-wx-sweep-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut store = DirStore::new(&dir);

    // The generations present in the tree, read from the object keys themselves.
    let generations_on_disk = |dir: &std::path::Path| -> BTreeSet<String> {
        published_tree(dir)
            .keys()
            .filter(|key| key.ends_with(".obcg"))
            .filter_map(|key| key.split('/').nth(2).map(str::to_string))
            .collect()
    };

    let mut swept = Vec::new();
    for step in 0..5 {
        let now = ts("2026-08-09T14:30:00Z") + step * 900;
        let mut upstream = european_upstream();
        let report = run_cycle(&lattice, &adapters, &mut upstream, &mut store, now, 2, false).expect("publishes");
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);

        let raw = std::fs::read(dir.join(manifest_v2::MANIFEST_KEY)).expect("the manifest");
        let document = manifest_v2::from_json(&raw).expect("v2");
        let named: BTreeSet<String> =
            std::iter::once(document.generation.clone()).chain(document.previous_generations.iter().cloned()).collect();
        assert_eq!(
            generations_on_disk(&dir),
            named,
            "step {step}: the tree and the manifest disagree about which generations exist"
        );
        swept.push((report.swept.generations.clone(), report.swept.deleted_objects > 0));
    }

    // Nothing to retire until a fourth generation exists; from then on, exactly one per cycle, and
    // it is the one that just fell off the chain.
    assert_eq!(
        swept,
        vec![
            (vec![], false),
            (vec![], false),
            (vec![], false),
            (vec!["20260809T1430Z".to_string()], true),
            (vec!["20260809T1445Z".to_string()], true),
        ]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A sweep that cannot delete must not turn a good publish into a failed cycle: the manifest is
/// already in place, the objects it no longer names are unreferenced, and the bucket's 1-day
/// lifecycle rule is what collects the leak. The cycle reports a warning and succeeds.
#[test]
fn a_store_that_refuses_to_delete_still_publishes_a_good_cycle() {
    use obc_wx_bake::publish::{Deleted, ObjectStore, PlannedObject};

    /// A directory store with its delete wired to fail — everything else is the real one.
    struct NoDelete(DirStore);
    impl ObjectStore for NoDelete {
        fn describe(&self) -> String {
            self.0.describe()
        }
        fn put(&mut self, object: &PlannedObject) -> Result<(), String> {
            self.0.put(object)
        }
        fn head(&mut self, key: &str) -> Result<Option<u64>, String> {
            self.0.head(key)
        }
        fn get(&mut self, key: &str) -> Result<Option<Vec<u8>>, String> {
            self.0.get(key)
        }
        fn delete(&mut self, _key: &str) -> Result<Deleted, String> {
            Err("503 SlowDown".to_string())
        }
    }

    let lattice = sub_lattice(45_680_000, 1_460_000, 64, 48);
    let dwd = dwd_rv::DwdRv;
    let icon = icon_eu::IconEu;
    let adapters: [&dyn Adapter; 2] = [&dwd, &icon];
    let dir = std::env::temp_dir().join(format!("obc-wx-sweep-fails-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut store = NoDelete(DirStore::new(&dir));

    let mut last = None;
    for step in 0..4 {
        let now = ts("2026-08-09T14:30:00Z") + step * 900;
        let mut upstream = european_upstream();
        last = Some(
            run_cycle(&lattice, &adapters, &mut upstream, &mut store, now, 2, false)
                .expect("a sweep failure is not a cycle failure"),
        );
    }
    let report = last.expect("four cycles");
    assert_eq!(report.swept.generations, vec!["20260809T1430Z"], "it still reports what it tried to retire");
    assert_eq!(report.swept.deleted_objects, 0);
    assert_eq!(report.warnings.len(), 1, "one warning for the generation, not one per key");
    assert!(report.warnings[0].contains("retention sweep"), "{}", report.warnings[0]);
    // The report's own field carries it too — it is public and documented, so draining it into
    // `warnings` and leaving it empty would be a lie (#1274 r1 finding 12).
    assert_eq!(report.swept.warnings, report.warnings);
    // …and the manifest is the one this cycle published, byte for byte the good outcome.
    let raw = std::fs::read(dir.join(manifest_v2::MANIFEST_KEY)).expect("the manifest");
    assert_eq!(manifest_v2::from_json(&raw).expect("v2").generation, "20260809T1515Z");
    let _ = std::fs::remove_dir_all(&dir);
}

/// **A cycle must refuse to publish a manifest older than the one already at the key** (#1274 r1
/// blocker 2). The sweep is correct at every individual step, so this is not about a wrong delete:
/// it is about a *stale republish* putting an old chain back over a newer one, naming generations
/// later cycles legitimately swept — and by §10.3 a 404 on a named generation is an error every
/// falling-back client receives.
///
/// Reachable without any concurrency at all: a backwards clock step, or a bake started by hand
/// outside the unit's `flock`. Both look like this.
#[test]
fn a_cycle_older_than_the_published_manifest_refuses_to_publish() {
    let lattice = sub_lattice(45_680_000, 1_460_000, 64, 48);
    let dwd = dwd_rv::DwdRv;
    let icon = icon_eu::IconEu;
    let adapters: [&dyn Adapter; 2] = [&dwd, &icon];
    let dir = std::env::temp_dir().join(format!("obc-wx-backwards-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut store = DirStore::new(&dir);

    let now = ts("2026-08-09T15:00:00Z");
    run_cycle(&lattice, &adapters, &mut european_upstream(), &mut store, now, 2, false).expect("the good cycle");
    let before = published_tree(&dir);

    // A stalled racer, or a clock that stepped back one cadence step.
    let error = run_cycle(&lattice, &adapters, &mut european_upstream(), &mut store, now - 900, 2, false)
        .expect_err("a manifest that goes backwards must not be published");
    assert!(error.contains("refusing to publish a manifest that goes backwards"), "{error}");
    assert!(error.contains("20260809T1500Z") && error.contains("20260809T1445Z"), "names both: {error}");
    assert_eq!(published_tree(&dir), before, "the refused cycle wrote nothing at all");

    // Re-baking the *same* reference time is the idempotent republish the design rests on, and
    // stays allowed — equality is not going backwards.
    run_cycle(&lattice, &adapters, &mut european_upstream(), &mut store, now, 2, false).expect("a re-bake is fine");
    let _ = std::fs::remove_dir_all(&dir);
}

/// **The manifest is read back before it licenses 216 deletes** (#1274 r1 finding 3). Every frame
/// object is `head`ed at its exact length before the swap; until round 1 the one object that both
/// wedges the next cycle when unreadable *and* authorises the whole sweep was the only one nobody
/// checked, on a bucket with a recorded history of tearing bodies.
///
/// The tear lands on the **fourth** cycle deliberately (#1274 r2). Tearing the first one tests only
/// the error string: a bootstrap's delete set is empty, so the sweep would have deleted nothing
/// anyway and the `panic!` below could never fire. The fourth cycle is the first with a generation
/// to retire, so it is the first where "the readback stopped the sweep" is a claim with teeth — and
/// the first where all four generations surviving on disk means something.
#[test]
fn a_manifest_that_does_not_read_back_stops_the_sweep() {
    use obc_wx_bake::publish::{Deleted, ObjectStore, PlannedObject};

    /// A directory store that can be told to corrupt the manifest the instant it is written — the
    /// shape of a torn body, applied to the one key that matters.
    struct TearsTheManifest {
        inner: DirStore,
        tear: bool,
    }
    impl ObjectStore for TearsTheManifest {
        fn describe(&self) -> String {
            self.inner.describe()
        }
        fn put(&mut self, object: &PlannedObject) -> Result<(), String> {
            self.inner.put(object)?;
            if self.tear && object.key == manifest_v2::MANIFEST_KEY {
                let half = object.bytes.len() / 2;
                self.inner.put(&PlannedObject { bytes: object.bytes[..half].to_vec(), ..object.clone() })?;
            }
            Ok(())
        }
        fn head(&mut self, key: &str) -> Result<Option<u64>, String> {
            self.inner.head(key)
        }
        fn get(&mut self, key: &str) -> Result<Option<Vec<u8>>, String> {
            self.inner.get(key)
        }
        fn delete(&mut self, _key: &str) -> Result<Deleted, String> {
            panic!("the sweep must not run against a manifest that did not read back");
        }
    }

    let lattice = sub_lattice(45_680_000, 1_460_000, 64, 48);
    let dwd = dwd_rv::DwdRv;
    let icon = icon_eu::IconEu;
    let adapters: [&dyn Adapter; 2] = [&dwd, &icon];
    let dir = std::env::temp_dir().join(format!("obc-wx-torn-put-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut store = TearsTheManifest { inner: DirStore::new(&dir), tear: false };

    // Three clean cycles fill the chain up. Nothing has fallen off it yet, so `delete` is never
    // reached and the tripwire above stays quiet on its own merits rather than on the tear's.
    for step in 0..3 {
        let now = ts("2026-08-09T14:30:00Z") + step * 900;
        let report = run_cycle(&lattice, &adapters, &mut european_upstream(), &mut store, now, 2, false)
            .expect("a clean cycle publishes");
        assert!(report.swept.generations.is_empty(), "nothing is off the chain until the fourth cycle");
    }

    // The fourth would retire 14:30 — if it ever got that far.
    store.tear = true;
    let error =
        run_cycle(&lattice, &adapters, &mut european_upstream(), &mut store, ts("2026-08-09T15:15:00Z"), 2, false)
            .expect_err("a manifest that does not read back fails the cycle");
    assert!(error.contains("refusing to sweep"), "{error}");

    // …and the generation it was about to retire is still there, along with the other three.
    let generations: BTreeSet<String> = published_tree(&dir)
        .keys()
        .filter(|key| key.ends_with(".obcg"))
        .filter_map(|key| key.split('/').nth(2).map(str::to_string))
        .collect();
    assert_eq!(
        generations,
        BTreeSet::from([
            "20260809T1430Z".to_string(),
            "20260809T1445Z".to_string(),
            "20260809T1500Z".to_string(),
            "20260809T1515Z".to_string(),
        ]),
        "a failed readback must leave every generation standing, including the one due for retirement"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------------------------
// The other half of the production adapter set
// ---------------------------------------------------------------------------------------------

const HRRR_OBJECTS: [(u32, u64); 3] = [(2, 210_757_046), (3, 214_632_128), (4, 220_555_508)];
const HRRR_RANGES: [(u32, u32, u64); 9] = [
    (2, 120, 183_664_477),
    (3, 135, 25_809_346),
    (3, 150, 79_031_140),
    (3, 165, 132_718_351),
    (3, 180, 186_502_886),
    (4, 195, 26_244_769),
    (4, 210, 80_983_359),
    (4, 225, 136_058_399),
    (4, 240, 191_463_451),
];
const GFS_SPANS: [(u32, u64, u64); 16] = [
    (1, 537_540_348, 427_603_385),
    (2, 538_822_727, 428_091_880),
    (3, 539_798_514, 428_475_805),
    (4, 540_724_755, 428_752_482),
    (5, 542_923_155, 430_080_077),
    (6, 544_451_780, 431_023_684),
    (7, 542_096_820, 432_070_312),
    (8, 543_890_390, 433_033_986),
    (9, 543_734_730, 432_288_308),
    (10, 544_255_893, 432_328_102),
    (11, 544_322_108, 431_989_179),
    (12, 545_133_960, 432_276_114),
    (13, 541_397_261, 431_060_039),
    (14, 541_818_663, 430_713_865),
    (15, 542_144_204, 430_643_461),
    (16, 546_445_777, 433_214_890),
];

/// The captured American snapshot, wired exactly as `us_gfs_cycle.rs` wires it.
fn american_upstream() -> FixtureUpstream {
    let mut upstream = FixtureUpstream::default();
    let observation = ts("2026-08-09T16:58:00Z");
    let hrrr_run = ts("2026-08-09T15:00:00Z");
    let gfs_run = ts("2026-08-09T12:00:00Z");
    upstream.insert(mrms::object_url(observation), fixture("mrms-conus-20260809-165800.grib2.gz"), None);
    for file in hrrr::SUBHOURLY_FILES {
        upstream.insert(
            hrrr::index_url(hrrr_run, file),
            fixture(&format!("hrrr-conus-20260809T15-f{file:02}.idx")),
            None,
        );
    }
    for (file, object_len) in HRRR_OBJECTS {
        upstream.declare(hrrr::object_url(hrrr_run, file), object_len);
    }
    for (file, lead, start) in HRRR_RANGES {
        let object_len = HRRR_OBJECTS.iter().find(|(candidate, _)| *candidate == file).expect("declared").1;
        upstream.insert_range(
            hrrr::object_url(hrrr_run, file),
            object_len,
            start,
            fixture(&format!("hrrr-conus-20260809T15-prate-t{lead}.grib2")),
        );
    }
    for (lead, object_len, start) in GFS_SPANS {
        upstream.insert(
            gfs::index_url(gfs_run, lead),
            fixture(&format!("gfs-global-20260809T12-f{lead:03}.idx")),
            None,
        );
        upstream.insert_range(
            gfs::object_url(gfs_run, lead),
            object_len,
            start,
            fixture(&format!("gfs-global-20260809T12-apcp-f{lead:03}.grib2")),
        );
    }
    upstream
}

/// **The two CONUS sources and the real floor, mosaicked from real fixtures.**
///
/// MRMS (1 km observation) and HRRR (3 km model) were one composed `us` product until #1246, whose
/// frames carried a *different window each*. They are two layers now, at two ranks, on two
/// windows — which is what the mosaic wanted all along — and they sit over the real GFS floor so
/// the CONUS radar edge (MRMS `NO_COVERAGE` → 15 → fall through) is a real fall-through rather
/// than a synthetic one.
#[test]
fn the_conus_sources_and_the_real_floor_mosaic_over_conus() {
    let now = ts("2026-08-09T17:00:00Z");
    let mut upstream = american_upstream();
    let mrms_source = bake(&mrms::Mrms, &mut upstream, now);
    let hrrr_source = bake(&hrrr::Hrrr, &mut upstream, now);
    let gfs_product = bake(&gfs::GfsFloor, &mut upstream, now);
    assert_eq!(
        (mrms_source.geometry.width, mrms_source.geometry.height),
        (mrms::GEOMETRY.width, mrms::GEOMETRY.height)
    );
    assert_eq!(
        (hrrr_source.geometry.width, hrrr_source.geometry.height),
        (hrrr::GEOMETRY.width, hrrr::GEOMETRY.height)
    );
    assert_eq!(mrms_source.frames.len(), 1, "MRMS contributes one observation");
    assert!(!hrrr_source.frames.is_empty(), "HRRR contributes the forward window");

    let mosaic = Mosaic::from_sources(vec![mrms_source, hrrr_source, gfs_product]).expect("all three are ranked");
    let times = CycleTimes::anchored_at(now);
    // Kansas: deep inside CONUS, and inside the GFS floor too.
    let lattice = sub_lattice(37_000_000, -100_000_000, 256, 192);
    let mut radar_or_model = 0usize;
    let mut floor_cells = 0usize;
    for shard in 0..lattice.shard_count() {
        let window = lattice.shard(shard).expect("shard");
        let object = emit_shard(&lattice, &mosaic, times, 0, shard).expect("emits and self-validates");
        for cell in (0..window.cells()).step_by(37) {
            let col = window.col0 + (cell as u32) % window.width;
            let row = window.row0 + (cell as u32) / window.width;
            // Nothing is unsourced over Kansas: the floor is beneath everything.
            let value = published_cell(&object.bytes, col - window.col0, row - window.row0);
            assert_ne!(value, INTENSITY_NODATA, "({col},{row}) is inside the floor and must not be no-data");
            match mosaic.winner_at(&lattice, times.valid_at(0), col, row) {
                Some(id) if id == mrms::ID || id == hrrr::ID => radar_or_model += 1,
                Some(id) if id == gfs::ID => floor_cells += 1,
                other => panic!("unexpected winner {other:?} at ({col},{row})"),
            }
        }
    }
    eprintln!("CONUS mosaic: {radar_or_model} us, {floor_cells} floor");
    assert!(radar_or_model > 0, "MRMS must answer inside CONUS");

    // The floor really is global: it answers a mid-Pacific cell no other source reaches, including
    // one in the antimeridian column the source window had to drop.
    let pacific = sub_lattice(-10_000_000, 179_900_000, 8, 8);
    let object = emit_shard(&pacific, &mosaic, times, 0, 0).expect("emits");
    for col in 0..pacific.shard(0).expect("shard").width {
        assert_eq!(mosaic.winner_at(&pacific, times.valid_at(0), col, 0), Some(gfs::ID), "column {col}");
        assert_ne!(published_cell(&object.bytes, col, 0), INTENSITY_NODATA, "the antimeridian is painted");
    }
}

/// A layer whose source has no row in the priority table cannot be mosaicked — that is a bakery
/// configuration bug and it fails the cycle closed rather than silently dropping a source.
#[test]
fn an_unranked_source_refuses_to_join_the_mosaic() {
    let valid_at = ts("2026-08-09T14:00:00Z");
    let orphan = synthetic("not-a-source", window(45_680_000, 1_460_000, 10_000, 4, 4), valid_at, |_, _| 1);
    let error = MosaicLayer::from_source(orphan).expect_err("an unranked source must be refused");
    assert!(error.contains("MOSAIC_PRIORITY"), "{error}");
}
