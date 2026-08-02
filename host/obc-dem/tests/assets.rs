//! The two committed terrain sidecars, checked as artifacts.
//!
//! `apps/obc-sim/assets/grimsel.obcd` and `host/obc-bake/assets/teningen-preview.obcd` are built
//! files whose source is not in the repo — `apps/obc-sim/assets/repack.sh terrain` is the only
//! supported way to regenerate them, exactly like the `.obcm` maps they sit beside. EL7 will run
//! its snapshot tests on them, so this file is what says they are still readable, still cover the
//! map they belong to, and still contain terrain rather than a plausible-looking accident.
//!
//! Runs offline: it reads the committed bytes and nothing else. Everything goes through
//! `obc_elevation::TerrainReader`, the same consumer the device uses.

use obc_elevation::{TerrainReader, TileCache, DEFAULT_TILE_SLOTS};
use obc_formats::io::SliceSource;

/// The pairing both sidecars are baked at: the **real** v1 posting with a 2^16 cell, so a map-sized
/// box is a few hundred KB instead of four 2 MiB blocks mostly outside the map. `OBCT_Spec.md` §1.3
/// makes both header data for this reason and §4.5 requires a reader to accept the pairing.
const POSTING_LOG2: u8 = 9;
const CELL_LOG2: u8 = 16;

fn asset(relative: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(relative);
    std::fs::read(&path)
        .unwrap_or_else(|e| panic!("{}: {e} — run `apps/obc-sim/assets/repack.sh terrain`", path.display()))
}

/// Parse a committed sidecar, check its header, and run `body` with a degree-taking sampler over it.
/// A closure rather than a returned sampler because the reader borrows its byte source.
fn with_sampler(bytes: &[u8], body: impl FnOnce(&mut dyn FnMut(f64, f64) -> Option<i16>)) {
    let src = SliceSource(bytes);
    let reader = TerrainReader::parse(&src).expect("a committed sidecar must parse as OBCT");
    let header = *reader.header();
    assert_eq!((header.posting_log2, header.cell_log2), (POSTING_LOG2, CELL_LOG2));
    assert_eq!(header.flags, 0);
    let mut cache = TileCache::<DEFAULT_TILE_SLOTS>::new();
    body(&mut |lat: f64, lon: f64| reader.sample(&mut cache, (lat * 1e6).round() as i32, (lon * 1e6).round() as i32));
}

/// The alpine sidecar: it must cover the whole of `grimsel.obcm`'s canonical crop, and the terrain
/// in it must be the Grimsel — real relief, at surveyed heights, not a flat or shifted raster.
#[test]
fn the_grimsel_sidecar_covers_its_map_and_reads_as_the_grimsel() {
    let bytes = asset("apps/obc-sim/assets/grimsel.obcd");
    assert_eq!(bytes.len(), 786_560, "20 cells of 32 KiB behind a 4 × 5 directory");

    with_sampler(&bytes, |at| {
        // The four corners of the map's canonical extract bbox (`repack.sh`'s `GRIMSEL_BBOX`).
        // Coverage is the point: a route anywhere on this map must find terrain under it.
        for (lat, lon) in [(46.48261, 8.15034), (46.48261, 8.46007), (46.72070, 8.15034), (46.72070, 8.46007)] {
            let height = at(lat, lon).unwrap_or_else(|| panic!("({lat}, {lon}) is on the map and must have terrain"));
            assert!((300..=4200).contains(&i32::from(height)), "({lat}, {lon}) read {height} m");
        }

        // Surveyed pins, through the reader — the same three the network-gated test checks end to end.
        for (name, lat, lon, surveyed) in [
            ("Grimsel Pass", 46.5611, 8.3372, 2164i32),
            ("Furka Pass", 46.5722, 8.4153, 2429),
            ("Nufenen Pass", 46.4783, 8.3878, 2478),
        ] {
            let baked = i32::from(at(lat, lon).expect("covered"));
            assert!((baked - surveyed).abs() <= 10, "{name}: sidecar says {baked} m, surveyed {surveyed} m");
        }

        // Real relief: the pass is well over a kilometre above the Haslital floor.
        let valley = i32::from(at(46.7020, 8.2300).expect("covered"));
        let pass = i32::from(at(46.5611, 8.3372).expect("covered"));
        assert!(pass - valley > 1200, "the alpine fixture must have alpine relief: {valley} m to {pass} m");

        // …and it stops where it stops. Half a degree east is another cell column entirely.
        assert_eq!(at(46.60, 9.00), None, "coverage must not be invented outside the rectangle");
    });
}

/// The skin-preview sidecar: the Rhine plain around Teningen, where the interesting property is that
/// the terrain is nearly flat and low — the opposite of the alpine one, and a check that a 2-cell
/// container is as valid as a wide one (`OBCT_Spec.md` §4.1).
#[test]
fn the_teningen_sidecar_covers_the_preview_camera() {
    let bytes = asset("host/obc-bake/assets/teningen-preview.obcd");
    assert_eq!(bytes.len(), 65_576, "2 cells of 32 KiB behind a 1 × 2 directory");

    with_sampler(&bytes, |at| {
        // The preview's published camera centre (`host/obc-bake/assets/README.md`), and the crop's
        // own corners. The Rhine plain here runs ~180–220 m.
        for (lat, lon) in [(48.130, 7.814), (48.119, 7.798), (48.119, 7.830), (48.141, 7.798), (48.141, 7.830)] {
            let height = i32::from(at(lat, lon).unwrap_or_else(|| panic!("({lat}, {lon}) must have terrain")));
            assert!((150..=350).contains(&height), "({lat}, {lon}) read {height} m on the Rhine plain");
        }
    });
}
