//! "Verify the output actually opens with `obc-reader`" — meant literally.
//!
//! A header sniff would pass on a file that is a valid 40-byte header followed by
//! garbage, and that is exactly the artifact a killed packer, a full disk, or a
//! truncated copy leaves behind. So verification runs the **real reader**, the same
//! crate the device runs, over the **whole** artifact: parse the tables, then walk
//! every LOD's quadtree and decode every feature in every chunk it reaches.
//!
//! Two things make that a real gate rather than a smoke test:
//!
//! - [`DecodeStatus`] is checked, not just the `Result`. The reader is written to
//!   survive a corrupt map on a rider's SD card — it consumes an undecodable
//!   feature whole and keeps going, counting it. So a walk that "succeeded" can
//!   still have skipped a thousand malformed features, and only the counters say
//!   so. Any `malformed` or `capacity_dropped` fails the artifact: the packer
//!   validates its chunk size against the reader's cap
//!   ([`obc_pack::serialize::validate_chunk_size`]), so neither can be a legitimate
//!   outcome of a good bake.
//! - The read goes through a **file-backed** [`ByteSource`], not a slice. A country
//!   artifact is hundreds of megabytes; verifying it must not need it resident, and
//!   reading it through `read_at` is also closer to how the device sees it (small
//!   positioned reads through a cache) than a `Vec<u8>` would be.
//!
//! Verification happens on the temporary file, **before** it is renamed into the
//! bake tree, so a failed artifact never exists at a path the catalog generator
//! walks. That is the mechanism behind "a corrupted artifact never reaches the
//! manifest": not a check the publisher performs, but a file that was never there.

use std::cell::RefCell;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use obc_formats::io::{ByteSource, Error as IoError};
use obc_reader::{BBox, MapCache, MapTables, Reader, MAX_FEAT_PTS, MAX_FEAT_RINGS};

/// What a verified artifact turned out to contain. Logged per artifact so a bake
/// that produces a *readable but empty* map is still visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verified {
    pub obcm_version: u8,
    pub bbox: BBox,
    pub lods: usize,
    pub chunks: u64,
    pub features: u64,
    /// POI categories with content (spec §7.4 ids 1..=6).
    pub poi_categories: usize,
    pub has_nav_graph: bool,
}

/// Open `path` with the real reader and walk all of it.
pub fn verify(path: &Path) -> Result<Verified, String> {
    let src = FileSource::open(path)?;
    let tables = MapTables::parse(&src).map_err(|e| format!("{}: not a readable OBCM map: {e:?}", path.display()))?;
    // The cache is ~277 KB — heap, never the stack (`alloc` feature).
    let cache = MapCache::new_boxed();
    let reader = Reader::new(&src, &tables, &cache);

    if reader.version != obc_formats::obcm::VERSION {
        return Err(format!(
            "{}: OBCM v{} but this build writes v{} — the artifact is stale",
            path.display(),
            reader.version,
            obc_formats::obcm::VERSION
        ));
    }
    let bbox = reader.bbox;
    if bbox.min_lon > bbox.max_lon || bbox.min_lat > bbox.max_lat {
        return Err(format!("{}: inside-out header bbox {bbox:?}", path.display()));
    }

    let mut chunks_total = 0u64;
    let mut features_total = 0u64;
    let mut points = heapless::Vec::<_, MAX_FEAT_PTS>::new();
    let mut ring_lens = heapless::Vec::<_, MAX_FEAT_RINGS>::new();
    for lod in 0..reader.lods().len() {
        // Collect first: `for_each_feature` borrows the same cache the walk does.
        let mut chunks: Vec<(u32, BBox)> = Vec::new();
        reader
            .for_each_chunk(lod, &bbox, |id, node| chunks.push((id, node)))
            .map_err(|e| format!("{}: LOD{lod} index walk failed: {e:?}", path.display()))?;
        chunks_total += chunks.len() as u64;
        for (chunk_id, node) in chunks {
            let status = reader
                .for_each_feature(lod, chunk_id, &node, &mut points, &mut ring_lens, |_| {})
                .map_err(|e| format!("{}: LOD{lod} chunk {chunk_id} unreadable: {e:?}", path.display()))?;
            if status.malformed > 0 || status.capacity_dropped > 0 {
                return Err(format!(
                    "{}: LOD{lod} chunk {chunk_id} decoded {} features but dropped {} malformed and {} oversized — \
                     the artifact is corrupt",
                    path.display(),
                    status.complete,
                    status.malformed,
                    status.capacity_dropped
                ));
            }
            features_total += u64::from(status.complete);
        }
    }
    if features_total == 0 {
        return Err(format!(
            "{}: reads cleanly but contains no features — refusing to publish an empty map",
            path.display()
        ));
    }

    Ok(Verified {
        obcm_version: reader.version,
        bbox,
        lods: reader.lods().len(),
        chunks: chunks_total,
        features: features_total,
        poi_categories: reader.poi_directory().entries.iter().filter(|e| !e.is_empty()).count(),
        has_nav_graph: tables.has_nav_graph(),
    })
}

/// A [`ByteSource`] over an open file: positioned reads, nothing resident.
///
/// `RefCell` because `ByteSource::read_at` takes `&self` (the device's sources are
/// interior-mutable too) and verification is single-threaded.
struct FileSource {
    file: RefCell<File>,
    len: u32,
}

impl FileSource {
    fn open(path: &Path) -> Result<Self, String> {
        let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let len = file.metadata().map_err(|e| format!("{}: {e}", path.display()))?.len();
        // The format addresses bytes with u32 offsets, so a >4 GB artifact is not a
        // map the reader could ever open — say so here rather than truncating.
        let len = u32::try_from(len)
            .map_err(|_| format!("{}: {len} bytes exceeds the format's 4 GB limit", path.display()))?;
        Ok(Self { file: RefCell::new(file), len })
    }
}

impl ByteSource for FileSource {
    fn read_at(&self, offset: u32, buf: &mut [u8]) -> Result<(), IoError> {
        let end = (offset as u64).checked_add(buf.len() as u64).ok_or(IoError::BadOffset)?;
        if end > u64::from(self.len) {
            return Err(IoError::BadOffset);
        }
        let mut file = self.file.try_borrow_mut().map_err(|_| IoError::Io)?;
        file.seek(SeekFrom::Start(u64::from(offset))).map_err(|_| IoError::Io)?;
        file.read_exact(buf).map_err(|_| IoError::Io)
    }

    fn len(&self) -> u32 {
        self.len
    }
}
