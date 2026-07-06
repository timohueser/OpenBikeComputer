//! On-device point-to-point routing over the OBCM §8 nav graph (epic #116, R3/R4).
//!
//! [`plan_route`] runs **weighted A\*** from the rider's fix to a POI and writes the
//! result as a complete OBCR through the shared [`ObcrEmitter`], so the caller (R4)
//! saves it to `/routes/_nav.obcr` and the rest of the device can't tell it from a
//! loaded GPX route. `no_std`, identical on device and sim; every buffer lives in
//! caller-owned structs (`NavScratch` + the reader's `NavTileCache`) because this
//! runs under the nRF's tight stack next to the render peak — a fat local here is a
//! HardFault on-glass (#419/#270).
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
//! **The scratch is fixed** ([`NAV_MAX_NODES`] tracked nodes, per-target sized)
//! because the router must coexist with the map cache and the render scratch in RAM;
//! on a dense graph it fills and the search aborts with [`NavError::Exhausted`].
//!
//! **Bounded suboptimality, not exactness** (decided 2026-07-06 — range in fixed
//! memory): the priority is `f = g + ε·h` with ε = [`NAV_EPSILON_NUM`] /
//! [`NAV_EPSILON_DEN`] = 1.3. The heuristic itself never overestimates — it is the
//! great-circle distance to the goal in the *same* local-equirectangular metric the
//! packer summed for every edge's `cost_m` — so weighted A\* returns a path of length
//! **≤ 1.3× the true shortest** (in practice a few percent on road networks). What ε
//! buys is *reach*: plain A\* explores roughly quadratically with distance and
//! exhausted the fixed table ~1.5 km out; the goal-greedy inflated search settles a
//! narrow corridor instead, planning multi-km routes in the same table.
//!
//! **The caller drives liveness** through the `progress` hook: [`plan_route`] is a
//! long synchronous computation (seconds on the SD-bound device), and the hook is
//! called every [`PROGRESS_EVERY_SETTLES`] settles so the board can feed its
//! watchdog (and a host can abort). Between calls the executor is still starved —
//! accepted for v1.

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

/// Weighted-A\* heuristic inflation ε as an integer ratio: `f = g + (NUM·h)/DEN`
/// (= 1.3). Applied on **all** targets — the found path is at most ε× the true
/// shortest (see the module doc's bounded-suboptimality note, decided 2026-07-06).
pub const NAV_EPSILON_NUM: u32 = 13;
/// ε's denominator — see [`NAV_EPSILON_NUM`].
pub const NAV_EPSILON_DEN: u32 = 10;

/// How many settles between two `progress` callbacks — frequent enough that the
/// board's watchdog is fed every few hundred ms of SD-bound settling, rare enough
/// to stay measurement noise.
pub const PROGRESS_EVERY_SETTLES: u32 = 8;

/// How the router surfaces failure — R4's two-tier UX maps [`NavError::TooFar`] to
/// "Too far to route here" and everything else to "Couldn't find a route."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavError {
    /// `to` is beyond the 10 km crow-flies cap (checked before any graph access).
    TooFar,
    /// No route: an endpoint failed to snap within 250 m, the frontier emptied without
    /// reaching the goal, a read/write failed mid-flight, or the host's `progress`
    /// hook aborted the search — every non-distance failure lands here so the UX
    /// stays two-tier.
    NoPath,
    /// The fixed scratch filled before the goal was reached (dense graph / long route).
    Exhausted,
}

/// Nodes the fixed A\* scratch tracks (open + closed together) — per-target sized.
///
/// Per tracked node, one open-addressed [`NavEntry`] plus one binary-heap slot:
///
/// | field       | type  | bytes |                                                    |
/// |-------------|-------|-------|----------------------------------------------------|
/// | `node_id`   | `u32` | 4     | hash key (pack-run-dense §8.3 id)                  |
/// | `lon`,`lat` | `i32` | 8     | µdeg coord — the settle's quadtree descent         |
/// | `edge_used` | `u32` | 4     | §8.4 edge taken from the predecessor               |
/// | `g`         | `u16` | 2     | best known cost from start, m (saturating)         |
/// | `h`         | `u16` | 2     | great-circle to goal, m (computed once, saturating)|
/// | `came_from` | `u16` | 2     | predecessor's **table slot index**                 |
/// | `meta`      | `u16` | 2     | bit15 occupied · bit14 closed · bits 0..14 heap_pos|
///
/// 24 B entry + 2 B heap slot = **26 B/node** (compile-time asserted below; was
/// 34 B/node with `u32` costs and id-keyed `came_from` before the 2026-07-06 range
/// fix). `g`/`h` in `u16` meters saturate at 65 535 m — far past the ~13 km any
/// meaningful path can reach under the 10 km crow-flies cap × ε — and a saturated
/// `g` only makes its node maximally unattractive (never mis-ordered, never wrapped).
/// `came_from` as a slot index (slots never move — open addressing, no deletion)
/// both saves 2 B and turns the emit chain-walk into direct indexing.
///
/// Per-target `N` (the const must stay trivial to bump):
/// - **host/sim** (`not(nrf-mem)`): 8192 nodes ≈ 208 KB — host RAM is free; the sim
///   heap-allocates it (a stack local would trap the wasm build). Sized so a 10 km
///   plan in dense urban graphs succeeds (the locked sim requirement).
/// - **device** (`nrf-mem`): 768 nodes ≈ 20 KB of `.bss`. Budget math (2026-07-06,
///   DK debug-uart build): stack region 69 736 B, pre-nav render peak 35 808 B; the
///   flattened plan frame's excursion is assumed ≤ render peak + ~8 KB ≈ 44 KB;
///   keeping ≥ 6 KB of margin leaves ~20 KB for this table (the ~4 KB tile cache
///   rides the same nav budget), i.e. ~800 slimmed nodes — chosen conservatively at
///   768; the coordinator re-measures on-glass and bumps.
#[cfg(not(feature = "nrf-mem"))]
pub const NAV_MAX_NODES: usize = 8192;
#[cfg(feature = "nrf-mem")]
pub const NAV_MAX_NODES: usize = 768;

/// `meta` bit 15: the slot is occupied (the open-addressing "live" marker).
const META_OCCUPIED: u16 = 1 << 15;
/// `meta` bit 14: the node is closed (settled; may re-open on a shorter `g`).
const META_CLOSED: u16 = 1 << 14;
/// `meta` bits 0..14: the heap position, [`HEAP_NONE`] = not queued.
const META_POS_MASK: u16 = 0x3FFF;
/// `heap_pos` sentinel within [`META_POS_MASK`]: not currently queued.
const HEAP_NONE: u16 = 0x3FFF;

/// One tracked node — see the layout table at [`NAV_MAX_NODES`]. `repr(C)` pins the
/// 24-byte size the budget math counts.
#[derive(Clone, Copy)]
#[repr(C)]
struct NavEntry {
    node_id: u32,
    lon: i32,
    lat: i32,
    edge_used: u32,
    g: u16,
    h: u16,
    /// Predecessor's **table slot index** (not a node id) — slots never move.
    came_from: u16,
    /// Packed occupied/closed flags + heap position (see the `META_*` consts).
    meta: u16,
}

impl NavEntry {
    /// All-zero (so a `static NavScratch` lands in `.bss`); `meta == 0` has the
    /// occupied bit clear, so a zeroed slot reads as free.
    const EMPTY: NavEntry = NavEntry { node_id: 0, lon: 0, lat: 0, edge_used: 0, g: 0, h: 0, came_from: 0, meta: 0 };

    /// The weighted-A\* priority `f = g + ε·h` in `u32` (max ≈ 65 535 + 1.3×65 535,
    /// nowhere near wrapping; saturating for form).
    #[inline]
    fn f(&self) -> u32 {
        (self.g as u32).saturating_add(NAV_EPSILON_NUM * self.h as u32 / NAV_EPSILON_DEN)
    }

    #[inline]
    fn occupied(&self) -> bool {
        self.meta & META_OCCUPIED != 0
    }

    #[inline]
    fn heap_pos(&self) -> u16 {
        self.meta & META_POS_MASK
    }

    #[inline]
    fn set_heap_pos(&mut self, pos: u16) {
        self.meta = (self.meta & !META_POS_MASK) | (pos & META_POS_MASK);
    }
}

/// Saturate a `u32` meter figure into the entry's `u16` cost field (see the layout
/// note at [`NAV_MAX_NODES`]: saturation only ever *overestimates*, pruning absurd
/// nodes — it can never wrap the ordering).
#[inline]
fn sat16(m: u32) -> u16 {
    m.min(u16::MAX as u32) as u16
}

/// The router's entire mutable state: an open-addressed `node_id → NavEntry` table and
/// a binary min-heap of table indices ordered by `f = g + ε·h` (heap-position
/// back-pointers make decrease-key O(log n), so a node is queued at most once — the
/// heap can never outgrow the table). Caller-owned; the device keeps one in `.bss`
/// ([`NavScratch::new`] is `const` and all-zero — an all-zero struct **is** `new()`,
/// which is what lets the sim heap-allocate it zeroed). `N` is generic so tests
/// exercise the exhaustion path with a deterministic tiny table; production uses the
/// per-target [`NAV_MAX_NODES`] default.
pub struct NavScratch<const N: usize = NAV_MAX_NODES> {
    entries: [NavEntry; N],
    heap: [u16; N],
    /// Occupied table slots. Insertion fails ([`NavError::Exhausted`]) at `N`, so probe
    /// loops always terminate: below `N` a free slot always exists.
    used: u16,
    heap_len: u16,
}

// Per-target table budget, enforced at compile time: the device table must stay a
// ~20 KB `.bss` static (the R4 budget math above); slot indices — heap positions and
// `came_from` — are 14-bit, with `HEAP_NONE` left over as the sentinel.
#[cfg(feature = "nrf-mem")]
const _: () = assert!(core::mem::size_of::<NavScratch<NAV_MAX_NODES>>() <= 20 * 1024, "NavScratch busts ~20 kB");
const _: () = assert!(NAV_MAX_NODES < HEAP_NONE as usize, "table indices are 14-bit (meta packs flags above them)");
const _: () = assert!(core::mem::size_of::<NavEntry>() == 24, "the slimmed 24-byte entry layout drifted");

impl<const N: usize> NavScratch<N> {
    pub const fn new() -> Self {
        assert!(N > 0 && N < HEAP_NONE as usize);
        NavScratch { entries: [NavEntry::EMPTY; N], heap: [0; N], used: 0, heap_len: 0 }
    }

    fn reset(&mut self) {
        for e in self.entries.iter_mut() {
            e.meta = 0;
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
            if !e.occupied() {
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
        while self.entries[i].occupied() {
            i = (i + 1) % N;
        }
        self.entries[i] =
            NavEntry { node_id: id, lon, lat, edge_used: 0, g: 0, h: 0, came_from: 0, meta: META_OCCUPIED | HEAP_NONE };
        self.used += 1;
        Ok(i)
    }

    #[inline]
    fn heap_swap(&mut self, a: usize, b: usize) {
        self.heap.swap(a, b);
        self.entries[self.heap[a] as usize].set_heap_pos(a as u16);
        self.entries[self.heap[b] as usize].set_heap_pos(b as u16);
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
    /// exists per table slot and the heap position keeps each entry queued at most once.
    fn heap_push(&mut self, idx: usize) {
        let pos = self.heap_len as usize;
        self.heap[pos] = idx as u16;
        self.entries[idx].set_heap_pos(pos as u16);
        self.heap_len += 1;
        self.sift_up(pos);
    }

    /// Pop the entry with the smallest `f`, or `None` when the frontier is empty.
    fn heap_pop(&mut self) -> Option<usize> {
        if self.heap_len == 0 {
            return None;
        }
        let idx = self.heap[0] as usize;
        self.entries[idx].set_heap_pos(HEAP_NONE);
        self.heap_len -= 1;
        if self.heap_len > 0 {
            self.heap[0] = self.heap[self.heap_len as usize];
            self.entries[self.heap[0] as usize].set_heap_pos(0);
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
/// edge costs, saturated at 65 535 m; elevation/ascent all zero — no DEM, locked on
/// #116). The found path is at most ε = 1.3× the true shortest (see the module doc).
///
/// The caller owns all big state: `scratch` (the fixed A\* table) and `tiles`
/// (the reader's 2-slot graph-tile cache, ~4 kB) — the device keeps both in `.bss`,
/// the sim heap-allocates the big host table. Both are reset here, so `tiles.stats()`
/// afterwards reads as this route's I/O.
///
/// `progress` is called every [`PROGRESS_EVERY_SETTLES`] settles; returning `false`
/// aborts the search cleanly as [`NavError::NoPath`] (the generic failure tier). The
/// board feeds its watchdog here — the plan is a long synchronous computation and
/// still starves the executor *between* callbacks (accepted for v1).
#[allow(clippy::too_many_arguments)] // the seam deliberately takes every caller-owned buffer
pub fn plan_route<const N: usize>(
    reader: &Reader,
    from: (i32, i32),
    to: (i32, i32),
    name: &str,
    scratch: &mut NavScratch<N>,
    tiles: &mut NavTileCache,
    progress: &mut dyn FnMut() -> bool,
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
    scratch.entries[si].h = sat16(ground_dist_m(start_c, goal_c) as u32);
    scratch.entries[si].came_from = si as u16;
    scratch.heap_push(si);

    // Settle loop: pop the best-f node, close it, and relax its record's neighbors.
    // Terminates: a settle closes a node or (re-open) strictly lowers an integer g ≥ 0,
    // and the frontier is bounded by the table.
    let mut settles: u32 = 0;
    while let Some(idx) = scratch.heap_pop() {
        if scratch.entries[idx].node_id == goal_id {
            return emit_route(reader, scratch, tiles, name, idx, start_id, sink);
        }
        settles = settles.wrapping_add(1);
        if settles.is_multiple_of(PROGRESS_EVERY_SETTLES) && !progress() {
            return Err(NavError::NoPath); // host-aborted — the generic failure tier
        }
        scratch.entries[idx].meta |= META_CLOSED;
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
                // u16-saturating tentative cost: a saturated g is just maximally
                // unattractive (see the layout note) — never wrapped, never mis-ordered.
                let tentative = sat16((settled.g as u32).saturating_add(nb.cost_m));
                match scratch.lookup(nb.id) {
                    Some(j) => {
                        if tentative < scratch.entries[j].g {
                            let e = &mut scratch.entries[j];
                            e.g = tentative;
                            e.came_from = idx as u16;
                            e.edge_used = nb.edge_id;
                            if e.heap_pos() == HEAP_NONE {
                                // Re-open a closed node: the inflated `h` (and its cos_lat
                                // banding) makes better-g rediscoveries routine — correctness
                                // over assuming consistency; the ε bound still holds.
                                e.meta &= !META_CLOSED;
                                scratch.heap_push(j);
                            } else {
                                let pos = scratch.entries[j].heap_pos() as usize;
                                scratch.sift_up(pos);
                            }
                        }
                    }
                    None => match scratch.insert(nb.id, nb.lon, nb.lat) {
                        Ok(j) => {
                            let e = &mut scratch.entries[j];
                            e.g = tentative;
                            e.h = sat16(ground_dist_m((nb.lon, nb.lat), goal_c) as u32);
                            e.came_from = idx as u16;
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
/// the shared OBCR emitter. `came_from` holds **slot indices**, so the walk is direct
/// indexing (no lookup); the chain is staged in the (now dead) heap array — path
/// length is bounded by the tracked-node count, so it always fits; no extra buffer.
/// Each hop's polyline is fetched oriented via [`Reader::nav_edge_oriented`] and the
/// shared seam vertex deduped, so the OBCR carries one continuous polyline. Elevation
/// is zero throughout (no DEM); the header total is the goal's `g` — the summed edge
/// costs (locked on #116), saturated at 65 535 m like every stored cost.
fn emit_route<const N: usize>(
    reader: &Reader,
    scratch: &mut NavScratch<N>,
    tiles: &mut NavTileCache,
    name: &str,
    goal_idx: usize,
    start_id: u32,
    sink: &mut dyn ByteSink,
) -> Result<RouteStats, NavError> {
    let total_m = scratch.entries[goal_idx].g as u32;

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
        cur = scratch.entries[cur].came_from as usize;
        if cur >= N {
            return Err(NavError::NoPath); // corrupt slot index — fail, don't index OOB
        }
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
