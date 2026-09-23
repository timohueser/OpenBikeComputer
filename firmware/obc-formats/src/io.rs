//! Neutral byte I/O traits and little-endian primitive codecs.

/// Errors crossing the random-access byte seam. Medium-specific errors collapse to [`Error::Io`];
/// format-specific parsers add their own context.
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

pub trait ByteSource {
    /// Fill `buf` from `offset`.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), Error>;
    /// Total length in bytes.
    fn len(&self) -> u64;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A sequential byte sink with a single random patch for streamed writers.
///
/// The offset here is `u32` on purpose: the only users are the OBCR route, track and trip writers,
/// whose format addresses its own structures with `uint32` fields, so a patch offset past 4 GiB is
/// one no route file can name. Map bytes are never written through this seam.
pub trait ByteSink {
    /// Append `buf` at the current write position.
    fn write(&mut self, buf: &[u8]) -> Result<(), Error>;
    /// Overwrite an already-written range at absolute `offset`.
    fn patch_at(&mut self, offset: u32, buf: &[u8]) -> Result<(), Error>;
    /// Append one OBCR chunk body: point records that are deltas from `anchor` (lon, lat, ele).
    /// The chunk index at the end of the stream holds the anchor, so a sink that reads the
    /// geometry while it streams gets it here.
    fn write_chunk(&mut self, anchor: (i32, i32, i16), body: &[u8]) -> Result<(), Error> {
        let _ = anchor;
        self.write(body)
    }
}

/// A [`ByteSource`] over an in-memory slice.
pub struct SliceSource<'a>(pub &'a [u8]);

impl ByteSource for SliceSource<'_> {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        // The narrowing is the host's, not the seam's: a slice lives in the address space, so an
        // offset past `usize` names a byte no in-memory source can hold. A wrapping cast would
        // read some other byte and call it a success.
        let start = usize::try_from(offset).map_err(|_| Error::BadOffset)?;
        let end = start.checked_add(buf.len()).ok_or(Error::BadOffset)?;
        let bytes = self.0.get(start..end).ok_or(Error::BadOffset)?;
        buf.copy_from_slice(bytes);
        Ok(())
    }

    fn len(&self) -> u64 {
        // Exact: `u64` covers every `usize` an in-memory source can be built from.
        self.0.len() as u64
    }
}

/// A [`ByteSource`] over a window of another one: the bytes `offset..offset + len`, re-based so
/// the window's first byte is byte `0`.
///
/// An OBCM file can hold a whole OBCT terrain container, and every offset inside that container is
/// relative to its own first byte, so a consumer needs a source whose zero is the container's
/// zero. Copying the region would need somewhere to copy it to, and on a 512 KB part there is no
/// such place.
///
/// The window is the truth, not a hint: a read that starts inside and runs past the end is
/// [`Error::BadOffset`], because serving the map's next section would hand a terrain parse a
/// plausible answer built out of somebody else's bytes.
///
/// [`new`](WindowSource::new) is the only constructor and refuses a window that does not fit
/// inside `inner`, so a bad region pointer is caught once, where the region is resolved.
pub struct WindowSource<'a> {
    inner: &'a dyn ByteSource,
    offset: u64,
    len: u64,
}

impl<'a> WindowSource<'a> {
    /// The window `offset..offset + len` of `inner`, or `None` when it is not wholly inside it.
    pub fn new(inner: &'a dyn ByteSource, offset: u64, len: u64) -> Option<WindowSource<'a>> {
        let end = offset.checked_add(len)?;
        (end <= inner.len()).then_some(WindowSource { inner, offset, len })
    }

    /// Where the window starts in the source it is a window onto.
    pub fn offset(&self) -> u64 {
        self.offset
    }
}

impl ByteSource for WindowSource<'_> {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        let end = offset.checked_add(buf.len() as u64).ok_or(Error::BadOffset)?;
        if end > self.len {
            return Err(Error::BadOffset);
        }
        // Cannot overflow: `new` proved `self.offset + self.len <= inner.len()`.
        self.inner.read_at(self.offset + offset, buf)
    }

    fn len(&self) -> u64 {
        self.len
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

/// Read a little-endian `i16` after caller-side validation. Panics for an invalid range; new
/// decoders should use the checked family ([`checked_rd_u16`] and friends).
#[inline]
pub fn rd_i16(d: &[u8], o: usize) -> i16 {
    i16::from_le_bytes([d[o], d[o + 1]])
}

/// Read a little-endian `u16` after caller-side validation. Panics for an invalid range.
#[inline]
pub fn rd_u16(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}

/// Read a little-endian `i32` after caller-side validation. Panics for an invalid range.
#[inline]
pub fn rd_i32(d: &[u8], o: usize) -> i32 {
    i32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

/// Read a little-endian `u32` after caller-side validation. Panics for an invalid range.
#[inline]
pub fn rd_u32(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

/// Read a little-endian `f32` after caller-side validation. Panics for an invalid range.
#[inline]
pub fn rd_f32(d: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

/// Write a little-endian `i16` after caller-side validation. Panics for an invalid range.
#[inline]
pub fn put_i16(b: &mut [u8], o: usize, v: i16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}

/// Write a little-endian `u16` after caller-side validation. Panics for an invalid range.
#[inline]
pub fn put_u16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}

/// Write a little-endian `i32` after caller-side validation. Panics for an invalid range.
#[inline]
pub fn put_i32(b: &mut [u8], o: usize, v: i32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

/// Write a little-endian `u32` after caller-side validation. Panics for an invalid range.
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

    /// The window re-bases and only re-bases: the bytes it serves are the inner source's, shifted,
    /// and the length it reports is the window's.
    #[test]
    fn a_window_serves_its_region_from_byte_zero() {
        let whole: [u8; 16] = core::array::from_fn(|i| i as u8);
        let inner = SliceSource(&whole);
        let window = WindowSource::new(&inner, 4, 8).expect("the window fits");

        assert_eq!(window.len(), 8);
        let mut out = [0u8; 8];
        window.read_at(0, &mut out).expect("the whole window");
        assert_eq!(out, [4, 5, 6, 7, 8, 9, 10, 11], "byte 0 of the window is byte 4 of the file");
        let mut one = [0u8; 1];
        window.read_at(7, &mut one).expect("the last byte");
        assert_eq!(one, [11]);
    }

    /// The end of a window is as hard as the end of a file: a straddling read must not be served
    /// out of the bytes that follow the region.
    #[test]
    fn a_window_refuses_reads_past_its_end_rather_than_running_on() {
        let whole: [u8; 16] = core::array::from_fn(|i| i as u8);
        let inner = SliceSource(&whole);
        let window = WindowSource::new(&inner, 4, 8).expect("the window fits");

        let mut out = [0u8; 4];
        assert_eq!(window.read_at(6, &mut out), Err(Error::BadOffset), "straddling the end");
        assert_eq!(window.read_at(8, &mut out), Err(Error::BadOffset), "starting at the end");
        assert_eq!(window.read_at(u64::MAX, &mut out), Err(Error::BadOffset), "an offset that wraps");
    }

    /// A region that does not fit is refused once, where it is resolved, rather than failing every
    /// read later.
    #[test]
    fn a_window_outside_its_source_is_refused_at_construction() {
        let whole = [0u8; 16];
        let inner = SliceSource(&whole);
        assert!(WindowSource::new(&inner, 8, 9).is_none(), "one byte past the end");
        assert!(WindowSource::new(&inner, 17, 0).is_none(), "an empty window past the end");
        assert!(WindowSource::new(&inner, u64::MAX, 1).is_none(), "an offset that wraps");
        assert!(WindowSource::new(&inner, 16, 0).is_some(), "an empty window at the end is inside it");
        assert!(WindowSource::new(&inner, 0, 16).is_some(), "the whole file is a legal window");
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
        assert_eq!(source.read_at(u32::MAX as u64, &mut out), Err(Error::BadOffset));
        assert_eq!(source.read_at(u64::MAX, &mut out), Err(Error::BadOffset), "past the address space, not past a u32");
        assert_eq!(source.len(), 6, "a slice reports its exact length, with nothing to saturate against");
    }
}
