//! Shared-fixture pin for the OBCU update container (`OBCU_Spec.md` §1).
//!
//! The checked-in `specs/vectors/update-container-v1.bin` is a full `UPDATE.BIN`
//! (64-byte header + 128-byte raw image). This test decodes it through the
//! production [`ImageHeader::decode`] and verifies both CRCs — the same bytes the
//! iOS companion's `OBCUHeader` decoder pins in `OBCUHeaderTests` and the
//! `obc-vectors` builder regenerates. A drift on either side goes red, so the file
//! is the contract between the firmware and the app.

use obc_dfu::{crc32, ImageHeader, HEADER_LEN};

/// `specs/vectors/update-container-v1.bin`, resolved from this crate's root
/// (`firmware/obc-dfu` → repo root).
fn container() -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../specs/vectors/update-container-v1.bin");
    std::fs::read(&path).unwrap_or_else(|e| {
        panic!("fixture {} unreadable ({e}) — run `cargo test -p obc-vectors regenerate -- --ignored`", path.display())
    })
}

/// The fixture header decodes to the pinned fields, and the raw-image CRC in the
/// header matches a fresh CRC over the body — the full "verify before trust" the
/// armer and the app both run.
#[test]
fn update_container_decodes_and_both_crcs_match() {
    let bytes = container();
    assert!(bytes.len() > HEADER_LEN, "container carries a header + image");

    let mut hdr = [0u8; HEADER_LEN];
    hdr.copy_from_slice(&bytes[..HEADER_LEN]);
    let header = ImageHeader::decode(&hdr).expect("valid OBCU header decodes to Some");

    // Pinned decoded facts (mirror the `obc-vectors` builder + the Swift test).
    assert_eq!(header.fw_version_str(), "1.2.0+abc1234");
    assert_eq!(header.image_len, 128);

    // The header's stored image CRC covers exactly the bytes after the header.
    let image = &bytes[HEADER_LEN..];
    assert_eq!(image.len(), header.image_len as usize);
    assert_eq!(crc32(image), header.image_crc32, "image CRC-32 matches the body");

    // Re-encoding the decoded header reproduces the fixture's 64 header bytes.
    assert_eq!(header.encode(), hdr, "header round-trips byte-for-byte");
}

/// A single flipped body byte breaks the image CRC (the app rejects it in the
/// picker, the armer before erase) even though the header itself still decodes.
#[test]
fn corrupt_image_body_fails_the_image_crc() {
    let mut bytes = container();
    let mut hdr = [0u8; HEADER_LEN];
    hdr.copy_from_slice(&bytes[..HEADER_LEN]);
    let header = ImageHeader::decode(&hdr).expect("header still valid");

    let last = bytes.len() - 1;
    bytes[last] ^= 0xFF;
    assert_ne!(crc32(&bytes[HEADER_LEN..]), header.image_crc32);
}
