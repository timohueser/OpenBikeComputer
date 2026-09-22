//! Persistent settings over the nRF54L's on-chip RRAM: the device side of
//! [`obc_ports::SettingsStore`], the SD-independent store the simulator mirrors with a file.
//!
//! The nRF54L's program memory is RRAM, not NOR flash. RRAM is byte-writable with no page erase, so
//! a small key-value blob is cheap. `build.rs` carves the top 4 KB of the RRAM image as the named
//! `SETTINGS` region and exports `__settings_base`, which [`region_offset`] reads at runtime, so no
//! address is hard-coded here. The fixed-length [`obc_app::settings`] blob (version, fields, CRC,
//! padded to the RRAM line) is written into it and read back at boot.
//!
//! One slot is used. The CRC rejects a half-written blob, so a power loss mid-write boots
//! [`Settings::default`], and settings change only while the user is in the Settings menu, never
//! mid-ride, so a torn write can lose at most the in-flight edit. The reserved page is sized for a
//! future two-slot plus sequence-counter upgrade.

use embassy_nrf::peripherals::RRAMC;
use embassy_nrf::rramc::{Rramc, Unbuffered};
use embassy_nrf::Peri;
use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use obc_app::Settings;
use obc_ports::SettingsStore;
use trouble_host::prelude::{
    AddrKind, Address, BdAddr, BondInformation, Identity, IdentityResolvingKey, LongTermKey, SecurityLevel,
};

/// Bytes the settings slot holds: one encoded blob. The RRAMC writes 16-byte lines, so the blob
/// must be a 16-byte multiple; the shared codec rounds `ENCODED_LEN` up to one, and the assert
/// below pins that.
pub const SLOT_LEN: usize = obc_app::settings::ENCODED_LEN;

/// Bytes the linker reserves for the whole settings page. The linker script is the authority, and
/// this mirror is what the ceiling asserts below hold the carve's last resident against.
const SETTINGS_PAGE_LEN: u32 = 4096;

/// RRAM write granularity (one 128-bit line). The slot length must be a whole number of these, or
/// [`Rramc::write`] rejects it as unaligned.
const RRAM_WRITE_LINE: usize = 16;
const _: () = assert!(
    SLOT_LEN.is_multiple_of(RRAM_WRITE_LINE),
    "RRAM writes 16-byte lines — pad ENCODED_LEN up to a 16-byte multiple if the codec grows",
);

/// Byte offset of the reserved settings page within the RRAM image: the address of the
/// `__settings_base` linker symbol, read at runtime so the address lives only in the linker script.
fn region_offset() -> u32 {
    extern "C" {
        static __settings_base: u8;
    }
    core::ptr::addr_of!(__settings_base) as u32
}

/// Base address of the DFU boot-state page, from the `__boot_state_base` linker symbol. RRAM starts
/// at 0, so the address doubles as the [`Rramc`] write offset.
fn boot_state_base() -> u32 {
    extern "C" {
        static __boot_state_base: u8;
    }
    core::ptr::addr_of!(__boot_state_base) as u32
}

/// Base address of the blob-stage carve, from the `__semmc_stage_base` linker symbol.
fn semmc_stage_base() -> u32 {
    extern "C" {
        static __semmc_stage_base: u8;
    }
    core::ptr::addr_of!(__semmc_stage_base) as u32
}

// The armer writes whole 16-byte RRAMC lines with no read-modify-write, because the shared codec
// pads every encoded blob to a line multiple. Mirrored here so a codec change fails loud.
const _: () = assert!(
    obc_dfu::MAX_ENCODED_LEN.is_multiple_of(RRAM_WRITE_LINE),
    "boot-state blobs must stay 16-byte-line aligned for RRAMC",
);
const _: () = assert!(obc_dfu::MAX_ENCODED_LEN <= obc_dfu::PAGE_LEN);

/// Byte offset of the boot-counter line within the reserved settings page. It sits at the page's
/// midpoint, so the low half stays free for the settings slot's future two-slot upgrade.
const BOOT_COUNT_OFFSET: u32 = 2048;
/// The boot-counter line's tag; anything else there reads as count 0 rather than garbage.
const BOOT_COUNT_MAGIC: [u8; 4] = *b"OBCD";

/// The carve layout is: settings slot @0 (the low 2 KB stays reserved for its future two-slot
/// upgrade), boot counter @2048, arm marker @2064 (48 B), @2560 and @2576 retired, BLE
/// bond @3072 (64 B). A retired offset must not be reused without a fresh magic, because a device
/// that carries the old bytes would decode them as the new record.
const RETIRED_ID_MARKS_OFFSET: u32 = 2560;

/// Byte offset of the DFU arm-marker slot: the armer's breadcrumb, written right after the `Armed`
/// boot-state write and consumed by the boot-outcome reconcile on the next boot. A torn slot reads
/// as "no arm happened".
const ARM_MARKER_OFFSET: u32 = 2064;
/// The marker is whole RRAM write lines by construction, and must stay clear of retired records.
const _: () = assert!(obc_app::dfu::ARM_MARKER_LEN.is_multiple_of(RRAM_WRITE_LINE));
const _: () = assert!(ARM_MARKER_OFFSET + obc_app::dfu::ARM_MARKER_LEN as u32 <= RETIRED_ID_MARKS_OFFSET);

/// Byte offset of the BLE bond slot: the one bonded peer's identity and keys (LTK and IRK),
/// persisted so a power cycle or a firmware reflash lands straight back in the bonded and encrypted
/// link. One slot: a fresh pairing replaces it.
const BOND_OFFSET: u32 = 3072;
const _: () = assert!(RETIRED_ID_MARKS_OFFSET + RRAM_WRITE_LINE as u32 <= BOND_OFFSET);
/// The bond slot's tag; anything else there reads as "no bond" rather than garbage, and the device
/// falls back to open pairing. `OBCP` is distinct from the boot-state page's `OBCB` and the boot
/// counter's `OBCD`, so the magic discriminates these separate CRC-framed RRAM records.
const BOND_MAGIC: [u8; 4] = *b"OBCP";

const _: () = assert!(SLOT_LEN as u32 <= BOOT_COUNT_OFFSET);
const _: () = assert!(BOND_OFFSET + BOND_SLOT_LEN as u32 <= SETTINGS_PAGE_LEN);
/// Bond blob layout version (bump on any field change — an old version reads as no bond).
const BOND_VERSION: u8 = 1;
/// The bond slot's fixed length: 4 RRAM lines (a whole number of the 16-byte write granularity).
/// Layout: `magic(4) · version(1) · is_bonded(1) · security_level(1) · addr_kind(1) · addr(6) ·
/// irk_present(1) · pad(1) · LTK(16) · IRK(16) · pad(12) · crc32(4)` over bytes `[0..60]`.
const BOND_SLOT_LEN: usize = 64;

/// RRAM-backed settings store: owns the [`Rramc`] controller and reads/writes the carved page.
pub struct RramSettingsStore {
    rram: Rramc<'static, Unbuffered>,
    /// This boot's ordinal, set by [`bump_boot_count`](Self::bump_boot_count); 0 before.
    boot_count: u32,
}

impl RramSettingsStore {
    /// Take the `RRAMC` peripheral and build the unbuffered controller. Only `save` drives the
    /// write FSM; the read path is a plain memory-mapped slice read.
    pub fn new(rram: Peri<'static, RRAMC>) -> Self {
        RramSettingsStore { rram: Rramc::new(rram), boot_count: 0 }
    }

    /// Read-increment-write the persisted boot counter and return this boot's ordinal. A missing
    /// or foreign line restarts the count at 1.
    ///
    /// `reset_reas`, this boot's `RESETREAS` snapshot, rides in the same 16-byte line, so the
    /// diagnostics blob also records why the device last rebooted: a watchdog boot stays visible
    /// after the RTT log is gone.
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

    /// Decode the DFU boot-state page: a plain memory-mapped read, because RRAM is XIP-readable.
    /// Anything torn, blank or foreign decodes to `Idle { installed: None }`.
    pub fn read_boot_state(&mut self) -> obc_dfu::BootState {
        // SAFETY: the linker reserves the 4 KB BOOT_STATE region; RRAM is memory-mapped and always
        // readable, and this store is the sole writer, on the one thread-mode executor.
        let page = unsafe { core::slice::from_raw_parts(boot_state_base() as *const u8, obc_dfu::PAGE_LEN) };
        obc_dfu::BootState::decode(page)
    }

    /// Persist a DFU boot state to the BOOT_STATE page, through the same `Rramc` line-write path
    /// as the settings blob. Returns `false` on a controller error, and the caller must then not
    /// proceed to the reset: an unwritten arm must never reboot the device into the bootloader.
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

    /// Stage the sEMMC soft-peripheral image into the `SEMMC_STAGE` carve: the armer's blob handoff
    /// to the bootloader, which boots the card through this copy.
    ///
    /// The write order inside the carve is the commit story. The blob body lands first, in 16-byte
    /// RRAMC lines with a zero-padded tail, and the CRC-framed header line lands last, so a power
    /// cut mid-stage leaves a carve that fails [`obc_dfu::validate_stage`] and is indistinguishable
    /// from "never staged". When the carve already validates to these exact bytes, nothing is
    /// written. Returns `false` on a controller error or a failed readback, and the armer then
    /// aborts with the boot-state page untouched.
    pub fn stage_semmc_blob(&mut self, blob: &[u8]) -> bool {
        let base = semmc_stage_base();
        // SAFETY: the linker reserves the SEMMC_STAGE carve; RRAM is memory-mapped and always
        // readable, and this store is the sole writer.
        let carve = unsafe { core::slice::from_raw_parts(base as *const u8, obc_dfu::STAGE_LEN) };
        if obc_dfu::validate_stage(carve) == Some(blob) {
            defmt::info!("dfu: sEMMC blob already staged ({=usize} B) — skipping the write", blob.len());
            return true;
        }
        let Some(header) = obc_dfu::encode_stage_header(blob) else {
            defmt::warn!("dfu: sEMMC blob ({=usize} B) cannot be staged (empty/oversize)", blob.len());
            return false;
        };
        // Blob body first, in whole 16-byte lines; the tail line is zero-padded.
        let mut off = base + obc_dfu::STAGE_HEADER_LEN as u32;
        for chunk in blob.chunks(256) {
            let mut lines = [0u8; 256];
            lines[..chunk.len()].copy_from_slice(chunk);
            let padded = chunk.len().div_ceil(RRAM_WRITE_LINE) * RRAM_WRITE_LINE;
            if let Err(e) = self.rram.write(off, &lines[..padded]) {
                defmt::warn!("dfu: sEMMC blob stage write failed @ {=u32:#010x}: {}", off, e);
                return false;
            }
            off += padded as u32;
        }
        // The header line is the commit point.
        if let Err(e) = self.rram.write(base, &header) {
            defmt::warn!("dfu: sEMMC blob stage header write failed: {}", e);
            return false;
        }
        // Readback through the same validator the bootloader uses: a stage the bootloader would
        // reject must fail the arm here, where the app can say so.
        let ok = obc_dfu::validate_stage(carve) == Some(blob);
        if ok {
            defmt::info!("dfu: staged sEMMC blob ({=usize} B) to RRAM @ {=u32:#010x}", blob.len(), base);
        } else {
            defmt::warn!("dfu: sEMMC blob stage readback mismatch @ {=u32:#010x}", base);
        }
        ok
    }

    /// Persist the DFU arm marker: the armer's breadcrumb, written right after the `Armed`
    /// boot-state write, so the next boot can tell a failed install from a plain boot. Best-effort,
    /// and the caller proceeds to the reboot either way.
    pub fn write_arm_marker(&mut self, marker: &obc_app::dfu::ArmMarker) -> bool {
        let off = region_offset() + ARM_MARKER_OFFSET;
        match self.rram.write(off, &obc_app::dfu::encode_arm_marker(marker)) {
            Ok(()) => true,
            Err(e) => {
                defmt::warn!("dfu: arm-marker RRAM write failed: {}", e);
                false
            }
        }
    }

    /// Load the DFU arm marker, or `None` when the slot is blank, torn or a foreign layout, which
    /// means no arm happened.
    pub fn read_arm_marker(&mut self) -> Option<obc_app::dfu::ArmMarker> {
        let off = region_offset() + ARM_MARKER_OFFSET;
        let mut buf = [0u8; obc_app::dfu::ARM_MARKER_LEN];
        match self.rram.read(off, &mut buf) {
            Ok(()) => obc_app::dfu::decode_arm_marker(&buf),
            Err(e) => {
                defmt::warn!("dfu: arm-marker RRAM read failed: {} → treating as no marker", e);
                None
            }
        }
    }

    /// Clear the DFU arm marker. Called wherever a boot-outcome verdict is delivered, so each
    /// arm's card shows exactly once.
    pub fn clear_arm_marker(&mut self) {
        let off = region_offset() + ARM_MARKER_OFFSET;
        if let Err(e) = self.rram.write(off, &[0u8; obc_app::dfu::ARM_MARKER_LEN]) {
            defmt::warn!("dfu: arm-marker RRAM clear failed: {}", e);
        }
    }

    /// Load the stored BLE bond, or `None` when the slot is blank, torn or CRC-bad, in which case
    /// the device advertises open and pairs afresh. Reconstructs the full [`BondInformation`] the
    /// host adds to its resolving list, so the bonded phone's rotating address resolves silently.
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

    /// Persist the single BLE bond; a fresh pairing replaces whatever was here. One aligned write,
    /// no erase.
    pub fn save_bond(&mut self, bond: &BondInformation) {
        let off = region_offset() + BOND_OFFSET;
        let bytes = encode_bond(bond);
        match self.rram.write(off, &bytes) {
            Ok(()) => defmt::info!("settings: wrote BLE bond to RRAM @ {=u32:#010x}", off),
            Err(e) => defmt::warn!("settings: bond RRAM write failed: {}", e),
        }
    }

    /// Clear the stored BLE bond, so [`load_bond`](Self::load_bond) reads "no bond" and the device
    /// returns to open pairing. Used when the peer signals it lost its keys.
    pub fn clear_bond(&mut self) -> Result<(), obc_app::ble::BondError> {
        let off = region_offset() + BOND_OFFSET;
        let zero = [0u8; BOND_SLOT_LEN];
        match self.rram.write(off, &zero) {
            Ok(()) => {
                let mut check = [0xff; BOND_SLOT_LEN];
                if self.rram.read(off, &mut check).is_err() || check != zero {
                    return Err(obc_app::ble::BondError::StoreVerifyFailed);
                }
                defmt::info!("settings: cleared BLE bond @ {=u32:#010x}", off);
                Ok(())
            }
            Err(e) => {
                defmt::warn!("settings: bond RRAM clear failed: {}", e);
                Err(obc_app::ble::BondError::StoreWriteFailed)
            }
        }
    }
}

/// Serialize a [`BondInformation`] into the fixed [`BOND_SLOT_LEN`] slot, with a trailing CRC-32
/// over the payload so a torn write reads back invalid.
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

/// Reconstruct a [`BondInformation`] from a slot, or `None` if the magic, version or CRC does not
/// check out.
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
    type Value = Settings;

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

    fn save(&mut self, s: &Settings) -> Result<(), obc_ports::SettingsSaveError> {
        let off = region_offset();
        let bytes: [u8; SLOT_LEN] = obc_app::settings::encode(s);
        // No erase: RRAM overwrites in place. One aligned 16-byte line, so this is a single write.
        match self.rram.write(off, &bytes) {
            Ok(()) => {
                defmt::info!("settings: wrote {=usize} B to RRAM @ {=u32:#010x}", SLOT_LEN, off);
                Ok(())
            }
            Err(e) => {
                defmt::warn!("settings: RRAM write failed: {}", e);
                Err(obc_ports::SettingsSaveError::Backend)
            }
        }
    }
}
