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
//! because the router must coexist with the map cache and the render scratch in RAM.
//! On a dense graph it fills — but a full table no longer aborts (N4, epic #533):
//! the first failed insert latches a `table_full` flag and the search **continues
//! without inserting new nodes** (relaxing already-tracked nodes — decrease-key — still
//! works with zero allocations), so a goal already discovered can still be reached and
//! returned (its path may then exceed the ε bound — accepted). Only when the frontier
//! finally drains does the planner report: `table_full` ⇒ [`NavError::Exhausted`], never
//! full ⇒ [`NavError::NoPath`]. Termination holds: the tracked set is finite, every
//! re-open strictly lowers an integer `g ≥ 0`, and no new node ever enters once full — the
//! frontier must empty.
//!
//! **Profile-weighted edges** (epic #533, N3): each edge is relaxed by the selected
//! **bike profile's** multiplier for its §8.3 `way_kind` — `weighted = (cost_m ×
//! mult(kind)) >> 4`, `mult` a 40-byte per-plan lookup (32 highway × 8 surface `u8`
//! 1/16 bytes, combined at lookup) resolved once from the map's §8.6 profile table. An
//! out-of-range profile index falls back to profile 0 (a stale device setting must
//! never brick routing — never an error). A **forbidden** class (`mult == 0`) is simply
//! not relaxed: the neighbor is skipped, so the graph stays whole for the other
//! profiles and an endpoint whose only escapes are forbidden drains the frontier to an
//! honest [`NavError::NoPath`]. The *displayed* distance is unweighted — the planner sums
//! each hop's raw edge `length_m` at emit, not the weighted `g`.
//!
//! **Bounded suboptimality, not exactness** (decided 2026-07-06 — range in fixed
//! memory): the priority is `f = g + ε·h` with ε = [`NAV_EPSILON_NUM`] /
//! [`NAV_EPSILON_DEN`] = 1.3. The heuristic itself never overestimates the *weighted*
//! cost — it is the great-circle distance to the goal in the *same*
//! local-equirectangular metric the packer summed for every edge's `cost_m`, and every
//! non-forbidden profile multiplier is **≥ 1.0** (packer-enforced + reader-clamped), so
//! `weighted cost ≥ ground length ≥ great-circle`: `h` stays admissible unchanged. Weighted
//! A\* therefore returns a path of cost **≤ 1.3× the cheapest route under the selected
//! profile** (in practice a few percent on road networks). What ε buys is *reach*: plain
//! A\* explores roughly quadratically with distance and exhausted the fixed table ~1.5 km
//! out; the goal-greedy inflated search settles a narrow corridor instead, planning
//! multi-km routes in the same table.
//!
//! **The planner is resumable** (#499): planning is a [`NavPlanner`] step machine —
//! snap → search (settles until [`NAV_MISSES_PER_STEP`] cache misses, capped at
//! [`NAV_SETTLES_PER_STEP_CAP`]) → emit ([`NAV_EMIT_HOPS_PER_STEP`] edge fetches per step) → done
//! — and the host runs
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

/// Search-phase step budget, **by cache misses** (N4, epic #533): a [`NavPlanner::step`] settles
/// nodes until it has incurred this many [`NavTileCache`] misses, then returns. A miss is the only
/// expensive unit of a settle — one ~512 B SD chunk read; a hit reads nothing. Budgeting by misses
/// (read `tiles.stats().misses` delta inside the loop — no clock, no new dependency) makes a step's
/// wall time roughly constant at ~6 SD reads whatever the cache hit rate, instead of the old fixed
/// "8 settles per step" which paced by *work attempted* and so ran a warm step (mostly hits) far
/// under the SD envelope while still charging a full pass. With the 8-slot cache the same real
/// route now paces to far fewer steps (measured on grimsel: the search's step count drops several-
/// fold), which is where the board's per-plan wall-time floor (`LOOP_MS` × steps) comes down.
pub const NAV_MISSES_PER_STEP: u32 = 6;

/// Hard settle cap per search [`NavPlanner::step`] (N4): even a **fully warm** step (every settle a
/// cache hit ⇒ [`NAV_MISSES_PER_STEP`] never reached) returns after this many settles, so a step's
/// worst-case pass time — pure in-RAM heap + record work, no SD — stays bounded and the host's
/// render/input/watchdog cadence is never starved by one runaway step. 8× the old fixed budget: a
/// warm step now does up to this much useful search where the pre-N4 code stopped at 8.
pub const NAV_SETTLES_PER_STEP_CAP: u32 = 64;

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
    /// The fixed scratch filled and the frontier then drained without the goal ever popping — the
    /// device's honest "too far" (dense graph, long route, or an unreachable/hopeless target). A
    /// full table no longer aborts on sight (N4): the search continues without inserting new nodes
    /// and only fails here once the frontier empties, so a goal reachable *before* the fill still
    /// succeeds (see [`NavPlanner`]).
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
/// fix). `g`/`h` in `u16` meters saturate at 65 535 m and a saturated cost only makes its
/// node maximally unattractive (never mis-ordered, never wrapped). `g` now accumulates
/// **weighted** cost (`(cost_m × mult) >> 4`, N3), so it saturates *earlier* than plain
/// distance — a 4× multiplier ⇒ ~16 km of that class fills the field — but this is the
/// same graceful degradation: the fixed table exhausts long before saturation matters on
/// real terrain (profiles are capped ≤ ~8×), and with no distance cap a very distant goal
/// still degrades the ordering toward uniform expansion until the table exhausts (see the
/// module doc's no-cap note). The emitted route's displayed length is the unweighted
/// `length_m` sum, immune to this.
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

/// The selected bike profile's edge-weight lookup, resolved **once per plan** from the map's §8.6
/// profile table (epic #533, N3): the 32 highway + 8 surface `u8` 1/16-fixed-point multipliers
/// (`16` = 1.0×, `0` = forbidden), copied by value into the planner. Kept as the raw 40 bytes and
/// **combined at lookup** ([`mult`](Self::mult)) rather than pre-expanded to a 256-entry
/// `way_kind → multiplier` table — 40 B of `.bss` next to the scratch, no 256 B table, and one
/// multiply per relaxation. Mirrors [`obc_reader::MapProfile::multiplier`]'s integer arithmetic
/// exactly (the reader owns the same combine for its own callers); the copy is what lets the
/// weighting run with no `Reader` borrow held across the search.
#[derive(Clone, Copy)]
struct ProfileMult {
    highway: [u8; 32],
    surface: [u8; 8],
}

impl ProfileMult {
    /// The all-1.0× table: every non-forbidden multiplier `16`. The pre-resolution placeholder a
    /// fresh [`NavPlanner`] holds (overwritten at its first step) and the fallback for the
    /// degenerate empty-profile-table map (which snaps to nothing and fails first anyway).
    const NEUTRAL: ProfileMult = ProfileMult { highway: [16; 32], surface: [16; 8] };

    /// Resolve the profile selected by `profile_idx` from the reader's parsed §8.6 table. An
    /// **out-of-range index falls back to profile 0** (locked on #536: a stale device profile
    /// setting must never brick routing — never an error). The table always carries ≥ 1 profile
    /// (the reader's parse rejects `profile_count == 0`), so profile 0 always exists for a map with
    /// a graph; [`NEUTRAL`](Self::NEUTRAL) only stands in for the pathological empty table.
    fn resolve(reader: &Reader, profile_idx: u8) -> ProfileMult {
        let profiles = reader.nav_profiles();
        match profiles.get(profile_idx as usize).or_else(|| profiles.first()) {
            Some(p) => ProfileMult { highway: p.highway, surface: p.surface },
            None => ProfileMult::NEUTRAL,
        }
    }

    /// Effective edge multiplier for a packed `way_kind` byte in 1/16 fixed-point:
    /// `(highway[kind & 31] × surface[kind >> 5]) >> 4`. `None` when either class is **forbidden**
    /// (a `0` byte) — the neighbor is then skipped in relaxation (§8.6).
    #[inline]
    fn mult(&self, way_kind: u8) -> Option<u32> {
        let mh = self.highway[(way_kind & 0x1F) as usize] as u32;
        let ms = self.surface[(way_kind >> 5) as usize] as u32;
        if mh == 0 || ms == 0 {
            None
        } else {
            Some((mh * ms) >> 4)
        }
    }
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

    /// Allocate a zeroed `NavScratch` **directly on the heap**, never on the stack.
    ///
    /// The A* table is tens of KB (~39 KB at [`NAV_MAX_NODES`]), so `Box::new(Self::new())` would
    /// first build the whole thing on the stack and then copy it — a silent overflow on a small
    /// stack (the simulator's wasm build). This owns the format crate's private invariant that a
    /// zeroed allocation *is* `new()`: every field is all-zero — `entries` is `[NavEntry::EMPTY;
    /// N]` (`EMPTY` is the zero entry), `heap` is `[0; N]`, and `used`/`heap_len` are `0`. Adding a
    /// non-zero-default field would break this, so the invariant lives *here*, in the crate that
    /// owns the fields, instead of leaking into every host that heap-allocates one.
    ///
    /// Host-only ([`alloc`](crate) feature): the device keeps its scratch as a `.bss` static and
    /// never calls this.
    #[cfg(feature = "alloc")]
    pub fn new_boxed() -> alloc::boxed::Box<Self> {
        // SAFETY: an all-zero `NavScratch` is bit-identical to `new()` (see the field-by-field
        // argument above), so a zeroed allocation is a fully initialised value.
        unsafe { alloc::boxed::Box::<Self>::new_zeroed().assume_init() }
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

    /// Insert a fresh entry for `id` (must not be present), un-queued. `Err(Exhausted)` when the
    /// scratch is full — the caller ([`settle`]) latches `table_full` and drops the node rather than
    /// aborting (N4 salvage); the seed insert in `SnapTo` is the one caller that still fails hard.
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
    /// The weighted-A\* search, settling until [`NAV_MISSES_PER_STEP`] cache misses (capped at
    /// [`NAV_SETTLES_PER_STEP_CAP`] settles).
    Search,
    /// Streaming the found path's edge geometry into the OBCR ([`NAV_EMIT_HOPS_PER_STEP`] hops
    /// per step), then the finishing header patch.
    Emit,
    /// Terminal — [`step`](NavPlanner::step) idempotently re-returns the outcome.
    Done,
}

/// One snapped plan endpoint: `(node_id, (lon, lat))` µdeg — see [`NavPlanner::endpoints`].
pub type NavEndpoint = (u32, (i32, i32));

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
    /// The selected bike profile index (device setting; N5 threads the real value — N3 hosts pass
    /// `0`). Resolved to [`mult`](Self::mult) at the first step; out-of-range falls back to profile 0.
    profile_idx: u8,
    /// The profile's 40-byte multiplier lookup, resolved once from the reader at the first step
    /// (neutral until then). Every edge is relaxed through [`ProfileMult::mult`].
    mult: ProfileMult,
    /// The snapped endpoints (valid once their phase has run).
    start_id: u32,
    start_c: (i32, i32),
    goal_id: u32,
    goal_c: (i32, i32),
    /// Total settles so far — the RTT line's `settles=` figure, and the budget tests' probe.
    settles: u32,
    /// Latched once an insert has failed — the scratch is full (N4 salvage). While set, the search
    /// relaxes only already-tracked nodes (decrease-key); new discoveries are dropped. Distinguishes
    /// the two frontier-drain outcomes: set ⇒ [`NavError::Exhausted`], clear ⇒ [`NavError::NoPath`].
    table_full: bool,
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
    /// A planner for one route request routed under bike profile `profile_idx` (§8.6; an
    /// out-of-range index falls back to profile 0 at the first step — never an error). Touches
    /// nothing yet — the first [`step`](NavPlanner::step) resets the caller's scratch + tile cache,
    /// resolves the profile, and starts snapping.
    pub fn new(from: (i32, i32), to: (i32, i32), name: &str, profile_idx: u8) -> Self {
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
            profile_idx,
            mult: ProfileMult::NEUTRAL,
            start_id: 0,
            start_c: (0, 0),
            goal_id: 0,
            goal_c: (0, 0),
            settles: 0,
            table_full: false,
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

    /// The snapped endpoints — `((start_id, start_coord), (goal_id, goal_coord))`, `(lon, lat)`
    /// µdeg; zeroes for an endpoint whose snap phase hasn't run yet. A diagnostic for the
    /// `nav_repro` harness (#501): lets a host run report exactly which graph nodes the plan
    /// ran between.
    pub fn endpoints(&self) -> (NavEndpoint, NavEndpoint) {
        ((self.start_id, self.start_c), (self.goal_id, self.goal_c))
    }

    /// Terminal-transition helper: latch and return the failure.
    fn fail(&mut self, e: NavError) -> Step {
        self.phase = PhaseState::Terminal(Err(e));
        Step::Failed(e)
    }

    /// Run **one bounded unit** of planning: one endpoint snap, a miss-budgeted burst of settles
    /// ([`NAV_MISSES_PER_STEP`] misses / [`NAV_SETTLES_PER_STEP_CAP`] cap), [`NAV_EMIT_HOPS_PER_STEP`]
    /// emit hops, or the finishing header patch — then return. `reader`/`scratch`/`tiles`/`sink` are the caller's per-pass views over the same
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
                // First step of the plan: claim the caller's buffers and resolve the bike profile
                // once (out-of-range index → profile 0; see `ProfileMult::resolve`).
                scratch.reset();
                tiles.reset();
                self.mult = ProfileMult::resolve(reader, self.profile_idx);
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
            // Settle until the step's miss budget: pop the best-f node, close it, relax its record's
            // neighbors. Terminates: a settle closes a node or (re-open) strictly lowers an integer
            // g ≥ 0, the frontier is bounded by the table, and no new node enters once full.
            PhaseState::Search => {
                let miss_start = tiles.stats().misses;
                let mut settled_this_step: u32 = 0;
                loop {
                    let Some(idx) = scratch.heap_pop() else {
                        // Frontier drained. A table that filled ran out of room short of the goal
                        // (Exhausted — the device's "too far"); one that never filled means the goal
                        // is genuinely disconnected/unreachable (NoPath). Preserves the two-tier UX.
                        let e = if self.table_full { NavError::Exhausted } else { NavError::NoPath };
                        return self.fail(e);
                    };
                    if scratch.entries[idx].node_id == self.goal_id {
                        // Goal reached — success even if the table filled en route (the returned
                        // path may then exceed the ε bound; accepted, see the type doc).
                        return match self.stage_chain(scratch, idx) {
                            Ok(()) => {
                                self.phase = PhaseState::Emit;
                                Step::Running
                            }
                            Err(e) => self.fail(e),
                        };
                    }
                    self.settles = self.settles.wrapping_add(1);
                    settled_this_step += 1;
                    scratch.entries[idx].meta |= META_CLOSED;
                    // Relax neighbors. A read failure is the only hard error; a full table latches
                    // `table_full` and keeps searching (decrease-key on tracked nodes still relaxes).
                    if let Err(e) =
                        settle::<N>(reader, scratch, tiles, idx, self.goal_c, &self.mult, &mut self.table_full)
                    {
                        return self.fail(e);
                    }
                    // Budget by cache misses — the only expensive unit (≈ one SD chunk read each) —
                    // so a step's wall time is ~constant whatever the hit rate; the settle cap bounds
                    // a fully-warm (hit-only) step. The check trails the settle, so a step always
                    // makes at least one node of progress.
                    if tiles.stats().misses - miss_start >= NAV_MISSES_PER_STEP
                        || settled_this_step >= NAV_SETTLES_PER_STEP_CAP
                    {
                        break;
                    }
                }
                Step::Running
            }
            // Stream up to the hop budget of edge geometry start→goal. I/O failures degrade to
            // the generic "couldn't find a route" tier — the UX is two-tier by design.
            PhaseState::Emit => {
                if self.em.is_none() {
                    match self.arm_emitter(scratch, sink) {
                        Ok(true) => return Step::Running, // degenerate single-point route emitted
                        Ok(false) => {}
                        Err(e) => return self.fail(e),
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
            PhaseState::Finish => match self.finish_emit(sink) {
                Ok(route) => {
                    self.phase = PhaseState::Terminal(Ok(route));
                    Step::Done(route)
                }
                Err(e) => self.fail(e),
            },
            PhaseState::Terminal(r) => match r {
                Ok(stats) => Step::Done(stats),
                Err(e) => Step::Failed(e),
            },
        }
    }

    /// Arm the emitter on entering the emit phase — its constructor writes the reserved OBCR
    /// header, the plan's **first** sink write (everything before this point was read-only).
    /// Returns `Ok(true)` when the degenerate single-point route (both endpoints snapped to one
    /// node) was fully emitted and the phase advanced straight to Finish.
    ///
    /// `#[inline(never)]` — the #419/#501 stack discipline: the ~9 kB `ObcrEmitter` construction
    /// temporary must live in THIS immediately-popped frame. Inlined, fat LTO reserved its slot
    /// in the whole step frame for **every** step — part of the measured 26.5 kB `nav_step`
    /// monster frame that overflowed the DK stack (the #501 on-glass HardFault's true cause).
    #[inline(never)]
    fn arm_emitter<const N: usize>(
        &mut self,
        scratch: &NavScratch<N>,
        sink: &mut dyn ByteSink,
    ) -> Result<bool, NavError> {
        let em = ObcrEmitter::new(sink).map_err(|_| NavError::NoPath)?;
        self.em = Some(em);
        if self.chain_len == 1 {
            let e = &scratch.entries[scratch.heap[0] as usize];
            let (lon, lat) = (e.lon, e.lat);
            if self.em.as_mut().is_none_or(|em| em.push(sink, lon, lat, 0, 0).is_err()) {
                return Err(NavError::NoPath);
            }
            self.phase = PhaseState::Finish;
            return Ok(true);
        }
        Ok(false)
    }

    /// Consume the emitter and patch the header — the plan's last writes.
    ///
    /// `#[inline(never)]` for the same reason as [`arm_emitter`](Self::arm_emitter):
    /// `Option::take` moves the ~9 kB emitter into a local; that temporary belongs in this
    /// popped frame, never in the step frame.
    #[inline(never)]
    fn finish_emit(&mut self, sink: &mut dyn ByteSink) -> Result<RouteStats, NavError> {
        let stats =
            EmitStats { min_ele_m: 0, max_ele_m: 0, ascent_m: 0, descent_m: 0, total_distance_m: Some(self.total_m) };
        let Some(em) = self.em.take() else {
            return Err(NavError::NoPath); // unreachable: Emit always arms it
        };
        em.finish(sink, &self.name, stats, &mut Vec::<WpPlace, MAX_WAYPOINTS>::new()).map_err(|_| NavError::NoPath)
    }

    /// Stage the found path goal→start in the (now dead) heap array — `came_from` holds **slot
    /// indices**, so the walk is direct indexing; path length is bounded by the tracked-node
    /// count, so it always fits. Sets the emit cursors only — the goal's `g` is now the *weighted*
    /// cost, so it is **not** the header total; [`emit_hop`](Self::emit_hop) sums the raw edge
    /// `length_m` into `total_m` instead (N3 distance honesty).
    #[inline(never)] // #419/#501: keep the step dispatcher thin
    fn stage_chain<const N: usize>(&mut self, scratch: &mut NavScratch<N>, goal_idx: usize) -> Result<(), NavError> {
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
    /// previous hop so the OBCR carries one continuous polyline. The edge's raw ground `length_m`
    /// (the call's return, no longer dead) accumulates into `total_m` — the **unweighted**
    /// displayed distance, summed here rather than read off the weighted `g` (N3). Elevation is
    /// zero throughout (no DEM, locked on #116).
    #[inline(never)] // #419/#501: a phase-boundary frame — never inlined into the step frame
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
        let length_m = reader
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
        // Real ground meters for the displayed total (saturating, like every stored cost).
        self.total_m = self.total_m.saturating_add(length_m);
        Ok(())
    }
}

/// One-shot convenience over [`NavPlanner`]: loop [`step`](NavPlanner::step) to completion under
/// bike profile `profile_idx` (out-of-range → profile 0). What the route-level tests and the
/// headless sim use; interactive hosts step the planner themselves, one bounded step per pass.
// The arg list is the plan request (`from`/`to`/`name`/`profile_idx`) plus the four caller-owned
// buffers the planner never allocates — grouping them into a struct would just move the noise.
#[allow(clippy::too_many_arguments)]
pub fn plan_route<const N: usize>(
    reader: &Reader,
    from: (i32, i32),
    to: (i32, i32),
    name: &str,
    profile_idx: u8,
    scratch: &mut NavScratch<N>,
    tiles: &mut NavTileCache,
    sink: &mut dyn ByteSink,
) -> Result<RouteStats, NavError> {
    let mut planner = NavPlanner::new(from, to, name, profile_idx);
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
/// the inline `(coord, cost_m, way_kind)`, weighting the cost by the plan's profile
/// (`mult`). A neighbor whose `way_kind` is **forbidden** under the profile (`mult` is
/// `None`) is skipped — not relaxed — so the graph stays whole for other profiles. A
/// node the walk doesn't yield (corrupt map) simply relaxes nothing; the search
/// continues on whatever frontier remains.
///
/// **Exhaustion salvage** (N4): when the scratch is full a *new* discovery can't be inserted, so it
/// is dropped and `*table_full` is latched — but decrease-key of an already-tracked neighbor still
/// relaxes normally (zero allocations). The only hard error is a read failure ([`NavError::NoPath`]);
/// running out of table is no longer an error here — the caller drains the frontier and maps a
/// latched `table_full` to [`NavError::Exhausted`] then.
#[inline(never)] // #419/#501: a phase-boundary frame — never inlined into the step frame
fn settle<const N: usize>(
    reader: &Reader,
    scratch: &mut NavScratch<N>,
    tiles: &mut NavTileCache,
    idx: usize,
    goal_c: (i32, i32),
    mult: &ProfileMult,
    table_full: &mut bool,
) -> Result<(), NavError> {
    let settled = scratch.entries[idx];
    let view = BBox { min_lon: settled.lon, min_lat: settled.lat, max_lon: settled.lon, max_lat: settled.lat };
    reader
        .for_each_nav_node_cached(&view, tiles, |n| {
            // Idempotent under N2's bin-packed chunks (a node may be yielded more than once when two
            // leaves share a chunk): only the settled node's own record relaxes anything.
            if n.id != settled.node_id {
                return;
            }
            for nb in n.neighbors() {
                // Profile-weighted edge cost: `weighted = (cost_m × mult(kind)) >> 4` (mult in
                // 1/16 fixed-point). A forbidden class (`mult == None`) is skipped entirely — the
                // neighbor is never relaxed, so the graph stays whole for the other profiles.
                let Some(m) = mult.mult(nb.way_kind) else {
                    continue;
                };
                let weighted = (nb.cost_m.saturating_mul(m)) >> 4;
                // u16-saturating tentative cost: a saturated g is just maximally
                // unattractive (see the layout note) — never wrapped, never mis-ordered.
                let tentative = sat16((settled.g as u32).saturating_add(weighted));
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
                    // A *new* node: insert only while the table has room. Once full, drop it and
                    // latch — the search continues, relaxing tracked nodes but adding no new ones
                    // (this is what makes the frontier provably drain: no new nodes, every re-open
                    // strictly lowers an integer g ≥ 0).
                    None => match scratch.insert(nb.id, nb.lon, nb.lat) {
                        Ok(j) => {
                            let e = &mut scratch.entries[j];
                            e.g = tentative;
                            e.h = sat16(ground_dist_m((nb.lon, nb.lat), goal_c) as u32);
                            e.came_from = idx as u16;
                            e.edge_used = nb.edge_id;
                            scratch.heap_push(j);
                        }
                        Err(_) => *table_full = true,
                    },
                }
            }
        })
        .map_err(|_| NavError::NoPath)?;
    Ok(())
}

/// Snap `p` to the nearest routable node within [`SNAP_RADIUS_M`] — the POI query's
/// expanding-ring walk shape over the node quadtree: start with a small square view,
/// double it until the best find is provably nearest (its distance ≤ the ring's
/// half-extent — everything outside the square is at least that far), capping at the
/// snap radius. The cap makes the final ring exhaustive by construction (a 250 m
/// half-extent square contains the whole 250 m disc), so unlike the POI query no
/// map-cover fallback is needed. Repeat visits across rings re-hit the tile cache.
///
/// **Kind-agnostic** (locked on #536): snap picks the nearest node of *any* kind — a rider
/// standing on a footway must still snap; the profile shapes the route *away* from it, not the
/// snap. One accepted v1 consequence: a node **all of whose edges are forbidden** under the active
/// profile is a dead snap — the search then drains the frontier to an honest [`NavError::NoPath`].
#[inline(never)] // #419/#501: a phase-boundary frame — never inlined into the step frame
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
