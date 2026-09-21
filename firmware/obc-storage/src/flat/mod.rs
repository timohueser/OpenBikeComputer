// The card's own byte layout, and nothing above the seam may name it. `seam`, `error`, `device` and
// `store` are the public face: a board crate has to be able to name `BlockDevice`, and a consumer
// `Store`.
pub(crate) mod bitmap;
pub(crate) mod catalog;
pub mod device;
pub mod error;
pub(crate) mod journal;
pub(crate) mod layout;
pub mod metadata;
pub(crate) mod raw;
pub mod seam;
pub mod source;
pub mod store;
pub(crate) mod superblock;
pub mod wire;

#[cfg(any(test, feature = "std"))]
pub mod model;
#[cfg(any(test, feature = "std"))]
pub mod sim;

#[cfg(test)]
mod block_runs;
#[cfg(test)]
mod cost;
#[cfg(test)]
mod crash;
#[cfg(test)]
mod fence;
#[cfg(test)]
mod fuzz;
#[cfg(test)]
mod granularity;
#[cfg(test)]
mod map_read;
#[cfg(test)]
mod read_cost;
#[cfg(test)]
mod sealed;
#[cfg(test)]
mod vectors;

pub use device::BlockDevice;
pub use error::{DecodeError, Reason, Record, StoreError};
pub use layout::MAX_RANGES;
pub use seam::{
    Allocation, DisplayName, EntryFlags, EntryMeta, Mutation, ObjectId, ObjectKind, PutSource, Revision,
    RideCheckpoint, Store, StoreId, RIDE_RESUME_LEN,
};
pub use source::{SealedSource, StoreSource};
pub use store::{BlockRun, FlatStore, Handle, Mode, RideRecovery, SealedAllocation};
/// The four bytes at block 0 of a formatted card: what names a card image.
pub use superblock::MAGIC as SUPERBLOCK_MAGIC;

/// The format version this store implements.
pub const FORMAT_VERSION: u16 = 1;

pub mod route_cleanup;
