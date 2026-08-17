//! The OBC2 storage kernel: the record codecs, the catalog reference model, and the recovery
//! decision frozen by [`OBC2_Storage_Format.md`].
//!
//! [`OBC2_Storage_Format.md`]: ../../../../specs/OBC2_Storage_Format.md
//!
//! ## What this is
//!
//! Every byte the store writes to `/OBC2` other than a payload has exactly one definition, and this
//! is it. Decoding is **total** — every input is either a typed record or a typed
//! [`DecodeError`](error::DecodeError) — **bounded**, and allocation-free. Encoding produces exact
//! bytes: fixed-size records return fixed-size arrays, and the one variable record (the resolution
//! generation) writes into a caller-provided slice.
//!
//! On top of the codecs sit two pure pieces: [`model::Projection`], the bounded projection whose
//! `apply` *is* the meaning of a journal record, and [`recovery::choose`], §6.3's decision written
//! as a function of what a mount observed. Between them they say what the store's state is after
//! any sequence of records and any crash, without touching a filesystem.
//!
//! The projection has two instantiations and one `apply`. [`model::CatalogModel`] holds whole
//! entries and is the **host oracle**; [`index::RamIndex`] is §13's resident shape — the same rows
//! with the catalog-projection envelopes, the resolution `GenerationId`s and the terminal-result
//! bodies left on the card and re-read on demand. The device places the second and never the first.
//!
//! ## The kernel, and the store above it
//!
//! Most of this module is the kernel: bounded, allocation-free machinery that owns bytes and knows
//! no domain. Above it sit the three pieces #1359 adds — [`store::CardStore`], the one owner of a
//! mounted volume; [`repositories`], the concrete per-kind types it lends that owner's capabilities
//! to one at a time; and [`commit`], §4's coalescing commit event. Still absent, and named rather
//! than implied: the transfer coordinator's session table, the resource arbiter, a typed mount/
//! recovery status snapshot, and any garbage-collector schedule. Nothing here is wired into the
//! shipping image yet.
//!
//! What the kernel half is. The §13.1 adapter seam — [`adapter`] over
//! `embedded_sdmmc`, with [`geometry`] deciding §1.1's volume preconditions ahead of it. The
//! resident [`index`] §13 fixes, and the [`compaction`] pass §6.3 materializes a checkpoint through.
//! The [`generation`] writer, [`leases`], the [`gc`] collector, [`mount`]'s classification, and
//! §3's `GEN`/`WORK` [`names`] mapping. Each is a bounded, allocation-free piece with a seam a store
//! composes rather than a trait that is the union of what a store does.
//!
//! The kernel half holds no domain knowledge, and that separation is now enforced by a seam rather
//! than by care: §1 gives domain repositories "validated projections and immutable payloads", so a
//! [`transaction::Validator`] is handed the sealed bytes and hands back the catalog projection its
//! head will carry, while the kernel owns byte counts, CRCs, ordering, publication and recovery and
//! never parses OBCR, OBCW, a map, a ride, or an update image.
//!
//! ## Reading order
//!
//! [`limits`] is the capacity table everything is bounded by, and [`gate`] the 512 bytes that make
//! a body a record. [`entries`] holds the projection rows; [`checkpoint`] and [`journal`] are the
//! two containers that carry them. [`work`], [`handoff`], [`init`] and [`resolution`] are the
//! remaining record shapes. [`model`] and [`recovery`] are the pure logic.
//!
//! On top of those sit the engine pieces, each answering one section: [`index`] (§13's resident
//! index), [`compaction`] (§6.3's forward pass), [`generation`] (§7's writer), [`leases`] (§9's
//! table), [`gc`] (§9's reachability and collector), [`mount`] (§12's classification and staging)
//! and [`names`] (§3's identity mapping).
//!
//! [`geometry`], [`adapter`] and [`blocklog`] are the media seam, and [`fat`] is the board's
//! composition of it: §12's mount classification and initialization over a real directory listing,
//! and the [`transaction::KernelMedia`] the device runs. [`media`], [`fatsim`] and [`vectors`] are
//! host-only.

pub mod adapter;
pub mod blocklog;
pub mod checkpoint;
pub mod commit;
pub mod compaction;
pub mod entries;
pub mod error;
pub mod fat;
pub mod gate;
pub mod gc;
pub mod generation;
pub mod geometry;
pub mod handoff;
pub mod index;
pub mod init;
pub mod journal;
pub mod leases;
pub mod limits;
pub mod model;
pub mod mount;
pub mod names;
mod raw;
pub mod recovery;
pub mod repositories;
pub mod resolution;
pub mod store;
pub mod transaction;
pub mod work;

#[cfg(any(test, feature = "std"))]
pub mod card;
#[cfg(any(test, feature = "std"))]
pub mod equivalence;
#[cfg(any(test, feature = "std"))]
pub mod fatsim;
#[cfg(any(test, feature = "std"))]
pub mod lock_law;
#[cfg(any(test, feature = "std"))]
pub mod media;
#[cfg(any(test, feature = "std"))]
pub mod samples;
#[cfg(any(test, feature = "std"))]
pub mod vectors;

#[cfg(test)]
mod crash;

/// The Device Object System v2 identity types this kernel's records carry, re-exported so a
/// consumer can name a `StoreId` or a `GenerationId` — both of which appear in public record
/// fields — without taking an `obc-link` dependency of its own.
pub use obc_link::ids::{GenerationId, LogicalObjectId, OperationId, StoreId};

pub use commit::{ChangeKind, CommitEvent, CommitLog};
pub use error::{ApplyError, DecodeError, Reason, Record};
pub use gate::Gate;
pub use index::RamIndex;
pub use model::{CatalogModel, Projection};
pub use recovery::{Decision, FailClosed};
pub use store::CardStore;

/// The OBC2 storage-format version this kernel implements.
pub const FORMAT_VERSION: u16 = 1;
