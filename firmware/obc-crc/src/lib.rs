//! `obc-crc` — CRC-32/IEEE, the one copy.
//!
//! Standard CRC-32/IEEE (zlib/gzip/PNG): reflected polynomial `0xEDB88320`, init/xor-out
//! `0xFFFFFFFF`, check value `crc32("123456789") == 0xCBF43926`. Incremental with O(1) state.
//! `no_std`, `core`-only, and with no dependencies of its own — small enough that both callers can
//! link it without giving anything up:
//!
//! - `obc-dfu` hashes the OBCU image header, the boot-state RRAM page and the staged image with
//!   it. The bootloader folds a ~900 KB staged image extent-by-extent (`OBCU_Spec.md` §S3) rather
//!   than buffering it, which is why [`Crc32`] is `Copy` and [`finalize`](Crc32::finalize) doesn't
//!   consume the hasher — a partial CRC is a resume anchor.
//! - `obc-ble` puts one CRC on each whole transferred **object**, never per chunk. Not an on-air
//!   check — the BLE Link Layer already CRCs every packet — but an end-to-end one, covering what
//!   the link can't: encode bugs and storage write errors, from the phone's encode to the device's
//!   card and back. It must stay byte-identical to the companion app's Swift `CRC32.Hasher`, and
//!   the shared `specs/vectors/` fixtures pin both sides.
//!
//! The two used to carry a byte-identical copy each, kept apart because a dependency from the
//! phone-facing wire crate onto a DFU crate would have been the wrong direction. This crate is the
//! third option both module docs asked for: neither depends on the other, both depend on nothing.

#![no_std]
#![forbid(unsafe_code)]

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

/// An incremental CRC-32/IEEE hasher. `Copy`, so a partial CRC is a trivial resume anchor —
/// snapshot it at a committed offset and continue from the copy after a drop, or fold a staged
/// image one extent at a time without ever holding the whole thing.
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

/// One-shot CRC-32/IEEE over `data` — the free-function spelling the DFU codecs use.
pub fn crc32(data: &[u8]) -> u32 {
    Crc32::checksum(data)
}

#[cfg(test)]
mod tests {
    use super::{crc32, Crc32};

    /// The standard check vector. Both wire contracts — the OBCU header and the app's Swift
    /// `CRC32.Hasher` — are anchored to it, so this one assert is what makes the merge safe.
    #[test]
    fn check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(Crc32::checksum(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn empty_is_zero() {
        assert_eq!(crc32(b""), 0);
    }

    /// Incremental hashing over any chunk split equals the one-shot checksum — the bootloader's
    /// extent-by-extent verify must land on the same value as the wrapper's one-shot, and the
    /// receiver must accept any CoC segmentation.
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

    /// A snapshot copy resumes to the same value — the offset-resume anchor.
    #[test]
    fn copy_resumes() {
        let data: [u8; 128] = core::array::from_fn(|i| i as u8);
        let mut h = Crc32::new();
        h.update(&data[..50]);
        let snapshot = h;
        let mut resumed = snapshot;
        resumed.update(&data[50..]);
        assert_eq!(resumed.finalize(), Crc32::checksum(&data));
    }
}
