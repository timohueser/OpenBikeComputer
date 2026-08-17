//! The board's OBC2 media: [`KernelMedia`] over a real FAT volume (`OBC2_Storage_Format.md` §3,
//! §7, §12, §13.1).
//!
//! [`card::Card`](super::card) implements the same two traits over a sector-addressed simulation so
//! the crash matrix can cut it anywhere. This is the other implementation — the one a device runs —
//! and it is deliberately in `obc-storage` rather than in the board crate: everything it does is
//! `no_std`, allocation-free and generic over [`BlockDevice`], so the same code that runs on the
//! card also runs against the FAT simulator in this crate's own tests. A board composes it; it does
//! not own it.
//!
//! ## What this is not
//!
//! It is **not** `CardStore` — that is [`super::store`], which composes this module's media with the
//! kernel, the repositories and the commit log. There is no admission lock, no session table, no
//! repository and no garbage-collector schedule *here*. What this is, is the layer directly beneath
//! them: the `/OBC2` tree's handles, §12's mount classification against a
//! real directory listing, §12's initialization order, and the eleven media operations
//! [`GenerationMedia`] and [`KernelMedia`] name. Nothing in this module is wired into the shipping
//! image; the board reaches it through `obc2_store_bench`.
//!
//! ## The handle budget is the shape of this module
//!
//! §13 gives the adapter **four directory handles and sixteen file handles**, and four is the exact
//! depth of a leaf: volume root, `OBC2`, role, shard. This module holds `/OBC2` and reaches a leaf
//! through a transient role/shard pair that is closed again before the call returns.
//!
//! The arithmetic, exactly, because it has **zero** headroom. With the volume root also held by the
//! composition — which is what the bench does — the peak is root + `/OBC2` + role + shard = **4 of
//! 4**. `make_dir_in_dir` refuses only when the table is already full and keeps no handle of its
//! own, and [`ensure_shards`](GenerationMedia::ensure_shards) calls it while three are held, so the
//! one free slot it needs is there. The budget is met and not one handle is spare.
//!
//! **That is a precondition on the cutover, not a property of this module.** Today `sd.rs` holds
//! root, `/routes` and `/tracks` open for the life of the mount. A composition that shared one
//! `VolumeManager` between v1 and OBC2 would reach four before `/OBC2` was even open, and the first
//! upload's `open_dir(OBC2, "GEN")` would fail `TooManyOpenDirs`. Either `SD_MAX_DIRS` rises or v1
//! releases its held handles before OBC2 goes live; this bench avoids the question by owning its own
//! volume manager.
//!
//! Files are not tight: permanently open are `COMMIT.JNL`, `CAT0.CHK`, `CAT1.CHK` and — while a
//! transaction is writing — one `GEN` payload and its `WORK` file. Five against sixteen, which is
//! the budget §13 records and the reason the mount limit for map files is eleven.
//!
//! ## Two syncs, and why they are different calls
//!
//! §13.1's clean flush forbids a gated file's sync from rewriting FSInfo or the directory entry, and
//! the fork's `flush_file` does both unconditionally. So the fixed `/OBC2` files sync through
//! [`Adapter::sync_fixed`], which writes nothing, and a **payload** — the one file in this format
//! whose recorded length legitimately changes — syncs through the fork's own flush, because
//! persisting a new length *is* a directory-entry write. The two paths are separate functions here
//! for the same reason [`GatedFile`] exists: so the wrong one cannot be reached by autocomplete.
//!
//! ## Lazy shards
//!
//! [`GenerationMedia::ensure_shards`] is the whole of the owner's 2026-08-16 lazy-shard decision:
//! two `make_dir` calls on possibly already-present directories, at admission, once per shard for
//! the life of the card. The eager tree cost 73.5 s of a 75 s first boot on the shipped media; this
//! costs about 140 ms the first time a shard is used and nothing afterwards.

use embedded_sdmmc::{BlockDevice, Mode, RawDirectory, RawFile, TimeSource};

use obc_link::ids::{GenerationId, StoreId};

use super::adapter::{classify, Adapter, AdapterError, GatedFile};
use super::checkpoint;
use super::gate::{Gate, INVALIDATED};
use super::generation::GenerationMedia;
use super::index::{self, RamIndex};
use super::init::InitRecord;
use super::journal::JournalBody;
use super::limits::{
    CHECKPOINT_FILE_LEN, CHECKPOINT_GATE_OFFSET, GATE_LEN, JOURNAL_BODY_LEN, JOURNAL_FILE_LEN, JOURNAL_GATE_OFFSET,
    JOURNAL_SLOTS, SECTOR, SLOT_FILE_LEN, SLOT_STRIDE, WORK_FILE_LEN,
};
use super::model::CatalogModel;
use super::mount::{self, Entry, EntryKind, MountClass, Outcome, StoreShape, CREATION_DIRECTORIES, CREATION_ORDER};
use super::names::{LeafName, Role};
use super::recovery::{self, CheckpointObservation, Decision, SlotObservation};
use super::transaction::KernelMedia;

/// The `/OBC2` directory name, at the volume root.
pub const ROOT_DIRECTORY: &str = "OBC2";
/// §12.1's sideload directory. Created by initialization; nothing in this slice reads it.
pub const IMPORT_DIRECTORY: &str = "IMPORT";

/// The two checkpoint files, in slot order.
const CHECKPOINT_NAMES: [&str; 2] = ["CAT0.CHK", "CAT1.CHK"];
/// The journal.
const JOURNAL_NAME: &str = "COMMIT.JNL";
/// §12's incomplete-initialization witness.
const WITNESS_NAME: &str = "INIT.REC";

/// The staging one journal slot, one `WORK` slot or one zero-fill granule needs: 16,384 bytes.
///
/// It is one program page, which is what makes a journal append a single write rather than a body
/// write followed by 29 sectors of padding.
pub type Stride = [u8; SLOT_STRIDE];

/// The 256 slot observations one all-slot journal scan produces.
///
/// 40 bytes each, so the array is 10,240 bytes and belongs in `.bss` on the board rather than on a
/// task frame.
pub type SlotTable = [Option<SlotObservation>; JOURNAL_SLOTS];

/// An empty observation table, `const` so it can initialize a `static`.
pub const NO_SLOTS: SlotTable = [None; JOURNAL_SLOTS];

/// What a survey of the card decided, and what it observed getting there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Survey {
    /// §12's classification.
    pub outcome: Outcome,
    /// The class §12's table reports for it.
    pub class: MountClass,
    /// How many of the 256 journal slots held a valid record for the selected epoch.
    pub valid_slots: usize,
    /// Which checkpoint files validated.
    pub checkpoints_valid: [bool; 2],
    /// The `StoreId` a valid `INIT.REC` witness carried, when one was there.
    pub witness: Option<StoreId>,
    /// How many `/OBC2` entries the listing pass saw, the skeleton directories excluded.
    pub entries: usize,
}

impl Survey {
    /// Whether the card holds a store this media can attach to.
    pub fn is_mountable(&self) -> bool {
        matches!(self.outcome, Outcome::Mount { .. })
    }

    /// Whether §12 wants initialization run before anything else.
    pub fn needs_initialization(&self) -> bool {
        matches!(
            self.outcome,
            Outcome::Initialize | Outcome::ResumeInitialization { .. } | Outcome::RestartPreBirth { .. }
        )
    }
}

/// One generation's two open files.
#[derive(Debug, Clone, Copy)]
struct Open {
    generation: GenerationId,
    /// The `GEN` payload: an ordinary growable file, §13.1's seek bound and write completeness only.
    payload: RawFile,
    /// The `WORK` file: fixed at 32,768 bytes, gated, and synced through the clean path.
    work: GatedFile,
}

/// The board's OBC2 media.
///
/// It holds the three permanently open gated files, the `/OBC2` directory handle, and the one
/// generation a transaction is writing. Everything else is opened, used and closed inside a call.
pub struct FatMedia<
    'a,
    D: BlockDevice,
    T: TimeSource,
    const MAX_DIRS: usize = 4,
    const MAX_FILES: usize = 16,
    const MAX_VOLUMES: usize = 1,
> {
    fat: Adapter<'a, D, T, MAX_DIRS, MAX_FILES, MAX_VOLUMES>,
    obc2: RawDirectory,
    journal: GatedFile,
    checkpoints: [GatedFile; 2],
    /// Which checkpoint file the last compaction wrote, so the next one alternates (§6.3).
    active_checkpoint: usize,
    stride: &'a mut Stride,
    open: Option<Open>,
    /// §11's free-space input. Seeded by the caller and maintained against payload growth; the
    /// authoritative one-time full-FAT scan §11 describes is not implemented in this slice.
    free: u64,
}

impl<'a, D: BlockDevice, T: TimeSource, const MAX_DIRS: usize, const MAX_FILES: usize, const MAX_VOLUMES: usize>
    FatMedia<'a, D, T, MAX_DIRS, MAX_FILES, MAX_VOLUMES>
{
    /// The adapter underneath, for a caller that needs the §13.1 primitives directly.
    pub fn adapter(&self) -> &Adapter<'a, D, T, MAX_DIRS, MAX_FILES, MAX_VOLUMES> {
        &self.fat
    }

    /// The `/OBC2` directory handle.
    pub fn obc2(&self) -> RawDirectory {
        self.obc2
    }

    /// Which checkpoint file §6.3 selected at mount.
    pub fn active_checkpoint(&self) -> usize {
        self.active_checkpoint
    }

    /// The free bytes §11's admission reserves against.
    pub fn free(&self) -> u64 {
        self.free
    }

    /// Closes every handle this media holds.
    ///
    /// This is the **one** place a gated `/OBC2` file may be closed: §13.1's law is that a close
    /// costs a directory-entry write, which is acceptable while the store is being torn down and at
    /// no other time.
    pub fn unmount(mut self) {
        self.close_open();
        let vmgr = self.fat.volume_manager();
        let _ = vmgr.close_file(self.journal.raw());
        for checkpoint in self.checkpoints {
            let _ = vmgr.close_file(checkpoint.raw());
        }
        let _ = vmgr.close_dir(self.obc2);
    }

    /// Materializes §6.3's selected projection into `model`: decode the checkpoint, replay the
    /// suffix.
    ///
    /// `survey` must be the one taken from this same card. The decision it carries — which
    /// checkpoint, how many leading slots — was made against these bytes, and replaying a stale
    /// count would apply records to the wrong base.
    pub fn load_index(&mut self, survey: &Survey, index: &mut RamIndex) -> Result<u64, AttachError> {
        let Outcome::Mount { checkpoint: selected, replay, .. } = survey.outcome else {
            return Err(AttachError::NotMountable(survey.class));
        };
        // §13: the checkpoint projection is card-resident, so the selected body is *streamed* into
        // the bounded index rather than staged and decoded. Nothing here holds more than `stride`.
        let mut source = CheckpointFile { fat: &self.fat, file: self.checkpoints[selected] };
        let header = index::load_checkpoint(index, &mut source, &mut self.stride[..]).map_err(|error| match error {
            checkpoint::StreamError::Media(error) => AttachError::Media(error),
            checkpoint::StreamError::Invalid(_) => AttachError::CheckpointUnreadable,
        })?;
        // §6.3's slot origin, taken before the suffix moves `through_sequence`.
        let epoch_base = header.through_sequence;
        // §6.3 chose this suffix because every record in it applies; one that does not means the
        // decision and the projection disagree, which is a finding rather than a repair.
        for slot in 0..replay {
            self.fat
                .read_at(self.journal, slot as u32 * SLOT_STRIDE as u32, &mut self.stride[..])
                .map_err(AttachError::Media)?;
            let body = JournalBody::validate_slot(&self.stride[..], slot as u16)
                .map_err(|_| AttachError::ReplayRejected(slot))?;
            index.absorb(&body).map_err(|_| AttachError::ReplayRejected(slot))?;
        }
        Ok(epoch_base)
    }

    /// Which checkpoint file §6.3's next compaction writes: the one that is not active.
    fn inactive_checkpoint(&self) -> usize {
        1 - self.active_checkpoint
    }

    /// Writes one gated checkpoint from a whole projection, in §6.3's order.
    ///
    /// Host-only, and a fixture rather than a path: it stages the 65,024-byte body, which is exactly
    /// what §13 forbids a mount or a compaction from doing. It exists so a test can *place* a
    /// checkpoint this store did not write — the degraded-bit and pair-alternation cases need one.
    /// The device's own path is [`KernelTransaction::compact`](super::transaction::KernelTransaction::compact),
    /// which streams.
    #[cfg(any(test, feature = "std"))]
    pub fn write_checkpoint(&mut self, index: usize, model: &CatalogModel) -> Result<(), AdapterError> {
        let mut body = std::boxed::Box::new([0u8; super::limits::CHECKPOINT_BODY_LEN]);
        model.encode_body(body.as_mut_slice()).map_err(|_| AdapterError::CallerBug)?;
        let file = self.checkpoints[index];
        let len = CHECKPOINT_FILE_LEN as u32;
        self.fat.write_gate(file, CHECKPOINT_GATE_OFFSET as u32, &INVALIDATED)?;
        self.fat.sync_fixed(file, len)?;
        self.fat.write_body(file, 0, CHECKPOINT_GATE_OFFSET as u32, body.as_slice())?;
        self.fat.sync_fixed(file, len)?;
        let gate = checkpoint::gate_for(body.as_slice(), index as u16);
        self.fat.write_gate(file, CHECKPOINT_GATE_OFFSET as u32, &gate.encode())?;
        self.fat.sync_fixed(file, len)?;
        self.active_checkpoint = index;
        Ok(())
    }

    // -- leaves ---------------------------------------------------------------------------------

    /// Opens `role`'s shard directory for `generation`, creating it when asked.
    ///
    /// The role directory is opened, used and closed inside the call, so the caller is left holding
    /// one handle rather than two. With the volume root held above this module the peak is exactly
    /// §13's four, and the `make_dir` below runs while three are held — which is the free slot
    /// `make_dir_in_dir` requires without keeping.
    fn open_shard(&self, role: Role, generation: GenerationId, create: bool) -> Result<RawDirectory, AdapterError> {
        let vmgr = self.fat.volume_manager();
        let shard = LeafName::of(generation).shard;
        let role_dir = vmgr.open_dir(self.obc2, role.directory()).map_err(classify)?;
        if create {
            if let Err(error) = self.fat.make_dir(role_dir, shard.as_str()) {
                let _ = vmgr.close_dir(role_dir);
                return Err(error);
            }
        }
        let shard_dir = vmgr.open_dir(role_dir, shard.as_str()).map_err(classify);
        let _ = vmgr.close_dir(role_dir);
        shard_dir
    }

    /// Opens this generation's `GEN` payload in `mode`.
    fn open_payload(&self, generation: GenerationId, mode: Mode) -> Result<RawFile, AdapterError> {
        let vmgr = self.fat.volume_manager();
        let shard = self.open_shard(Role::Gen, generation, false)?;
        let mut name = [0u8; 12];
        let leaf = LeafName::of(generation);
        let file = vmgr.open_file_in_dir(shard, leaf.write_8_3(&mut name), mode).map_err(classify);
        let _ = vmgr.close_dir(shard);
        file
    }

    /// Opens this generation's `WORK` file, creating it at its full 32,768 bytes if it is not there.
    ///
    /// §13.1 puts the `WORK` file's zero-fill at `BeginWork` rather than at initialization, which is
    /// exactly here: it is two strides, so it costs one cluster-sized write on this media rather
    /// than a share of a multi-second first boot.
    fn open_work(&mut self, generation: GenerationId) -> Result<GatedFile, AdapterError> {
        let shard = self.open_shard(Role::Work, generation, false)?;
        let mut name = [0u8; 12];
        let leaf = LeafName::of(generation);
        let name = leaf.write_8_3(&mut name);
        let opened = match self.fat.open_fixed(shard, name, WORK_FILE_LEN as u32) {
            Ok(file) => Ok(file),
            // Absent, or present but short — a cut during its own zero-fill. Either way the
            // recorded length is not one that can be slot-addressed, so it is written afresh.
            Err(AdapterError::NotFound) | Err(AdapterError::LengthChanged { .. }) => {
                let created = self.fat.create_fixed(shard, name, WORK_FILE_LEN as u32, &mut self.stride[..]);
                if created.is_ok() {
                    // The other half of what `collect_generation` credits back. A `WORK` file is
                    // 32,768 bytes of allocated space, so a free-space figure that ignored it here
                    // and returned it there would drift *up* on every generation.
                    self.free = self.free.saturating_sub(WORK_FILE_LEN as u64);
                }
                created
            }
            Err(error) => Err(error),
        };
        let _ = self.fat.volume_manager().close_dir(shard);
        opened
    }

    /// Closes whichever generation is open, if any.
    fn close_open(&mut self) {
        if let Some(open) = self.open.take() {
            let vmgr = self.fat.volume_manager();
            // The payload's length changes, so its close is the flush that records it — the one
            // file in this format for which that is correct.
            let _ = vmgr.close_file(open.payload);
            let _ = vmgr.close_file(open.work.raw());
        }
    }

    fn open_pair(&self) -> Result<Open, AdapterError> {
        self.open.ok_or(AdapterError::CallerBug)
    }

    /// Writes into a growable payload file, checking §13.1's write completeness afterwards.
    fn write_payload_at(&self, file: RawFile, offset: u64, bytes: &[u8]) -> Result<(), AdapterError> {
        let offset = u32::try_from(offset).map_err(|_| AdapterError::OutOfRange)?;
        let count = u32::try_from(bytes.len()).map_err(|_| AdapterError::OutOfRange)?;
        let end = offset.checked_add(count).ok_or(AdapterError::OutOfRange)?;
        let vmgr = self.fat.volume_manager();
        // §13.1's seek bound is the fork's: a start past the recorded length is refused rather than
        // extending. A start *at* it is an append, which is what a generation does.
        vmgr.file_seek_from_start(file, offset).map_err(classify)?;
        vmgr.write(file, bytes).map_err(classify)?;
        // "A short write is an error, never a success": the fork clamps and returns `Ok`, so the
        // resulting offset is the only evidence.
        let reached = vmgr.file_offset(file).map_err(classify)?;
        if reached != end {
            return Err(AdapterError::ShortWrite { wanted: end, reached });
        }
        Ok(())
    }
}

impl<'a, D: BlockDevice, T: TimeSource, const MAX_DIRS: usize, const MAX_FILES: usize, const MAX_VOLUMES: usize>
    GenerationMedia for FatMedia<'a, D, T, MAX_DIRS, MAX_FILES, MAX_VOLUMES>
{
    type Error = AdapterError;

    fn ensure_shards(&mut self, generation: GenerationId) -> Result<(), AdapterError> {
        // §12's reuse rule makes a second call free, so this is safe to place on every transaction's
        // path rather than behind a resident "have I done this" set that a reboot would lose.
        for role in Role::ALL {
            let shard = self.open_shard(role, generation, true)?;
            let _ = self.fat.volume_manager().close_dir(shard);
        }
        Ok(())
    }

    fn payload_length(&mut self) -> Result<u64, AdapterError> {
        let open = self.open_pair()?;
        self.fat.volume_manager().file_length(open.payload).map(u64::from).map_err(classify)
    }

    fn write_payload(&mut self, offset: u64, bytes: &[u8]) -> Result<(), AdapterError> {
        let open = self.open_pair()?;
        let before = u64::from(self.fat.volume_manager().file_length(open.payload).map_err(classify)?);
        self.write_payload_at(open.payload, offset, bytes)?;
        let after = u64::from(self.fat.volume_manager().file_length(open.payload).map_err(classify)?);
        self.free = self.free.saturating_sub(after.saturating_sub(before));
        Ok(())
    }

    fn sync_payload(&mut self) -> Result<(), AdapterError> {
        let open = self.open_pair()?;
        // The payload is the one OBC2 file whose recorded length legitimately changes, so its sync
        // is the fork's own flush: persisting a new length *is* a directory-entry write, and the
        // entry at risk is the shard's rather than the single-copy sector holding all of `/OBC2`.
        self.fat.volume_manager().flush_file(open.payload).map_err(classify)
    }

    fn truncate_payload(&mut self) -> Result<(), AdapterError> {
        let open = self.open_pair()?;
        let vmgr = self.fat.volume_manager();
        let length = u64::from(vmgr.file_length(open.payload).map_err(classify)?);
        // The fork has no truncate primitive on an open handle: `ReadWriteCreateOrTruncate` frees
        // the cluster chain, records length zero and writes the directory entry, which is exactly
        // §7's rewind, so the handle is closed and retaken in that mode.
        vmgr.close_file(open.payload).map_err(classify)?;
        // The handle is gone the instant `close_file` returns, so the resident record of it has to
        // go with it. Leaving `self.open` naming a closed file would make every later media call
        // report `CallerBug` — a real I/O failure misreported as this code's mistake, for the rest
        // of the mount, and a `debug_assert` in every debug build.
        self.open = None;
        let payload = self.open_payload(open.generation, Mode::ReadWriteCreateOrTruncate)?;
        self.open = Some(Open { payload, ..open });
        self.free = self.free.saturating_add(length);
        Ok(())
    }

    fn write_work(&mut self, offset: usize, bytes: &[u8]) -> Result<(), AdapterError> {
        let open = self.open_pair()?;
        let offset = u32::try_from(offset).map_err(|_| AdapterError::OutOfRange)?;
        if bytes.len() == GATE_LEN && (offset as usize).is_multiple_of(GATE_LEN) && offset % SLOT_STRIDE as u32 != 0 {
            // A 512-byte write at a sector-aligned offset that is not a slot base is a gate, and
            // §13.1's gate isolation is the property that makes invalidation all-or-nothing.
            return self.fat.write_gate(open.work, offset, bytes.try_into().expect("512 bytes"));
        }
        self.fat.write_at(open.work, offset, bytes)
    }

    fn sync_work(&mut self) -> Result<(), AdapterError> {
        let open = self.open_pair()?;
        // §13.1's clean flush: the `WORK` file's length never changes after its zero-fill, so this
        // writes no directory entry and no FSInfo.
        self.fat.sync_fixed(open.work, WORK_FILE_LEN as u32)
    }
}

/// A [`checkpoint::FileSource`] over one already-open gated checkpoint file.
///
/// This is what makes a mount stage nothing: the scan asks for spans and each one becomes a bounded
/// `read_at` straight off the card, so the largest buffer in the whole mount is whatever scratch the
/// caller lends it.
struct CheckpointFile<'f, D: BlockDevice, T: TimeSource, const A: usize, const B: usize, const C: usize> {
    fat: &'f Adapter<'f, D, T, A, B, C>,
    file: GatedFile,
}

impl<D: BlockDevice, T: TimeSource, const A: usize, const B: usize, const C: usize> checkpoint::FileSource
    for CheckpointFile<'_, D, T, A, B, C>
{
    type Error = AdapterError;

    fn read_span(&mut self, offset: usize, into: &mut [u8]) -> Result<(), AdapterError> {
        self.fat.read_at(self.file, offset as u32, into)
    }
}

impl<'a, D: BlockDevice, T: TimeSource, const MAX_DIRS: usize, const MAX_FILES: usize, const MAX_VOLUMES: usize>
    KernelMedia for FatMedia<'a, D, T, MAX_DIRS, MAX_FILES, MAX_VOLUMES>
{
    fn append_journal(
        &mut self,
        slot: u16,
        body: &[u8; JOURNAL_BODY_LEN],
        gate: &[u8; GATE_LEN],
    ) -> Result<(), AdapterError> {
        let base = slot as u32 * SLOT_STRIDE as u32;
        // §1's journal exemption: body-then-gate with no preceding invalidation, because every slot
        // of an earlier epoch is already inert against the selected checkpoint. The **whole stride**
        // is written with the gate sector zeroed — a slot that was torn once holds garbage across
        // its program page, and a reader rejects a nonzero pad.
        self.stride.fill(0);
        self.stride[..JOURNAL_BODY_LEN].copy_from_slice(body);
        self.fat.write_at(self.journal, base, &self.stride[..])?;
        self.fat.sync_fixed(self.journal, JOURNAL_FILE_LEN as u32)?;
        self.fat.write_gate(self.journal, base + JOURNAL_GATE_OFFSET as u32, gate)?;
        self.fat.sync_fixed(self.journal, JOURNAL_FILE_LEN as u32)
    }

    fn open_generation(&mut self, generation: GenerationId) -> Result<(), AdapterError> {
        if self.open.is_some_and(|open| open.generation == generation) {
            return Ok(());
        }
        self.close_open();
        let work = self.open_work(generation)?;
        match self.open_payload(generation, Mode::ReadWriteCreateOrAppend) {
            Ok(payload) => {
                self.open = Some(Open { generation, payload, work });
                Ok(())
            }
            Err(error) => {
                let _ = self.fat.volume_manager().close_file(work.raw());
                Err(error)
            }
        }
    }

    fn read_generation(
        &mut self,
        generation: GenerationId,
        offset: u64,
        into: &mut [u8],
    ) -> Result<usize, AdapterError> {
        let vmgr = self.fat.volume_manager();
        // The open generation is read through its own handle: the fork refuses a second open of a
        // file that is already open, and a reader that fell back to opening it would report a
        // caller bug on the one generation that is certainly there.
        let (file, borrowed) = match self.open {
            Some(open) if open.generation == generation => (open.payload, true),
            // §9: a generation nothing names is gone, and a read of one is a read of a file that is
            // not there rather than an empty read.
            _ => (self.open_payload(generation, Mode::ReadOnly)?, false),
        };
        let read = (|| {
            let length = vmgr.file_length(file).map_err(classify)?;
            let offset = u32::try_from(offset).map_err(|_| AdapterError::OutOfRange)?;
            let start = offset.min(length);
            let take = into.len().min((length - start) as usize);
            if take == 0 {
                return Ok(0);
            }
            vmgr.file_seek_from_start(file, start).map_err(classify)?;
            let mut done = 0;
            while done < take {
                match vmgr.read(file, &mut into[done..take]) {
                    // The bound above proved these bytes are inside the recorded length, so a
                    // zero-length read means the chain ends before the length says it does.
                    Ok(0) => return Err(AdapterError::CorruptStore),
                    Ok(count) => done += count,
                    Err(error) => return Err(classify(error)),
                }
            }
            Ok(done)
        })();
        if !borrowed {
            let _ = vmgr.close_file(file);
        } else if read.is_ok() {
            // A borrowed handle's offset is the writer's, and a read moved it. Put it back at the
            // end, which is where an appending writer expects to find it.
            let length = vmgr.file_length(file).unwrap_or(0);
            let _ = vmgr.file_seek_from_start(file, length);
        }
        read
    }

    fn collect_generation(&mut self, generation: GenerationId) -> Result<(), AdapterError> {
        if self.open.is_some_and(|open| open.generation == generation) {
            self.close_open();
        }
        let vmgr = self.fat.volume_manager();
        let mut name = [0u8; 12];
        let leaf = LeafName::of(generation);
        let name = leaf.write_8_3(&mut name);
        let mut collected = 0u64;
        for role in Role::ALL {
            // §9: "Deleting an unreachable GEN/WORK pair may be interrupted at either file; both
            // orderings recover as harmless orphan cleanup because no catalog fact points to it."
            let Ok(shard) = self.open_shard(role, generation, false) else { continue };
            // Both leaves are credited back, not just the payload: the `WORK` file is 32,768 bytes
            // of allocated space per generation, so accounting only the `GEN` half would let the
            // resident free-space figure drift down by that much on every collection.
            if let Ok(file) = vmgr.open_file_in_dir(shard, name, Mode::ReadOnly) {
                collected += u64::from(vmgr.file_length(file).unwrap_or(0));
                let _ = vmgr.close_file(file);
            }
            let _ = vmgr.delete_file_in_dir(shard, name);
            let _ = vmgr.close_dir(shard);
        }
        self.free = self.free.saturating_add(collected);
        Ok(())
    }

    fn free_bytes(&mut self) -> u64 {
        self.free
    }

    fn read_checkpoint(&mut self, offset: usize, into: &mut [u8]) -> Result<(), AdapterError> {
        self.fat.read_at(self.checkpoints[self.active_checkpoint], offset as u32, into)
    }

    fn read_record(&mut self, slot: u16) -> Result<Option<JournalBody>, AdapterError> {
        self.fat.read_at(self.journal, slot as u32 * SLOT_STRIDE as u32, &mut self.stride[..])?;
        Ok(JournalBody::validate_slot(&self.stride[..], slot).ok())
    }

    fn begin_checkpoint(&mut self) -> Result<(), AdapterError> {
        let file = self.checkpoints[self.inactive_checkpoint()];
        self.fat.write_gate(file, CHECKPOINT_GATE_OFFSET as u32, &INVALIDATED)?;
        self.fat.sync_fixed(file, CHECKPOINT_FILE_LEN as u32)
    }

    fn write_checkpoint_sector(&mut self, offset: usize, sector: &[u8; SECTOR]) -> Result<(), AdapterError> {
        let file = self.checkpoints[self.inactive_checkpoint()];
        self.fat.write_body(file, offset as u32, CHECKPOINT_GATE_OFFSET as u32, sector)
    }

    fn finish_checkpoint(&mut self, epoch: u64, through_sequence: u64, body_crc: u32) -> Result<(), AdapterError> {
        let index = self.inactive_checkpoint();
        let file = self.checkpoints[index];
        let len = CHECKPOINT_FILE_LEN as u32;
        self.fat.sync_fixed(file, len)?;
        let gate = Gate {
            magic: super::gate::MAGIC_CHECKPOINT,
            slot: index as u16,
            scope: epoch,
            sequence: through_sequence,
            body_crc,
        };
        self.fat.write_gate(file, CHECKPOINT_GATE_OFFSET as u32, &gate.encode())?;
        self.fat.sync_fixed(file, len)?;
        self.active_checkpoint = index;
        Ok(())
    }

    fn reset_store(&mut self, store: StoreId) -> Result<(), AdapterError> {
        // §16: reset destroys every object, result and lease. §12 defines it as file deletion,
        // never directory deletion, so the skeleton survives and the journal is blanked in place.
        //
        // The generation leaves are **not** enumerated here: walking 512 shards to delete orphans is
        // the incremental collector's pass, and §9 already makes an unreferenced leaf garbage rather
        // than a fault. What this does is make no catalog fact point at any of them.
        self.close_open();
        self.stride.fill(0);
        for slot in 0..JOURNAL_SLOTS as u32 {
            self.fat.write_at(self.journal, slot * SLOT_STRIDE as u32, &self.stride[..])?;
        }
        self.fat.sync_fixed(self.journal, JOURNAL_FILE_LEN as u32)?;
        // §12's initial projection, streamed through the stride: building a `CatalogModel` here to
        // encode one header and one repository row would cost 56 KiB of stack inside `execute`,
        // whose frame every command pays, and staging the body would cost 65,024 bytes of `.bss`.
        write_first_checkpoint(&self.fat, self.checkpoints[0], store, self.stride)?;
        self.active_checkpoint = 0;
        // The second checkpoint must not survive as an older store's valid gate.
        self.fat.write_gate(self.checkpoints[1], CHECKPOINT_GATE_OFFSET as u32, &INVALIDATED)?;
        self.fat.sync_fixed(self.checkpoints[1], CHECKPOINT_FILE_LEN as u32)
    }
}

// -------------------------------------------------------------------------------------------
// Survey, initialization and attach
// -------------------------------------------------------------------------------------------

/// One `/OBC2` entry, staged so [`mount::classify`] can borrow it as text.
///
/// §12 judges a pre-birth prefix over the FAT physical directory-entry order, so this keeps the
/// order the adapter enumerated and never sorts.
#[derive(Debug, Clone, Copy)]
struct Listed {
    name: [u8; 12],
    len_name: usize,
    kind: EntryKind,
    len: u32,
}

/// How many entries a listing keeps. §12's creation order is seven files; an eighth is already
/// enough to answer `Foreign`, so the ninth slot is the one that proves the listing overflowed.
const MAX_LISTED: usize = CREATION_ORDER.len() + 2;

/// Surveys the card: §1.1's verdict is the caller's, everything under `/OBC2` is this pass's.
///
/// It opens nothing it does not close, so a caller may run it and then decide to initialize, to
/// attach, or to do neither. `volume` is `Some` when §1.1 already refused the volume, which §12
/// decides "before `/OBC2` is looked for".
pub fn survey<
    D: BlockDevice,
    T: TimeSource,
    const MAX_DIRS: usize,
    const MAX_FILES: usize,
    const MAX_VOLUMES: usize,
>(
    fat: &Adapter<'_, D, T, MAX_DIRS, MAX_FILES, MAX_VOLUMES>,
    root: RawDirectory,
    volume: Option<super::geometry::Unsupported>,
    stride: &mut Stride,
    slots: &mut SlotTable,
) -> Survey {
    slots.fill(None);
    if let Some(reason) = volume {
        let outcome = mount::classify(Some(reason), None);
        return Survey {
            class: outcome.class(),
            outcome,
            valid_slots: 0,
            checkpoints_valid: [false; 2],
            witness: None,
            entries: 0,
        };
    }
    let vmgr = fat.volume_manager();
    let Ok(obc2) = vmgr.open_dir(root, ROOT_DIRECTORY) else {
        let outcome = mount::classify(None, None);
        return Survey {
            class: outcome.class(),
            outcome,
            valid_slots: 0,
            checkpoints_valid: [false; 2],
            witness: None,
            entries: 0,
        };
    };

    let mut listed = [Listed { name: [0; 12], len_name: 0, kind: EntryKind::File, len: 0 }; MAX_LISTED];
    let mut count = 0usize;
    let mut overflowed = false;
    let mut fat_intact = true;
    if let Err(error) = vmgr.iterate_dir(obc2, |entry| {
        let name = &entry.name;
        let base = name.base_name();
        let extension = name.extension();
        // `.` and `..` are the directory's own links, and the skeleton directories are exempt from
        // every shape judgement (§12 reuses a present empty directory rather than removing it).
        if base == b"." || base == b".." {
            return;
        }
        let is_dir = entry.attributes.is_directory();
        if is_dir && CREATION_DIRECTORIES.iter().any(|known| known.as_bytes() == base) {
            return;
        }
        if count == MAX_LISTED {
            overflowed = true;
            return;
        }
        let mut staged = Listed {
            name: [0; 12],
            len_name: 0,
            kind: if is_dir { EntryKind::Directory } else { EntryKind::File },
            len: entry.size,
        };
        let mut at = 0;
        for byte in base.iter().copied() {
            staged.name[at] = byte;
            at += 1;
        }
        if !extension.is_empty() {
            staged.name[at] = b'.';
            at += 1;
            for byte in extension.iter().copied() {
                staged.name[at] = byte;
                at += 1;
            }
        }
        staged.len_name = at;
        listed[count] = staged;
        count += 1;
    }) {
        // The directory could not be walked. That is a statement about the volume, not about the
        // store's records, and §12 mounts it recovery-failed rather than guessing a shape.
        if classify(error) == AdapterError::CorruptStore {
            fat_intact = false;
        }
    }

    // Every read below is gated on the listing having seen that name **as a file**. It is not
    // defensiveness: opening a directory as a file is `OpenedDirAsFile`, which the adapter classifies
    // as a caller bug and asserts on in a debug build — correctly, because asking for it is a
    // mistake in this code. §12 already says a directory where a fixed file belongs is a foreign
    // shape, so the survey must decide that from the listing rather than by trying the open.
    let is_file = |name: &str| {
        listed[..count].iter().any(|row| {
            row.kind == EntryKind::File
                && core::str::from_utf8(&row.name[..row.len_name]).is_ok_and(|text| text.eq_ignore_ascii_case(name))
        })
    };

    let witness = if is_file(WITNESS_NAME) { read_witness(fat, obc2, stride) } else { None };
    let mut checkpoints: [Option<CheckpointObservation>; 2] = [None, None];
    let mut checkpoints_valid = [false; 2];
    // §12's class 6, per checkpoint file, recorded while that file's bytes are still staged.
    let mut degraded = [false; 2];
    for (index, name) in CHECKPOINT_NAMES.iter().enumerate() {
        if !is_file(name) {
            continue;
        }
        let Ok(file) = fat.open_fixed(obc2, name, CHECKPOINT_FILE_LEN as u32) else { continue };
        // §13: no mount-time image. The scan asks for spans of at most one stride and each becomes a
        // bounded read, so a survey of both checkpoints stages `stride` and nothing else.
        let mut source = CheckpointFile { fat, file };
        let scanned = checkpoint::validate_file_streamed(&mut source, index as u16, &mut stride[..], &mut ());
        let _ = vmgr.close_file(file.raw());
        match scanned {
            Ok((header, body_crc)) => {
                checkpoints_valid[index] = true;
                // §5.2 byte 59 bit 0, captured per checkpoint rather than after the decision: §6.3's
                // choice is deterministic, so a store whose degraded bit lives in the file that is
                // *not* validated last would drop it at every mount, not just this one. It is a §12
                // class input rather than a §6.3 decision input, which is why it rides beside
                // `CheckpointObservation` instead of inside it.
                degraded[index] = header.flags & 1 != 0;
                checkpoints[index] = Some(CheckpointObservation {
                    store: header.store,
                    epoch: header.epoch,
                    through_sequence: header.through_sequence,
                    next_generation: header.next_generation,
                    // The scan proved the stored CRC equals the fresh one and hands it back, so
                    // re-deriving it would be a second sweep of 65,024 bytes.
                    body_crc,
                });
            }
            Err(checkpoint::StreamError::Media(AdapterError::CorruptStore)) => {
                fat_intact = false;
                continue;
            }
            Err(_) => continue,
        }
    }

    let mut valid_slots = 0usize;
    if let Some(Ok(journal)) =
        is_file(JOURNAL_NAME).then(|| fat.open_fixed(obc2, JOURNAL_NAME, JOURNAL_FILE_LEN as u32))
    {
        for (index, observation) in slots.iter_mut().enumerate() {
            // §6.3's all-slot scan, not a scan that stops at the first invalid slot: it is what
            // "turns any loss that does occur into a fail-closed mount rather than a silent
            // rollback".
            match fat.read_at(journal, index as u32 * SLOT_STRIDE as u32, &mut stride[..]) {
                Ok(()) => {}
                Err(AdapterError::CorruptStore) => {
                    fat_intact = false;
                    continue;
                }
                Err(_) => continue,
            }
            if let Ok(body) = JournalBody::validate_slot(&stride[..], index as u16) {
                *observation = Some(SlotObservation { store: body.store, epoch: body.epoch, sequence: body.sequence });
                valid_slots += 1;
            }
        }
        let _ = vmgr.close_file(journal.raw());
    }
    let _ = vmgr.close_dir(obc2);

    let decision = recovery::choose(&checkpoints, slots);
    let store_degraded = match decision {
        Decision::Mount { checkpoint, .. } | Decision::MountReadOnly { checkpoint, .. } => degraded[checkpoint],
        _ => false,
    };
    let entries: heapless::Vec<Entry<'_>, MAX_LISTED> = listed[..count]
        .iter()
        .map(|row| Entry {
            name: core::str::from_utf8(&row.name[..row.len_name]).unwrap_or("?"),
            kind: row.kind,
            len: row.len,
        })
        .collect();
    // An overflowed listing is not a short one: §12 fails closed on an unknown shape, and eight
    // OBC2-owned entries is already more than the creation order can be a prefix of.
    let shape = StoreShape {
        decision,
        witness,
        files: &entries,
        any_valid_gate: valid_slots > 0 || checkpoints_valid.iter().any(|valid| *valid),
        fat_intact,
        store_degraded,
    };
    let outcome = if overflowed {
        Outcome::RecoveryFailed(mount::Fault::UnknownShape)
    } else {
        mount::classify(None, Some(shape))
    };
    Survey { class: outcome.class(), outcome, valid_slots, checkpoints_valid, witness, entries: count }
}

/// Reads and validates `INIT.REC`, §12's incomplete-initialization witness.
fn read_witness<D: BlockDevice, T: TimeSource, const A: usize, const B: usize, const C: usize>(
    fat: &Adapter<'_, D, T, A, B, C>,
    obc2: RawDirectory,
    stride: &mut Stride,
) -> Option<StoreId> {
    let file = fat.open_fixed(obc2, WITNESS_NAME, SLOT_FILE_LEN as u32).ok()?;
    let read = fat.read_at(file, 0, &mut stride[..]);
    let _ = fat.volume_manager().close_file(file.raw());
    read.ok()?;
    InitRecord::validate_slot(&stride[..]).ok().map(|record| record.store)
}

/// How long each stage of an initialization took, for the caller to report.
///
/// Every field is a byte count rather than a duration: this module owns no clock, and a host test
/// and a board bench want different ones. The board times the stages around these calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Initialized {
    /// Bytes of fixed metadata files zero-filled (§13.1's 4,636,672 at these capacities).
    pub zero_filled: u64,
    /// Directories created: `OBC2`, `GEN`, `WORK`, `IMPORT`. Four, not 516 — shards are lazy.
    pub directories: usize,
}

/// Runs §12's initialization: the witness, the skeleton, the seven fixed files, the first
/// checkpoint, and then the witness's deletion.
///
/// The order is §12's and the last two steps are the ones that matter. The first checkpoint's gate
/// is the StoreId birth point, and `INIT.REC` is removed only once that gate is durable — so a cut
/// anywhere before it leaves a witness that says "this unadvertised StoreId owns the preallocation
/// prefix on this card", which is what lets the next mount resume rather than restart with a new
/// identity.
pub fn initialize<D: BlockDevice, T: TimeSource, const A: usize, const B: usize, const C: usize>(
    fat: &Adapter<'_, D, T, A, B, C>,
    root: RawDirectory,
    store: StoreId,
    stride: &mut Stride,
) -> Result<Initialized, AdapterError> {
    let vmgr = fat.volume_manager();
    fat.make_dir(root, ROOT_DIRECTORY)?;
    let obc2 = vmgr.open_dir(root, ROOT_DIRECTORY).map_err(classify)?;
    let outcome = initialize_in(fat, obc2, store, stride);
    let _ = vmgr.close_dir(obc2);
    outcome
}

fn initialize_in<D: BlockDevice, T: TimeSource, const A: usize, const B: usize, const C: usize>(
    fat: &Adapter<'_, D, T, A, B, C>,
    obc2: RawDirectory,
    store: StoreId,
    stride: &mut Stride,
) -> Result<Initialized, AdapterError> {
    let vmgr = fat.volume_manager();
    let mut report = Initialized { zero_filled: 0, directories: 1 };

    // Any earlier attempt's files are removed first. §12's restart verdict authorizes exactly this
    // — an ungated pre-birth prefix is deleted, never adopted — and a resumed initialization
    // reaching here has already had its witness's StoreId handed back to the caller.
    for file in CREATION_ORDER {
        let _ = vmgr.delete_file_in_dir(obc2, file.name);
    }

    // §12 writes the witness "before it creates anything else that could outlive a cut".
    InitRecord { store }.encode_slot_into(&mut stride[..]).map_err(|_| AdapterError::CallerBug)?;
    let body: [u8; 512] = stride[..512].try_into().expect("512 bytes");
    let gate: [u8; GATE_LEN] = stride[512..1_024].try_into().expect("512 bytes");
    // The zero-fill buffer is the stride itself, taken after the record has been lifted out of it.
    stride.fill(0);
    let witness = fat.create_fixed(obc2, WITNESS_NAME, SLOT_FILE_LEN as u32, &mut stride[..])?;
    report.zero_filled += SLOT_FILE_LEN as u64;
    fat.write_body(witness, 0, 512, &body)?;
    fat.sync_fixed(witness, SLOT_FILE_LEN as u32)?;
    fat.write_gate(witness, 512, &gate)?;
    fat.sync_fixed(witness, SLOT_FILE_LEN as u32)?;
    let _ = vmgr.close_file(witness.raw());

    // The role trees and the sideload directory — four directories, and not one shard.
    for name in [Role::Gen.directory(), Role::Work.directory(), IMPORT_DIRECTORY] {
        fat.make_dir(obc2, name)?;
        report.directories += 1;
    }

    // The remaining six fixed files of §12's creation order, each at its full length in zeros.
    for file in CREATION_ORDER.iter().skip(1) {
        let created = fat.create_fixed(obc2, file.name, file.len, &mut stride[..])?;
        report.zero_filled += u64::from(file.len);
        let _ = vmgr.close_file(created.raw());
    }

    // The first checkpoint: §12's initial projection, gated. This is the StoreId birth point.
    let checkpoint = fat.open_fixed(obc2, CHECKPOINT_NAMES[0], CHECKPOINT_FILE_LEN as u32)?;
    let written = write_first_checkpoint(fat, checkpoint, store, stride);
    let _ = vmgr.close_file(checkpoint.raw());
    written?;

    // Only now: the witness has done its job.
    vmgr.delete_file_in_dir(obc2, WITNESS_NAME).map_err(classify)?;
    Ok(report)
}

/// Writes §12's first checkpoint in §6.3's order — invalidate, body, sync, gate, sync — with the
/// body streamed through `stride` rather than staged whole.
fn write_first_checkpoint<D: BlockDevice, T: TimeSource, const A: usize, const B: usize, const C: usize>(
    fat: &Adapter<'_, D, T, A, B, C>,
    file: GatedFile,
    store: StoreId,
    stride: &mut Stride,
) -> Result<(), AdapterError> {
    let len = CHECKPOINT_FILE_LEN as u32;
    // A body written under a still-valid gate would be a record the instant its first sector landed.
    fat.write_gate(file, CHECKPOINT_GATE_OFFSET as u32, &INVALIDATED)?;
    fat.sync_fixed(file, len)?;
    let crc = CatalogModel::stream_initial_body(
        store,
        obc_link::registry::ObjectKind::Weather.to_u16(),
        &mut stride[..],
        |offset, bytes| fat.write_body(file, offset as u32, CHECKPOINT_GATE_OFFSET as u32, bytes),
    )?;
    fat.sync_fixed(file, len)?;
    let gate = Gate { magic: super::gate::MAGIC_CHECKPOINT, slot: 0, scope: 1, sequence: 0, body_crc: crc };
    fat.write_gate(file, CHECKPOINT_GATE_OFFSET as u32, &gate.encode())?;
    fat.sync_fixed(file, len)
}

/// Why an attach did not produce a media.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachError {
    /// §12 did not classify this card as mountable.
    NotMountable(MountClass),
    /// The medium or the volume refused an operation the attach needed.
    Media(AdapterError),
    /// The selected checkpoint validated during the survey and did not decode now, which means the
    /// two passes disagree about the same bytes.
    CheckpointUnreadable,
    /// A record §6.3 chose to replay did not apply to the projection it chose it against.
    ReplayRejected(usize),
}

/// Opens the three permanently held gated files. It reads no catalog: the projection is loaded
/// afterwards, by [`FatMedia::load_projection`], into storage the caller already owns.
///
/// The split is not stylistic. A board places its transaction — projection included — in `.bss`, and
/// the projection has to be decoded *through this media* into *that* transaction's own field; an
/// attach that produced the projection as an out-parameter would put a 56 KiB copy between the two.
pub fn attach<'a, D: BlockDevice, T: TimeSource, const A: usize, const B: usize, const C: usize>(
    fat: Adapter<'a, D, T, A, B, C>,
    root: RawDirectory,
    survey: &Survey,
    stride: &'a mut Stride,
    free_bytes: u64,
) -> Result<FatMedia<'a, D, T, A, B, C>, AttachError> {
    let Outcome::Mount { checkpoint: selected, .. } = survey.outcome else {
        return Err(AttachError::NotMountable(survey.class));
    };
    let vmgr = fat.volume_manager();
    let obc2 = vmgr.open_dir(root, ROOT_DIRECTORY).map_err(|error| AttachError::Media(classify(error)))?;
    let opened = (|| {
        let journal = fat.open_fixed(obc2, JOURNAL_NAME, JOURNAL_FILE_LEN as u32)?;
        let cat0 = fat.open_fixed(obc2, CHECKPOINT_NAMES[0], CHECKPOINT_FILE_LEN as u32)?;
        let cat1 = match fat.open_fixed(obc2, CHECKPOINT_NAMES[1], CHECKPOINT_FILE_LEN as u32) {
            Ok(file) => file,
            Err(error) => {
                let _ = vmgr.close_file(journal.raw());
                let _ = vmgr.close_file(cat0.raw());
                return Err(error);
            }
        };
        Ok((journal, [cat0, cat1]))
    })();
    let (journal, checkpoints) = match opened {
        Ok(handles) => handles,
        Err(error) => {
            let _ = vmgr.close_dir(obc2);
            return Err(AttachError::Media(error));
        }
    };

    Ok(FatMedia { fat, obc2, journal, checkpoints, active_checkpoint: selected, stride, open: None, free: free_bytes })
}

#[cfg(test)]
mod tests {
    use std::boxed::Box;

    use embedded_sdmmc::{VolumeIdx, VolumeManager};
    use obc_link::engine::{
        ClaimIntent, Command, OperationReport, Outcome as EngineOutcome, PrincipalScope, Transaction,
    };
    use obc_link::frame::Opcode;
    use obc_link::ids::{LogicalObjectId, OperationId};
    use obc_link::registry::ObjectKind;
    use obc_link::upload::Target;

    use super::*;
    use crate::fat_extents::SharedBlockDevice;
    use crate::obc2::blocklog::WriteLog;
    use crate::obc2::fatsim::{fat32_card, geometry_sectors, touched, Layout, NullTime, SparseDisk};
    use crate::obc2::geometry::{self, Region};
    use crate::obc2::transaction::{AcceptEverything, KernelTransaction, NoHooks};

    type Vmgr = VolumeManager<SharedBlockDevice<'static, SparseDisk>, NullTime, 4, 16, 1>;
    type Fat = Adapter<'static, SharedBlockDevice<'static, SparseDisk>, NullTime, 4, 16, 1>;
    type Media = FatMedia<'static, SharedBlockDevice<'static, SparseDisk>, NullTime, 4, 16, 1>;

    const STORE: StoreId = StoreId::new([0x0B; 16]);
    const PRINCIPAL: PrincipalScope = PrincipalScope::new([0x21; 16]);

    /// A mounted, empty FAT32 card with the handles a survey needs.
    struct Card {
        vmgr: &'static Vmgr,
        root: RawDirectory,
    }

    impl Card {
        fn blank() -> Card {
            let layout = Layout::default();
            let disk: &'static SparseDisk = Box::leak(Box::new(fat32_card(layout)));
            let vmgr: &'static Vmgr =
                Box::leak(Box::new(VolumeManager::new_with_limits(SharedBlockDevice(disk), NullTime, 9_000)));
            let (mbr, bpb) = geometry_sectors(disk, layout.partition_start_lba);
            geometry::admit(&mbr, &bpb, 0).expect("the simulated card is conforming");
            let volume = vmgr.open_raw_volume(VolumeIdx(0)).expect("the simulated card mounts");
            let root = vmgr.open_root_dir(volume).expect("a root directory");
            Card { vmgr, root }
        }

        fn fat(&self) -> Fat {
            Adapter::new(self.vmgr)
        }
    }

    /// The two staging buffers, leaked so a media can hold them for `'static`.
    ///
    /// Two, not three: §13's mount image is gone, and the stride is now the largest thing a whole
    /// mount touches.
    fn buffers() -> (&'static mut Stride, Box<SlotTable>) {
        (Box::leak(Box::new([0u8; SLOT_STRIDE])), Box::new(NO_SLOTS))
    }

    /// An initialized card with a mounted media, its resident index, and §6.3's slot origin.
    fn mounted() -> (Card, Media, Box<RamIndex>, u64) {
        let card = Card::blank();
        let (stride, mut slots) = buffers();
        let fat = card.fat();
        let survey = super::survey(&fat, card.root, None, stride, &mut slots);
        assert_eq!(survey.outcome, Outcome::Initialize, "a blank card is not a fresh card");
        initialize(&fat, card.root, STORE, stride).expect("initialization");
        let survey = super::survey(&fat, card.root, None, stride, &mut slots);
        assert!(survey.is_mountable(), "an initialized card did not mount: {survey:?}");
        let mut index = Box::new(RamIndex::new(STORE));
        let mut media = attach(fat, card.root, &survey, stride, 8 * 1024 * 1024).expect("attach");
        let epoch_base = media.load_index(&survey, &mut index).expect("projection");
        (card, media, index, epoch_base)
    }

    fn transaction(
        media: Media,
        index: &RamIndex,
        epoch_base: u64,
    ) -> Box<KernelTransaction<Media, AcceptEverything, NoHooks>> {
        Box::new(KernelTransaction::mount(media, AcceptEverything, NoHooks, index.clone(), epoch_base))
    }

    fn intent(operation: OperationId, length: u64, crc: u32) -> ClaimIntent {
        ClaimIntent {
            operation_id: operation,
            principal: PRINCIPAL,
            opcode: Opcode::StartUpload,
            digest: [0x77; 32],
            kind: ObjectKind::Route,
            target: Target::Create,
            declared_length: length,
            expected_crc: crc,
            metadata: obc_link::engine::IntentMetadata::NONE,
            target_operation_id: None,
        }
    }

    /// §12's initialization, end to end: the seven fixed files at their lengths, four directories,
    /// no shards, no witness, and a card that mounts afterwards.
    #[test]
    fn a_blank_card_initializes_and_then_mounts() {
        let card = Card::blank();
        let (stride, mut slots) = buffers();
        let fat = card.fat();

        let report = initialize(&fat, card.root, STORE, stride).expect("initialization");
        assert_eq!(report.directories, 4, "the shard tree was created eagerly");
        // §13.1's figure: the seven fixed files, the witness included, at their stated lengths.
        assert_eq!(report.zero_filled, crate::obc2::limits::INITIALIZATION_ZERO_FILL as u64);

        let survey = super::survey(&fat, card.root, None, stride, &mut slots);
        assert_eq!(survey.class, MountClass::Mounted);
        assert_eq!(survey.checkpoints_valid, [true, false], "the pair did not start on CAT0");
        assert_eq!(survey.valid_slots, 0, "a fresh journal holds records");
        assert_eq!(survey.witness, None, "the witness outlived the first checkpoint gate");
        assert_eq!(survey.entries, 6, "the /OBC2 listing is the six surviving fixed files");
    }

    /// §12's fresh-card class, and the one below it: a card with no `/OBC2` at all initializes, and
    /// so does one whose `/OBC2` holds nothing.
    #[test]
    fn a_card_with_no_store_is_classified_initialize() {
        let card = Card::blank();
        let (stride, mut slots) = buffers();
        let fat = card.fat();
        assert_eq!(super::survey(&fat, card.root, None, stride, &mut slots).outcome, Outcome::Initialize);

        fat.make_dir(card.root, ROOT_DIRECTORY).expect("OBC2");
        assert_eq!(super::survey(&fat, card.root, None, stride, &mut slots).outcome, Outcome::Initialize);
    }

    /// §1.1 is decided before `/OBC2` is looked for, so a refused volume is never surveyed.
    #[test]
    fn an_unsupported_volume_is_refused_without_reading_the_store() {
        let card = Card::blank();
        let (stride, mut slots) = buffers();
        let fat = card.fat();
        initialize(&fat, card.root, STORE, stride).expect("initialization");
        let survey = super::survey(
            &fat,
            card.root,
            Some(geometry::Unsupported::ClusterNotWholePages(24_576)),
            stride,
            &mut slots,
        );
        assert_eq!(survey.class, MountClass::UnsupportedFilesystem);
        assert!(!survey.outcome.admits_mutation());
    }

    /// §12's pre-birth restart: an ungated prefix of the creation order with no witness is deleted
    /// and initialization starts over.
    #[test]
    fn an_ungated_pre_birth_prefix_restarts() {
        let card = Card::blank();
        let (stride, mut slots) = buffers();
        let fat = card.fat();
        fat.make_dir(card.root, ROOT_DIRECTORY).expect("OBC2");
        let obc2 = card.vmgr.open_dir(card.root, ROOT_DIRECTORY).expect("OBC2 opens");
        // The first two files of the creation order and nothing else: a cut during initialization.
        for file in CREATION_ORDER.iter().take(2) {
            let created = fat.create_fixed(obc2, file.name, file.len, &mut stride[..]).expect("created");
            card.vmgr.close_file(created.raw()).expect("close");
        }
        card.vmgr.close_dir(obc2).expect("close");

        let survey = super::survey(&fat, card.root, None, stride, &mut slots);
        assert_eq!(survey.outcome, Outcome::RestartPreBirth { files: 2 });
        assert!(survey.needs_initialization());
    }

    /// The witness is what makes a cut mid-initialization resumable rather than a new identity.
    #[test]
    fn a_valid_witness_resumes_the_same_store_id() {
        let card = Card::blank();
        let (stride, mut slots) = buffers();
        let fat = card.fat();
        fat.make_dir(card.root, ROOT_DIRECTORY).expect("OBC2");
        let obc2 = card.vmgr.open_dir(card.root, ROOT_DIRECTORY).expect("OBC2 opens");
        InitRecord { store: STORE }.encode_slot_into(&mut stride[..]).expect("a stride");
        let body: [u8; 512] = stride[..512].try_into().unwrap();
        let gate: [u8; GATE_LEN] = stride[512..1_024].try_into().unwrap();
        stride.fill(0);
        let witness = fat.create_fixed(obc2, WITNESS_NAME, SLOT_FILE_LEN as u32, &mut stride[..]).expect("witness");
        fat.write_body(witness, 0, 512, &body).expect("body");
        fat.write_gate(witness, 512, &gate).expect("gate");
        card.vmgr.close_file(witness.raw()).expect("close");
        card.vmgr.close_dir(obc2).expect("close");

        let survey = super::survey(&fat, card.root, None, stride, &mut slots);
        assert_eq!(survey.outcome, Outcome::ResumeInitialization { store: STORE });
        assert_eq!(survey.witness, Some(STORE));
    }

    /// A directory where a fixed file belongs is a foreign shape, not a short file — and the
    /// difference matters because the pre-birth verdict authorizes deletion.
    #[test]
    fn a_directory_named_like_a_fixed_file_fails_closed() {
        let card = Card::blank();
        let (stride, mut slots) = buffers();
        let fat = card.fat();
        fat.make_dir(card.root, ROOT_DIRECTORY).expect("OBC2");
        let obc2 = card.vmgr.open_dir(card.root, ROOT_DIRECTORY).expect("OBC2 opens");
        fat.make_dir(obc2, "INIT.REC").expect("a directory in the file's place");
        card.vmgr.close_dir(obc2).expect("close");

        let survey = super::survey(&fat, card.root, None, stride, &mut slots);
        assert_eq!(survey.class, MountClass::RecoveryFailed);
        assert!(!survey.outcome.admits_mutation());
    }

    /// §3's lazy shards: nothing exists under `GEN`/`WORK` until a generation lands there, and the
    /// second call for the same shard is free.
    #[test]
    fn shards_are_created_on_first_use_and_are_idempotent() {
        let (card, mut media, _index, _base) = mounted();
        let generation = GenerationId::new(0x1234);
        let shard = LeafName::of(generation).shard;

        let gen_dir = card.vmgr.open_dir(media.obc2(), "GEN").expect("GEN opens");
        assert!(card.vmgr.open_dir(gen_dir, shard.as_str()).is_err(), "the shard existed before its first use");
        card.vmgr.close_dir(gen_dir).expect("close");

        media.ensure_shards(generation).expect("first use");
        media.ensure_shards(generation).expect("second use is free");
        for role in Role::ALL {
            let role_dir = card.vmgr.open_dir(media.obc2(), role.directory()).expect("role opens");
            let shard_dir = card.vmgr.open_dir(role_dir, shard.as_str()).expect("the shard was created");
            card.vmgr.close_dir(shard_dir).expect("close");
            card.vmgr.close_dir(role_dir).expect("close");
        }
    }

    /// The generation seam, end to end on a real filesystem: create, append, read back, truncate.
    #[test]
    fn a_generation_writes_reads_and_rewinds() {
        let (_card, mut media, _index, _base) = mounted();
        let generation = GenerationId::new(0x99);
        media.ensure_shards(generation).expect("shards");
        media.open_generation(generation).expect("open");

        assert_eq!(media.payload_length().unwrap(), 0);
        media.write_payload(0, &[0xA5; 1_000]).expect("append");
        media.sync_payload().expect("sync");
        assert_eq!(media.payload_length().unwrap(), 1_000);
        media.write_payload(1_000, &[0x5A; 24]).expect("append");
        media.sync_payload().expect("sync");
        assert_eq!(media.payload_length().unwrap(), 1_024);

        let mut back = [0u8; 1_024];
        assert_eq!(media.read_generation(generation, 0, &mut back).unwrap(), 1_024);
        assert!(back[..1_000].iter().all(|byte| *byte == 0xA5));
        assert!(back[1_000..].iter().all(|byte| *byte == 0x5A));

        // §7's rewind: the payload goes back to zero and the writer starts again.
        media.truncate_payload().expect("truncate");
        media.sync_payload().expect("sync");
        assert_eq!(media.payload_length().unwrap(), 0);
        media.write_payload(0, &[0x11; 16]).expect("append after the rewind");
        media.sync_payload().expect("sync");
        assert_eq!(media.payload_length().unwrap(), 16);
    }

    /// §9: a generation nothing names is gone, and a read of one is not an empty read.
    #[test]
    fn collecting_a_generation_removes_both_files() {
        let (_card, mut media, _index, _base) = mounted();
        let generation = GenerationId::new(0x4242);
        media.ensure_shards(generation).expect("shards");
        media.open_generation(generation).expect("open");
        media.write_payload(0, &[0x33; 512]).expect("append");
        media.sync_payload().expect("sync");
        media.write_work(0, &[0x44; 512]).expect("work body");
        media.sync_work().expect("work sync");

        media.collect_generation(generation).expect("collect");
        assert_eq!(media.read_generation(generation, 0, &mut [0u8; 16]), Err(AdapterError::NotFound));
        // And collecting one that was never created is not an error.
        media.collect_generation(GenerationId::new(0x777)).expect("already collected");
    }

    /// The journal append, in the shape §1 fixes: the whole stride with the gate zeroed, then the
    /// gate. What the test proves is that the record reads back as a record.
    #[test]
    fn a_journal_record_survives_its_append() {
        let (card, mut media, index, _base) = mounted();
        let body = crate::obc2::samples::retention_remove(index.epoch, 1, 3, 7);
        let encoded = body.encode_body();
        let gate = body.gate_for(&encoded).encode();
        media.append_journal(3, &encoded, &gate).expect("append");
        media.unmount();

        let (stride, mut slots) = buffers();
        let fat = card.fat();
        let survey = super::survey(&fat, card.root, None, stride, &mut slots);
        assert_eq!(survey.valid_slots, 1, "the appended record did not validate: {survey:?}");
        assert!(survey.is_mountable());
        // The record is at the physical slot it named, and nowhere else.
        assert!(slots[3].is_some() && slots[..3].iter().all(|slot| slot.is_none()));
    }

    /// §6.3's alternating pair, which the media bench could not cover because it writes no
    /// checkpoint at all: a second checkpoint written into the *other* file is the one the next
    /// mount selects, on the strength of its greater through-sequence.
    #[test]
    fn the_checkpoint_pair_alternates_and_the_newer_one_is_selected() {
        let (card, mut media, _index, _base) = mounted();
        // The oracle projection initialization wrote, which is what a *placed* checkpoint is made
        // from — the store's own path streams, and this test needs to put bytes there by hand.
        let mut model = CatalogModel::initial(STORE, ObjectKind::Weather.to_u16());
        assert_eq!(media.active_checkpoint(), 0);
        // A projection that is demonstrably later than the one initialization wrote.
        let claim = crate::obc2::journal::JournalBody {
            store: STORE,
            ..crate::obc2::samples::claim(model.epoch, 1, 0, [0x31; 16], 1)
        };
        model.apply(&claim).expect("a claim applies");
        media.write_checkpoint(1, &model).expect("the second checkpoint");
        assert_eq!(media.active_checkpoint(), 1);
        media.unmount();

        let (stride, mut slots) = buffers();
        let fat = card.fat();
        let survey = super::survey(&fat, card.root, None, stride, &mut slots);
        assert_eq!(survey.checkpoints_valid, [true, true], "both files should be valid: {survey:?}");
        assert!(matches!(survey.outcome, Outcome::Mount { checkpoint: 1, replay: 0, .. }), "{survey:?}");

        let mut remounted = Box::new(RamIndex::new(STORE));
        let mut media = attach(fat, card.root, &survey, stride, 8 * 1024 * 1024).expect("attach");
        media.load_index(&survey, &mut remounted).expect("projection");
        assert_eq!(remounted.through_sequence, 1);
        assert_eq!(remounted.actives.len(), 1, "the newer checkpoint's active row is missing");

        // The other parity. A third checkpoint alternates back to file 0, and §6.3 must select it —
        // which is also the parity that catches a class-6 bit read from the wrong staged file, since
        // a survey validates 0 first and 1 second.
        media.write_checkpoint(0, &model).expect("the third checkpoint");
        assert_eq!(media.active_checkpoint(), 0);
        media.unmount();
        let survey = super::survey(&card.fat(), card.root, None, stride, &mut slots);
        assert_eq!(survey.checkpoints_valid, [true, true]);
        assert!(matches!(survey.outcome, Outcome::Mount { checkpoint: 0, .. }), "{survey:?}");
    }

    /// §12's class 6, in **both** parities.
    ///
    /// The durable recovery-degraded bit lives in the selected checkpoint's header, and a survey
    /// stages the two files one after the other into one buffer. Reading the bit after the decision
    /// therefore reads whichever file was staged last — and because [`recovery::choose`] is
    /// deterministic, a store whose bit lives in the other file would drop it at *every* mount and
    /// report a fully writable store where §12 fixes a read-only one. So the bit is asserted from
    /// each file in turn, with the other one holding a clear bit.
    #[test]
    fn the_store_degraded_bit_is_observed_from_whichever_checkpoint_is_selected() {
        for degraded_slot in [0usize, 1] {
            let (card, mut media, _index, _base) = mounted();
            let mut model = CatalogModel::initial(STORE, ObjectKind::Weather.to_u16());
            // Two checkpoints, alternating, the later one in `degraded_slot`. The clear one is
            // written first so the degraded one always carries the greater through-sequence.
            let clear_slot = 1 - degraded_slot;
            let claim = crate::obc2::journal::JournalBody {
                store: STORE,
                ..crate::obc2::samples::claim(model.epoch, 1, 0, [0x31; 16], 1)
            };
            media.write_checkpoint(clear_slot, &model).expect("the clear checkpoint");
            model.apply(&claim).expect("a claim applies");
            model.flags |= 1;
            media.write_checkpoint(degraded_slot, &model).expect("the degraded checkpoint");
            media.unmount();

            let (stride, mut slots) = buffers();
            let fat = card.fat();
            let survey = super::survey(&fat, card.root, None, stride, &mut slots);
            assert!(
                matches!(survey.outcome, Outcome::Mount { checkpoint, store_degraded: true, .. } if checkpoint == degraded_slot),
                "the degraded bit was dropped with the flag in CAT{degraded_slot}: {survey:?}"
            );
            // §12's class 6 is what makes the store read-only, so the class and the refusal are the
            // two facts that matter downstream.
            assert_eq!(survey.class, MountClass::MountedStoreDegraded, "CAT{degraded_slot}");
            assert!(!survey.outcome.admits_mutation(), "a degraded store admitted a mutation");
        }
    }

    /// The complement: a clear bit in both files is not degraded in either parity, so the assertion
    /// above is testing the bit rather than the code path.
    #[test]
    fn a_store_with_no_degraded_bit_is_mounted_writable_in_either_parity() {
        for newer_slot in [0usize, 1] {
            let (card, mut media, _index, _base) = mounted();
            let model = CatalogModel::initial(STORE, ObjectKind::Weather.to_u16());
            media.write_checkpoint(1 - newer_slot, &model).expect("the first checkpoint");
            let mut later = model.clone();
            later
                .apply(&crate::obc2::journal::JournalBody {
                    store: STORE,
                    ..crate::obc2::samples::claim(model.epoch, 1, 0, [0x31; 16], 1)
                })
                .expect("a claim applies");
            media.write_checkpoint(newer_slot, &later).expect("the second checkpoint");
            media.unmount();

            let (stride, mut slots) = buffers();
            let survey = super::survey(&card.fat(), card.root, None, stride, &mut slots);
            assert!(
                matches!(survey.outcome, Outcome::Mount { checkpoint, store_degraded: false, .. } if checkpoint == newer_slot),
                "{survey:?}"
            );
            assert_eq!(survey.class, MountClass::Mounted);
            assert!(survey.outcome.admits_mutation());
        }
    }

    /// §16's reset: every object, result and journal record is gone, the identity is the new one,
    /// and the store still mounts.
    #[test]
    fn a_store_reset_leaves_a_freshly_initialized_store_under_the_new_identity() {
        const REPLACEMENT: StoreId = StoreId::new([0x77; 16]);
        let (card, media, index, base) = mounted();
        let mut store = transaction(media, &index, base);
        let operation = OperationId::new([0xD1; 16]);
        let payload = [0x2E; 512];
        let mut scratch = [0u8; 512];
        let crc = obc_crc::crc32(&payload);
        let claimed = store.execute(Command::Claim(intent(operation, payload.len() as u64, crc)), &mut scratch);
        assert!(matches!(claimed, EngineOutcome::Claim(obc_link::engine::ClaimOutcome::Claimed { .. })));
        store.execute(Command::Append { operation_id: operation, offset: 0, bytes: &payload }, &mut scratch);
        store.execute(
            Command::Seal { operation_id: operation, declared_length: payload.len() as u64, expected_crc: crc },
            &mut scratch,
        );
        store.execute(Command::Validate { operation_id: operation }, &mut scratch);
        store.execute(Command::Publish { operation_id: operation }, &mut scratch);
        assert!(store.retains(operation));

        store.media_mut().reset_store(REPLACEMENT).expect("reset");
        store.into_media().unmount();

        let (stride, mut slots) = buffers();
        let fat = card.fat();
        let survey = super::survey(&fat, card.root, None, stride, &mut slots);
        assert!(survey.is_mountable(), "a reset store did not mount: {survey:?}");
        assert_eq!(survey.valid_slots, 0, "the journal survived the reset");
        let mut remounted = Box::new(RamIndex::new(STORE));
        let mut media = attach(fat, card.root, &survey, stride, 8 * 1024 * 1024).expect("attach");
        media.load_index(&survey, &mut remounted).expect("projection");
        assert_eq!(remounted.store, REPLACEMENT, "the StoreId did not change");
        assert!(remounted.heads.is_empty() && remounted.results.is_empty());
        assert_eq!(*remounted, *RamIndex::project(&CatalogModel::initial(REPLACEMENT, ObjectKind::Weather.to_u16())));
    }

    /// **The simulator twin of the board's clean-flush measurement.**
    ///
    /// `obc2_store_bench` records every sector one `Command::Publish` writes on the real card and
    /// checks that none of them is single-copy metadata. That is the strongest evidence there is —
    /// and it runs on one card, on one desk, when somebody remembers to flash the bench. §13.1's
    /// obligation is what the whole commit path rests on, so it also needs a guard that runs on
    /// every push.
    ///
    /// This is that guard: the same publish, through the same kernel, over a genuine FAT32 volume
    /// with a logging block device under it. A sector written into FSInfo, either FAT, the root
    /// directory or `/OBC2`'s own directory sector fails the test — which is exactly what a
    /// `flush_file` reintroduced anywhere on the commit path would do.
    #[test]
    fn a_publish_writes_no_single_copy_metadata_sector() {
        type Logged = WriteLog<SparseDisk, 512>;
        type LoggedVmgr = VolumeManager<SharedBlockDevice<'static, Logged>, NullTime, 4, 16, 1>;
        type LoggedFat = Adapter<'static, SharedBlockDevice<'static, Logged>, NullTime, 4, 16, 1>;
        type LoggedMedia = FatMedia<'static, SharedBlockDevice<'static, Logged>, NullTime, 4, 16, 1>;

        let layout = Layout::default();
        let logged: &'static Logged = Box::leak(Box::new(WriteLog::new(fat32_card(layout))));
        let vmgr: &'static LoggedVmgr =
            Box::leak(Box::new(VolumeManager::new_with_limits(SharedBlockDevice(logged), NullTime, 9_000)));
        let (mbr, bpb) = geometry_sectors(logged.device(), layout.partition_start_lba);
        let geometry = geometry::admit(&mbr, &bpb, 0).expect("the simulated card is conforming");
        let volume = vmgr.open_raw_volume(VolumeIdx(0)).expect("mounts");
        let root = vmgr.open_root_dir(volume).expect("a root directory");

        let fat: LoggedFat = Adapter::new(vmgr);
        let (stride, mut slots) = buffers();
        initialize(&fat, root, STORE, stride).expect("initialization");
        let survey = super::survey(&fat, root, None, stride, &mut slots);
        assert!(survey.is_mountable(), "{survey:?}");
        let mut model = Box::new(RamIndex::new(STORE));
        let mut media: LoggedMedia = attach(fat, root, &survey, stride, 8 * 1024 * 1024).expect("attach");
        let base = media.load_index(&survey, &mut model).expect("projection");

        // The sector holding `COMMIT.JNL`'s own 32-byte directory entry. On FAT32 a directory lives
        // in the data region, so an entry rewrite is indistinguishable from a record write unless
        // this is known — the board bench refuses a verdict without it, and so does this.
        let mut entry_lba = None;
        vmgr.iterate_dir(media.obc2(), |entry| {
            if entry.name.base_name() == b"COMMIT" {
                entry_lba = Some(entry.entry_block.0);
            }
        })
        .expect("the /OBC2 listing");
        let entry_lba = entry_lba.expect("COMMIT.JNL has a directory entry");

        let mut store = Box::new(KernelTransaction::mount(media, AcceptEverything, NoHooks, *model, base));
        let operation = OperationId::new([0xE7; 16]);
        let payload = [0x6C; 2_048];
        let crc = obc_crc::crc32(&payload);
        let mut scratch = [0u8; 512];
        assert!(matches!(
            store.execute(Command::Claim(intent(operation, payload.len() as u64, crc)), &mut scratch),
            EngineOutcome::Claim(obc_link::engine::ClaimOutcome::Claimed { .. })
        ));
        store.execute(Command::Append { operation_id: operation, offset: 0, bytes: &payload }, &mut scratch);
        store.execute(
            Command::Seal { operation_id: operation, declared_length: payload.len() as u64, expected_crc: crc },
            &mut scratch,
        );
        store.execute(Command::Validate { operation_id: operation }, &mut scratch);

        // Only the publish is armed: the claim and the seal legitimately create files and change a
        // recorded length, so they *must* write directory entries. §13.1's obligation is about the
        // commit, which writes into files that reached their final length at initialization.
        logged.arm();
        let published = store.execute(Command::Publish { operation_id: operation }, &mut scratch);
        logged.disarm();
        assert!(matches!(published, EngineOutcome::Published(_)), "{published:?}");

        assert_eq!(logged.dropped(), 0, "the span log overflowed, so this window proves nothing");
        let written = logged.with_spans(touched);
        assert!(!written.is_empty(), "the publish wrote nothing at all — the log is not armed");
        let offenders: std::vec::Vec<(u32, Region)> = written
            .iter()
            .map(|&lba| (lba, geometry.region(lba)))
            .filter(|(lba, region)| *region != Region::Data || *lba == entry_lba)
            .collect();
        assert!(
            offenders.is_empty(),
            "§13.1's clean flush was violated: a publish wrote single-copy metadata {offenders:?} \
             (/OBC2's directory entry is LBA {entry_lba}, FSInfo {:?})",
            geometry.fs_info_lba
        );
        // And the positive half, because a publish that wrote *nothing* would also have no metadata.
        //
        // Two counts, and the difference between them is the reason to state both. **32** is the set
        // of distinct sectors: one 16,384-byte journal stride, contiguous. **33** is the number of
        // sector *writes*, because §6 makes the gate a second durability point — and the gate sits
        // at slot base + 1,536, which is sector 3 of the stride the body write already covered. So
        // the commit programs that one sector twice, on purpose, and the board bench's 33 and this
        // 32 are the same measurement counted two ways.
        assert_eq!(written.len(), 32, "a commit's distinct sectors are one journal stride: {written:?}");
        let first = written[0];
        assert!(
            written.iter().enumerate().all(|(step, lba)| *lba == first + step as u32),
            "the stride was not written contiguously: {written:?}"
        );
        let sector_writes: usize = logged.with_spans(|spans| spans.iter().map(|span| span.blocks as usize).sum());
        assert_eq!(sector_writes, 33, "the gate must re-program exactly one sector of the stride");
    }

    /// **The slice's acceptance**: one whole upload lifecycle through the kernel over a real FAT
    /// volume — claim, append, seal, validate, publish — and then the same store remounted from the
    /// card alone, still holding the head and the retained result.
    #[test]
    fn an_upload_lifecycle_commits_and_survives_a_remount() {
        let (card, media, index, base) = mounted();
        let mut store = transaction(media, &index, base);
        let operation = OperationId::new([0xC1; 16]);
        let payload = [0xAB; 4_096];
        let crc = obc_crc::crc32(&payload);
        let mut scratch = [0u8; 512];

        let claimed = store.execute(Command::Claim(intent(operation, payload.len() as u64, crc)), &mut scratch);
        let logical = match claimed {
            EngineOutcome::Claim(obc_link::engine::ClaimOutcome::Claimed { logical_object_id, .. }) => {
                logical_object_id
            }
            other => panic!("the claim was refused: {other:?}"),
        };
        for (index, chunk) in payload.chunks(1_024).enumerate() {
            let offset = (index * 1_024) as u64;
            assert!(
                matches!(
                    store.execute(Command::Append { operation_id: operation, offset, bytes: chunk }, &mut scratch),
                    EngineOutcome::Appended
                ),
                "append at {offset} was refused"
            );
        }
        assert!(matches!(
            store.execute(
                Command::Seal { operation_id: operation, declared_length: payload.len() as u64, expected_crc: crc },
                &mut scratch
            ),
            EngineOutcome::Sealed
        ));
        assert!(matches!(
            store.execute(Command::Validate { operation_id: operation }, &mut scratch),
            EngineOutcome::Validated
        ));
        let published = store.execute(Command::Publish { operation_id: operation }, &mut scratch);
        assert!(matches!(published, EngineOutcome::Published(_)), "publication failed: {published:?}");
        assert!(store.retains(operation), "the terminal result was not retained");
        let (revision, length, stored_crc) = store.head(ObjectKind::Route, logical).expect("a head");
        assert_eq!((length, stored_crc), (payload.len() as u64, crc));

        // A reboot: every resident fact is gone and the card is remounted from its own bytes.
        store.into_media().unmount();
        let (stride, mut slots) = buffers();
        let fat = card.fat();
        let survey = super::survey(&fat, card.root, None, stride, &mut slots);
        assert!(survey.is_mountable(), "the store did not remount: {survey:?}");
        assert_eq!(survey.valid_slots, 2, "a claim and a terminal record is two journal slots");
        let mut remounted = Box::new(RamIndex::new(STORE));
        let mut media = attach(fat, card.root, &survey, stride, 8 * 1024 * 1024).expect("attach");
        let base = media.load_index(&survey, &mut remounted).expect("projection");
        let mut store = transaction(media, &remounted, base);

        assert_eq!(store.head(ObjectKind::Route, logical), Some((revision, length, stored_crc)));
        assert!(store.retains(operation), "the retained result did not survive the remount");
        let mut back = [0u8; 4_096];
        assert_eq!(store.read_head(ObjectKind::Route, logical, &mut back), Some(4_096));
        assert_eq!(back, payload);

        // §8.1's query answers from the durable ledger, not from a resident memory of the upload.
        let report =
            store.execute(Command::QueryOperation { operation_id: operation, principal: PRINCIPAL }, &mut scratch);
        match report {
            EngineOutcome::OperationReport(OperationReport::Committed(_)) => {}
            other => panic!("query answered {other:?}"),
        }
        let _ = LogicalObjectId::ZERO;
    }

    /// The same lifecycle through **`CardStore`** — the owner, the route repository and the commit
    /// log — over a real FAT volume rather than a simulated card.
    ///
    /// The store's own tests run over the sector-level simulation; this is the one that proves the
    /// composition works against the media the board actually runs: `survey` → `initialize` →
    /// `attach` → `CardStore`, a real OBCR payload validated by the real route repository, and a
    /// projection that survives being read back out of the card's own bytes.
    #[test]
    fn a_card_store_publishes_a_validated_route_over_a_real_volume() {
        use crate::obc2::store::CardStore;
        use obc_link::engine::IntentMetadata;
        use obc_link::metadata::{MetadataEnvelope, MetadataWriter, SchemaClass, MAX_CATALOG_ENVELOPE};
        use obc_link::registry::retention;

        /// `specs/vectors/route-plain.obcr`, named "Vector Loop".
        const ROUTE: &[u8] = include_bytes!("../../../../specs/vectors/route-plain.obcr");

        let (_card, media, index, base) = mounted();
        let mut store = Box::new(CardStore::mount(media, index.as_ref().clone(), base));

        let mut buffer = [0u8; 32];
        let mut writer = MetadataWriter::new(&mut buffer).expect("a writer");
        writer.push(0x8001, &[retention::MONTH]).expect("retention");
        let put = writer.finish(ObjectKind::Route, SchemaClass::Put);
        let metadata = IntentMetadata::of(&MetadataEnvelope::decode(put, 128).expect("canonical")).expect("it fits");

        let operation = OperationId::new([0xC7; 16]);
        let crc = obc_crc::crc32(ROUTE);
        let mut scratch = [0u8; 512];
        let mut claim = intent(operation, ROUTE.len() as u64, crc);
        claim.metadata = metadata;
        let logical = match store.execute(Command::Claim(claim), &mut scratch) {
            EngineOutcome::Claim(obc_link::engine::ClaimOutcome::Claimed { logical_object_id, .. }) => {
                logical_object_id
            }
            other => panic!("the claim was refused: {other:?}"),
        };
        for (step, chunk) in ROUTE.chunks(64).enumerate() {
            let offset = (step * 64) as u64;
            assert!(matches!(
                store.execute(Command::Append { operation_id: operation, offset, bytes: chunk }, &mut scratch),
                EngineOutcome::Appended
            ));
        }
        assert!(matches!(
            store.execute(
                Command::Seal { operation_id: operation, declared_length: ROUTE.len() as u64, expected_crc: crc },
                &mut scratch
            ),
            EngineOutcome::Sealed
        ));
        assert!(matches!(
            store.execute(Command::Validate { operation_id: operation }, &mut scratch),
            EngineOutcome::Validated
        ));
        assert!(matches!(
            store.execute(Command::Publish { operation_id: operation }, &mut scratch),
            EngineOutcome::Published(_)
        ));

        let event = store.next_commit().expect("a durable commit wakes its repository");
        assert_eq!(event.kind, ObjectKind::Route);
        assert_eq!(event.logical_object_id, Some(logical));

        let mut staged = [0u8; MAX_CATALOG_ENVELOPE];
        let len = store.routes().projection(logical, &mut staged).expect("the re-read").expect("a published head");
        let envelope = MetadataEnvelope::decode(&staged[..len], MAX_CATALOG_ENVELOPE).expect("canonical");
        // Base tags: the critical bit is part of the encoding, not of a field's identity.
        assert_eq!(envelope.field(0x0001).and_then(|field| field.as_str()), Some("Vector Loop"));
        assert_eq!(envelope.field(0x0002).and_then(|field| field.as_u8()), Some(retention::MONTH));
        assert_eq!(store.routes().retention(logical).expect("a re-read"), Some(retention::MONTH));
        store.into_media().unmount();
    }
}
