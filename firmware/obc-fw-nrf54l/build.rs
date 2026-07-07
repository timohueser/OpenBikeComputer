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
    /// Top of the 256 KB SRAM — the carve grows down from here.
    pub const SRAM_TOP: usize = 0x2004_0000;
    /// FLPR execution base: the M33 copies the blob here and points `INITPC` at it. Everything from
    /// here up is the FLPR's (image + stack up to [`CONTROL_ADDR`], then the SHARED page), so the
    /// M33's carved `RAM` region ends here. **4 KB** for the image + stack: the scan blob is ~820 B
    /// with a shallow leaf-call stack (no recursion, no .bss), so 4 KB is still generous — shrunk
    /// from 8 KB when the on-glass stack margin ran out (#347: the M33's residual main stack is
    /// `RAM top − statics`, and the deep-render peak needs every KB this carve doesn't).
    pub const FLPR_RAM_BASE: usize = 0x2003_E000;
    /// The SHARED handshake page base = the control block's address (both cores reach it by this
    /// hardcoded address, never via a linker) = the top of the FLPR's stack.
    pub const CONTROL_ADDR: usize = 0x2003_F000;
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
/// [`contract::FLPR_RAM_BASE`] (248 KB); the top 8 KB is the FLPR's — a 4 KB image/stack + the
/// 4 KB SHARED handshake page. (F0's bring-up `FLPR_RAM` was 28 KB; #165 shrank it to 8 KB, and
/// #347 to 4 KB — the scan blob is ~820 B with a shallow leaf stack, and the M33's deep-render
/// stack margin needs every carved KB back.) The M33 reaches the FLPR region only by hardcoded address (`memcpy` + the
/// handshake word), never via the linker, so shrinking `RAM` is all that's needed here. It *also*
/// carves the top **4 KB of FLASH** into the named `SETTINGS` region for the persistent settings
/// store (#193).
fn flpr_memory_x() -> String {
    use contract::*;
    format!(
        "\
MEMORY
{{
    FLASH    : ORIGIN = 0x00000000, LENGTH = 1520K
    SETTINGS : ORIGIN = 0x0017C000, LENGTH = 4K    /* persistent settings page (#193) — top of RRAM */
    RAM      : ORIGIN = {SRAM_BASE:#010X}, LENGTH = {ram_kb}K   /* M33 .data/.bss/stack */
    /* Reserved for the FLPR (not linked by the M33; see the generated flpr.ld):
         FLPR_RAM {FLPR_RAM_BASE:#010X} .. {CONTROL_ADDR:#010X}  ({flpr_kb}K)   FLPR image + stack (INITPC = {FLPR_RAM_BASE:#010X})
         SHARED   {CONTROL_ADDR:#010X} .. {SRAM_TOP:#010X}  ({shared_kb}K)   cross-core handshake page */
}}
/* Base of the carved settings page (#193). */
PROVIDE(__settings_base = ORIGIN(SETTINGS));
",
        ram_kb = (FLPR_RAM_BASE - SRAM_BASE) / 1024,
        flpr_kb = (CONTROL_ADDR - FLPR_RAM_BASE) / 1024,
        shared_kb = (SRAM_TOP - CONTROL_ADDR) / 1024,
    )
}

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());

    // The FLPR drives the panel on every build (issue #173), so the memory carve + the RISC-V blob
    // are always emitted below. Cargo sets CARGO_FEATURE_<NAME> for the build script when a feature
    // is enabled — used for the `ble`/`has_nav` gates below.

    // The map plane compiles into **every** build (issue #270): map + BLE now coexist in one image —
    // the `ble` build streams the map *and* serves the companion link, both driving the shared SD +
    // settings store, so the old text-only BLE status UI is retired. The `nrf-mem` caps are trimmed
    // (PR #421) so the combined resident set fits the 256 KB DK; the budget assert in main.rs is the
    // binding check. `has_map` stays a cfg (rather than deleting it) so the map-plane seam remains
    // greppable and a future no-map variant can flip it back off in this one place.
    let has_map = true;
    println!("cargo:rustc-check-cfg=cfg(has_map)");
    if has_map {
        println!("cargo:rustc-cfg=has_map");
    }

    // The on-device POI router (epic #116, R4) — `has_map` minus the `ble` build. Its two `.bss`
    // statics (`NavScratch` ~10.2 KB + `NavTileCache` ~4.1 KB) don't fit next to the BLE stack on
    // the 256 KB DK: with them the combined image's stack region lands at ~33.9 KB, ~1.9 KB
    // **below** the ~35.8 KB measured deep-render peak — a silent on-glass overflow — and the
    // acceptance-neutral `nrf-mem` trims are exhausted (PR #496's RAM table). A 256 KB-DK
    // artifact, the same compile-time-fact pattern as main.rs's `MAP_RESIDENT`/`BLE_RESIDENT`
    // arbitration: the 512 KB LM20 deletes this gate and the router rides every build. The gated
    // ride loop still drains a create-route request and answers the generic failure tier
    // ("Couldn't find a route."), so the POI confirm never hangs.
    let ble = env::var_os("CARGO_FEATURE_BLE").is_some();
    let has_nav = has_map && !ble;
    println!("cargo:rustc-check-cfg=cfg(has_nav)");
    if has_nav {
        println!("cargo:rustc-cfg=has_nav");
    }

    fs::write(out.join("memory.x"), flpr_memory_x()).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=build.rs");

    emit_fw_git();
    emit_flpr_contract(&out);

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
/// The M33's carved RAM size (`FLPR_RAM_BASE − SRAM_BASE`) — pub(crate) for main.rs's RAM-budget
/// assert, so the budget can't fork from the carve.
pub(crate) const M33_RAM_BYTES: usize = {M33_RAM_BYTES};
const MAX_DIRTY_SPANS: usize = {MAX_DIRTY_SPANS};
const LAYOUT_MAGIC: u32 = {LAYOUT_MAGIC:#010X};
const FLPR_ALIVE: u32 = {FLPR_ALIVE:#010X};
const FLPR_BADMAG: u32 = {FLPR_BADMAG:#010X};
const CMD_RUN_FRAME: u32 = {CMD_RUN_FRAME:#010X};
",
        M33_RAM_BYTES = FLPR_RAM_BASE - SRAM_BASE,
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
