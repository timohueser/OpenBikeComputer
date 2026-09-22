//! The OBCU update-image container header (`OBCU_Spec.md`).
//!
//! An update image on the card is a fixed 64-byte header, the raw application image, and an
//! optional signature trailer. The header carries the image length, a CRC-32 over the raw image, a
//! version string, the signature-scheme marker, and its own CRC-32, so a torn or foreign file is
//! rejected before a byte of the app slot is touched.
//!
//! [`ImageHeader::decode`] returns `Some` only for a valid CRC; bad magic, version or CRC give
//! `None`.
//!
//! A signed container still carries header version 1. The bootloader is flashed once and never
//! updated, and its decode rejects any other header version, so bumping the field would stop every
//! fielded boot chain from installing. The signature marker lives in bytes `48..52`, which v1
//! reserved, and the signature itself in a trailer past `64 + image_len`. `sig_scheme` is the
//! signed/unsigned discriminator.

use crate::crc32::crc32;
use crate::layout::{RAM_END, RAM_START};
use crate::sig::{SIG_LEN, SIG_SCHEME_ED25519, SIG_SCHEME_NONE};

pub const HEADER_LEN: usize = 64;

pub const MAGIC: [u8; 4] = *b"OBCU";

/// The header-layout version, pinned at 1 forever; see the module doc. [`ImageHeader::decode`]
/// rejects every other value.
pub const HEADER_VERSION: u16 = 1;

/// Bytes the header CRC covers: everything but the trailing CRC itself.
const HEADER_CRC_LEN: usize = 60;

/// Byte cap of the NUL-padded `fw_version` field.
pub const FW_VERSION_LEN: usize = 32;

/// The largest raw image the wrapper accepts and the armer stages: the app slot, and nothing
/// less. The bootloader's install engine checks the same bound against the slot its linker gives
/// it ([`crate::layout`]), so an image the host tool wraps is one the device can flash.
pub const MAX_IMAGE_LEN: u32 = crate::layout::APP_SLOT_LEN;

/// The largest container a max-size image produces. The BLE and USB announce gate at this, so an
/// image exactly at [`MAX_IMAGE_LEN`] is never refused for its own framing.
pub const MAX_CONTAINER_LEN: u32 = MAX_IMAGE_LEN + HEADER_LEN as u32 + SIG_LEN as u32;

/// The recorded image bytes when `installed` describes the start of `slot` exactly.
///
/// The length gate runs before the slice is formed, so a corrupt record cannot read beyond the app
/// slot. CRC-32 then distinguishes the installed DFU image from a later probe flash.
pub fn matching_image<'a>(installed: Option<&ImageHeader>, slot: &'a [u8]) -> Option<&'a [u8]> {
    let installed = installed?;
    if installed.image_len == 0 || installed.image_len > MAX_IMAGE_LEN {
        return None;
    }
    let image = slot.get(..installed.image_len as usize)?;
    (crc32(image) == installed.image_crc32).then_some(image)
}

/// Does `image` begin with a plausible Cortex-M vector table? The first word of a bare-metal image
/// is the initial stack pointer, which must point into RAM. The wrapper uses it as a warn-only
/// guard, because an unusual stack pointer must not block wrapping.
pub fn looks_like_vector_table(image: &[u8]) -> bool {
    if image.len() < 4 {
        return false;
    }
    let sp = u32::from_le_bytes([image[0], image[1], image[2], image[3]]);
    (RAM_START..RAM_END).contains(&sp)
}

/// Enough to reject a bad or torn image before the app slot is erased, plus a readable version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageHeader {
    pub image_len: u32,
    /// CRC-32 over the raw image only, not the header.
    pub image_crc32: u32,
    /// `git describe` version, UTF-8, NUL-padded. Read it with
    /// [`fw_version_str`](ImageHeader::fw_version_str).
    pub fw_version: [u8; FW_VERSION_LEN],
    /// Header bytes `48..50`. This, not [`HEADER_VERSION`], says whether the container is signed.
    pub sig_scheme: u16,
    /// Header bytes `50..52`: the length of the trailer after the raw image. It is stored so a
    /// reader can size the container without knowing the scheme.
    pub sig_len: u16,
}

impl ImageHeader {
    /// `version` is truncated to [`FW_VERSION_LEN`] on a UTF-8 character boundary, never
    /// mid-codepoint.
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

    /// The same header marked as Ed25519-signed; the signature bytes live in the trailer. The
    /// marker is not part of the signed message, so it can be set before or after signing.
    pub fn signed(self) -> ImageHeader {
        ImageHeader { sig_scheme: SIG_SCHEME_ED25519, sig_len: SIG_LEN as u16, ..self }
    }

    /// The same header with the signature marker cleared. The device-local rollback snapshot uses
    /// it: the bootloader verifies those bytes by CRC, and nothing signs them.
    pub fn unsigned(self) -> ImageHeader {
        ImageHeader { sig_scheme: SIG_SCHEME_NONE, sig_len: 0, ..self }
    }

    pub const fn is_signed(&self) -> bool {
        self.sig_scheme != SIG_SCHEME_NONE
    }

    /// Header, raw image and signature trailer. It is `u64` because the caller usually compares it
    /// against an untrusted on-card file length.
    pub const fn container_len(&self) -> u64 {
        HEADER_LEN as u64 + self.image_len as u64 + self.sig_len as u64
    }

    /// File offset of the signature trailer: `64 + image_len`.
    pub const fn sig_offset(&self) -> u64 {
        HEADER_LEN as u64 + self.image_len as u64
    }

    /// The inverse of [`decode`](Self::decode).
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut b = [0u8; HEADER_LEN];
        b[0..4].copy_from_slice(&MAGIC);
        b[4..6].copy_from_slice(&HEADER_VERSION.to_le_bytes());
        // 6..8 reserved (0)
        b[8..12].copy_from_slice(&self.image_len.to_le_bytes());
        b[12..16].copy_from_slice(&self.image_crc32.to_le_bytes());
        b[16..48].copy_from_slice(&self.fw_version);
        // Bytes 48..52 were reserved in v1, so a zero marker gives a byte-identical header.
        b[48..50].copy_from_slice(&self.sig_scheme.to_le_bytes());
        b[50..52].copy_from_slice(&self.sig_len.to_le_bytes());
        // 52..60 still reserved (0)
        let crc = crc32(&b[..HEADER_CRC_LEN]);
        b[HEADER_CRC_LEN..HEADER_LEN].copy_from_slice(&crc.to_le_bytes());
        b
    }

    /// `None` for bad magic, the wrong version, or a failed header CRC, and nothing else. Whether
    /// this device installs what it decoded is the armer's policy call, not the codec's. Keeping
    /// the reject set this narrow is what lets an older bootloader decode a signed header.
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

    /// The version string with trailing NULs trimmed. Empty when the field is blank or not valid
    /// UTF-8, so a corrupt field never yields garbage.
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
        let long = "0123456789012345678901234567890é"; // 'é' is 2 bytes, starting at index 31
        let h = ImageHeader::new(b"", long);
        assert_eq!(h.fw_version_str(), "0123456789012345678901234567890");
        assert!(core::str::from_utf8(&h.fw_version).is_ok());
    }

    #[test]
    fn rejects_short_is_type_enforced() {
        // decode takes &[u8; 64], so a short buffer cannot reach it. The fixed length is what
        // protects the callers.
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
        let mut b = sample().encode();
        b[8] ^= 0xFF;
        assert!(ImageHeader::decode(&b).is_none());
        let mut b = sample().encode();
        b[60] ^= 0x01;
        assert!(ImageHeader::decode(&b).is_none());
    }

    #[test]
    fn reserved_bytes_are_zero() {
        let b = sample().encode();
        assert_eq!(&b[6..8], &[0, 0]);
        // An unsigned header keeps the whole reserved run zero.
        assert_eq!(&b[48..60], &[0u8; 12]);
    }

    #[test]
    fn signed_marker_lands_in_v1s_reserved_space() {
        let b = sample().signed().encode();
        assert_eq!(&b[48..50], &SIG_SCHEME_ED25519.to_le_bytes(), "sig_scheme at offset 48");
        assert_eq!(&b[50..52], &(SIG_LEN as u16).to_le_bytes(), "sig_len at offset 50");
        assert_eq!(&b[52..60], &[0u8; 8], "the rest of the reserved run is still reserved");
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
        assert_eq!(decoded.unsigned().encode(), sample().encode());
        assert_eq!(sample().container_len(), HEADER_LEN as u64 + 31);
    }

    #[test]
    fn header_version_is_pinned_at_one() {
        // A fielded bootloader rejects anything else, so changing this strands every device in
        // the field.
        assert_eq!(HEADER_VERSION, 1);
        assert_eq!(&sample().signed().encode()[4..6], &1u16.to_le_bytes());
    }

    #[test]
    fn installed_image_must_match_the_bounded_slot() {
        let image = b"running application";
        let header = ImageHeader::new(image, "v1.2.3");
        assert_eq!(matching_image(Some(&header), image), Some(image.as_slice()));

        let mut changed = *image;
        changed[0] ^= 1;
        assert_eq!(matching_image(Some(&header), &changed), None);
        assert_eq!(matching_image(None, image), None);

        let zero = ImageHeader { image_len: 0, ..header };
        let outside = ImageHeader { image_len: MAX_IMAGE_LEN + 1, ..header };
        let past_input = ImageHeader { image_len: image.len() as u32 + 1, ..header };
        assert_eq!(matching_image(Some(&zero), image), None);
        assert_eq!(matching_image(Some(&outside), image), None);
        assert_eq!(matching_image(Some(&past_input), image), None);
    }

    #[test]
    fn vector_table_sp_range() {
        fn sp(word: u32) -> bool {
            let mut image = [0u8; 8];
            image[..4].copy_from_slice(&word.to_le_bytes());
            looks_like_vector_table(&image)
        }
        assert!(sp(0x2002_0000));
        assert!(!sp(0x0000_8000)); // an app-slot LMA, not an SP
        assert!(!looks_like_vector_table(&[0u8; 3]));

        // The board image's own initial SP — the top of the M33's linked RAM, below the
        // coprocessor carve — is inside the range, and the part's RAM end is not.
        assert!(sp(0x2007_8000));
        assert!(sp(RAM_END - 4));
        assert!(!sp(RAM_END));
        assert!(sp(RAM_START));
        assert!(!sp(RAM_START - 4));
    }
}
