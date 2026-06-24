//! Put `memory.x` where the linker can find it, and pass the bin link args
//! (`--nmagic`, cortex-m-rt's `link.x`, defmt's interned-string section). Re-link
//! if `memory.x` changes. Mirrors embassy-nrf's `nrf54l15-app` example build.rs.
use std::env;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    File::create(out.join("memory.x")).unwrap().write_all(include_bytes!("memory.x")).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rerun-if-changed=build.rs");

    // `-arg-bins` (not `-arg`) so these only apply to the firmware binary, never to
    // build scripts / proc-macros built for the host.
    println!("cargo:rustc-link-arg-bins=--nmagic");
    println!("cargo:rustc-link-arg-bins=-Tlink.x"); // cortex-m-rt; pulls in our memory.x
    println!("cargo:rustc-link-arg-bins=-Tdefmt.x"); // defmt's interned-string section
}
