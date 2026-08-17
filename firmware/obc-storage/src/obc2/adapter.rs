//! The §13.1 FAT adapter: OBC2's gated-record I/O over `embedded_sdmmc`.
//!
//! `OBC2_Storage_Format.md` §13.1 lists eight obligations an adapter must meet before it can host
//! this format, "none of them may be emulated by relaxing a durability point". This module is that
//! adapter over the vendored `embedded_sdmmc` fork, and it is deliberately small: the fork already
//! satisfies three of the obligations outright, and the other five are met by *how* it is called,
//! which is what the wrapper exists to make non-optional.
//!
//! ## What the fork does today, obligation by obligation
//!
//! Measured against `timohueser/embedded-sdmmc-rs@e846db6` (branch `cmd25-multiblock-write`, forked
//! from upstream `0.9.0`). These are facts about that code, not aspirations:
//!
//! - **Synchronization.** There is no `sync` primitive and none is needed for payload bytes:
//!   `VolumeManager::write` pushes every touched block to the `BlockDevice` before it returns —
//!   through `BlockCache::write_back` on the single-block path and directly on the multi-block
//!   CMD25 path. The one-block cache is write-through, so no payload byte is ever left in software.
//!   Persisting therefore means the block device's `write` completed and the card left its busy
//!   state, which is the `BlockDevice` implementation's contract, not the filesystem's.
//! - **Clean flush.** ❌ **Not satisfied by `flush_file`.** `flush_file` writes the FAT32 FSInfo
//!   sector and then read-modify-writes the 32-byte directory entry, gated only on an
//!   `open_files[..].dirty` flag that `write` sets and that **nothing ever clears** — not
//!   `flush_file`, not `close_file`. Every `write` also stamps a fresh mtime into the entry, so the
//!   entry's bytes genuinely change each time. An OBC2 commit that called `flush_file` after each of
//!   its three syncs would put the single-copy `/OBC2` directory sector at risk three times per
//!   commit, which is exactly what §13.1 forbids. [`Adapter::sync_fixed`] is the flush that skips
//!   unchanged metadata: it verifies the recorded length is the one initialization set and then does
//!   nothing, because there is nothing left to persist. See the module note below for the fork
//!   change that would make this a positive primitive rather than an omission.
//! - **Full-length initialization.** `preallocate` extends the cluster chain and explicitly does not
//!   change the recorded length; `write` is the only thing that does. [`Adapter::create_fixed`]
//!   therefore preallocates and then writes the whole file in zeros, checking the recorded length
//!   afterwards.
//! - **Chain longer than length.** Nothing to implement: the fork's `preallocate` walks the existing
//!   chain before extending it and its free-cluster accounting reads the FAT, so a chain longer than
//!   the length is allocated space that no scan counts free.
//! - **Write completeness.** The fork's `write` clamps at `MAX_FILE_SIZE` and returns `Ok(())`
//!   without a byte count, so the check §13.1 requires cannot be a return-value check: every write
//!   here is followed by comparing `file_offset` against the intended end offset.
//! - **Seek bound.** `FileInfo::seek_from_start` rejects an offset past the recorded length with
//!   `InvalidOffset`. [`Adapter::write_at`] additionally bounds the *end* of the write, which the
//!   fork does not: a write starting inside the file and running past its end would extend it.
//! - **Gate isolation.** A 512-byte write at a sector-aligned offset takes the fork's
//!   `blank_mut` path — the whole block is replaced, so no other sector is read, modified or
//!   written. [`Adapter::write_gate`] is the same call with that alignment asserted.
//! - **Absent primitives.** The fork has no `delete_dir` and no `rename`, and this adapter exposes
//!   neither. `make_dir_in_dir` on a present directory returns `DirAlreadyExists`, which
//!   [`Adapter::make_dir`] treats as success, as §12's reuse rule requires.
//!
//! ## The fork change this adapter works around
//!
//! `flush_file` should be a no-op when nothing the directory entry records has changed. The minimal
//! upstream fix is to split `FileInfo::dirty` into "the entry's length/first-cluster changed" and
//! "bytes were written", clear it in `flush_file`, and skip both the FSInfo and the entry write when
//! only the latter is set — i.e. stop stamping mtime on an in-place overwrite, or accept that mtime
//! is not worth a single-copy sector. Until then OBC2's rule is the negative one this module
//! enforces: **never call `flush_file` or `close_file` on a gated file after initialization.** A
//! gated file is opened once at mount and closed at unmount, and the one directory-entry write that
//! close performs happens when the store is being torn down rather than three times per commit.
//!
//! A negative law in prose is one autocomplete away from being broken, so it is also expressed in
//! the types: [`GatedFile`] is what the adapter hands out and it cannot be passed to the volume
//! manager without [`GatedFile::raw`]. The two remaining ways past it — that accessor and
//! [`Adapter::volume_manager`] — carry the law on their own doc comments, and both are worth
//! grepping for during review.

use embedded_sdmmc::{BlockDevice, Mode, RawDirectory, RawFile, TimeSource, VolumeManager};

use super::limits::{GATE_LEN, SECTOR};

/// What a §13.1 adapter operation can fail with.
///
/// Every variant is a fact about the medium or about a caller's arithmetic; none of them is a
/// repair. `ShortWrite` and `LengthChanged` are the two §13.1 invents: they exist because the fork
/// reports a clamped write as success, so a length that is not exactly the intended one is the only
/// evidence.
///
/// ## Three of them are a mount classification, not just a failure
///
/// §12's table turns a failure into a mount class, and the three classes it needs cannot be
/// recovered from one undifferentiated I/O error: [`CorruptStore`](Self::CorruptStore) is the
/// "lost single-copy FAT structure" that mounts recovery-failed and read-only,
/// [`Media`](Self::Media) is a card that stopped answering — a different conversation with the
/// rider — and [`CallerBug`](Self::CallerBug) is neither, because it means this code asked for
/// something incoherent and no mount class should ever be derived from it. Collapsing them would
/// make a §12 mount decision unimplementable above this seam, so the split is here rather than in
/// the store that needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterError {
    /// The block device failed: the card stopped answering, or answered with an error.
    ///
    /// §12's mount classification treats this as a medium fault rather than a store fault — the
    /// store may be perfectly intact on a card that is no longer readable.
    Media,
    /// The filesystem itself is not coherent: a malformed BPB, a bad cluster, a FAT chain that runs
    /// into free space, or a volume that is no longer there.
    ///
    /// §1.1: losing a single-copy FAT structure "is an unrecoverable store fault … and mounts
    /// recovery-failed and read-only with evidence preserved". This is how that reaches the mount.
    CorruptStore,
    /// This code asked the filesystem for something incoherent — a stale handle, a file opened
    /// twice, a re-entrant call. Never a fact about the card, so never a mount class.
    ///
    /// It carries a `debug_assert` at the point of translation, because in a debug build the useful
    /// behaviour is to stop at the mistake rather than to propagate a typed error nobody will read.
    CallerBug,
    /// The file or directory does not exist.
    NotFound,
    /// The file already exists and the caller asked for an exclusive create.
    AlreadyExists,
    /// The medium has no space for this write.
    Full,
    /// The volume is mounted read-only, or the handle was opened read-only.
    ReadOnly,
    /// The handle budget of §13 is exhausted.
    Handles,
    /// The name is not a FAT 8.3 name firmware may create.
    BadName,
    /// §13.1's seek bound: the request lies past the file's recorded length.
    OutOfRange,
    /// A gate or record write was not sector-aligned, so gate isolation could not be guaranteed.
    Misaligned,
    /// A record body write would have reached into the gate sector that publishes it.
    ///
    /// The gate is written last and alone, so a body write that overlapped it would publish a record
    /// the same instant it wrote it — collapsing the two durability points §6 keeps apart.
    GateOverlap {
        /// Where the body write would have ended.
        body_end: u32,
        /// Where its gate begins.
        gate: u32,
    },
    /// §13.1's write completeness: the write reported success having moved fewer bytes.
    ShortWrite {
        /// The offset the write was expected to end at.
        wanted: u32,
        /// The offset it actually ended at.
        reached: u32,
    },
    /// A fixed-length file's recorded length is not the one initialization gave it.
    LengthChanged {
        /// The length §3 fixes for this file.
        expected: u32,
        /// What the directory entry says now.
        actual: u32,
    },
}

/// Translates the fork's error into the classes §12 needs, exhaustively.
///
/// Deliberately without a catch-all arm: a new variant in the fork should fail this build rather
/// than land silently in whichever class the wildcard happened to name.
///
/// It is public because [`Adapter::volume_manager`] is: a caller that reaches through the escape
/// hatch for one of the operations §13.1 does not constrain — directory iteration, deletion,
/// opening a `GEN` payload for streaming — must classify its failures the same way the constrained
/// operations do, or §12's mount table becomes unimplementable one call at a time.
pub fn classify<E: core::fmt::Debug>(error: embedded_sdmmc::Error<E>) -> AdapterError {
    use embedded_sdmmc::Error as E;
    match error {
        E::NotFound => AdapterError::NotFound,
        E::FileAlreadyExists | E::DirAlreadyExists => AdapterError::AlreadyExists,
        E::DiskFull | E::NotEnoughSpace => AdapterError::Full,
        E::ReadOnly => AdapterError::ReadOnly,
        E::TooManyOpenFiles | E::TooManyOpenDirs | E::TooManyOpenVolumes => AdapterError::Handles,
        E::FilenameError(_) => AdapterError::BadName,
        E::InvalidOffset | E::EndOfFile => AdapterError::OutOfRange,

        // The card, not the store.
        E::DeviceError(_) => AdapterError::Media,

        // The store, not the card. `ConversionError` and `AllocationError` join these because the
        // fork raises them when on-disk values will not fit the arithmetic the format guarantees —
        // an implausible cluster number or a chain the allocator could not follow — which is a
        // statement about the volume rather than about the request.
        E::FormatError(_)
        | E::BadCluster
        | E::UnterminatedFatChain
        | E::NoSuchVolume
        | E::BadBlockSize(_)
        | E::ConversionError
        | E::AllocationError => AdapterError::CorruptStore,

        // Neither. Every one of these is reachable only by asking for something incoherent: a handle
        // from a closed file, a second open of an open object, a call made from inside a directory
        // iterator. `Unsupported` is here too — it means this code asked the fork to do something it
        // does not implement, which is a mistake in the caller and not a fact about the medium.
        E::BadHandle
        | E::FileAlreadyOpen
        | E::DirAlreadyOpen
        | E::OpenedDirAsFile
        | E::OpenedFileAsDir
        | E::DeleteDirAsFile
        | E::VolumeStillInUse
        | E::VolumeAlreadyOpen
        | E::LockError
        | E::Unsupported => {
            debug_assert!(false, "OBC2 adapter misuse: the FAT layer refused an incoherent request");
            AdapterError::CallerBug
        }
    }
}

/// An open handle to a fixed-length gated OBC2 file.
///
/// It exists to make the module's one law hard to break by accident. The law is negative — *never
/// `flush_file` or `close_file` a gated file after initialization* — and a negative law expressed
/// only in prose is one autocomplete away from being broken: `vmgr.flush_file(file)` is the obvious
/// call, it compiles, it looks like a sync, and it silently rewrites FSInfo and the directory entry.
///
/// A `GatedFile` cannot be passed to the volume manager at all. Reaching the raw handle takes
/// [`raw`](Self::raw), which is a deliberate, greppable act with the law restated on it. That is not
/// containment in the type-system sense — [`Adapter::volume_manager`] still exists — but it moves
/// the mistake from "the natural thing to write" to "a thing you had to mean".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GatedFile(RawFile);

impl GatedFile {
    /// The underlying handle.
    ///
    /// **Only for closing the file at unmount.** §13.1: an adapter whose flush "unconditionally
    /// rewrites FSInfo and the 32-byte directory entry puts a single-copy sector at risk on every
    /// sync"; the fork's `close_file` flushes, and its `flush_file` never clears the dirty flag that
    /// `write` sets. So `close_file` on a gated file costs exactly one directory-entry write, which
    /// is acceptable while the store is being torn down and at no other time. Never hand this to
    /// `flush_file`; [`Adapter::sync_fixed`] is the sync a gated file has.
    pub fn raw(self) -> RawFile {
        self.0
    }
}

/// The §13.1 adapter over one mounted volume.
///
/// It borrows the [`VolumeManager`] shared, exactly as the other adapters in this crate do: the
/// manager has interior mutability, so the store can hold this alongside the map/route sources
/// without a second FAT owner. It carries no state of its own — every obligation is expressed as a
/// check against what the filesystem reports, so there is nothing here that can go stale.
pub struct Adapter<
    'a,
    D: BlockDevice,
    T: TimeSource,
    const MAX_DIRS: usize = 4,
    const MAX_FILES: usize = 16,
    const MAX_VOLUMES: usize = 1,
> {
    vmgr: &'a VolumeManager<D, T, MAX_DIRS, MAX_FILES, MAX_VOLUMES>,
}

impl<'a, D: BlockDevice, T: TimeSource, const MAX_DIRS: usize, const MAX_FILES: usize, const MAX_VOLUMES: usize>
    Adapter<'a, D, T, MAX_DIRS, MAX_FILES, MAX_VOLUMES>
{
    /// Wraps a mounted volume manager.
    pub fn new(vmgr: &'a VolumeManager<D, T, MAX_DIRS, MAX_FILES, MAX_VOLUMES>) -> Self {
        Adapter { vmgr }
    }

    /// The volume manager underneath, for the operations §13.1 does not constrain (directory
    /// iteration, deletion, opening a `GEN` payload for streaming).
    ///
    /// ⚠️ **The escape hatch, and the module's law applies through it.** Never call `flush_file` or
    /// `close_file` on a gated `/OBC2` file after initialization: both write the directory entry and
    /// FSInfo — the fork's dirty flag is set by every `write` and cleared by nothing — and that puts
    /// the single-copy sector holding every `/OBC2` entry at risk on a path §13.1 requires to write
    /// no metadata at all. A [`GatedFile`] cannot be passed here without [`GatedFile::raw`], which
    /// is the seam to grep for; unrelated files (a `GEN` payload, a staged import) are unaffected.
    pub fn volume_manager(&self) -> &'a VolumeManager<D, T, MAX_DIRS, MAX_FILES, MAX_VOLUMES> {
        self.vmgr
    }

    /// `make_dir` on a possibly already-present directory (§12: "not an error and does not restart
    /// the order").
    pub fn make_dir(&self, parent: RawDirectory, name: &str) -> Result<(), AdapterError> {
        match self.vmgr.make_dir_in_dir(parent, name) {
            Ok(()) => Ok(()),
            Err(embedded_sdmmc::Error::DirAlreadyExists) => Ok(()),
            Err(error) => Err(classify(error)),
        }
    }

    /// The file's recorded length.
    pub fn length(&self, file: GatedFile) -> Result<u32, AdapterError> {
        self.vmgr.file_length(file.raw()).map_err(classify)
    }

    /// Reads `buf.len()` bytes at `offset`, bounded by the recorded length.
    ///
    /// One SD read returns at most a block, so this loops until the buffer is full; a zero-length
    /// read before then is EOF, which the bound check above should already have excluded and which
    /// is therefore reported as an I/O failure rather than as a short read.
    pub fn read_at(&self, file: GatedFile, offset: u32, buf: &mut [u8]) -> Result<(), AdapterError> {
        self.end_of(file, offset, buf.len())?;
        self.vmgr.file_seek_from_start(file.raw(), offset).map_err(classify)?;
        let mut done = 0;
        while done < buf.len() {
            match self.vmgr.read(file.raw(), &mut buf[done..]) {
                // The bound check above proved these bytes are inside the recorded length, so a
                // zero-length read means the cluster chain ends before the length says it does —
                // a lost FAT structure, which is §1.1's unrecoverable store fault.
                Ok(0) => return Err(AdapterError::CorruptStore),
                Ok(read) => done += read,
                Err(error) => return Err(classify(error)),
            }
        }
        Ok(())
    }

    /// Writes `bytes` at `offset` inside a file that must not change length.
    ///
    /// This is the whole of §13.1's write completeness and seek bound in one call: the end of the
    /// write is bounded against the recorded length *before* the seek, so the fork can never be
    /// asked to extend the file, and the resulting offset is compared against the intended one
    /// afterwards, because the fork reports a clamped write as `Ok(())`.
    pub fn write_at(&self, file: GatedFile, offset: u32, bytes: &[u8]) -> Result<(), AdapterError> {
        let end = self.end_of(file, offset, bytes.len())?;
        let before = self.length(file)?;
        self.vmgr.file_seek_from_start(file.raw(), offset).map_err(classify)?;
        self.vmgr.write(file.raw(), bytes).map_err(classify)?;
        let reached = self.vmgr.file_offset(file.raw()).map_err(classify)?;
        if reached != end {
            return Err(AdapterError::ShortWrite { wanted: end, reached });
        }
        let after = self.length(file)?;
        if after != before {
            return Err(AdapterError::LengthChanged { expected: before, actual: after });
        }
        Ok(())
    }

    /// Writes one 512-byte gate sector.
    ///
    /// §13.1's gate isolation: "writing and synchronizing 512 bytes at a gate offset must not
    /// read-modify-write any other sector of the file". The fork replaces a whole block without
    /// reading it only when the write starts at a block boundary and covers the block, so the
    /// alignment is the property being checked here — a misaligned gate offset would silently become
    /// a read-modify-write and gate invalidation would stop being all-or-nothing.
    pub fn write_gate(&self, file: GatedFile, offset: u32, gate: &[u8; GATE_LEN]) -> Result<(), AdapterError> {
        if !(offset as usize).is_multiple_of(SECTOR) {
            return Err(AdapterError::Misaligned);
        }
        self.write_at(file, offset, gate)
    }

    /// Writes a record body, refusing a write that would reach the gate sector that publishes it.
    ///
    /// §6 keeps two durability points apart: the body is written and made durable, and only then
    /// does the gate say the body became a record. A body write that ran into its own gate offset
    /// would collapse them — and, because the gate sits immediately after the body in every record
    /// shape §4 tabulates, an off-by-one in a body length is exactly the arithmetic that does it.
    pub fn write_body(&self, file: GatedFile, offset: u32, gate_offset: u32, bytes: &[u8]) -> Result<(), AdapterError> {
        let count = u32::try_from(bytes.len()).map_err(|_| AdapterError::OutOfRange)?;
        let body_end = offset.checked_add(count).ok_or(AdapterError::OutOfRange)?;
        if offset <= gate_offset && body_end > gate_offset {
            return Err(AdapterError::GateOverlap { body_end, gate: gate_offset });
        }
        self.write_at(file, offset, bytes)
    }

    /// The clean flush of §13.1: synchronizing a fixed-length gated file.
    ///
    /// It writes nothing. Every payload byte reached the block device inside [`write_at`], and a
    /// file whose recorded length has not changed has no directory entry and no FSInfo left to
    /// persist — so the only honest implementation of this obligation over this fork is to prove the
    /// length is still the fixed one and return. Calling the fork's `flush_file` here instead is the
    /// behaviour §13.1 rules out; see the module documentation.
    ///
    /// `expected` is the length §3 fixes for the file. A mismatch means something extended a file
    /// that must never grow, which is a store fault rather than a sync failure.
    pub fn sync_fixed(&self, file: GatedFile, expected: u32) -> Result<(), AdapterError> {
        let actual = self.length(file)?;
        if actual != expected {
            return Err(AdapterError::LengthChanged { expected, actual });
        }
        Ok(())
    }

    /// The other sync: the one that *does* have metadata to persist.
    ///
    /// ⚠️ **Initialization only.** This is the fork's `flush_file`: it rewrites FSInfo and
    /// read-modify-writes the 32-byte directory entry, every time, because the dirty flag it is
    /// gated on is set by every `write` and cleared by nothing. That is correct exactly once per
    /// file — when [`create_fixed`](Self::create_fixed) has just changed the recorded length and the
    /// directory entry has to catch up — and forbidden on every path afterwards. §13.1: an adapter
    /// whose flush unconditionally rewrites those sectors "does not satisfy this contract", and a
    /// commit performs three syncs, so calling this per sync would risk the sector holding all of
    /// `/OBC2`'s directory entries three times per commit. [`sync_fixed`](Self::sync_fixed) is the
    /// sync a gated file has after initialization; it writes nothing, which is the point.
    pub fn sync_metadata(&self, file: GatedFile) -> Result<(), AdapterError> {
        self.vmgr.flush_file(file.raw()).map_err(classify)
    }

    /// Creates a fixed-size OBC2 file at its full length in zeros (§13.1, full-length
    /// initialization), leaving it open.
    ///
    /// Preallocation first, because it is much cheaper than letting `write` extend the chain one
    /// cluster at a time — but preallocation is not length, so the zeros are what make every slot
    /// offset addressable. `scratch` is the zero-fill buffer; its length is the write granule and
    /// the caller owns the RAM. The recorded length is checked exactly once at the end, and the
    /// directory entry is persisted before the handle is returned.
    pub fn create_fixed(
        &self,
        parent: RawDirectory,
        name: &str,
        len: u32,
        scratch: &mut [u8],
    ) -> Result<GatedFile, AdapterError> {
        // A granule that is not a whole number of sectors makes every write after the first one
        // sector-misaligned, which drops the fork onto its read-modify-write path and quietly costs
        // a read per block for the whole 4.6 MB zero-fill. Zero is worse: it is an infinite loop.
        // Both are mistakes in this code rather than facts about the card.
        if scratch.is_empty() || !scratch.len().is_multiple_of(SECTOR) {
            debug_assert!(false, "the zero-fill granule must be a nonzero multiple of 512 bytes");
            return Err(AdapterError::CallerBug);
        }
        scratch.fill(0);
        let file =
            GatedFile(self.vmgr.open_file_in_dir(parent, name, Mode::ReadWriteCreateOrTruncate).map_err(classify)?);
        match self.fill(file, len, scratch) {
            Ok(()) => Ok(file),
            Err(error) => {
                // A failed creation leaves a short file, which §12's pre-birth rules classify and
                // repair; closing the handle is all this level can do about it.
                let _ = self.vmgr.close_file(file.raw());
                Err(error)
            }
        }
    }

    fn fill(&self, file: GatedFile, len: u32, scratch: &mut [u8]) -> Result<(), AdapterError> {
        let granule = u32::try_from(scratch.len()).map_err(|_| AdapterError::OutOfRange)?;
        // Returns short rather than failing when the volume is nearly full, so the length check
        // below is what actually reports the failure.
        self.vmgr.preallocate(file.raw(), len).map_err(classify)?;
        self.vmgr.file_seek_from_start(file.raw(), 0).map_err(classify)?;
        let mut written = 0u32;
        while written < len {
            let chunk = granule.min(len - written) as usize;
            self.vmgr.write(file.raw(), &scratch[..chunk]).map_err(classify)?;
            let reached = self.vmgr.file_offset(file.raw()).map_err(classify)?;
            let wanted = written + chunk as u32;
            if reached != wanted {
                return Err(AdapterError::ShortWrite { wanted, reached });
            }
            written = wanted;
        }
        let actual = self.length(file)?;
        if actual != len {
            return Err(AdapterError::LengthChanged { expected: len, actual });
        }
        // The length changed, so this is the sync that must persist the directory entry.
        self.sync_metadata(file)
    }

    /// Opens an existing fixed-size file and proves it is at its full length.
    ///
    /// §13.1: "no offset beyond the recorded length is addressable, so a preallocated-but-short file
    /// cannot be slot-addressed at all". A store that mounted such a file and started addressing
    /// slots in it would fail at an arbitrary later offset instead of here.
    pub fn open_fixed(&self, parent: RawDirectory, name: &str, len: u32) -> Result<GatedFile, AdapterError> {
        let file = GatedFile(self.vmgr.open_file_in_dir(parent, name, Mode::ReadWriteAppend).map_err(classify)?);
        // A failure here is a medium or store fault and must not be flattened into "length zero",
        // which would report a readable-but-truncated file and an unreadable one identically.
        let actual = match self.length(file) {
            Ok(actual) => actual,
            Err(error) => {
                let _ = self.vmgr.close_file(file.raw());
                return Err(error);
            }
        };
        if actual != len {
            let _ = self.vmgr.close_file(file.raw());
            return Err(AdapterError::LengthChanged { expected: len, actual });
        }
        Ok(file)
    }

    /// The end offset of a request, refused when it would pass the recorded length.
    fn end_of(&self, file: GatedFile, offset: u32, count: usize) -> Result<u32, AdapterError> {
        let count = u32::try_from(count).map_err(|_| AdapterError::OutOfRange)?;
        let end = offset.checked_add(count).ok_or(AdapterError::OutOfRange)?;
        if end > self.length(file)? {
            return Err(AdapterError::OutOfRange);
        }
        Ok(end)
    }
}

#[cfg(test)]
mod tests {
    use std::boxed::Box;
    use std::vec::Vec;

    use embedded_sdmmc::VolumeIdx;

    use super::*;
    use crate::fat_extents::SharedBlockDevice;
    use crate::obc2::blocklog::WriteLog;
    use crate::obc2::fatsim::{self, fat32_card, geometry_sectors, touched, Layout, NullTime, SparseDisk};
    use crate::obc2::geometry::{self, Region, VolumeGeometry};
    use crate::obc2::limits::{RIDE_FILE_LEN, SLOT_FILE_LEN, SMALL_BODY_LEN, SMALL_GATE_OFFSET};

    /// The instrumented card: the sparse FAT32 image with the §13.1 write log over it.
    type Card = WriteLog<SparseDisk, 256>;
    /// The board's own handle budget (§13: four directory handles, sixteen file handles).
    type Vmgr = VolumeManager<SharedBlockDevice<'static, Card>, NullTime, 4, 16, 1>;
    type Fat = Adapter<'static, SharedBlockDevice<'static, Card>, NullTime, 4, 16, 1>;

    /// A mounted conforming card with `/OBC2` created, plus the log and the §1.1 geometry.
    ///
    /// The manager takes the device by value, so the log is reached through a leaked `'static`
    /// reference — the same `SharedBlockDevice` split the board uses to keep a raw device twin for
    /// the extent path, here keeping the instrument reachable.
    struct Store {
        card: &'static Card,
        vmgr: &'static Vmgr,
        geometry: VolumeGeometry,
        obc2: RawDirectory,
    }

    impl Store {
        fn mounted() -> Store {
            let layout = Layout::default();
            let card: &'static Card = Box::leak(Box::new(WriteLog::new(fat32_card(layout))));
            let vmgr: &'static Vmgr =
                Box::leak(Box::new(VolumeManager::new_with_limits(SharedBlockDevice(card), NullTime, 5_000)));
            let (mbr, bpb) = geometry_sectors(card.device(), layout.partition_start_lba);
            let geometry = geometry::admit(&mbr, &bpb, 0).expect("the simulated card is conforming");
            let volume = vmgr.open_raw_volume(VolumeIdx(0)).expect("the simulated card mounts");
            let root = vmgr.open_root_dir(volume).expect("a root directory");
            Adapter::new(vmgr).make_dir(root, "OBC2").expect("OBC2 is created");
            let obc2 = vmgr.open_dir(root, "OBC2").expect("OBC2 opens");
            Store { card, vmgr, geometry, obc2 }
        }

        fn fat(&self) -> Fat {
            Adapter::new(self.vmgr)
        }

        /// One 16,384-byte gated file at its full length — the state every rule below assumes.
        fn gated_file(&self, name: &str) -> GatedFile {
            let mut scratch = [0u8; 4_096];
            self.fat().create_fixed(self.obc2, name, SLOT_FILE_LEN as u32, &mut scratch).expect("created")
        }

        /// The regions the recorded spans landed in.
        fn regions(&self) -> Vec<Region> {
            self.card.with_spans(touched).iter().map(|&lba| self.geometry.region(lba)).collect()
        }
    }

    /// §1.1's decision over the card this suite runs against, so a geometry regression shows up here
    /// rather than as a mysterious alignment failure later.
    #[test]
    fn the_simulated_card_satisfies_both_geometry_preconditions() {
        let store = Store::mounted();
        assert_eq!(store.geometry.cluster_bytes, 16_384);
        assert_eq!(store.geometry.data_start_byte % 16_384, 0);
        assert_eq!(store.geometry.region(store.geometry.data_start_lba), Region::Data);
        assert_eq!(store.geometry.region(store.geometry.fs_info_lba.unwrap()), Region::FsInfo);
    }

    /// **The measurement, as a test.** A body write, a sync, a gate write and a sync on a
    /// fixed-length gated file must touch data sectors and nothing else: not FSInfo, not the
    /// directory sector holding `/OBC2`'s entries, not the FAT. This is the assumption OBC2's whole
    /// commit path rests on, and it is checked here in the shape the board bench checks on a card.
    #[test]
    fn syncing_a_fixed_length_gated_file_writes_no_metadata() {
        let store = Store::mounted();
        let fat = store.fat();
        let file = store.gated_file("ARM0.HND");

        store.card.arm();
        fat.write_at(file, 0, &[0xA5; SMALL_BODY_LEN]).expect("body");
        fat.sync_fixed(file, SLOT_FILE_LEN as u32).expect("clean flush");
        fat.write_gate(file, SMALL_GATE_OFFSET as u32, &[0x5A; 512]).expect("gate");
        fat.sync_fixed(file, SLOT_FILE_LEN as u32).expect("clean flush");
        store.card.disarm();

        assert_eq!(store.card.dropped(), 0, "the span log overflowed, so the assertion below proves nothing");
        let written = store.card.with_spans(touched);
        assert_eq!(written.len(), 2, "one sector per record write and no more: {written:?}");
        assert_eq!(store.regions(), [Region::Data, Region::Data]);
    }

    /// The falsification: the fork's own flush is what §13.1 rules out. Same file, same unchanged
    /// length, but through `sync_metadata` — and now FSInfo and the directory sector are rewritten.
    /// If this ever stops being true, the fork gained the primitive and `sync_fixed` can become a
    /// positive call instead of a documented omission.
    #[test]
    fn the_forks_flush_rewrites_fsinfo_and_the_directory_entry() {
        let store = Store::mounted();
        let fat = store.fat();
        let file = store.gated_file("ARM0.HND");

        store.card.arm();
        fat.write_at(file, 0, &[0xA5; SMALL_BODY_LEN]).expect("body");
        fat.sync_metadata(file).expect("the fork's flush");
        store.card.disarm();

        let regions = store.regions();
        assert!(regions.contains(&Region::FsInfo), "FSInfo was not rewritten: {regions:?}");
        // On FAT32 every directory is an ordinary cluster chain, so the entry rewrite is a second
        // data sector beside the record's own.
        assert!(
            regions.iter().filter(|region| **region == Region::Data).count() >= 2,
            "the directory entry was not rewritten: {regions:?}"
        );
    }

    /// §13.1 full-length initialization: preallocation is not length, and the length the adapter
    /// reports after `create_fixed` is the whole file — addressable to its last sector.
    #[test]
    fn a_created_file_is_at_its_full_length_and_addressable_to_its_last_sector() {
        let store = Store::mounted();
        let fat = store.fat();
        let file = store.gated_file("ARM0.HND");
        assert_eq!(fat.length(file).unwrap(), SLOT_FILE_LEN as u32);

        let last = SLOT_FILE_LEN as u32 - 512;
        fat.write_at(file, last, &[0x11; 512]).expect("the last sector is addressable");
        let mut back = [0u8; 512];
        fat.read_at(file, last, &mut back).unwrap();
        assert_eq!(back, [0x11; 512]);
        assert_eq!(fat.length(file).unwrap(), SLOT_FILE_LEN as u32, "an in-place write changed the length");
    }

    /// The zero-fill is real: a fresh gated file reads as zeros everywhere, which is what makes an
    /// absent gate an invalid gate rather than whatever the card happened to hold.
    #[test]
    fn a_created_file_reads_back_as_zeros() {
        let store = Store::mounted();
        let fat = store.fat();
        let file = store.gated_file("INIT.REC");
        let mut page = [0xFFu8; 512];
        for offset in (0..SLOT_FILE_LEN as u32).step_by(512) {
            fat.read_at(file, offset, &mut page).unwrap();
            assert!(page.iter().all(|&byte| byte == 0), "offset {offset} was not zero-filled");
        }
    }

    /// §13.1's seek bound, enforced on the *end* of the write rather than only its start: a write
    /// that would extend the file is refused before the medium is touched.
    #[test]
    fn a_write_past_the_recorded_length_is_refused() {
        let store = Store::mounted();
        let fat = store.fat();
        let file = store.gated_file("ARM0.HND");
        let len = SLOT_FILE_LEN as u32;

        store.card.arm();
        assert_eq!(fat.write_at(file, len, &[0u8; 512]), Err(AdapterError::OutOfRange));
        assert_eq!(fat.write_at(file, len - 256, &[0u8; 512]), Err(AdapterError::OutOfRange));
        assert_eq!(fat.read_at(file, len - 256, &mut [0u8; 512]), Err(AdapterError::OutOfRange));
        store.card.disarm();
        assert_eq!(store.card.counters().writes, 0, "a refused write reached the medium");
        assert_eq!(fat.length(file).unwrap(), len);
    }

    /// §13.1 gate isolation depends on the gate write replacing a whole sector, so a misaligned gate
    /// offset is refused rather than silently becoming a read-modify-write.
    #[test]
    fn a_misaligned_gate_write_is_refused() {
        let store = Store::mounted();
        let fat = store.fat();
        let file = store.gated_file("ARM0.HND");
        assert_eq!(fat.write_gate(file, 500, &[0u8; 512]), Err(AdapterError::Misaligned));
        assert_eq!(fat.write_gate(file, SMALL_GATE_OFFSET as u32, &[0u8; 512]), Ok(()));
    }

    /// A gate write touches exactly its own sector and leaves the body beside it alone — which is
    /// what "physically disjoint" buys, measured rather than asserted.
    #[test]
    fn a_gate_write_touches_exactly_its_own_sector() {
        let store = Store::mounted();
        let fat = store.fat();
        let file = store.gated_file("ARM0.HND");
        fat.write_at(file, 0, &[0xA5; SMALL_BODY_LEN]).unwrap();

        store.card.arm();
        fat.write_gate(file, SMALL_GATE_OFFSET as u32, &[0x5A; 512]).unwrap();
        store.card.disarm();
        assert_eq!(store.card.with_spans(touched).len(), 1);

        let mut body = [0u8; SMALL_BODY_LEN];
        fat.read_at(file, 0, &mut body).unwrap();
        assert_eq!(body, [0xA5; SMALL_BODY_LEN]);
    }

    /// §12's reuse rule: `make_dir` on a directory that is already present "is not an error and does
    /// not restart the order".
    #[test]
    fn make_dir_is_idempotent() {
        let store = Store::mounted();
        let fat = store.fat();
        assert_eq!(fat.make_dir(store.obc2, "GEN"), Ok(()));
        assert_eq!(fat.make_dir(store.obc2, "GEN"), Ok(()));
    }

    /// A short fixed file is refused at open rather than at the first unreachable slot offset — the
    /// state a cut during the zero-fill leaves behind, which §12 classifies as a pre-birth prefix.
    #[test]
    fn opening_a_short_fixed_file_fails_closed() {
        let store = Store::mounted();
        let fat = store.fat();
        let file = store.gated_file("RIDE.ACT");
        store.vmgr.close_file(file.raw()).unwrap();
        assert_eq!(
            fat.open_fixed(store.obc2, "RIDE.ACT", RIDE_FILE_LEN as u32),
            Err(AdapterError::LengthChanged { expected: 262_144, actual: SLOT_FILE_LEN as u32 })
        );
        assert!(fat.open_fixed(store.obc2, "RIDE.ACT", SLOT_FILE_LEN as u32).is_ok());
    }

    /// §6's two durability points stay apart: a body write that would reach its own gate sector is
    /// refused, and the gate offset itself is still writable as a gate.
    #[test]
    fn a_body_write_may_not_reach_its_gate() {
        let store = Store::mounted();
        let fat = store.fat();
        let file = store.gated_file("ARM0.HND");
        let gate = SMALL_GATE_OFFSET as u32;

        // One byte too long, and one byte too long from an earlier start: both reach the gate.
        assert_eq!(
            fat.write_body(file, 0, gate, &[0xA5; SMALL_BODY_LEN + 1]),
            Err(AdapterError::GateOverlap { body_end: gate + 1, gate })
        );
        assert_eq!(
            fat.write_body(file, 256, gate, &[0xA5; SMALL_BODY_LEN]),
            Err(AdapterError::GateOverlap { body_end: gate + 256, gate })
        );
        // Exactly up to the gate is the shape every record has.
        assert_eq!(fat.write_body(file, 0, gate, &[0xA5; SMALL_BODY_LEN]), Ok(()));
        // And a write that starts past the gate is a different sector's business, not this guard's.
        assert_eq!(fat.write_body(file, gate + 512, gate, &[0u8; 512]), Ok(()));
    }

    /// m9: a zero-length or ragged zero-fill granule is a mistake in this code, not a card fault.
    ///
    /// Zero loops forever; a granule that is not a whole number of sectors drops every write after
    /// the first onto the fork's read-modify-write path, which costs a read per block for the whole
    /// 4.6 MB initialization. Both are refused as [`AdapterError::CallerBug`] — and the
    /// `debug_assert` that comes with it is why this test runs in release only.
    #[test]
    #[cfg(not(debug_assertions))]
    fn a_ragged_zero_fill_granule_is_refused() {
        let store = Store::mounted();
        let fat = store.fat();
        let mut empty: [u8; 0] = [];
        assert_eq!(
            fat.create_fixed(store.obc2, "ARM0.HND", SLOT_FILE_LEN as u32, &mut empty),
            Err(AdapterError::CallerBug)
        );
        let mut ragged = [0u8; 700];
        assert_eq!(
            fat.create_fixed(store.obc2, "ARM0.HND", SLOT_FILE_LEN as u32, &mut ragged),
            Err(AdapterError::CallerBug)
        );
    }

    /// M3: the fork's errors reach §12 as the three classes its mount table needs.
    ///
    /// Checked through the public surface rather than by calling `map` directly, because what
    /// matters is that a store above this seam can tell a card that stopped answering from a volume
    /// that is not coherent — the difference between "the card is gone" and "mount recovery-failed,
    /// read-only, preserve the evidence".
    #[test]
    fn filesystem_failures_arrive_as_the_classes_section_12_classifies_on() {
        let store = Store::mounted();
        let fat = store.fat();
        // A name no FAT 8.3 encoder accepts.
        assert_eq!(
            fat.open_fixed(store.obc2, "not a valid 8.3 name", SLOT_FILE_LEN as u32),
            Err(AdapterError::BadName)
        );
        // A file that is not there.
        assert_eq!(fat.open_fixed(store.obc2, "ABSENT.BIN", SLOT_FILE_LEN as u32), Err(AdapterError::NotFound));
    }

    /// **`Media`**: the card stops answering mid-operation.
    ///
    /// §12 reports this differently from a store fault — the store may be perfectly intact on a card
    /// that is no longer readable — so it has to be produced, not asserted about.
    #[test]
    fn a_card_that_stops_answering_is_a_media_fault() {
        let store = Store::mounted();
        let fat = store.fat();
        let file = store.gated_file("ARM0.HND");

        store.card.device().start_failing();
        let mut buf = [0u8; 512];
        assert_eq!(fat.read_at(file, 0, &mut buf), Err(AdapterError::Media));
        assert_eq!(fat.write_at(file, 0, &[0xA5; 512]), Err(AdapterError::Media));

        // And the store is fine again the moment the card is: nothing about this was a store fault.
        store.card.device().stop_failing();
        assert_eq!(fat.write_at(file, 0, &[0xA5; 512]), Ok(()));
        assert_eq!(fat.read_at(file, 0, &mut buf), Ok(()));
        assert_eq!(buf, [0xA5; 512]);
    }

    /// **`CorruptStore`**: the FAT no longer describes where a file's bytes are.
    ///
    /// §1.1: losing a single-copy FAT structure "destroys file locations for the whole store: it is
    /// an unrecoverable store fault, not a gated-record fault". The card answers every request here
    /// — the volume is what is broken, and §12 mounts that recovery-failed and read-only rather than
    /// telling the rider their card is dying.
    #[test]
    fn a_broken_cluster_chain_is_a_store_fault_not_a_media_one() {
        let store = Store::mounted();
        let fat = store.fat();
        let mut scratch = [0u8; 4_096];
        // Sixteen clusters at this layout, so the chain has links to break.
        let file = fat.create_fixed(store.obc2, "RIDE.ACT", RIDE_FILE_LEN as u32, &mut scratch).expect("created");
        let mut buf = [0u8; 512];
        let far = RIDE_FILE_LEN as u32 - 512;
        assert_eq!(fat.read_at(file, far, &mut buf), Ok(()), "the last cluster is reachable to begin with");

        // Free every cluster from 4 up: the root directory and /OBC2 stay navigable, and every file
        // inside them now points into free space.
        fatsim::free_fat_entries_from(store.card.device(), Layout::default(), 4);
        // Read from the front so the handle's cached cluster cannot answer without walking the FAT —
        // the traversal is what meets the damage, and a cached position would hide it.
        assert_eq!(fat.read_at(file, 0, &mut buf), Ok(()), "the first cluster is in the directory entry");
        assert_eq!(fat.read_at(file, far, &mut buf), Err(AdapterError::CorruptStore));
    }

    /// **`CallerBug`**: a handle that is no longer a handle.
    ///
    /// Release-only, because the `debug_assert` at the point of translation is the intended debug
    /// behaviour — stopping at the mistake beats propagating a typed error nobody reads.
    #[test]
    #[cfg(not(debug_assertions))]
    fn a_stale_handle_is_a_caller_bug_and_never_a_mount_class() {
        let store = Store::mounted();
        let fat = store.fat();
        let file = store.gated_file("ARM0.HND");
        store.vmgr.close_file(file.raw()).unwrap();
        // The handle is dead; every use of it is this code's mistake and none of it is evidence
        // about the card or the volume.
        assert_eq!(fat.length(file), Err(AdapterError::CallerBug));
        assert_eq!(fat.read_at(file, 0, &mut [0u8; 512]), Err(AdapterError::CallerBug));
        assert_eq!(fat.sync_fixed(file, SLOT_FILE_LEN as u32), Err(AdapterError::CallerBug));
    }

    /// The fixed files of §3, created at their stated lengths and clean-flushable afterwards.
    #[test]
    fn the_fixed_files_of_section_3_are_created_at_their_stated_lengths() {
        let store = Store::mounted();
        let fat = store.fat();
        let mut scratch = [0u8; 4_096];
        for (name, len) in [("ARM0.HND", SLOT_FILE_LEN as u32), ("RIDE.ACT", RIDE_FILE_LEN as u32)] {
            let file = fat.create_fixed(store.obc2, name, len, &mut scratch).expect("created");
            assert_eq!(fat.length(file).unwrap(), len, "{name}");
            assert_eq!(fat.sync_fixed(file, len), Ok(()), "{name}");
            store.vmgr.close_file(file.raw()).unwrap();
        }
    }
}
