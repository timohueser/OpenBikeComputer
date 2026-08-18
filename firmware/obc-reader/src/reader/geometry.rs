//! Geometry LOD tables, quadtree chunk selection, and streaming feature decode.

use super::cache::{ChunkLoc, WalkEntry, WALK_CACHE_ENTRIES};
use super::{
    expand_walk_bbox, index_end, intersect_bbox, CacheError, CapacityError, DecodeStatus, FeatureDecodeError,
    FeatureReadError, MapReadError, QuadIndex, Reader, MAX_QUADTREE_DEPTH,
};
use crate::Error;
use heapless::Vec;
use obc_formats::io::{rd_f32, rd_i16, rd_i32, rd_u16, rd_u32, ByteSource};
use obc_formats::obcm::{
    OffsetScale, BRANCH_BIT, CHUNK_END, EMPTY_LEAF, FEATURE_FLAG_16BIT, FEATURE_FLAG_HOLES, FEATURE_FLAG_POLYGON,
    FEATURE_FLAG_WIDE, FEATURE_HEADER_COMPACT_LEN, FEATURE_HEADER_WIDE_LEN, HEADER_LEN, LOD_ENTRY_LEN,
};
use obc_map_scene::{BBox, Kind};

/// Upper bound on the vertices of a single decoded feature — the capacity a caller
/// sizes the `points` scratch buffer to for [`Reader::for_each_feature`].
pub const MAX_FEAT_PTS: usize = 2048;
/// Upper bound on the rings (exterior + holes) of a single decoded feature — the
/// capacity for the `ring_lens` scratch buffer of [`Reader::for_each_feature`].
pub const MAX_FEAT_RINGS: usize = 32;

/// Upper bound on a single map data chunk, in bytes — the size of the decode scratch, and the
/// largest `chunk_size` the reader accepts ([`super::MapTables::parse`] rejects a bigger one). The
/// format stores `chunk_size` as a `u16` (≤ 65535) but real maps pack far smaller (the packer
/// defaults to 4096), so this caps the scratch below the format ceiling to save RAM. A chunk
/// between a cache slot and this decodes through the scratch, uncached.
///
/// This is an **acceptance** bound, not just a buffer size: shrinking it makes the reader reject
/// deliberately large chunks the round-trip suite packs (obc-pack's
/// `max_feat_pts_boundary_survives` puts two features, one at `MAX_FEAT_PTS`, into one 8192-byte
/// chunk), and device and host share the one profile — so a map that packs, loads.
pub const MAX_CHUNK_BYTES: usize = 16384;

/// One level of the LOD pyramid: a self-contained quadtree index + chunk set.
#[derive(Debug, Clone, Copy)]
pub struct Lod {
    /// Upper bound of the meters-per-pixel range this level covers; the coarsest
    /// level is `f32::INFINITY`. Strictly decreasing from coarse (0) to fine.
    pub max_mpp: f32,
    pub index_offset: usize,
    pub node_count: usize,
    /// The **capacity bound** on one chunk (spec §3): the packer's leaf-split threshold and the
    /// largest length any single chunk may have. Not a stride — chunks are packed tight and
    /// addressed through the offset table.
    pub chunk_size: usize,
    pub chunk_count: usize,
    /// Total **units** of this level's chunk-data region — `offsets[chunk_count]`, the last entry
    /// of the offset table, read once in `parse_lod_table`. Resident so a per-chunk fetch can bound
    /// its offset pair without a second read. Kept in units, not bytes, so the comparison against a
    /// freshly-read offset pair is one the file's own arithmetic makes directly.
    pub chunk_units_total: u32,
    /// This file's offset unit (§1.1), carried so that an offset-table entry read lazily at render
    /// time resolves against the scale of the file it came out of. A mounted map can have more than
    /// one file open; pairing the two at the point of decode is what makes the wrong combination
    /// unspellable.
    pub scale: OffsetScale,
}

impl QuadIndex for Lod {
    #[inline]
    fn index_offset(&self) -> usize {
        self.index_offset
    }
    #[inline]
    fn node_count(&self) -> usize {
        self.node_count
    }
}

impl Lod {
    /// Byte offset of the LOD's per-chunk **offset table** (spec §5): `chunk_count + 1` `uint32`
    /// entries sitting between the quadtree index and the chunk data. `None` on `usize` overflow —
    /// reachable on the 32-bit MCU from a corrupt `index_offset`/`node_count`.
    #[inline]
    fn offset_table(&self) -> Option<usize> {
        index_end(self.index_offset, self.node_count)
    }

    /// Byte offset just past this level's offset table — where the chunk data would begin without
    /// v14's rounding step. `None` on `usize` overflow (see [`Lod::offset_table`]).
    #[inline]
    fn table_end(&self) -> Option<usize> {
        let table_len = self.chunk_count.checked_add(1)?.checked_mul(4)?;
        self.offset_table()?.checked_add(table_len)
    }

    /// Byte offset where this level's chunk **data** begins: `align_up(table_end, U)` (§3). The
    /// index and the offset table are read by 4-byte indexing from a start the LOD table names, so
    /// neither needs a unit boundary of its own — the chunks are addressed by scaled offsets, so
    /// they do, and the `0..U-1` bytes this rounds past are §1.2 filler. At `Offset Scale = 0` this
    /// is v13's arithmetic unchanged. `None` on overflow (see [`Lod::offset_table`]).
    #[inline]
    fn data_start(&self) -> Option<usize> {
        let aligned = self.scale.align_up(self.table_end()? as u64)?;
        usize::try_from(aligned).ok().filter(|&start| start as u64 == aligned)
    }

    /// Byte offset of chunk `chunk_id`'s entry in the offset table, or `None` if `chunk_id` is out
    /// of range or the arithmetic overflows `usize`. `chunk_id` comes straight from a quadtree leaf
    /// (arbitrary in a corrupt map), so it is validated against `chunk_count` with checked
    /// arithmetic. Entries `k` and `k+1` are adjacent, which is what makes a chunk extent **one**
    /// 8-byte read ([`Reader::chunk_range`]).
    #[inline]
    fn offset_entry(&self, chunk_id: u32) -> Option<usize> {
        let id = chunk_id as usize;
        if id >= self.chunk_count {
            return None;
        }
        self.offset_table()?.checked_add(id.checked_mul(4)?)
    }
}

/// A feature decoded into caller-owned scratch buffers, borrowed for one
/// [`Reader::for_each_feature`] callback. No per-feature allocation: `points`
/// holds every ring's vertices concatenated, `ring_lens[0]` is the exterior
/// length, the rest are holes. Coordinates are microdegrees.
#[derive(Debug, Clone, Copy)]
pub struct FeatureRef<'a> {
    pub style_id: u8,
    pub kind: Kind,
    points: &'a [(i32, i32)],
    ring_lens: &'a [usize],
    bbox: BBox,
    /// Byte offset of this feature's header within its chunk — what [`Reader::decode_feature_at`]
    /// needs to re-decode exactly this feature in a later pass without re-walking the whole chunk
    /// (the renderer's two-phase collect, issue #564). Always `< chunk_size ≤ MAX_CHUNK_BYTES`.
    offset: usize,
}

impl<'a> FeatureRef<'a> {
    /// Axis-aligned bounds (microdegrees) of every vertex, computed during decode.
    /// Empty for a zero-vertex feature.
    #[inline]
    pub fn bbox(&self) -> BBox {
        self.bbox
    }

    /// Byte offset of this feature's header within its chunk (see the field). Hand this back to
    /// [`Reader::decode_feature_at`] to re-decode just this feature.
    #[inline]
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// The exterior ring's vertices.
    #[inline]
    pub fn exterior(&self) -> &'a [(i32, i32)] {
        let n = self.ring_lens.first().copied().unwrap_or(0);
        &self.points[..n]
    }

    /// Iterator over the interior (hole) rings, if any.
    #[inline]
    pub fn interiors(&self) -> Interiors<'a> {
        let start = self.ring_lens.first().copied().unwrap_or(0);
        let rest = if self.ring_lens.is_empty() { &[][..] } else { &self.ring_lens[1..] };
        Interiors { points: self.points, lens: rest, offset: start }
    }

    /// All rings' vertices, concatenated (exterior first); partition with [`FeatureRef::ring_lens`].
    #[inline]
    pub fn points(&self) -> &'a [(i32, i32)] {
        self.points
    }

    /// Per-ring vertex counts: `[0]` exterior, `[1..]` holes.
    #[inline]
    pub fn ring_lens(&self) -> &'a [usize] {
        self.ring_lens
    }
}

/// Iterator over a feature's hole rings (see [`FeatureRef::interiors`]).
pub struct Interiors<'a> {
    points: &'a [(i32, i32)],
    lens: &'a [usize],
    offset: usize,
}

impl<'a> Iterator for Interiors<'a> {
    type Item = &'a [(i32, i32)];
    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        let (&len, rest) = self.lens.split_first()?;
        self.lens = rest;
        let s = self.offset;
        self.offset += len;
        Some(&self.points[s..s + len])
    }
}

impl<'a> Reader<'a> {
    /// The parsed LOD pyramid (coarsest first).
    #[inline]
    pub fn lods(&self) -> &[Lod] {
        self.shard_lods.unwrap_or(&self.tables.lods)
    }

    /// Pick the finest LOD whose range still covers `mpp` (meters/pixel). The
    /// coarsest level (`max_mpp == +inf`) always qualifies, so the result is a
    /// valid index in `0..lods().len()`.
    pub fn select_lod_for_mpp(&self, mpp: f32) -> usize {
        let mut chosen = 0;
        for (i, lod) in self.lods().iter().enumerate() {
            if lod.max_mpp >= mpp {
                chosen = i;
            }
        }
        chosen
    }

    /// Byte range `[start, end)` of geometry chunk `chunk_id` in `l`, resolved through the LOD's
    /// per-chunk offset table (spec §5). `offsets[k]` and `offsets[k+1]` are adjacent, so this is a
    /// single 8-byte read — routed through the **index** block cache, the same one `read_node`
    /// uses: the table is index-like metadata and lies immediately after the index, so a walk's
    /// node reads and its chunk lookups share blocks instead of hitting the card twice.
    ///
    /// `chunk_id` is unvalidated file data, so the pair is checked before it can address anything:
    /// in range, monotonic, inside the region [`parse_lod_table`] bounded, and no longer than the
    /// LOD's declared `chunk_size` (nor the decode scratch). A corrupt table therefore yields
    /// [`MapReadError::Malformed`], never an out-of-region read.
    ///
    /// The cache borrow is taken and **released here**, before the caller borrows it again for
    /// `load_chunk` — the same read-before-you-need-it discipline as `read_node` in `walk_leaves`.
    fn chunk_range(&self, l: &Lod, chunk_id: u32) -> Result<(usize, usize), MapReadError> {
        if !self.cache_ready {
            return Err(MapReadError::Cache(CacheError::Busy));
        }
        let entry = l.offset_entry(chunk_id).ok_or(MapReadError::Malformed)?;
        let entry = u32::try_from(entry).map_err(|_| MapReadError::Malformed)?;
        let mut b = [0u8; 8];
        self.cache
            .try_borrow_mut()
            .map_err(MapReadError::Cache)?
            .index_read(self.src, self.file, entry, &mut b)
            .map_err(MapReadError::Source)?;
        // §5.1's four validity rules on the pair, restated for v14's units. The last one is the
        // interesting one: a chunk's *content* still may not exceed `Chunk Size`, but its **span**
        // is that content rounded up to a unit, so `align_up(Chunk Size, U)` is the tight bound —
        // the looser `Chunk Size + U - 1` would admit spans no writer can produce.
        let (off0, off1) = (rd_u32(&b, 0), rd_u32(&b, 4));
        if off1 < off0 || off1 > l.chunk_units_total {
            return Err(MapReadError::Malformed);
        }
        let span = l.scale.offset(off1 - off0).bytes();
        let span_bound = l.scale.align_up(l.chunk_size as u64).ok_or(MapReadError::Malformed)?;
        if span > span_bound || span > MAX_CHUNK_BYTES as u64 {
            return Err(MapReadError::Malformed);
        }
        let span = span as usize;
        let start = l
            .data_start()
            .and_then(|d| usize::try_from(l.scale.offset(off0).bytes()).ok().and_then(|o| d.checked_add(o)))
            .ok_or(MapReadError::Malformed)?;
        let end = start.checked_add(span).ok_or(MapReadError::Malformed)?;
        if end > self.src.len() as usize {
            return Err(MapReadError::Malformed);
        }
        Ok((start, end))
    }

    /// Visit `(chunk_id, node_bbox)` for every non-empty leaf in `lod` overlapping `view`, in
    /// quadtree order. `lod` indexes [`Reader::lods`]; out-of-range visits nothing. Unlike a
    /// capacity-bounded collect, this streams through a callback with **no upper bound** on the
    /// chunk count — the renderer relies on this so a wide viewport never silently drops chunks.
    /// The walk only reads the index (bbox tests over `u32` nodes), so re-running it for the second
    /// (pass B) traversal is cheap relative to decoding.
    pub fn for_each_chunk(
        &self,
        lod: usize,
        view: &BBox,
        mut visit: impl FnMut(u32, BBox),
    ) -> Result<(), MapReadError> {
        let Some(l) = self.lods().get(lod) else {
            return Ok(());
        };
        if l.node_count == 0 {
            return Ok(());
        }
        let Some(query) = intersect_bbox(view, &self.bbox) else {
            return Ok(());
        };
        if !self.cache_ready {
            return Err(MapReadError::Cache(CacheError::Busy));
        }

        // A successful prior expanded walk is a complete ordered leaf list for every query wholly
        // inside its cover. Copy it out before invoking the callback: a callback loads geometry and
        // therefore borrows this same RefCell.
        let cached =
            self.cache.try_borrow_mut().map_err(MapReadError::Cache)?.cached_walk(self.file, lod as u8, &query);
        if let Some(entries) = cached {
            for entry in entries {
                if entry.node.intersects(&query) {
                    visit(entry.cid, entry.node);
                }
            }
            return Ok(());
        }

        let cover = expand_walk_bbox(&query, &self.bbox);
        let mut entries: Vec<WalkEntry, WALK_CACHE_ENTRIES> = Vec::new();
        let mut cacheable = true;
        self.walk_geometry_prefetch(l, 0, self.bbox, &query, &cover, 0, &mut entries, &mut cacheable, &mut visit)?;
        if cacheable {
            self.cache.try_borrow_mut().map_err(MapReadError::Cache)?.store_walk(self.file, lod as u8, cover, &entries);
        }
        Ok(())
    }

    /// Geometry-only walk that opportunistically explores `cover`, but preserves the exact
    /// `primary` query's behavior. Once the twelve-entry result budget overflows, subsequent
    /// recursion immediately shrinks back to `primary`; errors found solely in the speculative
    /// margin likewise abandon caching rather than failing a query that never touched that node.
    /// Leaves in `primary` are always streamed once and in ordinary quadtree order.
    #[allow(clippy::too_many_arguments)]
    fn walk_geometry_prefetch<F: FnMut(u32, BBox)>(
        &self,
        index: &dyn QuadIndex,
        idx: usize,
        node: BBox,
        primary: &BBox,
        cover: &BBox,
        depth: u32,
        entries: &mut Vec<WalkEntry, WALK_CACHE_ENTRIES>,
        cacheable: &mut bool,
        visit: &mut F,
    ) -> Result<(), MapReadError> {
        let target = if *cacheable { cover } else { primary };
        if idx >= index.node_count() || depth > MAX_QUADTREE_DEPTH || !node.intersects(target) {
            return Ok(());
        }
        let val = match self.read_node(index, idx) {
            Ok(val) => val,
            Err(error) if node.intersects(primary) => return Err(error),
            Err(_) => {
                *cacheable = false;
                return Ok(());
            }
        };
        if val & BRANCH_BIT == 0 {
            if val != EMPTY_LEAF {
                if *cacheable && entries.push(WalkEntry { cid: val, node }).is_err() {
                    *cacheable = false;
                }
                if node.intersects(primary) {
                    visit(val, node);
                }
            }
            return Ok(());
        }
        let child = (val & !BRANCH_BIT) as usize;
        if child <= idx {
            if node.intersects(primary) {
                return Err(MapReadError::Malformed);
            }
            *cacheable = false;
            return Ok(());
        }
        let mid_lon = (node.min_lon + node.max_lon).div_euclid(2);
        let mid_lat = (node.min_lat + node.max_lat).div_euclid(2);
        let kids = [
            BBox { min_lon: node.min_lon, min_lat: mid_lat, max_lon: mid_lon, max_lat: node.max_lat },
            BBox { min_lon: mid_lon, min_lat: mid_lat, max_lon: node.max_lon, max_lat: node.max_lat },
            BBox { min_lon: node.min_lon, min_lat: node.min_lat, max_lon: mid_lon, max_lat: mid_lat },
            BBox { min_lon: mid_lon, min_lat: node.min_lat, max_lon: node.max_lon, max_lat: mid_lat },
        ];
        for (i, kb) in kids.iter().enumerate() {
            self.walk_geometry_prefetch(index, child + i, *kb, primary, cover, depth + 1, entries, cacheable, visit)?;
        }
        Ok(())
    }

    /// Decode every feature in a chunk of `lod`, invoking `visit` once per feature with a
    /// [`FeatureRef`] borrowing the caller's `points`/`ring_lens` scratch. Allocation-free: the
    /// buffers grow to the largest feature once and are reused across features/chunks/frames.
    /// `node` is the leaf bbox yielded by [`Reader::for_each_chunk`].
    ///
    /// # Reentrancy
    ///
    /// The internal cache borrow is held while `visit` runs. Resident-table calls remain available;
    /// a nested streaming call returns [`MapReadError::Cache`] instead of panicking.
    pub fn for_each_feature<const P: usize, const R: usize>(
        &self,
        lod: usize,
        chunk_id: u32,
        node: &BBox,
        points: &mut Vec<(i32, i32), P>,
        ring_lens: &mut Vec<usize, R>,
        visit: impl FnMut(FeatureRef),
    ) -> Result<DecodeStatus, MapReadError> {
        self.for_each_feature_filtered(lod, chunk_id, node, points, ring_lens, |_| true, visit)
    }

    /// Like [`Reader::for_each_feature`], but `should_decode` is consulted with each feature's
    /// style id **before** its coordinates are decoded: `false` skips the geometry cheaply
    /// (advancing past its bytes with no coordinate math), `true` decodes it and hands a
    /// [`FeatureRef`] to `visit`. The renderer uses this so a collect traversal decodes only the
    /// features it needs — pass B decodes just the **selected winners** and skips everything else
    /// cheaply, advancing past their bytes with no coordinate math.
    ///
    /// # Reentrancy
    ///
    /// The internal cache borrow is held while `should_decode` and `visit` run. Resident-table calls
    /// remain available; a nested streaming call returns [`MapReadError::Cache`].
    #[allow(clippy::too_many_arguments)]
    pub fn for_each_feature_filtered<const P: usize, const R: usize>(
        &self,
        lod: usize,
        chunk_id: u32,
        node: &BBox,
        points: &mut Vec<(i32, i32), P>,
        ring_lens: &mut Vec<usize, R>,
        should_decode: impl Fn(u8) -> bool,
        visit: impl FnMut(FeatureRef),
    ) -> Result<DecodeStatus, MapReadError> {
        let l = match self.lods().get(lod) {
            Some(l) => l,
            None => return Err(MapReadError::Malformed),
        };
        // Resolve the chunk's extent from the offset table first — that read borrows the cache, so
        // it must finish before the `load_chunk` borrow below. `chunk_range` validates the pair
        // (range, monotonicity, `chunk_size`, `MAX_CHUNK_BYTES`, file length) so nothing here can
        // index past the decode scratch or the file.
        let (start, end) = self.chunk_range(l, chunk_id)?;
        let len = end - start;
        // Pull the chunk through the cache, then decode from the resident bytes. The borrow is held
        // across `decode_chunk_into` — safe because `should_decode`/`visit` only touch
        // `self.tables.styles`, never the cache.
        if !self.cache_ready {
            return Err(MapReadError::Cache(CacheError::Busy));
        }
        let mut cache = self.cache.try_borrow_mut().map_err(MapReadError::Cache)?;
        let loc = match cache.load_chunk(self.src, self.file, lod as u8, chunk_id, start as u32, len, node) {
            Ok(loc) => loc,
            Err(error) => return Err(MapReadError::Source(error)),
        };
        let chunk = match loc {
            ChunkLoc::Slot(i) => &cache.chunks[i].buf[..len],
            ChunkLoc::Scratch => &cache.scratch[..len],
        };
        Ok(decode_chunk_into(chunk, node, points, ring_lens, should_decode, visit))
    }

    /// Decode exactly the feature at byte `offset` within chunk `cid` of `lod`, into the caller's
    /// `points`/`ring_lens` scratch, returning its [`FeatureRef`]. The renderer's two-phase collect
    /// (issue #564) uses this in pass B to re-materialize a *winning* feature's geometry — one it
    /// selected in pass A by a lightweight stub ([`FeatureRef::offset`]) — without re-decoding the
    /// rest of the chunk.
    ///
    /// `node` is the leaf bbox [`Reader::for_each_chunk`] yields for `cid` (the per-feature anchor
    /// base). `offset` came from a [`FeatureRef::offset`] earlier this frame, but it is still
    /// validated against the chunk length and the `0xFF` end-marker, so a stale/corrupt offset
    /// yields [`FeatureReadError::Decode`], never a panic or an out-of-chunk read. Fetches the chunk
    /// through the same cache as the full walk, so consecutive calls for one `cid` (pass B visits a
    /// chunk's winners together) hit the resident slot instead of re-reading it.
    ///
    /// # Reentrancy
    ///
    /// Same rule as [`Reader::for_each_feature_filtered`]: this borrows the internal cache for the
    /// fetch + decode. Calling it from [`Reader::for_each_chunk`] is the normal pass-B path; legal
    /// re-entry from a `for_each_feature*` callback returns a typed cache error.
    pub fn decode_feature_at<'p, const P: usize, const R: usize>(
        &self,
        lod: usize,
        cid: u32,
        offset: usize,
        node: &BBox,
        points: &'p mut Vec<(i32, i32), P>,
        ring_lens: &'p mut Vec<usize, R>,
    ) -> Result<FeatureRef<'p>, FeatureReadError> {
        // Every error leaves caller scratch empty. Besides making retries deterministic, this is
        // the public expression of the whole-feature contract: no stale geometry from a previous
        // success and no prefix decoded before a malformed hole may escape through these buffers.
        points.clear();
        ring_lens.clear();
        let l = self.lods().get(lod).ok_or(FeatureReadError::Decode(FeatureDecodeError::Malformed))?;
        // Same offset-table lookup + validation as the full walk (and the same borrow-then-release
        // ordering ahead of `load_chunk`); a read failure there is a read failure here.
        let (start, end) = match self.chunk_range(l, cid) {
            Ok(range) => range,
            Err(MapReadError::Malformed) => return Err(FeatureReadError::Decode(FeatureDecodeError::Malformed)),
            Err(error) => return Err(FeatureReadError::Read(error)),
        };
        let len = end - start;
        // The re-decode offset must additionally land inside *this* chunk — `len` now comes from the
        // offset table, so a stale offset from a differently-sized chunk is rejected here.
        if offset >= len {
            return Err(FeatureReadError::Decode(FeatureDecodeError::Malformed));
        }
        if !self.cache_ready {
            return Err(FeatureReadError::Read(MapReadError::Cache(CacheError::Busy)));
        }
        let mut cache =
            self.cache.try_borrow_mut().map_err(|error| FeatureReadError::Read(MapReadError::Cache(error)))?;
        let loc = cache
            .load_chunk(self.src, self.file, lod as u8, cid, start as u32, len, node)
            .map_err(|error| FeatureReadError::Read(MapReadError::Source(error)))?;
        let chunk = match loc {
            ChunkLoc::Slot(i) => &cache.chunks[i].buf[..len],
            ChunkLoc::Scratch => &cache.scratch[..len],
        };
        // The `FeatureRef` borrows `points`/`ring_lens` (its coordinates are copied there), not the
        // cache bytes, so it outlives the `cache` borrow dropped at return.
        match decode_one_feature(chunk, offset, node, points, ring_lens) {
            DecodeOne::Complete(fref, _) => Ok(fref),
            DecodeOne::Dropped(error, _) => Err(FeatureReadError::Decode(error)),
        }
    }

    /// Decode a pass-A feature straight from its resident geometry slot, without re-walking the
    /// quadtree or re-reading the chunk-offset table. The renderer's pass B asks for features that
    /// pass A selected only moments earlier; when the chunk survived the four-slot cache, its leaf
    /// bbox is resident beside the bytes and is the complete anchor needed by `decode_one_feature`.
    ///
    /// `Ok(None)` is an ordinary cache miss (the caller may fall back to the index walk). Errors
    /// retain the same typed cache/decode behavior as [`Reader::decode_feature_at`].
    pub(crate) fn decode_cached_feature_at<'p, const P: usize, const R: usize>(
        &self,
        lod: usize,
        cid: u32,
        offset: usize,
        points: &'p mut Vec<(i32, i32), P>,
        ring_lens: &'p mut Vec<usize, R>,
    ) -> Result<Option<FeatureRef<'p>>, FeatureReadError> {
        if !self.cache_ready {
            return Err(FeatureReadError::Read(MapReadError::Cache(CacheError::Busy)));
        }
        let mut cache =
            self.cache.try_borrow_mut().map_err(|error| FeatureReadError::Read(MapReadError::Cache(error)))?;
        let Some(i) = cache
            .chunks
            .iter()
            .position(|slot| slot.valid() && slot.file == self.file && slot.lod() == lod as u8 && slot.cid == cid)
        else {
            return Ok(None);
        };

        points.clear();
        ring_lens.clear();
        let len = cache.chunks[i].len as usize;
        if offset >= len {
            return Err(FeatureReadError::Decode(FeatureDecodeError::Malformed));
        }
        cache.chunk_hits = cache.chunk_hits.saturating_add(1);
        cache.chunks[i].used = 0;
        let slot = &cache.chunks[i];
        match decode_one_feature(&slot.buf[..len], offset, &slot.node, points, ring_lens) {
            DecodeOne::Complete(fref, _) => Ok(Some(fref)),
            DecodeOne::Dropped(error, _) => Err(FeatureReadError::Decode(error)),
        }
    }
}

/// Running vertex bounds, accumulated as a feature decodes so its bbox is ready
/// with no extra pass over the points. Seeded inverted; widened per vertex.
#[derive(Clone, Copy)]
struct Bounds {
    min_lon: i32,
    min_lat: i32,
    max_lon: i32,
    max_lat: i32,
}

impl Bounds {
    #[inline]
    fn new() -> Self {
        Bounds { min_lon: i32::MAX, min_lat: i32::MAX, max_lon: i32::MIN, max_lat: i32::MIN }
    }
    #[inline]
    fn add(&mut self, x: i32, y: i32) {
        if x < self.min_lon {
            self.min_lon = x;
        }
        if x > self.max_lon {
            self.max_lon = x;
        }
        if y < self.min_lat {
            self.min_lat = y;
        }
        if y > self.max_lat {
            self.max_lat = y;
        }
    }
    #[inline]
    fn to_bbox(self) -> BBox {
        BBox { min_lon: self.min_lon, min_lat: self.min_lat, max_lon: self.max_lon, max_lat: self.max_lat }
    }
}

/// Walk a single chunk's bytes, decoding each feature into the shared `points`/`ring_lens` buffers
/// (cleared and refilled per feature) and handing a [`FeatureRef`] to `visit`. A feature whose
/// style `should_decode` rejects is skipped without decoding — its geometry bytes are advanced past
/// with no coordinate math ([`skip_ring`] mirrors [`read_ring`]'s offset arithmetic exactly, so the
/// two stay byte-for-byte in sync). Accepted features decode through [`decode_one_feature`], the
/// same path [`Reader::decode_feature_at`] takes, so a feature decodes identically either way.
#[inline]
fn decode_chunk_into<const P: usize, const R: usize>(
    chunk: &[u8],
    node: &BBox,
    points: &mut Vec<(i32, i32), P>,
    ring_lens: &mut Vec<usize, R>,
    should_decode: impl Fn(u8) -> bool,
    mut visit: impl FnMut(FeatureRef),
) -> DecodeStatus {
    let cs = chunk.len();
    let mut off = 0usize;
    let mut status = DecodeStatus::default();
    // A chunk is `features ++ [CHUNK_END]` (§5) — exactly one sentinel, no padding. Running off the
    // end without meeting it means the chunk was truncated or its offset-table length is wrong, so
    // the walk owes the caller a malformed drop rather than a silent clean finish. `verdict` also
    // covers the paths that already reported one and consumed the rest of the chunk.
    let mut verdict = false;

    while off < cs {
        if chunk[off] == CHUNK_END {
            verdict = true;
            break;
        }
        let style_id = chunk[off];

        // Skip path: the caller doesn't want this style this pass, so advance past the geometry
        // without decoding (read only the header fields the skip needs).
        if !should_decode(style_id) {
            match skip_feature(chunk, off) {
                Ok(next) => off = next,
                Err(error) => {
                    // A filtered feature still participates in the public whole-feature scratch
                    // contract. If its framing is malformed, discard geometry left by a previous
                    // selected feature (or stale caller prefill) before reporting the drop.
                    points.clear();
                    ring_lens.clear();
                    status.dropped(error);
                    verdict = true;
                    break;
                }
            }
            continue;
        }

        match decode_one_feature(chunk, off, node, points, ring_lens) {
            DecodeOne::Complete(fref, next) => {
                visit(fref);
                status.complete = status.complete.saturating_add(1);
                off = next;
            }
            DecodeOne::Dropped(FeatureDecodeError::Malformed, _) => {
                // Malformed framing consumes the whole rest of the chunk (there is no trustworthy
                // next offset), so this drop *is* the chunk's verdict — don't also charge it for a
                // missing sentinel it never got to.
                status.dropped(FeatureDecodeError::Malformed);
                verdict = true;
                break;
            }
            DecodeOne::Dropped(error, next) => {
                status.dropped(error);
                off = next;
            }
        }
    }
    if !verdict {
        status.dropped(FeatureDecodeError::Malformed);
    }
    status
}

/// One parsed §5 feature header, in either layout, plus the header's own `len` so the caller
/// knows where the deltas start. Built only by [`read_feature_header`], which is the single place
/// that decides what "malformed framing" means — the decode and the skip path share it, so they can
/// never disagree about which byte a feature ends on.
struct FeatHeader {
    style_id: u8,
    flags: u8,
    ext_pt_count: usize,
    /// Anchor, leaf-relative µdeg. Compact headers zero-extend their `uint16` fields; wide ones
    /// carry the full `int32`.
    ax: i32,
    ay: i32,
    len: usize,
}

/// Read the feature header at `off`, or `None` for every malformed-framing case: the end-of-features
/// sentinel, a header running past the chunk, an unknown flag bit, `pt_count == 0`, or holes on a
/// line. `style` + `flags` are the fixed 2-byte prefix of both layouts, and the `WIDE` bit lives in
/// `flags` — so the width is known before any field behind it is needed (v10's trailing flags byte
/// made that impossible, which is why the layout was reordered).
#[inline]
fn read_feature_header(chunk: &[u8], off: usize) -> Option<FeatHeader> {
    if off.checked_add(2).is_none_or(|end| end > chunk.len()) || chunk[off] == CHUNK_END {
        return None;
    }
    let style_id = chunk[off];
    let flags = chunk[off + 1];
    if flags & !(FEATURE_FLAG_16BIT | FEATURE_FLAG_POLYGON | FEATURE_FLAG_HOLES | FEATURE_FLAG_WIDE) != 0 {
        return None;
    }
    let wide = flags & FEATURE_FLAG_WIDE != 0;
    let len = if wide { FEATURE_HEADER_WIDE_LEN } else { FEATURE_HEADER_COMPACT_LEN };
    if off.checked_add(len).is_none_or(|end| end > chunk.len()) {
        return None;
    }
    let (ext_pt_count, ax, ay) = if wide {
        (rd_u16(chunk, off + 2) as usize, rd_i32(chunk, off + 4), rd_i32(chunk, off + 8))
    } else {
        // Compact anchors are **unsigned** — zero-extended, not sign-extended: the packer only picks
        // this layout when both fit `0..=65535`.
        (chunk[off + 2] as usize, rd_u16(chunk, off + 3) as i32, rd_u16(chunk, off + 5) as i32)
    };
    if ext_pt_count == 0 || (flags & FEATURE_FLAG_HOLES != 0 && flags & FEATURE_FLAG_POLYGON == 0) {
        return None;
    }
    Some(FeatHeader { style_id, flags, ext_pt_count, ax, ay, len })
}

/// Decode the single feature whose header starts at `off` in `chunk`, into `points`/
/// `ring_lens` (cleared first), returning its [`FeatureRef`] (borrowing those buffers, with
/// [`FeatureRef::offset`] set to `off`) plus the offset just past it. A malformed/capacity result
/// also leaves both buffers empty, so it is safe to call with an untrusted `off` (issue #564's
/// pass-B re-decode hands back a `FeatureRef::offset` from earlier this frame). `node` gives the
/// leaf's min corner, the per-feature anchor base. This is the exact decode
/// [`decode_chunk_into`] runs, so a feature decodes byte-for-byte identically whether it comes from
/// the full-chunk walk or from [`Reader::decode_feature_at`].
enum DecodeOne<'a> {
    Complete(FeatureRef<'a>, usize),
    Dropped(FeatureDecodeError, usize),
}

fn decode_one_feature<'b, const P: usize, const R: usize>(
    chunk: &[u8],
    off: usize,
    node: &BBox,
    points: &'b mut Vec<(i32, i32), P>,
    ring_lens: &'b mut Vec<usize, R>,
) -> DecodeOne<'b> {
    points.clear();
    ring_lens.clear();
    let head = match read_feature_header(chunk, off) {
        Some(head) => head,
        None => return dropped_feature(points, ring_lens, FeatureDecodeError::Malformed, chunk.len()),
    };
    let FeatHeader { style_id, flags, ext_pt_count, ax, ay, len } = head;
    let feat_off = off;
    let mut off = off + len;

    let is_16 = flags & FEATURE_FLAG_16BIT != 0;
    let is_poly = flags & FEATURE_FLAG_POLYGON != 0;
    let has_holes = flags & FEATURE_FLAG_HOLES != 0;
    let dsize = if is_16 { 2 } else { 1 };

    let anchor = (node.min_lon.wrapping_add(ax), node.min_lat.wrapping_add(ay));

    let mut bounds = Bounds::new();

    let mut failure = None;
    let ext_end = match ring_end(chunk, off, ext_pt_count, false, dsize) {
        Some(end) => end,
        None => return dropped_feature(points, ring_lens, FeatureDecodeError::Malformed, chunk.len()),
    };
    if ext_pt_count > points.capacity() {
        failure = Some(FeatureDecodeError::Capacity(CapacityError::Points));
    } else if ring_lens.is_full() {
        failure = Some(FeatureDecodeError::Capacity(CapacityError::Rings));
    } else {
        read_ring(chunk, off, ext_pt_count, anchor, is_16, false, points, &mut bounds);
        ring_lens.push(ext_pt_count).unwrap();
    }
    off = ext_end;

    if is_poly && has_holes {
        let hole_count = match chunk.get(off) {
            Some(count) => *count as usize,
            None => return dropped_feature(points, ring_lens, FeatureDecodeError::Malformed, chunk.len()),
        };
        off += 1;
        for _ in 0..hole_count {
            if off.checked_add(2).is_none_or(|end| end > chunk.len()) {
                return dropped_feature(points, ring_lens, FeatureDecodeError::Malformed, chunk.len());
            }
            let hpc = rd_u16(chunk, off) as usize;
            off += 2;
            if hpc == 0 {
                return dropped_feature(points, ring_lens, FeatureDecodeError::Malformed, chunk.len());
            }
            let end = match ring_end(chunk, off, hpc, true, dsize) {
                Some(end) => end,
                None => return dropped_feature(points, ring_lens, FeatureDecodeError::Malformed, chunk.len()),
            };
            if failure.is_none() {
                if hpc > points.capacity() - points.len() {
                    failure = Some(FeatureDecodeError::Capacity(CapacityError::Points));
                } else if ring_lens.is_full() {
                    failure = Some(FeatureDecodeError::Capacity(CapacityError::Rings));
                } else {
                    read_ring(chunk, off, hpc, anchor, is_16, true, points, &mut bounds);
                    ring_lens.push(hpc).unwrap();
                }
            }
            off = end;
        }
    }

    if let Some(error) = failure {
        return dropped_feature(points, ring_lens, error, off);
    }

    let fref = FeatureRef {
        style_id,
        kind: if is_poly { Kind::Polygon } else { Kind::Line },
        points,
        ring_lens,
        bbox: bounds.to_bbox(),
        offset: feat_off,
    };
    DecodeOne::Complete(fref, off)
}

#[inline]
fn dropped_feature<'a, const P: usize, const R: usize>(
    points: &mut Vec<(i32, i32), P>,
    ring_lens: &mut Vec<usize, R>,
    error: FeatureDecodeError,
    next: usize,
) -> DecodeOne<'a> {
    points.clear();
    ring_lens.clear();
    DecodeOne::Dropped(error, next)
}

fn ring_end(chunk: &[u8], off: usize, pt_count: usize, is_hole: bool, dsize: usize) -> Option<usize> {
    if pt_count == 0 {
        return None;
    }
    let num_deltas = if is_hole { pt_count } else { pt_count - 1 };
    let bytes = num_deltas.checked_mul(dsize.checked_mul(2)?)?;
    let end = off.checked_add(bytes)?;
    (end <= chunk.len()).then_some(end)
}

fn skip_feature(chunk: &[u8], off: usize) -> Result<usize, FeatureDecodeError> {
    let head = read_feature_header(chunk, off).ok_or(FeatureDecodeError::Malformed)?;
    let FeatHeader { flags, ext_pt_count, len, .. } = head;
    let is_poly = flags & FEATURE_FLAG_POLYGON != 0;
    let has_holes = flags & FEATURE_FLAG_HOLES != 0;
    let dsize = if flags & FEATURE_FLAG_16BIT != 0 { 2 } else { 1 };
    let mut next = ring_end(chunk, off + len, ext_pt_count, false, dsize).ok_or(FeatureDecodeError::Malformed)?;
    if is_poly && has_holes {
        let hole_count = *chunk.get(next).ok_or(FeatureDecodeError::Malformed)? as usize;
        next += 1;
        for _ in 0..hole_count {
            if next.checked_add(2).is_none_or(|end| end > chunk.len()) {
                return Err(FeatureDecodeError::Malformed);
            }
            let hpc = rd_u16(chunk, next) as usize;
            next += 2;
            next = ring_end(chunk, next, hpc, true, dsize).ok_or(FeatureDecodeError::Malformed)?;
        }
    }
    Ok(next)
}

#[allow(clippy::too_many_arguments)]
fn read_ring<const P: usize>(
    chunk: &[u8],
    mut off: usize,
    pt_count: usize,
    anchor: (i32, i32),
    is_16: bool,
    is_hole: bool,
    out: &mut Vec<(i32, i32), P>,
    bounds: &mut Bounds,
) {
    let (mut px, mut py) = anchor;
    let num_deltas = if is_hole {
        // holes store all points as deltas (first relative to anchor)
        pt_count
    } else {
        out.push(anchor).unwrap();
        bounds.add(anchor.0, anchor.1);
        pt_count - 1
    };
    for _ in 0..num_deltas {
        let (dx, dy) = if is_16 {
            (rd_i16(chunk, off) as i32, rd_i16(chunk, off + 2) as i32)
        } else {
            (chunk[off] as i8 as i32, chunk[off + 1] as i8 as i32)
        };
        off += if is_16 { 4 } else { 2 };
        px = px.wrapping_add(dx);
        py = py.wrapping_add(dy);
        out.push((px, py)).unwrap();
        bounds.add(px, py);
    }
}

/// Parse the `lod_count` LOD-table entries (resident from `src`); validates each layer's
/// index/table/chunk region lies within the file (`total` bytes) so `for_each_chunk`/`decode_chunk`
/// can skip bounds math, and that its `chunk_size` fits the decode scratch ([`MAX_CHUNK_BYTES`]).
///
/// The offset-table layout costs one extra `uint32` read per LOD: the table's **last** entry is the layer's total
/// chunk bytes ([`Lod::chunk_bytes_total`]), which both bounds the region here and bounds every
/// later per-chunk offset pair in [`Reader::chunk_range`] with no further reads.
pub(crate) fn parse_lod_table(
    src: &dyn ByteSource,
    scale: OffsetScale,
    offset: usize,
    lod_count: usize,
    total: usize,
) -> Result<Vec<Lod, 16>, Error> {
    let mut lods = Vec::new();
    let mut e = [0u8; LOD_ENTRY_LEN];
    // The lowest byte a scaled offset in this file can name past the header (§1.2).
    let floor = super::resolve_bytes(scale.align_up(HEADER_LEN as u64).ok_or(Error::BadOffset)?)?;
    for k in 0..lod_count {
        let o = offset + k * LOD_ENTRY_LEN;
        src.read_at(o as u32, &mut e).map_err(Error::Source)?;
        let mut lod = Lod {
            max_mpp: rd_f32(&e, 0),
            index_offset: super::resolve(scale.offset(rd_u32(&e, 4)))?,
            node_count: rd_u32(&e, 8) as usize,
            chunk_size: rd_u16(&e, 12) as usize,
            chunk_count: rd_u32(&e, 14) as usize,
            chunk_units_total: 0,
            scale,
        };
        // Checked: a corrupt entry's `node_count`/`chunk_count` products can wrap `usize` on the
        // 32-bit target, so an unchecked `data_start` could land below `total` and admit a layer
        // indexing out of the file.
        let data_start = lod.data_start().ok_or(Error::BadOffset)?;
        if lod.index_offset < floor || data_start > total {
            return Err(Error::BadOffset);
        }
        // A chunk decodes into the resident scratch, so reject a `chunk_size` over
        // [`MAX_CHUNK_BYTES`] rather than silently dropping its geometry at render time.
        if lod.chunk_size > MAX_CHUNK_BYTES {
            return Err(Error::BadOffset);
        }
        // `offsets[chunk_count]` is the table's last entry — since v14 that is *not* the four bytes
        // below `data_start`, because the rounding step may have put filler between the two. The
        // table always carries at least this one entry, and it lies inside the region the
        // `data_start` guard above bounded.
        let last = lod.table_end().and_then(|end| end.checked_sub(4)).ok_or(Error::BadOffset)?;
        let mut t = [0u8; 4];
        src.read_at(last as u32, &mut t).map_err(Error::Source)?;
        lod.chunk_units_total = rd_u32(&t, 0);
        let region = scale.offset(lod.chunk_units_total).bytes();
        if u64::try_from(data_start).ok().and_then(|s| s.checked_add(region)).is_none_or(|end| end > total as u64) {
            return Err(Error::BadOffset);
        }
        let _ = lods.push(lod);
    }
    Ok(lods)
}
