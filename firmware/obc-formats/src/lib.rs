//! Normative, platform-neutral building blocks for OpenBikeComputer's persistent formats.
//!
//! The root format specifications remain the byte-level contracts. This crate is their small
//! code authority: fixed sizes, versions, flags, sentinels, endian primitives, and the neutral
//! byte-source/sink seam shared by producers and consumers. It deliberately contains no reader,
//! cache, conversion pipeline, storage adapter, executor, or rendering policy.

#![no_std]

#[cfg(test)]
extern crate std;

// Only the host-only OBCG deflate codec (`obcg-deflate`) allocates: one inflate buffer per tile.
// The device build never enables that feature and stays allocator-free.
#[cfg(feature = "obcg-deflate")]
extern crate alloc;

pub mod io;
pub mod obcg;
pub mod obcm;
pub mod obcr;
pub mod obct;
pub mod obcw;
pub mod precip4;
pub mod ride;
pub mod track;
