//! `obc-mkimage` end-to-end tests: drive the built binary through the real
//! `keygen`/`wrap`/`sign`/`inspect` flow, including the OBCU v2 signature (#997).

use std::path::PathBuf;
use std::process::Command;

use obc_dfu::{crc32, ImageHeader, HEADER_LEN, MAX_IMAGE_LEN, SIG_LEN, SIG_SCHEME_ED25519};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_obc-mkimage"))
}

/// The committed test keypair (`firmware/obc-dfu/keys/test/`), resolved from this crate's root.
fn test_key(ext: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../firmware/obc-dfu/keys/test").join(format!("obcu-test.{ext}"))
}

/// A unique scratch path. Nothing is created: most of these name a file the CLI under test is
/// asked to write, and one names a directory it must create itself.
fn scratch(name: &str) -> PathBuf {
    obcm_testkit::scratch::scratch_path("obc-mkimage", name)
}

/// A plausible raw image: first word an in-RAM initial SP so the vector-table check passes.
fn fake_image(extra: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&0x2002_0000u32.to_le_bytes()); // initial SP in RAM
    v.extend_from_slice(&0x0000_8101u32.to_le_bytes()); // a reset-vector-ish word
    v.extend_from_slice(extra);
    v
}

/// wrap+sign → inspect → decode the produced file and byte-compare the header fields against a
/// header we build independently. Also asserts inspect exits 0 and prints "valid".
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
        .arg("--sign-seed")
        .arg(test_key("seed"))
        .output()
        .unwrap();
    assert!(w.status.success(), "wrap failed: {}", String::from_utf8_lossy(&w.stderr));

    // The produced file: 64-byte header + the exact raw image + the 64-byte signature trailer.
    let blob = std::fs::read(&out_path).unwrap();
    assert_eq!(blob.len(), HEADER_LEN + image.len() + SIG_LEN);
    assert_eq!(&blob[HEADER_LEN..HEADER_LEN + image.len()], &image[..], "raw image preserved verbatim");

    let decoded = ImageHeader::decode(blob[..HEADER_LEN].try_into().unwrap()).expect("header decodes");
    let expected = ImageHeader::new(&image, version).signed();
    assert_eq!(decoded, expected, "header fields match an independent build");
    assert_eq!(decoded.fw_version_str(), version);
    assert_eq!(decoded.image_len as usize, image.len());
    assert_eq!(decoded.sig_scheme, SIG_SCHEME_ED25519);
    assert_eq!(decoded.sig_len as usize, SIG_LEN);

    let i = bin().arg("inspect").arg(&out_path).arg("--pubkey").arg(test_key("pub")).output().unwrap();
    assert!(i.status.success(), "inspect should pass a good image: {}", String::from_utf8_lossy(&i.stdout));
    let stdout = String::from_utf8_lossy(&i.stdout);
    assert!(stdout.contains("=> valid"), "inspect output: {stdout}");
    assert!(stdout.contains(version));

    let _ = std::fs::remove_file(&bin_path);
    let _ = std::fs::remove_file(&out_path);
}

/// Signing is deterministic: the same seed + version + image always produces the same container.
/// A release artifact is reproducible, and the committed spec vector can be a fixed file.
#[test]
fn signing_is_reproducible() {
    let bin_path = scratch("repro.bin");
    let a_path = scratch("REPRO-A.BIN");
    let b_path = scratch("REPRO-B.BIN");
    std::fs::write(&bin_path, fake_image(b"reproducible payload")).unwrap();

    for out in [&a_path, &b_path] {
        let w = bin()
            .args(["wrap", "--bin"])
            .arg(&bin_path)
            .args(["--version", "v1.0.0", "--out"])
            .arg(out)
            .arg("--sign-seed")
            .arg(test_key("seed"))
            .output()
            .unwrap();
        assert!(w.status.success());
    }
    assert_eq!(std::fs::read(&a_path).unwrap(), std::fs::read(&b_path).unwrap());

    let _ = std::fs::remove_file(&bin_path);
    let _ = std::fs::remove_file(&a_path);
    let _ = std::fs::remove_file(&b_path);
}

/// The `sign` subcommand attaches a trailer to an already-wrapped container, and the result is
/// byte-identical to what `wrap --sign-seed` would have produced in one step.
#[test]
fn sign_matches_wrap_sign_seed() {
    let bin_path = scratch("late.bin");
    let unsigned_path = scratch("LATE-UNSIGNED.BIN");
    let signed_path = scratch("LATE-SIGNED.BIN");
    let one_shot_path = scratch("LATE-ONESHOT.BIN");
    std::fs::write(&bin_path, fake_image(b"signed after the fact")).unwrap();

    let w = bin()
        .args(["wrap", "--bin"])
        .arg(&bin_path)
        .args(["--version", "v2.0.0", "--out"])
        .arg(&unsigned_path)
        .output()
        .unwrap();
    assert!(w.status.success());
    assert!(String::from_utf8_lossy(&w.stderr).contains("UNSIGNED"), "an unsigned wrap warns loudly");

    let s = bin()
        .args(["sign", "--in"])
        .arg(&unsigned_path)
        .arg("--out")
        .arg(&signed_path)
        .arg("--seed")
        .arg(test_key("seed"))
        .output()
        .unwrap();
    assert!(s.status.success(), "sign failed: {}", String::from_utf8_lossy(&s.stderr));

    let w2 = bin()
        .args(["wrap", "--bin"])
        .arg(&bin_path)
        .args(["--version", "v2.0.0", "--out"])
        .arg(&one_shot_path)
        .arg("--sign-seed")
        .arg(test_key("seed"))
        .output()
        .unwrap();
    assert!(w2.status.success());
    assert_eq!(std::fs::read(&signed_path).unwrap(), std::fs::read(&one_shot_path).unwrap());

    let i = bin().arg("inspect").arg(&signed_path).arg("--pubkey").arg(test_key("pub")).output().unwrap();
    assert!(i.status.success());

    for p in [&bin_path, &unsigned_path, &signed_path, &one_shot_path] {
        let _ = std::fs::remove_file(p);
    }
}

/// The seed can come from the environment instead of a file — how CI signs without ever writing the
/// secret to disk.
#[test]
fn sign_seed_from_env() {
    let bin_path = scratch("env.bin");
    let out_path = scratch("ENV.BIN");
    std::fs::write(&bin_path, fake_image(b"signed from the environment")).unwrap();
    let seed_hex = std::fs::read_to_string(test_key("seed")).unwrap();

    let w = bin()
        .args(["wrap", "--bin"])
        .arg(&bin_path)
        .args(["--version", "v3.0.0", "--out"])
        .arg(&out_path)
        .args(["--sign-seed-env", "OBCU_TEST_SEED"])
        .env("OBCU_TEST_SEED", seed_hex.trim())
        .output()
        .unwrap();
    assert!(w.status.success(), "env-sourced seed: {}", String::from_utf8_lossy(&w.stderr));

    let i = bin().arg("inspect").arg(&out_path).arg("--pubkey").arg(test_key("pub")).output().unwrap();
    assert!(i.status.success());

    // …and an unset variable is a clean error, not a silent unsigned build.
    let miss = bin()
        .args(["wrap", "--bin"])
        .arg(&bin_path)
        .args(["--version", "v3.0.0", "--out"])
        .arg(&out_path)
        .args(["--sign-seed-env", "OBCU_DEFINITELY_UNSET_SEED"])
        .env_remove("OBCU_DEFINITELY_UNSET_SEED")
        .output()
        .unwrap();
    assert!(!miss.status.success());
    assert!(String::from_utf8_lossy(&miss.stderr).contains("is not set"));

    let _ = std::fs::remove_file(&bin_path);
    let _ = std::fs::remove_file(&out_path);
}

/// `keygen` produces a usable pair: sign with the new seed, verify with the new pubkey — and the
/// committed test key must *not* verify it (the rotation story, end to end).
#[test]
fn keygen_produces_a_working_pair() {
    let dir = scratch("keygen-dir");
    std::fs::create_dir_all(&dir).unwrap();
    let bin_path = scratch("kg.bin");
    let out_path = scratch("KG.BIN");
    std::fs::write(&bin_path, fake_image(b"signed with a fresh key")).unwrap();

    let k = bin().args(["keygen", "--out-dir"]).arg(&dir).args(["--name", "fresh"]).output().unwrap();
    assert!(k.status.success(), "keygen failed: {}", String::from_utf8_lossy(&k.stderr));
    let seed = dir.join("fresh.seed");
    let public = dir.join("fresh.pub");
    assert_eq!(std::fs::read_to_string(&seed).unwrap().trim().len(), 64, "seed is 64 hex chars");
    assert_eq!(std::fs::read_to_string(&public).unwrap().trim().len(), 64, "pubkey is 64 hex chars");

    // A second keygen into the same names must refuse rather than clobber a key.
    let again = bin().args(["keygen", "--out-dir"]).arg(&dir).args(["--name", "fresh"]).output().unwrap();
    assert!(!again.status.success(), "keygen must never overwrite an existing key");

    let w = bin()
        .args(["wrap", "--bin"])
        .arg(&bin_path)
        .args(["--version", "v4.0.0", "--out"])
        .arg(&out_path)
        .arg("--sign-seed")
        .arg(&seed)
        .output()
        .unwrap();
    assert!(w.status.success());

    let ok = bin().arg("inspect").arg(&out_path).arg("--pubkey").arg(&public).output().unwrap();
    assert!(ok.status.success(), "the fresh pair verifies its own image");

    let wrong = bin().arg("inspect").arg(&out_path).arg("--pubkey").arg(test_key("pub")).output().unwrap();
    assert!(!wrong.status.success(), "a different key must not verify it");
    assert!(String::from_utf8_lossy(&wrong.stdout).contains("INVALID"));

    let _ = std::fs::remove_file(&bin_path);
    let _ = std::fs::remove_file(&out_path);
    let _ = std::fs::remove_dir_all(&dir);
}

/// inspect is CI's gate: every way a container can be wrong exits non-zero.
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
        .arg("--sign-seed")
        .arg(test_key("seed"))
        .output()
        .unwrap();
    assert!(w.status.success());
    let good = std::fs::read(&out_path).unwrap();

    // A flipped image byte: the image CRC breaks (and so would the signature).
    let mut blob = good.clone();
    blob[HEADER_LEN + 4] ^= 0xFF;
    std::fs::write(&out_path, &blob).unwrap();
    let i = bin().arg("inspect").arg(&out_path).arg("--pubkey").arg(test_key("pub")).output().unwrap();
    assert!(!i.status.success(), "inspect must fail a corrupted image");

    // A flipped signature byte: CRCs still fine, signature is not.
    let mut blob = good.clone();
    let last = blob.len() - 1;
    blob[last] ^= 0xFF;
    std::fs::write(&out_path, &blob).unwrap();
    let i = bin().arg("inspect").arg(&out_path).arg("--pubkey").arg(test_key("pub")).output().unwrap();
    assert!(!i.status.success(), "inspect must fail a broken signature");
    assert!(String::from_utf8_lossy(&i.stdout).contains("INVALID"));

    // The trailer cut off entirely — the header still promises one.
    let mut blob = good;
    blob.truncate(HEADER_LEN + image.len());
    std::fs::write(&out_path, &blob).unwrap();
    let i = bin().arg("inspect").arg(&out_path).arg("--pubkey").arg(test_key("pub")).output().unwrap();
    assert!(!i.status.success(), "inspect must fail a missing trailer");
    assert!(String::from_utf8_lossy(&i.stdout).contains("MISSING"));

    let _ = std::fs::remove_file(&bin_path);
    let _ = std::fs::remove_file(&out_path);
}

/// `inspect` is the release gate, so it must implement the device's exact scheme policy rather
/// than treating every non-zero marker as if it meant Ed25519.
#[test]
fn inspect_rejects_unknown_or_inconsistent_signature_markers() {
    let bin_path = scratch("scheme.bin");
    let out_path = scratch("SCHEME.BIN");
    std::fs::write(&bin_path, fake_image(b"a correctly signed image")).unwrap();
    let w = bin()
        .args(["wrap", "--bin"])
        .arg(&bin_path)
        .args(["--version", "v5.0.0", "--out"])
        .arg(&out_path)
        .arg("--sign-seed")
        .arg(test_key("seed"))
        .output()
        .unwrap();
    assert!(w.status.success());
    let good = std::fs::read(&out_path).unwrap();

    for (scheme, sig_len) in [(99u16, SIG_LEN as u16), (0, SIG_LEN as u16), (SIG_SCHEME_ED25519, 0)] {
        let mut blob = good.clone();
        blob[48..50].copy_from_slice(&scheme.to_le_bytes());
        blob[50..52].copy_from_slice(&sig_len.to_le_bytes());
        let header_crc = crc32(&blob[..60]);
        blob[60..64].copy_from_slice(&header_crc.to_le_bytes());
        std::fs::write(&out_path, &blob).unwrap();

        let i = bin()
            .arg("inspect")
            .arg(&out_path)
            .arg("--pubkey")
            .arg(test_key("pub"))
            .arg("--allow-unsigned")
            .output()
            .unwrap();
        assert!(!i.status.success(), "scheme {scheme}, length {sig_len} must not pass the release gate");
        assert!(String::from_utf8_lossy(&i.stdout).contains("UNSUPPORTED"));
    }

    let _ = std::fs::remove_file(&bin_path);
    let _ = std::fs::remove_file(&out_path);
}

/// An unsigned (v1) container fails `inspect` by default — that default is what keeps an unsigned
/// artifact from reaching a release — but is inspectable with the explicit escape hatch.
#[test]
fn inspect_rejects_unsigned_unless_allowed() {
    let bin_path = scratch("v1.bin");
    let out_path = scratch("V1.BIN");
    std::fs::write(&bin_path, fake_image(b"an old-style container")).unwrap();
    let w = bin()
        .args(["wrap", "--bin"])
        .arg(&bin_path)
        .args(["--version", "v0.9.0", "--out"])
        .arg(&out_path)
        .output()
        .unwrap();
    assert!(w.status.success());

    let strict = bin().arg("inspect").arg(&out_path).output().unwrap();
    assert!(!strict.status.success(), "an unsigned container must not pass the gate");
    assert!(String::from_utf8_lossy(&strict.stdout).contains("NONE"));

    let lenient = bin().arg("inspect").arg(&out_path).arg("--allow-unsigned").output().unwrap();
    assert!(lenient.status.success(), "…but --allow-unsigned still reports on it");
    assert!(String::from_utf8_lossy(&lenient.stdout).contains("=> valid"));

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

    // The wrapped file still inspects clean as a container (it is simply unsigned).
    let i = bin().arg("inspect").arg(&out_path).arg("--allow-unsigned").output().unwrap();
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
