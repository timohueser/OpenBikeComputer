//! On-device point-to-point routing over the OBCM §8 nav graph (epic #116, R3).
//!
//! [`plan_route`] runs A* from the rider's fix to a POI and writes the result as a
//! complete OBCR through the shared [`ObcrEmitter`], so the caller (R4) saves it to
//! `/routes/_nav.obcr` and the rest of the device can't tell it from a loaded GPX
//! route. `no_std`, identical on device and sim; every buffer lives in caller-owned
//! structs (`NavScratch` + the reader's `NavTileCache`) because this later runs under
//! the nRF's ~36 kB stack next to the render peak — a fat local here is a HardFault
//! on-glass (#419/#270).
//!
//! **Expansion by spatial re-fetch, not an id→offset table** (locked on #116): a
//! global node-id index would be millions of entries and can't be resident. Settling
//! a node is one quadtree descent to its coord's leaf (a degenerate one-point view) +
//! one chunk read + relaxing its neighbors straight off the record — each neighbor's
//! coord and cost are inline (§8.3), so the heuristic needs no second fetch.
//! Consecutive settles have strong spatial locality; the [`NavTileCache`] turns the
//! per-settle re-read into a resident-slot hit (the device is SD-bound — the cache's
//! hit/miss counters are the number R4 logs on-glass).
//!
//! **The scratch is fixed** (~10 kB, [`NAV_MAX_NODES`] tracked nodes) because the
//! router must coexist with the map cache and the BLE stack in RAM; on a dense graph
//! it fills and the search aborts with [`NavError::Exhausted`] — the locked
//! "short routes only" framing accepts that a dense-urban 10 km route can fail.
//!
//! **Admissibility invariant:** the heuristic is the great-circle distance to the
//! goal node measured by [`ground_dist_m`] — the *same* local-equirectangular metric
//! the packer summed for every edge's `cost_m` (`obc-reader`'s shared distance core).
//! Any path's cost is a sum of polyline lengths ≥ the straight line between its ends
//! in that same metric, so `h` never overestimates by construction and A* returns the
//! true shortest path.

use heapless::Vec;

use crate::byte_io::ByteSink;
use crate::convert::{EmitStats, ObcrEmitter, RouteStats, WpPlace, MAX_WAYPOINTS};
use crate::geo::{cos_lat, ground_dist_m, ground_dist_m_cl};
use obc_reader::{BBox, NavTileCache, Reader, M_PER_DEG};

/// Crow-flies routing cap, meters (locked on #116): a farther target is rejected as
/// [`NavError::TooFar`] before any graph access.
const MAX_CROW_FLIES_M: f32 = 10_000.0;

/// Snap radius, meters (locked on #116): each endpoint snaps to the nearest routable
/// node within this, or the route fails as [`NavError::NoPath`]. v1 snaps to nodes,
/// not mid-edge (a noted future refinement).
const SNAP_RADIUS_M: f32 = 250.0;

/// How the router surfaces failure — R4's two-tier UX maps [`NavError::TooFar`] to
/// "Too far to route here" and everything else to "Couldn't find a route."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavError {
    /// `to` is beyond the 10 km crow-flies cap (checked before any graph access).
    TooFar,
    /// No route: an endpoint failed to snap within 250 m, the frontier emptied without
    /// reaching the goal, or a read/write failed mid-flight — every non-distance failure
    /// lands here so the UX stays two-tier.
    NoPath,
    /// The fixed scratch filled before the goal was reached (dense graph / long route).
    Exhausted,
}

/// Nodes the fixed A* scratch tracks (open + closed together) — what ~10 kB buys.
///
/// Per tracked node, one open-addressed [`NavEntry`] plus one binary-heap slot:
///
/// | field       | type  | bytes |                                               |
/// |-------------|-------|-------|-----------------------------------------------|
/// | `node_id`   | `u32` | 4     | hash key (pack-run-dense §8.3 id)             |
/// | `lon`,`lat` | `i32` | 8     | µdeg coord — the settle's quadtree descent    |
/// | `g`         | `u32` | 4     | best known cost from start, m                 |
/// | `h`         | `u32` | 4     | great-circle to goal, m (computed once)       |
/// | `came_from` | `u32` | 4     | predecessor node id                           |
/// | `edge_used` | `u32` | 4     | §8.4 edge taken from the predecessor          |
/// | `heap_pos`  | `u16` | 2     | index into the heap (`u16::MAX` = not queued) |
/// | `flags`     | `u8`  | 1     | occupied / closed                             |
/// | *(pad)*     |       | 1     |                                               |
///
/// 32 B entry + 2 B heap slot = **34 B/node** ⇒ 300 × 34 + 4 B of lengths = 10 204 B,
/// under the 10 kB budget (compile-time asserted below).
pub const NAV_MAX_NODES: usize = 300;

const FLAG_OCCUPIED: u8 = 1;
const FLAG_CLOSED: u8 = 2;
/// `heap_pos` sentinel: not currently queued.
const HEAP_NONE: u16 = u16::MAX;

/// One tracked node — see the layout table at [`NAV_MAX_NODES`]. `repr(C)` pins the
/// 32-byte size the budget math counts.
#[derive(Clone, Copy)]
#[repr(C)]
struct NavEntry {
    node_id: u32,
    lon: i32,
    lat: i32,
    g: u32,
    h: u32,
    came_from: u32,
    edge_used: u32,
    heap_pos: u16,
    flags: u8,
}

impl NavEntry {
    /// All-zero (so a `static NavScratch` lands in `.bss`); `flags == 0` means the slot is free.
    const EMPTY: NavEntry =
        NavEntry { node_id: 0, lon: 0, lat: 0, g: 0, h: 0, came_from: 0, edge_used: 0, heap_pos: 0, flags: 0 };

    /// The A* priority `f = g + h`; saturating so a corrupt cost can't wrap the ordering.
    #[inline]
    fn f(&self) -> u32 {
        self.g.saturating_add(self.h)
    }
}

/// The router's entire mutable state: an open-addressed `node_id → NavEntry` table and
/// a binary min-heap of table indices ordered by `f = g + h` (`heap_pos` back-pointers
/// make decrease-key O(log n), so a node is queued at most once — the heap can never
/// outgrow the table). Caller-owned; the device keeps one in `.bss`
/// ([`NavScratch::new`] is `const` and all-zero). `N` is generic so tests exercise the
/// exhaustion path with a deterministic tiny table; production uses the
/// [`NAV_MAX_NODES`] default.
pub struct NavScratch<const N: usize = NAV_MAX_NODES> {
    entries: [NavEntry; N],
    heap: [u16; N],
    /// Occupied table slots. Insertion fails ([`NavError::Exhausted`]) at `N`, so probe
    /// loops always terminate: below `N` a free slot always exists.
    used: u16,
    heap_len: u16,
}

// The ~10 kB budget, enforced at compile time (locked on #116); and the `u16`
// heap/index encoding must cover every slot with `u16::MAX` left over as the sentinel.
const _: () = assert!(core::mem::size_of::<NavScratch<NAV_MAX_NODES>>() <= 10 * 1024, "NavScratch busts ~10 kB");
const _: () = assert!(NAV_MAX_NODES < u16::MAX as usize, "table indices are u16");

impl<const N: usize> NavScratch<N> {
    pub const fn new() -> Self {
        assert!(N > 0 && N < u16::MAX as usize);
        NavScratch { entries: [NavEntry::EMPTY; N], heap: [0; N], used: 0, heap_len: 0 }
    }

    fn reset(&mut self) {
        for e in self.entries.iter_mut() {
            e.flags = 0;
        }
        self.used = 0;
        self.heap_len = 0;
    }

    /// Linear-probe lookup. Bounded at `N` probes: with no deletions ever, an occupied
    /// run can only end at a free slot — the full-table bound only guards corruption.
    fn lookup(&self, id: u32) -> Option<usize> {
        let mut i = id as usize % N;
        for _ in 0..N {
            let e = &self.entries[i];
            if e.flags & FLAG_OCCUPIED == 0 {
                return None;
            }
            if e.node_id == id {
                return Some(i);
            }
            i = (i + 1) % N;
        }
        None
    }

    /// Insert a fresh entry for `id` (must not be present), un-queued. Scratch full ⇒
    /// [`NavError::Exhausted`] — the locked abort path.
    fn insert(&mut self, id: u32, lon: i32, lat: i32) -> Result<usize, NavError> {
        if self.used as usize == N {
            return Err(NavError::Exhausted);
        }
        let mut i = id as usize % N;
        while self.entries[i].flags & FLAG_OCCUPIED != 0 {
            i = (i + 1) % N;
        }
        self.entries[i] = NavEntry {
            node_id: id,
            lon,
            lat,
            g: 0,
            h: 0,
            came_from: 0,
            edge_used: 0,
            heap_pos: HEAP_NONE,
            flags: FLAG_OCCUPIED,
        };
        self.used += 1;
        Ok(i)
    }

    #[inline]
    fn heap_swap(&mut self, a: usize, b: usize) {
        self.heap.swap(a, b);
        self.entries[self.heap[a] as usize].heap_pos = a as u16;
        self.entries[self.heap[b] as usize].heap_pos = b as u16;
    }

    fn sift_up(&mut self, mut pos: usize) {
        while pos > 0 {
            let parent = (pos - 1) / 2;
            if self.entries[self.heap[pos] as usize].f() >= self.entries[self.heap[parent] as usize].f() {
                break;
            }
            self.heap_swap(pos, parent);
            pos = parent;
        }
    }

    fn sift_down(&mut self, mut pos: usize) {
        let len = self.heap_len as usize;
        loop {
            let (l, r) = (2 * pos + 1, 2 * pos + 2);
            let mut min = pos;
            if l < len && self.entries[self.heap[l] as usize].f() < self.entries[self.heap[min] as usize].f() {
                min = l;
            }
            if r < len && self.entries[self.heap[r] as usize].f() < self.entries[self.heap[min] as usize].f() {
                min = r;
            }
            if min == pos {
                return;
            }
            self.heap_swap(pos, min);
            pos = min;
        }
    }

    /// Queue entry `idx` (must not already be queued). Cannot overflow: one heap slot
    /// exists per table slot and `heap_pos` keeps each entry queued at most once.
    fn heap_push(&mut self, idx: usize) {
        let pos = self.heap_len as usize;
        self.heap[pos] = idx as u16;
        self.entries[idx].heap_pos = pos as u16;
        self.heap_len += 1;
        self.sift_up(pos);
    }

    /// Pop the entry with the smallest `f`, or `None` when the frontier is empty.
    fn heap_pop(&mut self) -> Option<usize> {
        if self.heap_len == 0 {
            return None;
        }
        let idx = self.heap[0] as usize;
        self.entries[idx].heap_pos = HEAP_NONE;
        self.heap_len -= 1;
        if self.heap_len > 0 {
            self.heap[0] = self.heap[self.heap_len as usize];
            self.entries[self.heap[0] as usize].heap_pos = 0;
            self.sift_down(0);
        }
        Some(idx)
    }
}

impl<const N: usize> Default for NavScratch<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// Plan a route over the map's §8 nav graph from `from` (the rider fix) to `to` (the
/// POI coord), both `(lon, lat)` µdeg, and write it as a complete OBCR named `name` to
/// `sink`. Returns the emitted route's [`RouteStats`] (`total_distance_m` = summed
/// edge costs; elevation/ascent all zero — no DEM, locked on #116).
///
/// The caller owns all big state: `scratch` (the fixed A* table, ~10 kB) and `tiles`
/// (the reader's 2-slot graph-tile cache, ~4 kB) — the device keeps both in `.bss`.
/// Both are reset here, so `tiles.stats()` afterwards reads as this route's I/O.
pub fn plan_route<const N: usize>(
    reader: &Reader,
    from: (i32, i32),
    to: (i32, i32),
    name: &str,
    scratch: &mut NavScratch<N>,
    tiles: &mut NavTileCache,
    sink: &mut dyn ByteSink,
) -> Result<RouteStats, NavError> {
    // Cheap crow-flies pre-check — before ANY graph access (locked two-tier UX).
    if ground_dist_m(from, to) > MAX_CROW_FLIES_M {
        return Err(NavError::TooFar);
    }
    scratch.reset();
    tiles.reset();

    let (start_id, start_c) = snap(reader, tiles, from).ok_or(NavError::NoPath)?;
    let (goal_id, goal_c) = snap(reader, tiles, to).ok_or(NavError::NoPath)?;

    // Seed the frontier with the start node (its own predecessor, no edge).
    let si = scratch.insert(start_id, start_c.0, start_c.1)?;
    scratch.entries[si].h = ground_dist_m(start_c, goal_c) as u32;
    scratch.entries[si].came_from = start_id;
    scratch.heap_push(si);

    // Settle loop: pop the best-f node, close it, and relax its record's neighbors.
    // Terminates: a settle closes a node or (re-open) strictly lowers an integer g ≥ 0,
    // and the frontier is bounded by the table.
    while let Some(idx) = scratch.heap_pop() {
        if scratch.entries[idx].node_id == goal_id {
            return emit_route(reader, scratch, tiles, name, idx, start_id, sink);
        }
        scratch.entries[idx].flags |= FLAG_CLOSED;
        settle::<N>(reader, scratch, tiles, idx, goal_c)?;
    }
    // Frontier emptied without reaching the goal — disconnected (or the graph is empty).
    Err(NavError::NoPath)
}

/// One settle: descend the node quadtree to the settled node's leaf (a degenerate
/// one-point view — the spatial re-fetch) and relax each of its §8.3 neighbors from
/// the inline `(coord, cost_m)`. A node the walk doesn't yield (corrupt map) simply
/// relaxes nothing; the search continues on whatever frontier remains.
fn settle<const N: usize>(
    reader: &Reader,
    scratch: &mut NavScratch<N>,
    tiles: &mut NavTileCache,
    idx: usize,
    goal_c: (i32, i32),
) -> Result<(), NavError> {
    let settled = scratch.entries[idx];
    let view = BBox { min_lon: settled.lon, min_lat: settled.lat, max_lon: settled.lon, max_lat: settled.lat };
    // The walk callback can't return early, so exhaustion is latched and re-raised after.
    let mut full = false;
    reader
        .for_each_nav_node_cached(&view, tiles, |n| {
            if n.id != settled.node_id || full {
                return;
            }
            for nb in n.neighbors() {
                let tentative = settled.g.saturating_add(nb.cost_m);
                match scratch.lookup(nb.id) {
                    Some(j) => {
                        if tentative < scratch.entries[j].g {
                            let e = &mut scratch.entries[j];
                            e.g = tentative;
                            e.came_from = settled.node_id;
                            e.edge_used = nb.edge_id;
                            if e.heap_pos == HEAP_NONE {
                                // Re-open a closed node: `h` mixes cos_lat bands across the
                                // route, so tiny inconsistencies are possible — correctness
                                // over assuming perfect consistency.
                                e.flags &= !FLAG_CLOSED;
                                scratch.heap_push(j);
                            } else {
                                let pos = scratch.entries[j].heap_pos as usize;
                                scratch.sift_up(pos);
                            }
                        }
                    }
                    None => match scratch.insert(nb.id, nb.lon, nb.lat) {
                        Ok(j) => {
                            let e = &mut scratch.entries[j];
                            e.g = tentative;
                            e.h = ground_dist_m((nb.lon, nb.lat), goal_c) as u32;
                            e.came_from = settled.node_id;
                            e.edge_used = nb.edge_id;
                            scratch.heap_push(j);
                        }
                        Err(_) => full = true,
                    },
                }
            }
        })
        .map_err(|_| NavError::NoPath)?;
    if full {
        return Err(NavError::Exhausted);
    }
    Ok(())
}

/// Snap `p` to the nearest routable node within [`SNAP_RADIUS_M`] — the POI query's
/// expanding-ring walk shape over the node quadtree: start with a small square view,
/// double it until the best find is provably nearest (its distance ≤ the ring's
/// half-extent — everything outside the square is at least that far), capping at the
/// snap radius. The cap makes the final ring exhaustive by construction (a 250 m
/// half-extent square contains the whole 250 m disc), so unlike the POI query no
/// map-cover fallback is needed. Repeat visits across rings re-hit the tile cache.
fn snap(reader: &Reader, tiles: &mut NavTileCache, p: (i32, i32)) -> Option<(u32, (i32, i32))> {
    if reader.nav_directory().is_empty() {
        return None;
    }
    // Guard a degenerate cos_lat (poles / corrupt latitude) like the POI query.
    let cl = cos_lat(p.1).max(1e-3);
    // 250 m as µdeg of latitude (~2 246); the opening ring is a quarter of it.
    let full_half = (SNAP_RADIUS_M / M_PER_DEG as f32 * 1e6) as i32;
    let mut half = (full_half / 4).max(1);
    let mut best: Option<(u32, (i32, i32), f32)> = None;
    loop {
        let lon_half = ((half as f32 / cl) as i32).max(1);
        let view = BBox {
            min_lon: p.0.saturating_sub(lon_half),
            min_lat: p.1.saturating_sub(half),
            max_lon: p.0.saturating_add(lon_half),
            max_lat: p.1.saturating_add(half),
        };
        reader
            .for_each_nav_node_cached(&view, tiles, |n| {
                let d = ground_dist_m_cl(p, (n.lon, n.lat), cl);
                if d <= SNAP_RADIUS_M && best.is_none_or(|(_, _, bd)| d < bd) {
                    best = Some((n.id, (n.lon, n.lat), d));
                }
            })
            .ok()?;
        let half_m = half as f32 * (M_PER_DEG as f32) * 1e-6;
        if let Some((id, c, d)) = best {
            if d <= half_m {
                return Some((id, c));
            }
        }
        if half >= full_half {
            // Final ring covered the whole disc — whatever we found is the answer.
            return best.map(|(id, c, _)| (id, c));
        }
        half = (half * 2).min(full_half);
    }
}

/// Walk `came_from` goal→start, then stream the path's edge geometry start→goal into
/// the shared OBCR emitter. The chain is staged in the (now dead) heap array — path
/// length is bounded by the tracked-node count, so it always fits; no extra buffer.
/// Each hop's polyline is fetched oriented via [`Reader::nav_edge_oriented`] and the
/// shared seam vertex deduped, so the OBCR carries one continuous polyline. Elevation
/// is zero throughout (no DEM); the header total is the goal's `g` — the summed edge
/// costs (locked on #116).
fn emit_route<const N: usize>(
    reader: &Reader,
    scratch: &mut NavScratch<N>,
    tiles: &mut NavTileCache,
    name: &str,
    goal_idx: usize,
    start_id: u32,
    sink: &mut dyn ByteSink,
) -> Result<RouteStats, NavError> {
    let total_m = scratch.entries[goal_idx].g;

    // Stage the chain goal→start in the heap array (A* is done with it). Bounded by
    // `used`; a longer walk means a corrupt came_from cycle — fail, don't spin.
    let mut chain_len = 0usize;
    let mut cur = goal_idx;
    loop {
        if chain_len >= scratch.used as usize {
            return Err(NavError::NoPath);
        }
        scratch.heap[chain_len] = cur as u16;
        chain_len += 1;
        if scratch.entries[cur].node_id == start_id {
            break;
        }
        cur = scratch.lookup(scratch.entries[cur].came_from).ok_or(NavError::NoPath)?;
    }

    // Stream start→goal (the chain reversed). I/O failures degrade to the generic
    // "couldn't find a route" tier — the UX is two-tier by design.
    let mut em = ObcrEmitter::new(sink).map_err(|_| NavError::NoPath)?;
    if chain_len == 1 {
        // Both endpoints snapped to the same node: a single-point route of length 0.
        let e = &scratch.entries[goal_idx];
        em.push(sink, e.lon, e.lat, 0, 0).map_err(|_| NavError::NoPath)?;
    }
    let mut last: Option<(i32, i32)> = None;
    for hop in (1..chain_len).rev() {
        let prev = &scratch.entries[scratch.heap[hop] as usize];
        let cur = &scratch.entries[scratch.heap[hop - 1] as usize];
        let mut werr = false;
        reader
            .nav_edge_oriented(tiles, cur.edge_used, (prev.lon, prev.lat), |pt| {
                if werr || last == Some(pt) {
                    return; // seam vertex already emitted by the previous hop
                }
                if em.push(sink, pt.0, pt.1, 0, 0).is_err() {
                    werr = true;
                    return;
                }
                last = Some(pt);
            })
            .ok_or(NavError::NoPath)?;
        if werr {
            return Err(NavError::NoPath);
        }
    }

    let stats = EmitStats { min_ele_m: 0, max_ele_m: 0, ascent_m: 0, descent_m: 0, total_distance_m: Some(total_m) };
    em.finish(sink, name, stats, &mut Vec::<WpPlace, MAX_WAYPOINTS>::new()).map_err(|_| NavError::NoPath)
}
