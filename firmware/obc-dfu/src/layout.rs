//! Where the app slot begins and ends: the one definition the whole update path measures against.
//!
//! Three artifacts must agree on the slot. The board's `memory.x` is generated from these
//! constants (`obc-fw-nrf54l/build.rs`), the bootloader's `memory.x` is static and is pinned to
//! them by this crate's `layout` test, and [`crate::image::MAX_IMAGE_LEN`] is the slot's length.
//!
//! ```text
//!   0x0000_0000  obc-boot      32 KB
//!   0x0000_8000  app slot              APP_SLOT_BASE .. SEMMC_STAGE_BASE
//!   0x001F_6000  SEMMC_STAGE   20 KB   the staged blob (`crate::blobstage`)
//!   0x001F_B000  BOOT_STATE     4 KB
//! ```

use crate::blobstage::STAGE_LEN;
use crate::engine::RRAM_LINE_LEN;

/// Base of the app slot: one past the bootloader's 32 KB region.
pub const APP_SLOT_BASE: u32 = 0x0000_8000;

/// Base of the boot-state handoff page, at the top of the map the update path owns.
pub const BOOT_STATE_BASE: u32 = 0x001F_B000;

/// Base of the staged-blob carve, which sits directly below the boot-state page and so ends the
/// app slot.
pub const SEMMC_STAGE_BASE: u32 = BOOT_STATE_BASE - STAGE_LEN as u32;

/// The whole app slot. The install engine writes exactly this span and nothing outside it.
pub const APP_SLOT_LEN: u32 = SEMMC_STAGE_BASE - APP_SLOT_BASE;

// An image at the cap must still fit once the engine pads its tail up to an RRAM line.
const _: () = assert!(APP_SLOT_LEN.is_multiple_of(RRAM_LINE_LEN as u32));
