//! Byte I/O abstractions for the route format.
//!
//! These live in [`obc_reader::byte_io`] so the **map** reader streams through the same seam;
//! this module re-exports them so the route code's `crate::byte_io::{…}` paths and the public
//! `obc_route::{ByteSource, ByteSink, SliceSource, Error}` are unchanged.

pub use obc_reader::byte_io::{ByteSink, ByteSource, Error, SliceSource};
