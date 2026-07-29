//! D2 (#907): the vendored-GEOS proof, and the only thing that stops
//! `vendored-geos` from being a claim in a `Cargo.toml` comment.
//!
//! A successful *link* proves nothing interesting. libGEOS is a C++ library reached
//! through a C ABI, so a build can link, start, and then fail the first time it
//! assembles a multipolygon — and on Windows it can link against a GEOS that isn't
//! there at run time at all. So this suite runs the geometry, on whatever platform
//! CI is standing on, and pins the answer.
//!
//! It is the `desktop` job's smoke test on all three platforms. The fixture is the
//! checked-in `tiny.osm.pbf` rather than a downloaded Monaco extract for one
//! reason: **it is the same bytes everywhere and forever**, so the assertion can be
//! a digest rather than "something came out". A Geofabrik extract changes daily and
//! could only ever support the weaker claim.

use std::path::{Path, PathBuf};

use obc_pack::config::Config;
use obc_pack::pipeline::{pack, PackOptions};
use obc_pack::progress::Progress;
use sha2::{Digest, Sha256};

/// The `.obcm` that `builder/tests/corpus/data/tiny.osm.pbf` + `presets/default.json`
/// must produce, byte for byte, on every platform the app ships to.
///
/// Pinning this is a *cross-platform determinism* assertion, which is worth more
/// here than a per-platform "it packed something": one number covers "GEOS ran",
/// "GEOS was the version we vendored" (an older system libGEOS satisfying the
/// runtime check would assemble areas differently), and "x86_64 and arm64 agree".
///
/// If this fails, do not reach for the recorded digest — find out *which* claim
/// broke. `host/obc-pack`'s `the_cli_and_the_library_produce_the_same_bytes`
/// packs the same fixture against system GEOS and is the control.
/// Recorded on macOS arm64 against **both** the vendored static GEOS 3.14.1 and a
/// system libgeos 3.14.1 dylib — they agree, which is why this is pinned rather
/// than merely printed.
///
/// Re-pinned for default preset **v4** (the bikepacking restyle: 7-tier LOD
/// pyramid + footprint culling + merge_lines), which legitimately changes the
/// packed bytes; all three CI platforms produced this same digest (30 620 bytes).
const TINY_OBCM_SHA256: &str = "9a5aacb147b731d9a8edc7bfef172850599b27500dcee6659c5034a69d2764a4";

fn repo(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel)
}

/// Lowercase hex, the same shape `obc-pack`'s catalog digests take.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn out_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("obc-desktop-geos-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// The geometry work, end to end, through the libGEOS this binary actually links.
///
/// `no_land` because the land stage wants a 950 MB download; every *geometry*
/// primitive the packer uses — `build_area` for relation multipolygons, `node`,
/// `unary_union`, `line_merge`, `topology_preserve_simplify`, `intersection` for
/// the quadtree clip — is reached from the fixture without it.
#[test]
fn the_linked_geos_packs_the_fixture_to_the_expected_bytes() {
    let dir = out_dir("pack");
    let out = dir.join("tiny.obcm");
    let config = Config::load(&repo("builder/presets/default.json").to_string_lossy()).expect("preset parses");
    let summary = pack(
        &[repo("builder/tests/corpus/data/tiny.osm.pbf").to_string_lossy().into_owned()],
        &config,
        &out,
        &PackOptions { no_land: true, ..PackOptions::default() },
        &Progress::silent(),
    )
    .expect("pack the fixture");

    let bytes = std::fs::read(&out).expect("read the map back");
    assert_eq!(&bytes[..4], b"OBCM", "not an OBCM container");
    assert_eq!(bytes.len() as u64, summary.bytes, "the summary must report what was written");
    assert_eq!(summary.dropped, 0, "the fixture must fit its chunks — {} features were dropped", summary.dropped);

    let digest = hex(&Sha256::digest(&bytes));
    // Printed unconditionally (`cargo test -- --nocapture`) so a CI log records the
    // per-platform answer even on the run where the assertion below is what failed.
    println!("tiny.obcm sha256 = {digest} ({} bytes)", bytes.len());
    assert_eq!(digest, TINY_OBCM_SHA256, "the packed map differs from every other platform's");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The version check the `v3_14_0` feature cannot make. That feature only selects
/// which pre-generated bindings are compiled; whether the *library* behind them is
/// 3.14 is a build-time probe in `geos-sys` that a `GEOS_VERSION` env var can lie
/// to, and is not checked again at run time. `build_area` is 3.14-only API, so a
/// mismatch here is the difference between a working app and a crash on the first
/// relation with a hole.
#[cfg(feature = "vendored-geos")]
#[test]
fn geos_version_is_the_one_obc_pack_asked_for() {
    let version = geos::version().expect("libGEOS reports its version");
    println!("libGEOS = {version}");
    assert!(
        version.starts_with("3.14"),
        "linked libGEOS is {version}, not the 3.14 `obc-pack` compiles against — \
         a vendored build should be exactly geos-src's pinned 3.14.1"
    );
}
