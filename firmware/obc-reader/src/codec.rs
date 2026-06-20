//! Little-endian field codecs for the binary map/route formats.
//!
//! Both on-disk formats — `OBCM` (map) here and `OBCR` (route) in `obc-route` — are
//! fixed-offset little-endian records. These read/write one field at a time over an
//! already-bounds-checked slice: the readers validate a record's whole length up front,
//! so these helpers stay branch-free and simply index, panicking only on a truly
//! out-of-range offset (a format/bounds bug, not untrusted input).
//!
//! One copy here so the map reader ([`crate::reader`]), the route reader, and the route
//! writer can't disagree on byte order or field width — the kind of skew that corrupts a
//! file silently. `obc-route` reaches these through its dependency on `obc-reader`.

/// Read a little-endian `i16` at byte offset `o`.
#[inline]
pub fn rd_i16(d: &[u8], o: usize) -> i16 {
    i16::from_le_bytes([d[o], d[o + 1]])
}
/// Read a little-endian `u16` at byte offset `o`.
#[inline]
pub fn rd_u16(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}
/// Read a little-endian `i32` at byte offset `o`.
#[inline]
pub fn rd_i32(d: &[u8], o: usize) -> i32 {
    i32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}
/// Read a little-endian `u32` at byte offset `o`.
#[inline]
pub fn rd_u32(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}
/// Read a little-endian `f32` at byte offset `o`.
#[inline]
pub fn rd_f32(d: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

/// Write `v` as little-endian `i16` at byte offset `o`.
#[inline]
pub fn put_i16(b: &mut [u8], o: usize, v: i16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
/// Write `v` as little-endian `u16` at byte offset `o`.
#[inline]
pub fn put_u16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
/// Write `v` as little-endian `i32` at byte offset `o`.
#[inline]
pub fn put_i32(b: &mut [u8], o: usize, v: i32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
/// Write `v` as little-endian `u32` at byte offset `o`.
#[inline]
pub fn put_u32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
