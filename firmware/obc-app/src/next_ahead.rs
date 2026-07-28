//! The **per-category "next ahead" cache** the `Next: <category>` stat fields read (epic #946, U5).
//!
//! A stat tile is a glance, not a browse: the six [`NextWater`](crate::StatField)-style fields each
//! answer one question — *how far to the next water / campsite / … on my route?* — from the same two
//! sources the [Up-ahead timeline](crate::screen::UpAheadScreen) merges. The **waypoint** half is
//! resident RAM and needs no cache at all (the tile walks the table every draw, ~32 entries, zero
//! I/O). The **map-POI** half is an SD query, and this is the piece that keeps it off the frame
//! path.
//!
//! # Why a cache and not a snapshot
//!
//! The Up-ahead list needs *membership and order* frozen (rows must not shift under the cursor —
//! the #115/#425 contract). A stat tile needs neither: it shows exactly one entry, and the rider
//! wants its distance to **count down** as they ride. So the tile is fed a distilled fact per
//! category — `(dist_along_m, name)` — and re-derives the distance from live progress every frame.
//! The snapshot machinery stays underneath, unchanged: the distillation is harvested out of the
//! App-owned [`CorridorScratch`](crate::corridor::CorridorScratch), which is still the only thing
//! that ever touches the card.
//!
//! # The refresh policy (locked in #951)
//!
//! A category's cached entry is re-taken only when
//!
//! 1. **nothing is cached yet** for it (first visit to the stats page with that tile placed),
//! 2. matched progress has advanced [`REFRESH_STEP_M`] since the take that filled it,
//! 3. matched progress has fallen more than [`REWIND_TOLERANCE_M`] **behind** that take — a
//!    snapshot is only an answer for the axis *ahead of its anchor*, so progress moving back below
//!    it leaves the cache blind to everything in between, or
//! 4. **the cached entry was passed** (progress moved past its `dist_along_m`) — the one case where
//!    a stale answer is actively wrong.
//!
//! and only for categories a `Next:` tile is actually **placed** on the grid, and only while the
//! Statistics screen is the one being drawn. Everything else costs nothing: a rider with no such
//! tile never runs the query, and neither does a rider on the map, in a menu, or with no route.
//!
//! # One category per query
//!
//! Each refresh arms the corridor scratch for a **single** category
//! ([`PoiCategorySet::only`]). That is not an optimization, it is a correctness requirement: the
//! corridor query caps at [`MAX_CORRIDOR_RESULTS`](obc_reader::MAX_CORRIDOR_RESULTS) entries across
//! the whole filter, so a union query could return sixteen nearby fountains and never mention the
//! pharmacy 12 km on — and the pharmacy tile would lie. Filtered to one category the nearest of it
//! is entry `0` by construction. With `k` tiles placed the scheduler round-robins, so each category
//! refreshes every `k` × [`REFRESH_STEP_M`] of riding at worst, and no category can starve.
//!
//! # Cost
//!
//! One corridor query per refresh, i.e. per category per [`REFRESH_STEP_M`] of matched progress
//! while the Statistics screen is up — 12 queries/km with all six tiles placed, 2/km with the
//! typical one. The reader-build seam follows the same one-shot rule as everything else here: the
//! request is what [`pending`](crate::corridor::CorridorScratch::pending) reports, so the host
//! builds the `Reader` **only until the snapshot lands** and then stops. Off-route, with no fix, or
//! with the query failing, the tile keeps showing the last cached entry (and `--` once nothing is
//! cached) — it never blocks, spins, or re-queries per frame.

use obc_reader::{CorridorPoi, PoiCategory, PoiCategorySet};

use crate::corridor::CorridorKey;

/// How far the rider must ride before a cached category is re-taken. 500 m at typical touring speed
/// is ~1.5 min, which is well inside the resolution a `2.4km` readout even shows — while being far
/// enough that the query runs a handful of times per hour, not per frame.
pub const REFRESH_STEP_M: u32 = 500;

/// How far matched progress may drift **backwards** below a take's anchor before the slot is
/// re-taken.
///
/// Backward movement is not symmetric with riding on: the corridor query only returns entries
/// *ahead of its anchor*, so once progress falls below that anchor the cache is blind to whatever
/// sits in between — and a genuine rewind (a route re-upload or a second ride on the same route
/// both zero `progress_m` while the route index stays put) would otherwise keep an old,
/// far-along answer, or a `None`, for kilometres. So a rewind must re-take.
///
/// It cannot re-take on *any* backward step, though: the route matcher searches
/// `BACK_SEGS` segments behind the cursor (`obc_route::matcher`), so ordinary re-matching wobbles
/// progress back a few metres on GPS noise, and a zero-tolerance rule would burn a card query per
/// wobble. 100 m is comfortably past that slack (three route segments of a decimated OBCR are well
/// under it) while being a fifth of the forward step, so the blind window a tolerated rewind opens
/// stays far smaller than the one riding on inside [`REFRESH_STEP_M`] already accepts.
pub const REWIND_TOLERANCE_M: u32 = 100;

/// Longest cached POI name — the [`StatCell`](crate::stat_fields::StatCell) caption's capacity, so
/// the tile can show whatever the cache holds and the tile drawer does the ellipsizing.
pub const NEXT_NAME_CAP: usize = 24;

/// The number of POI categories the cache carries one slot for.
const CATEGORIES: usize = PoiCategory::ALL.len();

/// One cached "next map POI of this category ahead": where it sits on the route axis, and its row
/// name (already resolved through the POI browser's subtype fallback, so an unnamed POI still reads
/// as something).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NextPoi {
    /// Along-route position, meters from the route start — the same axis waypoints use, so the tile
    /// re-derives distance-to-go from live progress.
    pub dist_along_m: u32,
    /// The row name: the POI's own, or its subtype label.
    pub name: heapless::String<NEXT_NAME_CAP>,
}

/// One category's cache line.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Slot {
    /// The nearest map POI of this category ahead at the last take — `None` means "queried, nothing
    /// in the corridor", which is a real answer and not the same as never queried.
    poi: Option<NextPoi>,
    /// Progress the last take was anchored at; `None` until the first one lands.
    taken_at_m: Option<u32>,
}

impl Slot {
    const EMPTY: Slot = Slot { poi: None, taken_at_m: None };
}

/// The App-owned per-category cache + its refresh scheduler. Held once next to the corridor scratch
/// (never inside a [`Screen`](crate::screen::Screen) — see [`CorridorScratch`]'s #425 note); a
/// [`Readout`](crate::stat_fields::Readout) borrows it read-only.
///
/// [`CorridorScratch`]: crate::corridor::CorridorScratch
#[derive(Debug)]
pub struct NextAhead {
    slots: [Slot; CATEGORIES],
    /// The refresh currently asked for: the corridor key (a single-category filter) and the slot it
    /// will fill. `None` when nothing needs re-taking — the normal, quiet state.
    want: Option<(usize, CorridorKey)>,
    /// Round-robin cursor over the categories, so a busy grid refreshes them in turn rather than
    /// starving the last one.
    turn: usize,
    /// The route the cached entries belong to. A different route (or none) empties the cache —
    /// along-route distances from another route are meaningless, not merely stale.
    route: Option<usize>,
}

impl NextAhead {
    /// An empty cache — the `'static` stand-in a test or a non-`App` host hands a
    /// [`Readout`](crate::stat_fields::Readout) when it has no cache of its own (the tiles then read
    /// the resident waypoint table only).
    pub const EMPTY: NextAhead = NextAhead::new();

    pub const fn new() -> Self {
        NextAhead { slots: [Slot::EMPTY; CATEGORIES], want: None, turn: 0, route: None }
    }

    /// The cached nearest map POI of `cat`, if one has been taken. The caller compares it against
    /// the resident waypoint table and against live progress — this is a *fact about the route*,
    /// not a rendered answer.
    #[inline]
    pub fn poi(&self, cat: PoiCategory) -> Option<&NextPoi> {
        self.slots[slot_of(cat)].poi.as_ref()
    }

    /// The corridor snapshot this cache wants taken, if any — the request
    /// [`reconcile_corridor`](crate::ui_runtime::UiRuntime::reconcile_corridor) falls back to when
    /// no *screen* is asking for one.
    #[inline]
    pub(crate) fn request(&self) -> Option<CorridorKey> {
        self.want.map(|(_, key)| key)
    }

    /// Drop every cached entry because the **geometry under the current route index changed** —
    /// the same-index/new-bytes replace [`reconcile`](Self::reconcile) cannot see, because it keys
    /// identity on the catalog index alone.
    ///
    /// Called from `App`'s `drop_route_derived_state` seam, alongside the matcher / profile /
    /// climb / waypoint caches that are invalidated for exactly the same reason: an along-route
    /// distance measured on the old bytes names a different place on the new ones. The route key
    /// itself is left alone (the index really is unchanged), so the next
    /// [`reconcile`](Self::reconcile) simply finds every placed category stale and re-takes it.
    pub(crate) fn invalidate(&mut self) {
        self.clear();
    }

    /// Empty the cache (a route load/swap/close). Distances are route-relative, so entries from
    /// another route are wrong, not stale.
    fn clear(&mut self) {
        self.slots = [Slot::EMPTY; CATEGORIES];
        self.want = None;
        self.turn = 0;
    }

    /// Re-decide what (if anything) needs re-taking. Called once per pass from
    /// [`advance_animations`](crate::App::advance_animations), i.e. from the one hook every host
    /// runs — never from a draw.
    ///
    /// * `placed` — the categories with a `Next:` tile on the grid. Empty ⇒ nothing to keep warm.
    /// * `shown` — whether the Statistics screen is the one being drawn. The tiles are invisible
    ///   anywhere else, so the query is scoped to where the answer is actually read.
    /// * `active_route` / `progress_m` — the route the cache is keyed to and matched progress.
    ///
    /// An in-flight request is **kept as-is** while it is still wanted: re-deciding its anchor every
    /// pass would re-key the scratch every pass, and re-keying is what re-queries (the #115 rule
    /// the corridor key exists to enforce).
    pub(crate) fn reconcile(
        &mut self,
        placed: PoiCategorySet,
        shown: bool,
        active_route: Option<usize>,
        progress_m: u32,
    ) {
        if self.route != active_route {
            self.route = active_route;
            self.clear();
        }
        if !shown || active_route.is_none() || placed.is_empty() {
            self.want = None;
            return;
        }
        // Keep an in-flight request only while its category is still placed and still stale — a tile
        // deleted mid-query, or a snapshot that landed for someone else's identical key, drops it.
        if let Some((i, _)) = self.want {
            let cat = PoiCategory::ALL[i];
            if placed.contains(cat) && self.is_stale(i, progress_m) {
                return;
            }
        }
        self.want = self.pick(placed, progress_m);
    }

    /// Whether slot `i` needs re-taking at `progress_m` — the four locked triggers.
    fn is_stale(&self, i: usize, progress_m: u32) -> bool {
        let slot = &self.slots[i];
        match slot.taken_at_m {
            None => true, // (a) nothing cached yet
            Some(at) => {
                progress_m.saturating_sub(at) >= REFRESH_STEP_M              // (b) rode on
                    || at.saturating_sub(progress_m) > REWIND_TOLERANCE_M    // (c) rewound past the anchor
                    || slot.poi.as_ref().is_some_and(|p| progress_m > p.dist_along_m)
                // (d) passed it
            }
        }
    }

    /// The next stale placed category in round-robin order, as a single-category corridor request
    /// anchored at live progress. Advances [`turn`](Self::turn) past whatever it picks so the next
    /// refresh starts at the following category.
    fn pick(&mut self, placed: PoiCategorySet, progress_m: u32) -> Option<(usize, CorridorKey)> {
        for step in 0..CATEGORIES {
            let i = (self.turn + step) % CATEGORIES;
            let cat = PoiCategory::ALL[i];
            if placed.contains(cat) && self.is_stale(i, progress_m) {
                self.turn = (i + 1) % CATEGORIES;
                return Some((i, CorridorKey { filter: PoiCategorySet::only(cat), anchor_m: progress_m }));
            }
        }
        None
    }

    /// Distil a landed corridor snapshot into the slot it was asked for. A no-op unless `key` is
    /// exactly the request in flight, so a snapshot taken for the Up-ahead list can never overwrite
    /// a cache line with a differently-filtered (or differently-anchored) answer.
    ///
    /// `entries` is ascending by along-route distance and filtered to the one category, so its first
    /// element *is* the nearest of that category ahead of the anchor. An empty snapshot is a real
    /// answer ("nothing of this kind on the route ahead") and settles the slot just the same — which
    /// is what stops the query re-running every frame on a map with no pharmacies.
    pub(crate) fn harvest(&mut self, key: CorridorKey, entries: &[CorridorPoi]) {
        let Some((i, want)) = self.want else { return };
        if want != key {
            return;
        }
        self.slots[i].poi = entries.first().map(|e| {
            let mut name: heapless::String<NEXT_NAME_CAP> = heapless::String::new();
            // Truncate on the char boundary rather than failing: a name longer than a tile caption
            // is ellipsized by the tile drawer anyway.
            for ch in crate::screen::poi_row_name(&e.poi).chars() {
                if name.push(ch).is_err() {
                    break;
                }
            }
            NextPoi { dist_along_m: e.dist_along_m, name }
        });
        self.slots[i].taken_at_m = Some(key.anchor_m);
        self.want = None;
    }
}

impl Default for NextAhead {
    fn default() -> Self {
        NextAhead::new()
    }
}

/// The cache slot index of `cat` — its position in [`PoiCategory::ALL`], which is also the
/// round-robin order.
#[inline]
fn slot_of(cat: PoiCategory) -> usize {
    // `id()` is 1-based and dense over ALL (OBCM §7.4), so this never wraps or goes out of range.
    (cat.id() as usize - 1).min(CATEGORIES - 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_reader::Poi;

    /// A corridor snapshot entry at `dist_along_m` with `name`.
    fn poi(dist_along_m: u32, name: &str, subtype: u8) -> CorridorPoi {
        let mut n = heapless::String::new();
        n.push_str(name).unwrap();
        CorridorPoi {
            poi: Poi { lat: 0, lon: 0, subtype, name: n, hours_ref: 0xFFFF, distance_m: dist_along_m },
            dist_along_m,
            offset_m: 0,
        }
    }

    const WATER: u8 = 1;
    /// Water + bike shop — the two-tile grid most of these tests schedule against.
    fn two() -> PoiCategorySet {
        PoiCategorySet::only(PoiCategory::Water).with(PoiCategory::BikeShop)
    }

    /// Every category maps to its own slot, in `PoiCategory::ALL` order.
    #[test]
    fn slots_follow_the_canonical_category_order() {
        for (i, cat) in PoiCategory::ALL.iter().enumerate() {
            assert_eq!(slot_of(*cat), i, "{cat:?} owns slot {i}");
        }
    }

    /// The quiet states: nothing placed, no route, or the stats screen not up ⇒ no request at all,
    /// so `reconcile_corridor` never arms the scratch and the host never builds a `Reader`.
    #[test]
    fn nothing_placed_or_shown_asks_for_nothing() {
        let mut c = NextAhead::new();
        c.reconcile(PoiCategorySet::EMPTY, true, Some(0), 0);
        assert_eq!(c.request(), None, "no Next: tile on the grid ⇒ no query");
        c.reconcile(two(), false, Some(0), 0);
        assert_eq!(c.request(), None, "the stats screen isn't up ⇒ no query");
        c.reconcile(two(), true, None, 0);
        assert_eq!(c.request(), None, "no route ⇒ nothing is 'ahead'");
    }

    /// A placed category with nothing cached asks for a **single-category** snapshot anchored at
    /// live progress — the cap-correctness rule (a union query could bury a rare category).
    #[test]
    fn a_fresh_category_asks_for_a_single_category_snapshot() {
        let mut c = NextAhead::new();
        c.reconcile(PoiCategorySet::only(PoiCategory::Water), true, Some(0), 1_200);
        assert_eq!(
            c.request(),
            Some(CorridorKey { filter: PoiCategorySet::only(PoiCategory::Water), anchor_m: 1_200 }),
            "one category per query, anchored at progress"
        );
    }

    /// The whole refresh policy in one ride: a take settles the slot, riding on inside
    /// `REFRESH_STEP_M` re-queries **nothing**, and crossing it re-arms — the "never per frame"
    /// guarantee, asserted as a query count.
    #[test]
    fn a_settled_category_does_not_re_query_until_the_progress_step() {
        let mut c = NextAhead::new();
        let placed = PoiCategorySet::only(PoiCategory::Water);
        let mut queries = 0;
        // A frame: reconcile, and if something is wanted, satisfy it (the corridor scratch's job).
        let frame = |c: &mut NextAhead, progress: u32, queries: &mut u32| {
            c.reconcile(placed, true, Some(0), progress);
            if let Some(key) = c.request() {
                *queries += 1;
                c.harvest(key, &[poi(9_000, "Fontaine", WATER)]);
            }
        };

        frame(&mut c, 0, &mut queries);
        assert_eq!(queries, 1, "the first frame takes the one snapshot");
        assert_eq!(c.poi(PoiCategory::Water).unwrap().dist_along_m, 9_000);

        // 100 frames of riding inside the step: not one further query.
        for m in (10..=499).step_by(10) {
            frame(&mut c, m, &mut queries);
        }
        assert_eq!(queries, 1, "riding on inside the step never re-queries — the tile counts down instead");

        frame(&mut c, REFRESH_STEP_M, &mut queries);
        assert_eq!(queries, 2, "crossing the step re-takes it once");
        for m in (510..=999).step_by(10) {
            frame(&mut c, m, &mut queries);
        }
        assert_eq!(queries, 2, "…and then goes quiet again");
    }

    /// Trigger (c): riding past the cached entry re-arms immediately, without waiting for the
    /// progress step — the one case where a stale answer is actively wrong.
    #[test]
    fn passing_the_cached_entry_re_arms_at_once() {
        let mut c = NextAhead::new();
        let placed = PoiCategorySet::only(PoiCategory::Water);
        c.reconcile(placed, true, Some(0), 1_000);
        c.harvest(c.request().unwrap(), &[poi(1_100, "Fontaine", WATER)]);
        assert_eq!(c.request(), None, "settled");

        c.reconcile(placed, true, Some(0), 1_050);
        assert_eq!(c.request(), None, "still ahead (and only 50 m ridden) ⇒ quiet");
        c.reconcile(placed, true, Some(0), 1_101);
        assert_eq!(
            c.request(),
            Some(CorridorKey { filter: PoiCategorySet::only(PoiCategory::Water), anchor_m: 1_101 }),
            "one metre past the cached fountain re-arms, step or no step"
        );
    }

    /// An **empty** snapshot is an answer, not a failure: the slot settles on "nothing of this kind"
    /// and stops asking, instead of re-running the query every frame on a map without that category.
    #[test]
    fn an_empty_snapshot_settles_the_slot() {
        let mut c = NextAhead::new();
        let placed = PoiCategorySet::only(PoiCategory::Pharmacy);
        c.reconcile(placed, true, Some(0), 0);
        c.harvest(c.request().unwrap(), &[]);
        assert_eq!(c.poi(PoiCategory::Pharmacy), None);
        c.reconcile(placed, true, Some(0), 10);
        assert_eq!(c.request(), None, "queried-and-empty is settled, not pending");
    }

    /// An in-flight request keeps its key across passes — progress advancing under it must not
    /// re-anchor (which would re-key the scratch and re-run the query every single frame).
    #[test]
    fn an_in_flight_request_keeps_its_anchor() {
        let mut c = NextAhead::new();
        let placed = PoiCategorySet::only(PoiCategory::Water);
        c.reconcile(placed, true, Some(0), 1_000);
        let first = c.request().unwrap();
        for m in [1_010, 1_400, 2_000, 5_000] {
            c.reconcile(placed, true, Some(0), m);
            assert_eq!(c.request(), Some(first), "the key is frozen until the snapshot lands");
        }
    }

    /// Two placed tiles are served in turn, one query per pass, and neither starves.
    #[test]
    fn the_scheduler_round_robins_placed_categories() {
        let mut c = NextAhead::new();
        c.reconcile(two(), true, Some(0), 0);
        let a = c.request().unwrap();
        assert_eq!(a.filter, PoiCategorySet::only(PoiCategory::Water), "Water is first in ALL order");
        c.harvest(a, &[poi(500, "Fontaine", WATER)]);
        c.reconcile(two(), true, Some(0), 0);
        let b = c.request().unwrap();
        assert_eq!(b.filter, PoiCategorySet::only(PoiCategory::BikeShop), "the other placed tile is served next");
        c.harvest(b, &[]);
        c.reconcile(two(), true, Some(0), 0);
        assert_eq!(c.request(), None, "both settled ⇒ the reader seam goes quiet");
    }

    /// A snapshot the cache didn't ask for never lands in it — the Up-ahead list re-keying the
    /// shared scratch mid-refresh must not write a foreign answer into a tile.
    #[test]
    fn a_foreign_snapshot_is_ignored() {
        let mut c = NextAhead::new();
        let placed = PoiCategorySet::only(PoiCategory::Water);
        c.reconcile(placed, true, Some(0), 0);
        let mine = c.request().unwrap();
        // Same category, someone else's anchor (the Up-ahead screen's entry progress).
        c.harvest(CorridorKey { filter: mine.filter, anchor_m: 4_000 }, &[poi(100, "Not mine", WATER)]);
        assert_eq!(c.poi(PoiCategory::Water), None, "a differently-keyed snapshot is not this cache's answer");
        assert_eq!(c.request(), Some(mine), "…and the request is still in flight");
        // Everything filtered differently is ignored too.
        c.harvest(CorridorKey { filter: PoiCategorySet::ALL, anchor_m: 0 }, &[poi(100, "Not mine", WATER)]);
        assert_eq!(c.poi(PoiCategory::Water), None);
    }

    /// A route swap empties the cache: along-route distances from another route aren't stale, they
    /// are meaningless.
    #[test]
    fn a_route_change_empties_the_cache() {
        let mut c = NextAhead::new();
        let placed = PoiCategorySet::only(PoiCategory::Water);
        c.reconcile(placed, true, Some(0), 0);
        c.harvest(c.request().unwrap(), &[poi(500, "Fontaine", WATER)]);
        assert!(c.poi(PoiCategory::Water).is_some());
        c.reconcile(placed, true, Some(1), 0);
        assert_eq!(c.poi(PoiCategory::Water), None, "another route ⇒ another axis ⇒ drop it");
        c.reconcile(placed, true, None, 0);
        assert_eq!(c.request(), None, "and a route-less ride asks for nothing");
    }

    /// Trigger (c): progress **rewinding** far below the take's anchor re-arms. The reachable case
    /// is a route re-upload or a second ride on the same route — both zero `progress_m` while the
    /// catalog index stays put, so the route-identity check sees nothing change. A snapshot only
    /// answers for the axis ahead of its anchor, so keeping it would leave the tile blind (here:
    /// showing `--`) all the way back up to the old anchor.
    #[test]
    fn rewinding_past_the_anchor_re_arms() {
        let mut c = NextAhead::new();
        let placed = PoiCategorySet::only(PoiCategory::Water);
        c.reconcile(placed, true, Some(0), 8_000);
        // Nothing of this kind left this far along: a settled, *empty* answer for the tail.
        c.harvest(c.request().unwrap(), &[]);
        assert_eq!(c.request(), None, "settled");

        // The ride restarts on the same route (index unchanged, progress zeroed).
        c.reconcile(placed, true, Some(0), 0);
        assert_eq!(
            c.request(),
            Some(CorridorKey { filter: PoiCategorySet::only(PoiCategory::Water), anchor_m: 0 }),
            "progress back at the start re-takes — the old answer covered only the far tail"
        );
        c.harvest(c.request().unwrap(), &[poi(300, "Fontaine", WATER)]);
        assert_eq!(c.poi(PoiCategory::Water).unwrap().dist_along_m, 300, "and the tile has an answer again");
    }

    /// …but the matcher's backward slack (`BACK_SEGS`) must not cost a query: progress wobbling a
    /// few metres behind the anchor on GPS noise stays settled, right up to `REWIND_TOLERANCE_M`.
    #[test]
    fn matcher_jitter_behind_the_anchor_does_not_re_arm() {
        let mut c = NextAhead::new();
        let placed = PoiCategorySet::only(PoiCategory::Water);
        c.reconcile(placed, true, Some(0), 2_000);
        c.harvest(c.request().unwrap(), &[poi(2_400, "Fontaine", WATER)]);

        for back in [1, 5, 25, REWIND_TOLERANCE_M] {
            c.reconcile(placed, true, Some(0), 2_000 - back);
            assert_eq!(c.request(), None, "{back} m of re-match jitter is inside the tolerance");
        }
        c.reconcile(placed, true, Some(0), 2_000 - REWIND_TOLERANCE_M - 1);
        assert!(c.request().is_some(), "one metre past the tolerance is a real rewind");
    }

    /// `invalidate` is the seam a **same-index/new-bytes** replace needs: the route key doesn't
    /// move, so nothing else in here would notice, and the next reconcile must re-take.
    #[test]
    fn invalidate_drops_the_cache_under_an_unchanged_route_key() {
        let mut c = NextAhead::new();
        let placed = PoiCategorySet::only(PoiCategory::Water);
        c.reconcile(placed, true, Some(0), 0);
        c.harvest(c.request().unwrap(), &[poi(500, "Fontaine", WATER)]);
        assert!(c.poi(PoiCategory::Water).is_some());

        c.invalidate();
        assert_eq!(c.poi(PoiCategory::Water), None, "the old geometry's answer is gone");
        c.reconcile(placed, true, Some(0), 0);
        assert_eq!(
            c.request(),
            Some(CorridorKey { filter: PoiCategorySet::only(PoiCategory::Water), anchor_m: 0 }),
            "…and the very same route index re-queries"
        );
    }

    /// An unnamed POI caches its subtype label, so a tile never shows a blank caption.
    #[test]
    fn an_unnamed_poi_caches_its_subtype_label() {
        let mut c = NextAhead::new();
        let placed = PoiCategorySet::only(PoiCategory::Water);
        c.reconcile(placed, true, Some(0), 0);
        c.harvest(c.request().unwrap(), &[poi(700, "", WATER)]);
        assert_eq!(c.poi(PoiCategory::Water).unwrap().name.as_str(), "Drinking water");
    }
}
