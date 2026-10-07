//! Ordered source folds over bounded, parallel blob decoding.

use obc_map_core::progress::Progress;
use osmpbf::{Blob, BlobReader, BlobType, ByteOffset, Element};
use rayon::prelude::*;

/// What a blob scan should do after the element it was just handed.
pub enum Scan {
    Continue,
    /// Stop here. [`scan_blobs`] returns the offset of the blob this element came from, so a later
    /// pass can resume at exactly this point.
    StopAtThisBlob,
}

/// Stream a `.pbf`'s data blobs — from `start`, or from the beginning — handing every element to
/// `f`, and return the offset of the blob the scan stopped in (`None` if it ran to the end).
///
/// This is `ElementReader::for_each` plus the ability to stop and to resume. A sorted PBF stores
/// nodes, then ways, then relations, and the node section is about 85 % of the bytes, so a pass that
/// only wants ways skips straight to them. The blob boundary is also the ingest's cancellation
/// checkpoint, and every reading pass goes through here.
///
/// Blobs are decoded on the rayon pool a chunk at a time, but `f` is a stateful fold that must see
/// elements in file order, so the decoded blocks reach it one after another in that order and
/// memory stays bounded by the chunk. A chunk can overshoot a [`Scan::StopAtThisBlob`]: that work
/// is discarded unhandled, together with any read or decode error inside it, so a stopping scan
/// succeeds or fails exactly as a sequential one would.
pub fn scan_blobs<F>(
    path: &str,
    start: Option<ByteOffset>,
    progress: &Progress,
    mut f: F,
) -> Result<Option<ByteOffset>, String>
where
    F: FnMut(Element) -> Scan,
{
    let mut reader = BlobReader::seekable_from_path(path).map_err(|e| format!("open {path}: {e}"))?;
    if let Some(pos) = start {
        reader.seek(pos).map_err(|e| format!("seek {path}: {e}"))?;
    }
    let chunk = chunk_len();
    let mut raw: Vec<Blob> = Vec::with_capacity(chunk);
    loop {
        // A read error ends the chunk but is only reported after the blobs before it are handled,
        // and not at all if the scan stops first — matching the lazy sequential reader.
        raw.clear();
        let mut read_err: Option<String> = None;
        while raw.len() < chunk {
            match reader.next() {
                // The header blob carries no elements; only OSMData blocks do.
                Some(Ok(blob)) if matches!(blob.get_type(), BlobType::OsmData) => raw.push(blob),
                Some(Ok(_)) => {}
                Some(Err(e)) => {
                    read_err = Some(format!("read {path}: {e}"));
                    break;
                }
                None => break,
            }
        }

        // Decode in parallel; an error surfaces below, only if the scan reaches the failing blob.
        progress.check()?;
        let blocks: Vec<_> = raw.par_iter().map(|b| (b.offset(), b.to_primitiveblock())).collect();

        for (offset, block) in blocks {
            progress.check()?;
            let block = block.map_err(|e| format!("decode {path}: {e}"))?;
            for el in block.elements() {
                if let Scan::StopAtThisBlob = f(el) {
                    return Ok(offset);
                }
            }
        }
        if let Some(e) = read_err {
            return Err(e);
        }
        if raw.len() < chunk {
            return Ok(None);
        }
    }
}

/// How many raw blobs one [`scan_blobs`] chunk holds: enough to keep every rayon worker busy
/// through a decode round, small enough that the in-flight blobs stay tens of megabytes.
fn chunk_len() -> usize {
    2 * rayon::current_num_threads().max(1)
}

/// Run `f` over every source in parallel, collecting the results in source order. Each pass reads
/// each file independently, and only the fold that combines them has to be ordered.
pub fn par_sources<T, F>(paths: &[String], f: F) -> Result<Vec<T>, String>
where
    T: Send,
    F: Fn(usize, &str) -> Result<T, String> + Sync,
{
    paths.par_iter().enumerate().map(|(i, p)| f(i, p.as_str())).collect()
}

/// Per-element output tagged with the id of the OSM object that produced it.
///
/// The tag is what lets several `.pbf`s be read independently and still come out as one merged file
/// would have produced them: later copies of an already-seen object dropped
/// ([`Keyed::retain_keys`]), everything back in id order ([`Keyed::sort`]). With a single source
/// nothing is tagged and this is a plain `Vec<T>`, so an uncropped country pack pays nothing.
pub struct Keyed<T> {
    tagged: bool,
    keys: Vec<i64>,
    items: Vec<T>,
}

impl<T> Keyed<T> {
    pub fn new(tagged: bool) -> Self {
        Keyed { tagged, keys: Vec::new(), items: Vec::new() }
    }

    #[inline]
    pub fn push(&mut self, key: i64, item: T) {
        if self.tagged {
            self.keys.push(key);
        }
        self.items.push(item);
    }

    /// Concatenate a later source's outputs onto this one.
    pub fn append(&mut self, mut other: Self) {
        self.keys.append(&mut other.keys);
        self.items.append(&mut other.items);
    }

    /// Drop every item whose key `keep` rejects, preserving order. Tagged only: it is a merge
    /// operation and never runs on a single-source ingest.
    pub fn retain_keys(&mut self, mut keep: impl FnMut(i64) -> bool) {
        debug_assert!(self.tagged && self.keys.len() == self.items.len());
        let mut w = 0;
        for r in 0..self.items.len() {
            if keep(self.keys[r]) {
                if w != r {
                    self.keys.swap(w, r);
                    self.items.swap(w, r);
                }
                w += 1;
            }
        }
        self.keys.truncate(w);
        self.items.truncate(w);
    }

    /// Put the items back in ascending-id order — the order a merged, sorted `.pbf` would have
    /// handed them to the same pass.
    ///
    /// The sort is stable, so a file that repeats an id inside itself keeps its own order instead
    /// of picking one arbitrarily. The already-sorted check keeps the transient pair vector — the
    /// only copy of the payload this merge makes — out of the common case.
    pub fn sort(&mut self) {
        debug_assert!(self.tagged && self.keys.len() == self.items.len());
        if self.keys.is_sorted() {
            return;
        }
        let keys = std::mem::take(&mut self.keys);
        let items = std::mem::take(&mut self.items);
        let mut pairs: Vec<(i64, T)> = keys.into_iter().zip(items).collect();
        pairs.sort_by_key(|(k, _)| *k);
        (self.keys, self.items) = pairs.into_iter().unzip();
    }

    pub fn keys(&self) -> &[i64] {
        &self.keys
    }

    pub fn items(&self) -> &[T] {
        &self.items
    }

    pub fn into_items(self) -> Vec<T> {
        self.items
    }
}
