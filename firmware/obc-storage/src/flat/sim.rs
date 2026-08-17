//! A deterministic faulting card, host-only.
//!
//! The whole point of it is to be **hostile in exactly the ways the format admits** and in no others.
//! `FLAT_Store_Format.md` §1: "A power cut during programming may corrupt any block inside the media
//! **program page** being programmed ... A write may corrupt blocks inside the page it is programming
//! and does not corrupt blocks lying in another page." So a cut here tears exactly the pages a write
//! touched and never a byte outside them — a harness that corrupted more would prove nothing about
//! this format, and one that corrupted less would let a real bug through.
//!
//! Three things it models, each named by the format or by the store's write ordering:
//!
//! - **Synchronization.** A write lands in a volatile cache; only [`sync`](SparseDisk::sync) makes it
//!   durable. A cut before the sync loses it, and a cut *during* one commits a seeded subset — which
//!   is the state every "body synchronized before the gate" rule exists for.
//! - **Page tearing.** A cut during a write corrupts every block of every program page it touched.
//! - **A card that stops answering.** Reads and writes fail, which is how the read-only mount paths
//!   get produced rather than asserted.
//!
//! The disk is sparse — a `BTreeMap` of written blocks over an implicit sea of zeros — because a
//! 30 GiB card is the interesting geometry and no test cares about more than a few megabytes of it.
//! A block nobody wrote reads as zeros, exactly as an unformatted card does.
//!
//! Determinism is total: the same seed and the same [`FaultPlan`] produce the same bytes, so a
//! failing case in the crash matrix is a case anyone can rerun.
//!
//! A power cut is not the only way media fails, and it is the *less* demanding way: after a cut there
//! is no store left to ask anything of. [`FaultOnce`] is the other shape — one operation refused, the
//! card still there — which is the input every error path at the seam actually takes, and the only way
//! to hold a `StoreError::Media` to leaving the store's resident state where a retry can meet it.

use std::collections::BTreeMap;
use std::vec::Vec;

use core::cell::{Cell, RefCell};

use super::device::BlockDevice;
use super::layout::{BLOCK, PAGE_BLOCKS};

/// Where a power cut lands relative to one media operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    /// The operation never reached the card: nothing changed.
    Before,
    /// The card was mid-operation: a write tears the pages it was programming, and a sync commits an
    /// arbitrary subset of what was pending.
    During,
    /// The operation completed and then power was lost. Anything still unsynced is gone.
    After,
}

/// Every cut point [`When`] admits, in the order the matrix enumerates them.
pub const EVERY_WHEN: [When; 3] = [When::Before, When::During, When::After];

/// A scheduled power cut: the one-based index of the media operation it lands on, and where in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FaultPlan {
    pub op: u32,
    pub when: When,
}

/// What a media operation fails with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskError;

/// A card over a sparse image, with a fault plan.
pub struct SparseDisk {
    durable: RefCell<BTreeMap<u64, [u8; BLOCK]>>,
    /// Writes a sync has not yet made durable.
    pending: RefCell<Vec<(u64, [u8; BLOCK])>>,
    total: u64,
    plan: Cell<Option<FaultPlan>>,
    ops: Cell<u32>,
    powered: Cell<bool>,
    rng: Cell<u64>,
}

impl SparseDisk {
    /// An unformatted card of `total` blocks: every block reads as zeros.
    pub fn blank(total: u64, seed: u64) -> Self {
        SparseDisk {
            durable: RefCell::new(BTreeMap::new()),
            pending: RefCell::new(Vec::new()),
            total,
            plan: Cell::new(None),
            ops: Cell::new(0),
            powered: Cell::new(true),
            rng: Cell::new(seed | 1),
        }
    }

    /// Installs a fault plan. Operations are counted from `1` across the whole card, so a plan is
    /// written against the operation *sequence* a scenario performs.
    pub fn plan(&self, plan: FaultPlan) {
        self.plan.set(Some(plan));
    }

    /// How many counted operations have run. A scenario is enumerated by running it once with no
    /// plan and reading this.
    pub fn ops(&self) -> u32 {
        self.ops.get()
    }

    /// Restores power and drops everything that was never synced — which is what a reboot does. The
    /// fault plan is cleared, so recovery reads a stable image.
    pub fn reboot(&self) {
        self.pending.borrow_mut().clear();
        self.plan.set(None);
        self.powered.set(true);
    }

    /// The durable image of one block: what a reboot would see.
    pub fn block(&self, lba: u64) -> [u8; BLOCK] {
        self.durable.borrow().get(&lba).copied().unwrap_or([0; BLOCK])
    }

    /// Places durable bytes without counting an operation: the state a scenario *starts* from, not a
    /// modelled media operation, so a matrix over a long scenario does not also enumerate cuts inside
    /// the card it was handed.
    pub fn install(&self, lba: u64, bytes: &[u8]) {
        let mut durable = self.durable.borrow_mut();
        for (index, chunk) in bytes.chunks(BLOCK).enumerate() {
            let mut block = [0u8; BLOCK];
            block[..chunk.len()].copy_from_slice(chunk);
            durable.insert(lba + index as u64, block);
        }
    }

    fn next(&self) -> u64 {
        let mut x = self.rng.get();
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng.set(x);
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Counts one media operation, or refuses because the card has no power.
    fn begin(&self) -> Result<u32, DiskError> {
        if !self.powered.get() {
            return Err(DiskError);
        }
        self.ops.set(self.ops.get() + 1);
        Ok(self.ops.get())
    }

    fn cut_is(&self, op: u32, when: When) -> bool {
        self.plan.get().is_some_and(|plan| plan.op == op && plan.when == when)
    }

    fn power_off(&self) {
        self.powered.set(false);
        self.pending.borrow_mut().clear();
    }

    /// Corrupts every block of every program page the write touched, and nothing else. §1's isolation
    /// assumption is exactly this boundary.
    fn tear(&self, lba: u64, blocks: u64) {
        let first = lba / PAGE_BLOCKS * PAGE_BLOCKS;
        let last = (lba + blocks - 1) / PAGE_BLOCKS * PAGE_BLOCKS + PAGE_BLOCKS;
        let mut durable = self.durable.borrow_mut();
        for block in first..last {
            let mut bytes = [0u8; BLOCK];
            for byte in bytes.iter_mut() {
                *byte = self.next() as u8;
            }
            durable.insert(block, bytes);
        }
    }
}

impl BlockDevice for &SparseDisk {
    type Error = DiskError;

    fn block_count(&self) -> Result<u64, DiskError> {
        Ok(self.total)
    }

    fn read(&self, lba: u64, buf: &mut [u8]) -> Result<(), DiskError> {
        let blocks = (buf.len() / BLOCK) as u64;
        let op = self.begin()?;
        if !buf.len().is_multiple_of(BLOCK) || lba + blocks > self.total {
            return Err(DiskError);
        }
        if self.cut_is(op, When::Before) || self.cut_is(op, When::During) {
            self.power_off();
            return Err(DiskError);
        }
        {
            let durable = self.durable.borrow();
            let pending = self.pending.borrow();
            for index in 0..blocks {
                // A read sees the volatile cache, which is what makes "durable only after sync" a
                // property of power loss rather than of visibility.
                let block = pending
                    .iter()
                    .rev()
                    .find(|(at, _)| *at == lba + index)
                    .map(|(_, bytes)| *bytes)
                    .or_else(|| durable.get(&(lba + index)).copied())
                    .unwrap_or([0; BLOCK]);
                buf[index as usize * BLOCK..(index as usize + 1) * BLOCK].copy_from_slice(&block);
            }
        }
        if self.cut_is(op, When::After) {
            self.power_off();
        }
        Ok(())
    }

    fn write(&self, lba: u64, buf: &[u8]) -> Result<(), DiskError> {
        let blocks = (buf.len() / BLOCK) as u64;
        let op = self.begin()?;
        if !buf.len().is_multiple_of(BLOCK) || blocks == 0 || lba + blocks > self.total {
            return Err(DiskError);
        }
        if self.cut_is(op, When::Before) {
            self.power_off();
            return Err(DiskError);
        }
        {
            let mut pending = self.pending.borrow_mut();
            for index in 0..blocks {
                let mut block = [0u8; BLOCK];
                block.copy_from_slice(&buf[index as usize * BLOCK..(index as usize + 1) * BLOCK]);
                pending.push((lba + index, block));
            }
        }
        if self.cut_is(op, When::During) {
            self.tear(lba, blocks);
            self.power_off();
            return Err(DiskError);
        }
        if self.cut_is(op, When::After) {
            // The write returned, but nothing that was not already synced survives.
            self.power_off();
        }
        Ok(())
    }

    fn sync(&self) -> Result<(), DiskError> {
        let op = self.begin()?;
        if self.cut_is(op, When::Before) {
            self.power_off();
            return Err(DiskError);
        }
        if self.cut_is(op, When::During) {
            // A failed sync has an uncertain outcome: commit a seeded subset of the pending writes
            // and tear the page of one of the rest.
            //
            // This is deliberately the weaker of the two available models — a real card could tear
            // every page it was mid-programming, not just one. One torn page is enough to exercise
            // every gate and slot rule in this format, and keeping the choice seeded and singular
            // keeps a failing case reproducible.
            let pending = core::mem::take(&mut *self.pending.borrow_mut());
            let mut torn = None;
            for (lba, bytes) in pending {
                if self.next() & 1 == 0 {
                    self.durable.borrow_mut().insert(lba, bytes);
                } else if torn.is_none() {
                    torn = Some(lba);
                }
            }
            if let Some(lba) = torn {
                self.tear(lba, 1);
            }
            self.power_off();
            return Err(DiskError);
        }
        let pending = core::mem::take(&mut *self.pending.borrow_mut());
        for (lba, bytes) in pending {
            self.durable.borrow_mut().insert(lba, bytes);
        }
        if self.cut_is(op, When::After) {
            self.power_off();
        }
        Ok(())
    }
}

/// One media operation, as a fault-once plan names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaOp {
    Read,
    Write,
    Sync,
}

/// A card that refuses one operation and then behaves.
///
/// [`SparseDisk`]'s only failure is a power cut, which is total: after it nothing works until a reboot.
/// Real media also fails **transiently** — one read refused, one write refused, the card still there —
/// and that is the input every error path at the seam actually takes. What such a path owes its caller
/// is that a `StoreError::Media` leaves the store's resident state where a retry can meet it, and that
/// a failed read is never read as an answer. Neither can be tested by cutting power, because after a
/// cut there is no store left to ask.
///
/// So this wraps a card and fails the `skip + 1`-th operation of one kind, once. Everything before and
/// after it goes through to the card underneath, and the wrapper counts nothing else — the plan is
/// written against the operations one seam call performs.
pub struct FaultOnce<D> {
    inner: D,
    /// The kind to refuse and how many of that kind to let through first.
    armed: Cell<Option<(MediaOp, u32)>>,
    fired: Cell<bool>,
}

impl<D> FaultOnce<D> {
    /// Wraps a card, armed with nothing.
    pub fn new(inner: D) -> Self {
        FaultOnce { inner, armed: Cell::new(None), fired: Cell::new(false) }
    }

    /// Refuses the next operation of this kind.
    pub fn fault_next(&self, op: MediaOp) {
        self.fault_after(op, 0);
    }

    /// Refuses one operation of this kind, after letting `skip` of them through.
    pub fn fault_after(&self, op: MediaOp, skip: u32) {
        self.armed.set(Some((op, skip)));
        self.fired.set(false);
    }

    /// True once the armed fault has been delivered. A test asserts this, because a probe whose fault
    /// never fired proves nothing about the path it was aiming at.
    pub fn fired(&self) -> bool {
        self.fired.get()
    }

    /// The card underneath.
    pub fn inner(&self) -> &D {
        &self.inner
    }

    /// Refuses this operation, or lets it through.
    fn gate(&self, op: MediaOp) -> Result<(), DiskError> {
        match self.armed.get() {
            Some((armed, 0)) if armed == op => {
                self.armed.set(None);
                self.fired.set(true);
                Err(DiskError)
            }
            Some((armed, skip)) if armed == op => {
                self.armed.set(Some((armed, skip - 1)));
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

impl<D: BlockDevice> BlockDevice for &FaultOnce<D> {
    /// The wrapper's own refusal and the card's are the same to the store, which maps every media
    /// failure to `StoreError::Media` without looking.
    type Error = DiskError;

    fn block_count(&self) -> Result<u64, DiskError> {
        self.inner.block_count().map_err(|_| DiskError)
    }

    fn read(&self, lba: u64, buf: &mut [u8]) -> Result<(), DiskError> {
        self.gate(MediaOp::Read)?;
        self.inner.read(lba, buf).map_err(|_| DiskError)
    }

    fn write(&self, lba: u64, buf: &[u8]) -> Result<(), DiskError> {
        self.gate(MediaOp::Write)?;
        self.inner.write(lba, buf).map_err(|_| DiskError)
    }

    fn sync(&self) -> Result<(), DiskError> {
        self.gate(MediaOp::Sync)?;
        self.inner.sync().map_err(|_| DiskError)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disk() -> SparseDisk {
        SparseDisk::blank(4_096 + 64 * 2_048, 7)
    }

    #[test]
    fn an_unsynced_write_does_not_survive_a_cut() {
        let disk = disk();
        (&disk).write(0, &[0xAB; BLOCK]).unwrap();
        disk.plan(FaultPlan { op: 2, when: When::Before });
        assert_eq!((&disk).sync(), Err(DiskError));
        disk.reboot();
        assert_eq!(disk.block(0), [0; BLOCK]);
    }

    #[test]
    fn a_synced_write_survives() {
        let disk = disk();
        (&disk).write(0, &[0xAB; BLOCK]).unwrap();
        (&disk).sync().unwrap();
        disk.plan(FaultPlan { op: 3, when: When::Before });
        assert_eq!((&disk).write(1, &[0xCD; BLOCK]), Err(DiskError));
        disk.reboot();
        assert_eq!(disk.block(0), [0xAB; BLOCK]);
        assert_eq!(disk.block(1), [0; BLOCK]);
    }

    /// The isolation assumption, stated as a test: a torn write damages its own program page and
    /// leaves every byte of every other page exactly as it was.
    #[test]
    fn tearing_is_confined_to_the_program_page_being_written() {
        let disk = disk();
        for page in 0..3u64 {
            (&disk).write(page * PAGE_BLOCKS, &[0x11; BLOCK]).unwrap();
        }
        (&disk).sync().unwrap();

        disk.plan(FaultPlan { op: disk.ops() + 1, when: When::During });
        let _ = (&disk).write(PAGE_BLOCKS + 3, &[0x22; BLOCK]);
        disk.reboot();
        assert_eq!(disk.block(0), [0x11; BLOCK], "page 0 was damaged");
        assert_eq!(disk.block(2 * PAGE_BLOCKS), [0x11; BLOCK], "page 2 was damaged");
        assert_ne!(disk.block(PAGE_BLOCKS), [0x11; BLOCK], "page 1 was not torn");
        assert_ne!(disk.block(PAGE_BLOCKS + 31), [0; BLOCK], "the whole page was not torn");
    }

    #[test]
    fn a_cut_during_a_sync_commits_a_subset_and_tears_one_page() {
        let mut both = 0;
        for seed in 1..40u64 {
            let disk = SparseDisk::blank(8_192, seed);
            for block in 0..4u64 {
                (&disk).write(block * PAGE_BLOCKS, &[0xAB; BLOCK]).unwrap();
            }
            disk.plan(FaultPlan { op: disk.ops() + 1, when: When::During });
            assert_eq!((&disk).sync(), Err(DiskError));
            disk.reboot();
            let landed = (0..4).filter(|page| disk.block(page * PAGE_BLOCKS) == [0xAB; BLOCK]).count();
            if landed > 0 && landed < 4 {
                both += 1;
            }
        }
        assert!(both > 0, "no seed produced a partially committed sync");
    }

    #[test]
    fn every_operation_after_a_cut_fails_until_reboot() {
        let disk = disk();
        disk.plan(FaultPlan { op: 1, when: When::During });
        assert_eq!((&disk).write(0, &[0xAB; BLOCK]), Err(DiskError));
        assert_eq!((&disk).sync(), Err(DiskError));
        assert_eq!((&disk).read(0, &mut [0; BLOCK]), Err(DiskError));
        disk.reboot();
        assert!((&disk).read(0, &mut [0; BLOCK]).is_ok());
    }

    #[test]
    fn the_same_seed_and_plan_produce_the_same_bytes() {
        let run = || {
            let disk = disk();
            disk.plan(FaultPlan { op: 1, when: When::During });
            let _ = (&disk).write(0, &[0xAB; 4 * BLOCK]);
            disk.reboot();
            (0..64).map(|lba| disk.block(lba)).collect::<Vec<_>>()
        };
        assert_eq!(run(), run());
    }

    /// The other device's contract: one operation of the armed kind fails, the ones before and after it
    /// reach the card, and a fault armed for one kind does not disturb another.
    #[test]
    fn a_fault_once_device_refuses_one_operation_of_one_kind() {
        let disk = disk();
        let faulty = FaultOnce::new(&disk);
        faulty.fault_after(MediaOp::Write, 1);
        (&faulty).write(0, &[0xAB; BLOCK]).unwrap();
        assert_eq!((&faulty).write(1, &[0xCD; BLOCK]), Err(DiskError));
        assert!(faulty.fired());
        (&faulty).write(1, &[0xCD; BLOCK]).unwrap();
        (&faulty).sync().unwrap();
        assert_eq!(disk.block(0), [0xAB; BLOCK], "a write before the fault did not reach the card");
        assert_eq!(disk.block(1), [0xCD; BLOCK], "a write after the fault did not reach the card");

        faulty.fault_next(MediaOp::Read);
        let mut buf = [0u8; BLOCK];
        assert_eq!((&faulty).read(0, &mut buf), Err(DiskError));
        assert!(faulty.fired());
        (&faulty).read(0, &mut buf).unwrap();
        assert_eq!(buf, [0xAB; BLOCK]);
        (&faulty).write(2, &[0xEF; BLOCK]).unwrap();
    }

    #[test]
    fn a_read_past_the_card_fails_and_an_unwritten_block_is_zeros() {
        let disk = SparseDisk::blank(64, 3);
        let mut buf = [0xFFu8; BLOCK];
        (&disk).read(63, &mut buf).unwrap();
        assert_eq!(buf, [0; BLOCK]);
        assert_eq!((&disk).read(64, &mut buf), Err(DiskError));
        assert_eq!((&disk).write(64, &[0; BLOCK]), Err(DiskError));
    }
}
