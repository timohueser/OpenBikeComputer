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
//! **The planner is resumable** (#499): planning is a [`NavPlanner`] step machine —
//! snap → search ([`NAV_SETTLES_PER_STEP`] settles per step) → emit
//! ([`NAV_EMIT_HOPS_PER_STEP`] edge fetches per step) → done — and the host runs
//! **one bounded [`step`](NavPlanner::step) per loop pass**, so render/input/watchdog
//! all run normally *between* steps and a multi-second plan no longer freezes the UI
//! (the old sync `plan_route` starved the executor for the whole search). The
//! one-shot [`plan_route`] convenience just loops `step` — hosts that don't need
//! interactivity (tests, the headless sim) use it unchanged. Cancelling is simply
//! **not calling `step` again**: nothing is written to the sink before the emit
//! phase, so an abandoned search leaves the sink untouched and the caller only has
//! to discard its own file.
//!
//! **UB tripwire**: this module and the reader's record decode are deliberately
//! **cast-free** — every §8 record field is assembled byte-wise (`from_le_bytes` on
//! `&[u8]`), because records sit at odd offsets by design and any typed view over
//! them is instant alignment UB on ARM (PR #501's on-glass HardFault; the board
//! build also compiles `+strict-align`, see its `.cargo/config.toml`). The standing
//! host-side check is **Miri** over this suite:
//! `cargo +nightly miri test -p obc-route --test nav` — green as of 2026-07-06;
//! run it when touching the planner or the record decode.
//!
//! **No distance cap** (Timo, post-#496): the rider may try to route to *anything*;
//! the fixed table is the real limit and the attempt is the feedback —
//! [`NavError::Exhausted`] **is** the "too far for this device" answer. Two accepted
//! consequences: (1) `h` (`u16` meters, saturating) saturates for very distant goals,
//! so ordering degrades gracefully toward uniform expansion and the search exhausts —
//! exactly the intended outcome; (2) a hopeless target burns a **full exhaustion
//! search** before failing — bounded by the table, and spread across bounded steps,
//! so the stepping host's loop keeps feeding its watchdog (and rendering, and taking
//! input — including the cancel) between them; a step-count budget in the stepping
//! host is the named future lever if the wait annoys.

use heapless::Vec;

use crate::byte_io::ByteSink;
use crate::convert::{EmitStats, ObcrEmitter, RouteStats, WpPlace, MAX_WAYPOINTS};
use crate::geo::{cos_lat, ground_dist_m, ground_dist_m_cl};
use crate::reader::NAME_CAP;
use obc_reader::{BBox, NavTileCache, Reader, M_PER_DEG};

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

/// Search-phase step budget: settles per [`NavPlanner::step`] — small enough that a
/// step returns within a few SD chunk reads (the host's pass stays responsive and
/// the watchdog is fed between steps), large enough that per-step overhead stays
/// measurement noise. (Succeeds the removed per-`PROGRESS_EVERY_SETTLES` callback —
/// the step boundary *is* the liveness point now.)
pub const NAV_SETTLES_PER_STEP: u32 = 8;

/// Emit-phase step budget: path hops (edge-geometry fetches + OBCR pushes) per
/// [`NavPlanner::step`] — the emit is short next to the search, so a few hops per
/// step finishes it in a handful of passes without one long blocking tail.
pub const NAV_EMIT_HOPS_PER_STEP: u16 = 4;

/// How the router surfaces failure — the two-tier UX maps [`NavError::Exhausted`] to
/// "Too far to route here" (with no distance cap, running out of table **is** the
/// device's range limit) and [`NavError::NoPath`] to "Couldn't find a route."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavError {
    /// No route: an endpoint failed to snap within 250 m, the frontier emptied without
    /// reaching the goal, or a read/write failed mid-flight — every non-range failure
    /// lands here so the UX stays two-tier. (A rider cancel never produces an error at
    /// all: the host just stops stepping.)
    NoPath,
    /// The fixed scratch filled before the goal was reached — the device's honest
    /// "too far" (dense graph, long route, or an unreachable/hopeless target).
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
/// fix). `g`/`h` in `u16` meters saturate at 65 535 m — far past anything the fixed
/// table can span — and a saturated cost only makes its node maximally unattractive
/// (never mis-ordered, never wrapped); with no distance cap a very distant goal just
/// degrades the ordering toward uniform expansion until the table exhausts (see the
/// module doc's no-cap note).
/// `came_from` as a slot index (slots never move — open addressing, no deletion)
/// both saves 2 B and turns the emit chain-walk into direct indexing.
///
/// Per-target `N` (the const must stay trivial to bump):
/// - **host/sim** (`not(nrf-mem)`): 1536 nodes × 26 B = 39 936 B — deliberately
///   **emulating the final device's (LM20) 40 kB nav-budget cap** (Timo, 2026-07-06)
///   rather than using free host RAM, so the sim's plannable range **is** the final
///   device's range by construction. (The LM20's map gets RAM priority — real maps
///   are far bigger than the fixtures — with 60 kB the absolute nav ceiling only if
///   the map turns out not to need the space.) The sim still heap-allocates the
///   table (a stack local would trap the wasm build).
/// - **device (DK)** (`nrf-mem`): 768 nodes ≈ 20 KB of `.bss`. Budget math
///   (2026-07-06, DK debug-uart build): stack region 69 736 B, pre-nav render peak
///   35 808 B; the flattened plan frame's excursion is assumed ≤ render peak +
///   ~8 KB ≈ 44 KB; keeping ≥ 6 KB of margin leaves ~20 KB for this table (the ~4 KB
///   tile cache rides the same nav budget), i.e. ~800 slimmed nodes — chosen
///   conservatively at 768; the coordinator re-measures on-glass and bumps.
#[cfg(not(feature = "nrf-mem"))]
pub const NAV_MAX_NODES: usize = 1536;
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
#[cfg(not(feature = "nrf-mem"))]
const _: () =
    assert!(core::mem::size_of::<NavScratch<NAV_MAX_NODES>>() <= 40 * 1024, "NavScratch busts the LM20 40 kB cap");
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

/// One [`NavPlanner::step`] outcome: keep stepping, or the plan's terminal result.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    /// More work remains — call [`step`](NavPlanner::step) again (typically next host pass).
    Running,
    /// The route is fully emitted and the OBCR header patched; the plan's [`RouteStats`].
    Done(RouteStats),
    /// The plan failed; nothing useful is in the sink past the reserved header (and nothing at
    /// all if the failure predates the emit phase). The caller discards its file.
    Failed(NavError),
}

/// The planner's coarse phase — what the *next* [`step`](NavPlanner::step) will spend its budget
/// on. The board's per-phase RTT instrumentation attributes each step's wall time to the phase
/// read **before** the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavPhase {
    /// Snapping an endpoint to the graph (one endpoint per step; a bounded ring walk each).
    Snap,
    /// The weighted-A\* search, [`NAV_SETTLES_PER_STEP`] settles per step.
    Search,
    /// Streaming the found path's edge geometry into the OBCR ([`NAV_EMIT_HOPS_PER_STEP`] hops
    /// per step), then the finishing header patch.
    Emit,
    /// Terminal — [`step`](NavPlanner::step) idempotently re-returns the outcome.
    Done,
}

/// The fine-grained internal phase; [`NavPhase`] is its public projection.
enum PhaseState {
    SnapFrom,
    SnapTo,
    Search,
    Emit,
    Finish,
    Terminal(Result<RouteStats, NavError>),
}

/// The **resumable route planner** (#499): plans from `from` (the rider fix) to `to` (the POI
/// coord), both `(lon, lat)` µdeg, writing a complete OBCR named `name` to the step's `sink` —
/// one bounded unit of work per [`step`](NavPlanner::step) so the host's loop (render, input,
/// watchdog) keeps running between steps.
///
/// All *search* state lives in the caller-owned [`NavScratch`]/[`NavTileCache`] exactly as
/// before; what the planner itself holds is the phase + cursors **and the [`ObcrEmitter`]**,
/// which must survive across emit steps. The emitter is ~9 kB by value (a chunk-index table + a
/// point staging buffer), which is why a `NavPlanner` is a **caller-owned object** — a `.bss`
/// slot on the device, a heap box in the sim — and not a stack local. The zero-sum accounting
/// (stack ↔ `.bss`): the old one-shot carried the emitter in its deepest frame, so the measured
/// plan stack high-water shrinks by roughly the same ~9 kB the planner adds to `.bss` (the
/// emitter is only *constructed* on entering the emit phase, so a step's transient stack cost is
/// one emitter-sized move at the shallow step frame).
///
/// The plan's contract with the sink: **nothing is written before the emit phase** (snap +
/// search are read-only), so cancelling — dropping the planner, or just never stepping again —
/// during the search leaves the sink pristine; a cancel mid-emit leaves a headerless torn
/// prefix the caller deletes. `tiles.stats()` after the last step reads as the whole plan's I/O
/// (reset in the first step, like the scratch).
pub struct NavPlanner {
    phase: PhaseState,
    from: (i32, i32),
    to: (i32, i32),
    /// The route's name, applied by the finishing header patch.
    name: heapless::String<NAME_CAP>,
    /// The snapped endpoints (valid once their phase has run).
    start_id: u32,
    start_c: (i32, i32),
    goal_id: u32,
    goal_c: (i32, i32),
    /// Total settles so far — the RTT line's `settles=` figure, and the budget tests' probe.
    settles: u32,
    /// Emit cursors: the staged chain length, the next hop to emit (descending to 1), the summed
    /// path cost, and the cross-hop seam-dedup vertex.
    chain_len: u16,
    hop: u16,
    total_m: u32,
    last: Option<(i32, i32)>,
    /// The OBCR emitter — created on entering the emit phase (its constructor writes the
    /// reserved header), consumed by the finish. The planner's one big field (~9 kB).
    em: Option<ObcrEmitter>,
}

impl NavPlanner {
    /// A planner for one route request. Touches nothing yet — the first [`step`](NavPlanner::step)
    /// resets the caller's scratch + tile cache and starts snapping.
    pub fn new(from: (i32, i32), to: (i32, i32), name: &str) -> Self {
        let mut nm = heapless::String::new();
        for ch in name.chars() {
            if nm.push(ch).is_err() {
                break;
            }
        }
        NavPlanner {
            phase: PhaseState::SnapFrom,
            from,
            to,
            name: nm,
            start_id: 0,
            start_c: (0, 0),
            goal_id: 0,
            goal_c: (0, 0),
            settles: 0,
            chain_len: 0,
            hop: 0,
            total_m: 0,
            last: None,
            em: None,
        }
    }

    /// The public phase the **next** step will work on — the board's per-phase timing key.
    pub fn phase(&self) -> NavPhase {
        match &self.phase {
            PhaseState::SnapFrom | PhaseState::SnapTo => NavPhase::Snap,
            PhaseState::Search => NavPhase::Search,
            PhaseState::Emit | PhaseState::Finish => NavPhase::Emit,
            PhaseState::Terminal(_) => NavPhase::Done,
        }
    }

    /// Total nodes settled so far — the RTT line's `settles=` figure.
    pub fn settles(&self) -> u32 {
        self.settles
    }

    /// Terminal-transition helper: latch and return the failure.
    fn fail(&mut self, e: NavError) -> Step {
        self.phase = PhaseState::Terminal(Err(e));
        Step::Failed(e)
    }

    /// Run **one bounded unit** of planning: one endpoint snap, [`NAV_SETTLES_PER_STEP`]
    /// settles, [`NAV_EMIT_HOPS_PER_STEP`] emit hops, or the finishing header patch — then
    /// return. `reader`/`scratch`/`tiles`/`sink` are the caller's per-pass views over the same
    /// underlying state every step (on the board: a fresh `Reader` borrow + a sink over the same
    /// open file each pass). Terminal outcomes are idempotent — further steps re-return them.
    pub fn step<const N: usize>(
        &mut self,
        reader: &Reader,
        scratch: &mut NavScratch<N>,
        tiles: &mut NavTileCache,
        sink: &mut dyn ByteSink,
    ) -> Step {
        match self.phase {
            PhaseState::SnapFrom => {
                // First step of the plan: claim the caller's buffers.
                scratch.reset();
                tiles.reset();
                let Some((id, c)) = snap(reader, tiles, self.from) else {
                    return self.fail(NavError::NoPath);
                };
                self.start_id = id;
                self.start_c = c;
                self.phase = PhaseState::SnapTo;
                Step::Running
            }
            PhaseState::SnapTo => {
                let Some((id, c)) = snap(reader, tiles, self.to) else {
                    return self.fail(NavError::NoPath);
                };
                self.goal_id = id;
                self.goal_c = c;
                // Seed the frontier with the start node (its own predecessor, no edge).
                let si = match scratch.insert(self.start_id, self.start_c.0, self.start_c.1) {
                    Ok(si) => si,
                    Err(e) => return self.fail(e),
                };
                scratch.entries[si].h = sat16(ground_dist_m(self.start_c, self.goal_c) as u32);
                scratch.entries[si].came_from = si as u16;
                scratch.heap_push(si);
                self.phase = PhaseState::Search;
                Step::Running
            }
            // Settle up to the step budget: pop the best-f node, close it, relax its record's
            // neighbors. Terminates: a settle closes a node or (re-open) strictly lowers an
            // integer g ≥ 0, and the frontier is bounded by the table.
            PhaseState::Search => {
                for _ in 0..NAV_SETTLES_PER_STEP {
                    let Some(idx) = scratch.heap_pop() else {
                        // Frontier emptied without the goal — disconnected (or an empty graph).
                        return self.fail(NavError::NoPath);
                    };
                    if scratch.entries[idx].node_id == self.goal_id {
                        return match self.stage_chain(scratch, idx) {
                            Ok(()) => {
                                self.phase = PhaseState::Emit;
                                Step::Running
                            }
                            Err(e) => self.fail(e),
                        };
                    }
                    self.settles = self.settles.wrapping_add(1);
                    scratch.entries[idx].meta |= META_CLOSED;
                    if let Err(e) = settle::<N>(reader, scratch, tiles, idx, self.goal_c) {
                        return self.fail(e);
                    }
                }
                Step::Running
            }
            // Stream up to the hop budget of edge geometry start→goal. I/O failures degrade to
            // the generic "couldn't find a route" tier — the UX is two-tier by design.
            PhaseState::Emit => {
                if self.em.is_none() {
                    // Entering the emit phase: the constructor writes the reserved header — the
                    // plan's first sink write (everything before this point was read-only).
                    let em = match ObcrEmitter::new(sink) {
                        Ok(em) => em,
                        Err(_) => return self.fail(NavError::NoPath),
                    };
                    self.em = Some(em);
                    if self.chain_len == 1 {
                        // Both endpoints snapped to the same node: a single-point route, length 0.
                        let e = &scratch.entries[scratch.heap[0] as usize];
                        let (lon, lat) = (e.lon, e.lat);
                        if self.em.as_mut().unwrap().push(sink, lon, lat, 0, 0).is_err() {
                            return self.fail(NavError::NoPath);
                        }
                        self.phase = PhaseState::Finish;
                        return Step::Running;
                    }
                }
                for _ in 0..NAV_EMIT_HOPS_PER_STEP {
                    if self.hop < 1 {
                        break;
                    }
                    if let Err(e) = self.emit_hop(reader, scratch, tiles, sink) {
                        return self.fail(e);
                    }
                    self.hop -= 1;
                }
                if self.hop < 1 {
                    self.phase = PhaseState::Finish;
                }
                Step::Running
            }
            PhaseState::Finish => {
                let stats = EmitStats {
                    min_ele_m: 0,
                    max_ele_m: 0,
                    ascent_m: 0,
                    descent_m: 0,
                    total_distance_m: Some(self.total_m),
                };
                let Some(em) = self.em.take() else {
                    return self.fail(NavError::NoPath); // unreachable: Emit always sets it
                };
                match em.finish(sink, &self.name, stats, &mut Vec::<WpPlace, MAX_WAYPOINTS>::new()) {
                    Ok(route) => {
                        self.phase = PhaseState::Terminal(Ok(route));
                        Step::Done(route)
                    }
                    Err(_) => self.fail(NavError::NoPath),
                }
            }
            PhaseState::Terminal(r) => match r {
                Ok(stats) => Step::Done(stats),
                Err(e) => Step::Failed(e),
            },
        }
    }

    /// Stage the found path goal→start in the (now dead) heap array — `came_from` holds **slot
    /// indices**, so the walk is direct indexing; path length is bounded by the tracked-node
    /// count, so it always fits. Sets the emit cursors + the summed-cost header total (the
    /// goal's `g`, saturated at 65 535 m like every stored cost).
    fn stage_chain<const N: usize>(&mut self, scratch: &mut NavScratch<N>, goal_idx: usize) -> Result<(), NavError> {
        self.total_m = scratch.entries[goal_idx].g as u32;
        let mut chain_len = 0usize;
        let mut cur = goal_idx;
        loop {
            if chain_len >= scratch.used as usize {
                return Err(NavError::NoPath); // longer than the tracked set = a corrupt cycle
            }
            scratch.heap[chain_len] = cur as u16;
            chain_len += 1;
            if scratch.entries[cur].node_id == self.start_id {
                break;
            }
            cur = scratch.entries[cur].came_from as usize;
            if cur >= N {
                return Err(NavError::NoPath); // corrupt slot index — fail, don't index OOB
            }
        }
        self.chain_len = chain_len as u16;
        self.hop = chain_len as u16 - 1;
        self.last = None;
        Ok(())
    }

    /// Emit one path hop: fetch hop `self.hop`'s edge polyline oriented via
    /// [`Reader::nav_edge_oriented`] and push it, deduping the seam vertex shared with the
    /// previous hop so the OBCR carries one continuous polyline. Elevation is zero throughout
    /// (no DEM, locked on #116).
    fn emit_hop<const N: usize>(
        &mut self,
        reader: &Reader,
        scratch: &mut NavScratch<N>,
        tiles: &mut NavTileCache,
        sink: &mut dyn ByteSink,
    ) -> Result<(), NavError> {
        let hop = self.hop as usize;
        let prev = &scratch.entries[scratch.heap[hop] as usize];
        let cur = &scratch.entries[scratch.heap[hop - 1] as usize];
        let em = self.em.as_mut().ok_or(NavError::NoPath)?;
        let mut last = self.last;
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
        self.last = last;
        if werr {
            return Err(NavError::NoPath);
        }
        Ok(())
    }
}

/// One-shot convenience over [`NavPlanner`]: loop [`step`](NavPlanner::step) to completion.
/// What the route-level tests and the headless sim use; interactive hosts step the planner
/// themselves, one bounded step per pass.
pub fn plan_route<const N: usize>(
    reader: &Reader,
    from: (i32, i32),
    to: (i32, i32),
    name: &str,
    scratch: &mut NavScratch<N>,
    tiles: &mut NavTileCache,
    sink: &mut dyn ByteSink,
) -> Result<RouteStats, NavError> {
    let mut planner = NavPlanner::new(from, to, name);
    loop {
        match planner.step(reader, scratch, tiles, sink) {
            Step::Running => {}
            Step::Done(stats) => return Ok(stats),
            Step::Failed(e) => return Err(e),
        }
    }
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
