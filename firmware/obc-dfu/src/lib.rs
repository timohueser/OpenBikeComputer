//! `obc-dfu` — the SD-staged field-update foundation (epic #615).
//!
//! The byte formats and pure logic every other DFU piece shares, host-tested to the settings-codec
//! bar. `no_std`, `core`-only (no `alloc`, no `heapless`) so the 32 KB `obc-boot` bootloader links it
//! cheaply. Three concerns:
//!
//! - [`image`] — the **OBCU update-image container** (`OBCU_Spec.md` §1): a 64-byte [`ImageHeader`]
//!   (magic, image length + CRC-32, `git describe` version, header CRC-32) prepended to the raw app
//!   image. Produced by the `obc-mkimage` host tool; verified by the armer and bootloader before the
//!   app slot is touched.
//! - [`state`] — the **boot-state RRAM page** (`OBCU_Spec.md` §2): a CRC-framed, torn-write-safe
//!   [`BootState`] blob (`Idle` / `Armed` / `Trial`) handed from the app to the bootloader, plus the
//!   pure [`decide`] function that turns it into a [`BootDecision`].
//! - [`crc32`] — the DFU-side name for the shared CRC-32/IEEE in [`obc_crc`], a dependency-free
//!   leaf (the bootloader must not pull in the BLE stack, and now doesn't have to to share a CRC).
//! - [`engine`] — the bootloader's **install engine** (S3, #618): the verify → flash → readback →
//!   state-transition sequencing as a pure driver over the [`engine::InstallIo`] trait, so the
//!   whole failure matrix (power loss, bad stage, readback retries) is host-tested with mock IO.
//! - [`armer`] — the app-side **armer**'s decision core (S4, #619): the staging-scan validation
//!   matrix, the snapshot-before-page-write arm sequencing, the generation bump, and the trial
//!   confirm, as pure drivers over the [`armer::StageIo`]/[`armer::ArmIo`] traits — host-tested
//!   with mocks (`tests/armer.rs`) exactly like the engine.
//! - [`blobstage`] — the **storage-blob stage carve** (`OBCU_Spec.md` §3, #1158): the CRC-framed
//!   RRAM handoff of the sEMMC soft-peripheral image from the armer to the bootloader (which
//!   cannot carry it and must not read it out of the slot it is about to rewrite), plus the
//!   runtime validation of the image's own soft-peripheral metadata header.
//! - [`sig`] — the **OBCU v2 signature** (`OBCU_Spec.md` §1.3, #997): the domain-separated signed
//!   message, the embedded release public key, and a streaming Ed25519 [`sig::Verifier`]. The
//!   armer is the only place it runs — verify **before arm**; the flash-once bootloader stays
//!   cryptography-free.
//!
//! Everything is little-endian (repo convention, matching OBCM/OBCR). Both codecs follow the
//! settings-store convention: a version + CRC frame, **valid CRC ⇒ `Some`/decoded**, anything else
//! rejected (the image header to `None`, the boot-state page to [`BootState::Idle`]).

#![cfg_attr(not(test), no_std)]

pub mod armer;
pub mod blobstage;
pub mod crc32;
pub mod engine;
pub mod image;
pub mod sig;
pub mod state;

pub use armer::{ArmError, ArmIo, ArmTicket, ExtentsError, Rollback, ScanError, StageIo};
pub use blobstage::{
    encode_stage_header, sp_geometry, validate_stage, SpImageGeometry, MAX_BLOB_LEN, SP_ID_SEMMC, STAGE_HEADER_LEN,
    STAGE_LEN, STAGE_MAGIC, STAGE_VERSION,
};
pub use crc32::{crc32, Crc32};
pub use engine::{InstallIo, IoError, Outcome, Phase, Slot, FLASH_RETRIES, PAD_BYTE, SD_BLOCK_LEN};
pub use image::{
    looks_like_vector_table, ImageHeader, FW_VERSION_LEN, HEADER_LEN, MAGIC, MAX_CONTAINER_LEN, MAX_IMAGE_LEN, RAM_END,
    RAM_START,
};
// No `hex32` / `SigError` / `Verifier` at the root: the key parser is a build-time detail of this
// crate's own `RELEASE_PUBKEY`, and the streaming verifier is the armer's private machinery — a
// caller wanting either reaches through `obc_dfu::sig::` and says so.
pub use sig::{
    public_key_of, sign_image, signing_prefix, verify_image, PublicKey, PUBKEY_LEN, RELEASE_PUBKEY, SEED_LEN,
    SIG_CONTEXT, SIG_LEN, SIG_PREFIX_LEN, SIG_SCHEME_ED25519, SIG_SCHEME_NONE,
};
pub use state::{
    decide, verdict, BootDecision, BootState, EncodedPage, Extent, LastOutcome, OutcomeKind, StagedRef, Verdict,
    MAX_ENCODED_LEN, MAX_EXTENTS, PAGE_LEN, WDT_TIMEOUT_TICKS,
};
