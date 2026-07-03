//! Byte I/O abstractions shared by the map (`obcm`) and route (`obcr`) formats.
//!
//! Neither reader touches a filesystem directly: they read through a [`ByteSource`] and write
//! through a [`ByteSink`] — the seam that keeps the format code identical across platforms. On the
//! host a source is backed by an in-memory slice (or a `std` file), on the device by a FatFs handle
//! on the SD card; only the trait impls are platform-specific. `obc-route` re-exports these.

/// Errors from parsing, reading, or writing through the byte seam. Distinct from the map-parse
/// [`crate::Error`]: shared with the route code (re-exported as `obc_route::Error`), so it carries
/// the union of variants both formats need.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// A read ran past the end of the source, or an offset/length is out of range.
    BadOffset,
    /// Magic bytes were not the expected format tag.
    BadMagic,
    /// Unsupported format version.
    BadVersion,
    /// The file declares more chunks / points-per-chunk than the resident buffers
    /// hold (e.g. [`MAX_ROUTE_CHUNKS`](../../obc_route/constant.MAX_ROUTE_CHUNKS.html) /
    /// `MAX_POINTS_PER_CHUNK`).
    TooLarge,
    /// The underlying medium (SD card / file) failed.
    Io,
    /// The GPX held no usable track points.
    Empty,
}

/// A random-access, read-only byte source (a file, an SD-card handle, an in-memory
/// slice). `read_at` takes `&self` so a reader can hold a shared `&dyn ByteSource` and stay
/// monomorphic; an impl over a seeking medium uses interior mutability.
pub trait ByteSource {
    /// Fill `buf` from `offset`. Errors ([`Error::BadOffset`]) if the range exceeds
    /// the source.
    fn read_at(&self, offset: u32, buf: &mut [u8]) -> Result<(), Error>;
    /// Total length in bytes.
    fn len(&self) -> u32;
    /// Whether the source is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A sequential byte sink with a single random patch, matching the route writer's
/// flow: stream the body, then `patch_at(0, ..)` to backfill the header once the
/// offsets/totals are known (see `OBCR_Spec.md` §4).
pub trait ByteSink {
    /// Append `buf` at the current write position.
    fn write(&mut self, buf: &[u8]) -> Result<(), Error>;
    /// Overwrite `buf.len()` bytes at absolute `offset` (already written region).
    fn patch_at(&mut self, offset: u32, buf: &[u8]) -> Result<(), Error>;
}

/// A [`ByteSource`] over an in-memory slice — the host's "whole file is resident"
/// backing. The device uses a FatFs-backed impl instead.
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
        self.0.len() as u32
    }
}
