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
//! - [`crc32`] — the canonical DFU-side CRC-32/IEEE (the bootloader must not pull in the BLE stack).
//! - [`engine`] — the bootloader's **install engine** (S3, #618): the verify → flash → readback →
//!   state-transition sequencing as a pure driver over the [`engine::InstallIo`] trait, so the
//!   whole failure matrix (power loss, bad stage, readback retries) is host-tested with mock IO.
//! - [`armer`] — the app-side **armer**'s decision core (S4, #619): the staging-scan validation
//!   matrix, the snapshot-before-page-write arm sequencing, the generation bump, and the trial
//!   confirm, as pure drivers over the [`armer::StageIo`]/[`armer::ArmIo`] traits — host-tested
//!   with mocks (`tests/armer.rs`) exactly like the engine.
//!
//! Everything is little-endian (repo convention, matching OBCM/OBCR). Both codecs follow the
//! settings-store convention: a version + CRC frame, **valid CRC ⇒ `Some`/decoded**, anything else
//! rejected (the image header to `None`, the boot-state page to [`BootState::Idle`]).

#![cfg_attr(not(test), no_std)]

pub mod armer;
pub mod crc32;
pub mod engine;
pub mod image;
pub mod state;

pub use armer::{ArmError, ArmIo, ArmTicket, ExtentsError, Rollback, ScanError, StageIo};
pub use crc32::{crc32, Crc32};
pub use engine::{InstallIo, IoError, Outcome, Phase, Slot, FLASH_RETRIES, PAD_BYTE, RRAM_LINE_LEN, SD_BLOCK_LEN};
pub use image::{
    looks_like_vector_table, ImageHeader, FW_VERSION_LEN, HEADER_LEN, HEADER_VERSION, MAGIC, MAX_IMAGE_LEN, RAM_END,
    RAM_START,
};
pub use state::{
    decide, BootDecision, BootState, EncodedPage, Extent, StagedRef, MAX_ENCODED_LEN, MAX_EXTENTS, PAGE_LEN,
    WDT_TIMEOUT_TICKS,
};
