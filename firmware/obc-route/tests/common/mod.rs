//! Shared helpers for the `obc-route` integration tests.
//!
//! The `VecSink` `ByteSink`, the GPX→OBCR `convert` helper, and the single-chunk
//! `decode` helper were copy-pasted across `convert.rs`, `format.rs`, `matcher.rs`,
//! `profile.rs` and `track.rs`; this module is the single source. Not every test uses
//! every helper, so `#[allow(dead_code)]` keeps the unused-per-binary ones quiet.

#![allow(dead_code)]

use obc_formats::io::{ByteSink, Error, SliceSource};
use obc_route::{RoutePoint, RouteReader, MAX_POINTS_PER_CHUNK};

/// A `ByteSink` over a growable `Vec` — the host's "write the whole file to RAM"
/// backing (the device uses a FatFs-backed sink instead).
#[derive(Default)]
pub struct VecSink {
    pub buf: Vec<u8>,
}

impl ByteSink for VecSink {
    fn write(&mut self, b: &[u8]) -> Result<(), Error> {
        self.buf.extend_from_slice(b);
        Ok(())
    }
    fn patch_at(&mut self, off: u32, b: &[u8]) -> Result<(), Error> {
        let o = off as usize;
        self.buf[o..o + b.len()].copy_from_slice(b);
        Ok(())
    }
}

/// Convert an in-memory GPX string to `.obcr` bytes via the public converter.
pub fn convert(name: &str, gpx: &str) -> Vec<u8> {
    let src = SliceSource(gpx.as_bytes());
    let mut sink = VecSink::default();
    obc_route::gpx_to_obcr(&src, name, &mut sink).unwrap();
    sink.buf
}

/// Decode chunk `k` of `r` to an owned point vector.
pub fn decode(r: &RouteReader, k: usize) -> Vec<RoutePoint> {
    let mut out = heapless::Vec::<_, MAX_POINTS_PER_CHUNK>::new();
    r.decode_chunk(k, &mut out).unwrap();
    out.to_vec()
}
