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

/// POI directory categories (spec §7.1): services 1..6 and optional summit landmarks 7.
/// The parsed `MapTables::pois`
/// bounds its `heapless::Vec` at this so a corrupt `category_count` can't request an unbounded
/// allocation; a directory declaring more categories than this is rejected.
pub const POI_MAX_CATEGORIES: usize = 8;

/// Upper bound on the POI `chunk_size` the reader accepts (spec §7.1). POI records are a fixed 32
/// bytes and the packer writes 512-byte chunks (16 records); this caps the on-wire `u16` well below
/// the geometry [`super::MAX_CHUNK_BYTES`] so a corrupt directory can't advertise a huge chunk the
/// nearest-N query (#424) would try to buffer. Generous headroom over the packer's 512 without
/// approaching the geometry scratch.
pub const POI_MAX_CHUNK_BYTES: usize = 4096;

/// Max results the nearest-N POI query returns (locked on epic #115). The caller owns a
/// `heapless::Vec<Poi, MAX_POI_RESULTS>`; the query fills it ascending by distance and never
/// exceeds it. 16 × ≈36 B ≈ 600 B, on the caller's stack.
pub const MAX_POI_RESULTS: usize = 16;

/// The POI-scan stack scratch window, in bytes (spec §7.1's default chunk size, 8 records of 64 =
/// 504 bytes, plus a few slack bytes). One chunk streams through this fixed window at a time
/// regardless of the accepted `chunk_size`, so the query's scratch stays tiny (no `MapCache`
/// growth). Each read pulls a whole number of records (`take * POI_RECORD_LEN`), so a record never
/// straddles two reads.
const POI_SCAN_WINDOW: usize = 512;

/// One category's entry in the parsed POI directory (spec §7.1). The nearest-N query (#424)
/// walks this category's quadtree exactly as it walks a
/// [`super::Lod`] index — the layout is shared, so its `data_start`/`chunk_range` math
/// reuses the same convention.
#[derive(Debug, Clone, Copy)]
pub struct PoiCatEntry {
    /// Canonical category id (services 1..6, summit landmarks 7; spec §7.4).
    pub category_id: u8,
    /// Byte offset to this category's quadtree index.
    pub index_offset: u64,
    /// Number of `uint32` nodes in the index; `0` ⇒ the category is empty in this map.
    pub node_count: usize,
    /// Number of POI data chunks in this category.
    pub chunk_count: usize,
    /// This file's offset unit (§1.1), carried so the category's chunk start is rounded with the
    /// scale of the file the entry was read from.
    pub scale: OffsetScale,
}

impl PoiCatEntry {
    /// This category is empty in this map (no quadtree, no chunks).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.node_count == 0
    }

    /// Byte offset where this category's data chunks begin (right after its index),
    /// or `None` if the arithmetic overflows `u64` (a corrupt directory) — the
    /// shared §7.1 convention, see `index_end`.
    #[inline]
    pub fn data_start(&self) -> Option<u64> {
        aligned_index_end(self.scale, self.index_offset, self.node_count)
    }

    /// Byte range `[start, end)` of POI chunk `chunk_id` given the directory's shared `chunk_size`
    /// (the §7.1 chunk size is directory-wide, not per-entry, so it's passed in). See
    /// [`fixed_chunk_range`].
    #[inline]
    pub(super) fn chunk_range(&self, chunk_id: u32, chunk_size: usize) -> Option<(u64, u64)> {
        fixed_chunk_range(self.data_start(), self.chunk_count, chunk_size, chunk_id)
    }
}

/// A single POI result from [`Reader::nearest_pois`]. Coordinates are absolute microdegrees (§7.3);
/// `distance_m` is the ground distance from the query position, computed during the scan. `name` is
/// empty for an unnamed POI — the app then shows the subtype's fallback label
/// ([`poi_label_of`](obc_formats::obcm::poi_label_of)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Poi {
    pub opening: crate::hours::OpeningStatus,
    pub metadata: obc_formats::obcm::PoiMetadata,
    pub lat: i32,
    pub lon: i32,
    /// Canonical subtype id (§7.4), always in `1..=18` for a returned POI.
    pub subtype: u8,
    /// Stored name (≤ [`POI_NAME_LEN`] bytes); empty ⇒ unnamed.
    pub name: heapless::String<POI_NAME_LEN>,
    /// 0-based index into the hours pool (§7.5), decoded from record bytes `[34..36]`; `0xFFFF` = no
    /// hours. Carried into the detail screen (#444) so it can resolve the schedule via
    /// [`Reader::poi_hours`] without re-running the query.
    pub hours_ref: u16,
    /// Ground distance from the query position, rounded to whole meters.
    pub distance_m: u32,
}

/// The parsed POI directory (spec §7.1): the shared chunk size, one bounded entry per category, and
/// (v7) the hours-pool section's absolute offset + blob count. [`super::MapTables::parse`] fills it; the
/// nearest-N query walks each `entries[i]`'s quadtree, and the hours fields locate the pool for the
/// P3 (#443) per-POI hours lookup + open-now evaluation — parse-only here, the pool bytes are just
/// bounds-validated to lie in-file.
#[derive(Debug, Clone)]
pub struct PoiDirectory {
    /// Fixed capacity (bytes) of every POI chunk, shared by all categories (spec §7.1).
    pub chunk_size: usize,
    /// One entry per category present in the directory (bounded at [`POI_MAX_CATEGORIES`]).
    pub entries: Vec<PoiCatEntry, POI_MAX_CATEGORIES>,
    /// Absolute byte offset of the hours-pool section (spec §7.5): a `count u16` then `count ×
    /// 29-byte` blobs. Blob `i` (a record's `hours_ref`) lives at `hours_pool_offset + 2 + i*29`.
    /// Meaningful only when `hours_pool_count > 0`.
    pub hours_pool_offset: u64,
    /// Number of 29-byte blobs in the hours pool (spec §7.5); `0` ⇒ no hours in this map. Equals the
    /// `count u16` written at `hours_pool_offset`, validated equal at parse.
    pub hours_pool_count: usize,
}

impl PoiDirectory {
    /// A directory with nothing in it — no categories, no chunks, no hours pool. The POI twin of
    /// [`super::NavDirectory::EMPTY`], and the base an assembler builds a real one onto.
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
    /// The parsed POI directory (spec §7): the shared chunk size, one entry per category, and the
    /// v7 hours-pool offset/count. Always present (seven categories, some possibly empty).
    /// [`Reader::nearest_pois`] walks the per-category quadtrees; P3 (#443) reads
    /// [`PoiDirectory::hours_pool_offset`]/[`PoiDirectory::hours_pool_count`] to resolve a POI's
    /// pooled schedule.
    #[inline]
    pub fn poi_directory(&self) -> &PoiDirectory {
        &self.tables.pois
    }

    /// Resolve a POI's pooled weekly schedule (spec §7.5) from its `hours_ref`. `None` for the
    /// no-hours sentinel `0xFFFF`, an index `>= hours_pool_count`, or any read/decode failure — so a
    /// corrupt directory (a bad `hours_pool_offset`/`count`) or a flaky read yields `None`, never a
    /// panic/UB. On-demand: the detail screen (#444) calls this once with the [`Poi::hours_ref`] the
    /// list snapshot carried; it reads the single 29-byte blob into a **stack** buffer via
    /// [`ByteSource::read_at`] (no [`super::MapCache`] growth, no static/`.bss` buffer).
    ///
    /// Blob `hours_ref` lives at `hours_pool_offset + 2 + hours_ref*29` (the `+2` skips the pool's
    /// `count u16`). Every step is checked 32-bit so a corrupt offset/count can't wrap or read past
    /// the file.
    ///
    /// # Reentrancy
    ///
    /// Unlike [`Reader::nearest_pois`], this does **not** touch the [`super::MapCache`] — it's a plain
    /// stack read, safe to call from anywhere (including inside a `for_each_*` callback).
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

    /// Synchronous first-page adapter. Interactive callers use `PlaceQuery` to bound work,
    /// supply current hours, and continue beyond this page.
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
    /// chunk through `scan`, which is handed the chunk's byte offset and the per-chunk record cap.
    /// The shared skeleton behind both POI queries — the expanding-ring
    /// [`nearest_pois`](Reader::nearest_pois) pass and the per-route-chunk
    /// [`corridor_pois`](Reader::corridor_pois) pass — which differ only in what they do with a
    /// record.
    ///
    /// The chunk decode runs **inside** the walk callback: `walk_leaves` releases its index-cache
    /// borrow before invoking the callback, and the POI chunk read goes through a plain
    /// `src.read_at` stack scratch (never the `MapCache`), so the two never nest — and the pass is
    /// truly streaming with **no per-leaf buffer**, so an exhaustive (map-covering) final pass can't
    /// silently drop a leaf however dense the category. A leaf whose chunk id is out of range or
    /// whose extent runs past EOF is skipped; the first read failure stops the walk and is replayed
    /// as the return value (a `walk_leaves` callback cannot itself fail).
    pub(super) fn scan_poi_leaves(
        &self,
        entry: &PoiCatEntry,
        chunk_size: usize,
        search: &BBox,
        mut scan: impl FnMut(u64, usize) -> Result<(), IoError>,
    ) -> Result<(), Error> {
        // The whole chunk's record count. A chunk with no sentinel room (records × 32 == chunk_size)
        // is bounded by this count instead (mirrors `for_each_feature_filtered`).
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

    /// Stream one POI chunk's records through a single **512-byte** stack scratch — `POI_SCAN_WINDOW`
    /// bytes (16 records) at a time — handing each *valid* record to `visit` as
    /// `(window, record offset, lat, lon, subtype)`; the window slice stays borrowed so the caller
    /// can pull the name/hours fields out of it without a copy. Reading in a fixed window keeps the
    /// scratch tiny regardless of the accepted `chunk_size` (up to `POI_MAX_CHUNK_BYTES`);
    /// `POI_RECORD_LEN` divides the window so a record never straddles two reads. `start` is the
    /// chunk's byte offset, already bounds-checked by the caller. Terminates on the `0xFF` subtype
    /// sentinel or after `record_cap` records (a sentinel-less full chunk).
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
                half_width_m: crate::corridor::CORRIDOR_HALF_WIDTH_M as u16,
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

/// Decode a POI record's name (spec §7.3) from `buf` at record offset `off`: `name_len` at `off+9`,
/// the up-to-24-byte `Name` at `off+10` (bytes `[off+10 .. off+34]`; `hours_ref` follows at
/// `[off+34 .. off+36]`). Empty for an unnamed record (`name_len == 0`). The stored name is already
/// pre-folded printable ASCII, but this stays defensive — `name_len` is clamped to what the field
/// and the buffer hold, and any non-printable byte (a corrupt record) is dropped — so a bad chunk
/// yields a short/empty name, never a panic or garbage glyph.
pub(super) fn decode_poi_name(buf: &[u8], off: usize) -> heapless::String<POI_NAME_LEN> {
    let mut name = heapless::String::new();
    let name_off = off + 10;
    // Clamp to the 24-byte field and to the bytes actually present in the buffer.
    let len = (buf[off + 9] as usize).min(POI_NAME_LEN).min(buf.len().saturating_sub(name_off));
    for &b in &buf[name_off..name_off + len] {
        // Printable ASCII only (the device font's range); drop anything else rather than trust a
        // corrupt byte. `push` can't fail — `len <= POI_NAME_LEN` == the String capacity.
        if (0x20..=0x7E).contains(&b) {
            let _ = name.push(b as char);
        }
    }
    name
}

/// Parse the POI directory (spec §7.1) at `offset` from `src` (file is `total` bytes): the count
/// byte, the shared `chunk_size`, one 13-byte entry per category, then (v7) the `hours_pool_offset
/// u32` + `hours_pool_count u16`. Parse-only — validates the directory layout, each category's
/// index/chunk region, and that the hours-pool region lies in-file, but does **not** walk the trees
/// or decode any blob (the nearest-N query and the P3 (#443) hours lookup do). The directory is
/// always present, so `offset` at/past EOF, a `category_count` past [`POI_MAX_CATEGORIES`], a
/// `chunk_size` past [`POI_MAX_CHUNK_BYTES`], an out-of-file index/chunk region, or an out-of-file
/// hours-pool region is a corrupt header ⇒ [`Error::BadOffset`].
///
/// Every offset/length product is checked (32-bit target): a corrupt `node_count`/`chunk_count`/
/// `hours_pool_count` can wrap `u64`, so the region-end could land below `total` and admit a
/// category (or a pool blob) indexing out of the file — the same overflow guard style as
/// [`super::parse_lod_table`]/[`Reader::chunk_range`].
pub(super) fn parse_poi_directory(
    src: &dyn ByteSource,
    scale: OffsetScale,
    offset: u64,
    total: u64,
) -> Result<PoiDirectory, Error> {
    // The lowest byte a scaled offset in this file can name past the header (§1.2).
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
        // An empty category (node_count 0) still carries an entry; its index/chunk region is
        // zero-length, so only the offset itself needs to be in-file. A populated one must have its
        // whole index + chunk region inside the file — checked, so a corrupt count can't wrap past
        // `total`.
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

    // The two v7 hours-pool directory fields (spec §7.5): the section's absolute offset + blob
    // count. When the count is non-zero, the whole pool region (`count u16` + `count × 29-byte`
    // blobs) must lie in-file — checked, so a corrupt count can't wrap `u64` past `total`. An
    // empty pool (count 0) still validates its 2-byte `count` header lies in-file.
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
