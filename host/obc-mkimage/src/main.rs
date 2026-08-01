//! `obc-mkimage` — the producer side of the SD-staged DFU pipeline (epic #615, signed in #997).
//!
//! Four subcommands over the shared [`obc_dfu`] OBCU codec:
//!
//! ```text
//! obc-mkimage keygen  --out-dir keys/ --name obcu-release
//! obc-mkimage wrap    --bin app.bin --version "$(git describe --always --dirty)" \
//!                     --out UPDATE.BIN --sign-seed-env OBCU_SIGNING_SEED
//! obc-mkimage sign    --in UPDATE.BIN --out UPDATE.BIN --seed-env OBCU_SIGNING_SEED
//! obc-mkimage inspect UPDATE.BIN
//! ```
//!
//! `wrap` prepends the 64-byte OBCU header to a raw app image (and, given a seed, signs it in the
//! same step); `sign` attaches or replaces the Ed25519 trailer on an already-wrapped container;
//! `inspect` decodes, verifies both CRCs **and** the signature, and exits non-zero on any failure —
//! it is the release workflow's gate. See `firmware/README.md` (§Firmware update images) for how the
//! raw `.bin` is produced, and `specs/OBCU_Spec.md` §1 for the bytes.

use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use obc_dfu::sig::{public_key_of, sign_image, verify_image, PublicKey, PUBKEY_LEN, SEED_LEN, SIG_LEN};
use obc_dfu::{looks_like_vector_table, ImageHeader, HEADER_LEN, MAX_IMAGE_LEN, RAM_END, RAM_START, RELEASE_PUBKEY};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("keygen") => keygen(&args[1..]),
        Some("wrap") => wrap(&args[1..]),
        Some("sign") => sign(&args[1..]),
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
  obc-mkimage keygen  --out-dir <dir> [--name <base>]
  obc-mkimage wrap    --bin <app.bin> --version <str> --out <UPDATE.BIN>
                      [--sign-seed <file> | --sign-seed-env <VAR>]
  obc-mkimage sign    --in <UPDATE.BIN> --out <file> (--seed <file> | --seed-env <VAR>)
  obc-mkimage inspect <UPDATE.BIN> [--pubkey <file>] [--allow-unsigned]

Keys are 64 lowercase hex characters (32 raw bytes) on one line. `keygen` writes
<base>.seed (SECRET) and <base>.pub. `inspect` verifies against the release key
compiled into this build unless --pubkey overrides it.";

fn print_usage() {
    println!("{USAGE}");
}

// ==================== keygen ====================

/// `keygen`: a fresh Ed25519 keypair as two hex files — `<base>.seed` (secret, mode 0600) and
/// `<base>.pub`. Entropy comes from the OS; nothing about the key is derived from the machine.
fn keygen(args: &[String]) -> Result<(), String> {
    let mut out_dir: Option<PathBuf> = None;
    let mut name = String::from("obcu-signing");
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--out-dir" => out_dir = Some(next_value(&mut it, "--out-dir")?.into()),
            "--name" => name = next_value(&mut it, "--name")?,
            other => return Err(format!("unexpected argument `{other}`\n\n{USAGE}")),
        }
    }
    let out_dir = out_dir.ok_or("keygen: missing --out-dir")?;
    if !matches!(Path::new(&name).components().collect::<Vec<_>>().as_slice(), [Component::Normal(_)]) {
        return Err("keygen: --name must be one filename, not a path".to_string());
    }

    let mut seed = [0u8; SEED_LEN];
    getrandom::fill(&mut seed).map_err(|e| format!("keygen: no OS entropy available ({e})"))?;
    let public = public_key_of(&seed);

    let seed_path = out_dir.join(format!("{name}.seed"));
    let pub_path = out_dir.join(format!("{name}.pub"));
    if seed_path.exists() || pub_path.exists() {
        return Err(format!(
            "keygen: {} or {} already exists — refusing to overwrite a key",
            seed_path.display(),
            pub_path.display()
        ));
    }
    write_hex_new(&seed_path, &seed, true)?;
    write_hex_new(&pub_path, public.as_bytes(), false)?;

    println!("wrote {}  <-- SECRET, never commit this", seed_path.display());
    println!("wrote {}", pub_path.display());
    println!("  public key: {}", hex(public.as_bytes()));
    Ok(())
}

// ==================== wrap ====================

/// `wrap`: prepend the OBCU header to a raw image and write `<out>`; with a seed, sign it in the
/// same pass (the release pipeline's one-shot path).
fn wrap(args: &[String]) -> Result<(), String> {
    let mut bin: Option<PathBuf> = None;
    let mut version: Option<String> = None;
    let mut out: Option<PathBuf> = None;
    let mut seed_src: Option<SeedSource> = None;
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--bin" => bin = Some(next_value(&mut it, "--bin")?.into()),
            "--version" => version = Some(next_value(&mut it, "--version")?),
            "--out" => out = Some(next_value(&mut it, "--out")?.into()),
            "--sign-seed" => seed_src = Some(SeedSource::File(next_value(&mut it, "--sign-seed")?.into())),
            "--sign-seed-env" => seed_src = Some(SeedSource::Env(next_value(&mut it, "--sign-seed-env")?)),
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
    let blob = match &seed_src {
        Some(src) => build_signed(header, &image, &src.read()?),
        // Unsigned: byte-for-byte the v1 container. The device's armer rejects it (§1.4) — this is
        // the local-experiment / `--allow-unsigned` shape, not something to publish.
        None => {
            eprintln!(
                "warning: {} is UNSIGNED — a device running v2 firmware refuses to install it. \
                 Pass --sign-seed/--sign-seed-env to sign.",
                out.display()
            );
            let mut b = Vec::with_capacity(HEADER_LEN + image.len());
            b.extend_from_slice(&header.encode());
            b.extend_from_slice(&image);
            b
        }
    };
    std::fs::write(&out, &blob).map_err(|e| format!("writing {}: {e}", out.display()))?;

    println!(
        "wrote {} — version \"{}\", image {} bytes, image_crc32 {:#010X}, {}",
        out.display(),
        header.fw_version_str(),
        header.image_len,
        header.image_crc32,
        if seed_src.is_some() { "Ed25519-signed (OBCU v2)" } else { "unsigned (OBCU v1)" }
    );
    Ok(())
}

// ==================== sign ====================

/// `sign`: attach (or replace) the Ed25519 trailer on an already-wrapped container. Same result as
/// `wrap --sign-seed`; separate so a build artifact can be signed later, on a machine that holds the
/// key, without re-running the build.
fn sign(args: &[String]) -> Result<(), String> {
    let mut input: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut seed_src: Option<SeedSource> = None;
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--in" => input = Some(next_value(&mut it, "--in")?.into()),
            "--out" => out = Some(next_value(&mut it, "--out")?.into()),
            "--seed" => seed_src = Some(SeedSource::File(next_value(&mut it, "--seed")?.into())),
            "--seed-env" => seed_src = Some(SeedSource::Env(next_value(&mut it, "--seed-env")?)),
            other => return Err(format!("unexpected argument `{other}`\n\n{USAGE}")),
        }
    }
    let input = input.ok_or("sign: missing --in")?;
    let out = out.ok_or("sign: missing --out")?;
    let seed = seed_src.ok_or("sign: need --seed <file> or --seed-env <VAR>")?.read()?;

    let container = split_container(&input)?;
    let blob = build_signed(container.header, container.image(), &seed);
    std::fs::write(&out, &blob).map_err(|e| format!("writing {}: {e}", out.display()))?;
    println!(
        "wrote {} — version \"{}\", image {} bytes, Ed25519-signed by {}",
        out.display(),
        container.header.fw_version_str(),
        container.header.image_len,
        hex(public_key_of(&seed).as_bytes())
    );
    Ok(())
}

/// Header (marked signed) ‖ raw image ‖ 64-byte signature — the OBCU v2 container (§1).
fn build_signed(header: ImageHeader, image: &[u8], seed: &[u8; SEED_LEN]) -> Vec<u8> {
    let header = header.signed();
    let signature = sign_image(seed, &header, image);
    let mut blob = Vec::with_capacity(HEADER_LEN + image.len() + SIG_LEN);
    blob.extend_from_slice(&header.encode());
    blob.extend_from_slice(image);
    blob.extend_from_slice(&signature);
    blob
}

// ==================== inspect ====================

/// `inspect`: decode + verify both CRCs + the signature, human-readable, non-zero exit on anything
/// invalid. This is CI's gate on a release artifact, so every check is fatal by default; the one
/// escape hatch is `--allow-unsigned`, for looking at a v1 container or a device-written
/// `ROLLBACK.BIN`.
fn inspect(args: &[String]) -> Result<(), String> {
    let mut path: Option<PathBuf> = None;
    let mut pubkey_path: Option<PathBuf> = None;
    let mut allow_unsigned = false;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--pubkey" => pubkey_path = Some(next_value(&mut it, "--pubkey")?.into()),
            "--allow-unsigned" => allow_unsigned = true,
            other if other.starts_with("--") => return Err(format!("unexpected argument `{other}`\n\n{USAGE}")),
            other if path.is_none() => path = Some(other.into()),
            _ => return Err(format!("inspect: expected exactly one file argument\n\n{USAGE}")),
        }
    }
    let path = path.ok_or_else(|| format!("inspect: expected exactly one file argument\n\n{USAGE}"))?;

    let container = split_container(&path)?;
    let header = container.header;
    let header_bytes = container.header_bytes();
    let image = container.image();
    let trailer = container.trailer();

    let actual_image_crc = obc_dfu::crc32(image);
    let len_ok = header.image_len as usize == image.len();
    let crc_ok = header.image_crc32 == actual_image_crc;

    let (key, key_src) = match &pubkey_path {
        Some(p) => (read_pubkey(p)?, format!("{}", p.display())),
        None => (RELEASE_PUBKEY, String::from("compiled-in release key")),
    };

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

    let sig_ok = match (header.sig_scheme, header.sig_len as usize, trailer) {
        (obc_dfu::SIG_SCHEME_NONE, 0, _) => {
            println!("  signature    : NONE (OBCU v1 container — a v2 device refuses to install it)");
            allow_unsigned
        }
        (obc_dfu::SIG_SCHEME_ED25519, SIG_LEN, None) => {
            println!("  signature    : MISSING — the header claims a {}-byte trailer that isn't there", header.sig_len);
            false
        }
        (obc_dfu::SIG_SCHEME_ED25519, SIG_LEN, Some(sig)) => {
            let result = verify_image(&key, &header, image, sig);
            println!("  signature    : Ed25519, {} bytes, scheme {}", sig.len(), header.sig_scheme);
            println!("  verified vs  : {} ({})", hex(key.as_bytes()), key_src);
            match result {
                Ok(()) => {
                    println!("  signature    : OK");
                    true
                }
                Err(e) => {
                    println!("  signature    : INVALID ({e:?})");
                    false
                }
            }
        }
        (scheme, len, _) => {
            println!("  signature    : UNSUPPORTED scheme {scheme}, length {len}");
            false
        }
    };

    if len_ok && crc_ok && sig_ok {
        println!("  => valid");
        Ok(())
    } else {
        Err(format!("{}: image failed verification", path.display()))
    }
}

// ==================== shared helpers ====================

/// Where a secret seed comes from: a file (dev machines) or an environment variable (CI, so the
/// secret never touches the filesystem).
enum SeedSource {
    File(PathBuf),
    Env(String),
}

impl SeedSource {
    fn read(&self) -> Result<[u8; SEED_LEN], String> {
        let (raw, what) = match self {
            SeedSource::File(p) => (
                std::fs::read(p).map_err(|e| format!("reading seed {}: {e}", p.display()))?,
                format!("{}", p.display()),
            ),
            SeedSource::Env(var) => (
                std::env::var(var).map_err(|_| format!("${var} is not set (it must hold 64 hex characters)"))?.into(),
                format!("${var}"),
            ),
        };
        let mut out = [0u8; SEED_LEN];
        unhex(&raw, &mut out).map_err(|e| format!("{what}: {e}"))?;
        Ok(out)
    }
}

/// A container read off disk, with cheap borrowed views over its three parts (§1).
struct Container {
    header: ImageHeader,
    bytes: Vec<u8>,
    image_end: usize,
    trailer_end: Option<usize>,
}

impl Container {
    fn header_bytes(&self) -> &[u8; HEADER_LEN] {
        self.bytes[..HEADER_LEN].try_into().expect("split_container checked the header length")
    }

    fn image(&self) -> &[u8] {
        &self.bytes[HEADER_LEN..self.image_end]
    }

    fn trailer(&self) -> Option<&[u8]> {
        self.trailer_end.map(|end| &self.bytes[self.image_end..end])
    }
}

/// Read a container and split it into header / raw image / optional signature trailer. Rejects
/// anything that isn't a decodable OBCU container, or that is shorter than its own header claims.
fn split_container(path: &Path) -> Result<Container, String> {
    let blob = std::fs::read(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    if blob.len() < HEADER_LEN {
        return Err(format!("{} is only {} bytes — shorter than a 64-byte OBCU header", path.display(), blob.len()));
    }
    let header_bytes: &[u8; HEADER_LEN] = blob[..HEADER_LEN].try_into().expect("checked length");
    let header = ImageHeader::decode(header_bytes)
        .ok_or_else(|| format!("{}: not a valid OBCU image (bad magic, version, or header CRC)", path.display()))?;

    let image_end = HEADER_LEN
        .checked_add(header.image_len as usize)
        .ok_or_else(|| format!("{}: image length overflows this host", path.display()))?;
    if blob.len() < image_end {
        return Err(format!(
            "{}: truncated — the header claims {} image bytes but the file holds {}",
            path.display(),
            header.image_len,
            blob.len().saturating_sub(HEADER_LEN)
        ));
    }
    let trailer_end = image_end.checked_add(header.sig_len as usize).filter(|&end| end <= blob.len());
    Ok(Container { header, bytes: blob, image_end, trailer_end })
}

/// The header CRC as stored (decode already verified it matches; read it back for the report).
fn header_crc(bytes: &[u8; HEADER_LEN]) -> u32 {
    u32::from_le_bytes([bytes[60], bytes[61], bytes[62], bytes[63]])
}

fn read_pubkey(path: &Path) -> Result<PublicKey, String> {
    let raw = std::fs::read(path).map_err(|e| format!("reading pubkey {}: {e}", path.display()))?;
    let mut out = [0u8; PUBKEY_LEN];
    unhex(&raw, &mut out).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(PublicKey::from_bytes(out))
}

/// Parse `2 * out.len()` hex characters, ignoring surrounding whitespace. Strict about the length so
/// a truncated paste can never silently produce a short key.
fn unhex(raw: &[u8], out: &mut [u8]) -> Result<(), String> {
    let text = std::str::from_utf8(raw).map_err(|_| "not UTF-8 (expected hex text)".to_string())?;
    let text = text.trim();
    if text.len() != out.len() * 2 {
        return Err(format!("expected {} hex characters, found {}", out.len() * 2, text.len()));
    }
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).map_err(|_| "not hex".to_string())?;
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Create a key file atomically, refusing to follow the check-then-overwrite race that a plain
/// `fs::write` would leave. Secret permissions are set at creation, so the seed is never briefly
/// world-readable before a later chmod.
fn write_hex_new(path: &Path, bytes: &[u8], secret: bool) -> Result<(), String> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(if secret { 0o600 } else { 0o644 });
    }
    let _ = secret;
    let mut file = options.open(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            format!("{} already exists — refusing to overwrite a key", path.display())
        } else {
            format!("creating {}: {e}", path.display())
        }
    })?;
    writeln!(file, "{}", hex(bytes)).map_err(|e| format!("writing {}: {e}", path.display()))
}

fn next_value<'a>(it: &mut impl Iterator<Item = &'a String>, flag: &str) -> Result<String, String> {
    it.next().cloned().ok_or_else(|| format!("{flag} needs a value"))
}
