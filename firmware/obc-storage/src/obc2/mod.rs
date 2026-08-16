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
//! On top of the codecs sit two pure pieces: [`model::CatalogModel`], the bounded projection whose
//! `apply` *is* the meaning of a journal record, and [`recovery::choose`], §6.3's decision written
//! as a function of what a mount observed. Between them they say what the store's state is after
//! any sequence of records and any crash, without touching a filesystem.
//!
//! ## What this deliberately is not
//!
//! It is not the engine. There is no `CardStore`, no transaction API, no lease table, no garbage
//! collector and no compaction driver; those are the later slices of #1354. The §13.1 adapter seam
//! *is* here now — [`adapter`] over `embedded_sdmmc`, with [`geometry`] deciding §1.1's volume
//! preconditions ahead of it — but it is only the media seam: it knows sectors, lengths and syncs,
//! and still knows no filename. §3's `GEN`/`WORK` name mapping belongs to the store above it.
//!
//! It also holds no domain knowledge. §1: domain repositories "provide validated projections and
//! immutable payloads"; the kernel owns byte counts, CRCs, ordering, publication and recovery, and
//! never parses OBCR, OBCW, a map, a ride, or an update image.
//!
//! ## Reading order
//!
//! [`limits`] is the capacity table everything is bounded by, and [`gate`] the 512 bytes that make
//! a body a record. [`entries`] holds the projection rows; [`checkpoint`] and [`journal`] are the
//! two containers that carry them. [`work`], [`handoff`], [`init`] and [`resolution`] are the
//! remaining record shapes. [`model`] and [`recovery`] are the pure logic; [`geometry`],
//! [`adapter`] and [`blocklog`] are the media seam; [`media`], [`fatsim`] and [`vectors`] are
//! host-only.

pub mod adapter;
pub mod blocklog;
pub mod checkpoint;
pub mod entries;
pub mod error;
pub mod gate;
pub mod geometry;
pub mod handoff;
pub mod init;
pub mod journal;
pub mod limits;
pub mod model;
mod raw;
pub mod recovery;
pub mod resolution;
pub mod work;

#[cfg(any(test, feature = "std"))]
pub mod fatsim;
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

pub use error::{ApplyError, DecodeError, Reason, Record};
pub use gate::Gate;
pub use model::CatalogModel;
pub use recovery::{Decision, FailClosed};

/// The OBC2 storage-format version this kernel implements.
pub const FORMAT_VERSION: u16 = 1;
