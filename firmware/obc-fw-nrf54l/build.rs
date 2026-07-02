//! Emit `$OUT_DIR/memory.x` (the linker's region map) and pass the bin link args
//! (`--nmagic`, cortex-m-rt's `link.x`, defmt's interned-string section). Re-link
//! if the source region map changes. Mirrors embassy-nrf's `nrf54l15-app` example build.rs.
//!
//! ⚠️ **Why the default map lives in `memory-default.x`, not `memory.x`.** cortex-m-rt's `link.x`
//! does `INCLUDE memory.x`, and the linker resolves that from its **CWD (the crate root) first** —
//! ahead of the `-L $OUT_DIR` search path. So a `memory.x` committed in the crate root would
//! **shadow** the carved copy this script writes to `$OUT_DIR`, and the FLPR carve would silently
//! never apply (the M33 stack would start at the full-256 KB top and grow down *through* the FLPR
//! image — issue #165: it corrupted the blob on the first deep render). Keeping the source map under
//! a non-`memory.x` name means the *only* `memory.x` the linker can find is the one we emit here.
//!
//! For the FLPR build — the **default** LS021 map/ride `main.rs` (the real app on the LS021 panel,
//! issue #165 / #173) — it additionally (1) emits a *carved* `memory.x` that reserves the top
//! **12 KB** of SRAM for the FLPR image + the cross-core handshake, and (2) cross-compiles the
//! freestanding FLPR C blob with a RISC-V gcc into `$OUT_DIR/flpr.bin` for the M33 binary to
//! `include_bytes!`. Only the opt-in `tft` ST7789 build skips both, keeping the full 256 KB and
//! needing no RISC-V toolchain (see the `flpr` gate in `main` below). See `firmware/docs/ls021-flpr.md`.
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Carved SRAM layout for the FLPR builds: the M33 keeps the low **244 KB**; the top 12 KB
/// (`0x2003_D000..0x2004_0000`) is the FLPR's — an 8 KB image/stack + a 4 KB shared handshake
/// page. (F0's bring-up `FLPR_RAM` was 28 KB; the production blob is ~660 B + a shallow stack, so
/// #165 shrank it to 8 KB, handing ~20 KB back to the M33 so the full app + framebuffer fit — the
/// `SHARED` page is unchanged, so the control-block + ping-pong-buffer addresses did not move.) The
/// M33 reaches the FLPR region only by hardcoded address (`memcpy` + the handshake word), never via
/// the linker, so shrinking `RAM` is all that's needed here. Mirrors the region table in
/// `src/flpr/flpr.ld` and `firmware/docs/ls021-flpr.md`. It *also* carves the top **4 KB of FLASH**
/// into the named `SETTINGS` region for the persistent settings store (#193) — identical to
/// `memory-default.x`, so keep the two in sync.
const FLPR_MEMORY_X: &str = "\
MEMORY
{
    FLASH    : ORIGIN = 0x00000000, LENGTH = 1520K
    SETTINGS : ORIGIN = 0x0017C000, LENGTH = 4K    /* persistent settings page (#193) — top of RRAM */
    RAM      : ORIGIN = 0x20000000, LENGTH = 244K   /* M33 .data/.bss/stack */
    /* Reserved for the FLPR (not linked by the M33; see src/flpr/flpr.ld):
         FLPR_RAM 0x2003D000 .. 0x2003F000  (8K)   FLPR image + stack (INITPC = 0x2003D000)
         SHARED   0x2003F000 .. 0x20040000  (4K)   cross-core handshake word */
}
/* Base of the carved settings page (#193) — kept in sync with memory-default.x. */
PROVIDE(__settings_base = ORIGIN(SETTINGS));
";

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());

    // Cargo sets CARGO_FEATURE_<NAME> for the build script when a feature is enabled. The carve +
    // blob are needed wherever the FLPR drives the panel: the default LS021 map/ride `main.rs` build
    // (issue #173) — the *baseline*, selected by the absence of `tft`. Only the opt-in `tft` ST7789
    // build keeps the full 256 KB and needs no RISC-V toolchain.
    let tft = env::var_os("CARGO_FEATURE_TFT").is_some();
    let flpr = !tft;

    // The map-plane gate (issue #270): on the 256 KB DK the map path and the BLE stack do not
    // coexist, so the `ble` build compiles the map plane out (`App` + `MapCache` + `RouteCache` +
    // `RouteIndex`, ~128 KB) and boots the BLE status UI instead. **This line is the single
    // relaxation point for the 512 KB nRF54LM20**: make `has_map` unconditionally `true` there and
    // both planes compile back in together — the N3 budget assert in main.rs then arbitrates
    // whether they actually fit, at compile time (`ble` + map on 256 KB fails the build).
    let has_map = env::var_os("CARGO_FEATURE_BLE").is_none();
    println!("cargo:rustc-check-cfg=cfg(has_map)");
    if has_map {
        println!("cargo:rustc-cfg=has_map");
    }

    if flpr {
        fs::write(out.join("memory.x"), FLPR_MEMORY_X).unwrap();
    } else {
        fs::write(out.join("memory.x"), include_bytes!("memory-default.x")).unwrap();
    }
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory-default.x");
    println!("cargo:rerun-if-changed=build.rs");

    if flpr {
        build_flpr_blob(&manifest, &out);
    }

    // `-arg-bins` (not `-arg`) so these only apply to the firmware binary, never to
    // build scripts / proc-macros built for the host.
    println!("cargo:rustc-link-arg-bins=--nmagic");
    println!("cargo:rustc-link-arg-bins=-Tlink.x"); // cortex-m-rt; pulls in our memory.x
    println!("cargo:rustc-link-arg-bins=-Tdefmt.x"); // defmt's interned-string section
}

/// Cross-compile `src/flpr/{start.S,flpr_pingpong.c}` against `src/flpr/flpr.ld` into a raw
/// `$OUT_DIR/flpr.bin` the M33 embeds. Freestanding (`-nostdlib -nostartfiles`, integer ops only)
/// so the RV32E core needs no libgcc/newlib multilib — any `rv32emc`-capable GNU gcc works
/// (`brew install riscv64-elf-gcc`, the xPack `riscv-none-elf-gcc`, etc.).
fn build_flpr_blob(manifest: &Path, out: &Path) {
    let flpr_dir = manifest.join("src/flpr");
    let start_s = flpr_dir.join("start.S");
    let blob_c = flpr_dir.join("flpr_pingpong.c");
    let script = flpr_dir.join("flpr.ld");
    for f in [&start_s, &blob_c, &script] {
        println!("cargo:rerun-if-changed={}", f.display());
    }
    println!("cargo:rerun-if-env-changed=RISCV_GCC");
    println!("cargo:rerun-if-env-changed=RISCV_OBJCOPY");

    let gcc = find_riscv_gcc();
    let objcopy = riscv_objcopy(&gcc);
    println!("cargo:warning=FLPR blob: building with {gcc}");

    let elf = out.join("flpr.elf");
    let bin = out.join("flpr.bin");

    run(
        Command::new(&gcc)
            .args(["-march=rv32emc", "-mabi=ilp32e"])
            .args(["-Os", "-ffreestanding", "-nostdlib", "-nostartfiles", "-fno-pic"])
            .args(["-ffunction-sections", "-fdata-sections", "-Wall", "-Wextra"])
            .arg("-Wl,--gc-sections")
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
