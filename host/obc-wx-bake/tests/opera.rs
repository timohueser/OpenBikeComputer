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
use obc_wx_bake::source::opera::{self, Contract};
use obc_wx_bake::source::opera_cirrus::OperaCirrus;
use obc_wx_bake::source::{opera_cirrus, opera_nimbus, Adapter};
use obc_wx_bake::tiff;

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
    std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

const CIRRUS_CROP: &str = "opera-cirrus-20260810T0000-dbzh-crop.tiff";
const NIMBUS_CROP: &str = "opera-nimbus-20260810T0000-rate-crop.tiff";
/// Both crops come from the composites valid at 2026-08-10T00:00Z.
const VALID_AT: i64 = 1_786_320_000;

/// The CIRRUS crop's own registration: 1,024 x 1,024 native cells starting at tile (row 5,
/// col 3) of the 3,800 x 4,400 composite, so its grid corner is the composite's corner shifted
/// 1,536,000 m east and 2,560,000 m south. Its `ModelTiepoint` carries the same half-pixel offset
/// the upstream object does (the crop copies the tag and shifts it), so `verify`'s tiepoint
/// clause is exercised here exactly as it is in production.
fn cirrus_crop_contract() -> Contract {
    Contract { width: 1_024, height: 1_024, ul_x: 1_536_000.0, ul_y: -2_560_000.0, ..opera_cirrus::CONTRACT }
}

/// The NIMBUS crop: tiles (rows 3-4, cols 1-2), which include the composite's partial southern
/// tile row — 1,024 x 664 native cells rather than a clean 1,024 x 1,024.
fn nimbus_crop_contract() -> Contract {
    Contract { width: 1_024, height: 664, ul_x: 1_024_000.0, ul_y: -3_072_000.0, ..opera_nimbus::CONTRACT }
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
    assert_eq!(counts[usize::from(INTENSITY_DRY)], 775_780, "covered-but-dry cells");
    assert_eq!(counts[usize::from(INTENSITY_NODATA)], 37_295, "no-coverage cells");
    assert_eq!(counts.iter().sum::<usize>(), cirrus_window().cells());
    // 13 and 14 are reserved; nothing may ever land there.
    assert_eq!((counts[13], counts[14]), (0, 0));
}

/// Eleven of the twelve intensity bands appear in this one real frame, each pinned to a cell
/// whose rate sits at least 7 % inside its band — far enough that a last-bit difference in
/// `powf` between platforms cannot move it.
///
/// The frame's strongest echo is 54.5 dBZ, which is 42.2 mm/h after the column-max correction and
/// so lands in band 11; before the correction it was 92.9 mm/h and band 12. That is the review's
/// "a full intensity band" made concrete, at the top of the scale.
#[test]
fn the_cirrus_frame_reproduces_the_intensity_ladder_after_the_column_max_correction() {
    let geometry = cirrus_window();
    let mut warnings = Vec::new();
    let frame = opera::bake_frame_on(&cirrus_crop_contract(), &fixture(CIRRUS_CROP), geometry, VALID_AT, &mut warnings)
        .expect("the crop bakes");
    let at = |row: u32, col: u32| frame.cells[(row * geometry.width + col) as usize];
    // (row, col, intensity), native reflectivity in the comment: the ladder from an 11.5 dBZ
    // trace to a 54.5 dBZ core, through Marshall-Palmer and MAX_TO_SURFACE_RATIO.
    let pins = [
        (253u32, 146u32, 1u8), // 11.5 dBZ -> 0.087 mm/h
        (255, 149, 2),         // 14.5 -> 0.134
        (241, 182, 3),         // 20.5 -> 0.317
        (353, 1_149, 4),       // 25.5 -> 0.650
        (348, 1_143, 5),       // 29.0 -> 1.076
        (269, 1_029, 6),       // 35.5 -> 2.743
        (497, 664, 7),         // 39.5 -> 4.877
        (400, 157, 8),         // 41.5 -> 6.504
        (500, 695, 9),         // 45.0 -> 10.763
        (500, 770, 10),        // 49.5 -> 20.568
        (501, 770, 11),        // 54.5 -> 42.236, band 12 before the correction
    ];
    for (row, col, intensity) in pins {
        assert_eq!(at(row, col), intensity, "cell ({row}, {col})");
    }
    let counts = histogram(&frame.cells);
    for intensity in 1..=11u8 {
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
        (4u32, 111u32, 2u8), // 0.22 mm/h
        (0, 113, 3),         // 0.29
        (17, 101, 4),        // 0.92
        (16, 125, 5),        // 1.22
        (44, 47, 6),         // 3.44
        (22, 121, 8),        // 9.16
        (41, 186, 9),        // 13.70
        (68, 107, 10),       // 21.72
        (66, 111, 11),       // 38.62
        (10, 107, 12),       // 77.06
    ];
    for (row, col, intensity) in pins {
        assert_eq!(at(row, col), intensity, "cell ({row}, {col})");
    }
    let counts = histogram(&frame.cells);
    assert_eq!(counts[usize::from(INTENSITY_DRY)], 166_808, "covered-but-dry cells");
    assert_eq!(counts[usize::from(INTENSITY_NODATA)], 672_842, "no-coverage cells");
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
    assert!(error.contains("is not the pinned grid corner"), "{error}");

    // The half-pixel itself, in both directions. Pinning the grid at the file's own tiepoint —
    // the registration this adapter shipped with before review round 1 — must be refused, and so
    // must a file whose tiepoint has *stopped* carrying the offset (OPERA fixing its converter).
    let mut at_tiepoint = cirrus_crop_contract();
    at_tiepoint.ul_x -= 500.0;
    at_tiepoint.ul_y += 500.0;
    let error = bake(&at_tiepoint, &cirrus, VALID_AT).expect_err("the tiepoint is not the grid corner");
    assert!(error.contains("less half a pixel"), "{error}");

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

    // The ODIM `product` item is the fact the column-max correction is conditioned on: CIRRUS
    // turning into a near-surface PPI would make that correction wrong, so it must stop the bake
    // rather than silently re-point the calibration at a different measured quantity.
    let patched = replace_once(&cirrus, b"<Item name=\"product\">MAX<", b"<Item name=\"product\">PPI<");
    let error = bake(&cirrus_crop_contract(), &patched, VALID_AT).expect_err("a changed vertical sampling");
    assert!(error.contains("ODIM `product`"), "{error}");

    // …and the other half of that guard: CIRRUS carries no Z-R declaration today. If one appears,
    // OPERA has taken its own position on converting a column max and ours must be revisited.
    let patched = replace_once(
        &cirrus,
        b"<Item name=\"publisher_type\">institution</Item>",
        b"<Item name=\"zr_a\">300.0</Item>                ",
    );
    let error = bake(&cirrus_crop_contract(), &patched, VALID_AT).expect_err("an upstream CIRRUS Z-R");
    assert!(error.contains("declares its own Z-R"), "{error}");

    // The second plane is validated by identity, not merely by `SamplesPerPixel = 2`.
    let patched = replace_once(&cirrus, b"pl.imgw.quality.qi_total", b"pl.imgw.quality.xx_total");
    let error = bake(&cirrus_crop_contract(), &patched, VALID_AT).expect_err("a replaced quality index");
    assert!(error.contains("band 1 task"), "{error}");
}

/// `metadata_item` returns **band 0's** value, and that is load-bearing rather than incidental:
/// GDAL writes the per-sample items once per band in band order, so `DESCRIPTION` and
/// `_FillValue` each appear twice in these objects and only the first describes the band this
/// decoder reads. A change to first-match-wins would silently start validating the quality band.
#[test]
fn metadata_lookups_answer_for_band_zero_only() {
    let cog = tiff::decode_band0(&fixture(CIRRUS_CROP)).expect("the crop decodes");
    assert_eq!(tiff::metadata_item(&cog.metadata, "DESCRIPTION"), Some("DBZH"));
    assert!(cog.metadata.contains("quality1"), "the fixture must actually carry a band-1 description");
    assert_eq!(tiff::metadata_item(&cog.metadata, "_FillValue"), Some("-9.999e+06"));
    // A prefix of a real item name must not match it.
    assert_eq!(tiff::metadata_item(&cog.metadata, "DESCRIPT"), None);
    assert_eq!(tiff::metadata_item(&cog.metadata, "prod"), None);
}

/// Both real products carry `pl.imgw.quality.qi_total`, but its population is product-specific:
/// CIRRUS supplies QI only where it has a finite echo, while NIMBUS supplies it for every pixel.
/// That is why the production path validates and summarizes band 1 but does not apply a generic
/// threshold. In this Alpine CIRRUS crop, filtering `QI == 0` alone would erase 28,821 of 36,631
/// echo cells; filtering missing QI would also turn all 984,300 covered-dry pixels into no-data.
#[test]
fn the_real_quality_planes_are_validated_without_a_destructive_generic_threshold() {
    let cirrus = tiff::decode_band0(&fixture(CIRRUS_CROP)).expect("the CIRRUS crop decodes");
    let quality = cirrus.band1.expect("CIRRUS quality plane");
    assert_eq!(quality.finite, 36_631);
    assert_eq!(quality.nodata, 1_011_945);
    assert_eq!((quality.zero, quality.one), (28_821, 5_011));
    assert_eq!((quality.non_finite, quality.outside_unit_interval), (0, 0));
    assert_eq!(tiff::metadata_item_for_sample(&cirrus.metadata, "task", 1), Some("pl.imgw.quality.qi_total"));
    assert_eq!(tiff::metadata_item_for_sample(&cirrus.metadata, "DESCRIPTION", 1), Some("quality1"));

    let nimbus = tiff::decode_band0(&fixture(NIMBUS_CROP)).expect("the NIMBUS crop decodes");
    let quality = nimbus.band1.expect("NIMBUS quality plane");
    assert_eq!(quality.finite, 679_936);
    assert_eq!(quality.nodata, 0);
    assert_eq!((quality.zero, quality.one), (375_722, 200_710));
    assert_eq!((quality.non_finite, quality.outside_unit_interval), (0, 0));
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

/// Discovery walks the immutable key schema backwards at the source's own cadence, and it does it
/// with HEAD probes only — so finding the newest composite costs requests and no body bytes, and
/// finding nothing is an honest error rather than an empty source.
///
/// The unchanged and regression short-circuits this used to also cover are gone with #1246: they
/// compared against a previously *published product entry*, and the mosaic needs every source's
/// cells every cycle, so there is nothing to short-circuit against and nothing that would be saved
/// by it.
#[test]
fn discovery_probes_backwards_with_head_requests_and_fails_honestly() {
    // `now` is eleven minutes past a composite that exists; the two newer five-minute slots do
    // not, which is exactly the 4.1-minute publication lag plus a slow cycle.
    let now = VALID_AT + 11 * 60;
    let mut upstream = FixtureUpstream::default();
    upstream.declare(opera_cirrus::CONTRACT.object_url(VALID_AT), 3_563_217);
    assert_eq!(opera::discover_latest(&opera_cirrus::CONTRACT, &mut upstream, now), Ok(Some(VALID_AT)));
    assert_eq!(upstream.requests.len(), 3, "probed {:?}", upstream.requests);
    assert!(upstream.requests.iter().all(|request| request.starts_with("HEAD ")), "discovery must not GET");

    // Nothing published within the discovery window is an honest error, not an empty source.
    let mut warnings = Vec::new();
    let mut empty = FixtureUpstream::default();
    let error = OperaCirrus.bake(&mut empty, now, &mut warnings).expect_err("nothing to bake");
    assert!(error.contains("no composite published within the discovery window"), "{error}");
    // Eight five-minute probes for CIRRUS, five fifteen-minute ones for NIMBUS.
    assert_eq!(empty.requests.len(), 8);
    let mut empty = FixtureUpstream::default();
    assert_eq!(opera::discover_latest(&opera_nimbus::CONTRACT, &mut empty, now), Ok(None));
    assert_eq!(empty.requests.len(), 5);
}
