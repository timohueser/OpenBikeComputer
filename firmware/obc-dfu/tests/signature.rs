//! Host tests for the OBCU **v2 signature** (`OBCU_Spec.md` §1.3/§1.4, epic #773 / #997) — the
//! armer's accept/reject matrix, and the compatibility guarantee that keeps a **flash-once**
//! bootloader able to install v2 containers.
//!
//! Two halves, both at the settings-codec bar (every reject is its own named case, driven through
//! the production code path with mock IO — nothing here reimplements `scan`):
//!
//! 1. **[Armer vectors](#armer-vectors)** — one signed container, then one mutation per attack:
//!    a flipped payload byte, a re-labelled version, a re-labelled length, a truncated trailer, a
//!    plain v1/unsigned wrapper, a signature by the wrong key, a garbage signature.
//! 2. **v1 ↔ v2 offset compatibility** — a **from-the-spec-text**
//!    reimplementation of the v1 header decoder (the one a bootloader flashed before #997 is
//!    running) decodes a v2 header to byte-identical values, and the real install engine flashes a
//!    v2 container end to end. This is an executable form of the compatibility argument, not a
//!    comment claiming it.

use obc_dfu::armer::{scan, ExtentsError, ScanError, StageIo};
use obc_dfu::engine::IoError;
use obc_dfu::sig::test_key;
use obc_dfu::{
    sign_image, signing_prefix, Extent, ImageHeader, PublicKey, HEADER_LEN, MAX_EXTENTS, SIG_CONTEXT, SIG_LEN,
    SIG_PREFIX_LEN, SIG_SCHEME_ED25519, SIG_SCHEME_NONE,
};

// ==================== The staged-file fake ====================

/// An in-memory `UPDATE.BIN` the real [`scan`] reads through, byte for byte.
struct Stage {
    bytes: Vec<u8>,
}

impl StageIo for Stage {
    fn stage_len(&mut self) -> Option<u32> {
        Some(self.bytes.len() as u32)
    }
    fn read_stage(&mut self, offset: u32, buf: &mut [u8]) -> Result<(), IoError> {
        let start = offset as usize;
        self.bytes.get(start..start + buf.len()).map(|s| buf.copy_from_slice(s)).ok_or(IoError)
    }
    fn stage_extents(&mut self, out: &mut [Extent; MAX_EXTENTS]) -> Result<usize, ExtentsError> {
        out[0] = Extent { start_block: 100, blocks: (self.bytes.len() as u32).div_ceil(512) };
        Ok(1)
    }
}

/// A representative raw image — long enough that the scan's chunked pass has real tails.
fn image() -> Vec<u8> {
    (0..9_000u32).map(|i| (i.wrapping_mul(31) % 251) as u8).collect()
}

/// Wrap `image` into a signed OBCU v2 container under `seed`.
fn signed_container(image: &[u8], version: &str, seed: &[u8; 32]) -> Vec<u8> {
    let header = ImageHeader::new(image, version).signed();
    let mut bytes = header.encode().to_vec();
    bytes.extend_from_slice(image);
    bytes.extend_from_slice(&sign_image(seed, &header, image));
    bytes
}

/// Wrap `image` into a plain, unsigned **v1** container — exactly the bytes `obc-mkimage wrap`
/// produced before #997.
fn v1_container(image: &[u8], version: &str) -> Vec<u8> {
    let header = ImageHeader::new(image, version);
    let mut bytes = header.encode().to_vec();
    bytes.extend_from_slice(image);
    bytes
}

/// Run the production scan over `bytes`, trusting the committed test key.
fn scan_bytes(bytes: Vec<u8>) -> Result<obc_dfu::StagedRef, ScanError> {
    scan_bytes_with(bytes, &test_key::PUBLIC)
}

fn scan_bytes_with(bytes: Vec<u8>, key: &PublicKey) -> Result<obc_dfu::StagedRef, ScanError> {
    // An awkward chunk size on purpose: the signature hash and the CRC must agree about the tail.
    let mut chunk = [0u8; 96];
    scan(&mut Stage { bytes }, &mut chunk, key)
}

/// Re-seal a container's header after the caller mutated a field: recompute the header CRC so the
/// mutation reaches the *signature* check instead of dying at `BadHeader`. This is the strong form
/// of each attack — an attacker controls the CRC.
fn reseal(bytes: &mut [u8], mutate: impl FnOnce(&mut ImageHeader)) {
    let hdr: &[u8; HEADER_LEN] = bytes[..HEADER_LEN].try_into().unwrap();
    let mut header = ImageHeader::decode(hdr).expect("still a valid header");
    mutate(&mut header);
    bytes[..HEADER_LEN].copy_from_slice(&header.encode());
}

// ==================== The signed-message layout ====================

/// The normative byte layout of `OBCU_Spec.md` §1.3, asserted against a hand-built message rather
/// than against `signing_prefix`'s own output — the spec text and the code have to agree.
#[test]
fn signed_message_is_the_spec_bytes() {
    let img = image();
    let header = ImageHeader::new(&img, "v1.4.0-3-gcafe123").signed();

    let mut expected = Vec::new();
    expected.extend_from_slice(b"OBCUv2-sig\0"); // 11 bytes, the NUL included
    expected.extend_from_slice(&header.fw_version); // header[16..48], raw + NUL-padded
    expected.extend_from_slice(&header.image_len.to_le_bytes()); // header[8..12], u32 LE

    assert_eq!(SIG_CONTEXT, b"OBCUv2-sig\0");
    assert_eq!(SIG_PREFIX_LEN, 11 + 32 + 4);
    assert_eq!(signing_prefix(&header).as_slice(), expected.as_slice());
    // …and nothing else: the message is prefix ‖ image, so the total is 47 + image_len.
    assert_eq!(SIG_PREFIX_LEN + img.len(), 47 + 9_000);
}

// ==================== Armer vectors ====================

#[test]
fn armer_accepts_a_correctly_signed_v2_container() {
    let img = image();
    let staged = scan_bytes(signed_container(&img, "v1.4.0", &test_key::SEED)).expect("a signed v2 container arms");
    assert_eq!(staged.len, img.len() as u32);
    assert!(staged.header.is_signed());
    assert_eq!(staged.header.sig_scheme, SIG_SCHEME_ED25519);
    assert_eq!(staged.header.sig_len as usize, SIG_LEN);
    assert_eq!(staged.header.fw_version_str(), "v1.4.0");
}

#[test]
fn armer_rejects_a_bitflipped_payload() {
    // A flipped image byte breaks the CRC first — the rider is told the file is damaged (true) and
    // not that it is forged (misleading). The signature would have caught it either way.
    let img = image();
    let mut bytes = signed_container(&img, "v1.4.0", &test_key::SEED);
    bytes[HEADER_LEN + 4_321] ^= 0x01;
    assert_eq!(scan_bytes(bytes), Err(ScanError::BadCrc));
}

#[test]
fn armer_rejects_a_bitflipped_payload_with_a_repaired_crc() {
    // The interesting case: an attacker who edits the image *and* fixes both CRCs. Only the
    // signature stands between that file and the app slot.
    let mut img = image();
    img[4_321] ^= 0x01;
    let tampered = ImageHeader::new(&img, "v1.4.0").signed();
    let original = ImageHeader::new(&image(), "v1.4.0").signed();

    let mut bytes = tampered.encode().to_vec();
    bytes.extend_from_slice(&img);
    // …carrying the signature that was valid for the *original* image.
    bytes.extend_from_slice(&sign_image(&test_key::SEED, &original, &image()));
    assert_eq!(scan_bytes(bytes), Err(ScanError::BadSignature));
}

#[test]
fn armer_rejects_a_relabelled_version() {
    // Same signed bytes, announced as a different build — the whole point of binding `fw_version`
    // into the message. (A downgrade dressed up as an upgrade, or the reverse.)
    let img = image();
    let mut bytes = signed_container(&img, "v1.4.0", &test_key::SEED);
    reseal(&mut bytes, |h| h.fw_version = ImageHeader::new(&[], "v9.9.9").fw_version);
    assert_eq!(scan_bytes(bytes), Err(ScanError::BadSignature));
}

#[test]
fn armer_rejects_a_relabelled_length() {
    // `image_len` shortened by one, with both CRCs recomputed over the shorter body: without
    // `image_len` in the signed message this would install a truncated image.
    let img = image();
    let mut bytes = signed_container(&img, "v1.4.0", &test_key::SEED);
    let short = &img[..img.len() - 1];
    reseal(&mut bytes, |h| {
        h.image_len = short.len() as u32;
        h.image_crc32 = obc_dfu::crc32(short);
    });
    assert_eq!(scan_bytes(bytes), Err(ScanError::BadSignature));
}

#[test]
fn armer_rejects_a_truncated_container() {
    let img = image();
    let full = signed_container(&img, "v1.4.0", &test_key::SEED);

    // The trailer cut off entirely — the file ends where a v1 container would have.
    let mut no_trailer = full.clone();
    no_trailer.truncate(HEADER_LEN + img.len());
    assert_eq!(scan_bytes(no_trailer), Err(ScanError::Truncated));

    // …and one byte short of a whole trailer.
    let mut short_trailer = full.clone();
    short_trailer.truncate(full.len() - 1);
    assert_eq!(scan_bytes(short_trailer), Err(ScanError::Truncated));

    // A body shorter than `image_len` is still the plain torn-copy case.
    let mut torn = full;
    torn.truncate(HEADER_LEN + img.len() / 2);
    assert_eq!(scan_bytes(torn), Err(ScanError::Truncated));
}

#[test]
fn armer_rejects_an_unsigned_v1_container() {
    // THE bypass this whole change exists to close: if a v1 wrapper were still installable, an
    // attacker would never bother forging a signature — they would just omit it.
    let img = image();
    assert_eq!(scan_bytes(v1_container(&img, "v1.4.0")), Err(ScanError::Unsigned));
}

#[test]
fn armer_rejects_an_unknown_future_scheme() {
    // A scheme this firmware cannot verify is treated exactly like no signature: "I cannot vouch
    // for this" is never a reason to install.
    let img = image();
    let mut bytes = signed_container(&img, "v1.4.0", &test_key::SEED);
    reseal(&mut bytes, |h| h.sig_scheme = 0x1234);
    assert_eq!(scan_bytes(bytes), Err(ScanError::Unsigned));

    // …as is the right scheme with the wrong trailer size (nobody gets to renegotiate the length).
    let mut bytes = signed_container(&img, "v1.4.0", &test_key::SEED);
    reseal(&mut bytes, |h| h.sig_len = 32);
    assert_eq!(scan_bytes(bytes), Err(ScanError::Unsigned));

    // …and clearing the marker on a signed file is just an unsigned file with junk at the end.
    let mut bytes = signed_container(&img, "v1.4.0", &test_key::SEED);
    reseal(&mut bytes, |h| {
        h.sig_scheme = SIG_SCHEME_NONE;
        h.sig_len = 0;
    });
    assert_eq!(scan_bytes(bytes), Err(ScanError::Unsigned));
}

#[test]
fn armer_rejects_a_signature_by_the_wrong_key() {
    // A perfectly well-formed OBCU v2 container, signed by someone else's key.
    let img = image();
    let attacker_seed = *b"an attacker's own signing seed!!";
    let bytes = signed_container(&img, "v1.4.0", &attacker_seed);
    assert_eq!(scan_bytes(bytes), Err(ScanError::BadSignature));
}

#[test]
fn armer_rejects_a_garbage_trailer() {
    // All-zero and all-ones trailers: the two blobs a torn write or an erased flash leaves behind.
    let img = image();
    for fill in [0x00u8, 0xFF] {
        let mut bytes = signed_container(&img, "v1.4.0", &test_key::SEED);
        let n = bytes.len();
        bytes[n - SIG_LEN..].fill(fill);
        assert_eq!(scan_bytes(bytes), Err(ScanError::BadSignature), "fill {fill:#04x}");
    }
}

#[test]
fn armer_rejects_a_container_signed_for_a_different_device_key() {
    // The rotation story from the device's side: firmware carrying key A refuses an image signed
    // by key B, and vice versa — the same bytes, two verdicts, decided only by the seam's key.
    let img = image();
    let bytes = signed_container(&img, "v1.4.0", &test_key::SEED);
    let other = obc_dfu::public_key_of(b"a completely different signing k");
    assert_eq!(scan_bytes_with(bytes.clone(), &other), Err(ScanError::BadSignature));
    assert!(scan_bytes_with(bytes, &test_key::PUBLIC).is_ok());
}

// ==================== v1 ↔ v2 offset compatibility ====================
//
// The hard constraint of #997: `obc-boot` is 32 KB, flashed **once by probe**, and never updated by
// DFU. A bootloader already in the field must keep installing images produced after this change.
// The tests below are the executable form of that argument.

/// What the v1 header decoder does, transcribed from `OBCU_Spec.md` §1.1 as it read *before* this
/// change — deliberately a fresh reimplementation, not a call into `obc-dfu`, so it keeps testing
/// the old behavior no matter how the crate evolves.
///
/// Returns `(image_len, image_crc32, fw_version)` — the three fields a v1 `ImageHeader` carries, and
/// the only header state the bootloader's install engine consumes.
fn v1_decode(bytes: &[u8; 64]) -> Option<(u32, u32, [u8; 32])> {
    if &bytes[0..4] != b"OBCU" {
        return None;
    }
    if u16::from_le_bytes([bytes[4], bytes[5]]) != 1 {
        return None;
    }
    let stored = u32::from_le_bytes([bytes[60], bytes[61], bytes[62], bytes[63]]);
    if stored != obc_dfu::crc32(&bytes[..60]) {
        return None;
    }
    let mut fw_version = [0u8; 32];
    fw_version.copy_from_slice(&bytes[16..48]);
    Some((
        u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
        u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]),
        fw_version,
    ))
}

#[test]
fn a_fielded_v1_decoder_accepts_a_v2_header_with_identical_fields() {
    let img = image();
    let v1 = v1_container(&img, "v1.4.0-3-gcafe123");
    let v2 = signed_container(&img, "v1.4.0-3-gcafe123", &test_key::SEED);

    let v1_hdr: &[u8; 64] = v1[..64].try_into().unwrap();
    let v2_hdr: &[u8; 64] = v2[..64].try_into().unwrap();

    let from_v1 = v1_decode(v1_hdr).expect("v1 header decodes under the v1 rules");
    let from_v2 = v1_decode(v2_hdr).expect("v2 header ALSO decodes under the v1 rules");
    assert_eq!(from_v1, from_v2, "every field the bootloader reads is byte-identical");

    // Field by field, so a failure names the offset that moved.
    assert_eq!(&v1_hdr[0..4], &v2_hdr[0..4], "magic @0");
    assert_eq!(&v1_hdr[4..6], &v2_hdr[4..6], "header version @4 — still 1, deliberately");
    assert_eq!(&v1_hdr[6..8], &v2_hdr[6..8], "reserved @6");
    assert_eq!(&v1_hdr[8..12], &v2_hdr[8..12], "image_len @8");
    assert_eq!(&v1_hdr[12..16], &v2_hdr[12..16], "image_crc32 @12");
    assert_eq!(&v1_hdr[16..48], &v2_hdr[16..48], "fw_version @16");
    // Exactly one region differs: v1's reserved run, which v1 promised to a scheme marker.
    assert_ne!(&v1_hdr[48..52], &v2_hdr[48..52], "the scheme marker @48 is the only new content");
    assert_eq!(&v1_hdr[52..60], &v2_hdr[52..60], "the rest of the reserved run @52 is still zero");
    assert_ne!(&v1_hdr[60..64], &v2_hdr[60..64], "the header CRC @60 covers the marker, so it moves");
}

#[test]
fn the_v2_signature_lives_where_v1_already_ignored_bytes() {
    // v1 §1 specified the container as `64 + image_len` bytes and said anything past that is
    // ignored. The trailer sits exactly there, so a v1 reader's view of the file is unchanged.
    let img = image();
    let v2 = signed_container(&img, "v1.4.0", &test_key::SEED);
    let v1 = v1_container(&img, "v1.4.0");
    assert_eq!(v2.len(), 64 + img.len() + SIG_LEN);
    assert_eq!(&v2[64..64 + img.len()], &v1[64..], "the image bytes are in the same place");
    assert_eq!(v2.len() - v1.len(), SIG_LEN, "v2 adds exactly the trailer");
}

#[test]
fn the_install_engine_flashes_a_v2_container_unchanged() {
    // End to end through the *real* engine — the code `obc-boot` links — over a v2 container: the
    // verify pass must accept the header it finds on card (it compares the decoded header against
    // the armer's record), and the flash pass must write exactly `image_len` raw bytes, skipping the
    // 64-byte container header and never touching the signature trailer. This is the flash-once
    // guarantee in executable form: the engine is unchanged code, and it installs v2.
    let img = image();
    let container = signed_container(&img, "v1.4.0", &test_key::SEED);
    let staged = scan_bytes(container.clone()).expect("the v2 container scans");

    let flashed = mock_engine::install(&container, &staged);
    assert_eq!(flashed, img, "the raw image is flashed; the header and the trailer are not");
}

#[test]
fn the_install_engine_treats_v1_and_v2_identically() {
    // Same image, two containers; the slot contents must be indistinguishable. (The v1 case is what
    // a device installed before #997 — and what the rollback snapshot still is.)
    let img = image();
    let v1 = v1_container(&img, "v1.4.0");
    let v2 = signed_container(&img, "v1.4.0", &test_key::SEED);
    let v1_header = ImageHeader::decode(v1[..HEADER_LEN].try_into().unwrap()).unwrap();
    let v2_header = ImageHeader::decode(v2[..HEADER_LEN].try_into().unwrap()).unwrap();

    let staged_v1 = mock_engine::staged_ref(v1_header, &v1);
    let staged_v2 = mock_engine::staged_ref(v2_header, &v2);
    assert_eq!(mock_engine::install(&v1, &staged_v1), mock_engine::install(&v2, &staged_v2));
}

/// A minimal `InstallIo` over an in-memory card + app slot: enough to run the production install
/// engine and observe what it wrote. (The full failure matrix — power loss, retries, torn pages —
/// lives in `tests/engine.rs`; this mock exists only to prove a v2 container survives that engine.)
mod mock_engine {
    use obc_dfu::engine::{run, InstallIo, IoError, Slot};
    use obc_dfu::{BootState, Extent, ImageHeader, Outcome, StagedRef, PAGE_LEN, SD_BLOCK_LEN};

    /// The container is laid on the card from this block — one contiguous run, a freshly-copied
    /// file's FAT shape.
    const START_BLOCK: u32 = 100;
    const SLOT: Slot = Slot { base: 0x8000, len: 32 * 1024 };

    struct Io {
        card: Vec<u8>,
        flash: Vec<u8>,
        state_page: Vec<u8>,
    }

    impl InstallIo for Io {
        fn read_blocks(&mut self, start_block: u32, buf: &mut [u8]) -> Result<(), IoError> {
            let off = start_block as usize * SD_BLOCK_LEN;
            for (i, b) in buf.iter_mut().enumerate() {
                *b = self.card.get(off + i).copied().unwrap_or(0xEE);
            }
            Ok(())
        }
        fn write_lines(&mut self, addr: u32, data: &[u8]) -> Result<(), IoError> {
            let off = (addr - SLOT.base) as usize;
            self.flash[off..off + data.len()].copy_from_slice(data);
            Ok(())
        }
        fn read_flash(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), IoError> {
            let off = (addr - SLOT.base) as usize;
            buf.copy_from_slice(&self.flash[off..off + buf.len()]);
            Ok(())
        }
        fn write_state(&mut self, state: &BootState) -> Result<(), IoError> {
            let page = state.encode();
            self.state_page[..page.len()].copy_from_slice(page.as_bytes());
            Ok(())
        }
    }

    /// The `StagedRef` an armer would record for `container` laid out at [`START_BLOCK`].
    pub fn staged_ref(header: ImageHeader, container: &[u8]) -> StagedRef {
        let blocks = (container.len() as u32).div_ceil(SD_BLOCK_LEN as u32);
        StagedRef::new(header, header.image_len, header.image_crc32, &[Extent { start_block: START_BLOCK, blocks }])
            .expect("a coherent staged ref")
    }

    /// Run the engine's install over `container` and return exactly the `image_len` bytes that
    /// landed at the slot base.
    pub fn install(container: &[u8], staged: &StagedRef) -> Vec<u8> {
        let mut card = vec![0u8; START_BLOCK as usize * SD_BLOCK_LEN];
        card.extend_from_slice(container);
        card.resize(card.len().next_multiple_of(SD_BLOCK_LEN), 0xEE);
        let mut io = Io { card, flash: vec![0xAA; SLOT.len as usize], state_page: vec![0u8; PAGE_LEN] };

        let state = BootState::Armed { generation: 1, update: *staged, rollback: None };
        let mut buf = [0u8; 4096];
        assert_eq!(run(&state, &SLOT, &mut io, &mut buf), Outcome::Installed, "a v2 container installs");
        io.flash[..staged.len as usize].to_vec()
    }
}
