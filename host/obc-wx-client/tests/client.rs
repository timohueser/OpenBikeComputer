//! The client suite. Nothing here opens a socket: every byte comes from `specs/vectors/`, from the
//! captured MET document in `tests/fixtures/`, or from a shard this file encodes through the
//! production OBCG encoder. `--weather live` is the only network path in the project, and it is
//! behind a flag.
//!
//! What this suite stopped testing in WXR5 #1244 is as informative as what it tests: there is no
//! tier ladder, no bbox containment, no expired-product shadowing and no lattice-nesting refusal,
//! because there is one dataset on one lattice and nothing to choose between. Selection is
//! [`manifest_v2::Grid::shards_for`], four divisions, pinned against the Swift client by
//! `specs/vectors/wx-manifest-v2.json` in `tests/manifest_v2.rs`.

use obc_formats::obcg;
use obc_formats::precip4::{INTENSITY_DRY, INTENSITY_NODATA};
use obc_wx_client::bundle::{self, FrameInput, Lattice, Scene};
use obc_wx_client::corridor::{self, Corridor, Crop, ShardRead, CORRIDOR_RADIUS_M, METRES_PER_DEGREE_LAT};
use obc_wx_client::http::{FailureControls, FaultyHttp, FixtureHttp};
use obc_wx_client::manifest_v2::{self, Bbox, Grid, ShardId};
use obc_wx_client::{met, NoRainMap, WeatherClient};

const ORIGIN: &str = "https://wx.test";
const MANIFEST_URL: &str = "https://wx.test/wx/v2/manifest.json";
const MET_ENDPOINT: &str = "https://met.test/complete";
const GENERATION: &str = "20260810T1430Z";
const KEY_PREFIX: &str = "wx/v2";
/// The canonical lattice's own numbers, so the test dataset is a *window of* the production grid
/// rather than a shape of its own: 0.01° cells, 1,113 m, and the same tiling parameters.
const CELL_UDEG: u32 = 10_000;
const CELL_SIZE_M: u16 = 1_113;

fn fixture(name: &str) -> Vec<u8> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/");
    std::fs::read(format!("{path}{name}")).unwrap_or_else(|error| panic!("{name}: {error}"))
}

fn rfc3339(unix: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(unix, 0).unwrap().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn hourly() -> met::Hourly {
    met::decode(&fixture("met-freiburg-24h.json"), 0).expect("MET decode")
}

// ── a small window of the canonical lattice ────────────────────────────────────────────────

/// A 64 × 64-cell lattice at 47°N/7°E in 32 × 32 shards: four objects, one seam on each axis.
/// Small enough that a debug build encodes it instantly, and every cell is a canonical cell.
fn test_grid() -> Grid {
    Grid {
        south_lat_udeg: 47_000_000,
        west_lon_udeg: 7_000_000,
        cell_udeg: CELL_UDEG,
        width: 64,
        height: 64,
        shard_width: 32,
        shard_height: 32,
        shard_cols: 2,
        shard_rows: 2,
        tile_edge: 16,
        entries_per_page: 4,
        cell_size_m: CELL_SIZE_M,
        covered_rows: 0..64,
        key_prefix: KEY_PREFIX.to_string(),
        generation: GENERATION.to_string(),
    }
}

/// One shard object, encoded by the production OBCG encoder over the *derived* geometry — so a
/// fixture can never describe an object it does not match, which is the discipline the manifest's
/// header check depends on.
fn shard_object(grid: &Grid, shard: ShardId, valid_at: i64, observed: bool, cell: impl Fn(u32, u32) -> u8) -> Vec<u8> {
    let geometry = grid.shard_geometry(shard).expect("a shard of this grid");
    let mut cells = Vec::with_capacity((geometry.width * geometry.height) as usize);
    for row in 0..geometry.height {
        for col in 0..geometry.width {
            cells.push(cell(shard.col * grid.shard_width + col, shard.row * grid.shard_height + row));
        }
    }
    let input = obcg::FrameInput {
        product_id: obcg::PRODUCT_MOSAIC,
        tier: obcg::TIER_MOSAIC,
        flags: if observed { obcg::FLAG_OBSERVED } else { obcg::FLAG_FORECAST },
        valid_at,
        reference_time: valid_at,
        south_lat_udeg: geometry.south_udeg,
        west_lon_udeg: geometry.west_udeg,
        cell_lat_udeg: geometry.cell_udeg,
        cell_lon_udeg: geometry.cell_udeg,
        width: geometry.width,
        height: geometry.height,
        cell_size_m: geometry.cell_size_m,
        tile_edge: geometry.tile_edge,
        entries_per_page: geometry.entries_per_page,
        cells: &cells,
    };
    let mut scratch = vec![0u8; usize::from(geometry.tile_edge) * usize::from(geometry.tile_edge)];
    let mut bytes = vec![0u8; obcg::max_encoded_len(&input).expect("bound") as usize];
    let len = obcg::encode_format(&input, &mut scratch, &mut bytes).expect("encode");
    bytes.truncate(len);
    bytes
}

/// One frame of the test dataset: which shards were published, and what is in them.
struct FrameSpec {
    offset_min: u32,
    valid_at: i64,
    /// `(shard, bytes, observed)` — exactly the shards whose presence bit is set.
    objects: Vec<(ShardId, Vec<u8>, bool)>,
}

/// The v2 document for a dataset, written the way the baker writes it: the bitmap and `shards[]`
/// are one statement, and every length and CRC comes from the object itself.
fn manifest_json(grid: &Grid, frames: &[FrameSpec], stale_after: i64, manifest_max_age_s: i64) -> String {
    let mut frame_entries = Vec::new();
    for frame in frames {
        let mut present = vec![0u8; (grid.shard_count().div_ceil(8)) as usize];
        let mut shards = Vec::new();
        for (shard, bytes, observed) in &frame.objects {
            let bit = grid.bit_of(*shard).expect("a shard of this grid");
            present[(bit / 8) as usize] |= 1 << (bit % 8);
            let header = obcg::decode_header(bytes[..obcg::HEADER_LEN].try_into().unwrap()).expect("header");
            shards.push(format!(
                r#"{{"col":{},"row":{},"bytes":{},"object_crc32":"0x{:08X}","observed":{observed}}}"#,
                shard.col,
                shard.row,
                bytes.len(),
                header.object_crc32,
            ));
        }
        let hex: String = present.iter().map(|byte| format!("{byte:02x}")).collect();
        frame_entries.push(format!(
            r#"{{"offset_min":{},"valid_at":"{}","present":"{hex}","shards":[{}]}}"#,
            frame.offset_min,
            rfc3339(frame.valid_at),
            shards.join(","),
        ));
    }
    let reference_time = frames.first().map_or(0, |frame| frame.valid_at);
    format!(
        r#"{{"version":2,"generation":"{GENERATION}","generated_at":"{}","reference_time":"{}",
            "key_prefix":"{KEY_PREFIX}","previous_generations":[],
            "lattice":{{"south_lat_udeg":{},"west_lon_udeg":{},"cell_udeg":{},"width":{},"height":{},
                        "shard_width":{},"shard_height":{},"shard_cols":{},"shard_rows":{},
                        "tile_edge":{},"entries_per_page":{},"cell_size_m":{},
                        "covered_rows":{{"start":{},"end":{}}}}},
            "cadence":{{"frame_step_min":15,"frames":{},"max_source_skew_s":1800}},
            "freshness":{{"manifest_max_age_s":{manifest_max_age_s},
                          "next_generation_expected_at":"{}","stale_after":"{}"}},
            "attribution":[{{"source_id":"test","text":"Test data","url":"https://example.invalid"}}],
            "frames":[{}]}}"#,
        rfc3339(reference_time),
        rfc3339(reference_time),
        grid.south_lat_udeg,
        grid.west_lon_udeg,
        grid.cell_udeg,
        grid.width,
        grid.height,
        grid.shard_width,
        grid.shard_height,
        grid.shard_cols,
        grid.shard_rows,
        grid.tile_edge,
        grid.entries_per_page,
        grid.cell_size_m,
        grid.covered_rows.start,
        grid.covered_rows.end,
        frames.len(),
        rfc3339(stale_after),
        rfc3339(stale_after),
        frame_entries.join(","),
    )
}

/// The default dataset: two frames, all four shards published, a rain gradient across the seam.
fn dataset(now: i64, grid: &Grid) -> Vec<FrameSpec> {
    (0..2)
        .map(|index| {
            let valid_at = now + i64::from(index) * 900;
            let offset_min = index * 15;
            let objects = (0..grid.shard_rows)
                .flat_map(|row| (0..grid.shard_cols).map(move |col| ShardId { col, row }))
                .map(|shard| {
                    let bytes =
                        shard_object(grid, shard, valid_at, index == 0, |col, row| ((col + row + index) % 13) as u8);
                    (shard, bytes, index == 0)
                })
                .collect();
            FrameSpec { offset_min, valid_at, objects }
        })
        .collect()
}

/// A client wired to a fixture origin serving `frames` plus the manifest and MET.
fn wired(grid: &Grid, frames: &[FrameSpec], now: i64, stale_after: i64) -> (FixtureHttp, WeatherClient, Corridor) {
    let document = manifest_json(grid, frames, stale_after, 60);
    let mut http = FixtureHttp::new()
        .with_object(MANIFEST_URL, document.into_bytes())
        .with_object(format!("{MET_ENDPOINT}?lat=47.3200&lon=7.3200"), fixture("met-freiburg-24h.json"))
        // The far-side position the out-of-domain test asks about. MET answers everywhere; the OBC
        // lattice is what does not reach, and keeping the two independent is the point.
        .with_object(format!("{MET_ENDPOINT}?lat=-33.9000&lon=151.2000"), fixture("met-freiburg-24h.json"));
    for frame in frames {
        for (shard, bytes, _) in &frame.objects {
            http = http.with_object(corridor::join(ORIGIN, &grid.shard_key(frame.offset_min, *shard)), bytes.clone());
        }
    }
    // Centred on the lattice's own middle, so a 20 km disc straddles both seams.
    let corridor = Corridor::around(47_320_000, 7_320_000, 20_000.0);
    let _ = now;
    (http, WeatherClient::new(ORIGIN).with_met_endpoint(MET_ENDPOINT), corridor)
}

// ── the whole job ──────────────────────────────────────────────────────────────────────────

/// The fetch path end to end: manifest v2, shard keys by arithmetic, Range reads, one OBCW bundle
/// the **device's own reader** opens. Nothing selects anything.
#[test]
fn a_whole_fetch_reads_the_shards_arithmetic_names_and_builds_a_device_readable_bundle() {
    let now = 1_800_000_000;
    let grid = test_grid();
    let frames = dataset(now, &grid);
    let (mut http, mut client, corridor) = wired(&grid, &frames, now, now + 3_600);
    let bundle = client.fetch(&mut http, &corridor, now, 42).expect("fetch");

    assert_eq!(bundle.diagnostics.generation.as_deref(), Some(GENERATION));
    assert_eq!(bundle.diagnostics.no_rain_map, None);
    assert_eq!(bundle.diagnostics.failed_frames, 0);
    let source = obc_formats::io::SliceSource(&bundle.bytes);
    let reader = obc_weather::WeatherReader::open(&source).expect("the device must be able to open it");
    assert_eq!(reader.header().frame_count, 2, "both frames of the timeline");
    assert_eq!(reader.header().request_id, 42);
    // Every object the fetch read was named by arithmetic, not read out of the document.
    for (url, _) in &http.ledger {
        assert!(
            url == MANIFEST_URL || url.starts_with(MET_ENDPOINT) || url.contains("/wx/v2/20260810T1430Z/f"),
            "unexpected read: {url}"
        );
    }
}

/// A corridor is not required to sit inside one object. Four shards meet in the middle of this
/// lattice, and the frame the device sees is assembled from all four — the case v1 could not have,
/// because a product was a single grid.
#[test]
fn a_corridor_straddling_the_shard_seams_is_assembled_from_every_shard_it_touches() {
    let now = 1_800_000_000;
    let grid = test_grid();
    let frames = dataset(now, &grid);
    let (mut http, mut client, corridor) = wired(&grid, &frames, now, now + 3_600);
    client.fetch(&mut http, &corridor, now, 1).expect("fetch");

    let mut shards_read = std::collections::BTreeSet::new();
    for (url, _) in &http.ledger {
        if let Some(name) = url.rsplit('/').next().filter(|name| name.ends_with(".obcg")) {
            shards_read.insert(name.to_string());
        }
    }
    assert_eq!(
        shards_read,
        ["s0-0.obcg", "s0-1.obcg", "s1-0.obcg", "s1-1.obcg"].map(str::to_string).into_iter().collect(),
        "the corridor covers the four-shard corner, so all four objects are read"
    );
}

/// **Missing is not dry, and dry is not missing.** A shard the baker measured as rain-free
/// publishes no object; the frame it belongs to is still a frame, and its cells are intensity 0.
/// Dropping it would put a hole in the timeline where the honest answer is "no rain here".
#[test]
fn a_dry_shard_is_painted_dry_and_the_frame_still_ships() {
    let now = 1_800_000_000;
    let grid = test_grid();
    // One frame, one published shard (the north-east one), three measured dry.
    let published = ShardId { col: 1, row: 1 };
    let bytes = shard_object(&grid, published, now, true, |_, _| 7);
    let frames = vec![FrameSpec { offset_min: 0, valid_at: now, objects: vec![(published, bytes, true)] }];
    let (mut http, mut client, corridor) = wired(&grid, &frames, now, now + 3_600);
    let bundle = client.fetch(&mut http, &corridor, now, 1).expect("fetch");

    assert_eq!(bundle.diagnostics.dry_shards, 3, "three shards measured dry, and no request made for them");
    assert_eq!(bundle.diagnostics.failed_frames, 0, "a dry shard is not a failure");
    let cells = frame_cells(&bundle.bytes, 0);
    assert!(cells.contains(&INTENSITY_DRY), "the dry shards must read as dry");
    assert!(cells.contains(&7), "…and the published one as its own value");
    assert!(!cells.contains(&INTENSITY_NODATA), "nothing in this corridor is unknown");
    // The quality flag is about *when* the frame is, never about what is in it.
    assert_eq!(
        frame_quality(&bundle.bytes, 0),
        obc_formats::obcw::QUALITY_OBSERVED,
        "a rain-free radar scan is still an observation"
    );
}

/// The rule the two clients disagreed about, stated as a test in both: the OBCW quality flag
/// follows the frame's **place in the timeline**, not its content and not the per-shard `observed`
/// bits. A frame that is dry everywhere is not thereby a forecast, and a forward frame is not
/// thereby an observation because a radar happened to paint one of its shards.
#[test]
fn the_quality_flag_follows_the_frames_place_in_the_timeline_not_its_content() {
    let now = 1_800_000_000;
    let grid = test_grid();
    let shard = ShardId { col: 1, row: 1 };
    // f0 entirely dry (no objects at all), f15 entirely radar-observed.
    let forward = shard_object(&grid, shard, now + 900, true, |_, _| 5);
    let frames = vec![
        FrameSpec { offset_min: 0, valid_at: now, objects: Vec::new() },
        FrameSpec { offset_min: 15, valid_at: now + 900, objects: vec![(shard, forward, true)] },
    ];
    let (mut http, mut client, corridor) = wired(&grid, &frames, now, now + 3_600);
    let bundle = client.fetch(&mut http, &corridor, now, 1).expect("fetch");
    assert_eq!(
        frame_quality(&bundle.bytes, 0),
        obc_formats::obcw::QUALITY_OBSERVED,
        "offset 0 inside the source skew is the analysis, dry or not"
    );
    assert_eq!(
        frame_quality(&bundle.bytes, 1),
        obc_formats::obcw::QUALITY_FORECAST,
        "a forward frame is a forecast however it was painted"
    );
    assert_eq!(bundle.diagnostics.observed_shards, 1, "the per-shard bits stay a counter");
}

/// **A failed shard is a hole in its frame, not the loss of the frame.** Dropping the frame would
/// throw away the shards that did arrive to punish the one that did not — and the hole cannot make
/// an outage look rain-free, because no-data is a different code from dry all the way down.
#[test]
fn one_failing_shard_leaves_a_hole_rather_than_dropping_the_frame() {
    let now = 1_800_000_000;
    let grid = test_grid();
    let frames = dataset(now, &grid);
    let (http, mut client, corridor) = wired(&grid, &frames, now, now + 3_600);
    let _ = http;
    // Serve everything except one shard of frame 0 — exactly the shape of a single lost object.
    let missing = corridor::join(ORIGIN, &grid.shard_key(0, ShardId { col: 0, row: 0 }));
    let mut holed = FixtureHttp::new()
        .with_object(MANIFEST_URL, manifest_json(&grid, &frames, now + 3_600, 60).into_bytes())
        .with_object(format!("{MET_ENDPOINT}?lat=47.3200&lon=7.3200"), fixture("met-freiburg-24h.json"));
    for frame in &frames {
        for (shard, bytes, _) in &frame.objects {
            let url = corridor::join(ORIGIN, &grid.shard_key(frame.offset_min, *shard));
            if url != missing {
                holed = holed.with_object(url, bytes.clone());
            }
        }
    }
    let bundle = client.fetch(&mut holed, &corridor, now, 1).expect("fetch");

    assert_eq!(bundle.diagnostics.failed_frames, 1, "one shard failed, and it is counted");
    assert_eq!(bundle.diagnostics.no_rain_map, None, "one hole is not a lost rain map");
    let source = obc_formats::io::SliceSource(&bundle.bytes);
    let reader = obc_weather::WeatherReader::open(&source).expect("valid");
    assert_eq!(reader.header().frame_count, 2, "both frames still ship");
    let cells = frame_cells(&bundle.bytes, 0);
    assert!(cells.contains(&INTENSITY_NODATA), "the lost shard's cells are unknown");
    assert!(cells.iter().any(|&cell| cell != INTENSITY_NODATA), "…and the shards that arrived are still there");
    assert_ne!(
        reader.frame(0).expect("frame").quality_flags & obc_formats::obcw::QUALITY_PARTIAL_COVERAGE,
        0,
        "a frame with a hole says so"
    );
}

/// Nothing failed and nothing is usable: every frame the generation publishes is outside the window
/// the rain map answers. Deleting `NoFramesInWindow` left that path wearing `FramesUnavailable`,
/// which would have said "failed" about a service that answered perfectly.
#[test]
fn a_timeline_entirely_outside_the_window_is_not_a_failure() {
    let now = 1_800_000_000;
    let grid = test_grid();
    let frames = dataset(now, &grid);
    let (mut http, mut client, corridor) = wired(&grid, &frames, now, now + 24 * 3_600);
    // Twelve hours later: the frames are far past the six-hour observation age, and the generation
    // is still inside its own (deliberately generous) staleness deadline.
    let bundle = client.fetch(&mut http, &corridor, now + 12 * 3_600, 1).expect("still a bundle");
    assert_eq!(bundle.diagnostics.no_rain_map, Some(NoRainMap::OutsideWindow));
    assert_eq!(bundle.diagnostics.failed_frames, 0, "nothing failed — that is the whole point");
    assert!(!http.ledger.iter().any(|(url, _)| url.ends_with(".obcg")), "and nothing was read");
}

/// A whole fetch at the date line, not just the clamp in isolation. The corridor is cut at ±180°,
/// so the window it states is narrower than a 90 km disc on one side — and the interesting part is
/// that everything downstream of that is unremarkable: the shard arithmetic, the Range reads and
/// the bundle window are the same code paths as at 47°N, which is exactly the claim worth pinning.
#[test]
fn a_fetch_at_the_date_line_reads_the_clamped_window_and_nothing_beyond_it() {
    let now = 1_800_000_000;
    // One degree of lattice ending exactly on the antimeridian.
    let grid = Grid {
        south_lat_udeg: 0,
        west_lon_udeg: 179_000_000,
        width: 100,
        height: 100,
        shard_width: 50,
        shard_height: 50,
        shard_cols: 2,
        shard_rows: 2,
        covered_rows: 0..100,
        ..test_grid()
    };
    let corridor = Corridor::around(500_000, 179_980_000, 8_000.0);
    assert!(corridor.clamped, "the disc really does run off the edge here");
    assert_eq!(corridor.bounds.east_udeg, 180_000_000);

    let mut objects = Vec::new();
    for row in 0..grid.shard_rows {
        for col in 0..grid.shard_cols {
            let shard = ShardId { col, row };
            let bytes = shard_object(&grid, shard, now, true, |c, r| ((c + r) % 13) as u8);
            objects.push((shard, bytes, true));
        }
    }
    let frames = vec![FrameSpec { offset_min: 0, valid_at: now, objects }];
    let document = manifest_json(&grid, &frames, now + 3_600, 60);
    let mut http = FixtureHttp::new()
        .with_object(MANIFEST_URL, document.into_bytes())
        .with_object(format!("{MET_ENDPOINT}?lat=0.5000&lon=179.9800"), fixture("met-freiburg-24h.json"));
    for (shard, bytes, _) in &frames[0].objects {
        http = http.with_object(corridor::join(ORIGIN, &grid.shard_key(0, *shard)), bytes.clone());
    }
    let mut client = WeatherClient::new(ORIGIN).with_met_endpoint(MET_ENDPOINT);
    let bundle = client.fetch(&mut http, &corridor, now, 1).expect("fetch");

    assert_eq!(bundle.diagnostics.no_rain_map, None, "a clamped corridor is still answerable");
    assert_eq!(bundle.diagnostics.failed_frames, 0);
    let source = obc_formats::io::SliceSource(&bundle.bytes);
    let header = obc_weather::WeatherReader::open(&source).expect("valid").header();
    assert!(header.east_lon_udeg <= 180_000_000, "the stated window must not cross the antimeridian");
    assert!(header.west_lon_udeg < header.east_lon_udeg, "…nor read as wrapped");
    // Short is not partial. Every cell of the window this bundle states has data, and §5.1's flag
    // is about *in-bounds* cells being unavailable — so a corridor cut at the date line loses
    // window, not certainty. Pinned in both languages because Swift raised the flag here.
    assert_eq!(
        frame_quality(&bundle.bytes, 0) & obc_formats::obcw::QUALITY_PARTIAL_COVERAGE,
        0,
        "a clamped window is smaller, not less certain"
    );
    assert!(!frame_cells(&bundle.bytes, 0).contains(&INTENSITY_NODATA));
}

/// The last column and row of the lattice are **short** shards, and `shard_geometry`'s clamp is what
/// decides whether every edge shard on the planet is accepted or refused: it gates `agrees_with`,
/// so a rounded-up width would make the fetched header contradict the derived geometry. Both fetch
/// suites otherwise use grids that divide exactly, which never exercises it.
#[test]
fn a_short_edge_shard_is_fetched_and_its_narrow_header_agrees() {
    let now = 1_800_000_000;
    // 70 x 70 cells in 32 x 32 shards: three columns and three rows, the last of each only 6 wide.
    let grid = Grid {
        width: 70,
        height: 70,
        shard_width: 32,
        shard_height: 32,
        shard_cols: 3,
        shard_rows: 3,
        covered_rows: 0..70,
        ..test_grid()
    };
    let edge = ShardId { col: 2, row: 2 };
    let geometry = grid.shard_geometry(edge).expect("the corner shard");
    assert_eq!((geometry.width, geometry.height), (6, 6), "the corner shard is short on both axes");

    let bytes = shard_object(&grid, edge, now, true, |_, _| 4);
    let frames = vec![FrameSpec { offset_min: 0, valid_at: now, objects: vec![(edge, bytes, true)] }];
    let document = manifest_json(&grid, &frames, now + 3_600, 60);
    let mut http = FixtureHttp::new()
        .with_object(MANIFEST_URL, document.into_bytes())
        .with_object(format!("{MET_ENDPOINT}?lat=47.6800&lon=7.6800"), fixture("met-freiburg-24h.json"));
    for (shard, object, _) in &frames[0].objects {
        http = http.with_object(corridor::join(ORIGIN, &grid.shard_key(0, *shard)), object.clone());
    }
    // A corridor over the far corner, where only the short shard has anything to say.
    let corridor = Corridor::around(47_680_000, 7_680_000, 8_000.0);
    let mut client = WeatherClient::new(ORIGIN).with_met_endpoint(MET_ENDPOINT);
    let bundle = client.fetch(&mut http, &corridor, now, 1).expect("fetch");

    assert_eq!(bundle.diagnostics.failed_frames, 0, "the narrow header must agree with the derived geometry");
    assert!(frame_cells(&bundle.bytes, 0).contains(&4), "the short shard's cells reached the bundle");
}

/// An object the manifest **promised** is an error when it does not arrive — a 404 is never an
/// absence of rain. Losing every one of them is `FramesUnavailable`, and the bundle carries no
/// frames at all rather than an invented dry one.
#[test]
fn a_promised_object_that_is_missing_is_an_error_and_never_a_dry_map() {
    let now = 1_800_000_000;
    let grid = test_grid();
    let frames = dataset(now, &grid);
    let (http, mut client, corridor) = wired(&grid, &frames, now, now + 3_600);
    // The manifest still names every shard; the origin has lost them.
    let mut starved = FixtureHttp::new()
        .with_object(MANIFEST_URL, manifest_json(&grid, &frames, now + 3_600, 60).into_bytes())
        .with_object(format!("{MET_ENDPOINT}?lat=47.3200&lon=7.3200"), fixture("met-freiburg-24h.json"));
    let _ = http;

    let bundle = client.fetch(&mut starved, &corridor, now, 1).expect("still a bundle");
    assert!(bundle.diagnostics.failed_frames > 0, "the promised objects were genuinely asked for");
    assert_eq!(bundle.diagnostics.no_rain_map, Some(NoRainMap::FramesUnavailable));
    let source = obc_formats::io::SliceSource(&bundle.bytes);
    let reader = obc_weather::WeatherReader::open(&source).expect("valid");
    assert_eq!(reader.header().frame_count, 0, "no frames — never a fabricated dry one");
}

/// Expiry is **no weather**, which is not no rain. Past `stale_after` the client stops planning
/// reads entirely and says so; the hourly half still ships.
#[test]
fn an_expired_generation_is_no_weather_and_not_a_dry_map() {
    let now = 1_800_000_000;
    let grid = test_grid();
    let frames = dataset(now, &grid);
    let (mut http, mut client, corridor) = wired(&grid, &frames, now, now + 600);
    let bundle = client.fetch(&mut http, &corridor, now + 1_200, 1).expect("still a bundle");
    assert_eq!(bundle.diagnostics.no_rain_map, Some(NoRainMap::Expired));
    let source = obc_formats::io::SliceSource(&bundle.bytes);
    assert_eq!(obc_weather::WeatherReader::open(&source).expect("valid").header().frame_count, 0);
    assert!(
        !http.ledger.iter().any(|(url, _)| url.ends_with(".obcg")),
        "an expired generation is not worth a single Range read"
    );
}

/// Off the lattice is geometry, not weather — and it is a different sentence from "no rain".
#[test]
fn a_corridor_off_the_lattice_is_out_of_domain() {
    let now = 1_800_000_000;
    let grid = test_grid();
    let frames = dataset(now, &grid);
    let (mut http, mut client, _) = wired(&grid, &frames, now, now + 3_600);
    let elsewhere = Corridor::around(-33_900_000, 151_200_000, 20_000.0); // Sydney; this lattice is Swiss
    let bundle = client.fetch(&mut http, &elsewhere, now, 1).expect("still a bundle");
    assert_eq!(bundle.diagnostics.no_rain_map, Some(NoRainMap::OutOfDomain));
}

/// On the lattice but outside `covered_rows`: objects exist and are intensity 15 in every frame,
/// forever. Fetching them would buy round trips and the word "unknown", so the plan says so
/// instead — and the rider is never told it is dry there.
#[test]
fn a_corridor_with_no_source_behind_it_is_uncovered_rather_than_dry() {
    let now = 1_800_000_000;
    let grid = Grid { covered_rows: 0..1, ..test_grid() };
    let frames = dataset(now, &grid);
    let (mut http, mut client, corridor) = wired(&grid, &frames, now, now + 3_600);
    let bundle = client.fetch(&mut http, &corridor, now, 1).expect("still a bundle");
    assert_eq!(bundle.diagnostics.no_rain_map, Some(NoRainMap::Uncovered));
    assert!(!http.ledger.iter().any(|(url, _)| url.ends_with(".obcg")));
}

/// Offline is a truthful hourly-only bundle with a stated reason. Never a blank screen, never a
/// fabricated map, and never "dry".
#[test]
fn offline_degrades_to_a_stated_hourly_only_bundle() {
    let now = 1_800_000_000;
    let grid = test_grid();
    let frames = dataset(now, &grid);
    let (http, mut client, corridor) = wired(&grid, &frames, now, now + 24 * 3_600);
    // MET answers from its own cache; only the OBC service is down. Prime the cache first.
    let mut warm = http.clone();
    client.fetch(&mut warm, &corridor, now, 1).expect("warm fetch");
    let mut offline = FaultyHttp::new(http, FailureControls { offline: true, ..FailureControls::default() });
    // A wider corridor at the same position: the crop cache misses (its key is the cell window), so
    // the failing reads are genuinely attempted, while MET's cache — keyed on the rounded
    // coordinate, which has not moved — still answers.
    let wider = Corridor::around(corridor.lat_udeg, corridor.lon_udeg, 26_000.0);
    let bundle = client.fetch(&mut offline, &wider, now + 3_600, 2).expect("still a bundle");
    assert!(bundle.diagnostics.no_rain_map.is_some(), "the reason must be stated, not inferred");
    let source = obc_formats::io::SliceSource(&bundle.bytes);
    let reader = obc_weather::WeatherReader::open(&source).expect("valid");
    assert_eq!(reader.header().frame_count, 0, "no rain data means no rain frames, not empty ones");
    assert_ne!(reader.header().valid_from, 0, "…while the cached hourly forecast still ships");
}

// ── the manifest's own deadlines ───────────────────────────────────────────────────────────

/// The client holds **no** freshness constant: the window is `freshness.manifest_max_age_s`, from
/// the document. A cadence change is a baker deploy, not a client release.
#[test]
fn the_manifest_caches_for_the_window_the_document_states() {
    let now = 1_800_000_000;
    let grid = test_grid();
    let frames = dataset(now, &grid);
    let document = manifest_json(&grid, &frames, now + 3_600, 300);
    let mut http = FixtureHttp::new().with_object(MANIFEST_URL, document.into_bytes());
    let mut client = WeatherClient::new(ORIGIN);
    client.manifest(&mut http, now).expect("first");
    let after_first = http.ledger.len();
    client.manifest(&mut http, now + 299).expect("inside the stated window");
    assert_eq!(http.ledger.len(), after_first, "300 s is what this document asked for, not 60");
    client.manifest(&mut http, now + 301).expect("past it");
    assert_eq!(http.ledger.len(), after_first + 1);
}

/// Revalidation with the stored ETag, and a `304` that **restarts** the window rather than leaving
/// the client asking again a second later.
#[test]
fn the_manifest_revalidates_with_its_etag_and_a_304_restarts_the_window() {
    let now = 1_800_000_000;
    let grid = test_grid();
    let frames = dataset(now, &grid);
    let document = manifest_json(&grid, &frames, now + 3_600, 60);
    let mut http = FixtureHttp::new().with_object(MANIFEST_URL, document.into_bytes()).with_headers(
        MANIFEST_URL,
        Some("\"bake-42\""),
        None,
        None,
    );
    let mut client = WeatherClient::new(ORIGIN);

    client.manifest(&mut http, now).expect("first fetch");
    let after_first = http.ledger.len();
    client.manifest(&mut http, now + 30).expect("inside the window");
    assert_eq!(http.ledger.len(), after_first, "no request at all inside the window");
    client.manifest(&mut http, now + 61).expect("revalidated");
    assert_eq!(http.ledger.len(), after_first + 1);
    client.manifest(&mut http, now + 100).expect("inside the restarted window");
    assert_eq!(http.ledger.len(), after_first + 1, "a 304 must restart the freshness window");
}

// ── the privacy contract ───────────────────────────────────────────────────────────────────

/// **The epic's headline invariant, as an assertion.** Every request to the OBC service is a
/// key-addressed read of an immutable object: no query string, no coordinate, nothing derived from
/// one. The corridor decides *which* objects — and that is all the service ever learns. MET is the
/// single third party that receives a position, and it receives it rounded to four decimals.
#[test]
fn the_service_never_receives_a_coordinate() {
    let now = 1_800_000_000;
    let grid = test_grid();
    let frames = dataset(now, &grid);
    let (mut http, mut client, corridor) = wired(&grid, &frames, now, now + 3_600);
    client.fetch(&mut http, &corridor, now, 7).expect("fetch");

    let mut met_seen = 0;
    for (url, _) in &http.ledger {
        if url.starts_with(MET_ENDPOINT) {
            met_seen += 1;
            continue;
        }
        assert!(url.starts_with(ORIGIN), "an OBC request must address the service origin: {url}");
        assert!(!url.contains('?'), "an OBC request must carry no query string at all: {url}");
        for forbidden in ["lat", "lon", "coord", "="] {
            assert!(!url.contains(forbidden), "{url} contains {forbidden:?} — the service must learn no position");
        }
        // Not even the digits: a corridor edge smuggled into a key would defeat the whole design.
        for udeg in [corridor.lat_udeg, corridor.lon_udeg, corridor.bounds.south_udeg as i32] {
            let digits = udeg.abs().to_string();
            assert!(!url.contains(&digits), "{url} contains the coordinate {digits}");
        }
    }
    assert_eq!(met_seen, 1, "exactly one request carries the rider's position, and it goes to MET");
}

// ── the frame cache ────────────────────────────────────────────────────────────────────────

/// Shard objects are immutable by the publishing contract, so a corridor already cropped out of one
/// is knowledge, not a guess. Re-reading it would be bytes spent to learn what the client knows.
#[test]
fn an_immutable_shard_is_cropped_once_and_then_served_from_the_cache() {
    let now = 1_800_000_000;
    let grid = test_grid();
    let frames = dataset(now, &grid);
    let (mut http, mut client, corridor) = wired(&grid, &frames, now, now + 3_600);
    let first = client.fetch(&mut http, &corridor, now, 1).expect("first fetch");
    assert_eq!(first.diagnostics.cached_frames, 0, "the first fetch has nothing to reuse");
    let object_reads = |http: &FixtureHttp| http.ledger.iter().filter(|(url, _)| url.ends_with(".obcg")).count();
    let after_first = object_reads(&http);
    assert!(after_first > 4, "the first fetch really did read headers, pages and tiles");

    let second = client.fetch(&mut http, &corridor, now + 1, 2).expect("second fetch");
    assert_eq!(object_reads(&http), after_first, "an immutable object must not be fetched twice");
    assert_eq!(second.diagnostics.cached_frames, 8, "four shards x two frames, all from the cache");
    assert_eq!(second.diagnostics.service_requests, 0, "nothing at all was needed from the service");
}

// ── corridor extraction ────────────────────────────────────────────────────────────────────

fn multipage_setup() -> (FixtureHttp, ShardRead, Bbox) {
    // A 64 × 64-cell shard at 16-cell tiles and four entries per page: 16 tiles across 4 directory
    // pages, so "only the covering pages" is a claim with something to prove.
    let grid = Grid { width: 128, height: 128, shard_width: 64, shard_height: 64, ..test_grid() };
    let shard = ShardId { col: 0, row: 0 };
    let bytes = shard_object(&grid, shard, 1_800_000_000, true, |col, row| ((col * 7 + row * 3) % 13) as u8);
    let header = obcg::decode_header(bytes[..obcg::HEADER_LEN].try_into().unwrap()).expect("header");
    let read = ShardRead {
        key: grid.shard_key(0, shard),
        geometry: grid.shard_geometry(shard).expect("geometry"),
        bytes: bytes.len() as u64,
        object_crc32: header.object_crc32,
        valid_at: header.valid_at,
        observed: true,
    };
    // Cells (18,18)…(27,27): inside tile (1,1), which is entry 5 — one tile, on one page.
    let bounds =
        Bbox { south_udeg: 47_180_000, north_udeg: 47_280_000 - 1, west_udeg: 7_180_000, east_udeg: 7_280_000 - 1 };
    let http = FixtureHttp::new().with_object(corridor::join(ORIGIN, &read.key), bytes);
    (http, read, bounds)
}

/// The frozen §7 read pattern: the header, the directory pages arithmetic says cover the corridor,
/// and only the non-dry tiles those pages name. Nothing else — a corridor consumer that quietly
/// downloaded the object would still pass a cell-value test, so the *ledger* is the test.
#[test]
fn corridor_extraction_reads_only_the_header_covering_pages_and_needed_tiles() {
    let (mut http, read, bounds) = multipage_setup();
    let object = http.ledger.len(); // zero; the real size comes below
    let _ = object;
    let crop = corridor::crop_frame(&mut http, ORIGIN, &read, &bounds).expect("crop");
    assert_eq!((crop.width, crop.height), (10, 10));

    let header_reads = http.ledger.iter().filter(|(_, range)| *range == Some((0, 127))).count();
    assert_eq!(header_reads, 1, "exactly one header read");
    let object = read.bytes;
    for (_, range) in &http.ledger {
        let (_start, end) = range.expect("every corridor read is a Range read");
        assert!(end < object, "a corridor read must never run past the object");
    }
    assert!(
        http.fetched_bytes() < object,
        "corridor extraction moved {} of {object} bytes — it must never need the whole object",
        http.fetched_bytes()
    );
}

/// The manifest is a plan; the header is the truth. A manifest that re-stamped a frame to look
/// current, or that mis-states the lattice, is caught before a single cell is trusted.
#[test]
fn a_manifest_that_disagrees_with_the_header_refuses_the_shard() {
    for mutate in [
        (|read: &mut ShardRead| read.valid_at += 60) as fn(&mut ShardRead),
        |read: &mut ShardRead| read.object_crc32 ^= 1,
        |read: &mut ShardRead| read.bytes += 1,
        |read: &mut ShardRead| read.geometry.width += 1,
    ] {
        let (mut http, mut read, bounds) = multipage_setup();
        mutate(&mut read);
        assert!(
            corridor::crop_frame(&mut http, ORIGIN, &read, &bounds).is_err(),
            "a shard whose header contradicts the manifest must be refused"
        );
    }
}

/// A flipped bit anywhere in a fetched page or tile is caught by the production CRCs — the
/// simulator's corrupt-tile control is a transport fault, not a second validation path.
#[test]
fn a_corrupted_fetch_is_caught_by_the_production_crcs() {
    let (http, read, bounds) = multipage_setup();
    let reads = {
        let (mut http, read, bounds) = multipage_setup();
        let _ = corridor::crop_frame(&mut http, ORIGIN, &read, &bounds);
        http.ledger.len()
    };
    for index in 0..reads as u32 {
        let mut faulty = FaultyHttp::new(
            http.clone(),
            FailureControls { corrupt_request: Some(index), ..FailureControls::default() },
        );
        assert!(
            corridor::crop_frame(&mut faulty, ORIGIN, &read, &bounds).is_err(),
            "corrupting request {index} must be caught, not decoded into weather"
        );
    }
}

#[test]
fn a_truncated_fetch_is_refused() {
    let (http, read, bounds) = multipage_setup();
    let mut faulty = FaultyHttp::new(http, FailureControls { truncate_request: Some(0), ..FailureControls::default() });
    assert!(corridor::crop_frame(&mut faulty, ORIGIN, &read, &bounds).is_err());
}

/// A server that ignores `Range` and streams the whole object is answering lawfully; the client
/// slices it itself rather than reading the head of a file as if it were the middle. The proof is
/// that the crop is **identical** to the one an honest 206 origin produces.
#[test]
fn a_200_to_a_range_request_is_sliced_and_produces_the_same_crop() {
    let (mut honest, read, bounds) = multipage_setup();
    let expected = corridor::crop_frame(&mut honest, ORIGIN, &read, &bounds).expect("crop over 206");
    let (whole_object, read, bounds) = multipage_setup();
    let mut whole_object = whole_object.ignoring_ranges();
    let sliced = corridor::crop_frame(&mut whole_object, ORIGIN, &read, &bounds).expect("crop over 200");
    assert_eq!(sliced, expected, "the same bytes must decode to the same crop, whatever status carried them");
}

/// An origin that lies about partial content: `206` with more bytes than were asked for, or a
/// `Content-Range` naming other bytes. Both are refusals.
#[test]
fn a_206_that_does_not_match_the_request_is_refused() {
    use obc_wx_client::http::{Http, HttpError, Request, Response};

    struct Lying {
        object: Vec<u8>,
        honest_content_range: bool,
    }
    impl Http for Lying {
        fn perform(&mut self, request: &Request, _cap: u64) -> Result<Response, HttpError> {
            let (start, end) = request.range.expect("a corridor read is always a Range read");
            Ok(Response {
                status: 206,
                body: self.object.clone(), // the whole object, under a partial-content status
                content_range: Some(if self.honest_content_range {
                    format!("bytes 0-{}/{}", self.object.len() - 1, self.object.len())
                } else {
                    format!("bytes {start}-{end}/{}", self.object.len())
                }),
                ..Response::empty()
            })
        }
    }

    let grid = test_grid();
    let bytes =
        shard_object(&grid, ShardId { col: 0, row: 0 }, 1_800_000_000, true, |col, row| ((col + row) % 13) as u8);
    let (_, read, bounds) = multipage_setup();
    for honest_content_range in [true, false] {
        let mut http = Lying { object: bytes.clone(), honest_content_range };
        let error = corridor::crop_frame(&mut http, ORIGIN, &read, &bounds).expect_err("must be refused");
        assert!(
            matches!(error, corridor::CropError::Http(HttpError::RangeNotHonoured(_))),
            "an over-long 206 is a range that was not honoured, not something to slice: {error:?}"
        );
    }
}

/// 2xx is not a licence. Only `200` and `206` describe the bytes a corridor read asked for.
#[test]
fn only_200_and_206_are_acted_on() {
    use obc_wx_client::http::{Http, HttpError, Request, Response};

    struct Status(u16);
    impl Http for Status {
        fn perform(&mut self, _request: &Request, _cap: u64) -> Result<Response, HttpError> {
            Ok(Response { status: self.0, ..Response::empty() })
        }
    }
    let (_, read, bounds) = multipage_setup();
    for status in [201u16, 202, 203, 204, 205, 226] {
        let mut http = Status(status);
        let error = corridor::crop_frame(&mut http, ORIGIN, &read, &bounds).expect_err("must be refused");
        assert!(
            matches!(error, corridor::CropError::Http(HttpError::Status { code, .. }) if code == status),
            "status {status} must be refused as a status, not decoded"
        );
    }
}

// ── MET ────────────────────────────────────────────────────────────────────────────────────

#[test]
fn the_captured_met_document_decodes_to_24_consecutive_hours() {
    let hourly = met::decode(&fixture("met-freiburg-24h.json"), 0).expect("MET decode");
    for (index, record) in hourly.records.iter().enumerate() {
        assert_eq!(record.valid_time_offset_s, index as u32 * 3600);
        assert_ne!(record.temperature_deci_c, obc_formats::obcw::TEMP_UNAVAILABLE);
        assert_ne!(record.condition, obc_formats::obcw::CONDITION_UNAVAILABLE);
        assert!(record.wind_from_deg < 360);
    }
}

/// Freiburg supplies neither optional field. They must read *unavailable*, because a zero here
/// would be a forecast of "no chance of rain" that MET never made.
#[test]
fn absent_optional_fields_are_unavailable_and_never_zero() {
    let hourly = met::decode(&fixture("met-freiburg-24h.json"), 0).expect("MET decode");
    assert!(hourly
        .records
        .iter()
        .all(|record| record.precipitation_probability_pct == obc_formats::obcw::PROBABILITY_UNAVAILABLE));
    assert!(hourly.records.iter().all(|record| record.wind_gust_deci_ms == obc_formats::obcw::WIND_SPEED_UNAVAILABLE));
}

/// Present-but-wrong is malformed. Silently downgrading a bad value to "unavailable" would hide a
/// broken provider behind a plausible screen.
#[test]
fn a_present_but_invalid_optional_field_is_malformed() {
    let mut document: serde_json::Value = serde_json::from_slice(&fixture("met-freiburg-24h.json")).unwrap();
    document["properties"]["timeseries"][3]["data"]["next_1_hours"]["details"]["probability_of_precipitation"] =
        serde_json::json!(140.0);
    assert!(met::decode(document.to_string().as_bytes(), 0).is_err());

    let mut document: serde_json::Value = serde_json::from_slice(&fixture("met-freiburg-24h.json")).unwrap();
    document["properties"]["meta"]["units"]["air_temperature"] = serde_json::json!("fahrenheit");
    assert!(met::decode(document.to_string().as_bytes(), 0).is_err(), "a unit change is not something to convert");
}

/// The frozen WX1 table, including the order that makes thunder beat every precipitation family
/// and an unknown code become a truthful gap rather than a guess.
#[test]
fn the_symbol_table_is_the_frozen_wx1_mapping() {
    use obc_formats::obcw::*;
    let cases = [
        ("clearsky_day", CONDITION_CLEAR),
        ("fair_polartwilight", CONDITION_MOSTLY_CLEAR),
        ("partlycloudy_night", CONDITION_PARTLY_CLOUDY),
        ("cloudy", CONDITION_OVERCAST),
        ("fog", CONDITION_FOG),
        ("lightrain", CONDITION_DRIZZLE),
        ("heavyrain", CONDITION_RAIN),
        ("lightrainshowers_day", CONDITION_SHOWERS),
        ("sleetshowers_night", CONDITION_SLEET),
        ("heavysnow", CONDITION_SNOW),
        ("rainshowersandthunder_day", CONDITION_THUNDERSTORM),
        ("heavysleetandthunder", CONDITION_THUNDERSTORM),
        ("something_new_met_invented", CONDITION_UNAVAILABLE),
    ];
    for (symbol, expected) in cases {
        assert_eq!(met::condition_for(symbol), Some(expected), "{symbol}");
    }
    assert_eq!(met::condition_for("  "), None, "an empty code is a broken document, not a gap");
}

/// `Expires` is absolute: inside it MET is not contacted at all. That rule is MET's terms, and the
/// cache *is* the throttle.
#[test]
fn met_is_not_contacted_again_inside_its_expires_window() {
    let now = hourly().valid_from;
    let http_date = |unix: i64| {
        chrono::DateTime::<chrono::Utc>::from_timestamp(unix, 0)
            .unwrap()
            .format("%a, %d %b %Y %H:%M:%S GMT")
            .to_string()
    };
    let url = format!("{MET_ENDPOINT}?lat=48.0600&lon=7.9000");
    let mut http = FixtureHttp::new().with_object(url.clone(), fixture("met-freiburg-24h.json")).with_headers(
        url,
        None,
        Some(&http_date(now)),
        Some(&http_date(now + 1_800)),
    );
    let mut client = met::MetClient::new().with_endpoint(MET_ENDPOINT);
    client.hourly(&mut http, 48_060_000, 7_900_000, now).expect("first fetch");
    let after_first = http.ledger.len();
    client.hourly(&mut http, 48_060_000, 7_900_000, now + 60).expect("second call");
    assert_eq!(http.ledger.len(), after_first, "a second call inside Expires must issue no request at all");
    client.hourly(&mut http, 48_060_000, 7_900_000, now + 1_801).expect("revalidation");
    assert_eq!(http.ledger.len(), after_first + 1);
}

/// Four decimals is simultaneously the privacy contract (~11 m) and the refetch threshold.
#[test]
fn the_met_url_rounds_the_coordinate_to_four_decimals() {
    let client = met::MetClient::new().with_endpoint(MET_ENDPOINT);
    assert_eq!(client.url(47_123_456, -7_987_654), format!("{MET_ENDPOINT}?lat=47.1235&lon=-7.9877"));
    assert_eq!(client.url(47_123_456, -7_987_654), client.url(47_123_460, -7_987_650));
}

// ── the corridor ───────────────────────────────────────────────────────────────────────────

fn span_km(corridor: &Corridor) -> (f64, f64) {
    let lat = (corridor.bounds.north_udeg - corridor.bounds.south_udeg) as f64 / 1e6 * 111.32;
    let cos = (f64::from(corridor.lat_udeg) / 1e6).to_radians().cos();
    let lon = (corridor.bounds.east_udeg - corridor.bounds.west_udeg) as f64 / 1e6 * 111.32 * cos;
    (lon, lat)
}

/// One shape, one size, whatever the rider is doing: a 90 km disc. There is no heading to project
/// along and nothing a projection could change, because there is one dataset.
#[test]
fn the_corridor_is_a_ninety_kilometre_disc_at_every_latitude() {
    for lat_deg in [0, 41, 50, 60, 64, 69] {
        let corridor = Corridor::for_rider(lat_deg * 1_000_000, 7_900_000);
        let (width, height) = span_km(&corridor);
        assert!((height - 180.0).abs() < 1.0, "{lat_deg}: 90 km each way north-south, got {height}");
        assert!((width - 180.0).abs() < 1.0, "{lat_deg}: 90 km each way east-west, got {width}");
        assert!(!corridor.clamped);
    }
}

/// The two owed clamps (#1244). A disc at the date line is cut there rather than wrapped, because
/// OBCW §1 forbids a bundle window crossing ±180°; a disc at a pole is cut at ±90°. Both are
/// reported, and the polar case then reads as `Uncovered` rather than as an illegal window.
#[test]
fn the_disc_is_clamped_at_the_antimeridian_and_at_the_poles() {
    let dateline = Corridor::for_rider(0, 179_900_000);
    assert!(dateline.clamped);
    assert_eq!(dateline.bounds.east_udeg, 180_000_000, "cut at the date line, never wrapped into a v1 window");
    assert!(dateline.bounds.west_udeg < dateline.bounds.east_udeg, "the bundle window must not read as wrapped");
    dateline.bounds.validate().expect("a clamped disc is still a legal window");

    let pole = Corridor::for_rider(89_900_000, 0);
    assert!(pole.clamped);
    assert_eq!(pole.bounds.north_udeg, 90_000_000);
    pole.bounds.validate().expect("a clamped disc is still a legal window");
}

/// The reader keeps full antimeridian support even though the corridor declines to ask for it: the
/// wrap lives one layer up, where the shared fixture pins it against Swift.
#[test]
fn a_wrapped_window_is_still_answerable_by_the_grid() {
    let grid = Grid { west_lon_udeg: -180_000_000, width: 36_000, shard_width: 6_000, shard_cols: 6, ..test_grid() };
    let wrapped =
        Bbox { south_udeg: 47_100_000, north_udeg: 47_200_000, west_udeg: 179_000_000, east_udeg: -179_000_000 };
    let shards = grid.shards_for(&wrapped).expect("a legal wrapped window");
    let cols: Vec<u32> = shards.iter().map(|shard| shard.col).collect();
    assert!(cols.contains(&5) && cols.contains(&0), "both sides of the seam: {cols:?}");
}

// ── the bundle: the uniform east-west resample ─────────────────────────────────────────────

/// The canonical lattice, for the bundle-level tests: the real thing, since nothing is fetched.
fn canonical() -> Lattice {
    Lattice {
        south_udeg: -90_000_000,
        west_udeg: -180_000_000,
        cell_udeg: i64::from(CELL_UDEG),
        width: 36_000,
        height: 18_000,
        cell_size_m: CELL_SIZE_M,
    }
}

/// One crop covering a whole corridor window on the canonical lattice, with deliberately
/// incompressible cells — a uniform field would RLE4 down to nothing and the byte budget would pass
/// without measuring anything.
fn worst_case_crop(corridor: &Bbox, valid_at: i64, seed: u32) -> Crop {
    let cell = i64::from(CELL_UDEG);
    let south = corridor.south_udeg.div_euclid(cell) * cell;
    let west = corridor.west_udeg.div_euclid(cell) * cell;
    let height = ((corridor.north_udeg - south).div_euclid(cell) + 1) as u32;
    let width = ((corridor.east_udeg - west).div_euclid(cell) + 1) as u32;
    let cells = (0..width * height).map(|index| ((index * 7 + seed) % 13) as u8).collect();
    Crop {
        valid_at,
        observed: seed == 0,
        south_udeg: south,
        west_udeg: west,
        cell_lat_udeg: CELL_UDEG,
        cell_lon_udeg: CELL_UDEG,
        cell_size_m: CELL_SIZE_M,
        width,
        height,
        cells,
        partial: false,
    }
}

/// FNV-1a 64 over a byte sequence — the cell-image hash `resample_equivalence` pins.
///
/// Chosen because it is four lines in any language: the whole point of the row is that Swift
/// computes the same number over the same cells, and a hash needing a library would have been a
/// hash one of the two suites quietly skipped.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The shared vector directory, which carries `resample_equivalence` and `rejection_equivalence`.
fn shared_manifest() -> serde_json::Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../specs/vectors/manifest.json");
    serde_json::from_slice(&std::fs::read(path).expect("specs/vectors/manifest.json")).expect("json")
}

/// **The two-sided normalisation guard (#1254 phase 4a-norm), driven by the shared vector.**
///
/// Bytes alone are not enough, twice over. The rejected `round(1/cos φ)` max-merge also fit a byte
/// budget, and what killed it was the *mechanism* — 1,428 m cells at Frankfurt and a 2x step across
/// 48.19°N — so the guard is two-sided: the bundle fits a 200 KiB budget **and** the cells are
/// square. And at the raw4 worst case every tile is 128 bytes whatever is in it, so a half-cell
/// drift in the nearest-neighbour column map would move which cells the rider sees while every
/// length still matched: hence `frame0_cells_fnv1a64`, over the image the **device's** reader
/// decodes. Every number here comes out of `specs/vectors/manifest.json`, which is the same file the
/// Swift sweep reads — so the two clients agreeing is a checked fact rather than a coincidence
/// two suites happen to share.
#[test]
fn the_resampled_corridor_matches_the_shared_vector_at_every_latitude() {
    const BUDGET: usize = 200 * 1024;
    let hourly = hourly();
    let shared = shared_manifest();
    let rows = shared["wx_manifest_v2"]["resample_equivalence"]["rows"].as_array().expect("rows").clone();
    assert!(rows.len() >= 9, "the sweep must keep its latitudes");
    for row in rows {
        let lat_deg = row["lat_deg"].as_f64().expect("lat_deg");
        let lat_udeg = (lat_deg * 1e6) as i32;
        let corridor = Corridor::for_rider(lat_udeg, 7_900_000);
        let frames: Vec<FrameInput> = (0..9)
            .map(|index| FrameInput {
                valid_at: hourly.valid_from + index * 900,
                observed: index == 0,
                crops: vec![worst_case_crop(&corridor.bounds, hourly.valid_from + index * 900, index as u32)],
                dry: Vec::new(),
            })
            .collect();
        let (bytes, report) = bundle::build(
            1,
            1,
            hourly.valid_from,
            (lat_udeg, 7_900_000),
            &corridor.bounds,
            Some(Scene { lattice: canonical(), frames: &frames }),
            &hourly,
        )
        .expect("build");

        assert_eq!(report.frames, 9, "{lat_deg}: every frame of the timeline survives");
        assert_eq!(report.shrinks, 0, "{lat_deg}: the resample must make shrinking unnecessary");
        assert!(
            bytes.len() <= BUDGET,
            "{lat_deg}: {} bytes over the {BUDGET}-byte guard (cap is {})",
            bytes.len(),
            bundle::PRODUCER_CAP
        );
        assert_eq!(u64::from(report.source_columns), row["source_columns"].as_u64().unwrap(), "{lat_deg}: columns");
        let window = row["window"].as_array().expect("window");
        assert_eq!(u64::from(report.window_width), window[0].as_u64().unwrap(), "{lat_deg}: output width");
        assert_eq!(u64::from(report.window_height), window[1].as_u64().unwrap(), "{lat_deg}: output height");
        assert_eq!(bytes.len() as u64, row["bundle_bytes"].as_u64().unwrap(), "{lat_deg}: bundle length");
        let hash = format!("{:016x}", fnv1a64(&frame_cells(&bytes, 0)));
        assert_eq!(hash, row["frame0_cells_fnv1a64"].as_str().unwrap(), "{lat_deg}: the decoded cell image moved");

        // The pitch, read back out of the *bundle* rather than out of the builder's intention.
        let source = obc_formats::io::SliceSource(&bytes);
        let reader = obc_weather::WeatherReader::open(&source).expect("the device must open it");
        let header = reader.header();
        let frame = reader.frame(0).expect("frame");
        let cos = lat_deg.to_radians().cos();
        let east_west = f64::from(header.east_lon_udeg - header.west_lon_udeg) / 1e6 * METRES_PER_DEGREE_LAT * cos
            / f64::from(frame.width);
        let north_south = f64::from(header.north_lat_udeg - header.south_lat_udeg) / 1e6 * METRES_PER_DEGREE_LAT
            / f64::from(frame.height);
        assert!(
            (east_west - north_south).abs() / north_south < 0.02,
            "{lat_deg}: cells are {east_west:.0} m x {north_south:.0} m — more than 2 % from square"
        );
        assert_eq!(frame.cell_size_m, CELL_SIZE_M, "{lat_deg}: the frame states the lattice's own cell size");
        // Printed, not just asserted: `cargo test -- --nocapture` is where the epic's re-derived
        // budget table comes from, and a number nobody can read is a number nobody re-checks.
        println!(
            "{lat_deg:>6.2} N  {:>3} x {:<3} cells (from {:>3} lattice columns)  {:>6} B  E-W {east_west:.0} m / N-S {north_south:.0} m  {hash}",
            frame.width, frame.height, report.source_columns, bytes.len()
        );
    }
}

/// At the equator a 0.01° column is already ~1,113 m, so the resample is the **identity** — no
/// column is dropped and no cell moves. Anywhere else it decimates, and never interpolates.
#[test]
fn the_resample_is_the_identity_at_the_equator_and_only_ever_decimates() {
    let hourly = hourly();
    let mut previous = u32::MAX;
    for lat_deg in [0.0, 30.0, 50.0, 70.0] {
        let lat_udeg = (lat_deg * 1e6) as i32;
        let corridor = Corridor::for_rider(lat_udeg, 0);
        let frames = [FrameInput {
            valid_at: hourly.valid_from,
            observed: true,
            crops: vec![worst_case_crop(&corridor.bounds, hourly.valid_from, 0)],
            dry: Vec::new(),
        }];
        let (_, report) = bundle::build(
            1,
            1,
            hourly.valid_from,
            (lat_udeg, 0),
            &corridor.bounds,
            Some(Scene { lattice: canonical(), frames: &frames }),
            &hourly,
        )
        .expect("build");
        if lat_deg == 0.0 {
            assert_eq!(report.window_width, report.source_columns, "the equator is the identity map");
        } else {
            assert!(report.window_width < report.source_columns, "{lat_deg}: the map must decimate");
        }
        assert!(report.source_columns >= previous || previous == u32::MAX, "source columns grow with latitude");
        previous = report.source_columns;
    }
}

/// A built bundle opens through the production reader, and the shrink backstop is still wired: an
/// absurd corridor gives ground on its window before it will drop a timestamp.
#[test]
fn an_oversized_corridor_shrinks_its_window_before_dropping_a_frame() {
    let hourly = hourly();
    // Four times the real corridor: past the cap even after the resample, so the backstop fires.
    let corridor = Corridor::around(47_000_000, 7_000_000, CORRIDOR_RADIUS_M * 4.0);
    let frames: Vec<FrameInput> = (0..9)
        .map(|index| FrameInput {
            valid_at: hourly.valid_from + index * 900,
            observed: index == 0,
            crops: vec![worst_case_crop(&corridor.bounds, hourly.valid_from + index * 900, index as u32)],
            dry: Vec::new(),
        })
        .collect();
    let (bytes, report) = bundle::build(
        1,
        1,
        hourly.valid_from,
        (47_000_000, 7_000_000),
        &corridor.bounds,
        Some(Scene { lattice: canonical(), frames: &frames }),
        &hourly,
    )
    .expect("build");
    assert!(bytes.len() <= bundle::PRODUCER_CAP);
    assert!(report.shrinks > 0, "the window gives ground first");
    assert_eq!(report.dropped_oversize, 0, "every timestamp survives");
    assert_eq!(report.frames, 9);
    assert!(obc_weather::WeatherReader::open(&obc_formats::io::SliceSource(&bytes)).is_ok());
}

/// No rain data at all still produces a bundle, and it declares the **corridor** it answered rather
/// than an invented degree around the rider: the screens then say *hourly only here* over the
/// region the question was about.
#[test]
fn an_hourly_only_bundle_declares_the_corridor_it_answered() {
    let hourly = hourly();
    let corridor = Bbox { south_udeg: 47_950_000, west_udeg: 7_850_000, north_udeg: 48_170_000, east_udeg: 7_950_000 };
    let bytes =
        bundle::hourly_only(1, 1, hourly.valid_from, (48_060_000, 7_900_000), &corridor, &hourly).expect("build");
    let source = obc_formats::io::SliceSource(&bytes);
    let reader = obc_weather::WeatherReader::open(&source).expect("valid");
    let header = reader.header();
    assert_eq!(i64::from(header.south_lat_udeg), corridor.south_udeg);
    assert_eq!(i64::from(header.west_lon_udeg), corridor.west_udeg);
    assert_eq!(i64::from(header.north_lat_udeg), corridor.north_udeg);
    assert_eq!(i64::from(header.east_lon_udeg), corridor.east_udeg);
    assert_eq!(header.frame_count, 0);
}

// ── helpers ────────────────────────────────────────────────────────────────────────────────

/// One frame's known quality bits, as the **device** reads them.
fn frame_quality(bytes: &[u8], index: usize) -> u32 {
    let source = obc_formats::io::SliceSource(bytes);
    let reader = obc_weather::WeatherReader::open(&source).expect("valid");
    reader.frame(index).expect("frame").quality_flags & obc_formats::obcw::QUALITY_KNOWN_MASK
}

/// One frame of a bundle, decoded through the **device's** reader into a flat cell grid.
fn frame_cells(bytes: &[u8], index: usize) -> Vec<u8> {
    let source = obc_formats::io::SliceSource(bytes);
    let reader = obc_weather::WeatherReader::open(&source).expect("valid");
    let frame = reader.frame(index).expect("frame");
    let edge = obc_formats::precip4::TILE_EDGE as u32;
    let (width, height) = (u32::from(frame.width), u32::from(frame.height));
    let tile_cols = width.div_ceil(edge);
    let mut tile = [0u8; obc_formats::precip4::TILE_CELLS];
    let mut grid = vec![INTENSITY_NODATA; (width * height) as usize];
    for tile_index in 0..frame.tile_count {
        reader.decode_tile(index, tile_index, &mut tile).expect("tile");
        let (tile_col, tile_row) = (tile_index % tile_cols, tile_index / tile_cols);
        for local_row in 0..edge {
            for local_col in 0..edge {
                let (col, row) = (tile_col * edge + local_col, tile_row * edge + local_row);
                if col < width && row < height {
                    grid[(row * width + col) as usize] = tile[(local_row * edge + local_col) as usize];
                }
            }
        }
    }
    grid
}

/// The manifest module's own name for the document, so a key change is a compile error here too.
#[test]
fn the_client_reads_the_v2_manifest_key() {
    assert_eq!(manifest_v2::MANIFEST_KEY, "wx/v2/manifest.json");
}
