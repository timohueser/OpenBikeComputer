fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("none") {
        return;
    }
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    // Leave the top 4 KiB for the nRF54L15's reserved RAM and CRACEN protected RAM.
    std::fs::write(
        out.join("memory.x"),
        "MEMORY { FLASH : ORIGIN = 0, LENGTH = 1524K\nRAM : ORIGIN = 0x20000000, LENGTH = 252K }",
    )
    .unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rustc-link-arg-bins=-Tlink.x");
    println!("cargo:rustc-link-arg-bins=-Tdefmt.x");
}
