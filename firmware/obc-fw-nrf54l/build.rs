//! Put `memory.x` where the linker can find it, and pass the bin link args
//! (`--nmagic`, cortex-m-rt's `link.x`, defmt's interned-string section). Re-link
//! if `memory.x` changes. Mirrors embassy-nrf's `nrf54l15-app` example build.rs.
//!
//! Under the **`ls021-flpr`** feature (issue #150, epic #149) it additionally (1) emits a
//! *carved* `memory.x` that reserves the top 32 KB of SRAM for the FLPR image + the
//! cross-core handshake, and (2) cross-compiles the freestanding FLPR C blob with a RISC-V
//! gcc into `$OUT_DIR/flpr.bin` for the M33 bin to `include_bytes!`. Both are gated on the
//! feature so the default `main.rs` build keeps the full 256 KB (its `nrf-mem` budget is
//! tight) and needs no RISC-V toolchain. See `firmware/docs/ls021-flpr.md`.
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Carved SRAM layout for the FLPR bring-up: the M33 keeps the low 224 KB; the top 32 KB
/// (`0x2003_8000..0x2004_0000`) is the FLPR's — 28 KB image/stack + a 4 KB shared handshake
/// page. The M33 reaches the FLPR region only by hardcoded address (`memcpy` + the handshake
/// word), never via the linker, so shrinking `RAM` is all that's needed here. Mirrors the
/// region table in `src/flpr/flpr.ld` and `firmware/docs/ls021-flpr.md`.
const FLPR_MEMORY_X: &str = "\
MEMORY
{
    FLASH    : ORIGIN = 0x00000000, LENGTH = 1524K
    RAM      : ORIGIN = 0x20000000, LENGTH = 224K   /* M33 .data/.bss/stack */
    /* Reserved for the FLPR (not linked by the M33; see src/flpr/flpr.ld):
         FLPR_RAM 0x20038000 .. 0x2003F000  (28K)  FLPR image + stack (INITPC = 0x20038000)
         SHARED   0x2003F000 .. 0x20040000  (4K)   cross-core handshake word */
}
";

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());

    // Cargo sets CARGO_FEATURE_<NAME> for the build script when a feature is enabled; the F0
    // bin `required-features = ["ls021-flpr"]`, so `main.rs` builds with this unset.
    let flpr = env::var_os("CARGO_FEATURE_LS021_FLPR").is_some();

    if flpr {
        fs::write(out.join("memory.x"), FLPR_MEMORY_X).unwrap();
    } else {
        fs::write(out.join("memory.x"), include_bytes!("memory.x")).unwrap();
    }
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory.x");
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

/// Cross-compile `src/flpr/{start.S,flpr_frame.c}` against `src/flpr/flpr.ld` into a raw
/// `$OUT_DIR/flpr.bin` the M33 embeds. Freestanding (`-nostdlib -nostartfiles`, integer ops only)
/// so the RV32E core needs no libgcc/newlib multilib — any `rv32emc`-capable GNU gcc works
/// (`brew install riscv64-elf-gcc`, the xPack `riscv-none-elf-gcc`, etc.).
fn build_flpr_blob(manifest: &Path, out: &Path) {
    let flpr_dir = manifest.join("src/flpr");
    let start_s = flpr_dir.join("start.S");
    let blob_c = flpr_dir.join("flpr_frame.c");
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
