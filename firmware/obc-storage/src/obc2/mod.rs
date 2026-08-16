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
//! collector, no compaction driver, and no FAT adapter here; those are the later slices of #1354,
//! and the adapter seam of §13.1 is the last of them. Nothing in this module knows a filename: §3's
//! `GEN`/`WORK` name mapping is the adapter's, and no rule here needs it.
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
//! remaining record shapes. [`model`] and [`recovery`] are the pure logic; [`media`] and
//! [`vectors`] are host-only.

pub mod checkpoint;
pub mod entries;
pub mod error;
pub mod gate;
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
pub mod media;
#[cfg(any(test, feature = "std"))]
pub mod samples;
#[cfg(any(test, feature = "std"))]
pub mod vectors;

#[cfg(test)]
mod crash;

pub use error::{ApplyError, DecodeError, Reason, Record};
pub use gate::Gate;
pub use model::CatalogModel;
pub use recovery::{Decision, FailClosed};

/// The OBC2 storage-format version this kernel implements.
pub const FORMAT_VERSION: u16 = 1;
