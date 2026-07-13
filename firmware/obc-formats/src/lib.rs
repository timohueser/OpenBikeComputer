//! Normative, platform-neutral building blocks for OpenBikeComputer's persistent formats.
//!
//! The root format specifications remain the byte-level contracts. This crate is their small
//! code authority: fixed sizes, versions, flags, sentinels, endian primitives, and the neutral
//! byte-source/sink seam shared by producers and consumers. It deliberately contains no reader,
//! cache, conversion pipeline, storage adapter, executor, or rendering policy.

#![no_std]

pub mod io;
pub mod obcm;
pub mod obcr;
pub mod ride;
pub mod track;
