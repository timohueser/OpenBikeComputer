//! FatFs byte adapters over a microSD card — the board-agnostic half of "map/route/track
//! on SD" (issue #36).
//!
//! These mirror the simulator's `std`-file store (`obc-sim`'s `routes.rs` / `track.rs`): the
//! shared format code in [`obc_route`] never touches a filesystem, it reads through a
//! [`ByteSource`] and writes through a [`ByteSink`], and the app logs the ride through an
//! [`obc_app::TrackSink`]. On the host those seams are backed by `std::fs`; here they're
//! backed by an [`embedded_sdmmc`] FatFs file. Only these thin adapters are platform-specific
//! — so the same `RouteReader` / `track_to_gpx` run on the device and in the simulator, and
//! the same adapters port to the nRF firmware (only the concrete [`embedded_sdmmc::SdCard`]
//! bus type changes).
//!
//! All three are generic over the [`BlockDevice`] + [`TimeSource`] so they pull in no bus or
//! board types. They borrow the [`VolumeManager`] shared (`&'a`) and hold a [`RawFile`]
//! handle: the manager has interior mutability (every method takes `&self`), so a board can
//! hold an [`SdByteSource`] over the route and an [`SdTrackSink`] over the open track log at
//! the same time, and feed both to one [`obc_app::App::tick`].

use embedded_sdmmc::{BlockDevice, RawFile, TimeSource, VolumeManager};
use obc_app::TrackSink;
use obc_route::{encode_record, ByteSink, ByteSource, Error, TrackPoint};

/// A random-access [`ByteSource`] over an open FatFs file — the device's "stream from the SD
/// card" backing for [`RouteReader`](obc_route::RouteReader) / [`RouteSummary::read`]. Mirrors
/// [`SliceSource`](obc_route::SliceSource), but each [`read_at`](ByteSource::read_at) seeks the
/// file then reads, so a route never has to be resident.
///
/// The length is captured once at construction (the file doesn't grow under a reader), so
/// [`len`](ByteSource::len) is free and matches `SliceSource`'s cheap length.
pub struct SdByteSource<'a, D: BlockDevice, T: TimeSource> {
    vmgr: &'a VolumeManager<D, T>,
    file: RawFile,
    len: u32,
}

impl<'a, D: BlockDevice, T: TimeSource> SdByteSource<'a, D, T> {
    /// Wrap an already-open `file` (its length is `len`) for reading. The caller owns the
    /// handle's lifetime: the source borrows the manager but not the handle, so closing the
    /// file is the caller's job once the source is dropped.
    pub fn new(vmgr: &'a VolumeManager<D, T>, file: RawFile, len: u32) -> Self {
        SdByteSource { vmgr, file, len }
    }
}

impl<D: BlockDevice, T: TimeSource> ByteSource for SdByteSource<'_, D, T> {
    fn read_at(&self, offset: u32, buf: &mut [u8]) -> Result<(), Error> {
        // Seeking past EOF is an out-of-range request, not a medium failure.
        self.vmgr.file_seek_from_start(self.file, offset).map_err(|_| Error::BadOffset)?;
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

    fn len(&self) -> u32 {
        self.len
    }
}

/// A [`ByteSink`] over an open FatFs file — the device backing for the route writer's
/// "stream the body then patch the header" flow (`OBCR`/`GPX` export). Writes append at the
/// file's current offset (the manager tracks it); [`patch_at`](ByteSink::patch_at) seeks back,
/// overwrites, and returns to the append point.
///
/// The on-device track log (`.obct`) is header-less and append-only, and `track_to_gpx`
/// writes a GPX front-to-back, so `patch_at` is unused by #36's flow — it's implemented for
/// `ByteSink` completeness (and the host-side route conversion that does patch a header).
pub struct SdByteSink<'a, D: BlockDevice, T: TimeSource> {
    vmgr: &'a VolumeManager<D, T>,
    file: RawFile,
}

impl<'a, D: BlockDevice, T: TimeSource> SdByteSink<'a, D, T> {
    /// Wrap an open, writable `file`. The caller flushes/closes it when done.
    pub fn new(vmgr: &'a VolumeManager<D, T>, file: RawFile) -> Self {
        SdByteSink { vmgr, file }
    }
}

impl<D: BlockDevice, T: TimeSource> ByteSink for SdByteSink<'_, D, T> {
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

/// An [`obc_app::TrackSink`] writing each accepted fix to the open `.obct` ride log — the
/// device counterpart of the simulator's `OpenLog`. The app encodes a [`TrackPoint`] and hands
/// it here; this appends its fixed 16-byte record ([`encode_record`]) to the file.
///
/// [`record`](TrackSink::record) is infallible by the app's contract (see issue #11), so a
/// failed SD write can't propagate — it's swallowed and latched in [`had_error`](Self::had_error)
/// so the board can surface "the ride log dropped points" after the fact, rather than the ride
/// loop having to handle a write error mid-frame.
pub struct SdTrackSink<'a, D: BlockDevice, T: TimeSource> {
    vmgr: &'a VolumeManager<D, T>,
    file: RawFile,
    error: bool,
}

impl<'a, D: BlockDevice, T: TimeSource> SdTrackSink<'a, D, T> {
    /// Wrap the open, append-mode `.obct` log file.
    pub fn new(vmgr: &'a VolumeManager<D, T>, file: RawFile) -> Self {
        SdTrackSink { vmgr, file, error: false }
    }

    /// Whether any [`record`](TrackSink::record) write has failed since construction — a
    /// latched "the log is incomplete" flag the board can read when the ride ends.
    pub fn had_error(&self) -> bool {
        self.error
    }
}

impl<D: BlockDevice, T: TimeSource> TrackSink for SdTrackSink<'_, D, T> {
    fn record(&mut self, p: TrackPoint) {
        if self.vmgr.write(self.file, &encode_record(&p)).is_err() {
            self.error = true;
        }
    }
}
