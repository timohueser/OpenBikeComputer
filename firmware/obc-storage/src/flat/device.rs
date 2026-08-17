//! The block device the store sits on.
//!
//! The store owns the whole card from LBA 0 and addresses it in 512-byte blocks; a `sync` is what
//! makes a write durable. Nothing else is required of a card, and nothing here knows what a
//! partition, a cluster or a file is.
//!
//! Every method takes `&self` because [`Store::open`](super::seam::Store::open) and
//! [`Store::read`](super::seam::Store::read) do: a driver that needs exclusive access to its bus
//! holds it behind a cell, which is the same shape `embedded_sdmmc::BlockDevice` has.

/// A card, addressed in 512-byte blocks.
pub trait BlockDevice {
    /// What the card fails with. The store does not interpret it: every failure is
    /// [`StoreError::Media`](super::error::StoreError::Media).
    type Error;

    /// Blocks the card has.
    fn block_count(&self) -> Result<u64, Self::Error>;

    /// Reads `buf.len() / 512` blocks starting at `lba`.
    fn read(&self, lba: u64, buf: &mut [u8]) -> Result<(), Self::Error>;

    /// Writes `buf.len() / 512` blocks starting at `lba`. Durable only after [`sync`](Self::sync).
    fn write(&self, lba: u64, buf: &[u8]) -> Result<(), Self::Error>;

    /// Makes every write issued so far durable.
    fn sync(&self) -> Result<(), Self::Error>;
}
