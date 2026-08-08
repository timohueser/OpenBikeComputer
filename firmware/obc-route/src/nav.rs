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
//! Consecutive settles scatter among a bounded set of active leaves; the [`NavTileCache`]
//! keeps that working set resident and turns most per-settle re-reads into slot hits (the device
//! is SD-bound — the cache's hit/miss counters are the number R4 logs on-glass).
//!
//! **The scratch is fixed** — [`NAV_MAX_NODES`] tracked nodes, **one size on every
//! target**, host, sim and device alike, which is the point: the sim's plannable
//! range *is* the device's — because the router must coexist with the map cache and
//! the render scratch in RAM.
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
//! **Profile-weighted, climb-aware edges** (epic #533 N3; the climb term is EL6, epic #1068). Every
//! edge is relaxed through the §8.6 formula, verbatim:
//!
//! ```text
//! weighted = (cost_m × effective(way_kind)) >> 4  +  ascent_m × climb_weight     # saturating
//! ```
//!
//! `effective` is the selected **bike profile's** multiplier for the edge's §8.3 `way_kind`, from a
//! 40-byte per-plan lookup (32 highway × 8 surface `u8` 1/16 bytes, combined at lookup);
//! `climb_weight` is the same profile's flat-metres-per-metre-of-ascent byte. Both are resolved
//! **once per plan** from the map's §8.6 profile table into [`ProfileMult`]. An out-of-range profile
//! index falls back to profile 0 (a stale device setting must never brick routing — never an error).
//! A **forbidden** class (`effective == 0`) is simply not relaxed: the neighbor is skipped, so the
//! graph stays whole for the other profiles and an endpoint whose only escapes are forbidden drains
//! the frontier to an honest [`NavError::NoPath`]. The *displayed* distance is unweighted — the
//! planner sums each hop's raw edge `length_m` at emit, not the weighted `g`.
//!
//! `ascent_m` is the §8.3 neighbor entry's own **directional** integrated climb (v12) — it is read
//! off the adjacency record already in hand, which is why EL5 put it *there* and not in the §8.4
//! edge pool: relaxation still costs no second fetch. A map packed without terrain carries `0`
//! everywhere and a `climb_weight` of `0` is a legal, meaningful profile, so both zeroes reproduce
//! v11's costing exactly — the null path is provably today's router (pinned in `tests/nav.rs`).
//!
//! **Admissibility with elevation, restated where the code lives** (§8.6, normative): *`Ascent M`
//! and `Climb Weight` are both unsigned and the term is added, so a descent MUST NOT reduce an
//! edge's cost below its profile-weighted ground length.* That is the whole reason the climb term is
//! shaped as it is. `h` remains the great-circle distance to the goal in the same
//! local-equirectangular metric the packer summed every `cost_m` in, nothing anywhere subtracts from
//! a cost, and every non-forbidden multiplier is ≥ 1.0× — so `weighted ≥ ground length ≥
//! great-circle` still holds edge for edge and `h` stays a lower bound unchanged. Descent credits
//! are therefore **banned**, not merely unimplemented (epic #1068's out-of-scope list): an edge
//! cheaper than its straight-line distance would break the bound the whole ε-ladder rests on.
//! Gradient effects on *time* belong in ETA (EL9), never in the A\* cost.
//!
//! **Bounded suboptimality, not exactness** (decided 2026-07-06 — range in fixed
//! memory): the priority is `f = g + ε·h`, ε an inflation from the [`NAV_EPSILON_LADDER`].
//! The heuristic itself never overestimates the *weighted* cost — it is the great-circle
//! distance to the goal in the *same* local-equirectangular metric the packer summed for
//! every edge's `cost_m`, every non-forbidden profile multiplier is **≥ 1.0**
//! (packer-enforced + reader-clamped), and the climb term only ever *adds*, so
//! `weighted cost ≥ ground length ≥ great-circle`: `h` stays admissible unchanged. Weighted A\*
//! therefore returns a path of cost **≤ ε× the cheapest *climb-aware* route under the selected
//! profile** (that reading is the only thing EL6 changed about this bound — the ε numbers and every
//! rung's logic are untouched), where ε is **the rung the search
//! succeeded on** — 1.3× for the first-try success every route used before N8 (a few
//! percent on road networks in practice), and 2.0× or 3.0× only for a route that *would
//! otherwise have failed* the tight bound (see the ε-escalation note under [`NAV_EPSILON_LADDER`]).
//! What ε buys is *reach*: plain A\* explores roughly quadratically with distance and
//! exhausted the fixed table ~1.5 km out; the goal-greedy inflated search settles a narrow
//! corridor instead, planning multi-km routes in the same table — and on exhaustion the
//! ladder climbs to a greedier rung to reach farther still, on the same fixed memory.
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
//! the fixed table is the real limit and the attempt is the feedback — a
//! [`NavError::Exhausted`] that has climbed the whole [`NAV_EPSILON_LADDER`] **is** the
//! "too far for this device" answer. Two accepted consequences: (1) `h` (`u16` meters,
//! saturating) saturates for very distant goals, so ordering degrades gracefully toward
//! uniform expansion and the search exhausts — exactly the intended outcome; (2) a
//! hopeless target now burns **up to three** full exhaustion searches (one per rung)
//! before failing — each bounded by the table and spread across bounded steps, so the
//! stepping host's loop keeps feeding its watchdog (and rendering, and taking input —
//! including the cancel) between them exactly as with one search. That is the ladder's
//! accepted trade: same watchdog-safe, cancellable posture, a larger constant (≤ 3×) on
//! the truly-hopeless path, bought so a *reachable-but-far* target that the tight bound
//! couldn't fit now succeeds. A step-count budget in the stepping host is the named
//! future lever if the wait annoys.
//!
//! **Emit-time elevation fill** (EL7, epic #1068): every [`step`](NavPlanner::step) takes an
//! [`ElevationSource`] and the emit phase samples it at each emitted vertex, so a planned route's
//! OBCR carries real heights and its header the real min/max/ascent/descent. Nothing downstream
//! changed to make that work — the Climb screen, the elevation profile, the ride stats and the GPX
//! export have always read those fields; they were simply zero for a device-planned route. Three
//! rules govern the fill:
//!
//! - **Densify to [`ELE_SAMPLE_STEP_M`] when there is terrain.** A nav edge's polyline is OSM way
//!   geometry: it carries a vertex where the *road* bends, which on a straight alpine ramp can be
//!   kilometres away (up to the packer's 30 000 µdeg ≈ 3.3 km densification bound). Sampling only
//!   at those vertices would run a chord straight through a crest. Intermediate points are
//!   interpolated linearly in µdeg and sampled like any other vertex — they are ordinary OBCR
//!   points, and the emitter's decimator is free to drop the ones that carry neither shape nor
//!   height (see [`ObcrEmitter::keep_elevation_detail`](crate::convert::ObcrEmitter)).
//! - **A hole carries the last known height forward.** [`ElevationSource::sample`] answers `None`
//!   for a coverage edge, a `NODATA` corner or no terrain file at all; OBCR has no per-point
//!   "unknown" encoding, so the fill repeats the last resolved height — a flat segment across the
//!   gap, honest enough, and one that books no phantom climb through the dead-band. A hole
//!   **before the first resolved sample** has nothing to carry, so the integrator does not run at
//!   all until coverage begins ([`EleFill::resolve`]): pushing the `0` placeholder would anchor the
//!   band at sea level and book the whole first real height as ascent. If **no** sample ever
//!   resolves, the header stats stay zeroed exactly as they were before EL7.
//! - **The null source is bit-for-bit the old behaviour.** With
//!   [`NullElevation`](obc_elevation::NullElevation) nothing densifies, every stored height is 0
//!   and every stat is 0, so the emitted OBCR is byte-identical to the pre-EL7 one (pinned in
//!   `tests/nav.rs`). That is the property that makes the terrain file removable.
//!
//! The totals go through the same [`DeadBand`] at the same [`ELE_DEADBAND_M`] threshold the GPX
//! converter ([`crate::convert`]) runs over an imported track, so a route planned on the device and
//! the same route exported to GPX and re-imported agree on their climb.
//!
//! **The one boundary on that parity**, stated rather than hidden: a route whose *opening* points
//! fall outside terrain coverage still **stores** height `0` for them, because OBCR has no
//! "unknown" encoding. The route's own stats are right — the integrator ignored those points — but
//! an export of it re-imports as a `0 → first-real-height` step, which the converter's dead-band
//! *will* book. Parity therefore holds for a route lying wholly inside coverage, which is every
//! route on a map whose terrain was baked for it; the honest fix for the exception is a terrain
//! file that covers the map's graph, never a fabricated height.

use heapless::Vec;

use crate::convert::{EmitStats, ObcrEmitter, RouteStats, WpPlace};
use crate::corridor::Corridor;
use crate::reader::MAX_WAYPOINTS;
use obc_elevation::{DeadBand, ElevationSource, ELE_DEADBAND_M};
use obc_formats::io::{ByteSink, Error};
use obc_formats::obcr::NAME_CAP;
use obc_map_scene::{cos_lat, ground_dist_m};
use obc_map_scene::{BBox, M_PER_DEG};
use obc_reader::{NavEdgeCandidate, NavEdgePosition, NavEdgeSnap, NavTileCache, Reader};

/// Maximum accepted distance from the requested position to the winning full road polyline.
/// The wider [`SNAP_LOOKUP_RADIUS_M`] only discovers candidate edge ids; it never weakens this
/// actual point-to-road limit or quantizes the returned projection.
pub(crate) const SNAP_RADIUS_M: f32 = 100.0;

/// Maximum ground distance from any point on an indexed edge to its nearest endpoint/interior
/// anchor. The mathematical bound is 150 m; one metre covers microdegree anchor rounding.
const SNAP_INDEX_REACH_M: f32 = 151.0;
/// Node/anchor discovery radius. A road point is within [`SNAP_INDEX_REACH_M`] of a lookup record;
/// adding [`SNAP_RADIUS_M`] gives a complete ≈250 m search (the extra metre is rounding slack).
const SNAP_LOOKUP_RADIUS_M: f32 = SNAP_INDEX_REACH_M + SNAP_RADIUS_M;
/// The usual case searches less area once: if its winning road is within 49 m, the triangle bound
/// proves no record outside this window can name a closer edge. Otherwise one full pass follows.
const SNAP_INITIAL_LOOKUP_RADIUS_M: f32 = 200.0;

/// Reserved ids for exact projected endpoints. Packed node ids are dense from zero and cannot use
/// the top two values.
const VIRTUAL_START_ID: u32 = u32::MAX;
const VIRTUAL_GOAL_ID: u32 = u32::MAX - 1;

/// Largest ground gap (m) between two emitted OBCR points **while terrain is available** (EL7):
/// a longer edge segment is split with linearly interpolated points, each sampled like a real
/// vertex.
///
/// 250 m is chosen against the raster, not the road: v1 terrain is a 512 µdeg posting (≈ 40 m
/// north-south), so a 250 m step still lands ~6 postings apart — it cannot invent detail the DEM
/// does not have, and it cannot miss a col or a crest by more than a quarter of the shallowest
/// interesting climb. It is also cheap: at ≈ 4 samples/km a 100 km route asks the tile cache for
/// ~400 samples, and with terrain cells covering ~55 km of latitude those samples walk the raster
/// in order, so the resident 4-tile cache serves nearly all of them.
pub(crate) const ELE_SAMPLE_STEP_M: f32 = 250.0;

/// Hard cap on interpolated points inserted into one edge segment — a guard, not a tuning knob.
/// The packer's own 30 000 µdeg bound puts a real segment at ≤ 3.3 km (≈ 14 steps); anything that
/// asks for more than this is corrupt geometry, and the fill would rather emit a coarse line than
/// loop on it.
const ELE_MAX_DENSIFY_STEPS: u32 = 64;

/// The height move (m) that forces the emitter to keep a vertex once terrain is filling the route
/// (EL7). It is [`ELE_DEADBAND_M`] deliberately: the stored points are what a GPX export writes and
/// what a re-import integrates, so keeping every vertex the dead-band would *book* is exactly what
/// makes the exported route's climb agree with the header's. A geometric decimator alone would drop
/// a crest that sits on a straight road.
const ELE_KEEP_M: i16 = ELE_DEADBAND_M as i16;

/// The **ε-escalation ladder** (N8, epic #533): weighted-A\* heuristic inflation ε as a sequence
/// of integer `(num, den)` ratios — `f = g + (num·h)/den`. The search starts at rung 0 (1.3×, the
/// tight bound every route used before N8) and, **only on [`NavError::Exhausted`]** (the table
/// filled then the frontier drained — see [`NavPlanner`]), retries greedier: 2.0×, then 3.0×.
/// Raising ε shifts the frontier ordering toward the goal, so the same fixed table settles a
/// narrower corridor and reaches farther — trading the optimality bound for range, but **only after
/// the tight bound has already failed** (a route that plans at 1.3× never escalates, so nothing that
/// succeeds today gets worse). [`NavError::NoPath`] (disconnected — the frontier drained without the
/// table ever filling) never escalates: retrying can't connect an island and would triple the
/// failure latency. The found path is bounded ≤ (the successful rung's ε) × the profile-optimal cost
/// (the heuristic stays admissible — only the inflation grows; see the module-doc bounded-
/// suboptimality note).
pub const NAV_EPSILON_LADDER: [(u32, u32); 3] = [(13, 10), (2, 1), (3, 1)];

/// Search-phase step budget, **by logical source fills** (graph chunks plus route-private quadtree
/// index windows): a [`NavPlanner::step`] settles until it has incurred this many reads, then returns.
/// Sector-aligned v12 maps turn each 512-byte fill into one physical command; old unaligned maps may
/// need two. Budgeting by reads (no clock, no new dependency) makes a step's wall time roughly
/// constant whatever the cache hit rate, instead of the old fixed
/// "8 settles per step" which paced by *work attempted* and so ran a warm step (mostly hits) far
/// under the SD envelope while still charging a full pass. The enlarged route working set turns
/// more settles into hits, which is where the board's per-plan wall-time floor (`LOOP_MS` × steps)
/// comes down.
///
/// Twelve aligned fills preserve the former worst-case physical envelope of six unaligned graph
/// misses (twelve CMD17s), while halving the 8 ms scheduler floor on the newly-resident routes.
pub const NAV_MISSES_PER_STEP: u32 = 12;

/// Hard settle cap per search [`NavPlanner::step`] (N4): even a **fully warm** step (every settle a
/// cache hit ⇒ [`NAV_MISSES_PER_STEP`] never reached) returns after this many settles, so a step's
/// worst-case pass time — pure in-RAM heap + record work, no SD — stays bounded and the host's
/// render/input/watchdog cadence is never starved by one runaway step. 8× the old fixed budget: a
/// warm step now does up to this much useful search where the pre-N4 code stopped at 8.
pub const NAV_SETTLES_PER_STEP_CAP: u32 = 64;

/// Emit-phase step budget: path hops (edge-geometry fetches + OBCR pushes) per
/// [`NavPlanner::step`] — the emit is short next to the search, so a few hops per
/// step finishes it in a handful of passes without one long blocking tail.
/// Eight aligned edge chunks likewise preserve the former four-unaligned-hop command envelope.
pub(crate) const NAV_EMIT_HOPS_PER_STEP: u16 = 8;

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
/// **weighted** cost (`(cost_m × mult) >> 4 + ascent_m × climb_weight`, N3 + EL6), so it saturates
/// *earlier* than plain distance — a 4× multiplier ⇒ ~16 km of that class fills the field, and the
/// climb term charges a further 10 m per metre climbed at the stock Road weight — but this is the
/// same graceful degradation: the fixed table exhausts long before saturation matters on
/// real terrain (profiles are capped ≤ ~8×), and with no distance cap a very distant goal
/// still degrades the ordering toward uniform expansion until the table exhausts (see the
/// module doc's no-cap note). The emitted route's displayed length is the unweighted
/// `length_m` sum, immune to this.
/// `came_from` as a slot index (slots never move — open addressing, no deletion)
/// both saves 2 B and turns the emit chain-walk into direct indexing.
///
/// `N` = 1536 nodes × 26 B = 39 936 B — the device's (LM20) **40 kB nav-budget cap**
/// (Timo, 2026-07-06), shared by host/sim/device so the sim's plannable range **is**
/// the device's range by construction. (The map gets RAM priority — real maps are far
/// bigger than the fixtures — with 60 kB the absolute nav ceiling only if the map
/// turns out not to need the space.) The sim still heap-allocates the table (a stack
/// local would trap the wasm build).
pub const NAV_MAX_NODES: usize = 1536;

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

    /// The weighted-A\* priority `f = g + ε·h` in `u32` for the current rung's ε = `eps_num/eps_den`
    /// (max ≈ 65 535 + 3.0×65 535, nowhere near wrapping; saturating for form). ε is passed in from
    /// the owning [`NavScratch`]'s per-search fields rather than a const so the [`NAV_EPSILON_LADDER`]
    /// retry can re-order the same heap at a greedier ε (N8).
    ///
    /// **Zeroed-scratch degradation**: a fresh `.bss`/zeroed [`NavScratch`] has `eps_den == 0`; every
    /// search re-seeds it (setting a non-zero rung ε) before any heap op calls `f`, but should that
    /// contract ever break, `f` degrades to plain-Dijkstra ordering (the ε term dropped) rather than
    /// dividing by zero — the chosen safe branch, asserted at seed time in [`NavPlanner::reseed`].
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

/// Saturate a `u32` meter figure into the entry's `u16` cost field (see the layout
/// note at [`NAV_MAX_NODES`]: saturation only ever *overestimates*, pruning absurd
/// nodes — it can never wrap the ordering).
#[inline]
fn sat16(m: u32) -> u16 {
    m.min(u16::MAX as u32) as u16
}

/// The selected bike profile's edge-cost parameters, resolved **once per plan** from the map's §8.6
/// profile table (epic #533 N3; the climb weight is EL6, epic #1068): the 32 highway + 8 surface
/// `u8` 1/16-fixed-point multipliers (`16` = 1.0×, `0` = forbidden) plus the profile's climb weight,
/// copied by value into the planner. The multipliers are kept as the raw 40 bytes and
/// **combined at lookup** ([`edge_cost`](Self::edge_cost)) rather than pre-expanded to a 256-entry
/// `way_kind → multiplier` table — 40 B of `.bss` next to the scratch, no 256 B table, and one
/// multiply per relaxation. Mirrors [`obc_reader::MapProfile::multiplier`]'s integer arithmetic
/// exactly (the reader owns the same combine for its own callers); the copy is what lets the
/// weighting run with no `Reader` borrow held across the search.
///
/// **[`edge_cost`](Self::edge_cost) is the router's one cost model.** POI plans and #882's detour
/// dispatch run the same [`settle`] and therefore the same function — there is deliberately no
/// second formula anywhere for a detour to drift from (pinned in `tests/detour.rs`).
#[derive(Clone, Copy)]
struct ProfileMult {
    highway: [u8; 32],
    surface: [u8; 8],
    /// §8.6 v12 `Climb Weight`, widened once so relaxation does no cast: flat metres charged per
    /// metre of a neighbor entry's `ascent_m`. `0` = climb-blind, which is both what a pre-terrain
    /// map decodes to and a legal opinion a producer may hold.
    climb: u32,
}

impl ProfileMult {
    /// The all-1.0×, climb-blind table: every non-forbidden multiplier `16`, `climb` `0`. The
    /// pre-resolution placeholder a fresh [`NavPlanner`] holds (overwritten at its first step) and
    /// the fallback for the degenerate empty-profile-table map (which snaps to nothing and fails
    /// first anyway).
    const NEUTRAL: ProfileMult = ProfileMult { highway: [16; 32], surface: [16; 8], climb: 0 };

    /// Resolve the profile selected by `profile_idx` from the reader's parsed §8.6 table. An
    /// **out-of-range index falls back to profile 0** (locked on #536: a stale device profile
    /// setting must never brick routing — never an error). The table always carries ≥ 1 profile
    /// (the reader's parse rejects `profile_count == 0`), so profile 0 always exists for a map with
    /// a graph; [`NEUTRAL`](Self::NEUTRAL) only stands in for the pathological empty table.
    fn resolve(reader: &Reader, profile_idx: u8) -> ProfileMult {
        let profiles = reader.nav_profiles();
        match profiles.get(profile_idx as usize).or_else(|| profiles.first()) {
            Some(p) => ProfileMult { highway: p.highway, surface: p.surface, climb: u32::from(p.climb_weight()) },
            None => ProfileMult::NEUTRAL,
        }
    }

    /// The §8.6 weighted cost of one adjacency entry, **the formula verbatim**:
    ///
    /// ```text
    /// (cost_m × ((highway[kind & 31] × surface[kind >> 5]) >> 4)) >> 4  +  ascent_m × climb_weight
    /// ```
    ///
    /// `None` when either multiplier class is **forbidden** (a `0` byte) — the neighbor is then
    /// skipped in relaxation, never relaxed at a huge cost, so the graph stays whole for the other
    /// profiles (§8.6).
    ///
    /// **Overflow analysis** (why the saturating ops here are discipline, not need). Every input is
    /// bounded by its wire type: `cost_m` widens from a §8.3 `uint16` (≤ 65 535), `ascent_m` is a
    /// `uint16` (≤ 65 535), and the two multiplier bytes are `u8`, so `effective ≤ (255 × 255) >> 4
    /// = 4 064` and `climb ≤ 255`. The distance term is therefore at most
    /// `(65 535 × 4 064) >> 4 = 16 645 890` and the climb term at most `65 535 × 255 = 16 711 425`;
    /// their sum, ≤ 33 357 315, is **under 1 % of `u32::MAX`**. Nothing here can wrap even on a
    /// hand-forged file, and the spec's own range check (a 60 km edge with 3 000 m of ascent at
    /// weight 15) is three orders of magnitude inside it. The one place a real value is *lost* is
    /// the caller's [`sat16`] into the 16-bit frontier cost — and a saturated `g` only makes its
    /// node maximally unattractive, never mis-ordered (see the [`NAV_MAX_NODES`] layout note).
    #[inline]
    fn edge_cost(&self, cost_m: u32, ascent_m: u16, way_kind: u8) -> Option<u32> {
        let mh = self.highway[(way_kind & 0x1F) as usize] as u32;
        let ms = self.surface[(way_kind >> 5) as usize] as u32;
        if mh == 0 || ms == 0 {
            return None;
        }
        let distance = (cost_m.saturating_mul((mh * ms) >> 4)) >> 4;
        // Additive and non-negative, always — §8.6's normative rule and the reason `h` survives
        // elevation. Nothing in this crate subtracts from a cost.
        Some(distance.saturating_add(u32::from(ascent_m).saturating_mul(self.climb)))
    }
}

/// The router's entire mutable state: an open-addressed `node_id → NavEntry` table and
/// a binary min-heap of table indices ordered by `f = g + ε·h` (heap-position
/// back-pointers make decrease-key O(log n), so a node is queued at most once — the
/// heap can never outgrow the table). Caller-owned: the sim heap-allocates one, and
/// since #1146 P2 the device's is the nav arm of the board's scratch arena in
/// `.uninit` — not a `.bss` static of its own — zero-filled in place each time a
/// search claims it. Both rest on the same property: [`NavScratch::new`] is `const`
/// and all-zero, so an all-zero block **is** `new()`. `N` is generic so tests
/// exercise the exhaustion path with a deterministic tiny table; production uses the
/// [`NAV_MAX_NODES`] default, the same on every target.
pub struct NavScratch<const N: usize = NAV_MAX_NODES> {
    entries: [NavEntry; N],
    heap: [u16; N],
    /// Occupied table slots. Insertion fails ([`NavError::Exhausted`]) at `N`, so probe
    /// loops always terminate: below `N` a free slot always exists.
    used: u16,
    heap_len: u16,
    /// The current search's ε = `eps_num/eps_den` — the [`NAV_EPSILON_LADDER`] rung [`NavEntry::f`]
    /// orders the heap by. Set by [`NavPlanner::reseed`] at the start of every search attempt
    /// (`u16` — the ladder's `3.0` fits with room to spare, and keeps the scratch at 26 B/node + the
    /// two length fields). A zeroed scratch reads `0/0`, which `f` degrades to plain-`g` ordering; it
    /// is always re-seeded to a real rung before any heap op runs (see [`NavEntry::f`]).
    eps_num: u16,
    eps_den: u16,
}

// Table budget, enforced at compile time; slot indices — heap positions and
// `came_from` — are 14-bit, with `HEAP_NONE` left over as the sentinel.
const _: () =
    assert!(core::mem::size_of::<NavScratch<NAV_MAX_NODES>>() <= 40 * 1024, "NavScratch busts the LM20 40 kB cap");
const _: () = assert!(NAV_MAX_NODES < HEAP_NONE as usize, "table indices are 14-bit (meta packs flags above them)");
const _: () = assert!(core::mem::size_of::<NavEntry>() == 24, "the slimmed 24-byte entry layout drifted");

impl<const N: usize> NavScratch<N> {
    pub const fn new() -> Self {
        assert!(N > 0 && N < HEAP_NONE as usize);
        // `eps_num`/`eps_den` are `0` here (the `.bss`/zeroed-init contract; `f` degrades safely on
        // `0/0`) and set to a real [`NAV_EPSILON_LADDER`] rung by the first search's re-seed.
        NavScratch { entries: [NavEntry::EMPTY; N], heap: [0; N], used: 0, heap_len: 0, eps_num: 0, eps_den: 0 }
    }

    /// Allocate a zeroed `NavScratch` **directly on the heap**, never on the stack.
    ///
    /// The A* table is tens of KB (~39 KB at [`NAV_MAX_NODES`]), so `Box::new(Self::new())` would
    /// first build the whole thing on the stack and then copy it — a silent overflow on a small
    /// stack (the simulator's wasm build). This owns the format crate's private invariant that a
    /// zeroed allocation *is* `new()`: every field is all-zero — `entries` is `[NavEntry::EMPTY;
    /// N]` (`EMPTY` is the zero entry), `heap` is `[0; N]`, `used`/`heap_len` are `0`, and
    /// `eps_num`/`eps_den` are `0` (re-seeded before use — `f` degrades safely on `0/0`). Adding a
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
    /// Projecting an endpoint onto the graph (one bounded lookup window per step).
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

/// One snapped endpoint. Exact endpoint projections collapse back to real graph nodes; an interior
/// projection remains a virtual node connected to both edge endpoints.
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
    /// `0`). Resolved into [`mult`](Self::mult) at the first step; out-of-range falls back to
    /// profile 0.
    profile_idx: u8,
    /// The profile's 40-byte multiplier lookup **and its climb weight**, resolved once from the
    /// reader at the first step (neutral + climb-blind until then). Every edge — POI plan or detour
    /// alike — is relaxed through [`ProfileMult::edge_cost`].
    mult: ProfileMult,
    /// The snapped endpoints (valid once their phase has run).
    start_id: u32,
    start_c: (i32, i32),
    goal_id: u32,
    goal_c: (i32, i32),
    /// Exact interior-edge metadata for the two virtual endpoints, plus the expanding lookup's
    /// current unresolved best candidate and pass (initial 200 m, then complete ≈250 m).
    start_edge: Option<NavEdgeSnap>,
    goal_edge: Option<NavEdgeSnap>,
    snap_best: Option<NavEdgeCandidate>,
    snap_ordinal: u8,
    /// Total settles so far — the RTT line's `settles=` figure, and the budget tests' probe. Stays
    /// **cumulative across [`NAV_EPSILON_LADDER`] rungs** (N8): an escalated plan's `settles` is the
    /// honest total work over every attempt, not just the last.
    settles: u32,
    /// The current [`NAV_EPSILON_LADDER`] rung index (N8): `0` = 1.3× at start, bumped on each
    /// [`NavError::Exhausted`] retry ([`epsilon_used`](Self::epsilon_used) reports it). Never advances
    /// past the last rung — the final exhaustion fails honestly at 3.0×.
    rung: usize,
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
    /// The detour blacklist (#882): `Some` only for [`new_detour`](Self::new_detour) plans. Read
    /// on every settle, so it lives here (caller-owned like the emitter, ~1 kB) — POI plans carry
    /// `None` and relax byte-identically to a planner without the field.
    corridor: Option<Corridor>,
    /// Emit-time elevation fill state (EL7): the dead-band totals, the min/max and the carried
    /// height, accumulated across every emit step. ~40 B — it rides in the planner rather than a
    /// step frame because the fill spans steps, not because of its size.
    ele: EleFill,
}

/// The route's elevation as the emit phase builds it: the shared [`DeadBand`] over the emitted
/// point stream, the raw min/max, and the last height that actually resolved.
///
/// The dead-band is the **same** integrator, at the **same** [`ELE_DEADBAND_M`] threshold, that
/// [`crate::convert`] runs over an imported GPX's `<ele>` — the point of the shared crate. `f64`
/// matches the converter's sample type exactly, so the two producers' totals differ by nothing at
/// all, not merely by little.
#[derive(Debug, Clone, Copy)]
struct EleFill {
    band: DeadBand<f64>,
    /// The last height a sample resolved, carried forward across a coverage hole; `0` until the
    /// first one, which is what a null source leaves in every stored point.
    last_m: i16,
    /// Raw min/max over *resolved* samples only — never over the carried value, so a hole cannot
    /// widen the band the profile scales to. Meaningless while `seen` is false.
    min_m: i16,
    max_m: i16,
    /// Has any sample ever resolved? False ⇒ the header keeps the pre-EL7 zeroes.
    seen: bool,
}

impl EleFill {
    fn new() -> Self {
        EleFill { band: DeadBand::new(), last_m: 0, min_m: i16::MAX, max_m: i16::MIN, seen: false }
    }

    /// Resolve one point's stored height: a real sample re-anchors the carry and grows the min/max,
    /// a hole repeats the carry. Returns the height to store, having already integrated it.
    ///
    /// **Nothing is integrated before the first resolved sample.** The hole policy is *carry the
    /// last known height forward* — and until one has resolved there is no known height to carry,
    /// only the `0` placeholder. Pushing that into the band would anchor its reference at sea level
    /// and book the entire first real height as ascent the moment coverage begins: a route whose
    /// opening points fall outside the raster (the nav graph reaches past a terrain crop — complete-
    /// way retention means the graph legally runs beyond the extract the sidecar was baked for)
    /// would report a phantom +1400 m and poison every stored `cum_ascent` after it. Skipping the
    /// push makes the first *resolved* sample the band's own first reference, which books nothing —
    /// which is also exactly what the null source does forever.
    fn resolve(&mut self, sample: Option<i16>) -> i16 {
        if let Some(h) = sample {
            self.last_m = h;
            self.min_m = self.min_m.min(h);
            self.max_m = self.max_m.max(h);
            self.seen = true;
        }
        if self.seen {
            self.band.push(f64::from(self.last_m));
        }
        self.last_m
    }

    /// The cumulative dead-banded climb so far, as the emitter stores it per point (and per chunk).
    fn cum_ascent(&self) -> u32 {
        self.band.ascent() as u32
    }

    /// The header's `(min, max, ascent, descent)`. Zeroes when nothing ever resolved — the same
    /// "no elevation" shape the converter writes for a GPX with no `<ele>` at all.
    fn stats(&self) -> (i16, i16, u32, u32) {
        if !self.seen {
            return (0, 0, 0, 0);
        }
        (self.min_m, self.max_m, self.band.ascent() as u32, self.band.descent() as u32)
    }
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
        }
    }

    /// A **detour** planner (#882): like [`new`](Self::new), but the search additionally skips
    /// any candidate edge the `corridor` blacklists — the geometric corridor around the skipped
    /// route span, built host-side with [`Corridor::build`]. The exemption discs around the two
    /// snapped endpoints are wired in automatically once the snap phases resolve.
    pub fn new_detour(from: (i32, i32), to: (i32, i32), name: &str, profile_idx: u8, corridor: Corridor) -> Self {
        let mut p = Self::new(from, to, name, profile_idx);
        p.corridor = Some(corridor);
        p
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

    /// The ε = `(num, den)` the search is **currently on** — the [`NAV_EPSILON_LADDER`] rung reached
    /// so far (N8). `(13, 10)` until the first [`NavError::Exhausted`] retry escalates it; after a
    /// terminal outcome it reads the rung the plan ended on — `(2, 1)` or `(3, 1)` if it escalated,
    /// still `(13, 10)` for a plain success or a fast [`NavError::NoPath`]. The board's per-phase RTT
    /// line logs it as `eps=`.
    pub fn epsilon_used(&self) -> (u32, u32) {
        NAV_EPSILON_LADDER[self.rung]
    }

    /// Terminal-transition helper: latch and return the failure.
    fn fail(&mut self, e: NavError) -> Step {
        self.phase = PhaseState::Terminal(Err(e));
        Step::Failed(e)
    }

    /// (Re)start a search attempt on the caller's scratch at the current [`NAV_EPSILON_LADDER`] rung
    /// (N8): clear the table, set its ε, drop any latched `table_full`, and re-seed the start node
    /// (its own predecessor, no edge) into a fresh frontier. Used for the **initial** seed (rung 0)
    /// and every ε-escalation retry — the snapped endpoints and the warm tile cache are deliberately
    /// **kept** across retries (the re-walk revisits the same region), and `settles` stays cumulative.
    /// The seed insert cannot fail on a just-reset `N ≥ 1` table (asserted by `NavScratch::new`), but
    /// the fallible signature is preserved so a corrupt call site still fails cleanly.
    fn reseed<const N: usize>(&mut self, scratch: &mut NavScratch<N>) -> Result<(), NavError> {
        scratch.reset();
        let (num, den) = NAV_EPSILON_LADDER[self.rung];
        debug_assert!(den != 0, "an ε rung denominator must be non-zero (NavEntry::f divides by it)");
        scratch.eps_num = num as u16;
        scratch.eps_den = den as u16;
        self.table_full = false;
        let si = scratch.insert(self.start_id, self.start_c.0, self.start_c.1)?;
        scratch.entries[si].h = sat16(ground_dist_m(self.start_c, self.goal_c) as u32);
        scratch.entries[si].came_from = si as u16;
        scratch.heap_push(si);
        Ok(())
    }

    /// Run **one bounded unit** of planning: one endpoint lookup window, a miss-budgeted burst of settles
    /// ([`NAV_MISSES_PER_STEP`] misses / [`NAV_SETTLES_PER_STEP_CAP`] cap), [`NAV_EMIT_HOPS_PER_STEP`]
    /// emit hops, or the finishing header patch — then return. `reader`/`scratch`/`tiles`/`elev`/`sink`
    /// are the caller's per-pass views over the same
    /// underlying state every step (on the board: a fresh `Reader` borrow + a sink over the same
    /// open file each pass). Terminal outcomes are idempotent — further steps re-return them.
    ///
    /// `elev` is the map's terrain (EL7), read **only** by the emit phase — snap and search never
    /// touch it, so a host with no terrain hands in
    /// [`NullElevation`](obc_elevation::NullElevation) and every phase behaves exactly as it did
    /// before. It arrives per step rather than living in the planner because it is a *view of a
    /// mounted file*, like `reader`: the planner is a `.bss` object with no lifetime, and the
    /// source (plus its ~2.1 kB tile cache) is the caller's static.
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
                // First step of the plan: claim the caller's buffers and resolve the bike profile
                // once (out-of-range index → profile 0; see `ProfileMult::resolve`).
                if self.snap_ordinal == 0 {
                    scratch.reset();
                    tiles.reset();
                    self.mult = ProfileMult::resolve(reader, self.profile_idx);
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
                // Both endpoints are now snapped — arm the detour corridor's take-off/landing
                // exemptions (no-op for POI plans).
                if let Some(cor) = self.corridor.as_mut() {
                    cor.set_exempt_nodes(self.start_c, self.goal_c);
                }
                // Seed the frontier with the start node at rung 0 (1.3×).
                if let Err(e) = self.reseed(scratch) {
                    return self.fail(e);
                }
                self.phase = PhaseState::Search;
                Step::Running
            }
            // Settle until the step's miss budget: pop the best-f node, close it, relax its record's
            // neighbors. Terminates: a settle closes a node or (re-open) strictly lowers an integer
            // g ≥ 0, the frontier is bounded by the table, and no new node enters once full.
            PhaseState::Search => {
                let read_start = tiles.stats().source_reads();
                let mut settled_this_step: u32 = 0;
                loop {
                    let Some(idx) = scratch.heap_pop() else {
                        // Frontier drained. A table that filled ran out of room short of the goal
                        // (Exhausted — the device's "too far"); one that never filled means the goal
                        // is genuinely disconnected/unreachable (NoPath). Preserves the two-tier UX.
                        if self.table_full && self.rung + 1 < NAV_EPSILON_LADDER.len() {
                            // ε-escalation retry (N8): the table filled then drained without the goal
                            // — retry greedier (next rung) on the SAME snapped endpoints + warm tile
                            // cache, re-seeding a fresh frontier. `settles` stays cumulative; only the
                            // Exhausted terminal escalates, so a fast NoPath (island) never does.
                            self.rung += 1;
                            if let Err(e) = self.reseed(scratch) {
                                return self.fail(e);
                            }
                            return Step::Running; // stay in Search at the greedier ε
                        }
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
                    // An interior start is a virtual node with two partial-edge exits. It has no
                    // §8.3 record. If both endpoints lie on the same edge, add the direct projected
                    // connection as well so a short mid-block route does not detour via a junction.
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
                    // Relax neighbors. A read failure is the only hard error; a full table latches
                    // `table_full` and keeps searching (decrease-key on tracked nodes still relaxes).
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
                    // A virtual goal is reached from either real endpoint with the matching
                    // directional partial distance and proportional directional ascent.
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
                    // Budget by cache misses — the only expensive unit (≈ one SD chunk read each) —
                    // so a step's wall time is ~constant whatever the hit rate; the settle cap bounds
                    // a fully-warm (hit-only) step. The check trails the settle, so a step always
                    // makes at least one node of progress.
                    if tiles.stats().source_reads() - read_start >= NAV_MISSES_PER_STEP
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
                    match self.arm_emitter(scratch, elev, sink) {
                        Ok(true) => return Step::Running, // degenerate single-point route emitted
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
        elev: &mut dyn ElevationSource,
        sink: &mut dyn ByteSink,
    ) -> Result<bool, NavError> {
        let em = ObcrEmitter::new(sink).map_err(|_| NavError::NoPath)?;
        self.em = Some(em);
        if self.chain_len == 1 {
            let e = &scratch.entries[scratch.heap[0] as usize];
            let (lon, lat) = (e.lon, e.lat);
            // The degenerate route is one point, so it needs no densification — just its height
            // (0 under a null source, which keeps this arm byte-identical).
            let ele = self.ele.resolve(elev.sample(lat, lon));
            if self.em.as_mut().is_none_or(|em| em.push(sink, lon, lat, ele, 0).is_err()) {
                return Err(NavError::NoPath);
            }
            self.phase = PhaseState::Finish;
            return Ok(true);
        }
        Ok(false)
    }

    /// Consume the emitter and patch the header — the plan's last writes.
    ///
    /// The elevation figures are read off the emit phase's [`EleFill`] (EL7): the dead-banded
    /// totals over the *emitted* point stream and the raw min/max over the samples that resolved.
    /// The distance total is untouched — it stays the summed raw edge `length_m` (N3), never the
    /// emitter's re-measured polyline, and densifying the polyline does not change it.
    ///
    /// `#[inline(never)]` for the same reason as [`arm_emitter`](Self::arm_emitter):
    /// `Option::take` moves the ~9 kB emitter into a local; that temporary belongs in this
    /// popped frame, never in the step frame.
    #[inline(never)]
    fn finish_emit(&mut self, sink: &mut dyn ByteSink) -> Result<RouteStats, NavError> {
        let (min_ele_m, max_ele_m, ascent_m, descent_m) = self.ele.stats();
        let stats = EmitStats {
            min_ele_m,
            max_ele_m,
            ascent_m,
            descent_m,
            total_distance_m: Some(self.total_m),
            // The fill's own `seen` latch, handed out verbatim — the explicit "terrain answered"
            // signal a detour splice needs (#1091). Never inferred from the values: a route at
            // `0 m` throughout is a real sea-level route, not an elevation-less one.
            has_elevation: self.ele.seen,
        };
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
    /// displayed distance, summed here rather than read off the weighted `g` (N3). Every emitted
    /// point's height comes from `elev` through [`fill_segment`] (EL7), which also inserts the
    /// [`ELE_SAMPLE_STEP_M`] intermediates when there is terrain to put on them.
    ///
    /// The elevation work adds **no local** to this frame beyond the `EleFill` borrow: the state
    /// lives in the planner, the tile cache in the caller's static, and the per-segment
    /// interpolation runs one level down in [`fill_segment`]'s own popped frame.
    #[inline(never)] // #419/#501: a phase-boundary frame — never inlined into the step frame
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
        let mut last = self.last;
        let mut werr = false;
        let ele = &mut self.ele;
        let mut push = |pt| {
            if werr || last == Some(pt) {
                return; // seam vertex already emitted by the previous hop
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
        // Real ground meters for the displayed total (saturating, like every stored cost).
        self.total_m = self.total_m.saturating_add(length_m);
        Ok(())
    }

    /// Resolve a real/virtual A* entry to its exact position and raw offset on `edge_id`.
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

/// Emit one geometry segment `from → to` with its elevation (EL7).
///
/// Samples `to`'s height once, and — **only when that sample resolved**, i.e. only where there is
/// terrain — inserts linearly interpolated points so no two emitted points are more than
/// [`ELE_SAMPLE_STEP_M`] of ground apart, sampling each of them in travel order. Every point (real
/// or interpolated) goes through [`EleFill::resolve`], so the dead-band sees the whole stream and
/// the stored `cum_ascent` stays consistent with it.
///
/// With a null source `sample` is `None`, the loop never runs and the pushed height is 0: the exact
/// call the pre-EL7 emit made, which is what makes the no-terrain output byte-identical.
///
/// `#[inline(never)]`: this is the fill's own popped frame (#419/#501). It holds the interpolation
/// locals — a handful of scalars — and, more to the point, it keeps the *emitter's* `push` call
/// tree out of [`NavPlanner::emit_hop`]'s frame, which is the frame that already carries the
/// polyline closure.
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
    // The first height that resolves is also the moment the emitter's decimator has something to
    // preserve: from here on a vertex whose height has moved a dead-band from the last kept one is
    // kept whatever the geometry says (see `ObcrEmitter::keep_elevation_detail`). Latched on the
    // first sample rather than up front so a null source never touches the decimator at all.
    if sample.is_some() && !ele.seen {
        em.keep_elevation_detail(ELE_KEEP_M);
    }
    // Densify while this route has terrain — `sample` resolving, or any earlier one having. The
    // "or earlier" half matters at a coverage edge: the segment that *leaves* the raster still has
    // its far half on real ground, and a segment that crosses a hole entirely costs nothing anyway
    // (its interpolated points are all the carried height, so the decimator drops them again).
    if let (Some(prev), true) = (from, sample.is_some() || ele.seen) {
        let steps = densify_steps(ground_dist_m(prev, to));
        for k in 1..steps {
            let mid = lerp_udeg(prev, to, k, steps);
            let h = ele.resolve(elev.sample(mid.1, mid.0));
            em.push(sink, mid.0, mid.1, h, ele.cum_ascent())?;
        }
    }
    let h = ele.resolve(sample);
    em.push(sink, to.0, to.1, h, ele.cum_ascent())
}

/// How many equal pieces a `dist_m` segment is split into to keep every emitted step at or under
/// [`ELE_SAMPLE_STEP_M`]. `1` (no split) for anything already short enough; capped at
/// [`ELE_MAX_DENSIFY_STEPS`].
fn densify_steps(dist_m: f32) -> u32 {
    // `is_none_or` rather than `!(a > b)`: the NaN case is deliberate (a length that isn't a number
    // densifies nothing) and this spells it out instead of leaning on negated float comparison.
    if dist_m.partial_cmp(&ELE_SAMPLE_STEP_M).is_none_or(|o| o != core::cmp::Ordering::Greater) {
        return 1;
    }
    (libm::ceilf(dist_m / ELE_SAMPLE_STEP_M) as u32).clamp(1, ELE_MAX_DENSIFY_STEPS)
}

/// The point `k/den` of the way from `a` to `b`, interpolated **in microdegrees** — integer-only,
/// truncating (≤ 1 µdeg ≈ 11 cm, far below the raster's ~40 m posting). Interpolating the stored
/// integer coordinate rather than a projected metre pair keeps this deterministic across hosts, and
/// the segment is short enough that a great-circle path and a lattice-linear one are the same line.
fn lerp_udeg(a: (i32, i32), b: (i32, i32), k: u32, den: u32) -> (i32, i32) {
    let f = |s: i32, e: i32| {
        let d = i64::from(e) - i64::from(s);
        (i64::from(s) + d * i64::from(k) / i64::from(den)) as i32
    };
    (f(a.0, b.0), f(a.1, b.1))
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
    elev: &mut dyn ElevationSource,
    sink: &mut dyn ByteSink,
) -> Result<RouteStats, NavError> {
    let mut planner = NavPlanner::new(from, to, name, profile_idx);
    loop {
        match planner.step(reader, scratch, tiles, elev, sink) {
            Step::Running => {}
            Step::Done(stats) => return Ok(stats),
            Step::Failed(e) => return Err(e),
        }
    }
}

/// One-shot convenience over [`NavPlanner::new_detour`]: loop [`step`](NavPlanner::step) to
/// completion with the corridor blacklist — the headless sim's and the tests' detour twin of
/// [`plan_route`]; interactive hosts step the planner themselves.
#[allow(clippy::too_many_arguments)] // same shape rationale as `plan_route`
pub fn plan_detour<const N: usize>(
    reader: &Reader,
    from: (i32, i32),
    to: (i32, i32),
    name: &str,
    profile_idx: u8,
    corridor: Corridor,
    scratch: &mut NavScratch<N>,
    tiles: &mut NavTileCache,
    elev: &mut dyn ElevationSource,
    sink: &mut dyn ByteSink,
) -> Result<RouteStats, NavError> {
    let mut planner = NavPlanner::new_detour(from, to, name, profile_idx, corridor);
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
    match scratch.lookup(target_id) {
        Some(j) => {
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
        None => {
            if let Ok(j) = scratch.insert(target_id, target_coord.0, target_coord.1) {
                let entry = &mut scratch.entries[j];
                entry.g = tentative;
                entry.h = sat16(ground_dist_m(target_coord, goal_c) as u32);
                entry.came_from = from as u16;
                entry.edge_used = edge_id;
                scratch.heap_push(j);
                false
            } else {
                true
            }
        }
    }
}

/// One settle: descend the node quadtree to the settled node's leaf (a degenerate
/// one-point view — the spatial re-fetch) and relax each of its §8.3 neighbors from
/// the inline `(coord, cost_m, way_kind, ascent_m)` through the plan's profile
/// ([`ProfileMult::edge_cost`]). A neighbor whose `way_kind` is **forbidden** under the profile
/// (`edge_cost` is `None`) is skipped — not relaxed — so the graph stays whole for other profiles. A
/// node the walk doesn't yield (corrupt map) simply relaxes nothing; the search
/// continues on whatever frontier remains.
///
/// **Exhaustion salvage** (N4): when the scratch is full a *new* discovery can't be inserted, so it
/// is dropped and `*table_full` is latched — but decrease-key of an already-tracked neighbor still
/// relaxes normally (zero allocations). The only hard error is a read failure ([`NavError::NoPath`]);
/// running out of table is no longer an error here — the caller drains the frontier and maps a
/// latched `table_full` to [`NavError::Exhausted`] then.
// The arg list is the relax context (goal, profile, optional corridor) plus the caller-owned
// buffers — same shape rationale as `plan_route`'s allow.
#[allow(clippy::too_many_arguments)]
#[inline(never)] // #419/#501: a phase-boundary frame — never inlined into the step frame
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
            // Idempotent under N2's bin-packed chunks (a node may be yielded more than once when two
            // leaves share a chunk): only the settled node's own record relaxes anything.
            if n.id != settled.node_id {
                return;
            }
            for nb in n.neighbors() {
                // The §8.6 edge cost: profile-weighted ground length **plus** the entry's own
                // directional climb charged at the profile's weight. `ascent_m` rides on the
                // adjacency entry already in hand — no second fetch, which is why EL5 put it there.
                // A forbidden class (`edge_cost == None`) is skipped entirely — the neighbor is
                // never relaxed, so the graph stays whole for the other profiles.
                let Some(weighted) = mult.edge_cost(nb.cost_m, nb.ascent_m, nb.way_kind) else {
                    continue;
                };
                // Detour blacklist (#882): an edge whose chord hugs the skipped span is skipped
                // exactly like a forbidden class — never relaxed, graph untouched for other plans.
                if corridor.is_some_and(|c| c.blocks((settled.lon, settled.lat), (nb.lon, nb.lat))) {
                    continue;
                }
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

/// Scan one complete node-or-anchor lookup square and project all candidate edge geometries
/// exactly. The caller first tries 200 m; if the triangle bound cannot prove that winner final, it
/// follows with the complete ≈250 m square (whose overlapping center normally hits the cache).
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

/// Unit cover for the one piece of arithmetic the integration suite can only reach through a packed
/// map: [`ProfileMult::edge_cost`] at the extremes of its wire types (EL6). The routing *behaviour*
/// is pinned end-to-end over real writer→reader bytes in `tests/nav.rs`; what lives here is the
/// overflow argument from `edge_cost`'s doc, executed.
#[cfg(test)]
mod tests {
    use super::{sat16, ProfileMult};

    /// The maximum every input can legally reach — `cost_m` and `ascent_m` both `u16::MAX`, both
    /// multiplier bytes and the climb weight all `u8::MAX`. The exact sum is asserted (not merely
    /// "it didn't panic"), because the claim being pinned is that the true value *fits*: at
    /// 33 357 315 it is under a hundredth of `u32::MAX`, so the router never reaches its own
    /// saturation. Runs under `-C overflow-checks` in the debug test profile, which is what makes it
    /// a real tripwire rather than a wrap-tolerant smoke test.
    #[test]
    fn edge_cost_at_the_wire_maxima_is_exact_and_nowhere_near_wrapping() {
        let p = ProfileMult { highway: [u8::MAX; 32], surface: [u8::MAX; 8], climb: u32::from(u8::MAX) };
        let got = p.edge_cost(u32::from(u16::MAX), u16::MAX, 0xFF).expect("255 is not forbidden");
        // (65 535 × ((255 × 255) >> 4)) >> 4 = 16 645 890 distance, + 65 535 × 255 = 16 711 425 climb.
        assert_eq!(got, 16_645_890 + 16_711_425);
        assert!(got < u32::MAX / 64, "the worst legal edge must stay far inside u32");
        // The only lossy step is the frontier's own 16-bit field, and it clamps rather than wraps.
        assert_eq!(sat16(u32::from(u16::MAX).saturating_add(got)), u16::MAX);
    }

    /// A `climb_weight` of `0` and an `ascent_m` of `0` each independently reduce the formula to
    /// v11's — the null path, in arithmetic form.
    #[test]
    fn either_zero_reproduces_the_pre_terrain_cost() {
        let blind = ProfileMult { highway: [16; 32], surface: [16; 8], climb: 0 };
        let weighted = ProfileMult { climb: 10, ..blind };
        assert_eq!(blind.edge_cost(1_000, 400, 0), Some(1_000), "climb-blind ignores a 400 m climb");
        assert_eq!(weighted.edge_cost(1_000, 0, 0), Some(1_000), "a flat edge costs its ground length");
        assert_eq!(weighted.edge_cost(1_000, 400, 0), Some(5_000), "…and a climbing one is charged for it");
    }

    /// A forbidden class is `None` **whatever the climb**: the skip decision is the multiplier's
    /// alone, so an unroutable edge is never relaxed at some enormous cost instead.
    #[test]
    fn a_forbidden_class_stays_forbidden_under_any_climb() {
        let mut p = ProfileMult { highway: [16; 32], surface: [16; 8], climb: 255 };
        p.highway[4] = 0;
        assert_eq!(p.edge_cost(1_000, 0, 4), None);
        assert_eq!(p.edge_cost(1_000, u16::MAX, 4), None);
    }
}
