//! Compatibility paths for the neutral byte-I/O seam now owned by `obc-formats`.
//!
//! Existing `obc_reader::byte_io::*` and crate-root re-exports remain source-compatible.
//! Remove in the #812 final audit.

pub use obc_formats::io::{ByteSink, ByteSource, Error, SliceSource};
