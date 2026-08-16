//! Little-endian primitive reads and writes over a slice whose length the caller has checked.
//!
//! `OBC2_Storage_Format.md` §1: "Integers are little-endian. Byte offsets are zero-based." Every
//! record in this kernel is fixed-size, so each decoder checks its one length precondition once and
//! then indexes directly; that is why none of these helpers is fallible.

pub(crate) fn u16_at(bytes: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([bytes[off], bytes[off + 1]])
}

pub(crate) fn u32_at(bytes: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]])
}

pub(crate) fn u64_at(bytes: &[u8], off: usize) -> u64 {
    let mut raw = [0u8; 8];
    raw.copy_from_slice(&bytes[off..off + 8]);
    u64::from_le_bytes(raw)
}

pub(crate) fn i64_at(bytes: &[u8], off: usize) -> i64 {
    u64_at(bytes, off) as i64
}

pub(crate) fn i32_at(bytes: &[u8], off: usize) -> i32 {
    u32_at(bytes, off) as i32
}

pub(crate) fn bytes16_at(bytes: &[u8], off: usize) -> [u8; 16] {
    let mut raw = [0u8; 16];
    raw.copy_from_slice(&bytes[off..off + 16]);
    raw
}

pub(crate) fn bytes32_at(bytes: &[u8], off: usize) -> [u8; 32] {
    let mut raw = [0u8; 32];
    raw.copy_from_slice(&bytes[off..off + 32]);
    raw
}

pub(crate) fn put_u16(out: &mut [u8], off: usize, value: u16) {
    out[off..off + 2].copy_from_slice(&value.to_le_bytes());
}

pub(crate) fn put_u32(out: &mut [u8], off: usize, value: u32) {
    out[off..off + 4].copy_from_slice(&value.to_le_bytes());
}

pub(crate) fn put_u64(out: &mut [u8], off: usize, value: u64) {
    out[off..off + 8].copy_from_slice(&value.to_le_bytes());
}

pub(crate) fn put_i64(out: &mut [u8], off: usize, value: i64) {
    put_u64(out, off, value as u64);
}

pub(crate) fn put_i32(out: &mut [u8], off: usize, value: i32) {
    put_u32(out, off, value as u32);
}

pub(crate) fn put_bytes(out: &mut [u8], off: usize, value: &[u8]) {
    out[off..off + value.len()].copy_from_slice(value);
}

/// True when every byte of `bytes[off..off + len]` is zero.
pub(crate) fn is_zero(bytes: &[u8], off: usize, len: usize) -> bool {
    bytes[off..off + len].iter().all(|&b| b == 0)
}

/// Checks a reserved run — including a slot's pad to its 16,384-byte stride — and refuses it
/// nonzero, which is what §1's "reserved bytes ... must be zero when read" means at a slot.
pub(crate) fn require_zero(
    record: super::error::Record,
    bytes: &[u8],
    off: usize,
    len: usize,
) -> super::error::Result<()> {
    if is_zero(bytes, off, len) {
        Ok(())
    } else {
        Err(super::error::DecodeError::new(record, super::error::Reason::Reserved))
    }
}

/// The §1 CRC-32/IEEE of `bytes`.
///
/// One implementation, shared with the DFU container and the wire contract: reflected polynomial
/// `0xEDB88320`, initial value and xor-out `0xFFFF_FFFF`. §7's finalized prefix CRC is this CRC over
/// the payload's first `durable_offset` bytes.
pub(crate) fn crc32(bytes: &[u8]) -> u32 {
    obc_crc::crc32(bytes)
}

/// The §1 CRC-32/IEEE of a record whose own CRC field is inside the checksummed range: "A CRC field is
/// treated as zero while its containing record is checksummed."
///
/// Every OBC2 CRC field is a four-byte run, and every one of them sits at the end of the range it
/// covers except the gate's, which sits in the middle — so this takes the field's offset rather
/// than assuming either shape.
pub(crate) fn crc32_with_hole(bytes: &[u8], hole: usize) -> u32 {
    let mut hasher = obc_crc::Crc32::new();
    hasher.update(&bytes[..hole]);
    hasher.update(&[0, 0, 0, 0]);
    hasher.update(&bytes[hole + 4..]);
    hasher.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one implementation, shared with the DFU container and the wire contract: reflected
    /// polynomial `0xEDB88320`, initial value and xor-out `0xFFFF_FFFF`.
    #[test]
    fn crc_check_value() {
        assert_eq!(obc_crc::crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn hole_crc_equals_the_same_bytes_with_the_field_zeroed() {
        let mut record = [0u8; 32];
        for (index, byte) in record.iter_mut().enumerate() {
            *byte = index as u8;
        }
        let mut zeroed = record;
        zeroed[8..12].copy_from_slice(&[0, 0, 0, 0]);
        assert_eq!(crc32_with_hole(&record, 8), obc_crc::crc32(&zeroed));
    }
}
