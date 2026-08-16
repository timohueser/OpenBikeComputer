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

use embedded_sdmmc::{BlockDevice, Mode, RawDirectory, RawFile, TimeSource, VolumeManager};

use super::limits::{GATE_LEN, SECTOR};

/// What a §13.1 adapter operation can fail with.
///
/// Every variant is a fact about the medium or about a caller's arithmetic; none of them is a
/// repair. `ShortWrite` and `LengthChanged` are the two §13.1 invents: they exist because the fork
/// reports a clamped write as success, so a length that is not exactly the intended one is the only
/// evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterError {
    /// The block device or the filesystem failed.
    Io,
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

fn map<E: core::fmt::Debug>(error: embedded_sdmmc::Error<E>) -> AdapterError {
    use embedded_sdmmc::Error as E;
    match error {
        E::NotFound => AdapterError::NotFound,
        E::FileAlreadyExists => AdapterError::AlreadyExists,
        E::DiskFull | E::NotEnoughSpace => AdapterError::Full,
        E::ReadOnly => AdapterError::ReadOnly,
        E::TooManyOpenFiles | E::TooManyOpenDirs | E::TooManyOpenVolumes => AdapterError::Handles,
        E::FilenameError(_) => AdapterError::BadName,
        E::InvalidOffset | E::EndOfFile => AdapterError::OutOfRange,
        _ => AdapterError::Io,
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
    pub fn volume_manager(&self) -> &'a VolumeManager<D, T, MAX_DIRS, MAX_FILES, MAX_VOLUMES> {
        self.vmgr
    }

    /// `make_dir` on a possibly already-present directory (§12: "not an error and does not restart
    /// the order").
    pub fn make_dir(&self, parent: RawDirectory, name: &str) -> Result<(), AdapterError> {
        match self.vmgr.make_dir_in_dir(parent, name) {
            Ok(()) => Ok(()),
            Err(embedded_sdmmc::Error::DirAlreadyExists) => Ok(()),
            Err(error) => Err(map(error)),
        }
    }

    /// The file's recorded length.
    pub fn length(&self, file: RawFile) -> Result<u32, AdapterError> {
        self.vmgr.file_length(file).map_err(map)
    }

    /// Reads `buf.len()` bytes at `offset`, bounded by the recorded length.
    ///
    /// One SD read returns at most a block, so this loops until the buffer is full; a zero-length
    /// read before then is EOF, which the bound check above should already have excluded and which
    /// is therefore reported as an I/O failure rather than as a short read.
    pub fn read_at(&self, file: RawFile, offset: u32, buf: &mut [u8]) -> Result<(), AdapterError> {
        let end = self.end_of(file, offset, buf.len())?;
        let _ = end;
        self.vmgr.file_seek_from_start(file, offset).map_err(map)?;
        let mut done = 0;
        while done < buf.len() {
            match self.vmgr.read(file, &mut buf[done..]) {
                Ok(0) => return Err(AdapterError::Io),
                Ok(read) => done += read,
                Err(error) => return Err(map(error)),
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
    pub fn write_at(&self, file: RawFile, offset: u32, bytes: &[u8]) -> Result<(), AdapterError> {
        let end = self.end_of(file, offset, bytes.len())?;
        let before = self.length(file)?;
        self.vmgr.file_seek_from_start(file, offset).map_err(map)?;
        self.vmgr.write(file, bytes).map_err(map)?;
        let reached = self.vmgr.file_offset(file).map_err(map)?;
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
    pub fn write_gate(&self, file: RawFile, offset: u32, gate: &[u8; GATE_LEN]) -> Result<(), AdapterError> {
        if !(offset as usize).is_multiple_of(SECTOR) {
            return Err(AdapterError::Misaligned);
        }
        self.write_at(file, offset, gate)
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
    pub fn sync_fixed(&self, file: RawFile, expected: u32) -> Result<(), AdapterError> {
        let actual = self.length(file)?;
        if actual != expected {
            return Err(AdapterError::LengthChanged { expected, actual });
        }
        Ok(())
    }

    /// The other sync: the one that *does* have metadata to persist.
    ///
    /// Only initialization and file creation reach it — the points where a recorded length actually
    /// changed and the directory entry has to catch up. It rewrites FSInfo and the directory entry,
    /// which is correct there and forbidden afterwards.
    pub fn sync_metadata(&self, file: RawFile) -> Result<(), AdapterError> {
        self.vmgr.flush_file(file).map_err(map)
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
    ) -> Result<RawFile, AdapterError> {
        if scratch.is_empty() {
            return Err(AdapterError::Misaligned);
        }
        scratch.fill(0);
        let file = self.vmgr.open_file_in_dir(parent, name, Mode::ReadWriteCreateOrTruncate).map_err(map)?;
        match self.fill(file, len, scratch) {
            Ok(()) => Ok(file),
            Err(error) => {
                // A failed creation leaves a short file, which §12's pre-birth rules classify and
                // repair; closing the handle is all this level can do about it.
                let _ = self.vmgr.close_file(file);
                Err(error)
            }
        }
    }

    fn fill(&self, file: RawFile, len: u32, scratch: &mut [u8]) -> Result<(), AdapterError> {
        let granule = u32::try_from(scratch.len()).map_err(|_| AdapterError::OutOfRange)?;
        // Returns short rather than failing when the volume is nearly full, so the length check
        // below is what actually reports the failure.
        self.vmgr.preallocate(file, len).map_err(map)?;
        self.vmgr.file_seek_from_start(file, 0).map_err(map)?;
        let mut written = 0u32;
        while written < len {
            let chunk = granule.min(len - written) as usize;
            self.vmgr.write(file, &scratch[..chunk]).map_err(map)?;
            let reached = self.vmgr.file_offset(file).map_err(map)?;
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
    pub fn open_fixed(&self, parent: RawDirectory, name: &str, len: u32) -> Result<RawFile, AdapterError> {
        let file = self.vmgr.open_file_in_dir(parent, name, Mode::ReadWriteAppend).map_err(map)?;
        let actual = self.length(file).unwrap_or(0);
        if actual != len {
            let _ = self.vmgr.close_file(file);
            return Err(AdapterError::LengthChanged { expected: len, actual });
        }
        Ok(file)
    }

    /// The end offset of a request, refused when it would pass the recorded length.
    fn end_of(&self, file: RawFile, offset: u32, count: usize) -> Result<u32, AdapterError> {
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
    use crate::obc2::fatsim::{fat32_card, geometry_sectors, touched, Layout, NullTime, SparseDisk};
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
        fn gated_file(&self, name: &str) -> RawFile {
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
        store.vmgr.close_file(file).unwrap();
        assert_eq!(
            fat.open_fixed(store.obc2, "RIDE.ACT", RIDE_FILE_LEN as u32),
            Err(AdapterError::LengthChanged { expected: 262_144, actual: SLOT_FILE_LEN as u32 })
        );
        assert!(fat.open_fixed(store.obc2, "RIDE.ACT", SLOT_FILE_LEN as u32).is_ok());
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
            store.vmgr.close_file(file).unwrap();
        }
    }
}
