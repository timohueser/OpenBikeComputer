//! Deterministic fixture cycles for the WX6 products: the composed US timeline (MRMS observation
//! + HRRR forward frames, heterogeneous geometry in one product) and the worldwide GFS floor.
//!
//! Same checked-in upstream bytes ⇒ byte-identical published tree; corrupt upstream ⇒ the cycle
//! fails loudly and publishes nothing; unchanged upstream ⇒ no frame bytes move. Every published
//! cell is proven equal to the quantized nearest-neighbour source cell against independently
//! decoded upstream bytes — no smoothing, no invented cadence.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use obc_formats::obcg::{self, FLAG_FORECAST, FLAG_OBSERVED, PRODUCT_GFS, PRODUCT_HRRR, PRODUCT_MRMS};
use obc_formats::precip4;
use obc_wx_bake::cycle::{run_cycle, ProductStatus};
use obc_wx_bake::fetch::FixtureUpstream;
use obc_wx_bake::grib::{
    decode_field, decode_gzip_field, ExpectedGrib, GFS_GLOBAL_GRID_DEFINITION_HEX, HRRR_CONUS_GRID_DEFINITION_HEX,
    MRMS_CONUS_GRID_DEFINITION_HEX,
};
use obc_wx_bake::lcc;
use obc_wx_bake::manifest::{self, SourceClass};
use obc_wx_bake::publish::DirStore;
use obc_wx_bake::source::{gfs, hrrr, mrms, us, Adapter};

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
    std::fs::read(&path).unwrap_or_else(|error| panic!("fixture {name}: {error}"))
}

fn ts(text: &str) -> i64 {
    manifest::parse_rfc3339(text).expect("test timestamp")
}

/// The cycle's injected wall clock, and the upstream snapshot it selects.
fn now() -> i64 {
    ts("2026-08-09T17:00:00Z")
}
fn observation() -> i64 {
    ts("2026-08-09T16:58:00Z")
}
fn hrrr_run() -> i64 {
    ts("2026-08-09T15:00:00Z")
}
fn gfs_run() -> i64 {
    ts("2026-08-09T12:00:00Z")
}

/// The captured HRRR subhourly objects: file number → upstream `Content-Length`.
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
/// The captured GFS objects: lead hour → (upstream length, span start).
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

fn mrms_fixture() -> Vec<u8> {
    fixture("mrms-conus-20260809-165800.grib2.gz")
}

fn hrrr_message(lead: u32) -> Vec<u8> {
    fixture(&format!("hrrr-conus-20260809T15-prate-t{lead}.grib2"))
}

fn gfs_span(lead: u32) -> Vec<u8> {
    fixture(&format!("gfs-global-20260809T12-apcp-f{lead:03}.grib2"))
}

fn upstream() -> FixtureUpstream {
    let mut upstream = FixtureUpstream::default();
    upstream.insert(mrms::object_url(observation()), mrms_fixture(), None);
    // The whole subhourly set must be discoverable for the run to be selectable; only the
    // objects a published lead lives in are ever read.
    for file in hrrr::SUBHOURLY_FILES {
        upstream.insert(
            hrrr::index_url(hrrr_run(), file),
            fixture(&format!("hrrr-conus-20260809T15-f{file:02}.idx")),
            None,
        );
    }
    for (file, object_len) in HRRR_OBJECTS {
        upstream.declare(hrrr::object_url(hrrr_run(), file), object_len);
    }
    for (file, lead, start) in HRRR_RANGES {
        let object_len = HRRR_OBJECTS.iter().find(|(candidate, _)| *candidate == file).expect("declared object").1;
        upstream.insert_range(hrrr::object_url(hrrr_run(), file), object_len, start, hrrr_message(lead));
    }
    for (lead, object_len, start) in GFS_SPANS {
        upstream.insert(
            gfs::index_url(gfs_run(), lead),
            fixture(&format!("gfs-global-20260809T12-f{lead:03}.idx")),
            None,
        );
        upstream.insert_range(gfs::object_url(gfs_run(), lead), object_len, start, gfs_span(lead));
    }
    upstream
}

fn tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if !dir.is_dir() {
            continue;
        }
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let key = path
                    .strip_prefix(root)
                    .unwrap()
                    .components()
                    .map(|component| component.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                files.insert(key, std::fs::read(&path).unwrap());
            }
        }
    }
    files
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("obc-wx-bake-wx6-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Decode one cell out of a published frame the way a corridor client would.
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

fn adapters() -> (us::UsComposite, gfs::GfsFloor) {
    (us::UsComposite, gfs::GfsFloor)
}

#[test]
fn the_composed_us_product_and_the_global_floor_publish_a_valid_byte_stable_tree() {
    let (us_adapter, gfs_adapter) = adapters();
    let adapters: [&dyn Adapter; 2] = [&us_adapter, &gfs_adapter];

    let dir_a = scratch("cycle-a");
    let mut upstream_a = upstream();
    let mut store_a = DirStore::new(&dir_a);
    let report = run_cycle(&adapters, &mut upstream_a, &mut store_a, now(), false).expect("fixture cycle");
    eprintln!("wx6 cycle report:\n{}", report.summary());
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(report.published_objects, 9 + 16 + 1, "one observation + eight HRRR + sixteen GFS + the manifest");

    // Byte-stability: a second cycle from the same fixtures into a fresh store is identical.
    let dir_b = scratch("cycle-b");
    let mut upstream_b = upstream();
    let mut store_b = DirStore::new(&dir_b);
    run_cycle(&adapters, &mut upstream_b, &mut store_b, now(), false).expect("second fixture cycle");
    let tree_a = tree(&dir_a);
    let tree_b = tree(&dir_b);
    assert_eq!(tree_a.keys().collect::<Vec<_>>(), tree_b.keys().collect::<Vec<_>>());
    for (key, bytes) in &tree_a {
        assert_eq!(Some(bytes), tree_b.get(key), "{key} is not byte-stable across cycles");
    }

    let document = manifest::from_json(&tree_a["wx/v1/manifest.json"]).expect("published manifest parses");
    assert_eq!(document.products.len(), 2);
    let mut scratch_buffer = vec![0u8; precip4::MAX_CELLS];
    for product in &document.products {
        for frame in &product.frames {
            let bytes = tree_a.get(&frame.key).unwrap_or_else(|| panic!("{} is not published", frame.key));
            assert_eq!(bytes.len() as u64, frame.bytes, "{}", frame.key);
            let header =
                obcg::validate(bytes, &mut scratch_buffer).unwrap_or_else(|error| panic!("{}: {error:?}", frame.key));
            assert_eq!(format!("0x{:08X}", header.object_crc32), frame.object_crc32);
            assert_eq!(manifest::rfc3339(header.valid_at), frame.valid_at);
            // The manifest's per-frame geometry is the object's own geometry, restated.
            assert_eq!(header.width, frame.geometry.width);
            assert_eq!(header.height, frame.geometry.height);
            assert_eq!(header.south_lat_udeg, frame.geometry.south_udeg);
            assert_eq!(header.west_lon_udeg, frame.geometry.west_udeg);
            assert_eq!(header.cell_lat_udeg, frame.geometry.cell_lat_udeg);
            assert_eq!(header.cell_lon_udeg, frame.geometry.cell_lon_udeg);
            assert_eq!(header.tile_edge, frame.geometry.tile_edge);
            assert_eq!(header.entries_per_page, frame.geometry.entries_per_page);
            // The key, the header and the manifest agree on the offset.
            assert_eq!(
                i64::from(frame.offset_min) * 60,
                header.valid_at - header.reference_time,
                "{} offset disagrees with its own header",
                frame.key
            );
            assert!(frame.key.ends_with(&format!("/f{}.obcg", frame.offset_min)));
        }
    }

    // The composed US product: one timeline, two upstreams, two cell sizes, no resampling.
    let composed = document.products.iter().find(|product| product.id == "us").expect("us product");
    assert_eq!(composed.tier, 1);
    assert_eq!(composed.reference_time, "2026-08-09T16:58:00Z");
    assert_eq!(composed.staleness_deadline, "2026-08-09T17:28:00Z");
    assert!(composed.attribution.text.contains("NOAA"));
    assert!(composed.attribution.text.contains("no NOAA endorsement is implied"));
    assert_eq!(composed.frames.len(), 9);
    let observation_frame = &composed.frames[0];
    assert_eq!(observation_frame.source_class, SourceClass::Observation);
    assert_eq!(observation_frame.offset_min, 0);
    assert_eq!(observation_frame.valid_at, "2026-08-09T16:58:00Z");
    assert_eq!(observation_frame.geometry.cell_size_m, 1_000);
    assert_eq!(observation_frame.geometry.width, 7_000);
    // The forward frames keep HRRR's own valid times: the 15z run's +120..+225 minute steps,
    // which are 2, 17, ... minutes ahead of a 16:58 observation. No re-spacing, no round numbers.
    let forward: Vec<(u32, &str)> =
        composed.frames[1..].iter().map(|frame| (frame.offset_min, frame.valid_at.as_str())).collect();
    assert_eq!(
        forward,
        vec![
            (2, "2026-08-09T17:00:00Z"),
            (17, "2026-08-09T17:15:00Z"),
            (32, "2026-08-09T17:30:00Z"),
            (47, "2026-08-09T17:45:00Z"),
            (62, "2026-08-09T18:00:00Z"),
            (77, "2026-08-09T18:15:00Z"),
            (92, "2026-08-09T18:30:00Z"),
            (107, "2026-08-09T18:45:00Z"),
        ]
    );
    for frame in &composed.frames[1..] {
        assert_eq!(frame.source_class, SourceClass::Forecast);
        assert_eq!(frame.geometry.cell_size_m, 3_000);
        assert_eq!(frame.geometry.width, hrrr::GEOMETRY.width);
    }
    // Provenance survives into the bytes: the seam is visible per object.
    let observation_bytes = &tree_a[&observation_frame.key];
    let observation_header = obcg::decode_header(observation_bytes[..obcg::HEADER_LEN].try_into().unwrap()).unwrap();
    assert_eq!(observation_header.product_id, PRODUCT_MRMS);
    assert_eq!(observation_header.tier, 1);
    assert_eq!(observation_header.flags, FLAG_OBSERVED);
    let forward_bytes = &tree_a[&composed.frames[1].key];
    let forward_header = obcg::decode_header(forward_bytes[..obcg::HEADER_LEN].try_into().unwrap()).unwrap();
    assert_eq!(forward_header.product_id, PRODUCT_HRRR);
    assert_eq!(forward_header.tier, 2);
    assert_eq!(forward_header.flags, FLAG_FORECAST);
    // The product bbox is the intersection of the two windows: where the whole timeline answers.
    assert_eq!(composed.bbox_udeg.south_udeg, i64::from(hrrr::GEOMETRY.south_lat_udeg));
    assert_eq!(composed.bbox_udeg.west_udeg, i64::from(mrms::GEOMETRY.west_lon_udeg));
    assert_eq!(composed.bbox_udeg.north_udeg, hrrr::GEOMETRY.north_lat_udeg());
    assert_eq!(composed.bbox_udeg.east_udeg, hrrr::GEOMETRY.east_lon_udeg());

    // The floor: worldwide, tier 3, hourly forward frames at their real valid times.
    let floor = document.products.iter().find(|product| product.id == "gfs").expect("gfs product");
    assert_eq!(floor.tier, 3);
    assert_eq!(floor.reference_time, "2026-08-09T12:00:00Z");
    assert_eq!(floor.staleness_deadline, "2026-08-10T04:00:00Z");
    assert_eq!(floor.cell.nominal_m, 27_750);
    assert_eq!(floor.frames.len(), 16);
    assert!(floor.frames.iter().all(|frame| frame.source_class == SourceClass::Forecast), "GFS is never observed");
    assert_eq!(floor.frames[0].offset_min, 60);
    assert_eq!(floor.frames[0].valid_at, "2026-08-09T13:00:00Z");
    assert_eq!(floor.frames[15].valid_at, "2026-08-10T04:00:00Z");
    // Worldwide, and honest about the antimeridian column OBCG cannot represent.
    assert_eq!(floor.bbox_udeg.south_udeg, -89_875_000);
    assert_eq!(floor.bbox_udeg.north_udeg, 89_875_000);
    assert_eq!(floor.bbox_udeg.west_udeg, -179_875_000);
    assert_eq!(floor.bbox_udeg.east_udeg, 179_875_000);
}

/// Three non-radar coordinates (Europe outside the DWD domain, the southern hemisphere, the
/// mid-Pacific) resolve to correctly georeferenced floor cells, and a CONUS coordinate resolves
/// in both halves of the composed product.
#[test]
fn published_cells_equal_quantized_nearest_neighbour_source_cells() {
    let (us_adapter, gfs_adapter) = adapters();
    let adapters: [&dyn Adapter; 2] = [&us_adapter, &gfs_adapter];
    let dir = scratch("nn");
    let mut fixture_upstream = upstream();
    let mut store = DirStore::new(&dir);
    run_cycle(&adapters, &mut fixture_upstream, &mut store, now(), false).expect("fixture cycle");
    let published = tree(&dir);

    // --- MRMS observation: independently decode the gzipped GRIB and check the row flip.
    let expected_mrms = ExpectedGrib {
        discipline: 209,
        category: 6,
        parameter: 1,
        grid_template: 0,
        expected_points: 24_500_000,
        expected_grid_definition_hex: MRMS_CONUS_GRID_DEFINITION_HEX,
        product_template: 0,
        representation_templates: &[41],
        missing_sentinels: &[-1.0, -3.0],
        allowed_messages: &[1],
        require_identical_messages: false,
    };
    let field = decode_gzip_field(&mrms_fixture(), &expected_mrms).expect("MRMS decodes");
    let frame = &published["wx/v1/us/20260809T1658Z/f0.obcg"];
    let geometry = mrms::GEOMETRY;
    let mut wet = 0usize;
    let mut nodata = 0usize;
    for cell in (0..geometry.cells()).step_by(1_013) {
        let col = (cell as u32) % geometry.width;
        let row = (cell as u32) / geometry.width;
        // The GRIB scans north-to-south: published row 0 is the last native row.
        let native = (geometry.height - 1 - row) as usize * geometry.width as usize + col as usize;
        let value = field.values[native];
        let expected = if value == -1.0 || value == -3.0 || value.is_nan() {
            precip4::INTENSITY_NODATA
        } else {
            precip4::quantize_rate_mm_per_hour(f64::from(value))
        };
        assert_eq!(published_cell(frame, col, row), expected, "MRMS cell ({col},{row})");
        match expected {
            precip4::INTENSITY_NODATA => nodata += 1,
            0 => {}
            _ => wet += 1,
        }
    }
    eprintln!("mrms NN agreement: {wet} wet, {nodata} no-data sampled cells");
    assert!(wet > 0, "the captured MRMS frame must contain rain for the agreement to mean anything");
    assert!(nodata > 0, "and no-coverage cells, which must never decode as dry");

    // --- HRRR forward frame: independently decode and project through the pinned Lambert map.
    let expected_hrrr = ExpectedGrib {
        discipline: 0,
        category: 1,
        parameter: 7,
        grid_template: 30,
        expected_points: 1_905_141,
        expected_grid_definition_hex: HRRR_CONUS_GRID_DEFINITION_HEX,
        product_template: 0,
        representation_templates: &[3],
        missing_sentinels: &[],
        allowed_messages: &[1],
        require_identical_messages: false,
    };
    let field = decode_field(&hrrr_message(120), &expected_hrrr).expect("HRRR decodes");
    let frame = &published["wx/v1/us/20260809T1658Z/f2.obcg"];
    let geometry = hrrr::GEOMETRY;
    let mut wet = 0usize;
    let mut outside = 0usize;
    for cell in (0..geometry.cells()).step_by(97) {
        let col = (cell as u32) % geometry.width;
        let row = (cell as u32) / geometry.width;
        let expected = match lcc::native_index(geometry.center_lat_deg(row), geometry.center_lon_deg(col)) {
            None => {
                outside += 1;
                precip4::INTENSITY_NODATA
            }
            Some(index) => precip4::quantize_rate_mm_per_hour(f64::from(field.values[index]) * 3_600.0),
        };
        assert_eq!(published_cell(frame, col, row), expected, "HRRR cell ({col},{row})");
        if expected != 0 && expected != precip4::INTENSITY_NODATA {
            wet += 1;
        }
    }
    eprintln!("hrrr NN agreement: {wet} wet, {outside} outside the Lambert domain");
    assert!(wet > 0, "the captured HRRR frame must contain rain");
    assert!(outside > 0, "the window's corners lie outside the domain and must be no-data");

    // --- GFS floor: the remap is exact integer arithmetic, so check named coordinates too.
    let expected_gfs = ExpectedGrib {
        discipline: 0,
        category: 1,
        parameter: 8,
        grid_template: 0,
        expected_points: 1_038_240,
        expected_grid_definition_hex: GFS_GLOBAL_GRID_DEFINITION_HEX,
        product_template: 8,
        representation_templates: &[3],
        missing_sentinels: &[],
        allowed_messages: &[1, 2],
        require_identical_messages: true,
    };
    let hour_one = decode_field(&gfs_span(1), &expected_gfs).expect("GFS f001 decodes");
    let hour_two = decode_field(&gfs_span(2), &expected_gfs).expect("GFS f002 decodes");
    let frame = &published["wx/v1/gfs/20260809T1200Z/f120.obcg"];
    let geometry = gfs::GEOMETRY;
    let mut wet = 0usize;
    for cell in (0..geometry.cells()).step_by(37) {
        let col = (cell as u32) % geometry.width;
        let row = (cell as u32) / geometry.width;
        let native = gfs::native_index(col, row).unwrap();
        let delta = f64::from(hour_two.values[native]) - f64::from(hour_one.values[native]);
        let expected = precip4::quantize_rate_mm_per_hour(delta.max(0.0));
        assert_eq!(published_cell(frame, col, row), expected, "GFS cell ({col},{row})");
        if expected != 0 && expected != precip4::INTENSITY_NODATA {
            wet += 1;
        }
    }
    assert!(wet > 0, "the captured GFS run must contain rain");

    // Three coordinates outside every high-resolution domain still land on the right floor cell.
    for (name, lat, lon) in [
        ("Lisbon", 38.72, -9.14),
        ("Patagonia", -51.62, -69.22),
        ("Tasmania", -42.88, 147.33),
        ("Fiji", -17.71, 178.06),
    ] {
        let col = ((lon * 1e6 - f64::from(geometry.west_lon_udeg)) / f64::from(geometry.cell_lon_udeg)).floor() as u32;
        let row = ((lat * 1e6 - f64::from(geometry.south_lat_udeg)) / f64::from(geometry.cell_lat_udeg)).floor() as u32;
        let native = gfs::native_index(col, row).unwrap_or_else(|| panic!("{name} is inside the floor window"));
        // The sampled native point must be the GFS grid point nearest the coordinate.
        let native_lat = 90.0 - 0.25 * (native / 1_440) as f64;
        let native_lon = 0.25 * (native % 1_440) as f64;
        let native_lon = if native_lon > 180.0 { native_lon - 360.0 } else { native_lon };
        assert!((native_lat - lat).abs() <= 0.125 + 1e-9, "{name}: latitude {native_lat} is not the nearest point");
        assert!((native_lon - lon).abs() <= 0.125 + 1e-9, "{name}: longitude {native_lon} is not the nearest point");
        let delta = f64::from(hour_two.values[native]) - f64::from(hour_one.values[native]);
        assert_eq!(
            published_cell(frame, col, row),
            precip4::quantize_rate_mm_per_hour(delta.max(0.0)),
            "{name} floor cell"
        );
    }
}

#[test]
fn corrupt_upstream_fails_the_cycle_and_publishes_nothing() {
    let (us_adapter, gfs_adapter) = adapters();
    let adapters: [&dyn Adapter; 2] = [&us_adapter, &gfs_adapter];
    let dir = scratch("corrupt");
    let mut good = upstream();
    let mut store = DirStore::new(&dir);
    run_cycle(&adapters, &mut good, &mut store, now(), false).expect("good cycle");
    let before = tree(&dir);

    // A truncated MRMS object fails the whole cycle and moves nothing — including the GFS
    // product, which is baked in the same cycle. (The next two-minute observation exists, so the
    // product genuinely re-bakes instead of short-circuiting.)
    let next = observation() + 120;
    let mut truncated = upstream();
    let mut bytes = mrms_fixture();
    bytes.truncate(bytes.len() / 2);
    truncated.insert(mrms::object_url(next), bytes, None);
    let error = run_cycle(&adapters, &mut truncated, &mut store, next, false).unwrap_err();
    eprintln!("truncated MRMS: {error}");
    assert_eq!(before, tree(&dir), "a failed cycle must leave the previous publication untouched");

    // A flipped byte inside an HRRR message: ditto.
    let mut flipped = upstream();
    let mut message = hrrr_message(135);
    let middle = message.len() / 2;
    message[middle] ^= 0x40;
    flipped.insert(mrms::object_url(next), mrms_fixture(), None);
    flipped.insert_range(hrrr::object_url(hrrr_run(), 3), 214_632_128, 25_809_346, message);
    let error = run_cycle(&adapters, &mut flipped, &mut store, next, false).unwrap_err();
    eprintln!("flipped HRRR byte: {error}");
    assert_eq!(before, tree(&dir));

    // A GFS span carrying a different lead's bytes is never accepted as "successful weather":
    // splice hour 2 over hour 1's range, with the published run pushed back so GFS re-bakes.
    let manifest_path = dir.join("wx/v1/manifest.json");
    let mut document = manifest::from_json(&std::fs::read(&manifest_path).unwrap()).unwrap();
    for product in &mut document.products {
        if product.id == "gfs" {
            product.reference_time = "2026-08-09T06:00:00Z".to_string();
        }
    }
    std::fs::write(&manifest_path, manifest::to_json(&document)).unwrap();
    let before_gfs = tree(&dir);
    let mut swapped = upstream();
    swapped.insert(mrms::object_url(next), mrms_fixture(), None);
    swapped.insert_range(gfs::object_url(gfs_run(), 1), 537_540_348, 427_603_385, gfs_span(2));
    let error = run_cycle(&adapters, &mut swapped, &mut store, next, false).unwrap_err();
    eprintln!("mismatched GFS span: {error}");
    assert_eq!(before_gfs, tree(&dir));
}

#[test]
fn unchanged_upstream_short_circuits_and_moves_no_frame_bytes() {
    let (us_adapter, gfs_adapter) = adapters();
    let adapters: [&dyn Adapter; 2] = [&us_adapter, &gfs_adapter];
    let dir = scratch("unchanged");
    let mut first = upstream();
    let mut store = DirStore::new(&dir);
    let report = run_cycle(&adapters, &mut first, &mut store, now(), false).expect("first cycle");
    // The whole cycle's upstream ingress: one MRMS object, three HRRR indexes, eight HRRR
    // messages, sixteen GFS indexes and sixteen GFS spans.
    eprintln!("wx6 first-cycle ingress: {} bytes", report.fetched_bytes);
    assert!(report.fetched_bytes < 15_000_000, "ingress {} is above the WX1 budget", report.fetched_bytes);
    let before = tree(&dir);

    let mut second = upstream();
    let report = run_cycle(&adapters, &mut second, &mut store, now(), false).expect("second cycle");
    eprintln!("wx6 unchanged report:\n{}", report.summary());
    assert!(report.products.iter().all(|(_, status, _)| *status == ProductStatus::Unchanged));
    assert_eq!(report.fetched_bytes, 0, "no upstream bodies move on an unchanged cycle");
    assert_eq!(report.published_objects, 1, "only the manifest is republished");
    // Both short-circuits are name/run identity, so every request is a HEAD probe.
    assert!(
        second.requests.iter().all(|request| request.starts_with("HEAD ")),
        "an unchanged cycle must not fetch a body: {:?}",
        second.requests
    );
    assert_eq!(before, tree(&dir), "an unchanged cycle republishes identical bytes");
}

/// A withdrawn MRMS object (the newest discoverable observation is older than the published one)
/// keeps the published product and warns, instead of moving reference_time and the staleness
/// deadline backwards.
#[test]
fn an_observation_regression_keeps_the_published_product_and_warns() {
    let (us_adapter, gfs_adapter) = adapters();
    let adapters: [&dyn Adapter; 2] = [&us_adapter, &gfs_adapter];
    let dir = scratch("regression");
    let mut first = upstream();
    let mut store = DirStore::new(&dir);
    run_cycle(&adapters, &mut first, &mut store, now(), false).expect("first cycle");

    let manifest_path = dir.join("wx/v1/manifest.json");
    let mut document = manifest::from_json(&std::fs::read(&manifest_path).unwrap()).unwrap();
    for product in &mut document.products {
        if product.id == "us" {
            product.reference_time = "2026-08-09T17:20:00Z".to_string();
        }
    }
    std::fs::write(&manifest_path, manifest::to_json(&document)).unwrap();

    let mut second = upstream();
    let report = run_cycle(&adapters, &mut second, &mut store, now() + 120, false).expect("regression cycle");
    assert!(
        report.warnings.iter().any(|warning| warning.contains("older than the published")),
        "{:?}",
        report.warnings
    );
    assert_eq!(report.fetched_bytes, 0, "a regression bakes nothing");
    let republished = manifest::from_json(&std::fs::read(&manifest_path).unwrap()).unwrap();
    let entry = republished.products.iter().find(|product| product.id == "us").unwrap();
    assert_eq!(entry.reference_time, "2026-08-09T17:20:00Z", "reference_time must not regress");
}

/// A late HRRR publication shortens the forward window and warns; it never invents frames, and
/// the observation still publishes. Here the newest complete run is five hours old, so its whole
/// retained lead set is already behind the observation and the product is the radar frame alone.
#[test]
fn a_stale_hrrr_run_degrades_to_fewer_forward_frames_with_a_warning() {
    let (us_adapter, _) = adapters();
    let adapters: [&dyn Adapter; 1] = [&us_adapter];
    let dir = scratch("degraded");
    let mut fixture_upstream = FixtureUpstream::default();
    let mut store = DirStore::new(&dir);
    fixture_upstream.insert(mrms::object_url(observation()), mrms_fixture(), None);
    let stale_run = ts("2026-08-09T12:00:00Z");
    for file in hrrr::SUBHOURLY_FILES {
        fixture_upstream.declare(hrrr::index_url(stale_run, file), 11_842);
    }
    let report = run_cycle(&adapters, &mut fixture_upstream, &mut store, now(), false).expect("degraded cycle");
    eprintln!("degraded report:\n{}", report.summary());
    assert!(report.warnings.iter().any(|warning| warning.contains("forward frames")), "{:?}", report.warnings);
    let document = manifest::from_json(&std::fs::read(dir.join("wx/v1/manifest.json")).unwrap()).unwrap();
    let composed = document.products.iter().find(|product| product.id == "us").unwrap();
    assert_eq!(composed.frames.len(), 1, "the observation alone — no lead is still ahead of it");
    assert_eq!(composed.frames[0].source_class, SourceClass::Observation);
    assert_eq!(composed.frames[0].offset_min, 0);
    // Not one HRRR body was fetched for a run with nothing to contribute.
    assert!(
        fixture_upstream.requests.iter().all(|request| !request.contains("wrfsubh") || request.starts_with("HEAD ")),
        "{:?}",
        fixture_upstream.requests
    );
}

/// The published objects declare the registry codes the OBCG spec assigns, and nothing branches
/// on them: this is the provenance the manifest's product id mirrors.
#[test]
fn published_objects_carry_their_own_registry_codes() {
    assert_eq!(PRODUCT_MRMS, 3);
    assert_eq!(PRODUCT_HRRR, 4);
    assert_eq!(PRODUCT_GFS, 5);
}
