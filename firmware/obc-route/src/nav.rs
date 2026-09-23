//! On-device point-to-point routing over the OBCM nav graph.
//!
//! [`plan_route`] runs weighted A\* from the rider fix to a goal and writes the result as a
//! complete OBCR through the shared [`ObcrEmitter`]. `no_std`, identical on device and sim.
//!
//! Every buffer is caller-owned ([`NavScratch`] plus the reader's [`NavTileCache`]): a large local
//! here overflows the device stack. A settle is one quadtree descent to the node's coordinate, so
//! the planner needs no node-id index, and each neighbor record carries its own coordinate and
//! cost, so relaxation does no second fetch.
//!
//! The scratch is a fixed [`NAV_MAX_NODES`] table, the same size on every target, so the sim's
//! plannable range is the device's. A full table does not abort the search: it stops new inserts
//! and keeps relaxing tracked nodes, then reports [`NavError::Exhausted`] when the frontier
//! drains. The search is bounded-suboptimal, not exact: the priority is `f = g + eps*h` and the
//! [`NAV_EPSILON_LADDER`] climbs to a greedier rung after an exhaustion. There is no distance cap;
//! the fixed table is the range limit.
//!
//! An edge costs the profile-weighted ground length plus its own ascent charged at the profile
//! climb weight. Costs are only added, never subtracted, so `h` (great-circle distance in the same
//! metric the packer summed each `cost_m` in) stays a lower bound. Descent credits are therefore
//! not permitted; gradient effects on time belong in ETA.
//!
//! The planner is resumable: [`NavPlanner::step`] does one bounded unit of work, so the host runs
//! render, input and watchdog between steps. Nothing is written to the sink before the emit phase,
//! so a cancelled plan leaves the sink untouched.
//!
//! This module and the reader's record decode are cast-free: each field is assembled byte-wise
//! with `from_le_bytes`, because records sit at odd offsets and a typed view over them is
//! alignment UB on ARM. Run `cargo +nightly miri test -p obc-route --test nav` after a change to
//! the planner or the record decode.

use heapless::Vec;

use crate::convert::{ObcrEmitter, RouteStats, WpPlace};
use crate::corridor::Corridor;
use crate::reader::MAX_WAYPOINTS;
use obc_elevation::{ElevationSource, ELE_DEADBAND_M};
use obc_formats::bike::BikeType;
use obc_formats::io::{ByteSink, Error};
use obc_formats::obcr::NAME_CAP;
use obc_map_scene::{cos_lat, ground_dist_m};
use obc_map_scene::{BBox, M_PER_DEG};
use obc_reader::{NavEdgeCandidate, NavEdgePosition, NavEdgeSnap, NavTileCache, Reader};

/// Maximum accepted distance from the requested position to the winning road polyline. The wider
/// [`SNAP_LOOKUP_RADIUS_M`] only finds candidate edge ids; it does not weaken this limit.
pub(crate) const SNAP_RADIUS_M: f32 = 100.0;

/// Maximum ground distance from any point on an indexed edge to its nearest endpoint/interior
/// anchor. The mathematical bound is 150 m; one metre covers microdegree anchor rounding.
const SNAP_INDEX_REACH_M: f32 = 151.0;
/// Node/anchor discovery radius. A road point is within [`SNAP_INDEX_REACH_M`] of a lookup record,
/// so this gives a complete search.
const SNAP_LOOKUP_RADIUS_M: f32 = SNAP_INDEX_REACH_M + SNAP_RADIUS_M;
/// First pass. If its winner is within 49 m, the triangle bound proves no record outside this
/// window names a closer edge; if not, a full pass follows.
const SNAP_INITIAL_LOOKUP_RADIUS_M: f32 = 200.0;

/// Reserved ids for exact projected endpoints. Packed node ids are dense from zero and cannot use
/// the top two values.
const VIRTUAL_START_ID: u32 = u32::MAX;
const VIRTUAL_GOAL_ID: u32 = u32::MAX - 1;

/// Largest ground gap (m) between two emitted OBCR points while terrain is available; a longer
/// segment is split into interpolated points, each sampled like a real vertex. The step is set
/// against the raster: terrain postings are ~40 m apart, so 250 m cannot invent detail.
pub(crate) const ELE_SAMPLE_STEP_M: f32 = 250.0;

/// Guard on interpolated points per edge segment. The packer bounds a real segment to ~3.3 km
/// (~14 steps), so more than this is corrupt geometry.
const ELE_MAX_DENSIFY_STEPS: u32 = 64;

/// The height move (m) that makes the emitter keep a vertex. It is [`ELE_DEADBAND_M`] so that the
/// exported route's climb agrees with the header: a geometric decimator alone drops a crest that
/// sits on a straight road.
const ELE_KEEP_M: i16 = ELE_DEADBAND_M as i16;

/// Heuristic inflation as integer `(num, den)` ratios: `f = g + (num*h)/den`. The search starts at
/// rung 0 and retries at a greedier rung only on [`NavError::Exhausted`]. A greedier rung settles a
/// narrower corridor, so the same fixed table reaches farther. [`NavError::NoPath`] never
/// escalates: a retry cannot connect an island. The path is bounded by the successful rung's ratio
/// times the profile-optimal cost.
pub const NAV_EPSILON_LADDER: [(u32, u32); 3] = [(13, 10), (2, 1), (3, 1)];

/// Search-phase step budget, counted in source fills (graph chunks plus route-private index
/// windows): a [`NavPlanner::step`] settles until it incurs this many reads, then returns. A read
/// is the one expensive unit, so a step's wall time stays about constant at any cache hit rate.
pub const NAV_MISSES_PER_STEP: u32 = 12;

/// Settle cap per search step. A fully warm step never reaches [`NAV_MISSES_PER_STEP`], so this
/// bounds its pass time and keeps the host's render and watchdog cadence.
pub const NAV_SETTLES_PER_STEP_CAP: u32 = 64;

/// Emit-phase step budget: path hops (edge-geometry fetches plus OBCR pushes) per
/// [`NavPlanner::step`].
pub(crate) const NAV_EMIT_HOPS_PER_STEP: u16 = 8;

/// How the router reports failure. The UX has two tiers: "too far to route here" and "couldn't
/// find a route".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavError {
    /// An endpoint did not snap, the frontier emptied short of the goal, or a read or write
    /// failed. Every non-range failure lands here. A rider cancel is not an error: the host stops
    /// stepping.
    NoPath,
    /// The table filled and the frontier then drained without the goal popping.
    Exhausted,
}

/// Nodes the fixed A\* scratch tracks, open and closed together. One 24-byte [`NavEntry`] plus one
/// 2-byte heap slot each: 1536 nodes is 39 936 B, under the device's 40 kB nav budget. The same
/// value on every target, so the sim's plannable range is the device's.
pub const NAV_MAX_NODES: usize = 1536;

const META_OCCUPIED: u16 = 1 << 15;
/// The node is settled. It can re-open on a shorter `g`.
const META_CLOSED: u16 = 1 << 14;
const META_POS_MASK: u16 = 0x3FFF;
/// `heap_pos` sentinel: not queued.
const HEAP_NONE: u16 = 0x3FFF;

/// One tracked node. `repr(C)` pins the 24-byte size the [`NAV_MAX_NODES`] budget counts. `g` and
/// `h` are meters and saturate at 65 535; a saturated cost only makes its node maximally
/// unattractive, so the ordering is never wrong.
#[derive(Clone, Copy)]
#[repr(C)]
struct NavEntry {
    node_id: u32,
    lon: i32,
    lat: i32,
    edge_used: u32,
    g: u16,
    h: u16,
    /// The predecessor's table slot index, not a node id. Slots never move, so the emit
    /// chain-walk is direct indexing.
    came_from: u16,
    /// Packed occupied and closed flags plus the heap position (see the `META_*` consts).
    meta: u16,
}

impl NavEntry {
    /// All-zero, so a `static NavScratch` lands in `.bss` and a zeroed slot reads as free.
    const EMPTY: NavEntry = NavEntry { node_id: 0, lon: 0, lat: 0, edge_used: 0, g: 0, h: 0, came_from: 0, meta: 0 };

    /// The priority `f = g + eps*h`. The ratio comes from the owning [`NavScratch`] rather than a
    /// const, so a [`NAV_EPSILON_LADDER`] retry can re-order the same heap at a greedier rung. A
    /// zeroed scratch reads `0/0` and degrades to plain-`g` ordering instead of dividing by zero;
    /// every search re-seeds a real rung first.
    #[inline]
    fn f(&self, eps_num: u32, eps_den: u32) -> u32 {
        if eps_den == 0 {
            return self.g as u32;
        }
        (self.g as u32).saturating_add(eps_num * self.h as u32 / eps_den)
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

/// Saturate a `u32` meter figure into the entry's `u16` cost field.
#[inline]
fn sat16(m: u32) -> u16 {
    m.min(u16::MAX as u32) as u16
}

/// A fixed per-request preference. Highway/access prohibitions and the saved profile stay unchanged.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Objective {
    #[default]
    Profile,
    LessClimb,
    LeastClimb,
    Smoother,
    Smoothest,
    Shorter,
    Shortest,
}
impl Objective {
    pub const TRIALS: [Self; 7] = [
        Self::Profile,
        Self::LessClimb,
        Self::LeastClimb,
        Self::Smoother,
        Self::Smoothest,
        Self::Shorter,
        Self::Shortest,
    ];
}

/// The selected bike profile's edge-cost parameters, resolved once per plan and copied by value so
/// no `Reader` borrow is held across the search. Multipliers are 1/16 fixed point: `16` is 1.0x and
/// `0` is forbidden. The raw 40 bytes are combined at lookup rather than expanded to a 256-entry
/// table.
///
/// [`edge_cost`](Self::edge_cost) is the router's one cost model. POI plans and detours run the
/// same [`settle`], so there is no second formula to drift from.
#[derive(Clone, Copy)]
struct ProfileMult {
    highway: [u8; 32],
    surface: [u8; 8],
    /// Flat meters charged per meter of a neighbor's `ascent_m`. Widened once so relaxation does
    /// no cast. `0` is climb-blind, which is legal: a map without terrain decodes to it.
    climb: u32,
}

impl ProfileMult {
    /// The all-1.0x, climb-blind table: the placeholder a fresh [`NavPlanner`] holds until its
    /// first step, and the fallback for a map with an empty profile table.
    const NEUTRAL: ProfileMult = ProfileMult { highway: [16; 32], surface: [16; 8], climb: 0 };

    /// Resolve `bike`'s profile from the reader's profile table. A map with fewer profiles than
    /// the contract falls back to profile 0, so a malformed map never stops routing.
    fn resolve(reader: &Reader, bike: BikeType) -> ProfileMult {
        let profiles = reader.nav_profiles();
        match profiles.get(bike as usize).or_else(|| profiles.first()) {
            Some(p) => ProfileMult { highway: p.highway, surface: p.surface, climb: u32::from(p.climb_weight()) },
            None => ProfileMult::NEUTRAL,
        }
    }

    fn prefer(mut self, objective: Objective) -> Self {
        match objective {
            Objective::Profile => {}
            Objective::LessClimb => self.climb = (self.climb * 2).clamp(10, 255),
            Objective::LeastClimb => self.climb = (self.climb * 4).clamp(20, 255),
            Objective::Smoother | Objective::Smoothest => {
                let factor = if objective == Objective::Smoother { 2 } else { 4 };
                for surface in &mut self.surface[3..] {
                    *surface = surface.saturating_mul(factor);
                }
            }
            Objective::Shorter => self.climb /= 2,
            Objective::Shortest => self.climb = 0,
        }
        self
    }

    /// The weighted cost of one adjacency entry:
    ///
    /// ```text
    /// (cost_m * ((highway[kind & 31] * surface[kind >> 5]) >> 4)) >> 4  +  ascent_m * climb_weight
    /// ```
    ///
    /// `None` when either multiplier class is forbidden (a `0` byte). The neighbor is then skipped,
    /// not relaxed at a huge cost, so the graph stays whole for the other profiles.
    #[inline]
    fn edge_cost(&self, cost_m: u32, ascent_m: u16, way_kind: u8) -> Option<u32> {
        let mh = self.highway[(way_kind & 0x1F) as usize] as u32;
        let ms = self.surface[(way_kind >> 5) as usize] as u32;
        if mh == 0 || ms == 0 {
            return None;
        }
        let distance = (cost_m.saturating_mul((mh * ms) >> 4)) >> 4;
        // The climb term is only ever added. This is what keeps `h` admissible.
        Some(distance.saturating_add(u32::from(ascent_m).saturating_mul(self.climb)))
    }
}

/// The router's mutable state: an open-addressed `node_id -> NavEntry` table and a binary min-heap
/// of table indices ordered by `f`. Heap-position back-pointers make decrease-key O(log n) and keep
/// a node queued at most once, so the heap cannot outgrow the table.
///
/// Caller-owned, and callers depend on one property: [`NavScratch::new`] is `const` and all-zero,
/// so an all-zero block is `new()`. `N` is generic so tests can exercise exhaustion with a tiny
/// table.
pub struct NavScratch<const N: usize = NAV_MAX_NODES> {
    entries: [NavEntry; N],
    heap: [u16; N],
    /// Occupied table slots, also the bound for the emitted predecessor chain.
    used: u16,
    heap_len: u16,
    /// The current search's [`NAV_EPSILON_LADDER`] rung, set by [`NavPlanner::reseed`] before any
    /// heap operation runs.
    eps_num: u16,
    eps_den: u16,
}

// Slot indices (heap positions and `came_from`) are 14-bit, with `HEAP_NONE` as the sentinel.
const _: () =
    assert!(core::mem::size_of::<NavScratch<NAV_MAX_NODES>>() <= 40 * 1024, "NavScratch busts the LM20 40 kB cap");
const _: () = assert!(NAV_MAX_NODES < HEAP_NONE as usize, "table indices are 14-bit (meta packs flags above them)");
const _: () = assert!(core::mem::size_of::<NavEntry>() == 24, "the slimmed 24-byte entry layout drifted");

impl<const N: usize> NavScratch<N> {
    pub const fn new() -> Self {
        assert!(N > 0 && N < HEAP_NONE as usize);
        NavScratch { entries: [NavEntry::EMPTY; N], heap: [0; N], used: 0, heap_len: 0, eps_num: 0, eps_den: 0 }
    }

    /// Allocate a zeroed `NavScratch` directly on the heap, never on the stack. The table is tens
    /// of kB, so `Box::new(Self::new())` would build it on the stack first and overflow a small
    /// stack. Every field must stay zero-default for this to hold.
    #[cfg(feature = "alloc")]
    pub fn new_boxed() -> alloc::boxed::Box<Self> {
        // SAFETY: an all-zero `NavScratch` is bit-identical to `new()`, so a zeroed allocation is
        // a fully initialised value.
        unsafe { alloc::boxed::Box::<Self>::new_zeroed().assume_init() }
    }

    fn reset(&mut self) {
        for e in self.entries.iter_mut() {
            e.meta = 0;
        }
        self.used = 0;
        self.heap_len = 0;
    }

    /// Find an existing entry, or insert at the first free slot. The boolean is true only for a
    /// new entry. A full table still finds every tracked node, so exhaustion does not stop
    /// decrease-key or re-open.
    fn entry(&mut self, id: u32, lon: i32, lat: i32) -> Result<(usize, bool), NavError> {
        let mut i = id as usize % N;
        for _ in 0..N {
            if !self.entries[i].occupied() {
                self.entries[i] = NavEntry {
                    node_id: id,
                    lon,
                    lat,
                    edge_used: 0,
                    g: 0,
                    h: 0,
                    came_from: 0,
                    meta: META_OCCUPIED | HEAP_NONE,
                };
                self.used += 1;
                return Ok((i, true));
            }
            if self.entries[i].node_id == id {
                return Ok((i, false));
            }
            i = (i + 1) % N;
        }
        Err(NavError::Exhausted)
    }

    #[inline]
    fn heap_swap(&mut self, a: usize, b: usize) {
        self.heap.swap(a, b);
        self.entries[self.heap[a] as usize].set_heap_pos(a as u16);
        self.entries[self.heap[b] as usize].set_heap_pos(b as u16);
    }

    fn sift_up(&mut self, mut pos: usize) {
        let (en, ed) = (self.eps_num as u32, self.eps_den as u32);
        while pos > 0 {
            let parent = (pos - 1) / 2;
            if self.entries[self.heap[pos] as usize].f(en, ed) >= self.entries[self.heap[parent] as usize].f(en, ed) {
                break;
            }
            self.heap_swap(pos, parent);
            pos = parent;
        }
    }

    fn sift_down(&mut self, mut pos: usize) {
        let len = self.heap_len as usize;
        let (en, ed) = (self.eps_num as u32, self.eps_den as u32);
        loop {
            let (l, r) = (2 * pos + 1, 2 * pos + 2);
            let mut min = pos;
            if l < len
                && self.entries[self.heap[l] as usize].f(en, ed) < self.entries[self.heap[min] as usize].f(en, ed)
            {
                min = l;
            }
            if r < len
                && self.entries[self.heap[r] as usize].f(en, ed) < self.entries[self.heap[min] as usize].f(en, ed)
            {
                min = r;
            }
            if min == pos {
                return;
            }
            self.heap_swap(pos, min);
            pos = min;
        }
    }

    /// Queue entry `idx`, which must not already be queued. It cannot overflow: there is one heap
    /// slot per table slot.
    fn heap_push(&mut self, idx: usize) {
        let pos = self.heap_len as usize;
        self.heap[pos] = idx as u16;
        self.entries[idx].set_heap_pos(pos as u16);
        self.heap_len += 1;
        self.sift_up(pos);
    }

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

/// One [`NavPlanner::step`] outcome.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    Running,
    Done(RouteStats),
    /// The plan failed. The sink holds at most a torn prefix, which the caller discards.
    Failed(NavError),
}

/// The phase the *next* [`step`](NavPlanner::step) will spend its budget on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavPhase {
    Snap,
    Search,
    Emit,
    /// Terminal. [`step`](NavPlanner::step) re-returns the outcome.
    Done,
}

/// One snapped endpoint. A projection onto an exact endpoint collapses back to a real graph node;
/// an interior projection stays a virtual node joined to both edge endpoints.
#[derive(Clone, Copy)]
enum SnappedEndpoint {
    Node { id: u32, coord: (i32, i32) },
    Edge(NavEdgeSnap),
}

impl SnappedEndpoint {
    fn from_snap(edge: NavEdgeSnap) -> Self {
        if edge.position.coord == edge.a.coord {
            Self::Node { id: edge.a.id, coord: edge.a.coord }
        } else if edge.position.coord == edge.b.coord {
            Self::Node { id: edge.b.id, coord: edge.b.coord }
        } else {
            Self::Edge(edge)
        }
    }

    fn coord(self) -> (i32, i32) {
        match self {
            Self::Node { coord, .. } => coord,
            Self::Edge(edge) => edge.position.coord,
        }
    }

    fn node_id(self) -> u32 {
        match self {
            Self::Node { id, .. } => id,
            Self::Edge(_) => 0,
        }
    }

    fn edge(self) -> Option<NavEdgeSnap> {
        match self {
            Self::Node { .. } => None,
            Self::Edge(edge) => Some(edge),
        }
    }
}

/// The internal phase. [`NavPhase`] is its public projection.
enum PhaseState {
    SnapFrom,
    SnapTo,
    Search,
    Emit,
    Finish,
    Terminal(Result<RouteStats, NavError>),
}

/// The resumable route planner. It plans from `from` to `to`, both `(lon, lat)` microdegrees, and
/// writes a complete OBCR named `name` to the step's `sink`, one bounded unit of work per
/// [`step`](NavPlanner::step).
///
/// The planner holds the phase, the cursors and the [`ObcrEmitter`], which must survive across emit
/// steps. The emitter is about 9 kB by value, so a `NavPlanner` must be a caller-owned object and
/// not a stack local.
///
/// Nothing is written to the sink before the emit phase, so a cancel during the search leaves the
/// sink pristine. A cancel during the emit leaves a headerless prefix the caller deletes.
pub struct NavPlanner {
    phase: PhaseState,
    from: (i32, i32),
    to: (i32, i32),
    /// The route's name, applied by the finishing header patch.
    name: heapless::String<NAME_CAP>,
    /// Resolved into [`mult`](Self::mult) at the first step, and written into the route header.
    bike: BikeType,
    objective: Objective,
    /// Resolved from the reader at the first step; neutral until then.
    mult: ProfileMult,
    /// The snapped endpoints, valid once their phase has run.
    start_id: u32,
    start_c: (i32, i32),
    goal_id: u32,
    goal_c: (i32, i32),
    /// Interior-edge metadata for the two virtual endpoints, plus the expanding lookup's current
    /// best candidate and pass.
    start_edge: Option<NavEdgeSnap>,
    goal_edge: Option<NavEdgeSnap>,
    snap_best: Option<NavEdgeCandidate>,
    snap_ordinal: u8,
    /// Total settles so far, cumulative across [`NAV_EPSILON_LADDER`] rungs.
    settles: u32,
    /// The current [`NAV_EPSILON_LADDER`] rung. It never advances past the last one.
    rung: usize,
    /// Latched once an insert has failed. While set, the search relaxes only tracked nodes. It
    /// tells the two frontier-drain outcomes apart: set is [`NavError::Exhausted`], clear is
    /// [`NavError::NoPath`].
    table_full: bool,
    /// Emit cursors: the staged chain length, the next hop (counting down to 1), the summed path
    /// cost, and the seam vertex shared across hops.
    chain_len: u16,
    hop: u16,
    total_m: u32,
    last: Option<(i32, i32)>,
    /// Created on entering the emit phase, which is where the reserved header is written, and
    /// consumed by the finish. The planner's one large field, about 9 kB.
    em: Option<ObcrEmitter>,
    /// The detour blacklist, `Some` only for [`new_detour`](Self::new_detour) plans. It is read on
    /// every settle, so it lives here rather than in a step frame.
    corridor: Option<Corridor>,
    /// Elevation fill state. It lives here because the fill spans emit steps.
    ele: EleFill,
    map_source: Option<obc_formats::obcr::RouteSourceKey>,
    unresolved_avoidance: bool,
    assistant_candidate: bool,
}

/// Whether any sample has resolved. Sampling continues through coverage gaps.
struct EleFill {
    seen: bool,
}
impl EleFill {
    fn new() -> Self {
        Self { seen: false }
    }
    fn resolve(&mut self, sample: Option<i16>) -> i16 {
        self.seen |= sample.is_some();
        sample.unwrap_or(i16::MIN)
    }
}

impl NavPlanner {
    /// Bind measured graph surfaces to the exact installed map used for this operation.
    pub fn set_attribution_map(&mut self, source: obc_formats::obcr::RouteSourceKey) {
        self.map_source = Some(source);
    }

    pub fn set_objective(&mut self, objective: Objective) {
        self.objective = objective;
    }

    pub fn set_assistant_candidate(&mut self) {
        self.assistant_candidate = true;
    }

    pub fn set_unresolved_avoidance(&mut self) {
        self.unresolved_avoidance = true;
    }

    /// A planner for one route request. It touches nothing yet: the first
    /// [`step`](NavPlanner::step) resets the caller's scratch and tile cache, resolves the profile
    /// and starts snapping.
    pub fn new(from: (i32, i32), to: (i32, i32), name: &str, bike: BikeType) -> Self {
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
            bike,
            mult: ProfileMult::NEUTRAL,
            objective: Objective::Profile,
            start_id: 0,
            start_c: (0, 0),
            goal_id: 0,
            goal_c: (0, 0),
            start_edge: None,
            goal_edge: None,
            snap_best: None,
            snap_ordinal: 0,
            settles: 0,
            rung: 0,
            table_full: false,
            chain_len: 0,
            hop: 0,
            total_m: 0,
            last: None,
            em: None,
            corridor: None,
            ele: EleFill::new(),
            map_source: None,
            unresolved_avoidance: false,
            assistant_candidate: false,
        }
    }

    /// A detour planner: like [`new`](Self::new), but the search skips every edge the `corridor`
    /// blacklists. The exemption discs around the two snapped endpoints are set once the snap
    /// phases resolve.
    pub fn new_detour(from: (i32, i32), to: (i32, i32), name: &str, bike: BikeType, corridor: Corridor) -> Self {
        let mut p = Self::new(from, to, name, bike);
        p.corridor = Some(corridor);
        p
    }

    /// The phase the next step will work on.
    pub fn phase(&self) -> NavPhase {
        match &self.phase {
            PhaseState::SnapFrom | PhaseState::SnapTo => NavPhase::Snap,
            PhaseState::Search => NavPhase::Search,
            PhaseState::Emit | PhaseState::Finish => NavPhase::Emit,
            PhaseState::Terminal(_) => NavPhase::Done,
        }
    }

    pub fn settles(&self) -> u32 {
        self.settles
    }

    /// The graph coordinates the two endpoints snapped to, once the search has run.
    pub fn snapped_start(&self) -> (i32, i32) {
        self.start_c
    }
    pub fn snapped_goal(&self) -> (i32, i32) {
        self.goal_c
    }

    /// The [`NAV_EPSILON_LADDER`] rung the search is on. After a terminal outcome it reads the
    /// rung the plan ended on.
    pub fn epsilon_used(&self) -> (u32, u32) {
        NAV_EPSILON_LADDER[self.rung]
    }

    fn fail(&mut self, e: NavError) -> Step {
        self.phase = PhaseState::Terminal(Err(e));
        Step::Failed(e)
    }

    /// Start a search attempt at the current [`NAV_EPSILON_LADDER`] rung: clear the table, set its
    /// ratio, drop any latched `table_full`, and seed the start node into a fresh frontier. A retry
    /// keeps the snapped endpoints and the warm tile cache, because it walks the same region.
    fn reseed<const N: usize>(&mut self, scratch: &mut NavScratch<N>) -> Result<(), NavError> {
        scratch.reset();
        let (num, den) = NAV_EPSILON_LADDER[self.rung];
        debug_assert!(den != 0, "an ε rung denominator must be non-zero (NavEntry::f divides by it)");
        scratch.eps_num = num as u16;
        scratch.eps_den = den as u16;
        self.table_full = false;
        let (si, _) = scratch.entry(self.start_id, self.start_c.0, self.start_c.1)?;
        scratch.entries[si].h = sat16(ground_dist_m(self.start_c, self.goal_c) as u32);
        scratch.entries[si].came_from = si as u16;
        scratch.heap_push(si);
        Ok(())
    }

    /// Run one bounded unit of planning: one endpoint lookup window, a miss-budgeted burst of
    /// settles, [`NAV_EMIT_HOPS_PER_STEP`] emit hops, or the finishing header patch. Terminal
    /// outcomes are idempotent.
    ///
    /// The five borrows are the caller's per-pass views over the same underlying state each step.
    /// `elev` is read only by the emit phase; a host with no terrain passes
    /// [`NullElevation`](obc_elevation::NullElevation).
    pub fn step<const N: usize>(
        &mut self,
        reader: &Reader,
        scratch: &mut NavScratch<N>,
        tiles: &mut NavTileCache,
        elev: &mut dyn ElevationSource,
        sink: &mut dyn ByteSink,
    ) -> Step {
        match self.phase {
            PhaseState::SnapFrom => {
                // First step of the plan: claim the caller's buffers and resolve the profile.
                if self.snap_ordinal == 0 {
                    scratch.reset();
                    tiles.reset();
                    self.mult = ProfileMult::resolve(reader, self.bike).prefer(self.objective);
                }
                let cap = self.snap_best.map_or(SNAP_RADIUS_M, |best| best.distance_m);
                let lookup_radius = snap_lookup_radius(self.snap_ordinal);
                match snap_window(reader, tiles, self.from, lookup_radius, cap) {
                    Err(()) => return self.fail(NavError::NoPath),
                    Ok(Some(found)) if self.snap_best.is_none_or(|old| snap_candidate_beats(&found, &old)) => {
                        self.snap_best = Some(found);
                    }
                    Ok(_) => {}
                }
                if !snap_lookup_complete(self.snap_best.as_ref(), lookup_radius) {
                    self.snap_ordinal = 1;
                    return Step::Running;
                }
                self.snap_ordinal = 0;
                let Some(candidate) = self.snap_best.take() else {
                    return self.fail(NavError::NoPath);
                };
                let Ok(Some(edge)) = reader.resolve_nav_edge_candidate_cached(candidate, tiles) else {
                    return self.fail(NavError::NoPath);
                };
                let snapped = SnappedEndpoint::from_snap(edge);
                self.start_c = snapped.coord();
                self.start_edge = snapped.edge();
                self.start_id = if self.start_edge.is_some() { VIRTUAL_START_ID } else { snapped.node_id() };
                self.phase = PhaseState::SnapTo;
                Step::Running
            }
            PhaseState::SnapTo => {
                let cap = self.snap_best.map_or(SNAP_RADIUS_M, |best| best.distance_m);
                let lookup_radius = snap_lookup_radius(self.snap_ordinal);
                match snap_window(reader, tiles, self.to, lookup_radius, cap) {
                    Err(()) => return self.fail(NavError::NoPath),
                    Ok(Some(found)) if self.snap_best.is_none_or(|old| snap_candidate_beats(&found, &old)) => {
                        self.snap_best = Some(found);
                    }
                    Ok(_) => {}
                }
                if !snap_lookup_complete(self.snap_best.as_ref(), lookup_radius) {
                    self.snap_ordinal = 1;
                    return Step::Running;
                }
                self.snap_ordinal = 0;
                let Some(candidate) = self.snap_best.take() else {
                    return self.fail(NavError::NoPath);
                };
                let Ok(Some(edge)) = reader.resolve_nav_edge_candidate_cached(candidate, tiles) else {
                    return self.fail(NavError::NoPath);
                };
                let snapped = SnappedEndpoint::from_snap(edge);
                self.goal_c = snapped.coord();
                self.goal_edge = snapped.edge();
                self.goal_id = if self.goal_edge.is_some() { VIRTUAL_GOAL_ID } else { snapped.node_id() };
                // Both endpoints are snapped, so the corridor exemptions can be set.
                if let Some(cor) = self.corridor.as_mut() {
                    cor.set_exempt_nodes(self.start_c, self.goal_c);
                }
                if let Err(e) = self.reseed(scratch) {
                    return self.fail(e);
                }
                self.phase = PhaseState::Search;
                Step::Running
            }
            // The search terminates because a settle either closes a node or strictly lowers an
            // integer `g >= 0`, the frontier is bounded by the table, and no new node enters once
            // the table is full.
            PhaseState::Search => {
                let read_start = tiles.stats().source_reads();
                let mut settled_this_step: u32 = 0;
                loop {
                    let Some(idx) = scratch.heap_pop() else {
                        // A table that filled ran out of room short of the goal; one that never
                        // filled means the goal is disconnected.
                        if self.table_full && self.rung + 1 < NAV_EPSILON_LADDER.len() {
                            self.rung += 1;
                            if let Err(e) = self.reseed(scratch) {
                                return self.fail(e);
                            }
                            return Step::Running;
                        }
                        let e = if self.table_full { NavError::Exhausted } else { NavError::NoPath };
                        return self.fail(e);
                    };
                    if scratch.entries[idx].node_id == self.goal_id {
                        // The goal is reached even if the table filled on the way. The path may
                        // then exceed the rung's bound, which is accepted.
                        return match self.stage_chain(scratch, idx) {
                            Ok(()) => {
                                self.phase = PhaseState::Emit;
                                Step::Running
                            }
                            Err(e) => self.fail(e),
                        };
                    }
                    // An interior start is a virtual node with two partial-edge exits and no
                    // record of its own. When both endpoints lie on one edge, the direct projected
                    // connection is added too, so a short mid-block route takes no junction.
                    if scratch.entries[idx].node_id == VIRTUAL_START_ID {
                        let Some(start) = self.start_edge else {
                            return self.fail(NavError::NoPath);
                        };
                        let raw_a = start.from_a_m;
                        self.table_full |= relax_virtual_edge(
                            scratch,
                            idx,
                            start.a.id,
                            start.a.coord,
                            start.edge_id,
                            raw_a,
                            partial_ascent(start.ascent_ba, raw_a, start.length_m),
                            start.way_kind,
                            self.goal_c,
                            &self.mult,
                        );
                        let raw_b = start.length_m.saturating_sub(start.from_a_m);
                        self.table_full |= relax_virtual_edge(
                            scratch,
                            idx,
                            start.b.id,
                            start.b.coord,
                            start.edge_id,
                            raw_b,
                            partial_ascent(start.ascent_ab, raw_b, start.length_m),
                            start.way_kind,
                            self.goal_c,
                            &self.mult,
                        );
                        if let Some(goal) = self.goal_edge.filter(|goal| goal.edge_id == start.edge_id) {
                            let raw = start.from_a_m.abs_diff(goal.from_a_m);
                            let ascent = if goal.from_a_m >= start.from_a_m {
                                partial_ascent(start.ascent_ab, raw, start.length_m)
                            } else {
                                partial_ascent(start.ascent_ba, raw, start.length_m)
                            };
                            self.table_full |= relax_virtual_edge(
                                scratch,
                                idx,
                                VIRTUAL_GOAL_ID,
                                goal.position.coord,
                                start.edge_id,
                                raw,
                                ascent,
                                start.way_kind,
                                self.goal_c,
                                &self.mult,
                            );
                        }
                        scratch.entries[idx].meta |= META_CLOSED;
                        continue;
                    }
                    self.settles = self.settles.wrapping_add(1);
                    settled_this_step += 1;
                    scratch.entries[idx].meta |= META_CLOSED;
                    // A read failure is the only hard error; a full table latches `table_full`.
                    if let Err(e) = settle::<N>(
                        reader,
                        scratch,
                        tiles,
                        idx,
                        self.goal_c,
                        &self.mult,
                        self.corridor.as_ref(),
                        &mut self.table_full,
                    ) {
                        return self.fail(e);
                    }
                    if let Some(goal) = self.goal_edge {
                        let settled_id = scratch.entries[idx].node_id;
                        let partial = if settled_id == goal.a.id {
                            Some((goal.from_a_m, goal.ascent_ab))
                        } else if settled_id == goal.b.id {
                            Some((goal.length_m.saturating_sub(goal.from_a_m), goal.ascent_ba))
                        } else {
                            None
                        };
                        if let Some((raw, ascent)) = partial {
                            self.table_full |= relax_virtual_edge(
                                scratch,
                                idx,
                                VIRTUAL_GOAL_ID,
                                goal.position.coord,
                                goal.edge_id,
                                raw,
                                partial_ascent(ascent, raw, goal.length_m),
                                goal.way_kind,
                                self.goal_c,
                                &self.mult,
                            );
                        }
                    }
                    // The check trails the settle, so a step always makes at least one node of
                    // progress.
                    if tiles.stats().source_reads() - read_start >= NAV_MISSES_PER_STEP
                        || settled_this_step >= NAV_SETTLES_PER_STEP_CAP
                    {
                        break;
                    }
                }
                Step::Running
            }
            PhaseState::Emit => {
                if self.em.is_none() {
                    match self.arm_emitter(scratch, elev, sink) {
                        Ok(true) => return Step::Running,
                        Ok(false) => {}
                        Err(e) => return self.fail(e),
                    }
                }
                for _ in 0..NAV_EMIT_HOPS_PER_STEP {
                    if self.hop < 1 {
                        break;
                    }
                    if let Err(e) = self.emit_hop(reader, scratch, tiles, elev, sink) {
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

    /// Arm the emitter. Its constructor writes the reserved OBCR header, which is the plan's first
    /// sink write. Returns `Ok(true)` when the single-point route was emitted whole and the phase
    /// advanced to Finish.
    ///
    /// Must stay `#[inline(never)]`: the ~9 kB emitter construction temporary has to live in this
    /// immediately-popped frame. Inlined, it holds a slot in the step frame for every step and
    /// overflows the device stack.
    #[inline(never)]
    fn arm_emitter<const N: usize>(
        &mut self,
        scratch: &NavScratch<N>,
        elev: &mut dyn ElevationSource,
        sink: &mut dyn ByteSink,
    ) -> Result<bool, NavError> {
        let mut em = ObcrEmitter::new(sink).map_err(|_| NavError::NoPath)?;
        em.set_attribution_map(self.map_source);
        em.set_bike_type(self.bike);
        em.set_flags(
            if self.assistant_candidate { obc_formats::obcr::FLAG_ASSISTANT_CANDIDATE } else { 0 }
                | if self.unresolved_avoidance { obc_formats::obcr::FLAG_UNRESOLVED_AVOIDANCE } else { 0 },
        );
        self.em = Some(em);
        if self.chain_len == 1 {
            let e = &scratch.entries[scratch.heap[0] as usize];
            let (lon, lat) = (e.lon, e.lat);
            // One point needs no densification, only its height.
            let ele = self.ele.resolve(elev.sample(lat, lon));
            if self.em.as_mut().is_none_or(|em| em.push(sink, lon, lat, ele).is_err()) {
                return Err(NavError::NoPath);
            }
            self.phase = PhaseState::Finish;
            return Ok(true);
        }
        Ok(false)
    }

    /// Finish the emitter in place and patch the header, the plan's last writes.
    ///
    /// Must stay `#[inline(never)]`: the final chunk and index write scratch stays out of the
    /// planner step frame.
    #[inline(never)]
    fn finish_emit(&mut self, sink: &mut dyn ByteSink) -> Result<RouteStats, NavError> {
        let Some(em) = self.em.as_mut() else {
            return Err(NavError::NoPath);
        };
        em.finish(sink, &self.name, &mut Vec::<WpPlace, MAX_WAYPOINTS>::new()).map_err(|_| NavError::NoPath)
    }

    /// Stage the found path, goal to start, in the now-dead heap array. The path length is bounded
    /// by the tracked-node count, so it always fits. The goal's `g` is the weighted cost, so it is
    /// not the header total; [`emit_hop`](Self::emit_hop) sums the raw `length_m` instead.
    ///
    /// Must stay `#[inline(never)]`: it keeps the step dispatcher frame thin.
    #[inline(never)]
    fn stage_chain<const N: usize>(&mut self, scratch: &mut NavScratch<N>, goal_idx: usize) -> Result<(), NavError> {
        let mut chain_len = 0usize;
        let mut cur = goal_idx;
        loop {
            if chain_len >= scratch.used as usize {
                return Err(NavError::NoPath); // longer than the tracked set: a corrupt cycle
            }
            scratch.heap[chain_len] = cur as u16;
            chain_len += 1;
            if scratch.entries[cur].node_id == self.start_id {
                break;
            }
            cur = scratch.entries[cur].came_from as usize;
            if cur >= N {
                return Err(NavError::NoPath); // corrupt slot index: fail rather than index out of bounds
            }
        }
        self.chain_len = chain_len as u16;
        self.hop = chain_len as u16 - 1;
        self.last = None;
        Ok(())
    }

    /// Emit one path hop: fetch the hop's edge polyline, oriented, and push it. The seam vertex
    /// shared with the previous hop is dropped, so the OBCR carries one continuous polyline. The
    /// edge's raw ground `length_m` accumulates into `total_m`, the unweighted displayed distance.
    ///
    /// Must stay `#[inline(never)]`: this is a phase-boundary frame and must not join the step
    /// frame.
    #[inline(never)]
    fn emit_hop<const N: usize>(
        &mut self,
        reader: &Reader,
        scratch: &mut NavScratch<N>,
        tiles: &mut NavTileCache,
        elev: &mut dyn ElevationSource,
        sink: &mut dyn ByteSink,
    ) -> Result<(), NavError> {
        let hop = self.hop as usize;
        let prev = &scratch.entries[scratch.heap[hop] as usize];
        let cur = &scratch.entries[scratch.heap[hop - 1] as usize];
        let partial = prev.node_id == VIRTUAL_START_ID || cur.node_id == VIRTUAL_GOAL_ID;
        let positions = if partial {
            Some((
                self.edge_position(cur.edge_used, prev.node_id).ok_or(NavError::NoPath)?,
                self.edge_position(cur.edge_used, cur.node_id).ok_or(NavError::NoPath)?,
            ))
        } else {
            None
        };
        let em = self.em.as_mut().ok_or(NavError::NoPath)?;
        let (surface, elevation_complete) = reader.nav_edge_facts(cur.edge_used).ok_or(NavError::NoPath)?;
        em.set_surface(surface);
        em.set_elevation_incomplete(!elevation_complete);
        let mut last = self.last;
        let mut werr = false;
        let ele = &mut self.ele;
        let mut push = |pt| {
            if werr || last == Some(pt) {
                return; // the previous hop already emitted this seam vertex
            }
            if fill_segment(em, sink, elev, ele, last, pt).is_err() {
                werr = true;
                return;
            }
            last = Some(pt);
        };
        let length_m = if let Some((from, to)) = positions {
            reader.nav_edge_slice_oriented(tiles, cur.edge_used, from.0, to.0, &mut push).ok_or(NavError::NoPath)?;
            from.1.abs_diff(to.1)
        } else {
            reader.nav_edge_oriented(tiles, cur.edge_used, (prev.lon, prev.lat), &mut push).ok_or(NavError::NoPath)?
        };
        self.last = last;
        if werr {
            return Err(NavError::NoPath);
        }
        // Real ground meters for the displayed total.
        self.total_m = self.total_m.saturating_add(length_m);
        Ok(())
    }

    /// Resolve a real or virtual entry to its position and raw offset on `edge_id`.
    fn edge_position(&self, edge_id: u32, node_id: u32) -> Option<(NavEdgePosition, u32)> {
        if node_id == VIRTUAL_START_ID {
            let edge = self.start_edge.filter(|edge| edge.edge_id == edge_id)?;
            return Some((edge.position, edge.from_a_m));
        }
        if node_id == VIRTUAL_GOAL_ID {
            let edge = self.goal_edge.filter(|edge| edge.edge_id == edge_id)?;
            return Some((edge.position, edge.from_a_m));
        }
        for edge in [self.start_edge, self.goal_edge].into_iter().flatten() {
            if edge.edge_id != edge_id {
                continue;
            }
            if node_id == edge.a.id {
                return Some((edge.a.position, 0));
            }
            if node_id == edge.b.id {
                return Some((edge.b.position, edge.length_m));
            }
        }
        None
    }
}

/// Emit one geometry segment with its elevation. Only where terrain resolves does it insert
/// interpolated points, so no two emitted points are more than [`ELE_SAMPLE_STEP_M`] of ground
/// apart. With a null source no point is inserted and every height is 0.
///
/// Must stay `#[inline(never)]`: it keeps the emitter's `push` call tree out of
/// [`NavPlanner::emit_hop`]'s frame, which already carries the polyline closure.
#[inline(never)]
fn fill_segment(
    em: &mut ObcrEmitter,
    sink: &mut dyn ByteSink,
    elev: &mut dyn ElevationSource,
    ele: &mut EleFill,
    from: Option<(i32, i32)>,
    to: (i32, i32),
) -> Result<(), Error> {
    // Nav coordinates are `(lon, lat)`; the sampler takes `(lat, lon)`.
    let sample = elev.sample(to.1, to.0);
    // The first height that resolves is where the decimator gets something to preserve. It is
    // latched here, not up front, so a null source never touches the decimator.
    if sample.is_some() && !ele.seen {
        em.keep_elevation_detail(ELE_KEEP_M);
    }
    // "or any earlier sample" matters at a coverage edge: the segment that leaves the raster
    // still has its far half on real ground.
    if let (Some(prev), true) = (from, sample.is_some() || ele.seen) {
        let steps = densify_steps(ground_dist_m(prev, to));
        for k in 1..steps {
            let mid = lerp_udeg(prev, to, k, steps);
            let h = ele.resolve(elev.sample(mid.1, mid.0));
            em.push(sink, mid.0, mid.1, h)?;
        }
    }
    let h = ele.resolve(sample);
    em.push(sink, to.0, to.1, h)
}

/// How many equal pieces a segment is split into to keep every emitted step at or under
/// [`ELE_SAMPLE_STEP_M`]. Capped at [`ELE_MAX_DENSIFY_STEPS`].
fn densify_steps(dist_m: f32) -> u32 {
    // `is_none_or` states the NaN case: a length that is not a number densifies nothing.
    if dist_m.partial_cmp(&ELE_SAMPLE_STEP_M).is_none_or(|o| o != core::cmp::Ordering::Greater) {
        return 1;
    }
    (libm::ceilf(dist_m / ELE_SAMPLE_STEP_M) as u32).clamp(1, ELE_MAX_DENSIFY_STEPS)
}

/// The point `k/den` of the way from `a` to `b`, interpolated in microdegrees. Integer-only, so it
/// is deterministic across hosts; the truncation is at most 1 microdegree, about 11 cm.
fn lerp_udeg(a: (i32, i32), b: (i32, i32), k: u32, den: u32) -> (i32, i32) {
    let f = |s: i32, e: i32| {
        let d = i64::from(e) - i64::from(s);
        (i64::from(s) + d * i64::from(k) / i64::from(den)) as i32
    };
    (f(a.0, b.0), f(a.1, b.1))
}

/// One-shot convenience over [`NavPlanner`]: loop [`step`](NavPlanner::step) to completion. Tests
/// and the headless sim use it; interactive hosts step the planner themselves.
// The arguments are the plan request plus the caller-owned buffers the planner never allocates.
#[allow(clippy::too_many_arguments)]
pub fn plan_route<const N: usize>(
    reader: &Reader,
    from: (i32, i32),
    to: (i32, i32),
    name: &str,
    bike: BikeType,
    scratch: &mut NavScratch<N>,
    tiles: &mut NavTileCache,
    elev: &mut dyn ElevationSource,
    sink: &mut dyn ByteSink,
) -> Result<RouteStats, NavError> {
    let mut planner = NavPlanner::new(from, to, name, bike);
    loop {
        match planner.step(reader, scratch, tiles, elev, sink) {
            Step::Running => {}
            Step::Done(stats) => return Ok(stats),
            Step::Failed(e) => return Err(e),
        }
    }
}

/// The detour twin of [`plan_route`].
#[allow(clippy::too_many_arguments)]
pub fn plan_detour<const N: usize>(
    reader: &Reader,
    from: (i32, i32),
    to: (i32, i32),
    name: &str,
    bike: BikeType,
    corridor: Corridor,
    scratch: &mut NavScratch<N>,
    tiles: &mut NavTileCache,
    elev: &mut dyn ElevationSource,
    sink: &mut dyn ByteSink,
) -> Result<RouteStats, NavError> {
    let mut planner = NavPlanner::new_detour(from, to, name, bike, corridor);
    loop {
        match planner.step(reader, scratch, tiles, elev, sink) {
            Step::Running => {}
            Step::Done(stats) => return Ok(stats),
            Step::Failed(e) => return Err(e),
        }
    }
}

#[inline]
fn partial_ascent(total: u16, partial_m: u32, length_m: u32) -> u16 {
    let rounded = u64::from(total) * u64::from(partial_m) + u64::from(length_m / 2);
    rounded.checked_div(u64::from(length_m)).unwrap_or(0).min(u64::from(u16::MAX)) as u16
}

/// Relax one synthetic partial-edge adjacency used by an exact projected start or goal.
#[allow(clippy::too_many_arguments)]
fn relax_virtual_edge<const N: usize>(
    scratch: &mut NavScratch<N>,
    from: usize,
    target_id: u32,
    target_coord: (i32, i32),
    edge_id: u32,
    raw_cost_m: u32,
    ascent_m: u16,
    way_kind: u8,
    goal_c: (i32, i32),
    mult: &ProfileMult,
) -> bool {
    let Some(weighted) = mult.edge_cost(raw_cost_m, ascent_m, way_kind) else { return false };
    let tentative = sat16((scratch.entries[from].g as u32).saturating_add(weighted));
    match scratch.entry(target_id, target_coord.0, target_coord.1) {
        Ok((j, false)) => {
            if tentative < scratch.entries[j].g {
                let entry = &mut scratch.entries[j];
                entry.g = tentative;
                entry.came_from = from as u16;
                entry.edge_used = edge_id;
                if entry.heap_pos() == HEAP_NONE {
                    entry.meta &= !META_CLOSED;
                    scratch.heap_push(j);
                } else {
                    let pos = scratch.entries[j].heap_pos() as usize;
                    scratch.sift_up(pos);
                }
            }
            false
        }
        Ok((j, true)) => {
            let entry = &mut scratch.entries[j];
            entry.g = tentative;
            entry.h = sat16(ground_dist_m(target_coord, goal_c) as u32);
            entry.came_from = from as u16;
            entry.edge_used = edge_id;
            scratch.heap_push(j);
            false
        }
        Err(_) => true,
    }
}

/// One settle: descend the node quadtree to the settled node's leaf and relax each neighbor
/// through the plan's profile. A node the walk does not yield, which means a corrupt map, relaxes
/// nothing and the search continues on the rest of the frontier.
///
/// A full scratch drops the new discovery and latches `*table_full`; decrease-key of a tracked
/// neighbor still relaxes. The only hard error is a read failure.
///
/// Must stay `#[inline(never)]`: this is a phase-boundary frame and must not join the step frame.
#[allow(clippy::too_many_arguments)]
#[inline(never)]
fn settle<const N: usize>(
    reader: &Reader,
    scratch: &mut NavScratch<N>,
    tiles: &mut NavTileCache,
    idx: usize,
    goal_c: (i32, i32),
    mult: &ProfileMult,
    corridor: Option<&Corridor>,
    table_full: &mut bool,
) -> Result<(), NavError> {
    let settled = scratch.entries[idx];
    let view = BBox { min_lon: settled.lon, min_lat: settled.lat, max_lon: settled.lon, max_lat: settled.lat };
    reader
        .for_each_nav_node_cached(&view, tiles, |n| {
            // A node can be yielded more than once when two leaves share a chunk, so only the
            // settled node's own record relaxes anything.
            if n.id != settled.node_id {
                return;
            }
            for nb in n.neighbors() {
                // A forbidden class is skipped, never relaxed, so the graph stays whole for the
                // other profiles.
                let Some(weighted) = mult.edge_cost(nb.cost_m, nb.ascent_m, nb.way_kind) else {
                    continue;
                };
                // A blacklisted edge is skipped exactly like a forbidden class.
                if corridor.is_some_and(|c| c.blocks((settled.lon, settled.lat), (nb.lon, nb.lat))) {
                    continue;
                }
                let tentative = sat16((settled.g as u32).saturating_add(weighted));
                match scratch.entry(nb.id, nb.lon, nb.lat) {
                    Ok((j, false)) => {
                        if tentative < scratch.entries[j].g {
                            let e = &mut scratch.entries[j];
                            e.g = tentative;
                            e.came_from = idx as u16;
                            e.edge_used = nb.edge_id;
                            if e.heap_pos() == HEAP_NONE {
                                // The inflated `h` makes a better-`g` rediscovery of a closed
                                // node routine, so it re-opens. The bound still holds.
                                e.meta &= !META_CLOSED;
                                scratch.heap_push(j);
                            } else {
                                let pos = scratch.entries[j].heap_pos() as usize;
                                scratch.sift_up(pos);
                            }
                        }
                    }
                    // A new node is inserted only while the table has room. No new node once it
                    // is full is what makes the frontier drain.
                    Ok((j, true)) => {
                        let e = &mut scratch.entries[j];
                        e.g = tentative;
                        e.h = sat16(ground_dist_m((nb.lon, nb.lat), goal_c) as u32);
                        e.came_from = idx as u16;
                        e.edge_used = nb.edge_id;
                        scratch.heap_push(j);
                    }
                    Err(_) => *table_full = true,
                }
            }
        })
        .map_err(|_| NavError::NoPath)?;
    Ok(())
}

/// Scan one lookup square and project every candidate edge geometry. The caller tries
/// [`SNAP_INITIAL_LOOKUP_RADIUS_M`] first and follows with the complete square only when the
/// triangle bound cannot prove that winner final.
#[inline(never)]
fn snap_window(
    reader: &Reader,
    tiles: &mut NavTileCache,
    p: (i32, i32),
    lookup_radius_m: f32,
    cap: f32,
) -> Result<Option<NavEdgeCandidate>, ()> {
    if reader.nav_directory().is_empty() {
        return Err(());
    }
    let cl = cos_lat(p.1).max(1e-3);
    let full_half = libm::ceilf(lookup_radius_m / M_PER_DEG as f32 * 1e6) as i32;
    let lon_half = libm::ceilf(full_half as f32 / cl) as i32;
    let view = BBox {
        min_lon: p.0.saturating_sub(lon_half),
        min_lat: p.1.saturating_sub(full_half),
        max_lon: p.0.saturating_add(lon_half),
        max_lat: p.1.saturating_add(full_half),
    };
    reader.nearest_nav_edge_candidate_cached(&view, tiles, p, cap).map_err(|_| ())
}

#[inline]
fn snap_lookup_radius(pass: u8) -> f32 {
    if pass == 0 {
        SNAP_INITIAL_LOOKUP_RADIUS_M
    } else {
        SNAP_LOOKUP_RADIUS_M
    }
}

#[inline]
fn snap_lookup_complete(best: Option<&NavEdgeCandidate>, lookup_radius_m: f32) -> bool {
    lookup_radius_m >= SNAP_LOOKUP_RADIUS_M
        || best.is_some_and(|candidate| candidate.distance_m + SNAP_INDEX_REACH_M <= lookup_radius_m)
}

fn snap_candidate_beats(new: &NavEdgeCandidate, old: &NavEdgeCandidate) -> bool {
    new.distance_m < old.distance_m || (new.distance_m == old.distance_m && new.edge_id < old.edge_id)
}

#[cfg(test)]
mod tests {
    use super::{sat16, ProfileMult};

    #[test]
    fn objectives_keep_bans_suitability_and_distance_lower_bound() {
        use super::Objective;
        let mut profile = ProfileMult { highway: [24; 32], surface: [32; 8], climb: 12 };
        profile.highway[4] = 0;
        profile.surface[7] = 0;
        for objective in Objective::TRIALS {
            let p = profile.prefer(objective);
            assert_eq!(p.highway, profile.highway);
            assert_eq!(p.surface[..3], profile.surface[..3]);
            assert_eq!(p.edge_cost(100, 100, 4), None);
            assert_eq!(p.edge_cost(100, 100, 7 << 5), None);
            for surface in 0..7 {
                assert!(p.edge_cost(100, 0, surface << 5).unwrap() >= 100);
                assert!(p.surface[surface as usize] >= profile.surface[surface as usize]);
            }
        }
        assert_eq!(profile.prefer(Objective::LessClimb).climb, 24);
        assert_eq!(profile.prefer(Objective::LeastClimb).climb, 48);
        assert_eq!(profile.prefer(Objective::Smoother).surface[3], 64);
        assert_eq!(profile.prefer(Objective::Smoothest).surface[3], 128);
        for (objective, weight) in [(Objective::Shorter, 6), (Objective::Shortest, 0)] {
            let p = profile.prefer(objective);
            assert_eq!(p.climb, weight);
            assert_eq!(p.surface, profile.surface);
        }
    }

    /// The exact sum is asserted, not just the absence of a panic, because the claim is that the
    /// worst legal value fits.
    #[test]
    fn edge_cost_at_the_wire_maxima_is_exact_and_nowhere_near_wrapping() {
        let p = ProfileMult { highway: [u8::MAX; 32], surface: [u8::MAX; 8], climb: u32::from(u8::MAX) };
        let got = p.edge_cost(u32::from(u16::MAX), u16::MAX, 0xFF).expect("255 is not forbidden");
        assert_eq!(got, 16_645_890 + 16_711_425);
        assert!(got < u32::MAX / 64, "the worst legal edge must stay far inside u32");
        assert_eq!(sat16(u32::from(u16::MAX).saturating_add(got)), u16::MAX);
    }

    #[test]
    fn either_zero_reproduces_the_pre_terrain_cost() {
        let blind = ProfileMult { highway: [16; 32], surface: [16; 8], climb: 0 };
        let weighted = ProfileMult { climb: 10, ..blind };
        assert_eq!(blind.edge_cost(1_000, 400, 0), Some(1_000), "climb-blind ignores a 400 m climb");
        assert_eq!(weighted.edge_cost(1_000, 0, 0), Some(1_000), "a flat edge costs its ground length");
        assert_eq!(weighted.edge_cost(1_000, 400, 0), Some(5_000), "…and a climbing one is charged for it");
    }

    #[test]
    fn a_forbidden_class_stays_forbidden_under_any_climb() {
        let mut p = ProfileMult { highway: [16; 32], surface: [16; 8], climb: 255 };
        p.highway[4] = 0;
        assert_eq!(p.edge_cost(1_000, 0, 4), None);
        assert_eq!(p.edge_cost(1_000, u16::MAX, 4), None);
    }
}
