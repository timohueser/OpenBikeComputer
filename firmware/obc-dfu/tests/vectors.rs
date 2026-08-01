//! Shared-fixture pins for the OBCU update container (`OBCU_Spec.md` §1).
//!
//! Two checked-in files, both full `UPDATE.BIN`s over the same 128-byte image:
//! `specs/vectors/update-container-v1.bin` (unsigned) and
//! `update-container-v2.bin` (Ed25519-signed, #997). These tests decode both
//! through the production [`ImageHeader::decode`], verify both CRCs, verify the v2
//! signature, and assert the two headers agree on every v1 field — the same bytes
//! the iOS companion's `OBCUHeader` decoder pins in `OBCUHeaderTests` and the
//! `obc-vectors` builder regenerates. A drift on either side goes red, so the files
//! are the contract between the firmware and the app.

use obc_dfu::sig::test_key;
use obc_dfu::{crc32, verify_image, ImageHeader, HEADER_LEN, SIG_LEN, SIG_SCHEME_ED25519, SIG_SCHEME_NONE};

/// A fixture under `specs/vectors/`, resolved from this crate's root
/// (`firmware/obc-dfu` → repo root).
fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../specs/vectors").join(name);
    std::fs::read(&path).unwrap_or_else(|e| {
        panic!("fixture {} unreadable ({e}) — run `cargo test -p obc-vectors regenerate -- --ignored`", path.display())
    })
}

fn container() -> Vec<u8> {
    fixture("update-container-v1.bin")
}

fn container_v2() -> Vec<u8> {
    fixture("update-container-v2.bin")
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
    assert_eq!(header.sig_scheme, SIG_SCHEME_NONE, "the v1 fixture is unsigned");
    assert_eq!(header.sig_len, 0);

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

/// The v2 fixture: decodes, both CRCs match, the scheme marker is where §1.1 says, and the trailer
/// verifies against the committed test key.
#[test]
fn signed_container_decodes_and_verifies() {
    let bytes = container_v2();
    assert_eq!(bytes.len(), HEADER_LEN + 128 + SIG_LEN, "header + image + trailer");

    let mut hdr = [0u8; HEADER_LEN];
    hdr.copy_from_slice(&bytes[..HEADER_LEN]);
    let header = ImageHeader::decode(&hdr).expect("a signed OBCU header decodes to Some");

    assert_eq!(header.fw_version_str(), "1.2.0+abc1234");
    assert_eq!(header.image_len, 128);
    assert_eq!(header.sig_scheme, SIG_SCHEME_ED25519);
    assert_eq!(header.sig_len as usize, SIG_LEN);
    assert_eq!(header.container_len(), bytes.len() as u64);
    assert_eq!(header.sig_offset(), (HEADER_LEN + 128) as u64);

    let image = &bytes[HEADER_LEN..HEADER_LEN + header.image_len as usize];
    assert_eq!(crc32(image), header.image_crc32, "image CRC-32 matches the body");

    let signature = &bytes[header.sig_offset() as usize..];
    assert_eq!(verify_image(&test_key::PUBLIC, &header, image, signature), Ok(()));

    // Re-encoding the decoded header reproduces the fixture's 64 header bytes.
    assert_eq!(header.encode(), hdr, "header round-trips byte-for-byte");
}

/// The cross-implementation form of the flash-once guarantee: the two fixtures carry the same image
/// and agree on **every byte a v1 reader looks at**. Any decoder — the firmware's, the bootloader's,
/// the iOS companion's — must read v1 and v2 identically outside bytes `48..52` and the header CRC.
#[test]
fn the_two_fixtures_agree_on_every_v1_field() {
    let v1 = container();
    let v2 = container_v2();

    assert_eq!(&v1[0..48], &v2[0..48], "magic, header version, image_len, image_crc32, fw_version");
    assert_eq!(&v1[52..60], &v2[52..60], "the still-reserved tail of the reserved run");
    assert_ne!(&v1[48..52], &v2[48..52], "…only the scheme marker differs");
    assert_eq!(&v1[HEADER_LEN..], &v2[HEADER_LEN..HEADER_LEN + 128], "the image bytes are in the same place");
    assert_eq!(v2.len() - v1.len(), SIG_LEN, "v2 adds exactly the trailer");

    let h1 = ImageHeader::decode(v1[..HEADER_LEN].try_into().unwrap()).unwrap();
    let h2 = ImageHeader::decode(v2[..HEADER_LEN].try_into().unwrap()).unwrap();
    assert_eq!(h1, h2.unsigned(), "clearing the marker recovers the v1 header exactly");
}
