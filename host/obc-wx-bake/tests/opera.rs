//! WXR6 (#1245): the OPERA adapters against real checked-in bytes.
//!
//! The fixtures are crops of two live objects — a rectangular block of the upstream file's own
//! deflate streams, re-wrapped in a TIFF header whose tags are the upstream's apart from the
//! tiepoint (`tests/fixtures/README.md` records the recipe and both digests). Every test below
//! therefore runs the production path — TIFF parse, deflate, contract verification, LAEA
//! projection, Z-R conversion, quantization — over bytes EUMETNET actually published, on a window
//! sized to the crop rather than to the 25-million-cell continent.

use std::path::PathBuf;

use obc_formats::precip4::{INTENSITY_DRY, INTENSITY_NODATA};
use obc_wx_bake::fetch::FixtureUpstream;
use obc_wx_bake::geometry::GridGeometry;
use obc_wx_bake::manifest::{self, AttributionEntry, Bbox, Cell, Product};
use obc_wx_bake::source::opera::{self, Contract};
use obc_wx_bake::source::{opera_cirrus, opera_nimbus, Adapter, AdapterOutcome};

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
    std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

const CIRRUS_CROP: &str = "opera-cirrus-20260810T0000-dbzh-crop.tiff";
const NIMBUS_CROP: &str = "opera-nimbus-20260810T0000-rate-crop.tiff";
/// Both crops come from the composites valid at 2026-08-10T00:00Z.
const VALID_AT: i64 = 1_786_320_000;

/// The CIRRUS crop's own registration: 1,024 x 1,024 native cells starting at tile (row 5,
/// col 3) of the 3,800 x 4,400 composite, so its tiepoint is the composite's shifted by
/// 1,536,000 m east and 2,560,000 m south.
fn cirrus_crop_contract() -> Contract {
    Contract {
        width: 1_024,
        height: 1_024,
        ul_x: 1_535_499.999_728_566_7,
        ul_y: -2_559_500.000_087_613,
        ..opera_cirrus::CONTRACT
    }
}

/// The NIMBUS crop: tiles (rows 3-4, cols 1-2), which include the composite's partial southern
/// tile row — 1,024 x 664 native cells rather than a clean 1,024 x 1,024.
fn nimbus_crop_contract() -> Contract {
    Contract {
        width: 1_024,
        height: 664,
        ul_x: 1_022_999.999_728_566_7,
        ul_y: -3_071_000.000_087_613,
        ..opera_nimbus::CONTRACT
    }
}

/// A window on the canonical lattice, the same one the products publish on, sized to a crop.
fn window(south_udeg: i32, west_udeg: i32, width: u32, height: u32, cell_size_m: u16) -> GridGeometry {
    GridGeometry { south_lat_udeg: south_udeg, west_lon_udeg: west_udeg, width, height, cell_size_m, ..opera::WINDOW }
}

fn histogram(cells: &[u8]) -> [usize; 16] {
    let mut counts = [0usize; 16];
    for cell in cells {
        counts[*cell as usize] += 1;
    }
    counts
}

/// 12 x 7 degrees over the Alps, northern Italy and the Balkans — the CIRRUS crop's footprint.
fn cirrus_window() -> GridGeometry {
    window(42_000_000, 6_000_000, 1_200, 700, 1_000)
}

/// 12 x 7 degrees over Sicily, Malta and the central Mediterranean, where the NIMBUS crop sits.
fn nimbus_window() -> GridGeometry {
    window(36_000_000, 14_000_000, 1_200, 700, 2_000)
}

/// **The load-bearing distinction.** In one real CIRRUS frame the same window carries 775,442
/// cells the radars saw nothing in and 37,727 cells no radar can see, and the two must never
/// become one number: the first is dry weather, the second is a hole for the mosaic's floor
/// source to fill. Both counts are exact and free of any floating-point path — dry comes from a
/// `NaN` sample (the Z-R relation cannot produce a zero rate from a finite reflectivity) and
/// no-data from the `-9999000` sentinel or from falling off the raster.
#[test]
fn undetect_and_no_coverage_stay_different_cells() {
    let mut warnings = Vec::new();
    let frame =
        opera::bake_frame_on(&cirrus_crop_contract(), &fixture(CIRRUS_CROP), cirrus_window(), VALID_AT, &mut warnings)
            .expect("the crop bakes");
    let counts = histogram(&frame.cells);
    assert_eq!(counts[usize::from(INTENSITY_DRY)], 775_442, "covered-but-dry cells");
    assert_eq!(counts[usize::from(INTENSITY_NODATA)], 37_727, "no-coverage cells");
    assert_eq!(counts.iter().sum::<usize>(), cirrus_window().cells());
    // 13 and 14 are reserved; nothing may ever land there.
    assert_eq!((counts[13], counts[14]), (0, 0));
}

/// Every intensity band the table defines appears in this one real frame, and each of the twelve
/// is pinned to a cell whose rate sits at least 7 % inside its band — far enough that a
/// last-bit difference in `powf` between platforms cannot move it.
#[test]
fn the_cirrus_frame_reproduces_the_whole_intensity_ladder() {
    let geometry = cirrus_window();
    let mut warnings = Vec::new();
    let frame = opera::bake_frame_on(&cirrus_crop_contract(), &fixture(CIRRUS_CROP), geometry, VALID_AT, &mut warnings)
        .expect("the crop bakes");
    let at = |row: u32, col: u32| frame.cells[(row * geometry.width + col) as usize];
    // (row, col, native dBZ, intensity): the reflectivity ladder from a 5 dBZ trace to a 54.5 dBZ
    // core, converted with Marshall-Palmer.
    let pins = [
        (470u32, 582u32, 1u8),
        (253, 147, 2),
        (256, 149, 3),
        (241, 182, 4),
        (353, 1_149, 5),
        (348, 1_143, 6),
        (469, 695, 7),
        (486, 704, 8),
        (400, 156, 9),
        (400, 157, 10),
        (392, 187, 11),
        (501, 770, 12),
    ];
    for (row, col, intensity) in pins {
        assert_eq!(at(row, col), intensity, "cell ({row}, {col})");
    }
    let counts = histogram(&frame.cells);
    for intensity in 1..=12u8 {
        assert!(counts[usize::from(intensity)] > 0, "intensity {intensity} never appears");
    }
}

/// NIMBUS needs no Z-R relation — the samples are already mm/h — so the same ladder must come out
/// of a straight quantization, including the frame's 77 mm/h core.
#[test]
fn the_nimbus_frame_quantizes_its_native_rates() {
    let geometry = nimbus_window();
    let mut warnings = Vec::new();
    let frame = opera::bake_frame_on(&nimbus_crop_contract(), &fixture(NIMBUS_CROP), geometry, VALID_AT, &mut warnings)
        .expect("the crop bakes");
    let at = |row: u32, col: u32| frame.cells[(row * geometry.width + col) as usize];
    let pins = [
        (5u32, 110u32, 2u8),
        (1, 112, 3),
        (18, 100, 4),
        (17, 124, 5),
        (45, 46, 6),
        (0, 111, 7),
        (23, 120, 8),
        (42, 185, 9),
        (69, 105, 10),
        (67, 110, 11),
        (11, 106, 12),
    ];
    for (row, col, intensity) in pins {
        assert_eq!(at(row, col), intensity, "cell ({row}, {col})");
    }
    let counts = histogram(&frame.cells);
    assert_eq!(counts[usize::from(INTENSITY_DRY)], 165_932, "covered-but-dry cells");
    assert_eq!(counts[usize::from(INTENSITY_NODATA)], 673_714, "no-coverage cells");
    assert_eq!((counts[13], counts[14]), (0, 0));
}

/// A window the composite does not reach is entirely no-data — never dry, and never clamped onto
/// the nearest edge of the raster.
#[test]
fn a_window_outside_the_raster_is_all_no_data() {
    let geometry = window(58_000_000, -6_000_000, 32, 32, 1_000);
    let mut warnings = Vec::new();
    let frame = opera::bake_frame_on(&cirrus_crop_contract(), &fixture(CIRRUS_CROP), geometry, VALID_AT, &mut warnings)
        .expect("the crop bakes");
    assert!(frame.cells.iter().all(|cell| *cell == INTENSITY_NODATA), "a cell outside the raster is not no-data");
}

/// The coverage gate: a crop is nowhere near the domain-wide 50.3 % the network covers, so the
/// bake says so — and it says so as a warning, because a couple of national radars going out of
/// service must degrade the product, not fail the cycle.
#[test]
fn a_coverage_departure_warns_without_failing_the_cycle() {
    let mut warnings = Vec::new();
    opera::bake_frame_on(&cirrus_crop_contract(), &fixture(CIRRUS_CROP), cirrus_window(), VALID_AT, &mut warnings)
        .expect("the crop bakes");
    assert!(
        warnings.iter().any(|warning| warning.contains("radar coverage is")),
        "the coverage gate never fired: {warnings:?}"
    );
}

/// The pinned contract is what stops a swapped, re-registered or re-timed object being baked as
/// if nothing had changed. Each case below breaks exactly one clause of it.
#[test]
fn the_source_contract_refuses_what_it_is_there_to_refuse() {
    let cirrus = fixture(CIRRUS_CROP);
    let nimbus = fixture(NIMBUS_CROP);
    let bake = |contract: &Contract, bytes: &[u8], valid_at: i64| {
        let mut warnings = Vec::new();
        opera::bake_frame_on(contract, bytes, cirrus_window(), valid_at, &mut warnings).map(|_| ())
    };

    // The production contract expects the whole 3,800 x 4,400 composite, so a crop is refused —
    // the raster-size clause, exercised by the very fixtures that make the rest of this file
    // cheap.
    let error = bake(&opera_cirrus::CONTRACT, &cirrus, VALID_AT).expect_err("a crop is not the composite");
    assert!(error.contains("not the pinned 3800 x 4400"), "{error}");

    // The key and the composite's own stamp must agree.
    let error = bake(&cirrus_crop_contract(), &cirrus, VALID_AT + 300).expect_err("a re-timed object");
    assert!(error.contains("but its key claims"), "{error}");

    // A registration shifted by one native cell.
    let mut shifted = cirrus_crop_contract();
    shifted.ul_x += 1_000.0;
    let error = bake(&shifted, &cirrus, VALID_AT).expect_err("a shifted raster origin");
    assert!(error.contains("is not the pinned"), "{error}");

    // A different composite under a familiar key.
    let mut renamed = cirrus_crop_contract();
    renamed.prodname = "OPERA CIRRUS something else entirely";
    let error = bake(&renamed, &cirrus, VALID_AT).expect_err("a swapped product");
    assert!(error.contains("metadata `prodname`"), "{error}");

    // OPERA re-tuning its own Z-R relation must stop the reflectivity adapter, which borrows it.
    let patched =
        replace_once(&nimbus, b"<Item name=\"zr_a\" sample=\"0\">200.0<", b"<Item name=\"zr_a\" sample=\"0\">300.0<");
    let mut warnings = Vec::new();
    let error = opera::bake_frame_on(&nimbus_crop_contract(), &patched, nimbus_window(), VALID_AT, &mut warnings)
        .expect_err("a re-tuned Z-R relation");
    assert!(error.contains("upstream Z-R is"), "{error}");

    // `undetect` is what makes a NaN dry rather than missing; it may not silently change.
    let patched = replace_once(
        &cirrus,
        b"<Item name=\"undetect\" sample=\"0\">nan<",
        b"<Item name=\"undetect\" sample=\"0\">0.0<",
    );
    let error = bake(&cirrus_crop_contract(), &patched, VALID_AT).expect_err("a changed undetect");
    assert!(error.contains("metadata `undetect`"), "{error}");
}

/// Replace one occurrence of `needle` with an equal-length `replacement`, in place — the
/// fixtures' own discipline: negatives are derived from the real bytes in-test, never checked in.
fn replace_once(bytes: &[u8], needle: &[u8], replacement: &[u8]) -> Vec<u8> {
    assert_eq!(needle.len(), replacement.len(), "the patch must not move any tag offset");
    let at = bytes.windows(needle.len()).position(|window| window == needle).expect("the fixture contains the needle");
    let mut patched = bytes.to_vec();
    patched[at..at + needle.len()].copy_from_slice(replacement);
    patched
}

fn published(id: &str, reference_time: i64) -> Product {
    Product {
        id: id.to_string(),
        tier: 1,
        bbox_udeg: Bbox {
            south_udeg: 34_000_000,
            west_udeg: -28_000_000,
            north_udeg: 73_000_000,
            east_udeg: 36_000_000,
        },
        cell: Cell { lat_udeg: 10_000, lon_udeg: 10_000, nominal_m: 1_000 },
        reference_time: manifest::rfc3339(reference_time),
        generated_at: manifest::rfc3339(reference_time),
        staleness_deadline: manifest::rfc3339(reference_time + 1_800),
        attribution: AttributionEntry { text: "test".into(), url: "https://example.invalid".into() },
        upstream_etag: None,
        frames: Vec::new(),
    }
}

/// Discovery walks the immutable key schema backwards at the product's own cadence, and the two
/// short-circuits fire before a single byte of a 3 MB object moves: the published composite is
/// recognised by run identity, and an upstream that regresses to an older composite keeps the
/// published product rather than moving `reference_time` into the past.
#[test]
fn discovery_probes_backwards_and_both_short_circuits_fire_before_any_fetch() {
    // `now` is eleven minutes past a composite that exists; the two newer five-minute slots do
    // not, which is exactly the 4.1-minute publication lag plus a slow cycle.
    let now = VALID_AT + 11 * 60;
    let mut upstream = FixtureUpstream::default();
    upstream.declare(opera_cirrus::CONTRACT.object_url(VALID_AT), 3_563_217);
    assert_eq!(opera::discover_latest(&opera_cirrus::CONTRACT, &mut upstream, now), Ok(Some(VALID_AT)));
    assert_eq!(upstream.requests.len(), 3, "probed {:?}", upstream.requests);
    assert!(upstream.requests.iter().all(|request| request.starts_with("HEAD ")), "discovery must not GET");

    let mut warnings = Vec::new();
    let outcome = OperaCirrus
        .bake(&mut upstream, Some(&published(opera_cirrus::ID, VALID_AT)), now, &mut warnings)
        .expect("the published composite is the newest one");
    assert!(matches!(outcome, AdapterOutcome::Unchanged), "an unchanged upstream must not re-bake");
    assert!(warnings.is_empty(), "{warnings:?}");

    // Upstream regression: what is published is newer than anything the bucket now offers.
    let outcome = OperaCirrus
        .bake(&mut upstream, Some(&published(opera_cirrus::ID, VALID_AT + 3_600)), now, &mut warnings)
        .expect("a regression is not a failure");
    assert!(matches!(outcome, AdapterOutcome::Unchanged));
    assert!(warnings.iter().any(|warning| warning.contains("is older than the published")), "{warnings:?}");

    // Nothing published within the discovery window is an honest error, not an empty product.
    let mut empty = FixtureUpstream::default();
    let error = OperaCirrus.bake(&mut empty, None, now, &mut warnings).expect_err("nothing to bake");
    assert!(error.contains("no composite published within the discovery window"), "{error}");
    // Eight five-minute probes for CIRRUS, five fifteen-minute ones for NIMBUS.
    assert_eq!(empty.requests.len(), 8);
    let mut empty = FixtureUpstream::default();
    assert_eq!(opera::discover_latest(&opera_nimbus::CONTRACT, &mut empty, now), Ok(None));
    assert_eq!(empty.requests.len(), 5);
}

use obc_wx_bake::source::opera_cirrus::OperaCirrus;
