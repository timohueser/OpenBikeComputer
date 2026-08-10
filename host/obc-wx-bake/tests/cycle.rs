//! Deterministic fixture cycles: the WX5 acceptance tests.
//!
//! Same checked-in upstream bytes ⇒ byte-identical published tree. Corrupt upstream ⇒ the cycle
//! fails loudly and publishes nothing, leaving the previous publication byte-identical.
//! Unchanged upstream ⇒ no frame bytes move. And every published cell equals the quantized
//! nearest-neighbour source cell — no smoothing, provably.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use obc_wx_bake::cycle::{run_cycle, ProductStatus};
use obc_wx_bake::fetch::FixtureUpstream;
use obc_wx_bake::grib::{decode_bzip2_field, ExpectedGrib, ICON_EU_GRID_DEFINITION_HEX};
use obc_wx_bake::manifest::{self, SourceClass};
use obc_wx_bake::publish::DirStore;
use obc_wx_bake::source::{dwd_rv, icon_eu, Adapter};
use obc_wx_bake::stereo;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(fixture_dir().join(name)).unwrap_or_else(|error| panic!("fixture {name}: {error}"))
}

fn ts(text: &str) -> i64 {
    manifest::parse_rfc3339(text).expect("test timestamp")
}

fn icon_run() -> i64 {
    ts("2026-08-09T06:00:00Z")
}

/// The cycle's injected wall clock: shortly after the captured RV run.
fn now() -> i64 {
    ts("2026-08-09T14:30:00Z")
}

const RV_ETAG: &str = "\"6a788c2a-273800\"";

fn upstream() -> FixtureUpstream {
    let mut upstream = FixtureUpstream::default();
    upstream.insert(dwd_rv::LATEST_URL, fixture("composite_rv_20260809_1420.tar"), Some(RV_ETAG));
    for lead in 0..=12u32 {
        upstream.insert(
            icon_eu::lead_url(icon_run(), lead),
            fixture(&format!("icon-eu-2026080906_{lead:03}.grib2.bz2")),
            None,
        );
    }
    upstream
}

fn adapters() -> (dwd_rv::DwdRv, icon_eu::IconEu) {
    (dwd_rv::DwdRv, icon_eu::IconEu)
}

/// Every file under `root`, keyed by its forward-slash relative path.
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
    let dir = std::env::temp_dir().join(format!("obc-wx-bake-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn full_cycle_is_deterministic_byte_stable_and_valid() {
    let (dwd, icon) = adapters();
    let adapters: [&dyn Adapter; 2] = [&dwd, &icon];

    let dir_a = scratch("cycle-a");
    let mut upstream_a = upstream();
    let mut store_a = DirStore::new(&dir_a);
    let report = run_cycle(&adapters, &mut upstream_a, &mut store_a, now(), false).expect("fixture cycle");
    eprintln!("cycle report:\n{}", report.summary());
    assert_eq!(report.published_objects, 9 + 12 + 1, "nine RV frames, twelve ICON frames, one manifest");
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);

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

    // The manifest agrees with the published objects, and every object passes the same
    // validator the shared vectors pin.
    let document = manifest::from_json(&tree_a["wx/v1/manifest.json"]).expect("published manifest parses");
    assert_eq!(document.generated_at, "2026-08-09T14:30:00Z");
    assert_eq!(document.products.len(), 2);
    let mut scratch_buffer = vec![0u8; obc_formats::precip4::MAX_CELLS];
    for product in &document.products {
        let expected_frames: usize = match product.id.as_str() {
            "dwd-rv" => {
                assert_eq!(product.tier, 1);
                assert_eq!(product.reference_time, "2026-08-09T14:20:00Z");
                assert_eq!(product.staleness_deadline, "2026-08-09T14:50:00Z");
                assert_eq!(product.upstream_etag.as_deref(), Some(RV_ETAG));
                9
            }
            "icon-eu" => {
                assert_eq!(product.tier, 2);
                assert_eq!(product.reference_time, "2026-08-09T06:00:00Z");
                assert_eq!(product.staleness_deadline, "2026-08-09T16:00:00Z");
                12
            }
            other => panic!("unexpected product {other}"),
        };
        assert_eq!(product.frames.len(), expected_frames, "{}", product.id);
        assert!(product.attribution.text.contains("Deutscher Wetterdienst"));
        for frame in &product.frames {
            let bytes = tree_a.get(&frame.key).unwrap_or_else(|| panic!("{} is not published", frame.key));
            assert_eq!(bytes.len() as u64, frame.bytes, "{}", frame.key);
            let header = obc_formats::obcg::validate(bytes, &mut scratch_buffer)
                .unwrap_or_else(|error| panic!("{}: {error:?}", frame.key));
            assert_eq!(format!("0x{:08X}", header.object_crc32), frame.object_crc32);
            assert_eq!(manifest::rfc3339(header.valid_at), frame.valid_at);
            assert_eq!(
                i64::from(header.flags & obc_formats::obcg::FLAG_OBSERVED != 0),
                i64::from(frame.source_class == SourceClass::Observation)
            );
            assert_eq!(header.width, frame.geometry.width);
            assert_eq!(header.height, frame.geometry.height);
            assert_eq!(header.tile_edge, frame.geometry.tile_edge);
        }
        // The observation flag appears exactly on the RV lead-0 frame.
        let observations = product.frames.iter().filter(|frame| frame.source_class == SourceClass::Observation).count();
        assert_eq!(observations, usize::from(product.id == "dwd-rv"));
    }
}

/// Decode one cell out of a published frame the way a corridor client would.
fn published_cell(bytes: &[u8], col: u32, row: u32) -> u8 {
    use obc_formats::obcg;
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

/// No smoothing, provably: every sampled published cell equals the quantized nearest-neighbour
/// upstream cell, for both adapters, against independently decoded upstream bytes.
#[test]
fn published_cells_equal_quantized_nearest_neighbour_source_cells() {
    let (dwd, icon) = adapters();
    let adapters: [&dyn Adapter; 2] = [&dwd, &icon];
    let dir = scratch("nn");
    let mut fixture_upstream = upstream();
    let mut store = DirStore::new(&dir);
    run_cycle(&adapters, &mut fixture_upstream, &mut store, now(), false).expect("fixture cycle");
    let published = tree(&dir);

    // DWD RV lead 0: decode the raw ODIM member straight out of the tar.
    let tar_bytes = fixture("composite_rv_20260809_1420.tar");
    let mut archive = tar::Archive::new(tar_bytes.as_slice());
    let mut raw: Option<Vec<u32>> = None;
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        if entry.path().unwrap().file_name().unwrap().to_str() == Some("composite_rv_20260809_1420_000-hd5") {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            let file = hdf5_pure::File::from_bytes(bytes).unwrap();
            raw = Some(file.dataset("dataset1/data1/data").unwrap().read_u32().unwrap());
        }
    }
    let raw = raw.expect("lead-0 member in the fixture tar");
    let frame = &published["wx/v1/dwd-rv/20260809T1420Z/f0.obcg"];
    let geometry = dwd_rv::GEOMETRY;
    let gain = 0.000_999_999_931_780_621_3;
    let mut checked = 0usize;
    let mut wet = 0usize;
    for cell in (0..geometry.cells()).step_by(97) {
        let col = (cell as u32) % geometry.width;
        let row = (cell as u32) / geometry.width;
        let lat = geometry.center_lat_deg(row);
        let lon = geometry.center_lon_deg(col);
        let expected = match stereo::native_index(lat, lon) {
            None => obc_formats::precip4::INTENSITY_NODATA,
            Some(index) => {
                let encoded = u64::from(raw[index]);
                if encoded == 4_294_967_295 {
                    obc_formats::precip4::INTENSITY_NODATA
                } else if encoded == 0 {
                    obc_formats::precip4::INTENSITY_DRY
                } else {
                    let mm_5min = encoded as f64 * gain - gain;
                    obc_formats::precip4::quantize_rate_mm_per_hour(mm_5min * 12.0)
                }
            }
        };
        let actual = published_cell(frame, col, row);
        assert_eq!(actual, expected, "cell ({col},{row})");
        checked += 1;
        if expected != 0 && expected != 15 {
            wet += 1;
        }
    }
    eprintln!("dwd-rv NN agreement: {checked} sampled cells, {wet} wet");
    assert!(checked > 10_000);
    assert!(wet > 0, "the captured run must contain rain for the agreement to mean anything");

    // ICON-EU lead 1: f000 is the all-zero baseline, so the hourly rate is f001 itself.
    let expected_field = ExpectedGrib {
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
    };
    let f001 = decode_bzip2_field(&fixture("icon-eu-2026080906_001.grib2.bz2"), &expected_field).unwrap();
    let frame = &published["wx/v1/icon-eu/20260809T0600Z/f60.obcg"];
    let geometry = icon_eu::GEOMETRY;
    let mut wet = 0usize;
    for cell in (0..geometry.cells()).step_by(53) {
        let col = (cell as u32) % geometry.width;
        let row = (cell as u32) / geometry.width;
        let expected = obc_formats::precip4::quantize_rate_mm_per_hour(f64::from(f001.values[cell]));
        let actual = published_cell(frame, col, row);
        assert_eq!(actual, expected, "cell ({col},{row})");
        if expected != 0 && expected != 15 {
            wet += 1;
        }
    }
    assert!(wet > 0, "the captured ICON run must contain rain for the agreement to mean anything");
}

/// A corrupt upstream publishes **nothing of that product** and leaves its published frames
/// standing — and, since WXR6 (#1245), costs no *other* product its publication.
///
/// The isolation is the change: before it, one adapter's error propagated out of `run_cycle` and
/// blocked every healthy product in the same invocation, which is the coupling the per-adapter
/// systemd timers exist to work around. What has not changed is what fail-closed protects: the
/// failing product moves no bytes, its previous entry is carried forward verbatim, and a cycle in
/// which *every* selected adapter failed is still an error that publishes nothing.
#[test]
fn a_corrupt_upstream_publishes_nothing_of_its_own_and_does_not_block_the_others() {
    let (dwd, icon) = adapters();
    let adapters: [&dyn Adapter; 2] = [&dwd, &icon];

    // First, a good publication to protect.
    let dir = scratch("corrupt");
    let mut good_upstream = upstream();
    let mut store = DirStore::new(&dir);
    run_cycle(&adapters, &mut good_upstream, &mut store, now(), false).expect("good cycle");
    let before = tree(&dir);
    let frames_before: BTreeMap<_, _> =
        before.iter().filter(|(key, _)| key.ends_with(".obcg")).map(|(k, v)| (k.clone(), v.clone())).collect();

    // A truncated RV tar: RV publishes nothing, ICON is untouched and short-circuits, so no frame
    // object anywhere moves — only the manifest is rewritten, with the failure recorded.
    let mut truncated = upstream();
    let mut tar = fixture("composite_rv_20260809_1420.tar");
    tar.truncate(tar.len() / 2);
    truncated.insert(dwd_rv::LATEST_URL, tar, Some("\"changed-1\""));
    let report = run_cycle(&adapters, &mut truncated, &mut store, now() + 300, false).expect("the cycle survives");
    eprintln!("truncated tar:\n{}", report.summary());
    assert!(
        report.products.iter().any(|(id, status, _)| id == "dwd-rv" && *status == ProductStatus::Failed),
        "{:?}",
        report.products
    );
    assert!(
        report.products.iter().any(|(id, status, _)| id == "icon-eu" && *status == ProductStatus::Unchanged),
        "a healthy product must still publish: {:?}",
        report.products
    );
    assert!(report.warnings.iter().any(|warning| warning.contains("dwd-rv: bake failed")), "{:?}", report.warnings);
    let after: BTreeMap<_, _> = tree(&dir).into_iter().filter(|(key, _)| key.ends_with(".obcg")).collect();
    assert_eq!(frames_before, after, "a failed product must not move a single frame object");
    let document = manifest::from_json(&std::fs::read(dir.join("wx/v1/manifest.json")).unwrap()).unwrap();
    let rv = document.products.iter().find(|product| product.id == "dwd-rv").expect("carried forward");
    assert_eq!(rv.reference_time, "2026-08-09T14:20:00Z", "the failed product's published entry stands unchanged");

    // A flipped byte inside an HDF5 member: ditto.
    let mut flipped = upstream();
    let mut tar = fixture("composite_rv_20260809_1420.tar");
    let middle = tar.len() / 2;
    tar[middle] ^= 0x40;
    flipped.insert(dwd_rv::LATEST_URL, tar, Some("\"changed-2\""));
    let report = run_cycle(&adapters, &mut flipped, &mut store, now() + 600, false).expect("the cycle survives");
    eprintln!("flipped tar byte:\n{}", report.summary());
    let after: BTreeMap<_, _> = tree(&dir).into_iter().filter(|(key, _)| key.ends_with(".obcg")).collect();
    assert_eq!(frames_before, after);

    // A truncated ICON lead, with RV *also* broken: every selected adapter has now failed, so the
    // cycle itself is an error and nothing — not even the manifest — is republished.
    let mut both_bad = upstream();
    let mut tar = fixture("composite_rv_20260809_1420.tar");
    tar.truncate(tar.len() / 2);
    both_bad.insert(dwd_rv::LATEST_URL, tar, Some("\"changed-3\""));
    let mut lead = fixture("icon-eu-2026080906_005.grib2.bz2");
    lead.truncate(lead.len() - 100);
    both_bad.insert(icon_eu::lead_url(icon_run(), 5), lead, None);
    // Make ICON re-bake by pretending the published run is older.
    let mut document = manifest::from_json(&std::fs::read(dir.join("wx/v1/manifest.json")).unwrap()).unwrap();
    for product in &mut document.products {
        if product.id == "icon-eu" {
            product.reference_time = "2026-08-09T00:00:00Z".to_string();
        }
    }
    std::fs::write(dir.join("wx/v1/manifest.json"), manifest::to_json(&document)).unwrap();
    let before_both = tree(&dir);
    let error = run_cycle(&adapters, &mut both_bad, &mut store, now() + 900, false).unwrap_err();
    eprintln!("every adapter broken: {error}");
    assert_eq!(before_both, tree(&dir), "a wholly failed cycle must publish nothing at all");
}

/// One broken adapter must not be able to fail a cycle it is not selected in, and a single-
/// adapter invocation whose one adapter fails must still be a loud, non-zero failure — that is
/// what the shipped per-adapter systemd units read.
#[test]
fn a_single_adapter_invocation_still_fails_loudly() {
    let (dwd, icon) = adapters();
    let dir = scratch("single-fail");
    let mut store = DirStore::new(&dir);
    run_cycle(&[&dwd, &icon], &mut upstream(), &mut store, now(), false).expect("good cycle");
    let before = tree(&dir);

    let mut truncated = upstream();
    let mut tar = fixture("composite_rv_20260809_1420.tar");
    tar.truncate(tar.len() / 2);
    truncated.insert(dwd_rv::LATEST_URL, tar, Some("\"changed-1\""));
    let error = run_cycle(&[&dwd], &mut truncated, &mut store, now() + 300, false).unwrap_err();
    assert!(error.contains("dwd-rv: bake failed"), "{error}");
    assert_eq!(before, tree(&dir), "the only selected adapter failing must publish nothing");
}

/// Review finding 6: an upstream run regression (newest complete run older than the published
/// one) keeps the published product and warns, instead of moving reference_time and the
/// staleness deadline backwards.
#[test]
fn an_upstream_run_regression_keeps_the_published_product_and_warns() {
    let (dwd, icon) = adapters();
    let adapters: [&dyn Adapter; 2] = [&dwd, &icon];
    let dir = scratch("regression");
    let mut first = upstream();
    let mut store = DirStore::new(&dir);
    run_cycle(&adapters, &mut first, &mut store, now(), false).expect("first cycle");

    // Pretend a newer ICON run had been published, then withdrawn upstream: the fixture's 06Z
    // run is now a regression relative to the published 12Z reference.
    let manifest_path = dir.join("wx/v1/manifest.json");
    let mut document = manifest::from_json(&std::fs::read(&manifest_path).unwrap()).unwrap();
    for product in &mut document.products {
        if product.id == "icon-eu" {
            product.reference_time = "2026-08-09T12:00:00Z".to_string();
        }
    }
    std::fs::write(&manifest_path, manifest::to_json(&document)).unwrap();

    let mut second = upstream();
    let report = run_cycle(&adapters, &mut second, &mut store, now() + 300, false).expect("regression cycle");
    eprintln!("regression report:\n{}", report.summary());
    assert!(report.products.iter().all(|(_, status, _)| *status == ProductStatus::Unchanged));
    assert!(
        report.warnings.iter().any(|warning| warning.contains("older than the published")),
        "{:?}",
        report.warnings
    );
    assert_eq!(report.fetched_bytes, 0, "a regression bakes nothing");
    let republished = manifest::from_json(&std::fs::read(&manifest_path).unwrap()).unwrap();
    let icon_entry = republished.products.iter().find(|product| product.id == "icon-eu").unwrap();
    assert_eq!(icon_entry.reference_time, "2026-08-09T12:00:00Z", "reference_time must not regress");
}

/// Review finding 5: a carried-forward frame an unchanged product still references must be
/// fetchable at the destination, or the manifest swap is refused.
#[test]
fn a_deleted_carried_frame_blocks_the_manifest_swap() {
    let (dwd, icon) = adapters();
    let adapters: [&dyn Adapter; 2] = [&dwd, &icon];
    let dir = scratch("carried");
    let mut first = upstream();
    let mut store = DirStore::new(&dir);
    run_cycle(&adapters, &mut first, &mut store, now(), false).expect("first cycle");
    let manifest_before = std::fs::read(dir.join("wx/v1/manifest.json")).unwrap();

    // A lifecycle misconfiguration expires one published RV frame out from under the manifest.
    std::fs::remove_file(dir.join("wx/v1/dwd-rv/20260809T1420Z/f45.obcg")).unwrap();

    let mut second = upstream();
    let error = run_cycle(&adapters, &mut second, &mut store, now() + 300, false).unwrap_err();
    eprintln!("carried-frame error: {error}");
    assert!(error.contains("refusing to swap the manifest in"), "{error}");
    assert_eq!(
        std::fs::read(dir.join("wx/v1/manifest.json")).unwrap(),
        manifest_before,
        "the manifest must not be replaced past a missing carried frame"
    );
}

#[test]
fn unchanged_upstream_short_circuits_and_moves_no_frame_bytes() {
    let (dwd, icon) = adapters();
    let adapters: [&dyn Adapter; 2] = [&dwd, &icon];
    let dir = scratch("unchanged");
    let mut first = upstream();
    let mut store = DirStore::new(&dir);
    let report = run_cycle(&adapters, &mut first, &mut store, now(), false).expect("first cycle");
    assert!(report.fetched_bytes > 6_000_000, "the first cycle fetches the tar and thirteen leads");
    let before = tree(&dir);

    let mut second = upstream();
    let report = run_cycle(&adapters, &mut second, &mut store, now(), false).expect("second cycle");
    eprintln!("unchanged report:\n{}", report.summary());
    assert!(report.products.iter().all(|(_, status, _)| *status == ProductStatus::Unchanged));
    assert_eq!(report.fetched_bytes, 0, "no upstream bodies move on an unchanged cycle");
    assert_eq!(report.published_objects, 1, "only the manifest is republished");
    // The RV short-circuit is the etag (one conditional request, no body); the ICON one is run
    // identity (thirteen HEAD probes, no bodies).
    assert!(second.requests.iter().any(|request| request == dwd_rv::LATEST_URL));
    assert_eq!(before, tree(&dir), "an unchanged cycle republishes identical bytes");
}

/// WX18: the per-adapter systemd timers invoke one adapter at a time so a broken upstream can
/// only cost its own product freshness. The manifest is the whole service's state, so an
/// invocation that bakes a subset must carry every other still-usable product forward untouched.
#[test]
fn a_per_adapter_cycle_carries_the_products_it_did_not_select() {
    let (dwd, icon) = adapters();
    let both: [&dyn Adapter; 2] = [&dwd, &icon];
    let dir = scratch("per-adapter");
    let mut store = DirStore::new(&dir);
    run_cycle(&both, &mut upstream(), &mut store, now(), false).expect("full cycle");
    let before = tree(&dir);
    let published_before = manifest::from_json(&before["wx/v1/manifest.json"]).unwrap();
    let icon_before = published_before.products.iter().find(|product| product.id == "icon-eu").unwrap();

    // The radar timer alone, five minutes later.
    let radar_only: [&dyn Adapter; 1] = [&dwd];
    let report = run_cycle(&radar_only, &mut upstream(), &mut store, now() + 300, false).expect("radar-only cycle");
    eprintln!("radar-only report:\n{}", report.summary());
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(report.published_objects, 1, "only the manifest is republished");
    assert!(
        report.products.iter().any(|(id, status, _)| id == "icon-eu" && *status == ProductStatus::NotSelected),
        "{:?}",
        report.products
    );

    let after = tree(&dir);
    assert_eq!(before.len(), after.len(), "a radar-only cycle publishes no new objects");
    for (key, bytes) in &before {
        if key == "wx/v1/manifest.json" {
            continue;
        }
        assert_eq!(Some(bytes), after.get(key), "{key} changed on a radar-only cycle");
    }

    let document = manifest::from_json(&after["wx/v1/manifest.json"]).unwrap();
    assert_eq!(
        document.products.iter().map(|product| product.id.as_str()).collect::<Vec<_>>(),
        ["dwd-rv", "icon-eu"],
        "the unselected product stays in the manifest, in a stable order"
    );
    let icon_after = document.products.iter().find(|product| product.id == "icon-eu").unwrap();
    assert_eq!(icon_after.reference_time, icon_before.reference_time);
    assert_eq!(icon_after.generated_at, icon_before.generated_at, "a carried entry is not re-stamped");
    assert_eq!(icon_after.staleness_deadline, icon_before.staleness_deadline);
    assert_eq!(icon_after.frames.len(), icon_before.frames.len());
    for (carried, original) in icon_after.frames.iter().zip(&icon_before.frames) {
        assert_eq!(carried.key, original.key);
        assert_eq!(carried.bytes, original.bytes);
        assert_eq!(carried.object_crc32, original.object_crc32);
        assert_eq!(carried.valid_at, original.valid_at);
    }
}

/// The other half of the rule, and the one an adversarial review had to prove: an expired product
/// is **carried, not dropped** — visibly expired in the manifest, with its frames exempt from the
/// pre-swap fetchability proof. Dropping it would cost the product's own next tick its
/// short-circuit, so a stalled upstream would be re-downloaded and re-published every few minutes
/// while the manifest flickered the product present/absent (and the external alarm flapped with
/// it). This is that scenario: an upstream frozen on one run, two timers alternating across the
/// deadline.
#[test]
fn an_expired_product_is_carried_and_never_refetched_while_timers_alternate() {
    let (dwd, icon) = adapters();
    let both: [&dyn Adapter; 2] = [&dwd, &icon];
    let radar_only: [&dyn Adapter; 1] = [&dwd];
    let model_only: [&dyn Adapter; 1] = [&icon];
    let dir = scratch("stalled-upstream");
    let mut store = DirStore::new(&dir);
    run_cycle(&both, &mut upstream(), &mut store, now(), false).expect("full cycle");

    // ICON's deadline is its 06Z run + 10 h = 16:00Z. Both upstreams are frozen on the runs
    // already published (the fixture never moves), so from here on nothing may be fetched again.
    let mut manifest_bytes = std::fs::read(dir.join("wx/v1/manifest.json")).unwrap();
    let mut tick = ts("2026-08-09T15:50:00Z");
    for round in 0..6 {
        let (label, selection): (&str, &[&dyn Adapter]) =
            if round % 2 == 0 { ("radar", &radar_only) } else { ("model", &model_only) };
        let mut stalled = upstream();
        let report = run_cycle(selection, &mut stalled, &mut store, tick, false)
            .unwrap_or_else(|error| panic!("{label} tick at round {round}: {error}"));
        eprintln!("round {round} ({label}) at {tick}:\n{}", report.summary());

        assert_eq!(report.fetched_bytes, 0, "round {round}: a stalled upstream must never be re-downloaded");
        assert_eq!(report.published_objects, 1, "round {round}: only the manifest is republished");

        let document = manifest::from_json(&std::fs::read(dir.join("wx/v1/manifest.json")).unwrap()).unwrap();
        assert_eq!(
            document.products.iter().map(|product| product.id.as_str()).collect::<Vec<_>>(),
            ["dwd-rv", "icon-eu"],
            "round {round}: both products stay listed — a manifest that flickers flaps the alarm"
        );
        // Everything except the manifest's own generated_at is frozen: same entries, same objects.
        let now_bytes = std::fs::read(dir.join("wx/v1/manifest.json")).unwrap();
        let strip = |bytes: &[u8]| {
            let mut document = manifest::from_json(bytes).unwrap();
            document.generated_at = String::new();
            manifest::to_json(&document)
        };
        assert_eq!(strip(&manifest_bytes), strip(&now_bytes), "round {round}: the carried entries must not move");
        manifest_bytes = now_bytes;
        tick += 300;
    }

    // Past 16:00Z the ICON entry is expired — still published, still honest about why.
    let last = run_cycle(&radar_only, &mut upstream(), &mut store, ts("2026-08-09T16:30:00Z"), false)
        .expect("radar tick past the ICON deadline");
    eprintln!("expired-carry report:\n{}", last.summary());
    assert!(
        last.warnings.iter().any(|warning| warning.contains("icon-eu") && warning.contains("staleness deadline")),
        "{:?}",
        last.warnings
    );
    let document = manifest::from_json(&std::fs::read(dir.join("wx/v1/manifest.json")).unwrap()).unwrap();
    let icon = document.products.iter().find(|product| product.id == "icon-eu").expect("expired but present");
    assert_eq!(icon.staleness_deadline, "2026-08-09T16:00:00Z", "the entry keeps its true, passed deadline");
    assert_eq!(last.fetched_bytes, 0, "expiry does not trigger a re-fetch either");

    // And the expired product's frames no longer have to exist: the lifecycle rule is allowed to
    // collect them, and one dead product must not block a live one's publication.
    std::fs::remove_file(dir.join("wx/v1/icon-eu/20260809T0600Z/f120.obcg")).unwrap();
    let after = run_cycle(&radar_only, &mut upstream(), &mut store, ts("2026-08-09T16:35:00Z"), false)
        .expect("a healthy product still publishes with an expired product's frames collected");
    assert_eq!(after.published_objects, 1);
}

/// MINOR 4 from the review: the per-adapter and the whole-cycle routes to the same state must
/// produce the *same document*, byte for byte — otherwise "one manifest, whoever baked it" is a
/// claim rather than a property.
#[test]
fn a_subset_cycle_and_a_full_cycle_write_byte_identical_manifests() {
    let (dwd, icon) = adapters();
    let both: [&dyn Adapter; 2] = [&dwd, &icon];
    let radar_only: [&dyn Adapter; 1] = [&dwd];
    let model_only: [&dyn Adapter; 1] = [&icon];
    let later = now() + 300;

    let full_dir = scratch("permutation-full");
    let mut full = DirStore::new(&full_dir);
    run_cycle(&both, &mut upstream(), &mut full, now(), false).expect("full cycle");
    run_cycle(&both, &mut upstream(), &mut full, later, false).expect("second full cycle");

    let split_dir = scratch("permutation-split");
    let mut split = DirStore::new(&split_dir);
    run_cycle(&both, &mut upstream(), &mut split, now(), false).expect("full cycle");
    run_cycle(&radar_only, &mut upstream(), &mut split, later, false).expect("radar tick");
    run_cycle(&model_only, &mut upstream(), &mut split, later, false).expect("model tick");

    assert_eq!(
        std::fs::read(full_dir.join("wx/v1/manifest.json")).unwrap(),
        std::fs::read(split_dir.join("wx/v1/manifest.json")).unwrap(),
        "two timers and one cycle must agree on the published document"
    );
    assert_eq!(tree(&full_dir), tree(&split_dir), "and on every object beside it");
}
