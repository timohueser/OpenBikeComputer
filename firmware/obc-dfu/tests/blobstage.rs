//! The storage-blob stage carve (`OBCU_Spec.md` §3, #1158): the CRC frame the armer writes and the
//! bootloader validates, plus the soft-peripheral metadata parse — the runtime mirror of the board
//! `build.rs` build-time asserts. Same bar as the other codecs: valid ⇒ `Some`, any tear/foreign
//! bytes ⇒ `None`, total over arbitrary input.

use obc_dfu::blobstage::{
    encode_stage_header, sp_geometry, validate_stage, MAX_BLOB_LEN, STAGE_HEADER_LEN, STAGE_LEN, STAGE_VERSION,
};

/// A synthetic soft-peripheral image with a well-formed v2 metadata header declaring the shipped
/// blob's geometry (code 15,360 · exec/data 1,536 · VRI 512), padded to `len` bytes.
fn synthetic_sp_image(len: usize) -> Vec<u8> {
    let mut blob = vec![0u8; len];
    let mut put = |i: usize, w: u32| blob[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
    put(0, 0xA005 | 2 << 16 | 1 << 20); // magic · header v2 · comm REGIF · not self-boot
    put(1, 0x2208_E33C); // platform · softperiph_id (sEMMC)
    put(3, (15_360 / 16) | ((1_536 + 512) / 16) << 16); // code size · RAM-above-code footprint
    put(6, (512 / 16) | 1_536 << 16); // VRI size · VRI offset within the RAM region
    blob
}

/// The RAM carve the shipped geometry fits (the board's `SEMMC_CARVE_BYTES`).
const RAM_CARVE: usize = 20_480;

// ==================== The stage frame ====================

#[test]
fn stage_roundtrip() {
    let blob: Vec<u8> = (0..13_636u32).map(|i| (i % 253) as u8).collect();
    let header = encode_stage_header(&blob).expect("a shipped-size blob stages");

    let mut carve = vec![0xFFu8; STAGE_LEN]; // blank-RRAM fill around the payload
    carve[..STAGE_HEADER_LEN].copy_from_slice(&header);
    carve[STAGE_HEADER_LEN..STAGE_HEADER_LEN + blob.len()].copy_from_slice(&blob);

    assert_eq!(validate_stage(&carve), Some(blob.as_slice()));
}

#[test]
fn stage_rejects_the_empty_and_the_oversized() {
    assert_eq!(encode_stage_header(&[]), None);
    assert_eq!(encode_stage_header(&vec![0u8; MAX_BLOB_LEN + 1]), None);
    // At exactly the cap it must round-trip — the cap is a fit, not a fence-post.
    let blob = vec![7u8; MAX_BLOB_LEN];
    let header = encode_stage_header(&blob).expect("a cap-sized blob stages");
    let mut carve = vec![0u8; STAGE_LEN];
    carve[..STAGE_HEADER_LEN].copy_from_slice(&header);
    carve[STAGE_HEADER_LEN..].copy_from_slice(&blob);
    assert_eq!(validate_stage(&carve), Some(blob.as_slice()));
}

#[test]
fn stage_rejects_tears_and_foreign_bytes() {
    let blob = vec![3u8; 4_000];
    let header = encode_stage_header(&blob).unwrap();
    let mut carve = vec![0u8; STAGE_LEN];
    carve[..STAGE_HEADER_LEN].copy_from_slice(&header);
    carve[STAGE_HEADER_LEN..STAGE_HEADER_LEN + blob.len()].copy_from_slice(&blob);
    assert!(validate_stage(&carve).is_some(), "baseline must validate");

    // A blank page (never staged), a torn blob byte, a torn header byte, a foreign version, a
    // length pointing past the carve — every one reads as "no blob staged".
    assert_eq!(validate_stage(&vec![0u8; STAGE_LEN]), None, "blank");
    assert_eq!(validate_stage(&vec![0xFFu8; STAGE_LEN]), None, "erased");
    let mut torn = carve.clone();
    torn[STAGE_HEADER_LEN + 100] ^= 1;
    assert_eq!(validate_stage(&torn), None, "torn blob byte");
    let mut torn = carve.clone();
    torn[0] ^= 1;
    assert_eq!(validate_stage(&torn), None, "torn magic");
    let mut torn = carve.clone();
    torn[4..6].copy_from_slice(&(STAGE_VERSION + 1).to_le_bytes());
    assert_eq!(validate_stage(&torn), None, "foreign version");
    let mut torn = carve.clone();
    torn[6..10].copy_from_slice(&(STAGE_LEN as u32).to_le_bytes());
    assert_eq!(validate_stage(&torn), None, "length past the carve");
    // Truncated mappings (a short slice) must not panic and must reject.
    assert_eq!(validate_stage(&carve[..STAGE_HEADER_LEN - 1]), None, "short slice");
    assert_eq!(validate_stage(&carve[..STAGE_HEADER_LEN + 10]), None, "carve shorter than the length");
}

// ==================== The soft-peripheral metadata ====================

#[test]
fn sp_geometry_derives_the_shipped_layout() {
    let blob = synthetic_sp_image(13_636);
    let g = sp_geometry(&blob, RAM_CARVE).expect("the shipped geometry parses");
    assert_eq!(g.code_bytes, 15_360);
    assert_eq!(g.vri_offset, 15_360 + 1_536);
    assert_eq!(g.vri_bytes, 512);
    assert_eq!(g.image_bytes, 15_360 + 1_536 + 512);
}

#[test]
fn sp_geometry_accepts_the_vendored_image() {
    // The real blob the board crate ships — the same bytes `build.rs` asserts over. Skipped only
    // if the vendored file moves; then this path (and the armer's include) must move with it.
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../obc-fw-nrf54l/vendor/semmc/semmc_firmware_v0.1.1.bin");
    let blob = std::fs::read(path).expect("the vendored sEMMC image is in the board crate");
    let g = sp_geometry(&blob, RAM_CARVE).expect("the vendored image validates at runtime");
    assert_eq!(g.image_bytes, 17_408, "the shipped image's runtime footprint");
    assert!(blob.len() <= g.code_bytes);

    // And the full armer-side round trip: stage frame + metadata, exactly what obc-boot checks.
    let header = encode_stage_header(&blob).expect("the vendored image fits the carve");
    let mut carve = vec![0u8; STAGE_LEN];
    carve[..STAGE_HEADER_LEN].copy_from_slice(&header);
    carve[STAGE_HEADER_LEN..STAGE_HEADER_LEN + blob.len()].copy_from_slice(&blob);
    let staged = validate_stage(&carve).expect("the staged frame validates");
    assert!(sp_geometry(staged, RAM_CARVE).is_some());
}

#[test]
fn sp_geometry_rejects_structural_lies() {
    let ok = synthetic_sp_image(13_636);
    assert!(sp_geometry(&ok, RAM_CARVE).is_some(), "baseline must parse");

    let word = |blob: &mut Vec<u8>, i: usize, w: u32| blob[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());

    // Wrong magic / header version / comm id / self-boot bit.
    let mut b = ok.clone();
    word(&mut b, 0, 0xA006 | 2 << 16 | 1 << 20);
    assert_eq!(sp_geometry(&b, RAM_CARVE), None, "magic");
    let mut b = ok.clone();
    word(&mut b, 0, 0xA005 | 3 << 16 | 1 << 20);
    assert_eq!(sp_geometry(&b, RAM_CARVE), None, "header version");
    let mut b = ok.clone();
    word(&mut b, 0, 0xA005 | 2 << 16 | 2 << 20);
    assert_eq!(sp_geometry(&b, RAM_CARVE), None, "comm id");
    let mut b = ok.clone();
    word(&mut b, 0, 0xA005 | 2 << 16 | 1 << 20 | 1 << 31);
    assert_eq!(sp_geometry(&b, RAM_CARVE), None, "self-boot");

    // A different soft peripheral entirely (a UART image booted as an SD host is a silent wedge).
    let mut b = ok.clone();
    word(&mut b, 1, 0x2208_0001);
    assert_eq!(sp_geometry(&b, RAM_CARVE), None, "softperiph id");
    // A *newer platform* revision of the same peripheral must still parse — flash-once bootloader.
    let mut b = ok.clone();
    word(&mut b, 1, 0x2209_E33C);
    assert!(sp_geometry(&b, RAM_CARVE).is_some(), "platform half is deliberately unpinned");

    // Inconsistent RAM footprint declarations, a file longer than its own code region, and an
    // image that outgrows the execution carve.
    let mut b = ok.clone();
    word(&mut b, 6, (512 / 16) | 1_552 << 16);
    assert_eq!(sp_geometry(&b, RAM_CARVE), None, "w3/w6 footprint disagreement");
    let mut b = ok.clone();
    word(&mut b, 3, (13_636 / 16) | ((1_536 + 512) / 16) << 16);
    assert_eq!(sp_geometry(&b, RAM_CARVE), None, "file longer than its declared code region");
    assert_eq!(sp_geometry(&ok, 17_407), None, "image must fit the execution carve");
    assert!(sp_geometry(&ok, 17_408).is_some(), "…and exactly fitting is fitting");

    // Too short to carry a header at all.
    assert_eq!(sp_geometry(&ok[..31], RAM_CARVE), None, "short blob");
}
