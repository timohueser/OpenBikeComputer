//! SD-staged field updates: the OBCU byte formats and the pure update logic.
//!
//! `no_std` and `core`-only (no `alloc`, no `heapless`) so the 32 KB `obc-boot` bootloader links it
//! cheaply. Everything is little-endian. Both codecs are CRC-framed: a valid CRC decodes, anything
//! else is rejected (the image header to `None`, the boot-state page to [`BootState::Idle`]).

#![cfg_attr(not(test), no_std)]

pub mod armer;
pub mod blobstage;
pub mod crc32;
pub mod engine;
pub mod image;
pub mod layout;
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
pub use layout::{APP_SLOT_BASE, APP_SLOT_LEN, BOOT_STATE_BASE, SEMMC_STAGE_BASE, SETTINGS_BASE};
pub use sig::{
    public_key_of, sign_image, signing_prefix, verify_image, PublicKey, PUBKEY_LEN, RELEASE_PUBKEY, SEED_LEN,
    SIG_CONTEXT, SIG_LEN, SIG_PREFIX_LEN, SIG_SCHEME_ED25519, SIG_SCHEME_NONE,
};
pub use state::{
    decide, verdict, BootDecision, BootState, EncodedPage, Extent, LastOutcome, OutcomeKind, StagedRef, Verdict,
    MAX_ENCODED_LEN, MAX_EXTENTS, PAGE_LEN, WDT_TIMEOUT_TICKS,
};
