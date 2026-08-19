//! FatFs byte adapters over a microSD card — the board-agnostic half of "map/route/track on SD".
//!
//! The shared format code (in `obc-route`/`obc-reader`) never touches a filesystem: it reads through a
//! [`ByteSource`], writes through a [`ByteSink`], and logs the ride through an
//! [`obc_ports::TrackSink`]. On the host those seams are backed by `std::fs`; here by an
//! [`embedded_sdmmc`] FatFs file. Only these thin adapters are platform-specific.
//!
//! All three are generic over [`BlockDevice`] + [`TimeSource`], so they pull in no bus types. They
//! borrow the [`VolumeManager`] shared (`&'a`) and hold a [`RawFile`]: the manager has interior
//! mutability (every method takes `&self`), so a board can hold an [`SdByteSource`] over the route
//! and an [`SdTrackSink`] over the track log at once and feed both to one app tick.

use embedded_sdmmc::{BlockDevice, RawFile, TimeSource, VolumeManager};
use obc_formats::io::{ByteSink, ByteSource, Error};
use obc_formats::track::encode_record;
use obc_ports::{TrackError, TrackPoint, TrackSink};

/// A random-access [`ByteSource`] over an open FatFs file — the device backing for
/// `obc-route`'s `RouteReader` / `RouteSummary::read`. Each
/// [`read_at`](ByteSource::read_at) seeks the file then reads, so a route never has to be resident.
/// The length is captured once at construction (the file doesn't grow under a reader).
pub struct SdByteSource<
    'a,
    D: BlockDevice,
    T: TimeSource,
    const MAX_DIRS: usize = 4,
    const MAX_FILES: usize = 4,
    const MAX_VOLUMES: usize = 1,
> {
    vmgr: &'a VolumeManager<D, T, MAX_DIRS, MAX_FILES, MAX_VOLUMES>,
    file: RawFile,
    len: u32,
}

impl<'a, D: BlockDevice, T: TimeSource, const MAX_DIRS: usize, const MAX_FILES: usize, const MAX_VOLUMES: usize>
    SdByteSource<'a, D, T, MAX_DIRS, MAX_FILES, MAX_VOLUMES>
{
    /// Wrap an already-open `file` (its length is `len`) for reading. The caller owns the
    /// handle's lifetime: the source borrows the manager but not the handle, so closing the
    /// file is the caller's job once the source is dropped.
    pub fn new(vmgr: &'a VolumeManager<D, T, MAX_DIRS, MAX_FILES, MAX_VOLUMES>, file: RawFile, len: u32) -> Self {
        SdByteSource { vmgr, file, len }
    }
}

impl<D: BlockDevice, T: TimeSource, const MAX_DIRS: usize, const MAX_FILES: usize, const MAX_VOLUMES: usize> ByteSource
    for SdByteSource<'_, D, T, MAX_DIRS, MAX_FILES, MAX_VOLUMES>
{
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        // The FAT seam narrows once, here, and works in `u32` below: `file_seek_from_start` takes a
        // `u32` because a FAT32 file cannot be longer than one. That is FAT's own wall rather than
        // the read seam's — this arm dies with FS7/8 and is widened only where the trait requires.
        let offset = u32::try_from(offset).map_err(|_| Error::BadOffset)?;
        // Prove range errors before touching the medium. Once the range is known good, a seek
        // failure is an I/O failure (card removal, corrupt FAT chain, etc.), not malformed caller
        // input. Callers rely on that distinction to retry an object whose validity could not be
        // established because the medium became unreadable.
        let count = u32::try_from(buf.len()).map_err(|_| Error::BadOffset)?;
        let end = offset.checked_add(count).ok_or(Error::BadOffset)?;
        if end > self.len {
            return Err(Error::BadOffset);
        }
        self.vmgr.file_seek_from_start(self.file, offset).map_err(|_| Error::Io)?;
        // One SD read returns at most a block, so loop until `buf` is filled; a 0-length read
        // means we ran into EOF before filling it (the caller asked for too much).
        let mut done = 0;
        while done < buf.len() {
            match self.vmgr.read(self.file, &mut buf[done..]) {
                Ok(0) => return Err(Error::BadOffset),
                Ok(n) => done += n,
                Err(_) => return Err(Error::Io),
            }
        }
        Ok(())
    }

    fn len(&self) -> u64 {
        self.len.into()
    }
}

/// A [`ByteSink`] over an open FatFs file — the device backing for the route writer's "stream the
/// body then patch the header" flow. Writes append at the file's current offset;
/// [`patch_at`](ByteSink::patch_at) seeks back, overwrites, and returns to the append point.
///
/// `patch_at` is unused by the on-device flow (the `.obct` log is append-only and `track_to_gpx`
/// writes front-to-back); it's implemented for `ByteSink` completeness + the host-side route
/// conversion that does patch a header.
pub struct SdByteSink<
    'a,
    D: BlockDevice,
    T: TimeSource,
    const MAX_DIRS: usize = 4,
    const MAX_FILES: usize = 4,
    const MAX_VOLUMES: usize = 1,
> {
    vmgr: &'a VolumeManager<D, T, MAX_DIRS, MAX_FILES, MAX_VOLUMES>,
    file: RawFile,
}

impl<'a, D: BlockDevice, T: TimeSource, const MAX_DIRS: usize, const MAX_FILES: usize, const MAX_VOLUMES: usize>
    SdByteSink<'a, D, T, MAX_DIRS, MAX_FILES, MAX_VOLUMES>
{
    /// Wrap an open, writable `file`. The caller flushes/closes it when done.
    pub fn new(vmgr: &'a VolumeManager<D, T, MAX_DIRS, MAX_FILES, MAX_VOLUMES>, file: RawFile) -> Self {
        SdByteSink { vmgr, file }
    }
}

impl<D: BlockDevice, T: TimeSource, const MAX_DIRS: usize, const MAX_FILES: usize, const MAX_VOLUMES: usize> ByteSink
    for SdByteSink<'_, D, T, MAX_DIRS, MAX_FILES, MAX_VOLUMES>
{
    fn write(&mut self, buf: &[u8]) -> Result<(), Error> {
        self.vmgr.write(self.file, buf).map_err(|_| Error::Io)
    }

    fn patch_at(&mut self, offset: u32, buf: &[u8]) -> Result<(), Error> {
        // Snapshot the append point (= current length), drop back to `offset`, overwrite, then
        // restore — so a later sequential `write` resumes appending where the body left off.
        let end = self.vmgr.file_length(self.file).map_err(|_| Error::Io)?;
        self.vmgr.file_seek_from_start(self.file, offset).map_err(|_| Error::BadOffset)?;
        self.vmgr.write(self.file, buf).map_err(|_| Error::Io)?;
        self.vmgr.file_seek_from_start(self.file, end).map_err(|_| Error::BadOffset)?;
        Ok(())
    }
}

/// An [`obc_ports::TrackSink`] writing each accepted fix to the open `.obct` ride log. The app encodes
/// a [`TrackPoint`] and hands it here; this appends its fixed 20-byte record ([`encode_record`]).
///
/// A failed SD write is reported straight back through [`record`](TrackSink::record)'s
/// `Result` — the app owns the reaction (it raises the "recording error" indicator so the rider
/// knows the log dropped points; see issue #11). So this sink is a pure per-fix adapter with no
/// state of its own, safe to rebuild each tick over the open handle.
pub struct SdTrackSink<
    'a,
    D: BlockDevice,
    T: TimeSource,
    const MAX_DIRS: usize = 4,
    const MAX_FILES: usize = 4,
    const MAX_VOLUMES: usize = 1,
> {
    vmgr: &'a VolumeManager<D, T, MAX_DIRS, MAX_FILES, MAX_VOLUMES>,
    file: RawFile,
}

impl<'a, D: BlockDevice, T: TimeSource, const MAX_DIRS: usize, const MAX_FILES: usize, const MAX_VOLUMES: usize>
    SdTrackSink<'a, D, T, MAX_DIRS, MAX_FILES, MAX_VOLUMES>
{
    /// Wrap the open, append-mode `.obct` log file.
    pub fn new(vmgr: &'a VolumeManager<D, T, MAX_DIRS, MAX_FILES, MAX_VOLUMES>, file: RawFile) -> Self {
        SdTrackSink { vmgr, file }
    }
}

impl<D: BlockDevice, T: TimeSource, const MAX_DIRS: usize, const MAX_FILES: usize, const MAX_VOLUMES: usize> TrackSink
    for SdTrackSink<'_, D, T, MAX_DIRS, MAX_FILES, MAX_VOLUMES>
{
    fn record(&mut self, p: TrackPoint) -> Result<(), TrackError> {
        // A card pull / write error surfaces as `Err`; the app raises the recording-error indicator.
        // Never `panic!` here — a hard fault mid-ride is never the right answer to a bad SD write.
        self.vmgr.write(self.file, &encode_record(&p)).map_err(|_| TrackError)
    }
}
