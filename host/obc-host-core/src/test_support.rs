//! Small oracles other suites borrow when they exercise a map through this crate.
//!
//! Compiled for this crate's own tests, and for a dependent that asks for the `test-support`
//! feature from its `[dev-dependencies]`. Nothing here is in a host binary.

use obc_formats::io::{ByteSource, Error};
use std::cell::Cell;

/// A [`ByteSource`] that counts what the map reader asked of it.
///
/// It is the "the map was really read" oracle: a frame drawn from a mounted card has to fetch its
/// chunks through the card, so a zero read count means the renderer drew from something else.
pub struct CountedSource<'a> {
    source: &'a dyn ByteSource,
    reads: Cell<usize>,
    bytes: Cell<usize>,
}

impl<'a> CountedSource<'a> {
    pub fn new(source: &'a dyn ByteSource) -> Self {
        Self { source, reads: Cell::new(0), bytes: Cell::new(0) }
    }

    /// How many `read_at` calls have gone through.
    pub fn reads(&self) -> usize {
        self.reads.get()
    }

    /// How many bytes those calls asked for.
    pub fn bytes(&self) -> usize {
        self.bytes.get()
    }
}

impl ByteSource for CountedSource<'_> {
    fn len(&self) -> u64 {
        self.source.len()
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        self.reads.set(self.reads.get() + 1);
        self.bytes.set(self.bytes.get() + buf.len());
        self.source.read_at(offset, buf)
    }
}
