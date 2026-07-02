//! CRC-32/IEEE — the whole-object, end-to-end integrity check (spec §6).
//!
//! **Not the on-air check.** The BLE Link Layer already CRCs (24-bit) and retransmits every packet,
//! so the CoC is a reliable, ordered stream. This CRC covers what the link can't: encode bugs,
//! storage write errors, resume-logic mistakes — end to end from the phone's encode to the device's
//! flash and back. One CRC per **object**, never per chunk.
//!
//! Standard CRC-32/IEEE (zlib/gzip/PNG): reflected, polynomial `0xEDB88320` (reflected form),
//! init/xor-out `0xFFFFFFFF`. Pinned by spec §6 with the check value `crc32("123456789") ==
//! 0xCBF43926`. The [`Crc32`] hasher is incremental with O(1) state — exactly how a RAM-limited MCU
//! verifies bytes as it sinks them, and byte-identical to the app's `CRC32.Hasher` in Swift.

/// The reflected CRC-32/IEEE lookup table (`0xEDB88320`), built at compile time so a byte is one
/// table lookup + shift — no per-bit loop on the hot sink path.
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

/// An incremental CRC-32/IEEE hasher (spec §6). Feed chunks as they arrive with [`update`]; read the
/// running value any time with [`finalize`]. `Copy`, so a partial CRC is a trivial resume anchor:
/// snapshot it at a committed offset and continue from the copy after a drop.
///
/// [`update`]: Crc32::update
/// [`finalize`]: Crc32::finalize
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

    /// The CRC-32 of everything fed so far (applies the final xor-out; does not consume the hasher,
    /// so a partial value can be read mid-stream and hashing continued).
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

#[cfg(test)]
mod tests {
    use super::Crc32;

    /// Spec §6's pinned check value.
    #[test]
    fn check_value() {
        assert_eq!(Crc32::checksum(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn empty_is_zero() {
        assert_eq!(Crc32::checksum(b""), 0);
    }

    /// Incremental hashing over any chunk split equals the one-shot checksum (the receiver must
    /// accept any CoC segmentation, spec §5).
    #[test]
    fn incremental_matches_oneshot() {
        let data: [u8; 259] = core::array::from_fn(|i| (i * 7 + 3) as u8);
        let whole = Crc32::checksum(&data);
        for split in 0..=data.len() {
            let mut h = Crc32::new();
            h.update(&data[..split]);
            h.update(&data[split..]);
            assert_eq!(h.finalize(), whole, "split at {split}");
        }
    }

    /// A snapshot copy resumes to the same value — the offset-resume anchor (spec §4.2).
    #[test]
    fn copy_resumes() {
        let data: [u8; 128] = core::array::from_fn(|i| i as u8);
        let mut h = Crc32::new();
        h.update(&data[..50]);
        let snapshot = h; // committed-prefix CRC
        let mut resumed = snapshot;
        resumed.update(&data[50..]);
        assert_eq!(resumed.finalize(), Crc32::checksum(&data));
    }
}
