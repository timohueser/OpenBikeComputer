//! Copy the static `memory.x` into `$OUT_DIR` (cortex-m-rt's `link.x` does `INCLUDE memory.x`,
//! resolved via the `-L` search path) and pass the bin link args. The defmt section script is
//! linked only on the `rtt` feature — the default build carries no defmt at all, so the 32 KB
//! budget (CI's size guard) never depends on logging being compiled out by hand.
use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::copy("memory.x", out.join("memory.x")).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rerun-if-changed=build.rs");

    // `-arg-bins` (not `-arg`) so these only apply to the firmware binary, never to
    // build scripts / proc-macros built for the host.
    println!("cargo:rustc-link-arg-bins=--nmagic");
    println!("cargo:rustc-link-arg-bins=-Tlink.x"); // cortex-m-rt; pulls in our memory.x
    if env::var_os("CARGO_FEATURE_RTT").is_some() {
        println!("cargo:rustc-link-arg-bins=-Tdefmt.x"); // defmt's interned-string section
    }
}
