//! Shared-fixture pin for the OBCU update container: the checked-in signed `UPDATE.BIN`, decoded
//! here through the production decoder. The web planner and the iOS companion pin the same bytes,
//! so the file is the contract between the firmware and the apps.

use obc_dfu::sig::test_key;
use obc_dfu::{crc32, verify_image, ImageHeader, HEADER_LEN, SIG_LEN, SIG_SCHEME_ED25519};

fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../specs/vectors").join(name);
    std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "fixture {} unreadable ({e}) — run `cargo run -p obc-vectors --example regenerate --locked`",
            path.display()
        )
    })
}

fn container() -> Vec<u8> {
    fixture("update-container.bin")
}

/// The signed fixture: both CRCs match, the scheme marker is at its pinned offset, and the trailer
/// verifies against the committed test key.
#[test]
fn signed_container_decodes_and_verifies() {
    let bytes = container();
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
