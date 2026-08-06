//! Emit `$OUT_DIR/memory.x` (the linker's region map) and pass the bin link args
//! (`--nmagic`, cortex-m-rt's `link.x`, defmt's interned-string section). Re-link
//! if the source region map changes. Mirrors embassy-nrf's `nrf54l15-app` example build.rs.
//!
//! ⚠️ **Never commit a `memory.x` in the crate root.** cortex-m-rt's `link.x` does `INCLUDE
//! memory.x`, and the linker resolves that from its **CWD (the crate root) first** — ahead of the
//! `-L $OUT_DIR` search path. So a `memory.x` committed in the crate root would **shadow** the
//! carved copy this script writes to `$OUT_DIR`, and the FLPR carve would silently never apply (the
//! M33 stack would start at the full-256 KB top and grow down *through* the FLPR image — issue #165:
//! it corrupted the blob on the first deep render). The *only* `memory.x` the linker can find is the
//! carved one we emit here.
//!
//! The LS021 map/ride `main.rs` (the real app on the LS021 panel, issue #165 / #173) runs the FLPR
//! on every build, so this script always (1) emits a *carved* `memory.x` that reserves the top of
//! SRAM for the FLPR image + the cross-core handshake, and (2) cross-compiles the freestanding FLPR
//! C blob with a RISC-V gcc into `$OUT_DIR/flpr.bin` for the M33 binary to `include_bytes!`. See
//! `firmware/docs/ls021-flpr.md`.
//!
//! It is also the **single source of the M33↔FLPR cross-core contract** (issue #346): every
//! constant both cores and both linker scripts must agree on — the shared addresses, the layout
//! magic / status stamps, the command codes, the span cap — lives once in [`contract`] below and is
//! *emitted* into `$OUT_DIR` as `flpr_contract.rs` (include!'d by `ls021_flpr.rs`),
//! `flpr_contract.h` (included by `src/flpr/flpr_scan.c`), the carved `memory.x`, and the
//! FLPR's generated `flpr.ld` — so a one-sided edit is impossible by construction.
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// **The M33↔FLPR cross-core contract — the single definition site** (issue #346). Everything here
/// is emitted into the generated Rust/C/linker artifacts; nothing below may be redefined by hand in
/// `ls021_flpr.rs`, `flpr_scan.c`, or a linker script. The *struct layout* (the 96-byte
/// control block) stays hand-mirrored in the `.rs`/`.c` (guarded by the twin size asserts + the
/// boot magic); with the span cap single-sourced here the two sides can no longer disagree on the
/// array length.
mod contract {
    /// M33 SRAM base — the fixed origin the carve is measured from.
    pub const SRAM_BASE: usize = 0x2000_0000;
    /// Top of the carve — NOT the physical 512 KB top (0x2008_0000): the datasheet reserves the
    /// last ~704 B for the VPR saved context (0x2007_FD40) + ProtectedRAM/KMU (0x2007_FF00), and
    /// BLE's CRACEN/KMU path may use ProtectedRAM — so the whole top 4 KB page is left unmapped
    /// rather than shared with them.
    pub const SRAM_TOP: usize = 0x2007_F000;
    /// FLPR execution base: the M33 copies the display blob here and points `INITPC` at it.
    /// Everything from here up is the FLPR's (image + stack up to [`CONTROL_ADDR`], then the SHARED
    /// page). **4 KB** for the image + stack: the scan blob is ~820 B with a shallow leaf-call stack
    /// (no recursion, no .bss), so 4 KB is still generous — shrunk from 8 KB when the on-glass stack
    /// margin ran out (#347: the M33's residual main stack is `RAM top − statics`, and the
    /// deep-render peak needs every KB this carve doesn't).
    ///
    /// Since epic #1158 this is **no longer** the bottom of the carved region — the sEMMC carve
    /// ([`SEMMC_RAM_BASE`]) sits immediately below it, and *that* is where the M33's `RAM` ends.
    pub const FLPR_RAM_BASE: usize = 0x2007_D000;

    // ── The sEMMC soft-peripheral carve (epic #1158) ────────────────────────────────────────────
    //
    // The same FLPR (VPR00) is time-multiplexed between two resident images: the display scan blob
    // above, and Nordic's sEMMC soft peripheral — the SD host controller the card is driven through
    // since the SPI transport was deleted. Both images stay resident and a mode switch only reboots
    // the hart at the other `INITPC` (29 µs storage-ward / 138 µs display-ward, measured), so this
    // carve is **permanent**: storage reads happen mid-render, which rules out funding it from the
    // #1146 scratch arena.
    //
    // The sizes are the image's own (`softperipheral_metadata_t`, decoded in
    // `vendor/semmc/README.md`) and `assert_semmc_blob_metadata` re-derives them from the vendored
    // bytes at build time, so a blob update that changes the footprint fails the build.

    /// Code region the host reserves + zeroes before copying the image in (metadata
    /// `fw_code_size` × 16). The vendored image is 13,636 B; the tail is zero-init.
    pub const SEMMC_CODE_BYTES: usize = 15_360;
    /// The firmware's own exec/data RAM, immediately above the code region (metadata
    /// `fw_shared_ram_addr_offset` — the VRI's offset *within* the firmware's RAM region).
    pub const SEMMC_EXEC_DATA_BYTES: usize = 1_536;
    /// The virtual register interface (metadata `fw_shared_ram_size` × 16) — the 140-byte
    /// `NRF_SP_EMMC_Type` register block the M33 drives the peripheral through, in a 512 B page.
    pub const SEMMC_VRI_BYTES: usize = 512;
    /// VRI offset from the carve base = code + exec/data.
    pub const SEMMC_VRI_OFFSET: usize = SEMMC_CODE_BYTES + SEMMC_EXEC_DATA_BYTES;
    /// Everything the image actually occupies: code + exec/data + VRI.
    pub const SEMMC_IMAGE_BYTES: usize = SEMMC_VRI_OFFSET + SEMMC_VRI_BYTES;
    /// The reserved carve, [`SEMMC_IMAGE_BYTES`] rounded **up to 4 KiB**.
    ///
    /// Why round: the bench placed the image in a `#[repr(C, align(4096))]` static and every
    /// on-glass number was measured at that alignment, and the carve has to end exactly at
    /// [`FLPR_RAM_BASE`] (it is the region directly below it) — so with a 4 KiB-aligned base the
    /// length is necessarily a 4 KiB multiple. 17,408 B rounds to 20,480, i.e. **2,560 B of slack**
    /// is the price of the alignment. If Nordic ever documents a weaker alignment requirement for
    /// `INITPC` / the image base, dropping to a 512 B round would hand those bytes back to the M33
    /// stack; until then this is deliberate, not an oversight.
    pub const SEMMC_CARVE_BYTES: usize = SEMMC_IMAGE_BYTES.div_ceil(4096) * 4096;
    /// sEMMC execution base: the M33 copies the image here and points `INITPC` at it in storage
    /// mode. This is the bottom of the coprocessor carve and therefore the **top of the M33's
    /// linked `RAM` region**.
    pub const SEMMC_RAM_BASE: usize = FLPR_RAM_BASE - SEMMC_CARVE_BYTES;
    /// The SHARED handshake page base = the control block's address (both cores reach it by this
    /// hardcoded address, never via a linker) = the top of the FLPR's stack.
    pub const CONTROL_ADDR: usize = 0x2007_E000;
    /// Dirty-row span-list cap — the `spans[]` length on **both** sides of the contract.
    pub const MAX_DIRTY_SPANS: usize = 16;
    /// Control-block layout/version tag — the FLPR refuses to act otherwise. **v2** (issue #347):
    /// the ping-pong `buf[2]` descriptors left the block; `fb_addr` (the resident framebuffer the
    /// FLPR scans directly) took their place.
    pub const LAYOUT_MAGIC: u32 = 0xF1C0_0002;
    /// FLPR boot confirmation stamp.
    pub const FLPR_ALIVE: u32 = 0x0000_A11E;
    /// FLPR booted but saw the wrong magic (memory-map drift).
    pub const FLPR_BADMAG: u32 = 0x0BAD_CAFE;
    /// M33→FLPR command: drive one span-masked frame.
    pub const CMD_RUN_FRAME: u32 = 0x0000_0002;
}

/// The carved `memory.x` for the FLPR builds, generated from [`contract`]: the M33 keeps SRAM below
/// [`contract::SEMMC_RAM_BASE`] (480 KB on the LM20); above it sit the sEMMC soft-peripheral image
/// (20 KB, #1158), the FLPR display blob's 4 KB image/stack + the 4 KB SHARED handshake page, and
/// the top 4 KB stays unmapped (the VPR-context/ProtectedRAM reservation — see
/// [`contract::SRAM_TOP`]). The M33 reaches both coprocessor regions only by hardcoded address
/// (`memcpy` + the handshake word / the VRI), never via the linker, so shrinking `RAM` is all
/// that's needed here. It *also*
/// carves the RRAM tail (epic #615 S2, #617): the app is linked at **0x8000** — the 32 KB below
/// belong to the `obc-boot` bootloader (`firmware/obc-boot`, its own static `memory.x` — keep the
/// two maps in agreement) — and the top two 4 KB pages are the named `BOOT_STATE` (the obc-dfu
/// handoff page, #617) and `SETTINGS` (the persistent settings store, #193) regions:
///
/// ```text
///   0x0000_0000  obc-boot          32 KB
///   0x0000_8000  app slot        1996 KB   (FLASH below)
///   0x001F_B000  BOOT_STATE page    4 KB
///   0x001F_C000  SETTINGS page      4 KB   (top of the LM20's 2036 KB RRAM)
/// ```
fn flpr_memory_x() -> String {
    use contract::*;
    format!(
        "\
MEMORY
{{
    FLASH      : ORIGIN = 0x00008000, LENGTH = 0x1F3000 /* app slot (1996K) above the 32K obc-boot (#617) */
    BOOT_STATE : ORIGIN = 0x001FB000, LENGTH = 4K    /* DFU boot-state handoff page (#617, OBCU_Spec.md §2) */
    SETTINGS   : ORIGIN = 0x001FC000, LENGTH = 4K    /* persistent settings page (#193) — top of RRAM */
    RAM        : ORIGIN = {SRAM_BASE:#010X}, LENGTH = {ram_kb}K   /* M33 .data/.bss/stack */
    /* Reserved for the FLPR (not linked by the M33; see the generated flpr.ld):
         SEMMC    {SEMMC_RAM_BASE:#010X} .. {FLPR_RAM_BASE:#010X}  ({semmc_kb}K)  sEMMC soft-peripheral image (INITPC = {SEMMC_RAM_BASE:#010X}, VRI at +{SEMMC_VRI_OFFSET})
         FLPR_RAM {FLPR_RAM_BASE:#010X} .. {CONTROL_ADDR:#010X}  ({flpr_kb}K)   FLPR display image + stack (INITPC = {FLPR_RAM_BASE:#010X})
         SHARED   {CONTROL_ADDR:#010X} .. {SRAM_TOP:#010X}  ({shared_kb}K)   cross-core handshake page */
}}
/* Base of the carved settings page (#193). */
PROVIDE(__settings_base = ORIGIN(SETTINGS));
/* Base of the carved boot-state page (#617) — the armer's write target (S4). */
PROVIDE(__boot_state_base = ORIGIN(BOOT_STATE));
/* Base of the app slot (#619) — where the armer's rollback snapshot reads the running image
   from (memory-mapped; RRAM is XIP-readable). The linker map stays the only address authority. */
PROVIDE(__app_slot_base = ORIGIN(FLASH));
",
        ram_kb = (SEMMC_RAM_BASE - SRAM_BASE) / 1024,
        semmc_kb = SEMMC_CARVE_BYTES / 1024,
        flpr_kb = (CONTROL_ADDR - FLPR_RAM_BASE) / 1024,
        shared_kb = (SRAM_TOP - CONTROL_ADDR) / 1024,
    )
}

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());

    // The FLPR drives the panel on every build (issue #173), so the memory carve + the RISC-V blob
    // are always emitted below. Cargo sets CARGO_FEATURE_<NAME> for the build script when a feature
    // is enabled — used for the `ble`/`has_nav` gate below.

    // The map plane compiles into **every** build (issue #270): map + BLE coexist in one image — the
    // `ble` build streams the map *and* serves the companion link, both driving the shared SD +
    // settings store, so the old text-only BLE status UI is retired. The budget assert in main.rs
    // is the binding check.

    // The on-device POI router (epic #116, R4) rides **every** build on the LM20 — `has_nav` was
    // a 256 KB-L15-DK gate (the NavScratch/NavTileCache statics didn't fit beside the BLE stack
    // there) and is now unconditionally on; the cfg stays so the `#[cfg(has_nav)]` sites need no
    // churn, but no build shape turns it off any more.
    println!("cargo:rustc-check-cfg=cfg(has_nav)");
    println!("cargo:rustc-cfg=has_nav");

    fs::write(out.join("memory.x"), flpr_memory_x()).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=build.rs");

    emit_fw_git();
    emit_flpr_contract(&out);
    emit_semmc_contract(&manifest, &out);

    build_flpr_blob(&manifest, &out);

    // `-arg-bins` (not `-arg`) so these only apply to the firmware binary, never to
    // build scripts / proc-macros built for the host.
    println!("cargo:rustc-link-arg-bins=--nmagic");
    println!("cargo:rustc-link-arg-bins=-Tlink.x"); // cortex-m-rt; pulls in our memory.x
    println!("cargo:rustc-link-arg-bins=-Tdefmt.x"); // defmt's interned-string section
}

/// Emit `OBC_FW_GIT` — the short commit hash — for the DIS **Firmware Revision** string (A4,
/// #272: `env!("CARGO_PKG_VERSION") + "+" + OBC_FW_GIT`). Falls back to `unknown` when git isn't
/// reachable (a source tarball / a checkout with no `.git`), so the string is always well-formed.
/// Re-runs when `HEAD` moves so a rebuild reflects the current commit.
fn emit_fw_git() {
    let hash = Command::new("git")
        .args(["rev-parse", "--short=7", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=OBC_FW_GIT={hash}");
    // The repo root is two levels up from this crate; HEAD moving = a checkout / new commit.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
}

/// Emit the [`contract`] into `$OUT_DIR` for both languages: `flpr_contract.rs` (include!'d at the
/// top of `ls021_flpr.rs`'s memory-map section) and `flpr_contract.h` (included by
/// `src/flpr/flpr_scan.c` via the `-I $OUT_DIR` the blob compile passes).
fn emit_flpr_contract(out: &Path) {
    use contract::*;
    let rs = format!(
        "\
// Generated by build.rs from its `contract` module — the single source of the M33↔FLPR
// cross-core contract (issue #346). DO NOT EDIT; change build.rs instead.
const FLPR_RAM_BASE: usize = {FLPR_RAM_BASE:#010X};
const CONTROL_ADDR: usize = {CONTROL_ADDR:#010X};
/// The M33's carved RAM size (`SEMMC_RAM_BASE − SRAM_BASE` — the SRAM below the **lowest**
/// coprocessor carve, which since #1158 is the sEMMC image's, not the display FLPR's) — pub(crate)
/// for main.rs's RAM-budget assert, so the budget can't fork from the carve.
pub(crate) const M33_RAM_BYTES: usize = {M33_RAM_BYTES};
const MAX_DIRTY_SPANS: usize = {MAX_DIRTY_SPANS};
const LAYOUT_MAGIC: u32 = {LAYOUT_MAGIC:#010X};
const FLPR_ALIVE: u32 = {FLPR_ALIVE:#010X};
const FLPR_BADMAG: u32 = {FLPR_BADMAG:#010X};
const CMD_RUN_FRAME: u32 = {CMD_RUN_FRAME:#010X};
",
        M33_RAM_BYTES = SEMMC_RAM_BASE - SRAM_BASE,
    );
    fs::write(out.join("flpr_contract.rs"), rs).unwrap();

    let h = format!(
        "\
/* Generated by build.rs from its `contract` module — the single source of the M33<->FLPR
 * cross-core contract (issue #346). DO NOT EDIT; change build.rs instead. */
#ifndef FLPR_CONTRACT_H
#define FLPR_CONTRACT_H
#define FLPR_CONTROL_ADDR {CONTROL_ADDR:#010X}u
#define MAX_DIRTY_SPANS {MAX_DIRTY_SPANS}u
#define LAYOUT_MAGIC {LAYOUT_MAGIC:#010X}u
#define FLPR_ALIVE {FLPR_ALIVE:#010X}u
#define FLPR_BADMAG {FLPR_BADMAG:#010X}u
#define CMD_RUN_FRAME {CMD_RUN_FRAME:#010X}u
#endif
"
    );
    fs::write(out.join("flpr_contract.h"), h).unwrap();
}

/// The vendored sEMMC image's path, relative to the crate root. `src/semmc.rs` `include_bytes!`s
/// the same file; this script reads it only to cross-check the carve against the image's own
/// metadata header.
const SEMMC_BLOB: &str = "vendor/semmc/semmc_firmware_v0.1.1.bin";

/// Emit the sEMMC half of [`contract`] into `$OUT_DIR/semmc_contract.rs` (include!'d by
/// `src/semmc.rs`), after checking the constants against the vendored image's own metadata header.
/// Same single-definition discipline as [`emit_flpr_contract`]: the carve, the `memory.x` RAM
/// shrink, and the driver's VRI base all come from one place, and that place is now *also* pinned
/// to the blob's declared footprint.
fn emit_semmc_contract(manifest: &Path, out: &Path) {
    use contract::*;
    let blob = manifest.join(SEMMC_BLOB);
    println!("cargo:rerun-if-changed={}", blob.display());
    let bytes =
        fs::read(&blob).unwrap_or_else(|e| panic!("cannot read the vendored sEMMC image {}: {e}", blob.display()));
    assert_semmc_blob_metadata(&bytes);

    let rs = format!(
        "\
// Generated by build.rs from its `contract` module — the single source of the sEMMC carve
// (epic #1158), cross-checked against the vendored image's metadata header. DO NOT EDIT.
/// Carve base — the M33 copies the image here and points `VPR00.INITPC` at it.
const SEMMC_RAM_BASE: usize = {SEMMC_RAM_BASE:#010X};
/// Code region: reserved + zeroed before the (shorter) image is copied in.
const SEMMC_CODE_BYTES: usize = {SEMMC_CODE_BYTES};
/// The virtual register interface's offset from [`SEMMC_RAM_BASE`].
const SEMMC_VRI_OFFSET: usize = {SEMMC_VRI_OFFSET};
/// The VRI page's size (zeroed on every firmware boot).
const SEMMC_VRI_BYTES: usize = {SEMMC_VRI_BYTES};
/// Everything the image occupies — code + exec/data + VRI.
const SEMMC_IMAGE_BYTES: usize = {SEMMC_IMAGE_BYTES};
/// The reserved carve ([`SEMMC_IMAGE_BYTES`] rounded up to 4 KiB).
const SEMMC_CARVE_BYTES: usize = {SEMMC_CARVE_BYTES};
"
    );
    fs::write(out.join("semmc_contract.rs"), rs).unwrap();
}

/// Decode the vendored image's `softperipheral_metadata_t` (nrfxlib
/// `softperipheral/include/softperipheral_meta.h`, header version 2 — the first 32 B of the image)
/// and assert every field the carve is derived from. A Nordic blob update that grows the code
/// region, moves the VRI, or switches to a self-booting layout then fails **here**, loudly, instead
/// of running the FLPR off the end of its carve on glass.
fn assert_semmc_blob_metadata(bytes: &[u8]) {
    use contract::*;
    assert!(bytes.len() >= 32, "sEMMC image is {} B — too short to carry a metadata header", bytes.len());
    let w = |i: usize| u32::from_le_bytes([bytes[i * 4], bytes[i * 4 + 1], bytes[i * 4 + 2], bytes[i * 4 + 3]]);

    let (w0, w1, w3, w6) = (w(0), w(1), w(3), w(6));
    assert_eq!(w0 & 0xFFFF, 0xA005, "sEMMC image: bad soft-peripheral magic");
    assert_eq!((w0 >> 16) & 0xF, 2, "sEMMC image: unexpected metadata header version");
    assert_eq!((w0 >> 20) & 0xFF, 1, "sEMMC image: comm id is not REGIF — this driver speaks the register interface");
    assert_eq!(w0 >> 31, 0, "sEMMC image declares self_boot — the host must NOT copy it to RAM any more");
    // The platform word, pinned to what the shipped v0.1.1 image actually declares:
    // `softperiph_id` 0xE33C and platform.raw 0x2208 = series 54 / platform L / **device 8**, which
    // in the v2 metadata's device enum is `DEVICE_15` — the nRF54L15, not the LM20 (16) this crate
    // targets. That mismatch is real and deliberate to record: the image lives under nrfxlib's
    // `nrf54l/` directory, and it is glass-verified working on the LM20 (#1145, 2026-08-05/06), so
    // the declared device is narrower than the silicon it runs on. Asserting *what is* rather than
    // what we would like means a future image built for a different part — or a different soft
    // peripheral entirely — fails here instead of being copied into the carve and run.
    assert_eq!(w1, 0x2208_E33C, "sEMMC image: unexpected soft-peripheral id / platform word");
    assert!(
        bytes.len() <= SEMMC_CODE_BYTES,
        "sEMMC image ({} B) does not fit the {SEMMC_CODE_BYTES} B code region",
        bytes.len()
    );
    assert_eq!(
        (w3 & 0xFFFF) as usize * 16,
        SEMMC_CODE_BYTES,
        "sEMMC image declares a different code size — re-derive the carve from vendor/semmc/README.md"
    );
    assert_eq!(
        (w3 >> 16) as usize * 16,
        SEMMC_EXEC_DATA_BYTES + SEMMC_VRI_BYTES,
        "sEMMC image declares a different RAM footprint — re-derive the carve"
    );
    assert_eq!((w6 >> 16) as usize, SEMMC_EXEC_DATA_BYTES, "sEMMC image moved the VRI within its RAM region");
    assert_eq!((w6 & 0xFFFF) as usize * 16, SEMMC_VRI_BYTES, "sEMMC image changed the VRI size");
}

/// The FLPR's linker script, generated from [`contract`] so the image base / stack top can't fork
/// from the carve (`memory.x`) or the M33's `INITPC`. The FLPR executes from on-chip SRAM at the
/// *M33-visible* address (no remap); the M33 copies the image to `FLPR_RAM_BASE` and points
/// `VPR00.INITPC` there, so the entry (`_start`, in `.text.start`) is KEPT first. The stack grows
/// down from the top of `FLPR_RAM` — the boundary with the SHARED handshake page (`CONTROL_ADDR`),
/// which is *not* linked on either side (both cores reach it by hardcoded address). Freestanding:
/// no libgcc/newlib, no init/fini arrays.
fn flpr_linker_script() -> String {
    use contract::*;
    format!(
        "\
/* Generated by build.rs from its `contract` module (issue #346). DO NOT EDIT. */
MEMORY
{{
    FLPR_RAM (rwx) : ORIGIN = {FLPR_RAM_BASE:#010X}, LENGTH = {flpr_kb}K
}}

/* Stack grows down from the top of FLPR_RAM, i.e. the boundary with the SHARED block. */
_stack_top = ORIGIN(FLPR_RAM) + LENGTH(FLPR_RAM);

ENTRY(_start)

SECTIONS
{{
    .text ORIGIN(FLPR_RAM) :
    {{
        KEEP(*(.text.start))      /* _start MUST be first — INITPC points right here */
        *(.text .text.*)
        *(.rodata .rodata.*)
    }} > FLPR_RAM

    .data : {{ *(.data .data.*) }} > FLPR_RAM

    .bss :
    {{
        _bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        _bss_end = .;
    }} > FLPR_RAM

    /* Toolchain metadata the bare-metal blob does not need. */
    /DISCARD/ : {{ *(.eh_frame*) *(.comment) *(.riscv.attributes) *(.note*) }}
}}
",
        flpr_kb = (CONTROL_ADDR - FLPR_RAM_BASE) / 1024,
    )
}

/// Cross-compile `src/flpr/{start.S,flpr_scan.c}` against the generated `flpr.ld` into a raw
/// `$OUT_DIR/flpr.bin` the M33 embeds. Freestanding (`-nostdlib -nostartfiles`, integer ops only)
/// so the RV32E core needs no libgcc/newlib multilib — any `rv32emc`-capable GNU gcc works
/// (`brew install riscv64-elf-gcc`, the xPack `riscv-none-elf-gcc`, etc.). `-I $OUT_DIR` puts the
/// generated `flpr_contract.h` on the include path.
fn build_flpr_blob(manifest: &Path, out: &Path) {
    let flpr_dir = manifest.join("src/flpr");
    let start_s = flpr_dir.join("start.S");
    let blob_c = flpr_dir.join("flpr_scan.c");
    for f in [&start_s, &blob_c] {
        println!("cargo:rerun-if-changed={}", f.display());
    }
    println!("cargo:rerun-if-env-changed=RISCV_GCC");
    println!("cargo:rerun-if-env-changed=RISCV_OBJCOPY");

    let script = out.join("flpr.ld");
    fs::write(&script, flpr_linker_script()).unwrap();

    let gcc = find_riscv_gcc();
    let objcopy = riscv_objcopy(&gcc);
    println!("cargo:warning=FLPR blob: building with {gcc}");

    let elf = out.join("flpr.elf");
    let bin = out.join("flpr.bin");

    run(
        Command::new(&gcc)
            .args(["-march=rv32emc", "-mabi=ilp32e"])
            .args(["-O2", "-ffreestanding", "-nostdlib", "-nostartfiles", "-fno-pic"])
            .args(["-ffunction-sections", "-fdata-sections", "-Wall", "-Wextra"])
            .arg("-Wl,--gc-sections")
            .arg("-I")
            .arg(out)
            .arg("-T")
            .arg(&script)
            .arg(&start_s)
            .arg(&blob_c)
            .arg("-o")
            .arg(&elf),
        &gcc,
    );
    run(Command::new(&objcopy).arg("-O").arg("binary").arg(&elf).arg(&bin), &objcopy);
}

/// Locate a RISC-V gcc: `RISCV_GCC` override, else the common bare-metal triples. The blob
/// only needs the compiler's `rv32emc` *code-gen* (no multilib libraries are linked).
fn find_riscv_gcc() -> String {
    if let Some(g) = env::var_os("RISCV_GCC") {
        return g.to_string_lossy().into_owned();
    }
    for cand in ["riscv64-elf-gcc", "riscv-none-elf-gcc", "riscv64-unknown-elf-gcc"] {
        if Command::new(cand).arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
            return cand.to_string();
        }
    }
    panic!(
        "no RISC-V gcc found for the FLPR blob — install one (`brew install riscv64-elf-gcc`) \
         or set RISCV_GCC=<path>. See firmware/docs/ls021-flpr.md (issue #150)."
    );
}

/// The objcopy paired with `gcc`: `RISCV_OBJCOPY` override, else swap the `-gcc` suffix.
fn riscv_objcopy(gcc: &str) -> String {
    if let Some(o) = env::var_os("RISCV_OBJCOPY") {
        return o.to_string_lossy().into_owned();
    }
    match gcc.strip_suffix("-gcc") {
        Some(prefix) => format!("{prefix}-objcopy"),
        None => "riscv64-elf-objcopy".to_string(),
    }
}

fn run(cmd: &mut Command, what: &str) {
    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("failed to spawn {what} for the FLPR blob: {e} — is it installed / on PATH?"));
    assert!(status.success(), "{what} failed building the FLPR blob ({status})");
}
