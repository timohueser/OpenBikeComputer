//! Persistent settings over the nRF54L's on-chip **RRAM** — the device side of
//! [`obc_app::SettingsStore`], the SD-independent store the simulator mirrors with a file
//! ([`obc-sim/src/settings_store.rs`]).
//!
//! # Why RRAM (and why it's cheap)
//!
//! The nRF54L's program memory *is* RRAM (resistive RAM), not NOR flash — see the crate's
//! `memory-default.x` (`FLASH … the application-core RRAM`). RRAM is **byte-writable with no
//! page-erase**, so a tiny key-value blob is genuinely cheap: reserve a small region, write the
//! 16-byte [`obc_app::settings`] blob (version + fields + CRC), read it back at boot. No
//! wear-levelling gymnastics, no SD card, survives a reboot. A double-buffered two-slot write
//! (alternate slots + a sequence counter) makes it power-loss-safe; for a value that changes
//! only when the user opens Settings, even a single slot is fine.
//!
//! # Status: wired, RRAM I/O stubbed
//!
//! The boot-load + save-on-dirty calls are wired into `run_app` (so finishing this is a
//! localized edit), but the actual RRAM read/write is **not implemented yet** — it needs a
//! board to verify on glass. To complete it:
//!
//! 1. **Reserve the region.** Carve a page out of the top of the RRAM image in `build.rs`
//!    (the same mechanism as the FLPR carve), or place it via a linker section, so the
//!    settings slot never overlaps the application image. Expose its base/len here.
//! 2. **Write.** RRAM writes go through the RRAM controller (RRAMC) — reachable via `nrf-pac`
//!    registers, or an `embedded-storage` driver if `embassy-nrf` grows one. No erase needed;
//!    write the [`obc_app::settings::encode`] bytes, optionally to the next of two slots.
//! 3. **Read.** RRAM is memory-mapped, so `load` is a plain slice read at the region base,
//!    handed straight to [`obc_app::settings::decode`] (which rejects a blank/corrupt region →
//!    the app falls back to `Settings::default`).

use obc_app::{Settings, SettingsStore};

/// Bytes the settings slot must hold — one encoded blob. Kept here so the future RRAM carve can
/// size the reserved region from the shared codec rather than a magic number.
pub const SLOT_LEN: usize = obc_app::settings::ENCODED_LEN;

/// RRAM-backed settings store. A ZST today (the region is a fixed address once carved); becomes
/// a real read/write against the reserved RRAM page when implemented on glass (see module docs).
#[derive(Default)]
pub struct RramSettingsStore;

impl RramSettingsStore {
    pub fn new() -> Self {
        RramSettingsStore
    }
}

impl SettingsStore for RramSettingsStore {
    fn load(&mut self) -> Option<Settings> {
        // TODO(on-glass): read `SLOT_LEN` bytes from the reserved RRAM region and
        // `obc_app::settings::decode` them. Returning `None` until then makes the device boot
        // from `Settings::default` — correct behaviour for an empty store, just not yet persisted.
        None
    }

    fn save(&mut self, s: &Settings) {
        // TODO(on-glass): write these bytes to the reserved RRAM region (no erase needed). The
        // encode call is kept live so the codec — and `SLOT_LEN` — stay exercised on this side.
        let _bytes: [u8; SLOT_LEN] = obc_app::settings::encode(s);
    }
}
