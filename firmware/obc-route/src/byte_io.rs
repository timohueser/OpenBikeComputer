//! Byte I/O abstractions for the route format.
//!
//! These live in the format-neutral `obc-formats` foundation crate. This module re-exports them
//! so the route code's `crate::byte_io::{…}` paths and the public
//! `obc_route::{ByteSource, ByteSink, SliceSource, Error}` paths remain unchanged.
//! Remove in the #812 final audit.

pub use obc_formats::io::{ByteSink, ByteSource, Error, SliceSource};
