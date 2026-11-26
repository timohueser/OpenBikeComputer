//! Host tests for the OBCU signature: the armer's accept and reject matrix, and an install of a
//! signed container through the real engine.
//!
//! The armer vectors take one signed container and apply one mutation per attack, always through
//! the production `scan`.

use obc_dfu::armer::{scan, ScanError, StageIo};
use obc_dfu::engine::IoError;
use obc_dfu::sig::test_key;
use obc_dfu::{
    sign_image, signing_prefix, Extent, ImageHeader, PublicKey, HEADER_LEN, MAX_EXTENTS, SIG_CONTEXT, SIG_LEN,
    SIG_PREFIX_LEN, SIG_SCHEME_ED25519, SIG_SCHEME_NONE,
};

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
    fn stage_extents(&mut self, out: &mut [Extent; MAX_EXTENTS]) -> Result<usize, IoError> {
        out[0] = Extent { start_block: 100, blocks: (self.bytes.len() as u32).div_ceil(512) };
        Ok(1)
    }
}

/// Long enough that the scan's chunked pass has real tails.
fn image() -> Vec<u8> {
    (0..9_000u32).map(|i| (i.wrapping_mul(31) % 251) as u8).collect()
}

fn signed_container(image: &[u8], version: &str, seed: &[u8; 32]) -> Vec<u8> {
    let header = ImageHeader::new(image, version).signed();
    let mut bytes = header.encode().to_vec();
    bytes.extend_from_slice(image);
    bytes.extend_from_slice(&sign_image(seed, &header, image));
    bytes
}

fn unsigned_container(image: &[u8], version: &str) -> Vec<u8> {
    let header = ImageHeader::new(image, version);
    let mut bytes = header.encode().to_vec();
    bytes.extend_from_slice(image);
    bytes
}

fn scan_bytes(bytes: Vec<u8>) -> Result<obc_dfu::StagedRef, ScanError> {
    scan_bytes_with(bytes, &test_key::PUBLIC)
}

fn scan_bytes_with(bytes: Vec<u8>, key: &PublicKey) -> Result<obc_dfu::StagedRef, ScanError> {
    // An awkward chunk size on purpose: the signature hash and the CRC must agree about the tail.
    let mut chunk = [0u8; 96];
    scan(&mut Stage { bytes }, &mut chunk, key)
}

/// Recomputes the header CRC after a mutation, so the mutation reaches the signature check instead
/// of dying at `BadHeader`. An attacker controls the CRC, so this is the strong form of an attack.
fn reseal(bytes: &mut [u8], mutate: impl FnOnce(&mut ImageHeader)) {
    let hdr: &[u8; HEADER_LEN] = bytes[..HEADER_LEN].try_into().unwrap();
    let mut header = ImageHeader::decode(hdr).expect("still a valid header");
    mutate(&mut header);
    bytes[..HEADER_LEN].copy_from_slice(&header.encode());
}

/// The layout is asserted against a hand-built message, not against `signing_prefix`'s own output,
/// so the spec text and the code have to agree.
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

#[test]
fn armer_accepts_a_correctly_signed_container() {
    let img = image();
    let staged = scan_bytes(signed_container(&img, "v1.4.0", &test_key::SEED)).expect("a signed container arms");
    assert_eq!(staged.len, img.len() as u32);
    assert!(staged.header.is_signed());
    assert_eq!(staged.header.sig_scheme, SIG_SCHEME_ED25519);
    assert_eq!(staged.header.sig_len as usize, SIG_LEN);
    assert_eq!(staged.header.fw_version_str(), "v1.4.0");
}

#[test]
fn armer_rejects_a_bitflipped_payload() {
    // The CRC fails first, so the rider is told the file is damaged, not that it is forged.
    let img = image();
    let mut bytes = signed_container(&img, "v1.4.0", &test_key::SEED);
    bytes[HEADER_LEN + 4_321] ^= 0x01;
    assert_eq!(scan_bytes(bytes), Err(ScanError::BadCrc));
}

#[test]
fn armer_rejects_a_bitflipped_payload_with_a_repaired_crc() {
    // An attacker who edits the image and fixes both CRCs: only the signature is left.
    let mut img = image();
    img[4_321] ^= 0x01;
    let tampered = ImageHeader::new(&img, "v1.4.0").signed();
    let original = ImageHeader::new(&image(), "v1.4.0").signed();

    let mut bytes = tampered.encode().to_vec();
    bytes.extend_from_slice(&img);
    bytes.extend_from_slice(&sign_image(&test_key::SEED, &original, &image()));
    assert_eq!(scan_bytes(bytes), Err(ScanError::BadSignature));
}

#[test]
fn armer_rejects_a_relabelled_version() {
    // Same signed bytes, announced as a different build: a downgrade dressed as an upgrade.
    let img = image();
    let mut bytes = signed_container(&img, "v1.4.0", &test_key::SEED);
    reseal(&mut bytes, |h| h.fw_version = ImageHeader::new(&[], "v9.9.9").fw_version);
    assert_eq!(scan_bytes(bytes), Err(ScanError::BadSignature));
}

#[test]
fn armer_rejects_a_relabelled_length() {
    // Without `image_len` in the signed message, this would install a truncated image.
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

    let mut no_trailer = full.clone();
    no_trailer.truncate(HEADER_LEN + img.len());
    assert_eq!(scan_bytes(no_trailer), Err(ScanError::Truncated));

    let mut short_trailer = full.clone();
    short_trailer.truncate(full.len() - 1);
    assert_eq!(scan_bytes(short_trailer), Err(ScanError::Truncated));

    let mut torn = full;
    torn.truncate(HEADER_LEN + img.len() / 2);
    assert_eq!(scan_bytes(torn), Err(ScanError::Truncated));
}

#[test]
fn armer_rejects_an_unsigned_container() {
    // The bypass that matters: if an unsigned wrapper were installable, nobody would forge a
    // signature, they would omit it.
    let img = image();
    assert_eq!(scan_bytes(unsigned_container(&img, "v1.4.0")), Err(ScanError::Unsigned));
}

#[test]
fn armer_rejects_an_unknown_future_scheme() {
    // A scheme this firmware cannot verify counts as no signature.
    let img = image();
    let mut bytes = signed_container(&img, "v1.4.0", &test_key::SEED);
    reseal(&mut bytes, |h| h.sig_scheme = 0x1234);
    assert_eq!(scan_bytes(bytes), Err(ScanError::Unsigned));

    // The right scheme with the wrong trailer size: the length is not negotiable.
    let mut bytes = signed_container(&img, "v1.4.0", &test_key::SEED);
    reseal(&mut bytes, |h| h.sig_len = 32);
    assert_eq!(scan_bytes(bytes), Err(ScanError::Unsigned));

    // Clearing the marker leaves an unsigned file with junk at the end.
    let mut bytes = signed_container(&img, "v1.4.0", &test_key::SEED);
    reseal(&mut bytes, |h| {
        h.sig_scheme = SIG_SCHEME_NONE;
        h.sig_len = 0;
    });
    assert_eq!(scan_bytes(bytes), Err(ScanError::Unsigned));
}

#[test]
fn armer_rejects_a_garbage_trailer() {
    // All-zero and all-ones trailers: what a torn write or an erased flash leaves behind.
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
    // Key rotation from the device's side: the same bytes, two verdicts, decided by the key.
    let img = image();
    let bytes = signed_container(&img, "v1.4.0", &test_key::SEED);
    let other = obc_dfu::public_key_of(b"a completely different signing k");
    assert_eq!(scan_bytes_with(bytes.clone(), &other), Err(ScanError::BadSignature));
    assert!(scan_bytes_with(bytes, &test_key::PUBLIC).is_ok());
}

#[test]
fn the_install_engine_flashes_a_signed_container_unchanged() {
    // Through the real engine, the code `obc-boot` links: the flash pass must write exactly
    // `image_len` raw bytes, skipping the container header and never touching the trailer.
    let img = image();
    let container = signed_container(&img, "v1.4.0", &test_key::SEED);
    let staged = scan_bytes(container.clone()).expect("the signed container scans");

    let flashed = mock_engine::install(&container, &staged);
    assert_eq!(flashed, img, "the raw image is flashed; the header and the trailer are not");
}

/// A minimal `InstallIo` over an in-memory card and app slot: enough to run the real engine and
/// see what it wrote. The failure matrix lives in the engine tests.
mod mock_engine {
    use obc_dfu::engine::{run, InstallIo, IoError, Slot};
    use obc_dfu::{BootState, Outcome, StagedRef, PAGE_LEN, SD_BLOCK_LEN};

    /// The container sits from this block in one contiguous run, as a freshly copied file does.
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

    /// Returns exactly the `image_len` bytes that landed at the slot base.
    pub fn install(container: &[u8], staged: &StagedRef) -> Vec<u8> {
        let mut card = vec![0u8; START_BLOCK as usize * SD_BLOCK_LEN];
        card.extend_from_slice(container);
        card.resize(card.len().next_multiple_of(SD_BLOCK_LEN), 0xEE);
        let mut io = Io { card, flash: vec![0xAA; SLOT.len as usize], state_page: vec![0u8; PAGE_LEN] };

        let state = BootState::Armed { generation: 1, update: *staged, rollback: None };
        let mut buf = [0u8; 4096];
        assert_eq!(run(&state, &SLOT, &mut io, &mut buf), Outcome::Installed, "a signed container installs");
        io.flash[..staged.len as usize].to_vec()
    }
}
