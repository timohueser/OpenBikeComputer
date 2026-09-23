//! POI directories, hours lookup, nearest/corridor queries, and streaming decode.

use super::{aligned_index_end, fixed_chunk_range, resolve, QuadIndex, Reader};
use crate::corridor::{CorridorPoi, PoiCategorySet, RoutePath, MAX_CORRIDOR_RESULTS};
use crate::Error;
use heapless::Vec;
use obc_formats::io::{rd_i32, rd_u16, rd_u32, ByteSource, Error as IoError};
use obc_formats::obcm::{
    OffsetScale, PoiCategory, CHUNK_END, HEADER_LEN, POI_CAT_ENTRY_LEN, POI_HOURS_BLOB_LEN, POI_HOURS_REF_NONE,
    POI_NAME_LEN, POI_RECORD_LEN,
};
use obc_map_scene::{cos_lat, ground_dist_m_cl, BBox};

/// POI directory categories: services 1..6 and 8, plus summit landmarks 7 and settlements 9. The
/// parsed directory bounds its `Vec` at this, so a corrupt `category_count` cannot request an
/// unbounded allocation.
pub const POI_MAX_CATEGORIES: usize = 9;

/// Upper bound on the POI `chunk_size` the reader accepts. The packer writes 512-byte chunks, so
/// this caps the on-wire `u16` well below the geometry [`super::MAX_CHUNK_BYTES`] and a corrupt
/// directory cannot advertise a huge chunk the nearest-N query would buffer.
pub const POI_MAX_CHUNK_BYTES: usize = 4096;

/// Max results the nearest-N POI query returns. The caller owns the `Vec`, and the query fills it
/// ascending by distance.
pub const MAX_POI_RESULTS: usize = 16;

/// The POI-scan stack scratch window, in bytes. One chunk streams through this fixed window at a
/// time whatever the accepted `chunk_size`, and each read pulls a whole number of records, so a
/// record never straddles two reads.
const POI_SCAN_WINDOW: usize = 512;

/// One category's entry in the parsed POI directory. The nearest-N query walks this category's
/// quadtree exactly as it walks a [`super::Lod`] index, so the same `data_start` and
/// `chunk_range` math serves both.
#[derive(Debug, Clone, Copy)]
pub struct PoiCatEntry {
    pub category_id: u8,
    /// Byte offset to this category's quadtree index.
    pub index_offset: u64,
    /// Number of `uint32` nodes in the index; `0` means the category is empty in this map.
    pub node_count: usize,
    /// Number of POI data chunks in this category.
    pub chunk_count: usize,
    /// This file's offset unit, carried so the category's chunk start is rounded with the scale
    /// of the file the entry came from.
    pub scale: OffsetScale,
}

impl PoiCatEntry {
    /// This category is empty in this map (no quadtree, no chunks).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.node_count == 0
    }

    /// Byte offset where this category's data chunks begin, right after its index, or `None` on
    /// `u64` overflow from a corrupt directory.
    #[inline]
    pub fn data_start(&self) -> Option<u64> {
        aligned_index_end(self.scale, self.index_offset, self.node_count)
    }

    /// Byte range `[start, end)` of POI chunk `chunk_id`. The chunk size is directory-wide, not
    /// per-entry, so it is passed in.
    #[inline]
    pub(super) fn chunk_range(&self, chunk_id: u32, chunk_size: usize) -> Option<(u64, u64)> {
        fixed_chunk_range(self.data_start(), self.chunk_count, chunk_size, chunk_id)
    }
}

/// A single POI result from [`Reader::nearest_pois`]. Coordinates are absolute microdegrees and
/// `distance_m` is the ground distance from the query position. An empty `name` is unnamed, and
/// the app then shows the subtype's fallback label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Poi {
    pub opening: crate::hours::OpeningStatus,
    pub metadata: obc_formats::obcm::PoiMetadata,
    pub lat: i32,
    pub lon: i32,
    pub subtype: u8,
    /// Stored name; empty means unnamed.
    pub name: heapless::String<POI_NAME_LEN>,
    /// 0-based index into the hours pool, or `0xFFFF` for no hours. Carried into the detail
    /// screen so it can resolve the schedule without re-running the query.
    pub hours_ref: u16,
    /// Ground distance from the query position, rounded to whole meters.
    pub distance_m: u32,
}

/// The parsed POI directory: the shared chunk size, one bounded entry per category, and the
/// hours-pool offset and blob count. Parse-only here; the pool bytes are bounds-validated to lie
/// in file.
#[derive(Debug, Clone)]
pub struct PoiDirectory {
    /// Fixed capacity in bytes of every POI chunk, shared by all categories.
    pub chunk_size: usize,
    /// One entry per category present in the directory (bounded at [`POI_MAX_CATEGORIES`]).
    pub entries: Vec<PoiCatEntry, POI_MAX_CATEGORIES>,
    /// Absolute byte offset of the hours-pool section: a `count u16`, then `count` 29-byte blobs,
    /// so blob `i` lives at `hours_pool_offset + 2 + i*29`. Meaningful only when the count is
    /// non-zero.
    pub hours_pool_offset: u64,
    /// Number of 29-byte blobs in the hours pool; `0` means no hours in this map. Validated equal
    /// to the `count u16` at `hours_pool_offset`.
    pub hours_pool_count: usize,
}

impl PoiDirectory {
    /// A directory with nothing in it. The POI twin of [`super::NavDirectory::EMPTY`], and the
    /// base an assembler builds a real one onto.
    pub const EMPTY: PoiDirectory =
        PoiDirectory { chunk_size: 0, entries: Vec::new(), hours_pool_offset: 0, hours_pool_count: 0 };
}

impl QuadIndex for PoiCatEntry {
    #[inline]
    fn index_offset(&self) -> u64 {
        self.index_offset
    }
    #[inline]
    fn node_count(&self) -> usize {
        self.node_count
    }
}

/// A map overlay mark. Names and opening hours are deliberately not loaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MapPoint {
    pub position: (i32, i32),
    pub source: obc_formats::obcm::SourceId,
    pub subtype: u8,
    pub elevation_m: Option<i16>,
}

impl<'a> Reader<'a> {
    /// The parsed POI directory: the shared chunk size, one entry per category, and the
    /// hours-pool offset and count. Always present, with some categories possibly empty.
    #[inline]
    pub fn poi_directory(&self) -> &PoiDirectory {
        &self.tables.pois
    }

    /// Resolve a POI's pooled weekly schedule from its `hours_ref`. `None` for the no-hours
    /// sentinel, an index past `hours_pool_count`, or any read or decode failure, so a corrupt
    /// directory yields `None` and never a panic.
    ///
    /// Blob `hours_ref` lives at `hours_pool_offset + 2 + hours_ref*29`, and every step is checked
    /// for the 32-bit target. It reads the single 29-byte blob into a stack buffer and touches no
    /// [`super::MapCache`], so it is safe to call from inside a `for_each_*` callback.
    pub fn poi_hours(&self, hours_ref: u16) -> Option<crate::hours::WeeklySchedule> {
        self.try_poi_hours(hours_ref).ok().flatten()
    }

    /// Checked schedule access for queries: absent hours are unknown; broken storage is an error.
    pub fn try_poi_hours(&self, hours_ref: u16) -> Result<Option<crate::hours::WeeklySchedule>, Error> {
        let dir = &self.tables.pois;
        if hours_ref == POI_HOURS_REF_NONE {
            return Ok(None);
        }
        if hours_ref as usize >= dir.hours_pool_count {
            return Err(Error::BadOffset);
        }
        let offset = (hours_ref as u64)
            .checked_mul(POI_HOURS_BLOB_LEN as u64)
            .and_then(|n| n.checked_add(2)?.checked_add(dir.hours_pool_offset))
            .ok_or(Error::BadOffset)?;
        if offset.checked_add(POI_HOURS_BLOB_LEN as u64).is_none_or(|end| end > self.src.len()) {
            return Err(Error::BadOffset);
        }
        let mut blob = [0; POI_HOURS_BLOB_LEN];
        self.src.read_at(offset, &mut blob).map_err(Error::Source)?;
        crate::hours::WeeklySchedule::decode(&blob).map(Some).ok_or(Error::BadOffset)
    }

    /// Synchronous first-page adapter. Interactive callers use `PlaceQuery` to bound work and
    /// continue beyond this page.
    pub fn nearest_pois(
        &self,
        category: PoiCategory,
        pos: (i32, i32),
        out: &mut Vec<Poi, MAX_POI_RESULTS>,
    ) -> Result<(), Error> {
        let radius_m = [
            (self.bbox.min_lon, self.bbox.min_lat),
            (self.bbox.max_lon, self.bbox.max_lat),
            (self.bbox.min_lon, self.bbox.max_lat),
            (self.bbox.max_lon, self.bbox.min_lat),
        ]
        .into_iter()
        .map(|p| ground_dist_m_cl(pos, p, cos_lat(pos.1)) as u32)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
        let mut query = super::places::PlaceQuery::new(
            0,
            PoiCategorySet::only(category),
            super::places::PlaceWindow::Nearby { position: pos, radius_m },
            None,
        );
        let mut page = Vec::<CorridorPoi, MAX_POI_RESULTS>::new();
        loop {
            match query.step(self, None, 0, &mut page) {
                super::places::QueryProgress::Pending => {}
                super::places::QueryProgress::Failed(error) => {
                    out.clear();
                    return Err(error);
                }
                _ => break,
            }
        }
        out.clear();
        for hit in page {
            let _ = out.push(hit.poi);
        }
        Ok(())
    }

    /// Walk `entry`'s quadtree for leaves overlapping `search` and stream every non-empty leaf's
    /// chunk through `scan`, which gets the chunk's byte offset and the per-chunk record cap. The
    /// shared skeleton behind both POI queries, which differ only in what they do with a record.
    ///
    /// The chunk decode runs inside the walk callback: `walk_leaves` releases its index-cache
    /// borrow first, and the POI chunk read goes through a plain stack scratch rather than the
    /// `MapCache`, so the two never nest and the pass needs no per-leaf buffer. A leaf whose chunk
    /// id is out of range or whose extent runs past EOF is skipped; the first read failure stops
    /// the walk and is replayed as the return value.
    pub(super) fn scan_poi_leaves(
        &self,
        entry: &PoiCatEntry,
        chunk_size: usize,
        search: &BBox,
        mut scan: impl FnMut(u64, usize) -> Result<(), IoError>,
    ) -> Result<(), Error> {
        // The whole chunk's record count. A chunk with no sentinel room is bounded by this count
        // instead.
        let records_per_chunk = chunk_size / POI_RECORD_LEN;
        let mut read_error = None;
        self.walk_leaves(entry, 0, self.bbox, search, 0, &mut |cid, _node| {
            if read_error.is_some() {
                return;
            }
            let (start, end) = match entry.chunk_range(cid, chunk_size) {
                Some(r) => r,
                None => return,
            };
            if end > self.src.len() {
                return;
            }
            if let Err(error) = scan(start, records_per_chunk) {
                read_error = Some(error);
            }
        })
        .map_err(Error::from)?;
        if let Some(error) = read_error {
            return Err(Error::Source(error));
        }
        Ok(())
    }

    /// Stream one POI chunk's records through a single 512-byte stack scratch, handing each valid
    /// record to `visit` as `(window, record offset, lat, lon, subtype)`. The window slice stays
    /// borrowed, so the caller can pull the name and hours fields out of it without a copy.
    /// `POI_RECORD_LEN` divides the window, so a record never straddles two reads. Terminates on
    /// the `0xFF` subtype sentinel or after `record_cap` records.
    pub(super) fn stream_poi_records(
        &self,
        start: u64,
        record_cap: usize,
        mut visit: impl FnMut(&[u8], usize, i32, i32, u8),
    ) -> Result<(), IoError> {
        const RECS_PER_WINDOW: usize = POI_SCAN_WINDOW / POI_RECORD_LEN;
        let mut scratch = [0u8; POI_SCAN_WINDOW];
        let mut done = 0usize;
        while done < record_cap {
            let take = (record_cap - done).min(RECS_PER_WINDOW);
            let win = &mut scratch[..take * POI_RECORD_LEN];
            self.src.read_at(start + (done * POI_RECORD_LEN) as u64, win)?;
            for r in 0..take {
                let off = r * POI_RECORD_LEN;
                let subtype = win[off + 8];
                if subtype == CHUNK_END {
                    return Ok(()); // end-of-records sentinel — nothing valid follows in this chunk
                }
                visit(win, off, rd_i32(win, off), rd_i32(win, off + 4), subtype);
            }
            done += take;
        }
        Ok(())
    }

    /// Synchronous corridor first-page adapter over the shared continuable query.
    pub fn corridor_pois(
        &self,
        cats: PoiCategorySet,
        path: &dyn RoutePath,
        progress_m: u32,
        out: &mut Vec<CorridorPoi, MAX_CORRIDOR_RESULTS>,
    ) -> Result<(), Error> {
        let mut query = super::places::PlaceQuery::new(
            0,
            cats,
            super::places::PlaceWindow::Corridor {
                from_m: progress_m,
                to_m: u32::MAX,
                half_width_m: crate::corridor::CORRIDOR_HALF_WIDTH_M,
            },
            None,
        );
        out.clear();
        loop {
            match query.step(self, Some(path), 0, out) {
                super::places::QueryProgress::Pending => {}
                super::places::QueryProgress::Failed(error) => return Err(error),
                _ => return Ok(()),
            }
        }
    }
}

/// Decode a POI record's name from `buf` at record offset `off`: `name_len` at `off+9` and the
/// name at `off+10`. Empty for an unnamed record. The stored name is already pre-folded printable
/// ASCII, but this stays defensive: `name_len` is clamped to what the field and the buffer hold,
/// and a non-printable byte is dropped, so a bad chunk yields a short name and never a panic.
pub(super) fn decode_poi_name(buf: &[u8], off: usize) -> heapless::String<POI_NAME_LEN> {
    let mut name = heapless::String::new();
    let name_off = off + 10;
    // Clamp to the 24-byte field and to the bytes actually present in the buffer.
    let len = (buf[off + 9] as usize).min(POI_NAME_LEN).min(buf.len().saturating_sub(name_off));
    for &b in &buf[name_off..name_off + len] {
        // Printable ASCII only, the device font's range. `push` cannot fail, because
        // `len <= POI_NAME_LEN`, the String capacity.
        if (0x20..=0x7E).contains(&b) {
            let _ = name.push(b as char);
        }
    }
    name
}

/// Parse the POI directory at `offset` from `src`: the count byte, the shared `chunk_size`, one
/// 13-byte entry per category, then the `hours_pool_offset u32` and `hours_pool_count u16`.
/// Parse-only: it validates the layout, each category's index and chunk region, and that the
/// hours-pool region lies in file, but walks no tree and decodes no blob. The directory is always
/// present, so an `offset` at or past EOF, a count or `chunk_size` past its cap, or any
/// out-of-file region is [`Error::BadOffset`].
///
/// Every offset and length product is checked for the 32-bit target: a corrupt count can wrap
/// `u64`, and the region end could then land below `total` and admit a category indexing out of
/// the file.
pub(super) fn parse_poi_directory(
    src: &dyn ByteSource,
    scale: OffsetScale,
    offset: u64,
    total: u64,
) -> Result<PoiDirectory, Error> {
    // The lowest byte a scaled offset in this file can name past the header.
    let floor = scale.align_up(HEADER_LEN as u64).ok_or(Error::BadOffset)?;
    // The directory header is 3 bytes (count + chunk_size u16); it must fit the file.
    if offset < floor || offset.checked_add(3).is_none_or(|end| end > total) {
        return Err(Error::BadOffset);
    }
    let mut hdr = [0u8; 3];
    src.read_at(offset, &mut hdr).map_err(Error::Source)?;
    let category_count = hdr[0] as usize;
    let chunk_size = rd_u16(&hdr, 1) as usize;
    if category_count > POI_MAX_CATEGORIES || chunk_size > POI_MAX_CHUNK_BYTES {
        return Err(Error::BadOffset);
    }
    // The whole directory (header + entries + the two v7 pool fields) must lie within the file.
    let pool_fields_off = (category_count as u64)
        .checked_mul(POI_CAT_ENTRY_LEN as u64)
        .and_then(|len| offset.checked_add(3)?.checked_add(len))
        .ok_or(Error::BadOffset)?;
    // 4 (hours_pool_offset u32) + 2 (hours_pool_count u16) trail the per-category entries.
    let dir_end = pool_fields_off.checked_add(6).ok_or(Error::BadOffset)?;
    if dir_end > total {
        return Err(Error::BadOffset);
    }

    let mut entries = Vec::new();
    let mut e = [0u8; POI_CAT_ENTRY_LEN];
    for k in 0..category_count {
        let o = offset + 3 + (k * POI_CAT_ENTRY_LEN) as u64;
        src.read_at(o, &mut e).map_err(Error::Source)?;
        let entry = PoiCatEntry {
            category_id: e[0],
            index_offset: resolve(scale.offset(rd_u32(&e, 1))),
            node_count: rd_u32(&e, 5) as usize,
            chunk_count: rd_u32(&e, 9) as usize,
            scale,
        };
        // An empty category still carries an entry, and its index and chunk region are
        // zero-length, so only the offset itself must be in file. A populated one must have its
        // whole region inside the file, checked so a corrupt count cannot wrap past `total`.
        if entry.node_count > 0 {
            let region_end = entry
                .data_start()
                .and_then(|start| {
                    (entry.chunk_count as u64).checked_mul(chunk_size as u64).and_then(|len| start.checked_add(len))
                })
                .ok_or(Error::BadOffset)?;
            if entry.index_offset < floor || region_end > total {
                return Err(Error::BadOffset);
            }
        } else if entry.index_offset > total {
            return Err(Error::BadOffset);
        }
        let _ = entries.push(entry);
    }

    // The two hours-pool directory fields: the section's absolute offset and blob count. With a
    // non-zero count the whole pool region must lie in file, checked so a corrupt count cannot
    // wrap `u64`. An empty pool still validates its 2-byte header.
    let mut pf = [0u8; 6];
    src.read_at(pool_fields_off, &mut pf).map_err(Error::Source)?;
    let hours_pool_offset = resolve(scale.offset(rd_u32(&pf, 0)));
    let hours_pool_count = rd_u16(&pf, 4) as usize;
    if hours_pool_offset < floor {
        return Err(Error::BadOffset);
    }
    let pool_end = (hours_pool_count as u64)
        .checked_mul(POI_HOURS_BLOB_LEN as u64)
        .and_then(|blobs| hours_pool_offset.checked_add(2)?.checked_add(blobs))
        .ok_or(Error::BadOffset)?;
    if pool_end > total {
        return Err(Error::BadOffset);
    }

    Ok(PoiDirectory { chunk_size, entries, hours_pool_offset, hours_pool_count })
}
