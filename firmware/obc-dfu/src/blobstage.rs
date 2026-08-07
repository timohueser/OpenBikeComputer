//! The **storage-blob stage carve** (`OBCU_Spec.md` §3, epic #1158) — how the bootloader gets the
//! sEMMC soft-peripheral image it cannot afford to carry.
//!
//! Since the storage pivot (#1158) the card is only reachable through Nordic's sEMMC soft
//! peripheral: a ~13.6 KB position-independent RISC-V image the FLPR executes. The app ships that
//! image in its own flash; the 32 KB bootloader cannot (image + driver + the existing engine
//! overflow its carve), and it cannot read it out of the app slot either — the install engine
//! rewrites the slot *while still streaming from the card*, so a power cut mid-flash could destroy
//! the only copy of the thing needed to finish the job. Instead the **armer stages the blob into a
//! dedicated RRAM carve** directly below the BOOT_STATE page, before every arm:
//!
//! ```text
//!   0x0000_8000  app slot          (shrunk by STAGE_LEN)
//!   0x001F_6000  SEMMC_STAGE       20 KB   ← this module's carve: header line + blob
//!   0x001F_B000  BOOT_STATE page    4 KB
//! ```
//!
//! The carve is self-describing — one 16-byte header line (an RRAMC write line) followed by the
//! raw image bytes — and CRC-framed like every other persistent blob in this crate: **valid CRC ⇒
//! `Some`, anything else rejected**. The armer writes the blob body first and the header line
//! *last*, then writes `Armed`; a valid `Armed`/`Rollback` page therefore implies a valid carve
//! (normative ordering, asserted in `tests/armer.rs`).
//!
//! The bootloader trusts nothing it did not verify: [`validate_stage`] for the frame, then
//! [`sp_geometry`] for the image's own `softperipheral_metadata_t` header — the same structural
//! checks the board crate's `build.rs` makes against its vendored copy at build time, minus the
//! exact-platform pin (a future blob that still *is* a REGIF sEMMC image for this series must
//! stay usable by a bootloader that is flashed once and never updated). The geometry is returned
//! rather than pinned so the bootloader derives its VRI base from the staged image instead of a
//! hard-coded offset.

use crate::crc32::Crc32;

/// The carve's total length — five 4 KB RRAM pages, matching the RAM carve the image executes in
/// (`SEMMC_CARVE_BYTES` in the board crate's `build.rs`), so a grown future blob never forces a
/// second layout change. The board `build.rs` sizes the `SEMMC_STAGE` linker region from this
/// constant; `obc-boot`'s static `memory.x` mirrors it by hand (the existing two-maps convention).
pub const STAGE_LEN: usize = 20_480;

/// The header line: one 16-byte RRAMC write line.
/// `magic(4) · version u16 LE · blob_len u32 LE · blob_crc32 u32 LE · pad(2)`.
pub const STAGE_HEADER_LEN: usize = 16;

/// Largest blob the carve can stage.
pub const MAX_BLOB_LEN: usize = STAGE_LEN - STAGE_HEADER_LEN;

/// Carve magic: **O**pen**B**ikeComputer **S**taged **B**lob.
pub const STAGE_MAGIC: [u8; 4] = *b"OBSB";

/// Header layout version — bump on any field change (an old version reads as "no blob staged").
pub const STAGE_VERSION: u16 = 1;

/// Encode the header line for `blob`, or `None` when the blob cannot be staged (empty, or too
/// large for the carve). The CRC-32 is over the raw blob bytes only — the header is protected by
/// its own magic + version + the length bound.
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

/// Validate a memory-mapped carve and return the staged blob bytes.
///
/// `None` for anything that is not a well-formed stage: short slice, wrong magic/version, a length
/// that doesn't fit the carve, or a CRC mismatch (a torn or interrupted stage — unreachable from a
/// valid arm, by the write ordering, but the bootloader never assumes that). Total over any input;
/// never panics.
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

/// What a soft-peripheral image's own metadata header declares — everything a host needs to place
/// and drive it: how much RAM to reserve + zero, and where the VRI register block sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpImageGeometry {
    /// Code region the host reserves + zeroes before copying the (shorter) image in
    /// (`fw_code_size` × 16).
    pub code_bytes: usize,
    /// The VRI's offset from the image base (= code region + the firmware's own exec/data RAM).
    pub vri_offset: usize,
    /// The VRI register block's size (`fw_shared_ram_size` × 16).
    pub vri_bytes: usize,
    /// Everything the image occupies at runtime: `vri_offset + vri_bytes`.
    pub image_bytes: usize,
}

/// The sEMMC soft peripheral's id, from the shipped image's metadata (`softperiph_id`). The
/// *platform* half of that word is deliberately **not** checked here — the board `build.rs` pins
/// it exactly against the vendored bytes; a flashed-once bootloader must keep accepting a future
/// blob for a newer platform revision, but must never drive a *different* soft peripheral
/// (a UART image booted as an SD host is a silent wedge) — hence the id check.
pub const SP_ID_SEMMC: u16 = 0xE33C;

/// Parse and structurally validate a soft-peripheral image's `softperipheral_metadata_t` (nrfxlib
/// `softperipheral_meta.h`, header version 2 — the first 32 bytes), for an image that must be the
/// **sEMMC** peripheral, spoken over the **register interface**, host-copied (not self-booting),
/// and fit a `ram_carve`-byte execution carve.
///
/// Mirrors `obc-fw-nrf54l/build.rs::assert_semmc_blob_metadata` as a total runtime check: the
/// bootloader runs this over a staged blob it did not carry, where a build assert can't reach.
/// Returns the declared geometry so the caller derives its VRI base from the image itself.
pub fn sp_geometry(blob: &[u8], ram_carve: usize) -> Option<SpImageGeometry> {
    if blob.len() < 32 {
        return None;
    }
    let w = |i: usize| u32::from_le_bytes([blob[i * 4], blob[i * 4 + 1], blob[i * 4 + 2], blob[i * 4 + 3]]);
    let (w0, w1, w3, w6) = (w(0), w(1), w(3), w(6));
    // Magic, metadata version 2, comm id REGIF (this driver speaks the register interface), and
    // not self-booting (the host copies the image to RAM and points INITPC at it).
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
    // Internal consistency (the two declarations of the RAM-above-code footprint must agree),
    // then the placement bounds: the file fits its own code region, the whole image fits the carve.
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
