//! CRC-32/IEEE — the integrity check for both DFU byte formats.
//!
//! Standard CRC-32/IEEE (zlib/gzip/PNG): reflected polynomial `0xEDB88320`, init/xor-out
//! `0xFFFFFFFF`, check value `crc32("123456789") == 0xCBF43926`. This is the **canonical DFU copy**:
//! the bootloader (`obc-boot`) links `obc-dfu` but must never pull in the BLE stack, so the CRC the
//! image header, the boot-state page, and the staged-image verify all use lives here. It is
//! byte-identical to [`obc_ble::Crc32`](../../obc-ble/src/crc32.rs) — both pin the same standard
//! check vector — but the two are kept separate on purpose: `obc-ble` is the phone-facing protocol
//! crate (deliberately dependency-free, matched to the app's Swift `CRC32.Hasher`), and coupling it
//! to a DFU-side crate would be the wrong dependency direction. A future `obc-util` extraction could
//! merge them; until then each carries its own test-pinned copy.

/// The reflected CRC-32/IEEE lookup table (poly `0xEDB88320`), built at compile time.
const TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
};

/// An incremental CRC-32/IEEE hasher. `Copy`, so a partial CRC is a trivial resume anchor — the
/// bootloader folds a staged image extent-by-extent (§S3) without buffering the whole ~900 KB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Crc32 {
    state: u32,
}

impl Crc32 {
    /// A fresh hasher (init `0xFFFFFFFF`).
    pub const fn new() -> Self {
        Self { state: 0xFFFF_FFFF }
    }

    /// Fold `bytes` into the running CRC.
    pub fn update(&mut self, bytes: &[u8]) {
        let mut c = self.state;
        for &b in bytes {
            c = TABLE[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
        }
        self.state = c;
    }

    /// The CRC-32 of everything fed so far. Doesn't consume the hasher — read a partial value
    /// mid-stream and keep hashing.
    pub fn finalize(&self) -> u32 {
        self.state ^ 0xFFFF_FFFF
    }

    /// One-shot convenience: the CRC-32 of a whole buffer.
    pub fn checksum(bytes: &[u8]) -> u32 {
        let mut h = Self::new();
        h.update(bytes);
        h.finalize()
    }
}

impl Default for Crc32 {
    fn default() -> Self {
        Self::new()
    }
}

/// One-shot CRC-32/IEEE over `data` — the free-function spelling the codecs use.
pub fn crc32(data: &[u8]) -> u32 {
    Crc32::checksum(data)
}

#[cfg(test)]
mod tests {
    use super::{crc32, Crc32};

    #[test]
    fn check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn empty_is_zero() {
        assert_eq!(crc32(b""), 0);
    }

    /// Incremental hashing over any chunk split equals the one-shot checksum — the bootloader's
    /// extent-by-extent verify must land on the same value as the wrapper's one-shot.
    #[test]
    fn incremental_matches_oneshot() {
        let data: [u8; 259] = core::array::from_fn(|i| (i * 7 + 3) as u8);
        let whole = crc32(&data);
        for split in 0..=data.len() {
            let mut h = Crc32::new();
            h.update(&data[..split]);
            h.update(&data[split..]);
            assert_eq!(h.finalize(), whole, "split at {split}");
        }
    }
}
