//! Structured decode fuzzing: mutated upstream bytes must produce an error, never a panic.
//!
//! Deterministic (fixed xorshift seed) so a failure reproduces exactly. This is the WX5/WX6
//! "every decode is bounds-checked and fuzzed" gate over every adapter's full decode path — tar
//! + ODIM HDF5 for DWD RV, bzip2 + GRIB2/CCSDS for ICON-EU, gzip + GRIB2/PNG for MRMS, and the
//! byte-range GRIB2 complex-packing paths of HRRR and GFS.

use std::path::PathBuf;

use obc_wx_bake::grib::{decode_bzip2_field, decode_field, decode_gzip_field};
use obc_wx_bake::idx;
use obc_wx_bake::source::{dwd_rv, gfs, hrrr, icon_eu, mrms};

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
    std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

struct XorShift(u32);

impl XorShift {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }
}

/// Mutate `bytes` with 1..=8 random byte replacements and an occasional truncation.
fn mutate(bytes: &[u8], rng: &mut XorShift) -> Vec<u8> {
    let mut mutated = bytes.to_vec();
    for _ in 0..(rng.next() % 8 + 1) {
        let index = (rng.next() as usize) % mutated.len();
        mutated[index] = (rng.next() & 0xFF) as u8;
    }
    if rng.next().is_multiple_of(4) {
        let keep = (rng.next() as usize) % (mutated.len() + 1);
        mutated.truncate(keep);
    }
    mutated
}

#[test]
fn mutated_rv_tars_error_and_never_panic() {
    let good = fixture("composite_rv_20260809_1420.tar");
    let mut rng = XorShift(0x1190_0001);
    let mut rejected = 0usize;
    for _ in 0..24 {
        let mutated = mutate(&good, &mut rng);
        if dwd_rv::bake_tar(&mutated).is_err() {
            rejected += 1;
        }
    }
    // A mutation can land in tar padding or an unread gap; most must be rejected, none may panic.
    assert!(rejected >= 18, "only {rejected}/24 mutated tars were rejected");
    // Degenerate shapes.
    for garbage in [vec![], vec![0u8; 5], vec![0xFF; 10_000]] {
        assert!(dwd_rv::bake_tar(&garbage).is_err());
    }
}

#[test]
fn mutated_icon_leads_error_and_never_panic() {
    let expected = icon_eu::EXPECTED;
    let good = fixture("icon-eu-2026080906_002.grib2.bz2");
    let mut rng = XorShift(0x1190_0002);
    let mut rejected = 0usize;
    for _ in 0..48 {
        let mutated = mutate(&good, &mut rng);
        if decode_bzip2_field(&mutated, &expected).is_err() {
            rejected += 1;
        }
    }
    // bzip2 + GRIB CRC/structure catch essentially everything; none may panic.
    assert!(rejected >= 44, "only {rejected}/48 mutated leads were rejected");
    for garbage in [vec![], vec![0u8; 5], vec![0x42; 10_000]] {
        assert!(decode_bzip2_field(&garbage, &expected).is_err());
    }
}

#[test]
fn mutated_mrms_objects_error_and_never_panic() {
    let expected = mrms::EXPECTED;
    let good = fixture("mrms-conus-20260809-165800.grib2.gz");
    let mut rng = XorShift(0x1191_0003);
    let mut rejected = 0usize;
    for _ in 0..24 {
        let mutated = mutate(&good, &mut rng);
        if decode_gzip_field(&mutated, &expected).is_err() {
            rejected += 1;
        }
    }
    // gzip's CRC plus the GRIB structure catch everything; none may panic.
    assert!(rejected >= 22, "only {rejected}/24 mutated MRMS objects were rejected");
    for garbage in [vec![], vec![0u8; 5], vec![0x1f; 10_000]] {
        assert!(decode_gzip_field(&garbage, &expected).is_err());
    }
    // A whole GRIB from a different source never satisfies this contract either.
    assert!(decode_field(&fixture("hrrr-conus-20260809T15-prate-t120.grib2"), &expected).is_err());
}

#[test]
fn mutated_range_messages_error_and_never_panic() {
    let hrrr_expected = hrrr::EXPECTED;
    let gfs_expected = gfs::EXPECTED;
    let hrrr_good = fixture("hrrr-conus-20260809T15-prate-t120.grib2");
    let gfs_good = fixture("gfs-global-20260809T12-apcp-f001.grib2");
    let mut rng = XorShift(0x1191_0004);
    let mut hrrr_rejected = 0usize;
    let mut gfs_rejected = 0usize;
    for _ in 0..32 {
        if decode_field(&mutate(&hrrr_good, &mut rng), &hrrr_expected).is_err() {
            hrrr_rejected += 1;
        }
        if decode_field(&mutate(&gfs_good, &mut rng), &gfs_expected).is_err() {
            gfs_rejected += 1;
        }
    }
    assert!(hrrr_rejected >= 28, "only {hrrr_rejected}/32 mutated HRRR messages were rejected");
    assert!(gfs_rejected >= 28, "only {gfs_rejected}/32 mutated GFS spans were rejected");
    for garbage in [vec![], vec![0u8; 5], vec![0x47; 10_000]] {
        assert!(decode_field(&garbage, &hrrr_expected).is_err());
        assert!(decode_field(&garbage, &gfs_expected).is_err());
    }
    // Cross-contract: neither source's bytes satisfy the other's pinned geometry.
    assert!(decode_field(&hrrr_good, &gfs_expected).is_err());
    assert!(decode_field(&gfs_good, &hrrr_expected).is_err());
}

/// The `.idx` selection layer is pure text parsing over untrusted upstream bytes: mutate a real
/// index and require an error or a range that still lies inside the object — never a panic.
#[test]
fn mutated_indexes_error_or_stay_inside_the_object() {
    let good = fixture("gfs-global-20260809T12-f001.idx");
    let object_len = 537_540_348u64;
    let mut rng = XorShift(0x1191_0005);
    for _ in 0..256 {
        let mutated = mutate(&good, &mut rng);
        let text = String::from_utf8_lossy(&mutated).into_owned();
        if let Ok((range, matched)) = idx::resolve(&text, &gfs::selector(1), object_len, &[1, 2]) {
            assert!(range.start < range.end_inclusive && range.end_inclusive < object_len);
            assert!(matched == 1 || matched == 2);
        }
    }
    assert!(idx::resolve("", &gfs::selector(1), object_len, &[1, 2]).is_err());
}
