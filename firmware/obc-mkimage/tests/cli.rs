//! `obc-mkimage` end-to-end tests: drive the built binary through the real `wrap`/`inspect` flow.

use std::path::PathBuf;
use std::process::Command;

use obc_dfu::{ImageHeader, HEADER_LEN, MAX_IMAGE_LEN};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_obc-mkimage"))
}

/// A unique scratch path under the target dir (no external tempdir dep).
fn scratch(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    p.push(format!("obc-mkimage-{pid}-{nanos}-{name}"));
    p
}

/// A plausible raw image: first word an in-RAM initial SP so the vector-table check passes.
fn fake_image(extra: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&0x2002_0000u32.to_le_bytes()); // initial SP in RAM
    v.extend_from_slice(&0x0000_8101u32.to_le_bytes()); // a reset-vector-ish word
    v.extend_from_slice(extra);
    v
}

/// wrap → inspect → decode the produced file and byte-compare the header fields against a header we
/// build independently. Also asserts inspect exits 0 and prints "valid".
#[test]
fn golden_roundtrip() {
    let bin_path = scratch("app.bin");
    let out_path = scratch("UPDATE.BIN");
    let image = fake_image(b"the application payload, several bytes long");
    std::fs::write(&bin_path, &image).unwrap();

    let version = "v1.4.2-9-gabcdef0-dirty";
    let w = bin()
        .args(["wrap", "--bin"])
        .arg(&bin_path)
        .args(["--version", version, "--out"])
        .arg(&out_path)
        .output()
        .unwrap();
    assert!(w.status.success(), "wrap failed: {}", String::from_utf8_lossy(&w.stderr));

    // The produced file: 64-byte header + the exact raw image.
    let blob = std::fs::read(&out_path).unwrap();
    assert_eq!(&blob[HEADER_LEN..], &image[..], "raw image preserved verbatim");

    let decoded = ImageHeader::decode(blob[..HEADER_LEN].try_into().unwrap()).expect("header decodes");
    let expected = ImageHeader::new(&image, version);
    assert_eq!(decoded, expected, "header fields match an independent build");
    assert_eq!(decoded.fw_version_str(), version);
    assert_eq!(decoded.image_len as usize, image.len());

    let i = bin().arg("inspect").arg(&out_path).output().unwrap();
    assert!(i.status.success(), "inspect should pass a good image");
    let stdout = String::from_utf8_lossy(&i.stdout);
    assert!(stdout.contains("=> valid"), "inspect output: {stdout}");
    assert!(stdout.contains(version));

    let _ = std::fs::remove_file(&bin_path);
    let _ = std::fs::remove_file(&out_path);
}

/// inspect exits non-zero and reports the mismatch when the image bytes are corrupted after wrapping.
#[test]
fn inspect_rejects_corrupted_image() {
    let bin_path = scratch("app2.bin");
    let out_path = scratch("UPDATE2.BIN");
    let image = fake_image(b"payload to corrupt");
    std::fs::write(&bin_path, &image).unwrap();
    let w = bin()
        .args(["wrap", "--bin"])
        .arg(&bin_path)
        .args(["--version", "v0", "--out"])
        .arg(&out_path)
        .output()
        .unwrap();
    assert!(w.status.success());

    let mut blob = std::fs::read(&out_path).unwrap();
    let last = blob.len() - 1;
    blob[last] ^= 0xFF; // flip an image byte; header CRC still valid, image CRC won't be
    std::fs::write(&out_path, &blob).unwrap();

    let i = bin().arg("inspect").arg(&out_path).output().unwrap();
    assert!(!i.status.success(), "inspect must fail a corrupted image");

    let _ = std::fs::remove_file(&bin_path);
    let _ = std::fs::remove_file(&out_path);
}

/// wrap refuses an image over the slot limit (non-zero exit, no output file).
#[test]
fn wrap_rejects_oversize() {
    let bin_path = scratch("huge.bin");
    let out_path = scratch("HUGE.BIN");
    // One byte over the cap. Fill cheaply; content doesn't matter for the size check.
    let image = vec![0u8; MAX_IMAGE_LEN as usize + 1];
    std::fs::write(&bin_path, &image).unwrap();

    let w = bin()
        .args(["wrap", "--bin"])
        .arg(&bin_path)
        .args(["--version", "v0", "--out"])
        .arg(&out_path)
        .output()
        .unwrap();
    assert!(!w.status.success(), "oversize image must be refused");
    assert!(String::from_utf8_lossy(&w.stderr).contains("slot limit"));
    assert!(!out_path.exists(), "no output file on refusal");

    let _ = std::fs::remove_file(&bin_path);
}

/// wrap warns (on stderr) but still succeeds on an image shorter than the vector table's first word
/// — there is no word0 to inspect, and it must not panic. (An *empty* image stays a hard error.)
#[test]
fn wrap_warns_on_tiny_image() {
    let bin_path = scratch("tiny.bin");
    let out_path = scratch("TINY.BIN");
    std::fs::write(&bin_path, [0xAAu8, 0xBB]).unwrap(); // 2 bytes: shorter than one word

    let w = bin()
        .args(["wrap", "--bin"])
        .arg(&bin_path)
        .args(["--version", "v0", "--out"])
        .arg(&out_path)
        .output()
        .unwrap();
    assert!(w.status.success(), "a tiny image is warn-only, not a panic: {}", String::from_utf8_lossy(&w.stderr));
    assert!(String::from_utf8_lossy(&w.stderr).contains("warning"), "expected a short-image warning");
    assert!(out_path.exists(), "file is still written");

    // The wrapped file still inspects clean (the container itself is valid).
    let i = bin().arg("inspect").arg(&out_path).output().unwrap();
    assert!(i.status.success());

    let _ = std::fs::remove_file(&bin_path);
    let _ = std::fs::remove_file(&out_path);
}

/// wrap warns (on stderr) but still succeeds when word0 isn't a plausible initial SP.
#[test]
fn wrap_warns_on_bad_vector_table() {
    let bin_path = scratch("noheader.bin");
    let out_path = scratch("NOHEADER.BIN");
    // First word 0x0000_8000 (an app-slot LMA), not an in-RAM SP.
    let mut image = 0x0000_8000u32.to_le_bytes().to_vec();
    image.extend_from_slice(b"rest of a stripped-wrong binary");
    std::fs::write(&bin_path, &image).unwrap();

    let w = bin()
        .args(["wrap", "--bin"])
        .arg(&bin_path)
        .args(["--version", "v0", "--out"])
        .arg(&out_path)
        .output()
        .unwrap();
    assert!(w.status.success(), "a bad vector table is warn-only, not fatal");
    assert!(String::from_utf8_lossy(&w.stderr).contains("warning"), "expected a vector-table warning");
    assert!(out_path.exists(), "file is still written");

    let _ = std::fs::remove_file(&bin_path);
    let _ = std::fs::remove_file(&out_path);
}
