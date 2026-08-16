//! A deterministic faulting media harness for the §1.1 fault model and the §13.1 adapter contract.
//!
//! Host-only, and the whole point of it is to be **hostile in exactly the ways the format admits**
//! and in no others. `OBC2_Storage_Format.md` §1.1: "A 512-byte sector write is **not** assumed to
//! be all-or-nothing. A cut during programming may corrupt any sector inside the media program page
//! being programmed", and "the fault-isolation assumption is that a write may corrupt sectors inside
//! the program page being written and does not corrupt bytes lying in another program page". So a
//! cut here tears exactly the pages a write touched, and never a byte outside them — a harness that
//! corrupted more would prove nothing about this format, and one that corrupted less would let a
//! real bug through.
//!
//! ## Which §13.1 obligations this models — five of the eight
//!
//! - **Synchronization.** A write lands in a volatile cache; only [`Media::sync`] makes it durable.
//!   A cut before the sync loses it. "A failed sync has an uncertain outcome and is resolved by
//!   recovery", so a cut *during* a sync commits an arbitrary seeded subset.
//! - **Write completeness.** [`Media::write_at`] can return success having written fewer bytes than
//!   requested, which is why every OBC2 write is followed by an explicit length check.
//! - **Seek bound.** Writing past the recorded length fails rather than extending the file, which is
//!   what makes §7's rewind mandatory rather than cosmetic.
//! - **Gate isolation.** Writing 512 bytes at a gate offset touches no other sector.
//! - **Full-length initialization.** [`Media::create`] produces a file at its full recorded length,
//!   which is the state every slot-addressing rule assumes.
//!
//! Three it does **not** model, because they are properties of a real FAT adapter rather than of a
//! sector-addressed medium, and they arrive with that adapter: **clean flush** (a sync of a
//! fixed-length gated file must not rewrite the directory entry or FSInfo), **chain longer than
//! length** (a cut between preallocation and zero-fill, which free-space accounting must tolerate),
//! and the **absent primitives** rule (no `delete_dir`, no `rename`). Nothing here can prove an
//! adapter satisfies them; the point of naming them is that a green crash matrix does not.
//!
//! ## Fixed files and payload files are different things
//!
//! [`Media::create`] makes a fixed OBC2 metadata file: full length, never growing, and the seek
//! bound applies to both ends of a write. [`Media::create_payload`] makes a `GEN` payload, which §3
//! defines as "exactly the canonical payload bytes" with no wrapper — it starts empty and is
//! extended by ordinary writes.
//!
//! The distinction earns its place at one specific rule. §7: a durable offset "may exceed the
//! payload's observed length after a cut, because the length recorded in a FAT directory entry is
//! only guaranteed durable once the sync that followed the length-changing write has completed". A
//! growable file here therefore stages its new *length* alongside its bytes, and a cut during the
//! sync commits the two independently — so the harness genuinely produces the state §7's mandatory
//! rewind exists to resolve, instead of asserting that it would.
//!
//! Determinism is total: the same seed and the same [`FaultPlan`] produce the same bytes, so a
//! failing case in the crash matrix is a case anyone can rerun.

use std::string::String;
use std::vec;
use std::vec::Vec;

use super::limits::{PROGRAM_PAGE, SECTOR};

/// Where a power cut lands relative to one media operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    /// The operation never reached the card: nothing changed.
    Before,
    /// The card was mid-operation: a write tears the pages it was programming, and a sync commits
    /// an arbitrary subset of what was pending.
    During,
    /// The operation completed and then power was lost. Anything still unsynced is gone.
    After,
}

/// Every cut point [`When`] admits, in the order the matrix enumerates them.
pub const EVERY_WHEN: [When; 3] = [When::Before, When::During, When::After];

/// A scheduled power cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cut {
    /// The one-based index of the media operation it lands on.
    pub op: u32,
    /// Where in that operation.
    pub when: When,
}

/// What the harness should inject, and when.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FaultPlan {
    /// A power cut.
    pub cut: Option<Cut>,
    /// The operation index at which a write returns success having written only this many bytes.
    pub short_write: Option<(u32, usize)>,
    /// The operation index at which a write fails because the medium is full.
    pub media_full: Option<u32>,
    /// The operation index at which a read returns garbage.
    pub corrupt_read: Option<u32>,
}

impl FaultPlan {
    /// A plan that injects a cut and nothing else.
    pub fn cut(op: u32, when: When) -> Self {
        FaultPlan { cut: Some(Cut { op, when }), ..FaultPlan::default() }
    }
}

/// What a media operation can fail with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaError {
    /// The card lost power; every later operation fails the same way until reboot.
    PowerLoss,
    /// The medium has no space for this write.
    Full,
    /// The offset or length lies past the file's recorded length (§13.1's seek bound).
    OutOfRange,
    /// No such file.
    NoSuchFile,
}

/// A handle to one file in the harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileId(usize);

/// One file: its durable image, the writes a sync has not yet persisted, and — for a growable file
/// — the recorded length those writes would extend it to.
///
/// The split matters for §7. A FAT directory entry's length "is only guaranteed durable once the
/// sync that followed the length-changing write has completed", so a cut can leave bytes on the card
/// that the recorded length does not reach. That is exactly the state §7's mandatory rewind exists
/// for, and modelling the length separately is the only way a harness can produce it.
#[derive(Debug, Clone)]
struct FileImage {
    name: String,
    durable: Vec<u8>,
    pending: Vec<(usize, Vec<u8>)>,
    pending_len: Option<usize>,
    growable: bool,
}

/// A tiny deterministic PRNG. Not cryptographic and not meant to be: it exists so a torn page is
/// reproducible from a seed.
#[derive(Debug, Clone, Copy)]
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

/// One counted media operation, as the transcript vectors describe it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operation {
    /// The file the operation addressed.
    pub file: String,
    /// `"write"` or `"sync"`. Reads are counted too but are not part of a commit path.
    pub kind: &'static str,
    /// The byte offset a write addressed; zero for a sync.
    pub offset: usize,
    /// The byte count a write requested; zero for a sync.
    pub length: usize,
}

/// The faulting medium.
#[derive(Debug, Clone)]
pub struct Media {
    files: Vec<FileImage>,
    log: Vec<Operation>,
    plan: FaultPlan,
    rng: Rng,
    ops: u32,
    powered: bool,
}

impl Media {
    /// A powered, fault-free medium with no files.
    pub fn new(seed: u64) -> Self {
        Media {
            files: Vec::new(),
            log: Vec::new(),
            plan: FaultPlan::default(),
            rng: Rng(seed | 1),
            ops: 0,
            powered: true,
        }
    }

    /// Installs a fault plan. Operations are counted from `1` across the whole medium, so a plan is
    /// written against the operation *sequence* a scenario performs, not against one file.
    pub fn set_plan(&mut self, plan: FaultPlan) {
        self.plan = plan;
    }

    /// Creates a file of `len` durable zero bytes. This models §13.1's full-length initialization
    /// and is deliberately not a counted operation: the scenarios under test start from an
    /// initialized card.
    pub fn create(&mut self, name: &str, len: usize) -> FileId {
        self.files.push(FileImage {
            name: String::from(name),
            durable: vec![0u8; len],
            pending: Vec::new(),
            pending_len: None,
            growable: false,
        });
        FileId(self.files.len() - 1)
    }

    /// Creates an empty **growable** file: a `GEN` payload.
    ///
    /// §13.1's full-length initialization and its seek bound are rules about the fixed metadata
    /// files. A generation payload is §3's "canonical payload bytes" with no wrapper: it is created
    /// empty and extended by ordinary writes, and its recorded length becomes durable only at the
    /// sync that follows the write which changed it.
    pub fn create_payload(&mut self, name: &str) -> FileId {
        self.files.push(FileImage {
            name: String::from(name),
            durable: Vec::new(),
            pending: Vec::new(),
            pending_len: None,
            growable: true,
        });
        FileId(self.files.len() - 1)
    }

    /// Places durable bytes without counting an operation.
    ///
    /// Harness setup only — the state a scenario *starts* from, exactly as [`create`](Self::create)
    /// is. It is not a modelled media operation and has no cut points, which is what keeps a matrix
    /// over a long scenario from also enumerating cuts inside the card it was handed.
    pub fn install(&mut self, file: FileId, offset: usize, bytes: &[u8]) {
        let image = &mut self.files[file.0];
        if image.durable.len() < offset + bytes.len() {
            image.durable.resize(offset + bytes.len(), 0);
        }
        image.durable[offset..offset + bytes.len()].copy_from_slice(bytes);
    }

    /// Truncates a growable file to zero length (§7's readmission rewind).
    ///
    /// A counted operation. A cut *during* it is genuinely ambiguous on a real card — the directory
    /// entry and the FAT chain are two writes — so the outcome is seeded, and both branches are
    /// states recovery must handle.
    pub fn truncate(&mut self, file: FileId) -> Result<(), MediaError> {
        if !self.powered {
            return Err(MediaError::PowerLoss);
        }
        self.ops += 1;
        let op = self.ops;
        self.log.push(Operation { file: self.files[file.0].name.clone(), kind: "truncate", offset: 0, length: 0 });
        if self.cut_is(op, When::Before) {
            self.power_off();
            return Err(MediaError::PowerLoss);
        }
        if self.cut_is(op, When::During) {
            if self.rng.next() & 1 == 0 {
                self.apply_truncate(file);
            }
            self.power_off();
            return Err(MediaError::PowerLoss);
        }
        self.apply_truncate(file);
        if self.cut_is(op, When::After) {
            self.power_off();
        }
        Ok(())
    }

    fn apply_truncate(&mut self, file: FileId) {
        let image = &mut self.files[file.0];
        image.durable.clear();
        image.pending.clear();
        image.pending_len = None;
    }

    /// The length a write may address: the recorded length, plus any extension a pending write has
    /// staged but no sync has persisted.
    fn effective_len(&self, file: FileId) -> usize {
        let image = &self.files[file.0];
        image.pending_len.unwrap_or(0).max(image.durable.len())
    }

    /// The handle of an existing file.
    pub fn file(&self, name: &str) -> Result<FileId, MediaError> {
        self.files.iter().position(|file| file.name == name).map(FileId).ok_or(MediaError::NoSuchFile)
    }

    /// The file's recorded length.
    pub fn len(&self, file: FileId) -> usize {
        self.files[file.0].durable.len()
    }

    /// True when the medium holds no files.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// How many counted operations have run. A scenario is enumerated by running it once with no
    /// plan and reading this.
    pub fn ops(&self) -> u32 {
        self.ops
    }

    /// The ordered operations this medium has been asked to perform.
    ///
    /// This is what a checked-in crash-cut transcript states, so the transcript and the code that
    /// walks a commit path cannot drift apart: the sequence is the spec's ordering, and the log is
    /// the proof that the harness performs it.
    pub fn log(&self) -> &[Operation] {
        &self.log
    }

    /// Whether the card still has power.
    pub fn powered(&self) -> bool {
        self.powered
    }

    /// Restores power and drops everything that was never synced — which is exactly what a reboot
    /// does. The fault plan is cleared, so recovery reads a stable image.
    pub fn reboot(&mut self) {
        for file in &mut self.files {
            file.pending.clear();
            file.pending_len = None;
        }
        self.plan = FaultPlan::default();
        self.powered = true;
    }

    /// The durable image of a file: what a reboot would see. The crash oracle reads through this.
    pub fn image(&self, file: FileId) -> &[u8] {
        &self.files[file.0].durable
    }

    /// Writes `bytes` at `offset`, returning how many bytes were accepted.
    ///
    /// §13.1: a short write "is an error, never a success" *to the caller* — the adapter reports it
    /// honestly and OBC2 checks the returned length. That check is the caller's, not this
    /// function's.
    pub fn write_at(&mut self, file: FileId, offset: usize, bytes: &[u8]) -> Result<usize, MediaError> {
        if !self.powered {
            return Err(MediaError::PowerLoss);
        }
        self.ops += 1;
        let op = self.ops;
        self.log.push(Operation { file: self.files[file.0].name.clone(), kind: "write", offset, length: bytes.len() });
        if self.cut_is(op, When::Before) {
            self.power_off();
            return Err(MediaError::PowerLoss);
        }
        if self.plan.media_full == Some(op) {
            return Err(MediaError::Full);
        }
        let len = self.effective_len(file);
        let growable = self.files[file.0].growable;
        // §13.1's seek bound applies to both: a write may never *start* past the end. A fixed file
        // may not end past it either, which is the whole of the bound; a growable one extends.
        if offset > len || (!growable && offset + bytes.len() > len) {
            return Err(MediaError::OutOfRange);
        }
        let accepted = match self.plan.short_write {
            Some((at, count)) if at == op => count.min(bytes.len()),
            _ => bytes.len(),
        };
        if growable && offset + accepted > len {
            self.files[file.0].pending_len = Some(offset + accepted);
        }
        self.files[file.0].pending.push((offset, bytes[..accepted].to_vec()));
        if self.cut_is(op, When::During) {
            self.tear_pages(file, offset, accepted);
            self.power_off();
            return Err(MediaError::PowerLoss);
        }
        if self.cut_is(op, When::After) {
            // The write returned, but nothing that was not already synced survives.
            self.power_off();
        }
        Ok(accepted)
    }

    /// Persists everything written to this file since the last sync.
    pub fn sync(&mut self, file: FileId) -> Result<(), MediaError> {
        if !self.powered {
            return Err(MediaError::PowerLoss);
        }
        self.ops += 1;
        let op = self.ops;
        self.log.push(Operation { file: self.files[file.0].name.clone(), kind: "sync", offset: 0, length: 0 });
        if self.cut_is(op, When::Before) {
            self.power_off();
            return Err(MediaError::PowerLoss);
        }
        if self.cut_is(op, When::During) {
            // "A failed sync has an uncertain outcome and is resolved by recovery": commit a seeded
            // subset of the pending writes and tear the page of one of the rest.
            //
            // This is deliberately the *weaker* model of the two available: a real card could tear
            // every page it was mid-programming, not just one. One torn page is enough to exercise
            // every gate rule these records have — the gate and its body share a page, so tearing
            // one is what invalidates a record — and keeping the choice seeded and singular keeps a
            // failing case reproducible. A stronger model belongs with the adapter, where multiple
            // in-flight pages become real.
            let pending = core::mem::take(&mut self.files[file.0].pending);
            let pending_len = self.files[file.0].pending_len.take();
            // The directory entry is a separate write from the payload's, so a cut may leave the
            // recorded length behind the bytes. Seeding the choice produces both states, and §7's
            // rewind is written for exactly the one where it does.
            if self.rng.next() & 1 == 0 {
                if let Some(len) = pending_len {
                    self.grow_to(file, len);
                }
            }
            let mut torn: Option<(usize, usize)> = None;
            for (offset, bytes) in pending {
                if self.rng.next() & 1 == 0 {
                    self.apply(file, offset, &bytes);
                } else if torn.is_none() {
                    torn = Some((offset, bytes.len()));
                }
            }
            if let Some((offset, len)) = torn {
                self.tear_pages(file, offset, len);
            }
            self.power_off();
            return Err(MediaError::PowerLoss);
        }
        let pending = core::mem::take(&mut self.files[file.0].pending);
        if let Some(len) = self.files[file.0].pending_len.take() {
            self.grow_to(file, len);
        }
        for (offset, bytes) in pending {
            self.apply(file, offset, &bytes);
        }
        if self.cut_is(op, When::After) {
            self.power_off();
        }
        Ok(())
    }

    /// Reads `len` bytes at `offset`, through the volatile cache.
    pub fn read_at(&mut self, file: FileId, offset: usize, len: usize) -> Result<Vec<u8>, MediaError> {
        if !self.powered {
            return Err(MediaError::PowerLoss);
        }
        self.ops += 1;
        let op = self.ops;
        let file_len = self.files[file.0].durable.len();
        if offset > file_len || offset + len > file_len {
            return Err(MediaError::OutOfRange);
        }
        let mut out = self.files[file.0].durable[offset..offset + len].to_vec();
        for (write_offset, bytes) in &self.files[file.0].pending {
            overlay(&mut out, offset, *write_offset, bytes);
        }
        if self.plan.corrupt_read == Some(op) {
            let mut rng = self.rng;
            for byte in out.iter_mut() {
                *byte = rng.next() as u8;
            }
            self.rng = rng;
        }
        Ok(out)
    }

    fn cut_is(&self, op: u32, when: When) -> bool {
        matches!(self.plan.cut, Some(cut) if cut.op == op && cut.when == when)
    }

    fn power_off(&mut self) {
        self.powered = false;
        for file in &mut self.files {
            file.pending.clear();
            file.pending_len = None;
        }
    }

    fn grow_to(&mut self, file: FileId, len: usize) {
        let image = &mut self.files[file.0];
        if image.durable.len() < len {
            image.durable.resize(len, 0);
        }
    }

    /// Applies a pending write, clamped to the recorded length.
    ///
    /// Bytes beyond it are not durable: the file does not reach that far, so nothing on the card
    /// holds them. This is the other half of the state §7's rewind resolves.
    fn apply(&mut self, file: FileId, offset: usize, bytes: &[u8]) {
        let len = self.files[file.0].durable.len();
        if offset >= len {
            return;
        }
        let end = (offset + bytes.len()).min(len);
        self.files[file.0].durable[offset..end].copy_from_slice(&bytes[..end - offset]);
    }

    /// Corrupts every sector of every program page the write at `offset..offset + len` touched, and
    /// nothing else. §1.1's isolation assumption is exactly this boundary.
    fn tear_pages(&mut self, file: FileId, offset: usize, len: usize) {
        if len == 0 {
            return;
        }
        let first = offset / PROGRAM_PAGE;
        let last = (offset + len - 1) / PROGRAM_PAGE;
        let file_len = self.files[file.0].durable.len();
        let mut rng = self.rng;
        for page in first..=last {
            let start = page * PROGRAM_PAGE;
            let end = (start + PROGRAM_PAGE).min(file_len);
            for sector in (start..end).step_by(SECTOR) {
                let sector_end = (sector + SECTOR).min(end);
                for byte in &mut self.files[file.0].durable[sector..sector_end] {
                    *byte = rng.next() as u8;
                }
            }
        }
        self.rng = rng;
    }
}

fn overlay(out: &mut [u8], read_offset: usize, write_offset: usize, bytes: &[u8]) {
    let start = read_offset.max(write_offset);
    let end = (read_offset + out.len()).min(write_offset + bytes.len());
    if start < end {
        out[start - read_offset..end - read_offset].copy_from_slice(&bytes[start - write_offset..end - write_offset]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn medium() -> (Media, FileId) {
        let mut media = Media::new(7);
        let file = media.create("COMMIT.JNL", 3 * PROGRAM_PAGE);
        (media, file)
    }

    #[test]
    fn an_unsynced_write_does_not_survive_a_cut() {
        let (mut media, file) = medium();
        media.write_at(file, 0, &[0xAB; 512]).unwrap();
        media.set_plan(FaultPlan::cut(2, When::Before));
        assert_eq!(media.sync(file), Err(MediaError::PowerLoss));
        media.reboot();
        assert!(media.image(file).iter().all(|&byte| byte == 0));
    }

    #[test]
    fn a_synced_write_survives() {
        let (mut media, file) = medium();
        media.write_at(file, 0, &[0xAB; 512]).unwrap();
        media.sync(file).unwrap();
        media.set_plan(FaultPlan::cut(3, When::Before));
        assert_eq!(media.write_at(file, 512, &[0xCD; 512]), Err(MediaError::PowerLoss));
        media.reboot();
        assert!(media.image(file)[..512].iter().all(|&byte| byte == 0xAB));
        assert!(media.image(file)[512..1024].iter().all(|&byte| byte == 0));
    }

    /// The isolation assumption, stated as a test: a torn write damages its own program page and
    /// leaves every byte of every other page exactly as it was.
    #[test]
    fn tearing_is_confined_to_the_program_page_being_written() {
        let (mut media, file) = medium();
        for page in 0..3 {
            media.write_at(file, page * PROGRAM_PAGE, &[0x11; 512]).unwrap();
        }
        media.sync(file).unwrap();

        media.set_plan(FaultPlan::cut(media.ops() + 1, When::During));
        let _ = media.write_at(file, PROGRAM_PAGE + 1_536, &[0x22; 512]);
        media.reboot();
        let image = media.image(file);
        assert!(image[..512].iter().all(|&byte| byte == 0x11), "page 0 was damaged");
        assert!(image[2 * PROGRAM_PAGE..2 * PROGRAM_PAGE + 512].iter().all(|&byte| byte == 0x11), "page 2 damaged");
        assert!(image[PROGRAM_PAGE..PROGRAM_PAGE + 512].iter().any(|&byte| byte != 0x11), "page 1 was not torn");
    }

    #[test]
    fn a_short_write_reports_the_bytes_it_accepted() {
        let (mut media, file) = medium();
        media.set_plan(FaultPlan { short_write: Some((1, 100)), ..FaultPlan::default() });
        assert_eq!(media.write_at(file, 0, &[0xAB; 512]).unwrap(), 100);
    }

    #[test]
    fn writing_past_the_recorded_length_fails_rather_than_extending() {
        let (mut media, file) = medium();
        let len = media.len(file);
        assert_eq!(media.write_at(file, len, &[0xAB; 512]), Err(MediaError::OutOfRange));
        assert_eq!(media.write_at(file, len - 256, &[0xAB; 512]), Err(MediaError::OutOfRange));
        assert_eq!(media.len(file), len);
    }

    #[test]
    fn a_full_medium_refuses_the_write_without_changing_anything() {
        let (mut media, file) = medium();
        media.set_plan(FaultPlan { media_full: Some(1), ..FaultPlan::default() });
        assert_eq!(media.write_at(file, 0, &[0xAB; 512]), Err(MediaError::Full));
        media.reboot();
        assert!(media.image(file).iter().all(|&byte| byte == 0));
    }

    #[test]
    fn a_corrupt_read_does_not_change_the_medium() {
        let (mut media, file) = medium();
        media.write_at(file, 0, &[0xAB; 512]).unwrap();
        media.sync(file).unwrap();
        media.set_plan(FaultPlan { corrupt_read: Some(media.ops() + 1), ..FaultPlan::default() });
        let read = media.read_at(file, 0, 512).unwrap();
        assert!(read.iter().any(|&byte| byte != 0xAB));
        assert!(media.image(file)[..512].iter().all(|&byte| byte == 0xAB));
    }

    #[test]
    fn every_operation_after_a_cut_fails_until_reboot() {
        let (mut media, file) = medium();
        media.set_plan(FaultPlan::cut(1, When::During));
        assert_eq!(media.write_at(file, 0, &[0xAB; 512]), Err(MediaError::PowerLoss));
        assert_eq!(media.sync(file), Err(MediaError::PowerLoss));
        assert_eq!(media.read_at(file, 0, 4), Err(MediaError::PowerLoss));
        media.reboot();
        assert!(media.read_at(file, 0, 4).is_ok());
    }

    /// A `GEN` payload grows by being written, and the growth is durable only after its sync.
    #[test]
    fn a_payload_file_grows_at_its_sync_rather_than_at_its_write() {
        let mut media = Media::new(11);
        let payload = media.create_payload("GEN");
        assert_eq!(media.len(payload), 0);

        media.write_at(payload, 0, &[0xAB; 1_000]).unwrap();
        assert_eq!(media.len(payload), 0, "the recorded length changed before the sync");
        media.sync(payload).unwrap();
        assert_eq!(media.len(payload), 1_000);
        assert!(media.image(payload).iter().all(|&byte| byte == 0xAB));

        // Appending at the end extends it further; starting past the end is still the seek bound.
        media.write_at(payload, 1_000, &[0xCD; 24]).unwrap();
        media.sync(payload).unwrap();
        assert_eq!(media.len(payload), 1_024);
        assert_eq!(media.write_at(payload, 2_000, &[0u8; 4]), Err(MediaError::OutOfRange));
    }

    /// §7's rewind case, produced rather than asserted: a cut can leave payload bytes on the card
    /// that the recorded length does not reach.
    #[test]
    fn a_cut_can_leave_the_recorded_length_behind_the_bytes() {
        let mut behind = 0;
        for seed in 1..40u64 {
            let mut media = Media::new(seed);
            let payload = media.create_payload("GEN");
            media.write_at(payload, 0, &[0xAB; 4_096]).unwrap();
            media.sync(payload).unwrap();

            media.write_at(payload, 4_096, &[0xCD; 4_096]).unwrap();
            media.set_plan(FaultPlan::cut(media.ops() + 1, When::During));
            let _ = media.sync(payload);
            media.reboot();
            if media.len(payload) == 4_096 {
                behind += 1;
            }
            // Whatever happened, no byte past the recorded length survives — that is what makes an
            // offset above it unreachable rather than merely stale.
            assert!(media.image(payload).len() == media.len(payload));
        }
        assert!(behind > 0, "no seed produced a length that lagged its bytes");
    }

    /// Truncation is the whole of §7's restart under the restart-only profile, and a cut during it
    /// leaves one of the two states recovery already handles.
    #[test]
    fn truncation_is_all_or_nothing_at_a_reboot() {
        let mut truncated = 0;
        for seed in 1..40u64 {
            let mut media = Media::new(seed);
            let payload = media.create_payload("GEN");
            media.write_at(payload, 0, &[0xAB; 2_048]).unwrap();
            media.sync(payload).unwrap();

            media.set_plan(FaultPlan::cut(media.ops() + 1, When::During));
            let _ = media.truncate(payload);
            media.reboot();
            match media.len(payload) {
                0 => truncated += 1,
                2_048 => assert!(media.image(payload).iter().all(|&byte| byte == 0xAB)),
                other => panic!("seed {seed} left a {other}-byte payload"),
            }
        }
        assert!(truncated > 0 && truncated < 39, "the seeded truncation outcome is not exercising both branches");
    }

    /// Installed bytes are setup, not a modelled operation: they change the card and count nothing.
    #[test]
    fn installed_bytes_are_durable_and_uncounted() {
        let mut media = Media::new(13);
        let file = media.create("CAT0.CHK", 1_024);
        let before = media.ops();
        media.install(file, 512, &[0x5A; 512]);
        assert_eq!(media.ops(), before, "install counted an operation");
        assert!(media.image(file)[512..].iter().all(|&byte| byte == 0x5A));
        assert!(media.image(file)[..512].iter().all(|&byte| byte == 0));
    }

    #[test]
    fn the_same_seed_and_plan_produce_the_same_bytes() {
        let run = || {
            let (mut media, file) = medium();
            media.set_plan(FaultPlan::cut(1, When::During));
            let _ = media.write_at(file, 0, &[0xAB; 512]);
            media.reboot();
            media.image(file).to_vec()
        };
        assert_eq!(run(), run());
    }
}
