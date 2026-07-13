//! Persistent settings over the nRF54L's on-chip **RRAM** — the device side of
//! [`obc_app::SettingsStore`], the SD-independent store the simulator mirrors with a file
//! ([`obc-sim/src/settings_store.rs`]).
//!
//! # Why RRAM (and why it's cheap)
//!
//! The nRF54L's program memory *is* RRAM (resistive RAM), not NOR flash — see the memory map
//! `build.rs` emits (`FLASH … the application-core RRAM`). RRAM is **byte-writable with no
//! page-erase**, so a tiny key-value blob is genuinely cheap: a 4 KB page is carved off the top of
//! the RRAM image (the named `SETTINGS` linker region, see below), the fixed-length
//! [`obc_app::settings`] blob (version + fields + CRC, padded to the RRAM line) is written into it,
//! and read back at boot. No wear-levelling gymnastics, no SD card, survives a reboot.
//!
//! # Region & power-loss safety
//!
//! `build.rs` reserves the top 4 KB of RRAM (`0x0017_C000`, above both the app slot and the
//! DFU `BOOT_STATE` page, #617) as a named `SETTINGS` region, exporting `__settings_base`
//! (= `ORIGIN(SETTINGS)`). [`region_offset`] reads that symbol's address at runtime, so nothing
//! hard-codes the magic offset — which is also why the address survived the #617 bootloader
//! relocation unchanged (settings persist across the update).
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
#[cfg(feature = "ble")]
use trouble_host::prelude::{
    AddrKind, Address, BdAddr, BondInformation, Identity, IdentityResolvingKey, LongTermKey, SecurityLevel,
};

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

/// Base address of the DFU **boot-state page** (#617/#619): the `__boot_state_base` linker symbol
/// (`ORIGIN(BOOT_STATE)`), read at runtime like [`region_offset`] so the linker map stays the only
/// address authority. RRAM starts at 0, so the address doubles as the [`Rramc`] write offset.
fn boot_state_base() -> u32 {
    extern "C" {
        static __boot_state_base: u8;
    }
    core::ptr::addr_of!(__boot_state_base) as u32
}

// The armer writes whole 16-byte RRAMC lines with no read-modify-write — the shared codec pads
// every encoded blob to a line multiple (pinned in obc-dfu too; mirrored here like the settings
// SLOT_LEN guard so a codec change fails loud at this write site's crate).
const _: () = assert!(
    obc_dfu::MAX_ENCODED_LEN.is_multiple_of(RRAM_WRITE_LINE),
    "boot-state blobs must stay 16-byte-line aligned for RRAMC",
);
const _: () = assert!(obc_dfu::MAX_ENCODED_LEN <= obc_dfu::PAGE_LEN);

/// Byte offset of the **boot-counter line** within the reserved settings page — the diagnostics
/// blob's one persisted fact. Placed at the page's midpoint so the low half stays free for the
/// settings slot's future two-slot + sequence upgrade (module doc).
const BOOT_COUNT_OFFSET: u32 = 2048;
/// The boot-counter line's tag; anything else there (a blank page, an older layout) reads as
/// count 0 rather than garbage.
const BOOT_COUNT_MAGIC: [u8; 4] = *b"OBCD";

/// Byte offset of the **durable object-id high-water line** (#450) within the reserved settings
/// page — one 16-byte line holding the next fresh route + ride object id, so an id is never
/// reused after a delete (allocation = `max(scan_max + 1, stored floor)`; the phone's persisted
/// `deviceObjectID` / ride tombstones key on these ids). Placed at the upper half's quarter mark,
/// clear of the other residents; the carve layout is now: **settings slot @0** (low 2 KB reserved
/// for its future two-slot upgrade) · **boot counter @2048** · **arm marker @2064** (48 B) ·
/// **id high-water @2560** · **@2576 RETIRED** · **BLE bond @3072** (64 B). Codec (magic/version/CRC,
/// torn line → "no floor") lives host-tested in [`obc_app::settings`].
///
/// **@2576 is retired** (#776): it held the protocol-v2 store-epoch line (#767), but the epoch is now
/// **card-resident** (`EPOCH.OBE`, so the card carries its own id-era name — a swap transplants the
/// store identity). The offset must **not** be reused for a new record without thought — same
/// discipline as the retired GATT UUID blocks — because a stale foreign write there would need a
/// fresh magic to be safely ignored anyway.
const ID_MARKS_OFFSET: u32 = 2560;
/// The id line is one RRAM write line by construction — pin it so a codec growth fails loud.
const _: () = assert!(obc_app::settings::ID_MARKS_LEN == RRAM_WRITE_LINE);

/// Byte offset of the **DFU arm-marker slot** within the reserved settings page — the armer's
/// breadcrumb, written right after the `Armed` boot-state write and consumed by the boot-outcome
/// reconcile (`dfu::reconcile_boot_outcome`) on the next boot. Placed on the line right after the
/// boot counter (@2048, one line); the carve layout is now: **settings slot @0** (low 2 KB
/// reserved) · **boot counter @2048** · **arm marker @2064** (48 B) · **id high-water @2560** ·
/// **@2576 RETIRED** (former store-epoch line, #767; epoch is now card-resident #776) · **BLE bond
/// @3072**. Codec (magic/version/CRC, torn slot → "no arm happened") lives host-tested in
/// [`obc_app::settings`].
const ARM_MARKER_OFFSET: u32 = 2064;
/// The marker is whole RRAM write lines by construction — pin it so a codec growth fails loud,
/// and pin that it stays clear of the id high-water line.
const _: () = assert!(obc_app::settings::ARM_MARKER_LEN.is_multiple_of(RRAM_WRITE_LINE));
const _: () = assert!(ARM_MARKER_OFFSET + obc_app::settings::ARM_MARKER_LEN as u32 <= ID_MARKS_OFFSET);

/// Byte offset of the **BLE bond slot** within the reserved settings page: the one bonded peer's
/// identity + keys (LTK/IRK), persisted so a power cycle or a firmware reflash lands straight back in
/// the bonded-and-encrypted link. Placed in the page's upper half — clear of the settings slot @0
/// (which reserves the low half for a future two-slot upgrade), the boot counter @2048, the id
/// high-water line @2560, and the retired @2576 (former store-epoch line, now card-resident #776).
/// One slot: a fresh pairing replaces it (single-peer policy).
#[cfg(feature = "ble")]
const BOND_OFFSET: u32 = 3072;
/// Pin the id high-water line (and the retired @2576 gap above it) clear of the bond slot: the
/// 16-byte line at @2560 must end at or before @3072, with @2576 left unused. Guarded here where
/// `BOND_OFFSET` is in scope (both are `cfg(ble)`-relevant).
#[cfg(feature = "ble")]
const _: () = assert!(ID_MARKS_OFFSET + obc_app::settings::ID_MARKS_LEN as u32 <= BOND_OFFSET);
/// The bond slot's tag; anything else there (blank page, torn write, older layout) reads as
/// "no bond" rather than garbage — the device falls back to open pairing.
///
/// `OBCP` (OpenBikeComputer Pairing) — distinct from the boot-state page's `OBCB`
/// (`obc-dfu::state::MAGIC`) and the boot counter's `OBCD`, so the magic actually discriminates
/// these separate CRC-framed RRAM records instead of colliding on one tag (DR4, #732). The tag
/// was `OBCB` before this change: any bond written by an older firmware fails the magic check
/// once and reads as "no bond", so the first connection after this update is a one-time re-pair.
#[cfg(feature = "ble")]
const BOND_MAGIC: [u8; 4] = *b"OBCP";
/// Bond blob layout version (bump on any field change — an old version reads as no bond).
#[cfg(feature = "ble")]
const BOND_VERSION: u8 = 1;
/// The bond slot's fixed length: 4 RRAM lines (a whole number of the 16-byte write granularity).
/// Layout: `magic(4) · version(1) · is_bonded(1) · security_level(1) · addr_kind(1) · addr(6) ·
/// irk_present(1) · pad(1) · LTK(16) · IRK(16) · pad(12) · crc32(4)` over bytes `[0..60]`.
#[cfg(feature = "ble")]
const BOND_SLOT_LEN: usize = 64;

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

    /// Read-increment-write the persisted boot counter and return this boot's ordinal. A
    /// missing/foreign line (blank page, torn write) restarts the count at 1.
    ///
    /// `reset_reas` — this boot's `RESETREAS` snapshot (#349) — rides in the same 16-byte line
    /// (bytes 8..12, previously padding), so the diagnostics blob's one durable fact also records
    /// *why* the device last rebooted: a watchdog boot (`dog0`, bit 1) stays visible after the RTT
    /// log is gone. Same single line write — the annotation is free.
    pub fn bump_boot_count(&mut self, reset_reas: u32) -> u32 {
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
        out[8..12].copy_from_slice(&reset_reas.to_le_bytes());
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

    /// Decode the DFU **boot-state page** (S4, #619) — a plain memory-mapped read (RRAM is
    /// XIP-readable; no controller round-trip, no 4 KB stack buffer). Anything torn/blank/foreign
    /// decodes to `Idle { installed: None }` per the shared codec's safety net (`OBCU_Spec.md`
    /// §2.4). `&mut self` only for symmetry with the write path (the store owns the page).
    pub fn read_boot_state(&mut self) -> obc_dfu::BootState {
        // SAFETY: the linker reserves the 4 KB BOOT_STATE region; RRAM is memory-mapped and
        // always readable, and nothing writes it concurrently (this store is the sole writer,
        // on the one thread-mode executor).
        let page = unsafe { core::slice::from_raw_parts(boot_state_base() as *const u8, obc_dfu::PAGE_LEN) };
        obc_dfu::BootState::decode(page)
    }

    /// Persist a DFU boot state to the BOOT_STATE page (S4, #619) — the armer's page write and
    /// the trial confirm's, through the same `Rramc` line-write path as the settings blob (the
    /// encoded blob is a whole number of 16-byte lines by construction; see the compile guards
    /// above). Returns `false` on a controller error — the caller must **not** proceed to the
    /// reset (an unwritten arm must never reboot the device into the bootloader).
    pub fn write_boot_state(&mut self, state: &obc_dfu::BootState) -> bool {
        let off = boot_state_base();
        let page = state.encode();
        match self.rram.write(off, page.as_bytes()) {
            Ok(()) => {
                defmt::info!("dfu: wrote boot state ({=usize} B) to RRAM @ {=u32:#010x}", page.len(), off);
                true
            }
            Err(e) => {
                defmt::warn!("dfu: boot-state RRAM write failed: {}", e);
                false
            }
        }
    }

    /// Persist the DFU **arm marker** — the armer's breadcrumb, written right after the `Armed`
    /// boot-state write so the next boot can tell a failed install apart from a plain boot.
    /// Best-effort: the caller proceeds to the reboot either way (a missing marker only costs
    /// the failure card its version string, never the install).
    pub fn write_arm_marker(&mut self, marker: &obc_app::settings::ArmMarker) -> bool {
        let off = region_offset() + ARM_MARKER_OFFSET;
        match self.rram.write(off, &obc_app::settings::encode_arm_marker(marker)) {
            Ok(()) => true,
            Err(e) => {
                defmt::warn!("dfu: arm-marker RRAM write failed: {}", e);
                false
            }
        }
    }

    /// Load the DFU arm marker, or `None` when the slot is blank / torn / a foreign layout —
    /// "no arm happened", a plain boot.
    pub fn read_arm_marker(&mut self) -> Option<obc_app::settings::ArmMarker> {
        let off = region_offset() + ARM_MARKER_OFFSET;
        let mut buf = [0u8; obc_app::settings::ARM_MARKER_LEN];
        match self.rram.read(off, &mut buf) {
            Ok(()) => obc_app::settings::decode_arm_marker(&buf),
            Err(e) => {
                defmt::warn!("dfu: arm-marker RRAM read failed: {} → treating as no marker", e);
                None
            }
        }
    }

    /// Clear the DFU arm marker (a zeroed slot decodes to "no arm happened") — called wherever a
    /// boot-outcome verdict is delivered, so each arm's card shows exactly once.
    pub fn clear_arm_marker(&mut self) {
        let off = region_offset() + ARM_MARKER_OFFSET;
        if let Err(e) = self.rram.write(off, &[0u8; obc_app::settings::ARM_MARKER_LEN]) {
            defmt::warn!("dfu: arm-marker RRAM clear failed: {}", e);
        }
    }

    /// Load the durable object-id high-water marks (#450), or `None` when the line is blank /
    /// torn / a foreign layout — "no floor", i.e. allocation falls back to scan-max + 1 exactly
    /// as before the marks existed (fresh devices and reflashes behave identically until the
    /// first delete).
    pub fn load_id_marks(&mut self) -> Option<obc_app::settings::IdMarks> {
        let off = region_offset() + ID_MARKS_OFFSET;
        let mut buf = [0u8; obc_app::settings::ID_MARKS_LEN];
        match self.rram.read(off, &mut buf) {
            Ok(()) => obc_app::settings::decode_id_marks(&buf),
            Err(e) => {
                defmt::warn!("settings: id-marks RRAM read failed: {} → no floor (scan-max+1)", e);
                None
            }
        }
    }

    /// Persist the id high-water marks — one aligned 16-byte line write, no erase; called once
    /// per id assignment (a route upload commit / a ride finish), so the write rate is negligible.
    pub fn save_id_marks(&mut self, m: &obc_app::settings::IdMarks) {
        let off = region_offset() + ID_MARKS_OFFSET;
        let bytes = obc_app::settings::encode_id_marks(m);
        if let Err(e) = self.rram.write(off, &bytes) {
            defmt::warn!("settings: id-marks RRAM write failed: {}", e);
        }
    }

    // The store-epoch nonce (protocol v2, #632 item 5) is **no longer an RRAM record** — it moved to
    // a card-resident file (`EPOCH.OBE`, #776), so the SD card is the sole home of the id-era name (a
    // card swap transplants the store identity). Its load/save live on `sd::Storage`, not here; the
    // former RRAM offset @2576 is documented retired above. The boot mint pass reads the card epoch +
    // this store's id-marks line and writes the epoch back to the card, the marks back here.

    /// Load the stored BLE bond, or `None` when the slot is blank / torn / CRC-bad — in which case
    /// the device advertises open and pairs afresh. Reconstructs the full
    /// [`BondInformation`] (LTK, peer identity + IRK, security level) the host adds to its resolving
    /// list so the bonded phone's rotating RPA reconnect resolves and re-encrypts silently.
    #[cfg(feature = "ble")]
    pub fn load_bond(&mut self) -> Option<BondInformation> {
        let off = region_offset() + BOND_OFFSET;
        let mut buf = [0u8; BOND_SLOT_LEN];
        match self.rram.read(off, &mut buf) {
            Ok(()) => {
                let bond = decode_bond(&buf);
                match &bond {
                    Some(_) => defmt::info!("settings: loaded BLE bond from RRAM @ {=u32:#010x}", off),
                    None => defmt::info!("settings: no valid BLE bond @ {=u32:#010x} → open pairing", off),
                }
                bond
            }
            Err(e) => {
                defmt::warn!("settings: bond RRAM read failed: {} → open pairing", e);
                None
            }
        }
    }

    /// Persist the single BLE bond — a fresh pairing replaces whatever was here (single-peer policy).
    /// One aligned write, no erase (RRAM overwrites in place).
    #[cfg(feature = "ble")]
    pub fn save_bond(&mut self, bond: &BondInformation) {
        let off = region_offset() + BOND_OFFSET;
        let bytes = encode_bond(bond);
        match self.rram.write(off, &bytes) {
            Ok(()) => defmt::info!("settings: wrote BLE bond to RRAM @ {=u32:#010x}", off),
            Err(e) => defmt::warn!("settings: bond RRAM write failed: {}", e),
        }
    }

    /// Clear the stored BLE bond — zero the slot so [`load_bond`](Self::load_bond) reads "no bond"
    /// and the device returns to open pairing. Used when the peer signals it lost
    /// its keys (the app/OS "forgot" the device) so the next contact re-pairs cleanly.
    #[cfg(feature = "ble")]
    pub fn clear_bond(&mut self) {
        let off = region_offset() + BOND_OFFSET;
        let zero = [0u8; BOND_SLOT_LEN];
        match self.rram.write(off, &zero) {
            Ok(()) => defmt::info!("settings: cleared BLE bond @ {=u32:#010x}", off),
            Err(e) => defmt::warn!("settings: bond RRAM clear failed: {}", e),
        }
    }
}

/// Serialize a [`BondInformation`] into the fixed [`BOND_SLOT_LEN`] slot (see the layout note on
/// [`BOND_SLOT_LEN`]), with a trailing CRC-32 over the payload so a torn write reads back invalid.
#[cfg(feature = "ble")]
fn encode_bond(bond: &BondInformation) -> [u8; BOND_SLOT_LEN] {
    let mut buf = [0u8; BOND_SLOT_LEN];
    buf[0..4].copy_from_slice(&BOND_MAGIC);
    buf[4] = BOND_VERSION;
    buf[5] = bond.is_bonded as u8;
    buf[6] = match bond.security_level {
        SecurityLevel::NoEncryption => 0,
        SecurityLevel::Encrypted => 1,
        SecurityLevel::EncryptedAuthenticated => 2,
    };
    buf[7] = bond.identity.addr.kind.into_inner();
    buf[8..14].copy_from_slice(bond.identity.addr.addr.raw());
    match bond.identity.irk {
        Some(irk) => {
            buf[14] = 1;
            buf[32..48].copy_from_slice(&irk.to_le_bytes());
        }
        None => buf[14] = 0,
    }
    buf[16..32].copy_from_slice(&bond.ltk.to_le_bytes());
    let mut crc = obc_ble::Crc32::new();
    crc.update(&buf[..BOND_SLOT_LEN - 4]);
    buf[BOND_SLOT_LEN - 4..].copy_from_slice(&crc.finalize().to_le_bytes());
    buf
}

/// Reconstruct a [`BondInformation`] from a slot, or `None` if the magic / version / CRC don't
/// check out (blank page, torn write, older layout).
#[cfg(feature = "ble")]
fn decode_bond(buf: &[u8; BOND_SLOT_LEN]) -> Option<BondInformation> {
    if buf[0..4] != BOND_MAGIC || buf[4] != BOND_VERSION {
        return None;
    }
    let mut crc = obc_ble::Crc32::new();
    crc.update(&buf[..BOND_SLOT_LEN - 4]);
    let stored = u32::from_le_bytes([buf[60], buf[61], buf[62], buf[63]]);
    if crc.finalize() != stored {
        return None;
    }
    let is_bonded = buf[5] != 0;
    let security_level = match buf[6] {
        0 => SecurityLevel::NoEncryption,
        1 => SecurityLevel::Encrypted,
        _ => SecurityLevel::EncryptedAuthenticated,
    };
    let mut addr = [0u8; 6];
    addr.copy_from_slice(&buf[8..14]);
    let address = Address::new(AddrKind(buf[7]), BdAddr::new(addr));
    let irk = if buf[14] != 0 {
        let mut b = [0u8; 16];
        b.copy_from_slice(&buf[32..48]);
        IdentityResolvingKey::from_le_bytes(b)
    } else {
        None
    };
    let mut ltk = [0u8; 16];
    ltk.copy_from_slice(&buf[16..32]);
    let identity = Identity { addr: address, irk };
    Some(BondInformation::new(identity, LongTermKey::from_le_bytes(ltk), security_level, is_bonded))
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
