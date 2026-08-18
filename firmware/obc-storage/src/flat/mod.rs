//! The flat card store: the whole of [`FLAT_Store_Format.md`] and §2 of [`FLAT_Store_Protocol.md`].
//!
//! [`FLAT_Store_Format.md`]: ../../../../specs/FLAT_Store_Format.md
//! [`FLAT_Store_Protocol.md`]: ../../../../specs/FLAT_Store_Protocol.md
//!
//! The rule the whole thing serves:
//!
//! > An object never changes. New bytes get new space. One commit makes the new bytes visible and
//! > makes the old bytes free.
//!
//! The active ride is the one exception — it grows for hours — and [`journal`] is the one mechanism
//! it gets.
//!
//! ## Reading order
//!
//! [`seam`] is what everything above the store sees: five operations, `EntryMeta`, and an opaque
//! `Allocation`. [`device`] is what it sits on: 512-byte blocks and a sync. Between them,
//! [`layout`] is the card's geometry and the address arithmetic that *is* the read path, and
//! [`superblock`], [`catalog`] and [`journal`] are the three record shapes. [`bitmap`] is the free
//! map, which is the catalog's complement and nothing else. [`store`] composes them: mount,
//! initialization, the alternating commit, and the ride journal's write half. [`source`] is the one
//! adapter above the seam: an open object as an `obc_formats::io::ByteSource`, which is the only
//! vocabulary the readers — chunk caches, A\*, the route and weather readers — speak.
//!
//! Decoding is **total** — every input is either a typed record or a typed
//! [`DecodeError`](error::DecodeError) — bounded, and allocation-free. The seam is total in the same
//! way: every operation either does its work or returns a typed [`StoreError`], including on hostile
//! arguments. Both counters stop one short of wrapping rather than overflowing — a `Revision` or an
//! `ObjectId` at `u64::MAX` is refused, and a card whose newest gate carries commit sequence
//! `u64::MAX` mounts read-only. Resident state is the 8 KiB free bitmap plus a handful of rows; the
//! entry array stays on the card.
//!
//! [`sim`] and [`model`] are host-only: a sparse block device that tears exactly the pages the fault
//! model admits, and the reference model the crash matrix holds a recovered card to.

// The card's own byte layout, and nothing above the seam may name it: an LBA, an extent range and a
// gate sector are this module's business. `seam`, `error`, `device` and `store` are the public face —
// a board crate has to be able to name `BlockDevice`, and a consumer `Store`.
pub(crate) mod bitmap;
pub(crate) mod catalog;
pub mod device;
pub mod error;
pub(crate) mod journal;
pub(crate) mod layout;
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
mod cost;
#[cfg(test)]
mod crash;
#[cfg(test)]
mod fuzz;
#[cfg(test)]
mod read_cost;
#[cfg(test)]
mod vectors;

pub use device::BlockDevice;
pub use error::{DecodeError, Reason, Record, StoreError};
pub use seam::{
    Allocation, DisplayName, EntryFlags, EntryMeta, Mutation, ObjectId, ObjectKind, PutSource, Revision,
    RideCheckpoint, Store, StoreId,
};
pub use source::StoreSource;
pub use store::{FlatStore, Handle, Mode, RideRecovery};

/// The format version this store implements. A card whose layout differs is a different version,
/// which the version field of every record already names.
pub const FORMAT_VERSION: u16 = 1;
