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
//!   0x001F_C000  SETTINGS       4 KB   the app's, never the update path's
//! ```
//!
//! RAM is here for the same reason: the initial-SP plausibility check
//! ([`crate::image::looks_like_vector_table`]) needs the part's bounds, not an image's.

use crate::blobstage::STAGE_LEN;
use crate::state::PAGE_LEN;

/// Base of the app slot: one past the bootloader's 32 KB region.
pub const APP_SLOT_BASE: u32 = 0x0000_8000;

/// Base of the boot-state handoff page, at the top of the map the update path owns.
pub const BOOT_STATE_BASE: u32 = 0x001F_B000;

/// Base of the staged-blob carve, which sits directly below the boot-state page and so ends the
/// app slot.
pub const SEMMC_STAGE_BASE: u32 = BOOT_STATE_BASE - STAGE_LEN as u32;

/// The whole app slot. The install engine writes exactly this span and nothing outside it.
pub const APP_SLOT_LEN: u32 = SEMMC_STAGE_BASE - APP_SLOT_BASE;

/// Base of the app's settings page, one page above the boot-state page. The update path never
/// touches it; it is here because it is what a moved boot-state page collides with.
pub const SETTINGS_BASE: u32 = BOOT_STATE_BASE + PAGE_LEN as u32;

/// Base of the SRAM the M33 sees.
pub const RAM_START: u32 = 0x2000_0000;

/// One past the end of the nRF54LM20's 512 KB SRAM. An image links less than all of it — the top
/// pages are the coprocessor carve — so this is the part's bound, not any image's.
pub const RAM_END: u32 = RAM_START + 512 * 1024;
