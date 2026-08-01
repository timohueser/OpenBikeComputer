//! The OBCU update-image container header (see `OBCU_Spec.md` §1).
//!
//! An update image on the SD card is a fixed **64-byte header** ([`ImageHeader`]) followed by the
//! raw application image (the bytes an `objcopy -O binary` of the app ELF produces, starting at the
//! vector table) and — since v2 (#997) — an optional **signature trailer**. The header carries the
//! image length + a CRC-32 over the raw image, a `git describe` version string, the signature-scheme
//! marker, and its own CRC-32 — so a torn or foreign file is rejected before a single byte of the app
//! slot is touched (epic #615 safety invariant 1, "verify before erase").
//!
//! [`decode`](ImageHeader::decode) follows the settings-codec convention: **valid CRC ⇒ `Some`**, and
//! any bad magic / version / CRC ⇒ `None`.
//!
//! ## Why `HEADER_VERSION` is still 1 in a "v2" container
//!
//! The 32 KB `obc-boot` bootloader is flashed **once, by probe**, and is never updated in the field —
//! so a bootloader already on a device must keep installing images produced years later. Its
//! [`decode`](ImageHeader::decode) hard-rejects any Header Version but `1`, so bumping that field
//! would brick every fielded boot chain's ability to install. v1 anticipated exactly this and
//! reserved header bytes `48..60` for "a future signature-scheme marker"; v2 spends them. Everything
//! the bootloader reads — magic, header version, `image_len`, `image_crc32`, `fw_version`, the header
//! CRC over bytes `0..60` — is byte-identical at its v1 offset, and the signature itself lives in a
//! **trailer** past `64 + image_len`, in bytes v1 already specified as ignorable. `sig_scheme` (not
//! Header Version) is the v1/v2 discriminator.

use crate::crc32::crc32;
use crate::sig::{SIG_LEN, SIG_SCHEME_ED25519, SIG_SCHEME_NONE};

/// Fixed image-header length, bytes.
pub const HEADER_LEN: usize = 64;

/// Header magic — `b"OBCU"` (OpenBikeComputer Update).
pub const MAGIC: [u8; 4] = *b"OBCU";

/// The header-layout version, **pinned at 1 forever** — see the module doc. A v2 (signed) container
/// still carries `1` here so a fielded bootloader's `decode` accepts it; the v1/v2 discriminator is
/// [`ImageHeader::sig_scheme`]. [`decode`] rejects every other value (like the settings codec, a
/// version change is a hard reject, not a migration).
///
/// [`decode`]: ImageHeader::decode
pub const HEADER_VERSION: u16 = 1;

/// Bytes of the header covered by `header_crc32` (everything but the trailing CRC itself).
const HEADER_CRC_LEN: usize = 60;

/// Byte cap of the NUL-padded `fw_version` string field.
pub const FW_VERSION_LEN: usize = 32;

/// The largest raw image the wrapper accepts and the armer stages: the L15 DK app slot
/// (`0x8000 … 0x17B000` = 1,484 KB) minus a small margin. The constant lives here so the host tool,
/// the armer, and the bootloader all agree on the ceiling. The LM20's larger slot is a future
/// mechanical bump (epic #615, "LM20 memory-layout constants" out of scope).
pub const MAX_IMAGE_LEN: u32 = 1_480_000;

/// The largest **container** (header + raw image + signature trailer) a max-size image produces —
/// the ceiling the BLE/USB `fwImage` announce gates at, so an image exactly at [`MAX_IMAGE_LEN`] is
/// never refused for its own framing. v2 widened it by [`SIG_LEN`].
pub const MAX_CONTAINER_LEN: u32 = MAX_IMAGE_LEN + HEADER_LEN as u32 + SIG_LEN as u32;

/// Start of the nRF54L15's RAM (`0x2000_0000`) — the low bound for the vector table's initial-SP
/// sanity check ([`looks_like_vector_table`]).
pub const RAM_START: u32 = 0x2000_0000;
/// One past the end of the nRF54L15 DK's 256 KB RAM (`0x2004_0000`) — the high bound for the initial
/// SP. A raw image whose first word (the reset vector's initial stack pointer) is outside
/// `RAM_START..RAM_END` almost certainly isn't a vector-table-first binary (e.g. an ELF, or a `.bin`
/// stripped in the wrong section order).
pub const RAM_END: u32 = 0x2004_0000;

/// Heuristic: does `image` begin with a plausible Cortex-M vector table? The first 32-bit word of a
/// bare-metal image is the **initial stack pointer**, which for this device must point into RAM. Used
/// as a *warn-only* guard in the wrapper — a stripped-wrong `.bin` (LMA order, an ELF by mistake)
/// fails it, but a legitimately unusual SP shouldn't block wrapping. Returns `false` for a too-short
/// image.
pub fn looks_like_vector_table(image: &[u8]) -> bool {
    if image.len() < 4 {
        return false;
    }
    let sp = u32::from_le_bytes([image[0], image[1], image[2], image[3]]);
    (RAM_START..RAM_END).contains(&sp)
}

/// The 64-byte OBCU image header: enough to reject a bad/torn image before erasing the app slot, plus
/// a human-readable version for the UI. `Copy` (a small POD) so it nests freely inside the boot-state
/// records without borrowing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageHeader {
    /// Length of the raw image following the header, bytes.
    pub image_len: u32,
    /// CRC-32/IEEE over the raw image only (the bytes after the header).
    pub image_crc32: u32,
    /// `git describe` version, UTF-8, NUL-padded to [`FW_VERSION_LEN`]. Read it via
    /// [`fw_version_str`](ImageHeader::fw_version_str).
    pub fw_version: [u8; FW_VERSION_LEN],
    /// Signature scheme (header bytes `48..50`, v1's reserved space):
    /// [`SIG_SCHEME_NONE`] = an unsigned v1 container, [`SIG_SCHEME_ED25519`] = OBCU v2.
    /// **This**, not [`HEADER_VERSION`], is the v1/v2 discriminator (see the module doc).
    pub sig_scheme: u16,
    /// Bytes of the signature trailer that follows the raw image (header bytes `50..52`); `0` when
    /// `sig_scheme` is [`SIG_SCHEME_NONE`], [`SIG_LEN`] for Ed25519. Stored explicitly so a reader
    /// can size the container without knowing the scheme.
    pub sig_len: u16,
}

impl ImageHeader {
    /// Build a header for `image` tagged with `version`: computes `image_len` + `image_crc32` over
    /// the raw bytes and packs `version` into the NUL-padded field (truncated to [`FW_VERSION_LEN`]
    /// on a UTF-8 char boundary — never mid-codepoint). The wrapper (`obc-mkimage`) is the caller.
    pub fn new(image: &[u8], version: &str) -> ImageHeader {
        let mut fw_version = [0u8; FW_VERSION_LEN];
        let mut end = version.len().min(FW_VERSION_LEN);
        while end > 0 && !version.is_char_boundary(end) {
            end -= 1;
        }
        fw_version[..end].copy_from_slice(&version.as_bytes()[..end]);
        ImageHeader {
            image_len: image.len() as u32,
            image_crc32: crc32(image),
            fw_version,
            sig_scheme: SIG_SCHEME_NONE,
            sig_len: 0,
        }
    }

    /// The same header marked as **Ed25519-signed** (OBCU v2): `sig_scheme` = [`SIG_SCHEME_ED25519`],
    /// `sig_len` = [`SIG_LEN`]. The signature bytes themselves live in the container's trailer, not
    /// here. Signing the image doesn't change what the message covers (`fw_version` + `image_len` +
    /// the image), so it is safe to mark before or after computing the signature.
    pub fn signed(self) -> ImageHeader {
        ImageHeader { sig_scheme: SIG_SCHEME_ED25519, sig_len: SIG_LEN as u16, ..self }
    }

    /// The same header with the signature marker cleared — a plain v1 container. Used for the
    /// device-local `ROLLBACK.BIN` snapshot, which genuinely carries no signature (it is written by
    /// the device from its own running slot, and only ever re-flashed by the bootloader, which
    /// verifies it by CRC).
    pub fn unsigned(self) -> ImageHeader {
        ImageHeader { sig_scheme: SIG_SCHEME_NONE, sig_len: 0, ..self }
    }

    /// Does this container carry a signature trailer?
    pub const fn is_signed(&self) -> bool {
        self.sig_scheme != SIG_SCHEME_NONE
    }

    /// The whole container's byte length: header + raw image + signature trailer. `u64` because the
    /// caller is usually comparing it against an untrusted on-card file length.
    pub const fn container_len(&self) -> u64 {
        HEADER_LEN as u64 + self.image_len as u64 + self.sig_len as u64
    }

    /// File offset of the signature trailer (`64 + image_len`) — where [`sig_len`](Self::sig_len)
    /// bytes of signature begin.
    pub const fn sig_offset(&self) -> u64 {
        HEADER_LEN as u64 + self.image_len as u64
    }

    /// Pack into the fixed 64-byte header: magic, version, the little-endian fields, the NUL-padded
    /// version string, then the header CRC over bytes `0..60`. The inverse of [`decode`](Self::decode).
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut b = [0u8; HEADER_LEN];
        b[0..4].copy_from_slice(&MAGIC);
        b[4..6].copy_from_slice(&HEADER_VERSION.to_le_bytes());
        // 6..8 reserved (0)
        b[8..12].copy_from_slice(&self.image_len.to_le_bytes());
        b[12..16].copy_from_slice(&self.image_crc32.to_le_bytes());
        b[16..48].copy_from_slice(&self.fw_version);
        // 48..52: the signature-scheme marker v1 reserved (zero ⇒ byte-identical to a v1 header).
        b[48..50].copy_from_slice(&self.sig_scheme.to_le_bytes());
        b[50..52].copy_from_slice(&self.sig_len.to_le_bytes());
        // 52..60 still reserved (0)
        let crc = crc32(&b[..HEADER_CRC_LEN]);
        b[HEADER_CRC_LEN..HEADER_LEN].copy_from_slice(&crc.to_le_bytes());
        b
    }

    /// Decode a 64-byte header, or `None` for anything but a clean read of *this* format — bad magic,
    /// the wrong version, or a failed header CRC. `Some` guarantees the length/CRC fields are the ones
    /// the writer stored (the raw-image CRC is verified separately, against the staged bytes).
    ///
    /// The rule is deliberately **unchanged from v1** — magic, version, CRC, nothing else. A v1
    /// container decodes to `sig_scheme: 0`, a v2 one to `sig_scheme: 1`; whether *this* device will
    /// install what it decoded is the armer's policy call ([`crate::armer::scan`]), not the codec's.
    /// Keeping the decoder's reject set identical is what makes the fielded-bootloader compatibility
    /// argument hold: a v2 header decodes to the same three fields under a v1 decoder.
    pub fn decode(bytes: &[u8; HEADER_LEN]) -> Option<ImageHeader> {
        if bytes[0..4] != MAGIC {
            return None;
        }
        if u16::from_le_bytes([bytes[4], bytes[5]]) != HEADER_VERSION {
            return None;
        }
        let stored = u32::from_le_bytes([bytes[60], bytes[61], bytes[62], bytes[63]]);
        if stored != crc32(&bytes[..HEADER_CRC_LEN]) {
            return None;
        }
        let mut fw_version = [0u8; FW_VERSION_LEN];
        fw_version.copy_from_slice(&bytes[16..48]);
        Some(ImageHeader {
            image_len: u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            image_crc32: u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]),
            fw_version,
            sig_scheme: u16::from_le_bytes([bytes[48], bytes[49]]),
            sig_len: u16::from_le_bytes([bytes[50], bytes[51]]),
        })
    }

    /// The version string, trailing NULs trimmed. Empty if the field is blank or not valid UTF-8 (a
    /// corrupt-but-CRC-valid field never yields garbage — same defensive shape as the device-name
    /// codec).
    pub fn fw_version_str(&self) -> &str {
        let end = self.fw_version.iter().position(|&b| b == 0).unwrap_or(FW_VERSION_LEN);
        core::str::from_utf8(&self.fw_version[..end]).unwrap_or("")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ImageHeader {
        ImageHeader::new(b"the raw application image bytes", "v1.2.3-4-gdeadbee-dirty")
    }

    #[test]
    fn roundtrip() {
        let h = sample();
        let decoded = ImageHeader::decode(&h.encode()).expect("valid header decodes");
        assert_eq!(decoded, h);
        assert_eq!(decoded.fw_version_str(), "v1.2.3-4-gdeadbee-dirty");
        assert_eq!(decoded.image_len, 31);
    }

    #[test]
    fn version_truncates_on_char_boundary() {
        // A multi-byte char straddling the 32-byte cap must not be split mid-codepoint.
        let long = "0123456789012345678901234567890é"; // 'é' is 2 bytes, starting at index 31
        let h = ImageHeader::new(b"", long);
        // Byte 31 is the first byte of 'é'; the cap can't fit both bytes, so it drops the whole char.
        assert_eq!(h.fw_version_str(), "0123456789012345678901234567890");
        assert!(core::str::from_utf8(&h.fw_version).is_ok());
    }

    #[test]
    fn rejects_short_is_type_enforced() {
        // decode takes &[u8; 64] so a short buffer can't reach it — the caller slices. Assert the
        // fixed length instead, which is what protects the callers.
        assert_eq!(sample().encode().len(), HEADER_LEN);
    }

    #[test]
    fn rejects_bad_magic() {
        let mut b = sample().encode();
        b[0] = b'X';
        assert!(ImageHeader::decode(&b).is_none());
    }

    #[test]
    fn rejects_wrong_version() {
        let mut b = sample().encode();
        b[4] = 2; // header_version 2
        assert!(ImageHeader::decode(&b).is_none());
    }

    #[test]
    fn rejects_corrupt_crc() {
        // Flip a payload byte without fixing the CRC.
        let mut b = sample().encode();
        b[8] ^= 0xFF;
        assert!(ImageHeader::decode(&b).is_none());
        // Flip a CRC byte directly.
        let mut b = sample().encode();
        b[60] ^= 0x01;
        assert!(ImageHeader::decode(&b).is_none());
    }

    #[test]
    fn reserved_bytes_are_zero() {
        let b = sample().encode();
        assert_eq!(&b[6..8], &[0, 0]);
        // An unsigned header is byte-identical to a v1 one: the whole v1 reserved run stays zero.
        assert_eq!(&b[48..60], &[0u8; 12]);
    }

    #[test]
    fn signed_marker_lands_in_v1s_reserved_space() {
        let b = sample().signed().encode();
        assert_eq!(&b[48..50], &SIG_SCHEME_ED25519.to_le_bytes(), "sig_scheme at offset 48");
        assert_eq!(&b[50..52], &(SIG_LEN as u16).to_le_bytes(), "sig_len at offset 50");
        assert_eq!(&b[52..60], &[0u8; 8], "the rest of the reserved run is still reserved");
        // Every field a v1 reader looks at is untouched at its v1 offset.
        let v1 = sample().encode();
        assert_eq!(&b[0..48], &v1[0..48], "magic/version/len/crc/fw_version are byte-identical");
    }

    #[test]
    fn signed_roundtrips_and_sizes_the_container() {
        let h = sample().signed();
        let decoded = ImageHeader::decode(&h.encode()).expect("a signed header still decodes");
        assert_eq!(decoded, h);
        assert!(decoded.is_signed());
        assert_eq!(decoded.sig_offset(), HEADER_LEN as u64 + 31);
        assert_eq!(decoded.container_len(), HEADER_LEN as u64 + 31 + SIG_LEN as u64);
        // …and `unsigned()` is the exact inverse, back to v1 bytes.
        assert_eq!(decoded.unsigned().encode(), sample().encode());
        assert_eq!(sample().container_len(), HEADER_LEN as u64 + 31);
    }

    #[test]
    fn header_version_is_pinned_at_one() {
        // Load-bearing, not incidental: a fielded bootloader hard-rejects anything else, so a v2
        // container must still say 1 here (see the module doc). Changing this constant means
        // deciding to strand every device already in the field.
        assert_eq!(HEADER_VERSION, 1);
        assert_eq!(&sample().signed().encode()[4..6], &1u16.to_le_bytes());
    }

    #[test]
    fn vector_table_sp_range() {
        // Initial SP inside RAM ⇒ plausible; outside ⇒ not; too short ⇒ not.
        let mut good = [0u8; 8];
        good[..4].copy_from_slice(&0x2002_0000u32.to_le_bytes());
        assert!(looks_like_vector_table(&good));
        let mut flash = [0u8; 8];
        flash[..4].copy_from_slice(&0x0000_8000u32.to_le_bytes()); // an app-slot LMA, not an SP
        assert!(!looks_like_vector_table(&flash));
        assert!(!looks_like_vector_table(&[0u8; 3]));
    }
}
