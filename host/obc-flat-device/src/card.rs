//! The card a device owns.
//!
//! The Rust harness hands its device a borrowed [`SparseDisk`] that the test outlives, which is
//! exactly right there: the test formats it, cuts power on it and remounts it. A device that lives
//! behind a wasm boundary has no such owner, and a reboot has to keep the durable bytes, so the card
//! is shared by handle instead — one sparse disk, one fault gate, any number of holders.

use std::rc::Rc;

use obc_storage::flat::sim::{DiskError, FaultOnce, SparseDisk};
use obc_storage::flat::{BlockDevice, FlatStore, StoreId};

pub use obc_storage::flat::sim::MediaOp;

use crate::CATALOG_BLOCKS;

/// A shared handle to the sparse disk. `SparseDisk` itself is only a `BlockDevice` by reference, so
/// this is what makes an owned, clonable card possible at all.
struct Disk(Rc<SparseDisk>);

impl BlockDevice for Disk {
    type Error = DiskError;

    fn block_count(&self) -> Result<u64, DiskError> {
        (&*self.0).block_count()
    }

    fn read(&self, lba: u64, buf: &mut [u8]) -> Result<(), DiskError> {
        (&*self.0).read(lba, buf)
    }

    fn write(&self, lba: u64, buf: &[u8]) -> Result<(), DiskError> {
        (&*self.0).write(lba, buf)
    }

    fn sync(&self) -> Result<(), DiskError> {
        (&*self.0).sync()
    }
}

/// A durable card several owners can hold: the store mounts one clone, the device keeps another to
/// cut power and refuse media operations with.
#[derive(Clone)]
pub struct Card {
    disk: Rc<SparseDisk>,
    gate: Rc<FaultOnce<Disk>>,
}

impl Card {
    /// An unformatted card: every block reads as zeros, which is what a new card is.
    pub fn blank(blocks: u64, seed: u64) -> Card {
        let disk = Rc::new(SparseDisk::blank(blocks, seed));
        let gate = Rc::new(FaultOnce::new(Disk(Rc::clone(&disk))));
        Card { disk, gate }
    }

    /// A card formatted with the identity the caller names.
    pub fn formatted(blocks: u64, seed: u64, store: StoreId) -> Card {
        let card = Card::blank(blocks, seed);
        FlatStore::initialize(card.clone(), store).expect("the test card formats");
        card
    }

    /// Refuse the next media operation of this kind.
    pub fn fault_next(&self, op: MediaOp) {
        self.gate.fault_next(op);
    }

    /// Refuse one media operation of this kind, after letting `skip` of them through.
    pub fn fault_after(&self, op: MediaOp, skip: u32) {
        self.gate.fault_after(op, skip);
    }

    /// True once the armed fault has been delivered. A probe whose fault never fired proves nothing.
    pub fn fault_fired(&self) -> bool {
        self.gate.fired()
    }

    /// Power came back: everything unsynced is gone and the fault plan is cleared. The durable image
    /// stays, which is the whole difference between a reboot and a new card.
    pub fn reboot(&self) {
        self.disk.reboot();
    }

    /// Overwrite both catalog copies with bytes no gate will validate, leaving the superblock alone.
    /// A card that mounts and then cannot read its catalog is a different state from an unformatted
    /// one, and only the real thing produces it.
    pub fn corrupt_catalog(&self) {
        let garbage = vec![0x5a_u8; 512];
        for lba in CATALOG_BLOCKS {
            self.disk.install(lba, &garbage);
        }
    }

    /// The durable image of one block: what a reboot would see.
    pub fn block(&self, lba: u64) -> [u8; 512] {
        self.disk.block(lba)
    }
}

impl BlockDevice for Card {
    type Error = DiskError;

    fn block_count(&self) -> Result<u64, DiskError> {
        (&*self.gate).block_count()
    }

    fn read(&self, lba: u64, buf: &mut [u8]) -> Result<(), DiskError> {
        (&*self.gate).read(lba, buf)
    }

    fn write(&self, lba: u64, buf: &[u8]) -> Result<(), DiskError> {
        (&*self.gate).write(lba, buf)
    }

    fn sync(&self) -> Result<(), DiskError> {
        (&*self.gate).sync()
    }
}
