//! The storage-blob stage carve (`OBCU_Spec.md`): how the bootloader gets the sEMMC
//! soft-peripheral image that the card is only reachable through.
//!
//! The app ships that image in its own flash. The 32 KB bootloader cannot carry it, and it cannot
//! read it out of the app slot either, because the install engine rewrites the slot while it still
//! streams from the card. So the armer stages the blob into a dedicated RRAM carve below the
//! boot-state page, before every arm:
//!
//! ```text
//!   0x0000_8000  app slot          (shrunk by STAGE_LEN)
//!   0x001F_6000  SEMMC_STAGE       20 KB   ← this module's carve: header line + blob
//!   0x001F_B000  BOOT_STATE page    4 KB
//! ```
//!
//! The carve is one 16-byte header line followed by the raw image bytes, CRC-framed: a valid CRC
//! decodes, anything else is rejected. The armer writes the blob body first and the header line
//! last, then writes `Armed`, so a valid `Armed` page implies a valid carve. The ordering is
//! normative.
//!
//! The bootloader verifies what it did not carry: [`validate_stage`] for the frame, then
//! [`sp_geometry`] for the image's own metadata header. It returns the geometry rather than pin it,
//! so the bootloader derives its VRI base from the staged image and not from a hard-coded offset.

use crate::crc32::Crc32;

/// The carve's total length: five 4 KB RRAM pages, matching the RAM carve the image executes in.
/// The board `build.rs` sizes its `SEMMC_STAGE` linker region from this constant, and `obc-boot`'s
/// static `memory.x` mirrors it by hand.
pub const STAGE_LEN: usize = 20_480;

/// The header line: one 16-byte RRAMC write line.
/// `magic(4) · version u16 LE · blob_len u32 LE · blob_crc32 u32 LE · pad(2)`.
pub const STAGE_HEADER_LEN: usize = 16;

pub const MAX_BLOB_LEN: usize = STAGE_LEN - STAGE_HEADER_LEN;

pub const STAGE_MAGIC: [u8; 4] = *b"OBSB";

/// Bump it on any field change; an old version reads as "no blob staged".
pub const STAGE_VERSION: u16 = 1;

/// `None` when the blob is empty or too large for the carve. The CRC covers the raw blob bytes
/// only; the magic, the version and the length bound protect the header itself.
pub fn encode_stage_header(blob: &[u8]) -> Option<[u8; STAGE_HEADER_LEN]> {
    if blob.is_empty() || blob.len() > MAX_BLOB_LEN {
        return None;
    }
    let mut crc = Crc32::new();
    crc.update(blob);
    let mut h = [0u8; STAGE_HEADER_LEN];
    h[..4].copy_from_slice(&STAGE_MAGIC);
    h[4..6].copy_from_slice(&STAGE_VERSION.to_le_bytes());
    h[6..10].copy_from_slice(&(blob.len() as u32).to_le_bytes());
    h[10..14].copy_from_slice(&crc.finalize().to_le_bytes());
    Some(h)
}

/// Validates a memory-mapped carve and returns the staged blob bytes. `None` for a short slice, a
/// wrong magic or version, a length that does not fit the carve, or a CRC mismatch. It is total
/// over any input and never panics.
pub fn validate_stage(carve: &[u8]) -> Option<&[u8]> {
    if carve.len() < STAGE_HEADER_LEN {
        return None;
    }
    if carve[..4] != STAGE_MAGIC || u16::from_le_bytes([carve[4], carve[5]]) != STAGE_VERSION {
        return None;
    }
    let len = u32::from_le_bytes([carve[6], carve[7], carve[8], carve[9]]) as usize;
    if len == 0 || len > carve.len().saturating_sub(STAGE_HEADER_LEN) || len > MAX_BLOB_LEN {
        return None;
    }
    let want = u32::from_le_bytes([carve[10], carve[11], carve[12], carve[13]]);
    let blob = &carve[STAGE_HEADER_LEN..STAGE_HEADER_LEN + len];
    let mut crc = Crc32::new();
    crc.update(blob);
    if crc.finalize() != want {
        return None;
    }
    Some(blob)
}

/// What a soft-peripheral image's metadata header declares: how much RAM the host reserves and
/// zeroes, and where the VRI register block sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpImageGeometry {
    /// Code region the host reserves and zeroes before it copies the shorter image in.
    pub code_bytes: usize,
    /// The VRI's offset from the image base: the code region plus the firmware's exec and data RAM.
    pub vri_offset: usize,
    pub vri_bytes: usize,
    /// Everything the image occupies at runtime: `vri_offset + vri_bytes`.
    pub image_bytes: usize,
}

/// The sEMMC soft peripheral's id, from the image metadata's `softperiph_id`. The platform half of
/// that word is not checked here, because a flashed-once bootloader must keep accepting a blob for
/// a newer platform revision. The id itself is checked, because driving a different soft peripheral
/// is a silent wedge.
pub const SP_ID_SEMMC: u16 = 0xE33C;

/// Parses and validates the first 32 bytes of a soft-peripheral image's `softperipheral_metadata_t`
/// (nrfxlib `softperipheral_meta.h`, header version 2). The image must be the sEMMC peripheral,
/// spoken over the register interface, host-copied rather than self-booting, and it must fit a
/// `ram_carve`-byte execution carve.
///
/// It is the runtime twin of the board `build.rs` assert, for a blob the bootloader did not carry.
pub fn sp_geometry(blob: &[u8], ram_carve: usize) -> Option<SpImageGeometry> {
    if blob.len() < 32 {
        return None;
    }
    let w = |i: usize| u32::from_le_bytes([blob[i * 4], blob[i * 4 + 1], blob[i * 4 + 2], blob[i * 4 + 3]]);
    let (w0, w1, w3, w6) = (w(0), w(1), w(3), w(6));
    // Magic, metadata version 2, comm id REGIF, and not self-booting: the host copies the image
    // to RAM and points INITPC at it.
    if w0 & 0xFFFF != 0xA005 || (w0 >> 16) & 0xF != 2 || (w0 >> 20) & 0xFF != 1 || w0 >> 31 != 0 {
        return None;
    }
    if (w1 & 0xFFFF) as u16 != SP_ID_SEMMC {
        return None;
    }
    let code_bytes = (w3 & 0xFFFF) as usize * 16;
    let ram_footprint = (w3 >> 16) as usize * 16; // exec/data + VRI, above the code region
    let exec_data_bytes = (w6 >> 16) as usize;
    let vri_bytes = (w6 & 0xFFFF) as usize * 16;
    // The two declarations of the RAM-above-code footprint must agree. Then the placement bounds:
    // the file fits its own code region, and the whole image fits the carve.
    if ram_footprint != exec_data_bytes + vri_bytes {
        return None;
    }
    if blob.len() > code_bytes {
        return None;
    }
    let vri_offset = code_bytes + exec_data_bytes;
    let image_bytes = vri_offset + vri_bytes;
    if vri_bytes == 0 || image_bytes > ram_carve {
        return None;
    }
    Some(SpImageGeometry { code_bytes, vri_offset, vri_bytes, image_bytes })
}
