use super::*;
use obc_formats::io::rd_i16;
use obc_formats::obcm::{NAV_EDGE_FIXED_LEN, NAV_NEIGHBOR_ASCENT_OFF, NAV_NEIGHBOR_LEN, NAV_NODE_FIXED_LEN};
#[derive(Debug, Clone, Copy)]
pub struct NavNodeRef<'a> {
    /// Absolute microdegrees.
    pub lat: i32,
    /// Absolute microdegrees.
    pub lon: i32,
    /// The pack-run-dense node id (the A* hash key).
    pub id: u32,
    /// Raw neighbor bytes, `degree` × [`NAV_NEIGHBOR_LEN`] — length-validated by the walk.
    neighbors: &'a [u8],
}

impl<'a> NavNodeRef<'a> {
    /// This junction's degree; the packer caps it far lower.
    #[inline]
    pub fn degree(&self) -> usize {
        self.neighbors.len() / NAV_NEIGHBOR_LEN
    }

    /// Iterate the adjacency entries in record order. Each neighbor's absolute coord is
    /// reconstructed as `record coord + i16 delta` through `from_le_bytes` on the slice, never a
    /// typed view.
    #[inline]
    pub fn neighbors(&self) -> impl Iterator<Item = NavNeighbor> + 'a {
        let (base_lat, base_lon) = (self.lat, self.lon);
        self.neighbors.as_chunks::<NAV_NEIGHBOR_LEN>().0.iter().map(move |e| NavNeighbor {
            id: rd_u32(e, 0),
            lat: base_lat.wrapping_add(rd_i16(e, 4) as i32),
            lon: base_lon.wrapping_add(rd_i16(e, 6) as i32),
            edge_id: rd_u32(e, 8),
            cost_m: rd_u16(e, 12) as u32,
            way_kind: e[14],
            ascent_m: rd_u16(e, NAV_NEIGHBOR_ASCENT_OFF),
        })
    }
}

pub(in crate::reader) struct LegacyReader<'a>(pub Reader<'a>);
impl<'a> core::ops::Deref for LegacyReader<'a> {
    type Target = Reader<'a>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl<'a> LegacyReader<'a> {
    /// Visit every junction record whose quadtree leaf overlaps `view`, in quadtree order: the A*
    /// spatial-refetch primitive.
    ///
    /// `scratch` is the caller-owned chunk buffer and must hold the directory's `chunk_size`
    /// bytes, or this returns `Err(Error::TooShort)`. A truncated record ends that chunk cleanly;
    /// source or index-cache failures return a typed [`Error`] rather than looking like an empty
    /// leaf.
    ///
    /// # Reentrancy
    ///
    /// The walk streams through the internal index cache; legal re-entry returns
    /// [`Error::CacheBusy`].
    pub fn for_each_nav_node(
        &self,
        view: &BBox,
        scratch: &mut [u8],
        mut visit: impl FnMut(NavNodeRef),
    ) -> Result<(), Error> {
        let dir = *self.nav_directory();
        if dir.is_empty() {
            return Ok(());
        }
        if scratch.len() < dir.chunk_size {
            return Err(Error::TooShort);
        }
        let mut read_error = None;
        self.for_each_nav_chunk(view, |cid| {
            if read_error.is_some() {
                return;
            }
            // A leaf that names no chunk of the section is skipped, not fatal.
            if let Err(Error::Source(error)) = self.for_each_nav_node_in_chunk(cid, scratch, &mut visit) {
                read_error = Some(error);
            }
        })?;
        if let Some(error) = read_error {
            return Err(Error::Source(error));
        }
        Ok(())
    }

    /// Visit the node chunk id of every non-empty nav leaf overlapping `view`, in quadtree order. It
    /// reads only the index; bin packing lets several leaves name one chunk.
    ///
    /// # Reentrancy
    ///
    /// As [`Reader::for_each_nav_node`].
    pub fn for_each_nav_chunk(&self, view: &BBox, mut visit: impl FnMut(u32)) -> Result<(), Error> {
        let dir = *self.nav_directory();
        if dir.is_empty() {
            return Ok(());
        }
        self.walk_leaves(&dir, 0, self.bbox, view, 0, &mut |cid, _node| visit(cid)).map_err(Error::from)
    }

    /// Visit every junction record of node chunk `chunk_id`, in record order. `scratch` is as for
    /// [`Reader::for_each_nav_node`]; an id past the section's chunks is [`Error::BadOffset`].
    pub fn for_each_nav_node_in_chunk(
        &self,
        chunk_id: u32,
        scratch: &mut [u8],
        mut visit: impl FnMut(NavNodeRef),
    ) -> Result<(), Error> {
        let dir = self.nav_directory();
        if scratch.len() < dir.chunk_size {
            return Err(Error::TooShort);
        }
        let (start, end) = dir.chunk_range(chunk_id).ok_or(Error::BadOffset)?;
        if end > self.src.len() {
            return Err(Error::BadOffset);
        }
        let chunk = &mut scratch[..dir.chunk_size];
        self.src.read_at(start, chunk).map_err(Error::Source)?;
        decode_nav_chunk(chunk, &mut visit);
        Ok(())
    }

    /// Resolve an `Edge Id`, a packed `(chunk_index, ordinal)` pair, to the chunk that holds the
    /// record and the record's byte position inside it.
    ///
    /// The `ordinal` is a position within the chunk, not a byte offset, so this reads the one
    /// chunk and walks `ordinal` records from its first byte. There is no extra I/O: the chunk
    /// that holds the record holds every record before it. A refused id is a malformed map, but
    /// every caller degrades to "no geometry" rather than panicking.
    fn nav_edge_record<'t>(&self, tiles: &'t mut NavTileCache, edge_id: u32) -> Option<(&'t [u8], usize)> {
        let chunk_start = self.nav_edge_chunk_start(edge_id)?;
        let chunk = tiles.chunk(self.src, chunk_start, NAV_CHUNK_SIZE)?;
        let (start, _end) = nav_edge_record_range(chunk, nav_edge_id_ordinal(edge_id))?;
        Some((chunk, start))
    }

    /// The absolute file offset of the chunk holding `edge_id`'s record, split out so the two
    /// readers below differ in one line: a cache working set against 512 bytes of stack.
    fn nav_edge_chunk_start(&self, edge_id: u32) -> Option<u64> {
        let dir = self.nav_directory();
        let cs = dir.chunk_size;
        if dir.edge_chunk_count == 0 || cs != NAV_CHUNK_SIZE {
            return None;
        }
        let chunk_index = nav_edge_id_chunk(edge_id) as u64;
        if chunk_index >= dir.edge_chunk_count as u64 {
            return None;
        }
        let chunk_start = dir.edge_pool_offset.checked_add(chunk_index.checked_mul(cs as u64)?)?;
        if chunk_start.checked_add(cs as u64)? > self.src.len() {
            return None;
        }
        Some(chunk_start)
    }

    /// [`Reader::nav_edge_record`] with the chunk read into a caller-owned 512-byte buffer instead
    /// of through a [`NavTileCache`], which is 24,852 bytes and would put two thirds of the
    /// device's stack into one frame for a single read.
    fn nav_edge_record_uncached<'b>(
        &self,
        buf: &'b mut [u8; NAV_CHUNK_SIZE],
        edge_id: u32,
    ) -> Option<(&'b [u8], usize)> {
        let chunk_start = self.nav_edge_chunk_start(edge_id)?;
        self.src.read_at(chunk_start, &mut buf[..]).ok()?;
        let (start, _end) = nav_edge_record_range(&buf[..], nav_edge_id_ordinal(edge_id))?;
        Some((&buf[..], start))
    }

    /// Surface class and whether every terrain integration sample was present.
    pub fn nav_edge_facts(&self, edge_id: u32) -> Option<(u8, bool)> {
        let mut bytes = [0u8; NAV_EDGE_STACK_BUDGET];
        let (chunk, within) = self.nav_edge_record_uncached(&mut bytes, edge_id)?;
        Some((chunk[within + 6] >> 5, rd_u16(chunk, within + 4) & 0x8000 != 0))
    }

    /// Fetch one edge polyline by its `edge_id`, decoding the anchor and deltas into `points` as
    /// `(lon, lat)` µdeg pairs. Returns the edge's `length_m`.
    ///
    /// `None` for an empty graph, an id the walk refuses, a read failure, or a polyline longer
    /// than `P`. No cache is touched, so this is safe to call from anywhere; it holds one chunk on
    /// the stack, which is what the ordinal walk needs.
    pub fn nav_edge<const P: usize>(&self, edge_id: u32, points: &mut Vec<(i32, i32), P>) -> Option<u32> {
        points.clear();
        let mut chunk_buf = [0u8; NAV_EDGE_STACK_BUDGET];
        let (chunk, within) = self.nav_edge_record_uncached(&mut chunk_buf, edge_id)?;
        let length_m = rd_u32(chunk, within);
        let pt_count = (rd_u16(chunk, within + 4) & 0x7fff) as usize;
        // Byte 6 is `way_kind`; the anchor sits behind it, at 7 (lat) and 11 (lon).
        let anchor_lat = rd_i32(chunk, within + 7);
        let anchor_lon = rd_i32(chunk, within + 11);
        // The walk already refused `Pt Count < 2` and any record claiming bytes past its chunk.
        let rec_len = NAV_EDGE_FIXED_LEN + (pt_count - 1) * 4;
        if pt_count > P {
            return None; // caller's buffer can't hold the polyline — corrupt or mis-sized
        }
        points.push((anchor_lon, anchor_lat)).ok()?;
        let (mut lat, mut lon) = (anchor_lat, anchor_lon);
        for pair in chunk[within + NAV_EDGE_FIXED_LEN..within + rec_len].as_chunks::<4>().0 {
            lat = lat.wrapping_add(rd_i16(pair, 0) as i32);
            lon = lon.wrapping_add(rd_i16(pair, 2) as i32);
            points.push((lon, lat)).ok()?;
        }
        Some(length_m)
    }

    /// [`Reader::for_each_nav_node`] with the chunk read routed through a caller-owned
    /// [`NavTileCache`]: the router's settle primitive. A* pops the globally best node, so
    /// successive settles scatter across the frontier's live leaves and the working set keeps them
    /// resident. Same decode and reentrancy rule as the uncached walk.
    pub fn for_each_nav_node_cached(
        &self,
        view: &BBox,
        tiles: &mut NavTileCache,
        mut visit: impl FnMut(NavNodeRef),
    ) -> Result<(), Error> {
        let dir = *self.nav_directory();
        if dir.is_empty() {
            return Ok(());
        }
        let mut read_error = None;
        self.walk_nav_leaves(&dir, 0, self.bbox, view, 0, tiles, &mut |tiles, cid, _node| {
            if read_error.is_some() {
                return;
            }
            let (start, end) = match dir.chunk_range(cid) {
                Some(r) => r,
                None => return,
            };
            if end > self.src.len() {
                return;
            }
            // A failed fill skips this leaf cleanly (the cache never keeps a bad slot).
            match tiles.chunk(self.src, start, dir.chunk_size) {
                Some(chunk) => decode_nav_chunk(chunk, &mut |node| visit(node)),
                None => read_error = Some(IoError::Io),
            }
        })
        .map_err(Error::from)?;
        if let Some(error) = read_error {
            return Err(Error::Source(error));
        }
        Ok(())
    }

    /// Return a projection only when exactly one nearby edge qualifies.
    pub fn unique_nav_edge_candidate_cached(
        &self,
        view: &BBox,
        tiles: &mut NavTileCache,
        p: (i32, i32),
        max_distance_m: f32,
    ) -> Result<Option<NavEdgeCandidate>, Error> {
        self.nav_edge_candidate_cached(view, tiles, p, max_distance_m, true)
    }

    /// Find the nearest exact edge projection among edges incident to graph nodes in `view` and
    /// edges named by interior anchors in `view`. Together the two indexes are the completeness
    /// argument: endpoints cover short edges, and a long edge has anchors at most 300 m apart.
    #[inline(never)] // keep the one 512-byte node-chunk copy in this bounded snap-only frame
    pub fn nearest_nav_edge_candidate_cached(
        &self,
        view: &BBox,
        tiles: &mut NavTileCache,
        p: (i32, i32),
        max_distance_m: f32,
    ) -> Result<Option<NavEdgeCandidate>, Error> {
        self.nav_edge_candidate_cached(view, tiles, p, max_distance_m, false)
    }

    #[inline(never)]
    fn nav_edge_candidate_cached(
        &self,
        view: &BBox,
        tiles: &mut NavTileCache,
        p: (i32, i32),
        max_distance_m: f32,
        require_unique: bool,
    ) -> Result<Option<NavEdgeCandidate>, Error> {
        let dir = *self.nav_directory();
        if dir.is_empty() {
            return Ok(None);
        }
        let mut best: Option<NavEdgeCandidate> = None;
        let mut ambiguous = false;
        let mut read_error = None;

        // First the ordinary node tree, which supplies every short edge. Copy each chunk before
        // edge-pool reads can evict its cache slot.
        self.walk_nav_leaves(&dir, 0, self.bbox, view, 0, tiles, &mut |tiles, cid, _node| {
            if read_error.is_some() {
                return;
            }
            let Some((start, end)) = dir.chunk_range(cid) else { return };
            if end > self.src.len() {
                return;
            }
            let mut local = [0u8; NAV_MAX_CHUNK_BYTES];
            {
                let Some(chunk) = tiles.chunk(self.src, start, dir.chunk_size) else {
                    read_error = Some(IoError::Io);
                    return;
                };
                local[..dir.chunk_size].copy_from_slice(chunk);
            }
            decode_nav_chunk(&local[..dir.chunk_size], &mut |n| {
                if n.lon < view.min_lon || n.lon > view.max_lon || n.lat < view.min_lat || n.lat > view.max_lat {
                    return;
                }
                for nb in n.neighbors() {
                    let Some(candidate) = self.project_nav_edge_cached(tiles, nb.edge_id, p) else {
                        if require_unique {
                            read_error = Some(IoError::Io);
                            return;
                        }
                        continue;
                    };
                    if candidate.distance_m <= max_distance_m
                        && best.is_some_and(|old| old.edge_id != candidate.edge_id)
                    {
                        ambiguous = true;
                    }
                    if candidate.distance_m <= max_distance_m
                        && best.is_none_or(|old| candidate_beats(&candidate, &old))
                    {
                        best = Some(candidate);
                    }
                }
            });
        })
        .map_err(Error::from)?;

        // Then the sparse long-edge anchors. Leaves may share chunks, so filter by the absolute
        // record coordinate.
        if dir.snap_node_count > 0 && read_error.is_none() {
            let index = NavSnapIndex { index_offset: dir.snap_index_offset, node_count: dir.snap_node_count };
            self.walk_nav_leaves(&index, 0, self.bbox, view, 0, tiles, &mut |tiles, cid, _node| {
                if read_error.is_some() {
                    return;
                }
                let Some((start, end)) = dir.snap_chunk_range(cid) else { return };
                if end > self.src.len() {
                    return;
                }
                let mut local = [CHUNK_END; NAV_CHUNK_SIZE];
                {
                    let Some(chunk) = tiles.chunk(self.src, start, dir.chunk_size) else {
                        read_error = Some(IoError::Io);
                        return;
                    };
                    local[..dir.chunk_size].copy_from_slice(chunk);
                }
                for rec in local[..dir.chunk_size].as_chunks::<NAV_SNAP_RECORD_LEN>().0 {
                    let edge_id = rd_u32(rec, 8);
                    if edge_id == u32::MAX {
                        break;
                    }
                    let (lat, lon) = (rd_i32(rec, 0), rd_i32(rec, 4));
                    if lon < view.min_lon || lon > view.max_lon || lat < view.min_lat || lat > view.max_lat {
                        continue;
                    }
                    let Some(candidate) = self.project_nav_edge_cached(tiles, edge_id, p) else {
                        if require_unique {
                            read_error = Some(IoError::Io);
                            return;
                        }
                        continue;
                    };
                    if candidate.distance_m <= max_distance_m
                        && best.is_some_and(|old| old.edge_id != candidate.edge_id)
                    {
                        ambiguous = true;
                    }
                    if candidate.distance_m <= max_distance_m
                        && best.is_none_or(|old| candidate_beats(&candidate, &old))
                    {
                        best = Some(candidate);
                    }
                }
            })
            .map_err(Error::from)?;
        }
        if let Some(error) = read_error {
            return Err(Error::Source(error));
        }
        Ok(if require_unique && ambiguous { None } else { best })
    }

    /// Resolve the winning candidate's endpoint ids and directional ascents through two
    /// degenerate node-tree queries. Only the winner pays the lookups.
    pub fn resolve_nav_edge_candidate_cached(
        &self,
        candidate: NavEdgeCandidate,
        tiles: &mut NavTileCache,
    ) -> Result<Option<NavEdgeSnap>, Error> {
        let mut ids: Option<(u32, u32)> = None;
        let mut ascent_ab = None;
        let mut ascent_ba = None;
        for (coord, other, forward) in
            [(candidate.a_coord, candidate.b_coord, true), (candidate.b_coord, candidate.a_coord, false)]
        {
            let view = BBox { min_lon: coord.0, min_lat: coord.1, max_lon: coord.0, max_lat: coord.1 };
            self.for_each_nav_node_cached(&view, tiles, |n| {
                if (n.lon, n.lat) != coord {
                    return;
                }
                for nb in n.neighbors() {
                    if nb.edge_id != candidate.edge_id || (nb.lon, nb.lat) != other {
                        continue;
                    }
                    let pair = if forward { (n.id, nb.id) } else { (nb.id, n.id) };
                    if ids.is_none_or(|old| old == pair) {
                        ids = Some(pair);
                        if forward {
                            ascent_ab = Some(nb.ascent_m);
                        } else {
                            ascent_ba = Some(nb.ascent_m);
                        }
                    }
                }
            })?;
        }
        let Some((a_id, b_id)) = ids else { return Ok(None) };
        Ok(Some(NavEdgeSnap {
            edge_id: candidate.edge_id,
            way_kind: candidate.way_kind,
            length_m: candidate.length_m,
            from_a_m: candidate.from_a_m,
            distance_m: candidate.distance_m,
            position: candidate.position,
            a: NavEdgeEndpoint { id: a_id, coord: candidate.a_coord, position: candidate.a_position },
            b: NavEdgeEndpoint { id: b_id, coord: candidate.b_coord, position: candidate.b_position },
            ascent_ab: ascent_ab.unwrap_or(0),
            ascent_ba: ascent_ba.unwrap_or(0),
        }))
    }

    /// Stream an inclusive edge slice between two exact projected positions, in either direction.
    pub fn nav_edge_slice_oriented(
        &self,
        tiles: &mut NavTileCache,
        edge_id: u32,
        from: NavEdgePosition,
        to: NavEdgePosition,
        mut emit: impl FnMut((i32, i32)),
    ) -> Option<u32> {
        let dir = self.nav_directory();
        let cs = dir.chunk_size;
        if dir.edge_chunk_count == 0 || cs == 0 {
            return None;
        }
        let (chunk, within) = self.nav_edge_record(tiles, edge_id)?;
        let length_m = rd_u32(chunk, within);
        let pt_count = (rd_u16(chunk, within + 4) & 0x7fff) as usize;
        if pt_count < 2 || from.segment as usize + 1 >= pt_count || to.segment as usize + 1 >= pt_count {
            return None;
        }
        let rec_len = NAV_EDGE_FIXED_LEN.checked_add((pt_count - 1).checked_mul(4)?)?;
        if within + rec_len > cs {
            return None;
        }
        let deltas = &chunk[within + NAV_EDGE_FIXED_LEN..within + rec_len];
        let anchor = (rd_i32(chunk, within + 11), rd_i32(chunk, within + 7));
        let step = |(lon, lat): (i32, i32), pair: &[u8]| {
            (
                lon.wrapping_add(i16::from_le_bytes([pair[2], pair[3]]) as i32),
                lat.wrapping_add(i16::from_le_bytes([pair[0], pair[1]]) as i32),
            )
        };
        let forward = (from.segment, from.fraction) <= (to.segment, to.fraction);
        emit(from.coord);
        if forward {
            let mut point = anchor;
            for (i, pair) in deltas.as_chunks::<4>().0.iter().enumerate() {
                point = step(point, pair);
                let vertex = i + 1;
                if vertex > from.segment as usize && vertex <= to.segment as usize {
                    emit(point);
                }
            }
        } else {
            let mut point = anchor;
            for pair in deltas.as_chunks::<4>().0 {
                point = step(point, pair);
            }
            for (i, pair) in deltas.as_chunks::<4>().0.iter().enumerate().rev() {
                point = (
                    point.0.wrapping_sub(i16::from_le_bytes([pair[2], pair[3]]) as i32),
                    point.1.wrapping_sub(i16::from_le_bytes([pair[0], pair[1]]) as i32),
                );
                let vertex = i;
                if vertex <= from.segment as usize && vertex > to.segment as usize {
                    emit(point);
                }
            }
        }
        if to.coord != from.coord {
            emit(to.coord);
        }
        Some(length_m)
    }

    pub(super) fn project_nav_edge_cached(
        &self,
        tiles: &mut NavTileCache,
        edge_id: u32,
        p: (i32, i32),
    ) -> Option<NavEdgeCandidate> {
        let dir = self.nav_directory();
        let cs = dir.chunk_size;
        if dir.edge_chunk_count == 0 || cs == 0 {
            return None;
        }
        let (chunk, within) = self.nav_edge_record(tiles, edge_id)?;
        let length_m = rd_u32(chunk, within);
        let pt_count = (rd_u16(chunk, within + 4) & 0x7fff) as usize;
        if pt_count < 2 || pt_count - 1 > u16::MAX as usize {
            return None;
        }
        let way_kind = chunk[within + 6];
        let rec_len = NAV_EDGE_FIXED_LEN.checked_add((pt_count - 1).checked_mul(4)?)?;
        if within + rec_len > cs {
            return None;
        }
        let deltas = &chunk[within + NAV_EDGE_FIXED_LEN..within + rec_len];
        let anchor = (rd_i32(chunk, within + 11), rd_i32(chunk, within + 7));
        let step = |(lon, lat): (i32, i32), pair: &[u8]| {
            (
                lon.wrapping_add(i16::from_le_bytes([pair[2], pair[3]]) as i32),
                lat.wrapping_add(i16::from_le_bytes([pair[0], pair[1]]) as i32),
            )
        };
        let cl = cos_lat(p.1).max(1e-3);
        let mut a = anchor;
        let mut along = 0.0f32;
        let mut best_distance = f32::INFINITY;
        let mut best_along = 0.0f32;
        let mut best_position = NavEdgePosition { segment: 0, fraction: 0, coord: anchor };
        for (i, pair) in deltas.as_chunks::<4>().0.iter().enumerate() {
            let b = step(a, pair);
            let (t, distance) = project_to_nav_segment(a, b, p, cl);
            let segment_m = ground_dist_m_cl(a, b, cl);
            if distance < best_distance {
                best_distance = distance;
                best_along = along + t * segment_m;
                let lon = a.0.saturating_add(libm::roundf((b.0 - a.0) as f32 * t) as i32);
                let lat = a.1.saturating_add(libm::roundf((b.1 - a.1) as f32 * t) as i32);
                best_position = NavEdgePosition {
                    segment: i as u16,
                    fraction: libm::roundf(t * u16::MAX as f32) as u16,
                    coord: (lon, lat),
                };
            }
            along += segment_m;
            a = b;
        }
        let from_a_m = if along <= f32::EPSILON {
            0
        } else {
            (libm::roundf(length_m as f32 * best_along / along) as u32).min(length_m)
        };
        Some(NavEdgeCandidate {
            edge_id,
            way_kind,
            length_m,
            from_a_m,
            distance_m: best_distance,
            position: best_position,
            a_coord: anchor,
            b_coord: a,
            a_position: NavEdgePosition { segment: 0, fraction: 0, coord: anchor },
            b_position: NavEdgePosition { segment: (pt_count - 2) as u16, fraction: u16::MAX, coord: a },
        })
    }

    /// Route-private node walk. Node words are served from [`NavTileCache`]'s sixteen windows, so
    /// thousands of point descents do not churn the seven render-index windows. The callback gets
    /// the mutable cache only after the node-word borrow has ended.
    #[allow(clippy::too_many_arguments)]
    fn walk_nav_leaves<F: FnMut(&mut NavTileCache, u32, BBox)>(
        &self,
        index: &dyn QuadIndex,
        idx: usize,
        node: BBox,
        view: &BBox,
        depth: u32,
        tiles: &mut NavTileCache,
        visit: &mut F,
    ) -> Result<(), MapReadError> {
        if idx >= index.node_count() || depth > MAX_QUADTREE_DEPTH || !node.intersects(view) {
            return Ok(());
        }
        let val = tiles.index_node(self.src, index, idx).map_err(MapReadError::Source)?;
        if val & BRANCH_BIT == 0 {
            if val != EMPTY_LEAF {
                visit(tiles, val, node);
            }
            return Ok(());
        }
        let child = (val & !BRANCH_BIT) as usize;
        if child <= idx {
            return Err(MapReadError::Malformed);
        }
        let mid_lon = (node.min_lon + node.max_lon).div_euclid(2);
        let mid_lat = (node.min_lat + node.max_lat).div_euclid(2);
        let kids = [
            BBox { min_lon: node.min_lon, min_lat: mid_lat, max_lon: mid_lon, max_lat: node.max_lat },
            BBox { min_lon: mid_lon, min_lat: mid_lat, max_lon: node.max_lon, max_lat: node.max_lat },
            BBox { min_lon: node.min_lon, min_lat: node.min_lat, max_lon: mid_lon, max_lat: mid_lat },
            BBox { min_lon: mid_lon, min_lat: node.min_lat, max_lon: node.max_lon, max_lat: mid_lat },
        ];
        for (i, child_bbox) in kids.iter().enumerate() {
            self.walk_nav_leaves(index, child + i, *child_bbox, view, depth + 1, tiles, visit)?;
        }
        Ok(())
    }

    /// Fetch one edge polyline oriented to begin at `start`, streaming each point through `emit`
    /// and returning the edge's `length_m`.
    ///
    /// Edge records run `a → b` and the router traverses them either way, so this picks the
    /// direction by matching `start` against the record's endpoints, which the packer makes
    /// bit-identical to the node coords, and reverses on the fly. A resident chunk makes the
    /// reversed decode free. `None` for an out-of-pool id, a record matching neither endpoint, or
    /// a read failure.
    pub fn nav_edge_oriented(
        &self,
        tiles: &mut NavTileCache,
        edge_id: u32,
        start: (i32, i32),
        mut emit: impl FnMut((i32, i32)),
    ) -> Option<u32> {
        let dir = self.nav_directory();
        let cs = dir.chunk_size;
        if dir.edge_chunk_count == 0 || cs == 0 {
            return None;
        }
        let (chunk, within) = self.nav_edge_record(tiles, edge_id)?;
        let length_m = rd_u32(chunk, within);
        let pt_count = (rd_u16(chunk, within + 4) & 0x7fff) as usize;
        // Byte +6 is `way_kind`; the anchor sits behind it, at +7 (lat) and +11 (lon).
        let anchor = (rd_i32(chunk, within + 11), rd_i32(chunk, within + 7)); // (lon, lat)
        if pt_count == 0 {
            return None;
        }
        let rec_len = NAV_EDGE_FIXED_LEN.checked_add((pt_count - 1).checked_mul(4)?)?;
        if within + rec_len > cs {
            return None;
        }
        let deltas = &chunk[within + NAV_EDGE_FIXED_LEN..within + rec_len];
        let step = |(lon, lat): (i32, i32), pair: &[u8]| {
            (lon.wrapping_add(rd_i16(pair, 2) as i32), lat.wrapping_add(rd_i16(pair, 0) as i32))
        };
        if anchor == start {
            // Forward: the record already runs `start → …`.
            let mut p = anchor;
            emit(p);
            for pair in deltas.as_chunks::<4>().0 {
                p = step(p, pair);
                emit(p);
            }
            return Some(length_m);
        }
        // Maybe reversed: forward-sum the deltas for the `b` endpoint…
        let mut p = anchor;
        for pair in deltas.as_chunks::<4>().0 {
            p = step(p, pair);
        }
        if p != start {
            return None; // matches neither endpoint — a stale/corrupt edge id
        }
        // …then walk them backward, undoing one delta per point.
        emit(p);
        for pair in deltas.as_chunks::<4>().0.iter().rev() {
            p = (p.0.wrapping_sub(rd_i16(pair, 2) as i32), p.1.wrapping_sub(rd_i16(pair, 0) as i32));
            emit(p);
        }
        Some(length_m)
    }
}
fn decode_nav_chunk(chunk: &[u8], visit: &mut impl FnMut(NavNodeRef)) {
    let mut off = 0usize;
    while off + NAV_NODE_FIXED_LEN <= chunk.len() {
        let degree = chunk[off + 12] as usize;
        if degree == usize::from(CHUNK_END) {
            break;
        }
        let end = off + NAV_NODE_FIXED_LEN + degree * NAV_NEIGHBOR_LEN;
        if end > chunk.len() {
            break;
        }
        visit(NavNodeRef {
            lat: rd_i32(chunk, off),
            lon: rd_i32(chunk, off + 4),
            id: rd_u32(chunk, off + 8),
            neighbors: &chunk[off + NAV_NODE_FIXED_LEN..end],
        });
        off = end;
    }
}

use obc_formats::obcm::{NAV_EDGE_MIN_LEN, NAV_EDGE_POINT_COUNT_MASK, NAV_EDGE_PT_COUNT_SENTINEL};
pub fn nav_edge_step(chunk: &[u8], p: usize) -> Option<usize> {
    if chunk.len() < NAV_CHUNK_SIZE {
        return None;
    }
    if p.checked_add(NAV_EDGE_MIN_LEN)? > NAV_CHUNK_SIZE {
        return None;
    }
    let n = rd_u16(chunk, p + 4);
    if n == NAV_EDGE_PT_COUNT_SENTINEL {
        return None;
    }
    let n = n & NAV_EDGE_POINT_COUNT_MASK;
    if n < 2 {
        return None;
    }
    // 32-bit evaluation; `n - 1` cannot underflow behind the `n < 2` refusal.
    let len = NAV_EDGE_FIXED_LEN as u32 + 4 * (n as u32 - 1);
    let len = len as usize;
    if p.checked_add(len)? > NAV_CHUNK_SIZE {
        return None;
    }
    Some(len)
}

/// Resolve `ordinal` to the record's byte range within its chunk, walking from the chunk's first
/// byte and taking each record's length from its own `Pt Count`. Every record the walk touches
/// gets the same checks, and a refused id is a malformed map, not an absent edge.
#[inline]
pub fn nav_edge_record_range(chunk: &[u8], ordinal: u32) -> Option<(usize, usize)> {
    let mut p = 0usize;
    for _ in 0..ordinal {
        p += nav_edge_step(chunk, p)?;
    }
    let len = nav_edge_step(chunk, p)?;
    Some((p, p + len))
}

use super::super::cache::{WalkEntry, WALK_CACHE_ENTRIES};
use super::super::{expand_walk_bbox, intersect_bbox, CacheError};
impl<'a> LegacyReader<'a> {
    fn walk_leaves<F: FnMut(u32, BBox)>(
        &self,
        index: &dyn QuadIndex,
        idx: usize,
        node: BBox,
        view: &BBox,
        depth: u32,
        visit: &mut F,
    ) -> Result<(), MapReadError> {
        // The depth cap is the hard stack bound against a corrupt cyclic branch.
        if idx >= index.node_count() || depth > MAX_QUADTREE_DEPTH || !node.intersects(view) {
            return Ok(());
        }
        // Read the node before descending, so the index-cache borrow is released before a leaf's
        // `visit` triggers a geometry-chunk read.
        let val = self.read_node(index, idx)?;
        if val & BRANCH_BIT == 0 {
            if val != EMPTY_LEAF {
                visit(val, node);
            }
            return Ok(());
        }
        let child = (val & !BRANCH_BIT) as usize;
        // The packer flattens the quadtree breadth-first, so a branch's children lie after it. A
        // back-reference appears only in a corrupt map and would re-enter a node on the stack.
        if child <= idx {
            return Err(MapReadError::Malformed);
        }
        // Floor-division midpoints must match the packer's split.
        let mid_lon = (node.min_lon + node.max_lon).div_euclid(2);
        let mid_lat = (node.min_lat + node.max_lat).div_euclid(2);
        // NW, NE, SW, SE
        let kids = [
            BBox { min_lon: node.min_lon, min_lat: mid_lat, max_lon: mid_lon, max_lat: node.max_lat },
            BBox { min_lon: mid_lon, min_lat: mid_lat, max_lon: node.max_lon, max_lat: node.max_lat },
            BBox { min_lon: node.min_lon, min_lat: node.min_lat, max_lon: mid_lon, max_lat: mid_lat },
            BBox { min_lon: mid_lon, min_lat: node.min_lat, max_lon: node.max_lon, max_lat: mid_lat },
        ];
        for (i, kb) in kids.iter().enumerate() {
            self.walk_leaves(index, child + i, *kb, view, depth + 1, visit)?;
        }
        Ok(())
    }
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

        // A successful prior expanded walk is a complete ordered leaf list for every query inside
        // its cover. Copy it out before the callback, which loads geometry and so borrows this
        // same RefCell.
        let cached = self.cache.try_borrow_mut().map_err(MapReadError::Cache)?.cached_walk(lod as u8, &query);
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
            self.cache.try_borrow_mut().map_err(MapReadError::Cache)?.store_walk(lod as u8, cover, &entries);
        }
        Ok(())
    }

    /// Geometry-only walk that opportunistically explores `cover` while preserving the exact
    /// `primary` query's behaviour. Once the result budget overflows, later recursion shrinks back
    /// to `primary`, and an error found only in the speculative margin abandons caching rather
    /// than failing a query that never touched that node.
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
}
