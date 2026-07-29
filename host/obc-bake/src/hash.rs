//! Content hashes: the idempotency key's ingredients and the artifact digest.

use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

const HASH_BUF: usize = 64 * 1024;

/// Stream `path`, returning `(bytes, lowercase hex sha256)`.
///
/// Streamed because the inputs here are the largest files in the project — the
/// German extract is 4.8 GB — and the bakery must never need one resident just to
/// decide whether it changed.
pub fn file(path: &Path) -> Result<(u64, String), String> {
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; HASH_BUF];
    let mut total = 0u64;
    loop {
        let n = f.read(&mut buf).map_err(|e| format!("{}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total += n as u64;
    }
    Ok((total, hex(&hasher.finalize())))
}

/// Hash of a string — used for preset configs and for the composed bake key.
pub fn text(s: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    hex(&hasher.finalize())
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}
