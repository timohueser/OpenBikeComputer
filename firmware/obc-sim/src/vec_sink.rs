//! A `ByteSink` collecting OBCR/GPX output into a growable `Vec` before one `fs::write`.
//!
//! Both the route store (GPX → OBCR) and the track store (`.obct` log → GPX) convert in
//! memory and write once; a host conversion is a few MB at most. Native-only — the web
//! build has no filesystem to write to.

use obc_route::{ByteSink, Error};

#[derive(Default)]
pub struct VecSink {
    buf: Vec<u8>,
}

impl VecSink {
    /// The collected bytes, ready to hand to `fs::write`.
    pub fn bytes(&self) -> &[u8] {
        &self.buf
    }
}

impl ByteSink for VecSink {
    fn write(&mut self, b: &[u8]) -> Result<(), Error> {
        self.buf.extend_from_slice(b);
        Ok(())
    }
    fn patch_at(&mut self, off: u32, b: &[u8]) -> Result<(), Error> {
        let o = off as usize;
        let end = o.checked_add(b.len()).ok_or(Error::BadOffset)?;
        if end > self.buf.len() {
            return Err(Error::BadOffset);
        }
        self.buf[o..end].copy_from_slice(b);
        Ok(())
    }
}
