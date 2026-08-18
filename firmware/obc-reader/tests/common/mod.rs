//! Shared helpers for the `obc-reader` integration tests.
//!
//! The read-counting [`ByteSource`] and the "decode a whole chunk into owned features" collector
//! were copy-pasted across `extremes.rs`, `format.rs`, `poi_corridor.rs` and `volume_set.rs`; this
//! module is the single source. Fixtures that are genuinely about one suite (a fault-injecting
//! source, a route-path fixture) stay in that suite. Not every test uses every helper, so
//! `#[allow(dead_code)]` keeps the unused-per-binary ones quiet.

#![allow(dead_code)]

use std::cell::Cell;

use obc_formats::io::{ByteSource, Error as IoError};
use obc_map_scene::{BBox, Kind};
use obc_reader::{DecodeStatus, Reader, SliceSource, MAX_FEAT_PTS, MAX_FEAT_RINGS};

/// A [`ByteSource`] that counts the `read_at` calls it serves and the bytes they move — the SD-read
/// proxy the cost conversation runs on (the device reads one block per `read_at`), and the way a
/// test asserts which *files* a query touched: the observable §5.6 property of a volume set is the
/// **absence** of I/O, not a return value.
pub struct CountingSource<'a> {
    inner: SliceSource<'a>,
    pub reads: Cell<u32>,
    pub bytes: Cell<u64>,
}

impl<'a> CountingSource<'a> {
    pub fn new(bytes: &'a [u8]) -> CountingSource<'a> {
        CountingSource { inner: SliceSource(bytes), reads: Cell::new(0), bytes: Cell::new(0) }
    }

    /// The read count since the last `take`, zeroing it — so a test can bracket one query without
    /// subtracting a baseline. The byte counter is left alone.
    pub fn take(&self) -> u32 {
        let count = self.reads.get();
        self.reads.set(0);
        count
    }
}

impl ByteSource for CountingSource<'_> {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), IoError> {
        self.reads.set(self.reads.get() + 1);
        self.bytes.set(self.bytes.get() + buf.len() as u64);
        self.inner.read_at(offset, buf)
    }
    fn len(&self) -> u64 {
        self.inner.len()
    }
}

/// One decoded feature, owned — the borrowing `FeatureRef` the reader yields only lives for the
/// callback, so a test that wants to assert over a whole chunk copies it out first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    pub style_id: u8,
    pub kind: Kind,
    pub exterior: Vec<(i32, i32)>,
    pub interiors: Vec<Vec<(i32, i32)>>,
    pub bbox: BBox,
}

impl Decoded {
    fn of(f: &obc_reader::FeatureRef<'_>) -> Decoded {
        Decoded {
            style_id: f.style_id,
            kind: f.kind,
            exterior: f.exterior().to_vec(),
            interiors: f.interiors().map(|h| h.to_vec()).collect(),
            bbox: f.bbox(),
        }
    }
}

/// Decode every feature in `(lod, chunk_id)`, using exactly the reader's scratch capacities.
pub fn decode_chunk(r: &Reader, lod: usize, chunk_id: u32, node: &BBox) -> Vec<Decoded> {
    decode_chunk_status(r, lod, chunk_id, node).0
}

/// [`decode_chunk`] plus the walk's drop tally, for the suites that assert *why* a feature is
/// missing rather than only that it is.
pub fn decode_chunk_status(r: &Reader, lod: usize, chunk_id: u32, node: &BBox) -> (Vec<Decoded>, DecodeStatus) {
    let mut out = Vec::new();
    let mut points = heapless::Vec::<_, MAX_FEAT_PTS>::new();
    let mut ring_lens = heapless::Vec::<_, MAX_FEAT_RINGS>::new();
    let status =
        r.for_each_feature(lod, chunk_id, node, &mut points, &mut ring_lens, |f| out.push(Decoded::of(&f))).unwrap();
    (out, status)
}

/// Like [`decode_chunk`] but only the features for which `keep(style_id)` is true are decoded and
/// returned; the rest are skipped in the reader.
pub fn decode_filtered(r: &Reader, lod: usize, chunk_id: u32, node: &BBox, keep: impl Fn(u8) -> bool) -> Vec<Decoded> {
    let mut out = Vec::new();
    let mut points = heapless::Vec::<_, MAX_FEAT_PTS>::new();
    let mut ring_lens = heapless::Vec::<_, MAX_FEAT_RINGS>::new();
    r.for_each_feature_filtered(lod, chunk_id, node, &mut points, &mut ring_lens, keep, |f| out.push(Decoded::of(&f)))
        .unwrap();
    out
}
