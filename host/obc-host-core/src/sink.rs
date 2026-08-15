//! A `ByteSink` collecting OBCR/GPX output into a growable `Vec` before one write. Shared by the
//! hosts' route stores (GPX → OBCR, the nav router's emit) and `obc-sim`'s track store
//! (`.obct` log → GPX).

use obc_formats::io::{ByteSink, Error};

#[derive(Default)]
pub struct VecSink {
    buf: Vec<u8>,
}

impl VecSink {
    /// The collected bytes, ready to hand to `fs::write` (or to keep in memory on the web).
    pub fn bytes(&self) -> &[u8] {
        &self.buf
    }

    /// Take the collected bytes by value — the detour plan's held-until-commit handoff (#882).
    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `patch_at` rewrites inside the collected bytes and rejects any range past the end —
    /// the emitters rely on both (they patch headers after streaming the body).
    #[test]
    fn patch_at_rewrites_in_bounds_and_rejects_out_of_bounds() {
        let mut s = VecSink::default();
        s.write(&[1, 2, 3, 4]).unwrap();
        s.patch_at(1, &[9, 9]).unwrap();
        assert_eq!(s.bytes(), &[1, 9, 9, 4]);
        assert!(s.patch_at(3, &[0, 0]).is_err(), "range past the end");
        assert!(s.patch_at(u32::MAX, &[0]).is_err(), "offset overflow");
        assert_eq!(s.bytes(), &[1, 9, 9, 4], "a rejected patch changes nothing");
    }
}
