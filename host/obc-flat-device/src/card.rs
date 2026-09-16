//! The card a device owns.
//!
//! The Rust harness hands its device a borrowed [`SparseDisk`] that the test outlives, which is
//! exactly right there: the test formats it, cuts power on it and remounts it. A device that lives
//! behind a wasm boundary has no such owner, and a reboot has to keep the durable bytes, so the card
//! is shared by handle instead — one sparse disk, any number of holders.

use std::rc::Rc;

use obc_storage::flat::sim::{DiskError, SparseDisk};
use obc_storage::flat::{BlockDevice, FlatStore};

use crate::STORE;

/// A durable card several owners can hold: the store mounts one clone, the device keeps another to
/// cut power with. `SparseDisk` is only a `BlockDevice` by reference, so this handle is what makes
/// an owned, clonable card possible at all.
#[derive(Clone)]
pub struct Card(Rc<SparseDisk>);

impl Card {
    /// An unformatted card: every block reads as zeros, which is what a new card is.
    pub fn blank(blocks: u64, seed: u64) -> Card {
        Card(Rc::new(SparseDisk::blank(blocks, seed)))
    }

    /// A card this crate has formatted with [`STORE`].
    pub fn formatted(blocks: u64, seed: u64) -> Card {
        let card = Card::blank(blocks, seed);
        FlatStore::initialize(card.clone(), STORE).expect("the test card formats");
        card
    }

    /// Power came back: everything unsynced is gone. The durable image stays, which is the whole
    /// difference between a reboot and a new card.
    pub fn reboot(&self) {
        self.0.reboot();
    }
}

impl BlockDevice for Card {
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
