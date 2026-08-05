//! Neutral byte I/O traits and little-endian primitive codecs.

/// Errors crossing the random-access byte seam.
///
/// This type intentionally preserves the variants historically exposed by `obc-reader` and
/// `obc-route`. Medium-specific errors are collapsed to [`Error::Io`]; format-specific parsers
/// remain responsible for adding their own context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// A byte range lies outside the source or an offset computation overflowed.
    BadOffset,
    /// Magic bytes were not the expected format tag.
    BadMagic,
    /// The version byte is unsupported.
    BadVersion,
    /// Declared counts exceed a consumer's fixed-capacity buffers.
    TooLarge,
    /// The underlying medium failed.
    Io,
    /// A streamed input contained no usable records.
    Empty,
}

/// Primitive validation failure, independent of storage and high-level reader errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// The requested fixed field is not fully present or offset arithmetic overflowed.
    Bounds,
    /// The fixed prefix carries an unsupported version.
    Version,
    /// Fixed bytes or layout invariants do not match the format contract.
    Layout,
}

/// A random-access, read-only byte source.
pub trait ByteSource {
    /// Fill `buf` from `offset`.
    fn read_at(&self, offset: u32, buf: &mut [u8]) -> Result<(), Error>;
    /// Total length in bytes.
    fn len(&self) -> u32;
    /// Whether the source is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A sequential byte sink with a single random patch for streamed writers.
pub trait ByteSink {
    /// Append `buf` at the current write position.
    fn write(&mut self, buf: &[u8]) -> Result<(), Error>;
    /// Overwrite an already-written range at absolute `offset`.
    fn patch_at(&mut self, offset: u32, buf: &[u8]) -> Result<(), Error>;
}

/// A [`ByteSource`] over an in-memory slice.
pub struct SliceSource<'a>(pub &'a [u8]);

impl ByteSource for SliceSource<'_> {
    fn read_at(&self, offset: u32, buf: &mut [u8]) -> Result<(), Error> {
        let start = offset as usize;
        let end = start.checked_add(buf.len()).ok_or(Error::BadOffset)?;
        let bytes = self.0.get(start..end).ok_or(Error::BadOffset)?;
        buf.copy_from_slice(bytes);
        Ok(())
    }

    fn len(&self) -> u32 {
        // Saturate rather than wrap: a ≥4 GiB slice reporting a truncated total would let
        // downstream bounds checks pass against the wrong length. `u32::MAX` fails closed.
        self.0.len().min(u32::MAX as usize) as u32
    }
}

/// Validate a five-byte `magic + version` prefix without importing a format-specific error.
pub fn validate_prefix(bytes: &[u8], magic: &[u8; 4], min_version: u8, max_version: u8) -> Result<u8, DecodeError> {
    let prefix = bytes.get(..5).ok_or(DecodeError::Bounds)?;
    if &prefix[..4] != magic {
        return Err(DecodeError::Layout);
    }
    let version = prefix[4];
    if version < min_version || version > max_version {
        return Err(DecodeError::Version);
    }
    Ok(version)
}

fn checked_field<const N: usize>(bytes: &[u8], offset: usize) -> Result<&[u8; N], DecodeError> {
    let end = offset.checked_add(N).ok_or(DecodeError::Bounds)?;
    bytes.get(offset..end).ok_or(DecodeError::Bounds)?.try_into().map_err(|_| DecodeError::Bounds)
}

fn checked_field_mut<const N: usize>(bytes: &mut [u8], offset: usize) -> Result<&mut [u8; N], DecodeError> {
    let end = offset.checked_add(N).ok_or(DecodeError::Bounds)?;
    bytes.get_mut(offset..end).ok_or(DecodeError::Bounds)?.try_into().map_err(|_| DecodeError::Bounds)
}

/// Read a little-endian `u16`, returning [`DecodeError::Bounds`] for a short or overflowing range.
#[inline]
pub fn checked_rd_u16(d: &[u8], o: usize) -> Result<u16, DecodeError> {
    Ok(u16::from_le_bytes(*checked_field(d, o)?))
}

/// Read a little-endian `i32`, returning [`DecodeError::Bounds`] for a short or overflowing range.
#[inline]
pub fn checked_rd_i32(d: &[u8], o: usize) -> Result<i32, DecodeError> {
    Ok(i32::from_le_bytes(*checked_field(d, o)?))
}

/// Read a little-endian `u32`, returning [`DecodeError::Bounds`] for a short or overflowing range.
#[inline]
pub fn checked_rd_u32(d: &[u8], o: usize) -> Result<u32, DecodeError> {
    Ok(u32::from_le_bytes(*checked_field(d, o)?))
}

/// Write a little-endian `i32`, returning [`DecodeError::Bounds`] for a short or overflowing range.
#[inline]
pub fn checked_put_i32(b: &mut [u8], o: usize, v: i32) -> Result<(), DecodeError> {
    checked_field_mut::<4>(b, o)?.copy_from_slice(&v.to_le_bytes());
    Ok(())
}

/// Write a little-endian `u32`, returning [`DecodeError::Bounds`] for a short or overflowing range.
#[inline]
pub fn checked_put_u32(b: &mut [u8], o: usize, v: u32) -> Result<(), DecodeError> {
    checked_field_mut::<4>(b, o)?.copy_from_slice(&v.to_le_bytes());
    Ok(())
}

/// Read a little-endian `i16` after caller-side validation.
///
/// Panics for an invalid range; new primitive decoders should use the checked family
/// ([`checked_rd_u16`] and friends), growing it by the missing width if there isn't one yet.
#[inline]
pub fn rd_i16(d: &[u8], o: usize) -> i16 {
    i16::from_le_bytes([d[o], d[o + 1]])
}

/// Read a little-endian `u16` after caller-side validation.
///
/// Panics for an invalid range; new primitive decoders should use [`checked_rd_u16`].
#[inline]
pub fn rd_u16(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}

/// Read a little-endian `i32` after caller-side validation.
///
/// Panics for an invalid range; new primitive decoders should use [`checked_rd_i32`].
#[inline]
pub fn rd_i32(d: &[u8], o: usize) -> i32 {
    i32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

/// Read a little-endian `u32` after caller-side validation.
///
/// Panics for an invalid range; new primitive decoders should use [`checked_rd_u32`].
#[inline]
pub fn rd_u32(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

/// Read a little-endian `f32` after caller-side validation.
///
/// Panics for an invalid range; new primitive decoders should use the checked family
/// ([`checked_rd_u16`] and friends), growing it by the missing width if there isn't one yet.
#[inline]
pub fn rd_f32(d: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

/// Write a little-endian `i16` after caller-side validation.
///
/// Panics for an invalid range; new primitive encoders should use the checked family
/// ([`checked_put_u32`] and friends), growing it by the missing width if there isn't one yet.
#[inline]
pub fn put_i16(b: &mut [u8], o: usize, v: i16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}

/// Write a little-endian `u16` after caller-side validation.
///
/// Panics for an invalid range; new primitive encoders should use the checked family
/// ([`checked_put_u32`] and friends), growing it by the missing width if there isn't one yet.
#[inline]
pub fn put_u16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}

/// Write a little-endian `i32` after caller-side validation.
///
/// Panics for an invalid range; new primitive encoders should use [`checked_put_i32`].
#[inline]
pub fn put_i32(b: &mut [u8], o: usize, v: i32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

/// Write a little-endian `u32` after caller-side validation.
///
/// Panics for an invalid range; new primitive encoders should use [`checked_put_u32`].
#[inline]
pub fn put_u32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endian_primitives_pin_exact_bytes() {
        let mut bytes = [0u8; 12];
        put_i16(&mut bytes, 0, -0x1234);
        put_u16(&mut bytes, 2, 0xABCD);
        put_i32(&mut bytes, 4, -0x0123_4567);
        put_u32(&mut bytes, 8, 0x89AB_CDEF);
        assert_eq!(bytes, [0xCC, 0xED, 0xCD, 0xAB, 0x99, 0xBA, 0xDC, 0xFE, 0xEF, 0xCD, 0xAB, 0x89]);
        assert_eq!(rd_i16(&bytes, 0), -0x1234);
        assert_eq!(rd_u16(&bytes, 2), 0xABCD);
        assert_eq!(rd_i32(&bytes, 4), -0x0123_4567);
        assert_eq!(rd_u32(&bytes, 8), 0x89AB_CDEF);
    }

    #[test]
    fn prefix_errors_keep_bounds_version_and_layout_distinct() {
        assert_eq!(validate_prefix(b"OBC", b"OBCM", 10, 10), Err(DecodeError::Bounds));
        assert_eq!(validate_prefix(b"NOPE\x0a", b"OBCM", 10, 10), Err(DecodeError::Layout));
        assert_eq!(validate_prefix(b"OBCM\x09", b"OBCM", 10, 10), Err(DecodeError::Version));
        assert_eq!(validate_prefix(b"OBCM\x0a", b"OBCM", 10, 10), Ok(10));
    }

    #[test]
    fn checked_primitives_reject_short_and_overflowing_ranges() {
        let short = [0u8; 3];
        assert_eq!(checked_rd_u16(&short, usize::MAX), Err(DecodeError::Bounds));
        assert_eq!(checked_rd_i32(&short, 0), Err(DecodeError::Bounds));
        assert_eq!(checked_rd_u32(&short, usize::MAX), Err(DecodeError::Bounds));

        let mut short = [0u8; 3];
        assert_eq!(checked_put_i32(&mut short, 0, 1), Err(DecodeError::Bounds));
        assert_eq!(checked_put_u32(&mut short, usize::MAX, 1), Err(DecodeError::Bounds));
    }

    #[test]
    fn slice_source_rejects_overflow_and_short_reads() {
        let source = SliceSource(b"abcdef");
        let mut out = [0u8; 3];
        source.read_at(2, &mut out).unwrap();
        assert_eq!(&out, b"cde");
        assert_eq!(source.read_at(5, &mut out), Err(Error::BadOffset));
        assert_eq!(source.read_at(u32::MAX, &mut out), Err(Error::BadOffset));
        assert_eq!(source.len(), 6, "a sub-4-GiB slice reports its exact length");
    }
}
