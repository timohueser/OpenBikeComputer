//! Little-endian primitive reads and writes.
//!
//! `Device_Object_Protocol_v3.md` §1: every multi-byte field is byte-packed at exactly its stated
//! offset and no wire structure contains alignment padding. These helpers therefore take an
//! absolute offset into a slice whose length the caller has already checked against the message's
//! fixed size — which is why they may index directly and why none of them is fallible.

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

pub(crate) fn i32_at(bytes: &[u8], off: usize) -> i32 {
    u32_at(bytes, off) as i32
}

pub(crate) fn i64_at(bytes: &[u8], off: usize) -> i64 {
    u64_at(bytes, off) as i64
}

pub(crate) fn bytes16_at(bytes: &[u8], off: usize) -> [u8; 16] {
    let mut raw = [0u8; 16];
    raw.copy_from_slice(&bytes[off..off + 16]);
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

pub(crate) fn put_i32(out: &mut [u8], off: usize, value: i32) {
    put_u32(out, off, value as u32);
}

pub(crate) fn put_i64(out: &mut [u8], off: usize, value: i64) {
    put_u64(out, off, value as u64);
}

pub(crate) fn put_bytes(out: &mut [u8], off: usize, value: &[u8]) {
    out[off..off + value.len()].copy_from_slice(value);
}

/// True when every byte of `bytes[off..off + len]` is zero.
///
/// §1: "Reserved fields and inactive fixed-width alternatives are encoded as zero and rejected when
/// nonzero." Every reserved run in this crate is checked through this one predicate.
pub(crate) fn is_zero(bytes: &[u8], off: usize, len: usize) -> bool {
    bytes[off..off + len].iter().all(|&b| b == 0)
}
