//! Structured decode fuzzing: mutated upstream bytes must produce an error, never a panic.
//!
//! Deterministic (fixed xorshift seed) so a failure reproduces exactly. This is the WX5
//! "every decode is bounds-checked and fuzzed" gate over the two adapters' full decode paths —
//! tar + ODIM HDF5 for DWD RV, bzip2 + GRIB2/CCSDS for ICON-EU.

use std::path::PathBuf;

use obc_wx_bake::grib::{decode_bzip2_field, ExpectedGrib, ICON_EU_GRID_DEFINITION_HEX};
use obc_wx_bake::source::dwd_rv;

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
    let expected = ExpectedGrib {
        discipline: 0,
        category: 1,
        parameter: 52,
        grid_template: 0,
        expected_points: 904_689,
        expected_grid_definition_hex: ICON_EU_GRID_DEFINITION_HEX,
        product_template: 8,
        representation_templates: &[42],
    };
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
