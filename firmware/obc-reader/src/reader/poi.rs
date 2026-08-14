//! POI directories, hours lookup, nearest/corridor queries, and streaming decode.

use super::{fixed_chunk_range, index_end, QuadIndex, Reader};
use crate::corridor::{
    inflate_bbox, project_onto_chunk, CorridorPoi, PoiCategorySet, RoutePath, CORRIDOR_HALF_WIDTH_M,
    MAX_CORRIDOR_RESULTS,
};
use crate::Error;
use heapless::Vec;
use obc_formats::io::{rd_i32, rd_u16, rd_u32, ByteSource, Error as IoError};
use obc_formats::obcm::{
    PoiCategory, CHUNK_END, HEADER_LEN, POI_CAT_ENTRY_LEN, POI_HOURS_BLOB_LEN, POI_HOURS_REF_NONE, POI_NAME_LEN,
    POI_RECORD_LEN,
};
use obc_map_scene::{cos_lat, ground_dist_m_cl, BBox, M_PER_DEG};

/// POI directory categories in v7 (spec §7.1): category ids `1..=6`. The parsed `MapTables::pois`
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

/// Initial half-extent of the POI search bbox, in latitude µdeg (~2 km: `2000 / 111.32e-3 m/µdeg ≈
/// 17 966`, rounded up). Doubled each pass until the nearest-16 are provably found (see
/// [`Reader::nearest_pois`]). The longitude half-extent is this scaled by `1/cos_lat`.
const POI_SEARCH_HALF_UDEG: i32 = 18_000;

/// The POI-scan stack scratch window, in bytes (spec §7.1's default chunk size, 14 records of 36 =
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
    /// Canonical category id (1..=6; spec §7.4).
    pub category_id: u8,
    /// Byte offset to this category's quadtree index.
    pub index_offset: usize,
    /// Number of `uint32` nodes in the index; `0` ⇒ the category is empty in this map.
    pub node_count: usize,
    /// Number of POI data chunks in this category.
    pub chunk_count: usize,
}

impl PoiCatEntry {
    /// This category is empty in this map (no quadtree, no chunks).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.node_count == 0
    }

    /// Byte offset where this category's data chunks begin (right after its index),
    /// or `None` if the arithmetic overflows `usize` (a corrupt directory on the
    /// 32-bit MCU) — the shared §7.1 convention, see [`index_end`].
    #[inline]
    pub fn data_start(&self) -> Option<usize> {
        index_end(self.index_offset, self.node_count)
    }

    /// Byte range `[start, end)` of POI chunk `chunk_id` given the directory's shared `chunk_size`
    /// (the §7.1 chunk size is directory-wide, not per-entry, so it's passed in). See
    /// [`fixed_chunk_range`].
    #[inline]
    fn chunk_range(&self, chunk_id: u32, chunk_size: usize) -> Option<(usize, usize)> {
        fixed_chunk_range(self.data_start(), self.chunk_count, chunk_size, chunk_id)
    }
}

/// A single POI result from [`Reader::nearest_pois`]. Coordinates are absolute microdegrees (§7.3);
/// `distance_m` is the ground distance from the query position, computed during the scan. `name` is
/// empty for an unnamed POI — the app then shows the subtype's fallback label
/// ([`poi_label_of`](obc_formats::obcm::poi_label_of)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Poi {
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
    pub hours_pool_offset: usize,
    /// Number of 29-byte blobs in the hours pool (spec §7.5); `0` ⇒ no hours in this map. Equals the
    /// `count u16` written at `hours_pool_offset`, validated equal at parse.
    pub hours_pool_count: usize,
}

/// The one shared empty POI directory a shard reader hands out — a `static` rather than a
/// promoted temporary because [`PoiDirectory`] holds a `heapless::Vec` and does not const-promote.
static EMPTY_POI_DIRECTORY: PoiDirectory = PoiDirectory::EMPTY;

impl PoiDirectory {
    /// The directory a reader with no POI section of its own reports — no categories, no chunks,
    /// no hours pool. The POI twin of [`super::NavDirectory::EMPTY`], and what a **volume-set shard**
    /// reader answers (`OBCA_Spec` §5.1: POIs live in the core file alone).
    pub const EMPTY: PoiDirectory =
        PoiDirectory { chunk_size: 0, entries: Vec::new(), hours_pool_offset: 0, hours_pool_count: 0 };
}

impl QuadIndex for PoiCatEntry {
    #[inline]
    fn index_offset(&self) -> usize {
        self.index_offset
    }
    #[inline]
    fn node_count(&self) -> usize {
        self.node_count
    }
}

impl<'a> Reader<'a> {
    /// The parsed POI directory (spec §7): the shared chunk size, one entry per category, and the
    /// v7 hours-pool offset/count. Always present (six categories, some possibly empty).
    /// [`Reader::nearest_pois`] walks the per-category quadtrees; P3 (#443) reads
    /// [`PoiDirectory::hours_pool_offset`]/[`PoiDirectory::hours_pool_count`] to resolve a POI's
    /// pooled schedule.
    ///
    /// [`PoiDirectory::EMPTY`] on a volume-set shard — see [`Reader::is_set_shard`].
    #[inline]
    pub fn poi_directory(&self) -> &PoiDirectory {
        if self.is_set_shard() {
            return &EMPTY_POI_DIRECTORY;
        }
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
        // A volume-set shard carries no hours pool (see `is_set_shard`).
        if self.is_set_shard() {
            return None;
        }
        // The no-hours sentinel and any index past the pool ⇒ no schedule.
        let dir = &self.tables.pois;
        if hours_ref == POI_HOURS_REF_NONE || (hours_ref as usize) >= dir.hours_pool_count {
            return None;
        }
        // Byte offset of blob `hours_ref`: hours_pool_offset + 2 + hours_ref*29. All checked so a
        // corrupt directory can't wrap `u32` or address past the file.
        let blob_off = (hours_ref as u32)
            .checked_mul(POI_HOURS_BLOB_LEN as u32)?
            .checked_add(2)?
            .checked_add(u32::try_from(dir.hours_pool_offset).ok()?)?;
        let end = blob_off.checked_add(POI_HOURS_BLOB_LEN as u32)?;
        if end > self.src.len() {
            return None;
        }
        // A single small stack read — no cache, no static buffer.
        let mut blob = [0u8; POI_HOURS_BLOB_LEN];
        self.src.read_at(blob_off, &mut blob).ok()?;
        crate::hours::WeeklySchedule::decode(&blob)
    }

    /// The nearest [`MAX_POI_RESULTS`] POIs of `category` to `pos` (a `(lon, lat)` µdeg pair, the
    /// crate's coordinate order), ascending by ground distance. Fills the caller-owned `out`
    /// (cleared first) — fewer than 16 when the category holds fewer in the whole map, empty when
    /// the category is empty. On-demand (a user opening a list), never per-frame.
    ///
    /// **Expanding-ring scan (spec §7.2 / epic #115).** Walks the category's quadtree over a square
    /// search bbox that starts ~2 km half-extent around `pos` and **doubles** until the nearest-16
    /// are provably found — the set is full *and* its 16th is no farther than the bbox half-extent
    /// (anything outside a square bbox is at least half-extent away), or the bbox has grown to
    /// contain the whole map (then the pass was exhaustive). No new persistent state: each chunk
    /// streams through a single 512-byte stack scratch, `pos`'s `cos_lat` is hoisted once, and the
    /// 16-slot best-set lives in `out`. A record revisited on a wider pass is deduped by its
    /// `(lat, lon, subtype)` so it's never returned twice. Structurally invalid records such as an
    /// out-of-range subtype are skipped; source or index-cache failures return a typed [`Error`]
    /// rather than being mistaken for an empty category.
    ///
    /// # Reentrancy
    ///
    /// Like the geometry walk, this streams through the internal cache. Legal re-entry while a
    /// feature callback holds that cache returns [`Error::CacheBusy`] instead of panicking.
    pub fn nearest_pois(
        &self,
        category: PoiCategory,
        pos: (i32, i32),
        out: &mut Vec<Poi, MAX_POI_RESULTS>,
    ) -> Result<(), Error> {
        out.clear();
        // A volume-set shard carries no POI section (see `is_set_shard`).
        if self.is_set_shard() {
            return Ok(());
        }
        let dir = &self.tables.pois;
        let entry = match dir.entries.iter().find(|e| e.category_id == category.id()) {
            // An absent or empty category is a valid "no POIs here" answer, not an error.
            Some(e) if !e.is_empty() => *e,
            _ => return Ok(()),
        };
        // `chunk_size / POI_RECORD_LEN` is the record cap; a corrupt 0 would divide-by-zero / loop
        // forever, so treat the whole (unwalkable) section as empty.
        if dir.chunk_size < POI_RECORD_LEN {
            return Ok(());
        }

        // Hoist `cos_lat` once for the query band. Guard a degenerate `cl` (≈0 near the poles, or a
        // corrupt latitude) so the lon half-extent below can't divide by zero / overflow.
        let cl = cos_lat(pos.1).max(1e-3);
        let map = self.bbox;

        let mut half = POI_SEARCH_HALF_UDEG;
        loop {
            // Square in ground meters: the lon half-extent is scaled by 1/cos_lat so both axes span
            // the same ~ `half`-µdeg-of-latitude ground distance. Saturating so a huge `half` (late
            // passes) can't wrap i32.
            let lon_half = ((half as f32 / cl) as i32).max(1);
            let search = BBox {
                min_lon: pos.0.saturating_sub(lon_half),
                min_lat: pos.1.saturating_sub(half),
                max_lon: pos.0.saturating_add(lon_half),
                max_lat: pos.1.saturating_add(half),
            };
            // Re-walk from scratch each pass (the set dedups revisits). The set only ever holds the
            // true nearest-16 seen so far, so a superset pass converges it.
            self.poi_scan(&entry, dir.chunk_size, pos, cl, &search, out)?;

            // The half-extent as a ground radius: everything outside the square is at least this far
            // (the tighter of the two axes' meter half-extents — they're ~equal by construction, but
            // take the min to stay a sound lower bound). `half` µdeg-of-latitude → meters.
            let half_m = (half as f32) * (M_PER_DEG as f32) * 1e-6;
            let full = out.len() == MAX_POI_RESULTS;
            if full && (out[MAX_POI_RESULTS - 1].distance_m as f32) <= half_m {
                return Ok(());
            }
            // The search bbox already covers the whole map ⇒ this pass was exhaustive; whatever is in
            // the set is the final answer (even if < 16).
            if search.min_lon <= map.min_lon
                && search.min_lat <= map.min_lat
                && search.max_lon >= map.max_lon
                && search.max_lat >= map.max_lat
            {
                return Ok(());
            }
            // Double the ring and re-walk. Saturating so we can't overflow before the map-cover check
            // above trips.
            half = half.saturating_mul(2);
        }
    }

    /// One expanding-ring pass: walk `entry`'s quadtree for leaves overlapping `search` and fold
    /// every valid record of every non-empty leaf into the nearest-16 `out` set (deduped by
    /// `(lat, lon, subtype)`). `cl` is the hoisted `cos_lat`; distances are equirectangular ground
    /// meters via the shared `obc-map-scene` distance core. The walk and the record streaming are
    /// [`Reader::scan_poi_leaves`] / [`Reader::stream_poi_records`]; this is only the tail that
    /// scores a record.
    fn poi_scan(
        &self,
        entry: &PoiCatEntry,
        chunk_size: usize,
        pos: (i32, i32),
        cl: f32,
        search: &BBox,
        out: &mut Vec<Poi, MAX_POI_RESULTS>,
    ) -> Result<(), Error> {
        self.scan_poi_leaves(entry, chunk_size, search, |start, record_cap| {
            self.stream_poi_records(start, record_cap, |win, off, lat, lon, subtype| {
                let distance_m = ground_dist_m_cl(pos, (lon, lat), cl) as u32;
                consider_poi(out, PoiCand { lat, lon, subtype, distance_m }, win, off);
            })
        })
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
    fn scan_poi_leaves(
        &self,
        entry: &PoiCatEntry,
        chunk_size: usize,
        search: &BBox,
        mut scan: impl FnMut(u32, usize) -> Result<(), IoError>,
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
            if end > self.src.len() as usize {
                return;
            }
            if let Err(error) = scan(start as u32, records_per_chunk) {
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
    fn stream_poi_records(
        &self,
        start: u32,
        record_cap: usize,
        mut visit: impl FnMut(&[u8], usize, i32, i32, u8),
    ) -> Result<(), IoError> {
        const RECS_PER_WINDOW: usize = POI_SCAN_WINDOW / POI_RECORD_LEN;
        let mut scratch = [0u8; POI_SCAN_WINDOW];
        let mut done = 0usize;
        while done < record_cap {
            let take = (record_cap - done).min(RECS_PER_WINDOW);
            let win = &mut scratch[..take * POI_RECORD_LEN];
            self.src.read_at(start + (done * POI_RECORD_LEN) as u32, win)?;
            for r in 0..take {
                let off = r * POI_RECORD_LEN;
                let subtype = win[off + 8];
                if subtype == CHUNK_END {
                    return Ok(()); // end-of-records sentinel — nothing valid follows in this chunk
                }
                // Skip an out-of-range subtype (0, or past the table) cleanly — never panic/UB.
                if obc_formats::obcm::poi_subtype_row(subtype).is_none() {
                    continue;
                }
                visit(win, off, rd_i32(win, off), rd_i32(win, off + 4), subtype);
            }
            done += take;
        }
        Ok(())
    }

    /// The POIs of `cats` sitting within [`CORRIDOR_HALF_WIDTH_M`] of the route **ahead** of
    /// `progress_m`, ascending by along-route distance, capped at [`MAX_CORRIDOR_RESULTS`] — the
    /// data source behind the "Up ahead" timeline (epic #946). Fills the caller-owned `out` (cleared
    /// first). On-demand (a snapshot taken on screen entry / filter change), **never** per frame.
    ///
    /// Each result carries where it projects onto the route ([`CorridorPoi::dist_along_m`], on the
    /// same axis stored waypoints use) and a **signed** lateral offset
    /// ([`CorridorPoi::offset_m`]: positive = right of the direction of travel).
    ///
    /// # The walk
    ///
    /// One pass over the route's chunks in route order, driven by [`RoutePath`] — the resident chunk
    /// index the breadcrumb/progress machinery already holds, so no full-route re-read. For each
    /// chunk still ahead of `progress_m`:
    ///
    /// 1. its bbox is inflated by the corridor half-width ([`inflate_bbox`]) — a tight window, since
    ///    a route chunk spans a few hundred meters, not the whole route;
    /// 2. the chunk's polyline is decoded **once** into the path's own scratch;
    /// 3. each selected category's quadtree is walked over that window (the same
    ///    [`walk_leaves`](Reader::walk_leaves) the geometry and nearest-N queries use), and every POI
    ///    record streams through a 512-byte stack scratch exactly as in
    ///    [`nearest_pois`](Reader::nearest_pois) — no per-leaf buffer, no [`super::MapCache`] growth;
    /// 4. each record is projected onto that chunk ([`project_onto_chunk`]) and folded into `out` if
    ///    it is inside the corridor and at or past `progress_m`.
    ///
    /// **Cost bound.** At most one route-chunk decode plus one quadtree descent per (remaining
    /// chunk × selected non-empty category); an absent or empty category costs nothing. The walk
    /// **stops early** once `out` is full and the current chunk starts farther along than the
    /// worst-held result — no POI from there on could displace one — so a POI-dense route pays for
    /// the first ~16 results, not for its whole length.
    ///
    /// **Dedupe.** A POI is keyed by `(lat, lon, subtype)` and appears once, at its nearest
    /// projection: `project_onto_chunk` already resolves a switchback *within* one chunk, and a POI
    /// re-found from a later chunk replaces the held entry only when its offset is smaller.
    /// (Refinement is naturally bounded to the chunks actually walked — the early exit above stops
    /// at the point where nothing new can enter the list anyway.)
    ///
    /// # Reentrancy
    ///
    /// Like [`nearest_pois`](Reader::nearest_pois) the quadtree walk streams through the internal
    /// index cache; legal re-entry returns [`Error::CacheBusy`]. The POI chunk reads go through
    /// plain stack `read_at`s, so they never nest with it.
    pub fn corridor_pois(
        &self,
        cats: PoiCategorySet,
        path: &dyn RoutePath,
        progress_m: u32,
        out: &mut Vec<CorridorPoi, MAX_CORRIDOR_RESULTS>,
    ) -> Result<(), Error> {
        out.clear();
        // A volume-set shard carries no POI section (see `is_set_shard`).
        if self.is_set_shard() {
            return Ok(());
        }
        let dir = &self.tables.pois;
        // `chunk_size / POI_RECORD_LEN` is the per-chunk record cap; a corrupt 0 would divide by
        // zero, so treat the whole (unwalkable) section as empty — same guard as `nearest_pois`.
        if dir.chunk_size < POI_RECORD_LEN || cats.is_empty() {
            return Ok(());
        }
        // Resolve the filter to the directory entries once, dropping absent/empty categories so the
        // per-chunk loop below never pays for a category this map doesn't carry.
        let mut entries: Vec<PoiCatEntry, POI_MAX_CATEGORIES> = Vec::new();
        for cat in cats.iter() {
            if let Some(e) = dir.entries.iter().find(|e| e.category_id == cat.id() && !e.is_empty()) {
                let _ = entries.push(*e);
            }
        }
        if entries.is_empty() {
            return Ok(());
        }

        let chunks = path.chunk_count();
        for k in 0..chunks {
            let start_m = path.chunk_start_m(k);
            // The chunk ends where the next one starts; the last chunk runs to the route end.
            let end_m = if k + 1 < chunks { path.chunk_start_m(k + 1) } else { u32::MAX };
            if end_m < progress_m {
                continue; // wholly behind the rider — nothing here can be "ahead"
            }
            // Early exit: `out` is sorted ascending and this chunk (and every later one) projects no
            // nearer than its own start, so a full set whose worst is already nearer is final.
            if out.len() == MAX_CORRIDOR_RESULTS && start_m >= out[MAX_CORRIDOR_RESULTS - 1].dist_along_m {
                break;
            }
            let search = inflate_bbox(path.chunk_bbox(k), CORRIDOR_HALF_WIDTH_M);
            let mut scan_error = None;
            // The chunk's polyline is decoded into the path's scratch and borrowed for this
            // callback; the quadtree walks run inside it so the geometry is never copied.
            path.visit_chunk_points(k, &mut |pts| {
                if pts.len() < 2 || scan_error.is_some() {
                    return;
                }
                for entry in &entries {
                    if let Err(error) =
                        self.corridor_scan_category(entry, dir.chunk_size, &search, pts, start_m, progress_m, out)
                    {
                        scan_error = Some(error);
                        return;
                    }
                }
            });
            if let Some(error) = scan_error {
                return Err(error);
            }
        }
        Ok(())
    }

    /// Walk one category's quadtree over `search` and fold every record that projects inside the
    /// corridor of `pts` into `out`. Shares its walk and its record streaming with the nearest-N
    /// query ([`Reader::scan_poi_leaves`] / [`Reader::stream_poi_records`], and so the same "no
    /// per-leaf buffer, never nested with the index cache" discipline); this is only the tail that
    /// projects a record onto the route.
    #[allow(clippy::too_many_arguments)]
    fn corridor_scan_category(
        &self,
        entry: &PoiCatEntry,
        chunk_size: usize,
        search: &BBox,
        pts: &[(i32, i32)],
        chunk_start_m: u32,
        progress_m: u32,
        out: &mut Vec<CorridorPoi, MAX_CORRIDOR_RESULTS>,
    ) -> Result<(), Error> {
        self.scan_poi_leaves(entry, chunk_size, search, |start, record_cap| {
            self.stream_poi_records(start, record_cap, |win, off, lat, lon, subtype| {
                // The corridor half-width is handed to the projection so it can prune segments as it
                // walks (a chunk is up to 256 points); `None` **is** the outside-the-corridor reject.
                let Some(proj) = project_onto_chunk(pts, chunk_start_m, (lon, lat), CORRIDOR_HALF_WIDTH_M) else {
                    return;
                };
                // The route axis is non-negative and the projection is clamped to the chunk, so the
                // round is a plain truncation; entries behind the rider are dropped here.
                let dist_along_m = proj.dist_along_m.max(0.0) as u32;
                if dist_along_m < progress_m {
                    return;
                }
                let cand = CorridorCand {
                    lat,
                    lon,
                    subtype,
                    dist_along_m,
                    offset_m: libm::roundf(proj.offset_m) as i32,
                    to_go_m: dist_along_m - progress_m,
                };
                consider_corridor_poi(out, cand, win, off);
            })
        })
    }
}

/// A decoded POI record's scalar fields, before the (lazy) name decode — the value
/// [`consider_poi`] folds into the nearest-16 set.
struct PoiCand {
    lat: i32,
    lon: i32,
    subtype: u8,
    distance_m: u32,
}

/// Fold one decoded record into the sorted nearest-16 `out` set: reject it if the set is full and
/// it's no closer than the current 16th, dedup an already-present `(lat, lon, subtype)` (a record
/// revisited on a wider ring), else insert it in distance order (ties keep the earlier-seen, a
/// stable order). `buf`/`off` locate the record for the **lazy** name decode, which only runs once
/// the record is known to belong in the set.
fn consider_poi(out: &mut Vec<Poi, MAX_POI_RESULTS>, cand: PoiCand, buf: &[u8], off: usize) {
    let PoiCand { lat, lon, subtype, distance_m } = cand;
    // Cheap rejection before any dedup scan or name decode: a full set whose worst is closer.
    if out.len() == MAX_POI_RESULTS && distance_m >= out[MAX_POI_RESULTS - 1].distance_m {
        return;
    }
    // Dedup: the same POI reappears on every wider ring. Key on (lat, lon, subtype).
    if out.iter().any(|p| p.lat == lat && p.lon == lon && p.subtype == subtype) {
        return;
    }
    // Insertion index: first slot whose distance is strictly greater (so equal distances keep
    // insertion order — a stable, deterministic tie-break).
    let at = out.iter().position(|p| p.distance_m > distance_m).unwrap_or(out.len());
    // If the set is full, drop the current last to make room (its distance is > this one, since the
    // cheap-reject above let this through).
    if out.len() == MAX_POI_RESULTS {
        let _ = out.pop();
    }
    // The record's `hours_ref` at `[off+34 .. off+36]` (§7.3); carried so the detail screen can
    // resolve the pooled schedule without a re-query. The scan window always holds a whole record
    // (`take * POI_RECORD_LEN`), so these two bytes are in-bounds.
    let hours_ref = rd_u16(buf, off + 34);
    let poi = Poi { lat, lon, subtype, name: decode_poi_name(buf, off), hours_ref, distance_m };
    // `at` is a valid index in `0..=out.len()` and the set has room now; the insert can't fail.
    let _ = out.insert(at, poi);
}

/// A projected POI record's scalar fields, before the (lazy) name decode — the value
/// [`consider_corridor_poi`] folds into the corridor set.
struct CorridorCand {
    lat: i32,
    lon: i32,
    subtype: u8,
    dist_along_m: u32,
    offset_m: i32,
    /// Along-route distance still to go from the query's progress anchor (what the row shows).
    to_go_m: u32,
}

/// Fold one projected record into the along-route-sorted corridor set.
///
/// Order of business, cheapest-first but **dedupe before the capacity reject** so a POI already held
/// can still improve its projection when the set is full:
///
/// 1. an already-held `(lat, lon, subtype)` keeps its **nearest** projection — a strictly smaller
///    `|offset_m|` removes the old entry so the better one re-inserts in its new order slot, an
///    equal-or-worse one is dropped (this is the switchback dedupe across chunks);
/// 2. a full set whose farthest entry is already nearer rejects the candidate;
/// 3. otherwise insert in ascending `dist_along_m`, evicting the farthest when full. Ties keep the
///    earlier-seen entry, so the order is stable and deterministic.
///
/// `buf`/`off` locate the record for the **lazy** name decode, which only runs once the record is
/// known to belong in the set.
fn consider_corridor_poi(out: &mut Vec<CorridorPoi, MAX_CORRIDOR_RESULTS>, cand: CorridorCand, buf: &[u8], off: usize) {
    let CorridorCand { lat, lon, subtype, dist_along_m, offset_m, to_go_m } = cand;
    if let Some(at) = out.iter().position(|c| c.poi.lat == lat && c.poi.lon == lon && c.poi.subtype == subtype) {
        if offset_m.abs() >= out[at].offset_m.abs() {
            return; // already held at an equal or nearer projection
        }
        out.remove(at);
    }
    // Cheap rejection: a full set whose farthest entry is already nearer along the route.
    if out.len() == MAX_CORRIDOR_RESULTS && dist_along_m >= out[MAX_CORRIDOR_RESULTS - 1].dist_along_m {
        return;
    }
    // Insertion index: first slot strictly farther along (equal distances keep insertion order).
    let at = out.iter().position(|c| c.dist_along_m > dist_along_m).unwrap_or(out.len());
    if out.len() == MAX_CORRIDOR_RESULTS {
        let _ = out.pop();
    }
    // `hours_ref` at `[off+34 .. off+36]` (§7.3), carried so the detail screen resolves the pooled
    // schedule without a re-query. The scan window always holds a whole record, so this is in bounds.
    let hours_ref = rd_u16(buf, off + 34);
    let poi = Poi { lat, lon, subtype, name: decode_poi_name(buf, off), hours_ref, distance_m: to_go_m };
    // `at` is a valid index in `0..=out.len()` and the set has room now; the insert can't fail.
    let _ = out.insert(at, CorridorPoi { poi, dist_along_m, offset_m });
}

/// Decode a POI record's name (spec §7.3) from `buf` at record offset `off`: `name_len` at `off+9`,
/// the up-to-24-byte `Name` at `off+10` (bytes `[off+10 .. off+34]`; `hours_ref` follows at
/// `[off+34 .. off+36]`). Empty for an unnamed record (`name_len == 0`). The stored name is already
/// pre-folded printable ASCII, but this stays defensive — `name_len` is clamped to what the field
/// and the buffer hold, and any non-printable byte (a corrupt record) is dropped — so a bad chunk
/// yields a short/empty name, never a panic or garbage glyph.
fn decode_poi_name(buf: &[u8], off: usize) -> heapless::String<POI_NAME_LEN> {
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
/// `hours_pool_count` can wrap `usize`, so the region-end could land below `total` and admit a
/// category (or a pool blob) indexing out of the file — the same overflow guard style as
/// [`super::parse_lod_table`]/[`Reader::chunk_range`].
pub(super) fn parse_poi_directory(src: &dyn ByteSource, offset: usize, total: usize) -> Result<PoiDirectory, Error> {
    // The directory header is 3 bytes (count + chunk_size u16); it must fit the file.
    if offset < HEADER_LEN || offset.checked_add(3).is_none_or(|end| end > total) {
        return Err(Error::BadOffset);
    }
    let mut hdr = [0u8; 3];
    src.read_at(offset as u32, &mut hdr).map_err(Error::Source)?;
    let category_count = hdr[0] as usize;
    let chunk_size = rd_u16(&hdr, 1) as usize;
    if category_count > POI_MAX_CATEGORIES || chunk_size > POI_MAX_CHUNK_BYTES {
        return Err(Error::BadOffset);
    }
    // The whole directory (header + entries + the two v7 pool fields) must lie within the file.
    let pool_fields_off = category_count
        .checked_mul(POI_CAT_ENTRY_LEN)
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
        let o = offset + 3 + k * POI_CAT_ENTRY_LEN;
        src.read_at(o as u32, &mut e).map_err(Error::Source)?;
        let entry = PoiCatEntry {
            category_id: e[0],
            index_offset: rd_u32(&e, 1) as usize,
            node_count: rd_u32(&e, 5) as usize,
            chunk_count: rd_u32(&e, 9) as usize,
        };
        // An empty category (node_count 0) still carries an entry; its index/chunk region is
        // zero-length, so only the offset itself needs to be in-file. A populated one must have its
        // whole index + chunk region inside the file — checked, so a corrupt count can't wrap past
        // `total`.
        if entry.node_count > 0 {
            let region_end = entry
                .data_start()
                .and_then(|start| entry.chunk_count.checked_mul(chunk_size).and_then(|len| start.checked_add(len)))
                .ok_or(Error::BadOffset)?;
            if entry.index_offset < HEADER_LEN || region_end > total {
                return Err(Error::BadOffset);
            }
        } else if entry.index_offset > total {
            return Err(Error::BadOffset);
        }
        let _ = entries.push(entry);
    }

    // The two v7 hours-pool directory fields (spec §7.5): the section's absolute offset + blob
    // count. When the count is non-zero, the whole pool region (`count u16` + `count × 29-byte`
    // blobs) must lie in-file — checked, so a corrupt count can't wrap `usize` past `total`. An
    // empty pool (count 0) still validates its 2-byte `count` header lies in-file.
    let mut pf = [0u8; 6];
    src.read_at(pool_fields_off as u32, &mut pf).map_err(Error::Source)?;
    let hours_pool_offset = rd_u32(&pf, 0) as usize;
    let hours_pool_count = rd_u16(&pf, 4) as usize;
    if hours_pool_offset < HEADER_LEN {
        return Err(Error::BadOffset);
    }
    let pool_end = hours_pool_count
        .checked_mul(POI_HOURS_BLOB_LEN)
        .and_then(|blobs| hours_pool_offset.checked_add(2)?.checked_add(blobs))
        .ok_or(Error::BadOffset)?;
    if pool_end > total {
        return Err(Error::BadOffset);
    }

    Ok(PoiDirectory { chunk_size, entries, hours_pool_offset, hours_pool_count })
}
