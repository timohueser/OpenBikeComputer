//! Normative, platform-neutral building blocks for OpenBikeComputer's persistent formats.
//!
//! The root format specifications remain the byte-level contracts. This crate is their small
//! code authority: fixed sizes, versions, flags, sentinels, endian primitives, the neutral
//! byte-source/sink seam shared by producers and consumers, and — sitting directly on that seam —
//! the one index-block cache every quadtree walk reads through. It deliberately contains no
//! reader, conversion pipeline, storage adapter, executor, or rendering policy.

#![no_std]

#[cfg(test)]
extern crate std;

pub mod articles;
pub mod assistant;
pub mod bike;
pub mod cache;
pub mod io;
pub mod obcm;
pub mod obcr;
pub mod obct;
pub mod ride;
pub mod track;
