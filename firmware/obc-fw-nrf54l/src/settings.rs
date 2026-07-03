//! Persistent settings over the nRF54L's on-chip **RRAM** — the device side of
//! [`obc_app::SettingsStore`], the SD-independent store the simulator mirrors with a file
//! ([`obc-sim/src/settings_store.rs`]).
//!
//! # Why RRAM (and why it's cheap)
//!
//! The nRF54L's program memory *is* RRAM (resistive RAM), not NOR flash — see the crate's
//! `memory-default.x` (`FLASH … the application-core RRAM`). RRAM is **byte-writable with no
//! page-erase**, so a tiny key-value blob is genuinely cheap: a 4 KB page is carved off the top of
//! the RRAM image (the named `SETTINGS` linker region, see below), the fixed-length
//! [`obc_app::settings`] blob (version + fields + CRC, padded to the RRAM line) is written into it,
//! and read back at boot. No wear-levelling gymnastics, no SD card, survives a reboot.
//!
//! # Region & power-loss safety
//!
//! `memory-default.x` / `build.rs` shrink `FLASH` to 1520 KB and reserve the top 4 KB as a named
//! `SETTINGS` region, exporting `__settings_base` (= `ORIGIN(SETTINGS)`). [`region_offset`] reads
//! that symbol's address at runtime, so nothing hard-codes the magic offset and a future MCUboot
//! partition map (#120) can adopt the named region as-is.
//!
//! A **single slot** is used. The CRC already rejects a half-written blob (a power-loss mid-write
//! → `decode` fails → the app boots [`Settings::default`]), and settings only change while the user
//! is in the Settings menu — never mid-ride — so a torn write can lose at most the in-flight edit,
//! not corrupt a ride. The reserved page is sized for a future two-slot + sequence-counter upgrade
//! (write the inactive slot, load the highest valid sequence) if belt-and-braces is ever wanted.

use embassy_nrf::peripherals::RRAMC;
use embassy_nrf::rramc::{Rramc, Unbuffered};
use embassy_nrf::Peri;
use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use obc_app::{Settings, SettingsStore};

/// Bytes the settings slot holds — one encoded blob. Kept here so the RRAM carve sizes from the
/// shared codec rather than a magic number. The RRAMC writes 16-byte lines, so the blob must be a
/// 16-byte multiple — the shared codec rounds `ENCODED_LEN` up to one (currently 32 B: the v2
/// field-selection tail pushed it past 16); the assert below pins that for a future bump.
pub const SLOT_LEN: usize = obc_app::settings::ENCODED_LEN;

/// RRAM write granularity (one 128-bit line). The slot length must be a whole number of these, or
/// [`Rramc::write`] rejects it as unaligned — guard it at compile time so a codec growth fails loud.
const RRAM_WRITE_LINE: usize = 16;
const _: () = assert!(
    SLOT_LEN.is_multiple_of(RRAM_WRITE_LINE),
    "RRAM writes 16-byte lines — pad ENCODED_LEN up to a 16-byte multiple if the codec grows",
);

/// Byte offset of the reserved settings page within the RRAM image: the address of the
/// `__settings_base` linker symbol (`ORIGIN(SETTINGS)` from the memory map). Read at runtime so the
/// magic address lives only in the linker script.
fn region_offset() -> u32 {
    extern "C" {
        static __settings_base: u8;
    }
    core::ptr::addr_of!(__settings_base) as u32
}

/// Byte offset of the **boot-counter line** within the reserved settings page — the diagnostics
/// blob's one persisted fact (issue #275). Placed at the page's midpoint so the low half stays
/// free for the settings slot's future two-slot + sequence upgrade (module doc).
const BOOT_COUNT_OFFSET: u32 = 2048;
/// The boot-counter line's tag; anything else there (a blank page, an older layout) reads as
/// count 0 rather than garbage.
const BOOT_COUNT_MAGIC: [u8; 4] = *b"OBCD";

/// RRAM-backed settings store: owns the [`Rramc`] controller and reads/writes the carved page.
pub struct RramSettingsStore {
    rram: Rramc<'static, Unbuffered>,
    /// This boot's ordinal, set by [`bump_boot_count`](Self::bump_boot_count); 0 before.
    boot_count: u32,
}

impl RramSettingsStore {
    /// Take the `RRAMC` peripheral and build the unbuffered controller (the read path is a plain
    /// memory-mapped slice read; only `save` actually drives the write FSM).
    pub fn new(rram: Peri<'static, RRAMC>) -> Self {
        RramSettingsStore { rram: Rramc::new(rram), boot_count: 0 }
    }

    /// Read-increment-write the persisted boot counter (one aligned 16-byte line, one RRAM write
    /// per boot — nothing against the endurance budget) and return this boot's ordinal. Called
    /// once from `main` on every build, so the counter reflects *device* boots, not just `ble`
    /// ones. A missing/foreign line (blank page, torn write) restarts the count at 1 — the
    /// diagnostics blob is a debugging artifact, not an API (S0 §7.5), so honest-and-simple wins.
    pub fn bump_boot_count(&mut self) -> u32 {
        let off = region_offset() + BOOT_COUNT_OFFSET;
        let mut line = [0u8; RRAM_WRITE_LINE];
        let stored = match self.rram.read(off, &mut line) {
            Ok(()) if line[..4] == BOOT_COUNT_MAGIC => u32::from_le_bytes([line[4], line[5], line[6], line[7]]),
            _ => 0,
        };
        let count = stored.wrapping_add(1);
        let mut out = [0u8; RRAM_WRITE_LINE];
        out[..4].copy_from_slice(&BOOT_COUNT_MAGIC);
        out[4..8].copy_from_slice(&count.to_le_bytes());
        if let Err(e) = self.rram.write(off, &out) {
            defmt::warn!("settings: boot-counter write failed: {}", e);
        }
        self.boot_count = count;
        count
    }

    /// This boot's ordinal (0 if [`bump_boot_count`](Self::bump_boot_count) hasn't run). Only
    /// the BLE diagnostics blob reads it back today (the map build just bumps + logs).
    #[cfg(feature = "ble")]
    pub fn boot_count(&self) -> u32 {
        self.boot_count
    }
}

impl SettingsStore for RramSettingsStore {
    fn load(&mut self) -> Option<Settings> {
        let off = region_offset();
        let mut buf = [0u8; SLOT_LEN];
        match self.rram.read(off, &mut buf) {
            Ok(()) => {
                let settings = obc_app::settings::decode(&buf);
                if settings.is_some() {
                    defmt::info!("settings: loaded {=usize} B from RRAM @ {=u32:#010x}", SLOT_LEN, off);
                } else {
                    defmt::info!("settings: RRAM slot @ {=u32:#010x} blank/invalid → booting defaults", off);
                }
                settings
            }
            Err(e) => {
                defmt::warn!("settings: RRAM read failed: {} → booting defaults", e);
                None
            }
        }
    }

    fn save(&mut self, s: &Settings) {
        let off = region_offset();
        let bytes: [u8; SLOT_LEN] = obc_app::settings::encode(s);
        // No erase: RRAM overwrites in place. One aligned 16-byte line, so this is a single write.
        match self.rram.write(off, &bytes) {
            Ok(()) => defmt::info!("settings: wrote {=usize} B to RRAM @ {=u32:#010x}", SLOT_LEN, off),
            Err(e) => defmt::warn!("settings: RRAM write failed: {}", e),
        }
    }
}
