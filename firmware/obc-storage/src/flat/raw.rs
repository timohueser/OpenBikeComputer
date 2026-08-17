//! Little-endian primitives over a slice whose length the caller has already checked.
//!
//! `FLAT_Store_Format.md` §0: "Integers are unsigned little-endian ... Byte offsets are zero-based."
//! Every record in this format is fixed-size, so each decoder checks its one length precondition
//! once and then indexes directly; that is why none of these is fallible.

pub fn u16_at(bytes: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([bytes[off], bytes[off + 1]])
}

pub fn u32_at(bytes: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]])
}

pub fn u64_at(bytes: &[u8], off: usize) -> u64 {
    let mut raw = [0u8; 8];
    raw.copy_from_slice(&bytes[off..off + 8]);
    u64::from_le_bytes(raw)
}

pub fn bytes16_at(bytes: &[u8], off: usize) -> [u8; 16] {
    let mut raw = [0u8; 16];
    raw.copy_from_slice(&bytes[off..off + 16]);
    raw
}

pub fn put_u16(out: &mut [u8], off: usize, value: u16) {
    out[off..off + 2].copy_from_slice(&value.to_le_bytes());
}

pub fn put_u32(out: &mut [u8], off: usize, value: u32) {
    out[off..off + 4].copy_from_slice(&value.to_le_bytes());
}

pub fn put_u64(out: &mut [u8], off: usize, value: u64) {
    out[off..off + 8].copy_from_slice(&value.to_le_bytes());
}

pub fn put_bytes(out: &mut [u8], off: usize, value: &[u8]) {
    out[off..off + value.len()].copy_from_slice(value);
}

/// True when every byte of `bytes[off..off + len]` is zero — §0's "reserved fields inside a record
/// are written as zero and MUST be zero when read".
pub fn is_zero(bytes: &[u8], off: usize, len: usize) -> bool {
    bytes[off..off + len].iter().all(|&byte| byte == 0)
}

/// The one CRC of this format: CRC-32/IEEE, `crc32("123456789") = 0xCBF43926`.
pub fn crc32(bytes: &[u8]) -> u32 {
    obc_crc::Crc32::checksum(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn round_trips_every_width() {
        let mut buf = [0u8; 32];
        put_u16(&mut buf, 0, 0x1234);
        put_u32(&mut buf, 2, 0x89AB_CDEF);
        put_u64(&mut buf, 6, 0x0123_4567_89AB_CDEF);
        put_bytes(&mut buf, 14, &[1, 2, 3]);
        assert_eq!(u16_at(&buf, 0), 0x1234);
        assert_eq!(u32_at(&buf, 2), 0x89AB_CDEF);
        assert_eq!(u64_at(&buf, 6), 0x0123_4567_89AB_CDEF);
        assert!(!is_zero(&buf, 14, 3));
        assert!(is_zero(&buf, 17, 15));
        assert_eq!(bytes16_at(&buf, 14), [1, 2, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    }
}
