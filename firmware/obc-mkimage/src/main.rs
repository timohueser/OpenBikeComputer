//! `obc-mkimage` — the producer side of the SD-staged DFU pipeline (epic #615).
//!
//! Two subcommands over the shared [`obc_dfu`] OBCU codec:
//!
//! ```text
//! obc-mkimage wrap --bin app.bin --version "$(git describe --always --dirty)" --out UPDATE.BIN
//! obc-mkimage inspect UPDATE.BIN
//! ```
//!
//! `wrap` prepends the 64-byte OBCU header to a raw app image; `inspect` decodes and verifies both
//! CRCs. See `firmware/README.md` (§Firmware update images) for how the raw `.bin` is produced.

use std::path::PathBuf;
use std::process::ExitCode;

use obc_dfu::{looks_like_vector_table, ImageHeader, HEADER_LEN, MAX_IMAGE_LEN, RAM_END, RAM_START};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("wrap") => wrap(&args[1..]),
        Some("inspect") => inspect(&args[1..]),
        Some("--help") | Some("-h") | None => {
            print_usage();
            return ExitCode::SUCCESS;
        }
        Some(other) => Err(format!("unknown subcommand `{other}`\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("obc-mkimage: {e}");
            ExitCode::FAILURE
        }
    }
}

const USAGE: &str = "\
usage:
  obc-mkimage wrap --bin <app.bin> --version <str> --out <UPDATE.BIN>
  obc-mkimage inspect <UPDATE.BIN>";

fn print_usage() {
    println!("{USAGE}");
}

/// `wrap`: prepend the OBCU header to a raw image and write `<out>`.
fn wrap(args: &[String]) -> Result<(), String> {
    let mut bin: Option<PathBuf> = None;
    let mut version: Option<String> = None;
    let mut out: Option<PathBuf> = None;
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--bin" => bin = Some(next_value(&mut it, "--bin")?.into()),
            "--version" => version = Some(next_value(&mut it, "--version")?),
            "--out" => out = Some(next_value(&mut it, "--out")?.into()),
            other => return Err(format!("unexpected argument `{other}`\n\n{USAGE}")),
        }
    }
    let bin = bin.ok_or("wrap: missing --bin")?;
    let version = version.ok_or("wrap: missing --version")?;
    let out = out.ok_or("wrap: missing --out")?;

    let image = std::fs::read(&bin).map_err(|e| format!("reading {}: {e}", bin.display()))?;
    if image.is_empty() {
        return Err(format!("{} is empty", bin.display()));
    }
    if image.len() as u64 > MAX_IMAGE_LEN as u64 {
        return Err(format!(
            "image is {} bytes, over the {MAX_IMAGE_LEN}-byte slot limit — build too large to stage",
            image.len()
        ));
    }
    // Vector-table sanity: a bare-metal .bin starts with the initial SP, which must point into RAM.
    // Warn only (an ELF or wrong-section-order strip fails this), never block.
    if !looks_like_vector_table(&image) {
        if image.len() < 4 {
            // Shorter than the first vector-table word — there is no word0 to report.
            eprintln!(
                "warning: {} is only {} byte(s) — shorter than a vector table's first word; \
                 is it a vector-table-first raw binary?",
                bin.display(),
                image.len()
            );
        } else {
            let word0 = u32::from_le_bytes([image[0], image[1], image[2], image[3]]);
            eprintln!(
                "warning: first word 0x{word0:08X} is not an initial SP in {RAM_START:#010X}..{RAM_END:#010X} — \
                 is {} a vector-table-first raw binary? (a cargo-objcopy strip must be -O binary of the ELF)",
                bin.display()
            );
        }
    }

    let header = ImageHeader::new(&image, &version);
    let mut blob = Vec::with_capacity(HEADER_LEN + image.len());
    blob.extend_from_slice(&header.encode());
    blob.extend_from_slice(&image);
    std::fs::write(&out, &blob).map_err(|e| format!("writing {}: {e}", out.display()))?;

    println!(
        "wrote {} — version \"{}\", image {} bytes, image_crc32 {:#010X}",
        out.display(),
        header.fw_version_str(),
        header.image_len,
        header.image_crc32
    );
    Ok(())
}

/// `inspect`: decode + verify both CRCs, human-readable, non-zero exit on invalid.
fn inspect(args: &[String]) -> Result<(), String> {
    let path: PathBuf = match args {
        [p] => p.into(),
        _ => return Err(format!("inspect: expected exactly one file argument\n\n{USAGE}")),
    };
    let blob = std::fs::read(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    if blob.len() < HEADER_LEN {
        return Err(format!("{} is only {} bytes — shorter than a 64-byte OBCU header", path.display(), blob.len()));
    }
    let header_bytes: &[u8; HEADER_LEN] = blob[..HEADER_LEN].try_into().expect("checked length");
    let header = ImageHeader::decode(header_bytes)
        .ok_or_else(|| format!("{}: not a valid OBCU image (bad magic, version, or header CRC)", path.display()))?;

    let image = &blob[HEADER_LEN..];
    let actual_image_crc = obc_dfu::crc32(image);
    let len_ok = header.image_len as usize == image.len();
    let crc_ok = header.image_crc32 == actual_image_crc;

    println!("{}", path.display());
    println!("  version      : {}", header.fw_version_str());
    println!("  header CRC-32: OK (0x{:08X})", header_crc(header_bytes));
    println!(
        "  image_len    : {} (file carries {} image bytes){}",
        header.image_len,
        image.len(),
        if len_ok { "" } else { "  <-- MISMATCH" }
    );
    println!(
        "  image CRC-32 : {} (header 0x{:08X}, computed 0x{:08X})",
        if crc_ok { "OK" } else { "MISMATCH" },
        header.image_crc32,
        actual_image_crc
    );

    if len_ok && crc_ok {
        println!("  => valid");
        Ok(())
    } else {
        Err(format!("{}: image failed verification", path.display()))
    }
}

/// The header CRC as stored (decode already verified it matches; read it back for the report).
fn header_crc(bytes: &[u8; HEADER_LEN]) -> u32 {
    u32::from_le_bytes([bytes[60], bytes[61], bytes[62], bytes[63]])
}

fn next_value<'a>(it: &mut impl Iterator<Item = &'a String>, flag: &str) -> Result<String, String> {
    it.next().cloned().ok_or_else(|| format!("{flag} needs a value"))
}
