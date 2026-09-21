//! Active-route state and route-following policy owned by [`NavigatorMachine`](super::NavigatorMachine).

use core::num::NonZeroUsize;

use obc_route::{Climbs, RouteReader, Waypoints};

use super::NavigatorMachine;

/// A route-catalog index stored as `index + 1`, so [`Option`] gets a compact empty state. Catalog
/// indices are bounded by [`crate::MAX_ROUTES`], so the nonzero form preserves every valid value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RouteIndex(NonZeroUsize);

impl RouteIndex {
    fn new(index: usize) -> Self {
        debug_assert!(index < crate::MAX_ROUTES);
        RouteIndex(NonZeroUsize::new(index + 1).expect("a route-catalog index is bounded"))
    }

    const fn get(self) -> usize {
        self.0.get() - 1
    }
}

/// A seam re-anchor waiting for the next tick with matching route geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SeamRequest {
    route: RouteIndex,
    anchor_m: u32,
}

/// The active route and every visible fact derived from following it. Screens borrow this value
/// from Navigator; they do not read a copied mirror from [`crate::Activity`].
#[derive(Debug, Clone, Copy, Default)]
pub struct RouteState {
    pub(crate) active_route: Option<usize>,
    pub(crate) route_total_m: u32,
    pub(crate) progress_m: u32,
    pub(crate) off_route: bool,
    pub(crate) dist_to_route_m: u32,
    pub(crate) active_climb: Option<usize>,
    pub(crate) next_waypoint: Option<usize>,
    pub(crate) waypoint_count: usize,
    seam_request: Option<SeamRequest>,
}

impl RouteState {
    pub(crate) const fn new() -> Self {
        RouteState {
            active_route: None,
            route_total_m: 0,
            progress_m: 0,
            off_route: false,
            dist_to_route_m: 0,
            active_climb: None,
            next_waypoint: None,
            waypoint_count: 0,
            seam_request: None,
        }
    }

    pub(super) fn apply_match(&mut self, result: obc_route::Match) {
        self.progress_m = result.progress_m;
        self.off_route = result.off_route;
        self.dist_to_route_m = result.dist_m;
    }

    fn request_seam(&mut self, route: usize, anchor_m: u32) {
        self.seam_request = Some(SeamRequest { route: RouteIndex::new(route), anchor_m });
    }

    fn remap_seam(&mut self, remap: &dyn Fn(usize) -> Option<usize>) {
        self.seam_request = self.seam_request.and_then(|request| {
            remap(request.route.get())
                .map(|route| SeamRequest { route: RouteIndex::new(route), anchor_m: request.anchor_m })
        });
    }

    #[cfg(test)]
    pub(super) fn assert_boot_state(&self) {
        assert!(self.active_route.is_none() && self.active_climb.is_none(), "no active route or climb");
        assert!(self.next_waypoint.is_none() && self.seam_request.is_none(), "no waypoint or seam request");
        assert_eq!(
            (self.route_total_m, self.progress_m, self.dist_to_route_m, self.waypoint_count),
            (0, 0, 0, 0),
            "all route counters start at zero"
        );
        assert!(!self.off_route, "an unloaded route is not off-route");
    }
}

/// Metres before a climb's base the "on climb" state arms. The matched `progress_m` jitters a few
/// metres each fix, so a raw interval lookup would toggle the Climb screen at the base or the
/// summit. Both margins are well under [`obc_route::MIN_LEN`], so two kept climbs' bands cannot
/// overlap.
const CLIMB_ENTER_MARGIN_M: u32 = 50;
/// Metres past a climb's summit the "on climb" state is held before it disarms.
const CLIMB_EXIT_MARGIN_M: u32 = 30;

/// The active-climb hysteresis, as a pure function of the climbs list, the matched progress and the
/// previous active index. While on climb `prev`, hold it until `progress` passes its summit plus
/// [`CLIMB_EXIT_MARGIN_M`]; otherwise arm the first climb whose entry band contains `progress`.
fn resolve_active_climb(climbs: &Climbs, progress: u32, prev: Option<usize>) -> Option<usize> {
    if let Some(i) = prev {
        if let Some(seg) = climbs.as_slice().get(i) {
            if progress <= seg.end_m.saturating_add(CLIMB_EXIT_MARGIN_M) {
                return Some(i);
            }
        }
    }
    climbs
        .as_slice()
        .iter()
        .position(|c| progress >= c.start_m.saturating_sub(CLIMB_ENTER_MARGIN_M) && progress <= c.end_m)
}

/// Metres a passed waypoint lingers as "next" before the index advances. GPS jitter around a
/// waypoint's position stays inside this band, so the resolved index cannot flap there.
pub(crate) const WAYPOINT_LINGER_M: u32 = 100;

/// The next-waypoint index, as a pure function of the resident table, the matched progress and the
/// previously-resolved index. The next waypoint is the first entry whose linger band is still open.
/// `prev` only keeps the index from regressing on a progress dip; `None` once every one is passed.
fn resolve_next_waypoint(wpts: &Waypoints, progress_m: u32, prev: Option<usize>) -> Option<usize> {
    let ahead = wpts.as_slice().iter().position(|w| progress_m < w.dist_along_m.saturating_add(WAYPOINT_LINGER_M));
    match ahead {
        // Past every waypoint's linger — the chip / fields go empty even if one was held.
        None => None,
        // Hold the furthest-reached index against a jittering cursor (never un-pass a waypoint).
        // A stale `prev` (≥ len, after a table shrink) falls through to `a`.
        Some(a) => match prev {
            Some(p) if p > a && p < wpts.len() => Some(p),
            _ => Some(a),
        },
    }
}

impl NavigatorMachine {
    pub(crate) fn route_state(&self) -> &RouteState {
        &self.following
    }

    #[cfg(test)]
    pub(crate) fn route_state_mut(&mut self) -> &mut RouteState {
        &mut self.following
    }

    #[cfg(test)]
    pub(crate) fn pending_seam(&self) -> bool {
        self.following.seam_request.is_some()
    }

    #[cfg(test)]
    pub(crate) fn climb_fill_count(&self) -> u32 {
        self.climb_fill_count
    }

    #[cfg(test)]
    pub(crate) fn cache_keys(&self) -> (Option<usize>, Option<usize>) {
        (self.climbs_route, self.waypoints_route)
    }

    /// Select or clear the active catalog route. Route-keyed caches reconcile on the next tick.
    pub(crate) fn set_active_route(&mut self, route: Option<usize>) {
        if self.select_after_checkpoint(route) {
            self.following.active_route = route;
        }
    }

    /// Recording recovery suspends guidance without cancelling a saved Assistant journey.
    pub(crate) fn suspend_for_recording_recovery(&mut self) {
        self.following.active_route = None;
    }

    pub(crate) fn replace_active_route(&mut self, route: usize) -> Option<usize> {
        let previous = self.following.active_route;
        self.set_active_route(Some(route));
        previous
    }

    pub(crate) fn request_seam(&mut self, route: usize, anchor_m: u32) {
        self.following.request_seam(route, anchor_m);
    }

    pub(crate) fn profile(&self) -> Option<&obc_route::Profile> {
        self.profile.as_ref()
    }

    pub(crate) fn climbs(&self) -> &Climbs {
        &self.climbs
    }

    #[cfg(test)]
    pub(crate) fn climbs_mut(&mut self) -> &mut Climbs {
        &mut self.climbs
    }

    pub(crate) fn climb_profile(&self) -> &obc_route::ClimbProfile {
        &self.climb_profile
    }

    pub(crate) fn waypoints(&self) -> &Waypoints {
        &self.waypoints
    }

    /// Start a fresh route-following pass for a new ride session while keeping the selected route.
    pub(crate) fn reset_ride(&mut self) {
        self.route_match.reset();
        self.following.seam_request = None;
        self.following.progress_m = 0;
        self.following.off_route = false;
        self.following.dist_to_route_m = 0;
        self.following.active_climb = None;
        self.following.next_waypoint = None;
    }

    /// Discard the matcher's forward-only floor when a ride session opens or closes.
    pub(crate) fn relock_matcher(&mut self) {
        self.route_match.reset();
    }

    /// The once-per-load route sync, run at the top of every tick. Returns whether the map must
    /// repaint. The climbs and waypoints caches build once per load and advance their build key only
    /// when the geometry is streamable: a `None` route keeps the old state and retries next tick.
    ///
    /// The matcher follows the navigated route, so a load or a swap re-locks it. The accumulators,
    /// the trail and the pace window follow the ride session, which is Recorder's.
    pub(crate) fn sync_route_state(&mut self, route: Option<&RouteReader>) -> bool {
        let mut dirty = false;
        if self.following.active_route != self.matched_route {
            // Deliberately do NOT clear a pending seam re-anchor here: a detour commit queues it
            // for the *just-adopted* spliced route, so this route-change edge is exactly the tick
            // it must survive into. Stale seams die on the request's own route-key check.
            self.route_match.reset();
            self.matched_route = self.following.active_route;
            dirty = true; // route load / swap repaints the route line + recenters
        }
        let route_total_before = self.following.route_total_m;
        self.following.route_total_m = route.map_or(0, |r| r.total_distance_m);
        if self.following.route_total_m != route_total_before {
            dirty = true;
        }

        if self.following.active_route != self.climbs_route {
            match (self.following.active_route, route) {
                (Some(_), Some(r)) => {
                    self.climbs = r.detect_climbs();
                    self.climbs_route = self.following.active_route;
                    self.following.active_climb = None; // a fresh list — re-derive the active climb on the next match
                }
                (None, _) => {
                    self.climbs = Climbs::new();
                    self.climbs_route = None;
                    self.following.active_climb = None;
                }
                (Some(_), None) => { /* geometry not yet streamable — keep the old state, retry next tick */ }
            }
        }

        // Load the route's named waypoints once per load, on the same streamable-geometry guard.
        // Loaded from the route start; a truncated table is slid forward in `update_next_waypoint`.
        if self.following.active_route != self.waypoints_route {
            match (self.following.active_route, route) {
                (Some(_), Some(r)) => {
                    self.waypoints = r.load_waypoints(0);
                    self.waypoints_route = self.following.active_route;
                    self.following.next_waypoint = None; // a fresh table — re-derive the next waypoint on the next match
                }
                (None, _) => {
                    self.waypoints = Waypoints::new();
                    self.waypoints_route = None;
                    self.following.next_waypoint = None;
                }
                (Some(_), None) => { /* geometry not yet streamable — keep the old table, retry next tick */ }
            }
        }
        self.following.waypoint_count = self.waypoints.len();
        if let Some(route) = route {
            dirty |= self.reconcile_visit(route);
        }
        dirty
    }

    /// Snap a fresh fix onto the active route: run the matcher and store the result on
    /// [`RouteState`]. Called once per fresh fix, never on a dropout, so progress is not re-derived
    /// from a stale position.
    pub(crate) fn match_fix(&mut self, fix: obc_ports::Fix, route: &RouteReader) {
        let m = self.route_match.update_to(fix.lon, fix.lat, route, self.visit_ceiling().unwrap_or(u32::MAX));
        self.following.apply_match(m);
        self.advance_visit((fix.lon, fix.lat), route);
    }

    /// A fresh fix went unmatched. The Recalculating freeze holds the matcher for the length of a
    /// route search, which is seconds rather than one fix, so arm a one-shot wide re-lock: the tight
    /// on-route window is sized for one fix's travel, and a rider who rode past it would otherwise
    /// come out of the freeze with a false off-route chip and frozen progress.
    pub(crate) fn note_unmatched_fix(&mut self) {
        self.route_match.relock_wide();
    }

    /// Apply a queued seam re-anchor once matching route geometry is available: install matcher
    /// progress and the forward-only floor at the splice seam. A transient `None` reader leaves the
    /// request queued; a route-key mismatch drops it rather than applying the distance to different
    /// geometry.
    pub(crate) fn apply_pending_seam(&mut self, route: Option<&RouteReader>) -> bool {
        let Some(req) = self.following.seam_request else { return false };
        if self.following.active_route != Some(req.route.get()) {
            self.following.seam_request = None;
            return false;
        }
        let Some(route) = route else { return false };
        if let Some(pos) = self.route_match.set_progress_floor(route, req.anchor_m) {
            self.following.seam_request = None;
            self.following.apply_match(obc_route::Match { progress_m: pos.progress_m, off_route: false, dist_m: 0 });
            self.following.active_climb = None;
            self.following.next_waypoint = None;
            true
        } else {
            // A transient decode failure is retryable. Keep both the request and the old visible
            // progress; clearing one without moving the matcher would split the two anchors.
            false
        }
    }

    /// Recompute the active climb from the freshly-matched progress, and refill the resident
    /// [`climb_profile`](NavigatorMachine::climb_profile) only on a new climb entry, never per frame,
    /// because the fill streams the climb's chunks. Returns the `(prev, next)` transition when the
    /// active climb changed.
    ///
    /// Off-route, the matcher freezes `progress_m`, so leaving the route mid-climb keeps the current
    /// climb rather than snapping it away on a frozen cursor.
    pub(crate) fn update_active_climb(&mut self, route: &RouteReader) -> Option<(Option<usize>, Option<usize>)> {
        // Off-route freezes the cursor, so keep whatever climb we were on.
        if self.following.off_route {
            return None;
        }
        let prev = self.following.active_climb;
        let next = resolve_active_climb(&self.climbs, self.following.progress_m, prev);
        if next == prev {
            return None; // unchanged — no refill, no SD read.
        }
        self.following.active_climb = next;
        if let Some(seg) = next.and_then(|i| self.climbs.as_slice().get(i)) {
            self.climb_profile.fill(route, seg);
            #[cfg(test)]
            {
                self.climb_fill_count += 1;
            }
        }
        Some((prev, next))
    }

    /// Recompute the next waypoint from the freshly-matched progress, and slide a truncated table's
    /// window forward when the rider passes its tail. Returns whether the next waypoint changed.
    ///
    /// The re-window is gated on [`truncated`](obc_route::Waypoints), so a normal route never
    /// re-streams, and it starts strictly past the old window, so it cannot re-fire next tick.
    /// Off-route, `progress_m` is frozen, so the index self-freezes and is left alone.
    pub(crate) fn update_next_waypoint(&mut self, route: &RouteReader) -> bool {
        if self.following.off_route {
            return false;
        }
        // Slide a truncated window forward once its whole resident span is behind the rider.
        if self.waypoints.truncated {
            if let Some(last) = self.waypoints.as_slice().last() {
                if self.following.progress_m >= last.dist_along_m.saturating_add(WAYPOINT_LINGER_M) {
                    self.waypoints = route.load_waypoints(self.following.progress_m);
                    self.following.next_waypoint = None; // the window slid — re-derive against it below
                }
            }
        }
        self.following.waypoint_count = self.waypoints.len();
        let prev = self.following.next_waypoint;
        let next = resolve_next_waypoint(&self.waypoints, self.following.progress_m, prev);
        if next != prev {
            self.following.next_waypoint = next;
            return true;
        }
        false
    }

    /// Build once per active route at render time. A missing reader clears stale geometry but
    /// leaves the build pending; an unloaded route has no profile.
    pub(crate) fn refresh_route_profile(&mut self, route: Option<&RouteReader>) {
        if self.following.active_route != self.profile_route {
            self.profile = self.following.active_route.and(route).map(|r| r.elevation_profile());
            self.profile_route = self.profile.as_ref().and(self.following.active_route);
        }
    }

    /// Drop everything derived from the active route's geometry — matcher lock, elevation profile,
    /// climbs, waypoints and the match-derived readouts in [`RouteState`] — so the next tick and
    /// render re-derive it. New bytes under a kept route id are exactly the case the same-id remap
    /// would otherwise treat as unchanged state. The recording session is untouched.
    pub(crate) fn drop_route_derived_state(&mut self) {
        // `reset` also clears any wide re-lock armed by a freeze: an unstarted matcher scans the
        // whole route on its next fix, which is wider still.
        self.route_match.reset();
        self.matched_route = None;
        self.profile = None;
        self.profile_route = None;
        self.climbs = Climbs::new();
        self.climbs_route = None;
        self.following.active_climb = None;
        self.waypoints = Waypoints::new();
        self.waypoints_route = None;
        self.following.next_waypoint = None;
        self.following.waypoint_count = 0;
        self.following.progress_m = 0;
        self.following.off_route = false;
        self.following.dist_to_route_m = 0;
        self.following.seam_request = None;
    }

    /// Re-point every route-keyed cache after a catalog replacement: each build key follows its
    /// route's identity through `remap`, and a key whose route vanished drops its cache. The
    /// active-route remap lives here too, so a caller cannot forget the matcher reset.
    pub(crate) fn remap_route_keys(&mut self, remap: &dyn Fn(usize) -> Option<usize>) {
        // When the identity survives, the navigated route and its caches all move together, so
        // nothing resets. When it vanished, navigation unloads and the per-route state goes with it.
        let old_active = self.following.active_route;
        self.following.active_route = old_active.and_then(remap);
        // A queued seam re-anchor follows the same durable route identity as `active_route`, or is
        // cancelled if that route vanished. So does Navigator's undelivered detour request.
        self.following.remap_seam(remap);
        if old_active.is_some() && self.following.active_route.is_none() {
            self.route_match.reset();
        }
        self.matched_route = self.matched_route.and_then(remap);
        let old_profile = self.profile_route;
        self.profile_route = old_profile.and_then(remap);
        if old_profile.is_some() && self.profile_route.is_none() {
            self.profile = None;
        }
        // Clearing the active-climb state with the cache keeps a stale "on climb" flag from
        // stranding the rider on a gone route.
        let old_climbs = self.climbs_route;
        self.climbs_route = old_climbs.and_then(remap);
        if old_climbs.is_some() && self.climbs_route.is_none() {
            self.climbs = Climbs::new();
            self.following.active_climb = None;
        }
        let old_wpts = self.waypoints_route;
        self.waypoints_route = old_wpts.and_then(remap);
        if old_wpts.is_some() && self.waypoints_route.is_none() {
            self.waypoints = Waypoints::new();
            self.following.next_waypoint = None;
            self.following.waypoint_count = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::support::wpts;

    #[test]
    fn route_profile_retries_missing_readers_without_repeating_completed_reads() {
        use core::cell::Cell;
        use obc_formats::io::{ByteSource, Error, SliceSource};

        struct Counted<'a> {
            bytes: &'a [u8],
            reads: Cell<usize>,
        }
        impl ByteSource for Counted<'_> {
            fn read_at(&self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
                self.reads.set(self.reads.get() + 1);
                SliceSource(self.bytes).read_at(offset, out)
            }
            fn len(&self) -> u64 {
                self.bytes.len() as u64
            }
        }
        let bytes = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/sources/sim-grimsel/routes/grimsel-climb.obcr"
        ));
        let source = Counted { bytes, reads: Cell::new(0) };
        let index = obc_route::RouteIndex::read(&source).unwrap();
        let route = RouteReader::new(&index, &source);
        source.reads.set(0);
        let mut navigator = NavigatorMachine::new();

        navigator.set_active_route(Some(0));
        navigator.refresh_route_profile(None);
        assert!(navigator.profile().is_none());
        navigator.refresh_route_profile(Some(&route));
        assert!(navigator.profile().is_some());
        let one_build = source.reads.get();
        assert!(one_build > 0, "the available reader supplies actual geometry");
        navigator.refresh_route_profile(None);
        navigator.refresh_route_profile(Some(&route));
        assert_eq!(source.reads.get(), one_build, "a completed profile is cached");

        navigator.set_active_route(Some(1));
        navigator.refresh_route_profile(None);
        assert!(navigator.profile().is_none(), "the previous route's profile must not remain visible");
        navigator.refresh_route_profile(Some(&route));
        assert!(navigator.profile().is_some());
        assert_eq!(source.reads.get(), 2 * one_build);

        navigator.drop_route_derived_state();
        navigator.refresh_route_profile(None);
        assert!(navigator.profile().is_none());
        navigator.refresh_route_profile(Some(&route));
        assert!(navigator.profile().is_some());
        assert_eq!(source.reads.get(), 3 * one_build, "same-ID replacement still rebuilds");

        navigator.set_active_route(None);
        navigator.refresh_route_profile(Some(&route));
        assert!(navigator.profile().is_none(), "unloading clears the profile even if a reader remains");
        assert_eq!(source.reads.get(), 3 * one_build);
    }

    /// The placement path must land exactly the state the by-value path builds.
    #[test]
    fn init_in_place_matches_new() {
        NavigatorMachine::new().assert_boot_state();

        let mut slot = core::mem::MaybeUninit::<NavigatorMachine>::uninit();
        // SAFETY: `slot` is a valid, aligned, exclusively-owned Navigator region.
        let placed = unsafe {
            NavigatorMachine::init_in_place(slot.as_mut_ptr());
            slot.assume_init_ref()
        };
        placed.assert_boot_state();
    }

    use obc_route::ClimbSeg;

    /// A `ClimbSeg` over `[start_m, end_m]` — the other fields don't affect the interval hysteresis.
    fn seg(start_m: u32, end_m: u32) -> ClimbSeg {
        ClimbSeg {
            start_m,
            end_m,
            base_ele_m: 0,
            top_ele_m: (end_m - start_m) as i16,
            gain_m: (end_m - start_m) as u16,
            avg_grade_pct: 5,
        }
    }

    fn climbs(spans: &[(u32, u32)]) -> Climbs {
        let mut c = Climbs::new();
        for &(s, e) in spans {
            c.0.push(seg(s, e)).unwrap();
        }
        c
    }

    /// Below the entry band there is no active climb; the climb arms slightly before the base and
    /// stays armed through the interval.
    #[test]
    fn resolve_arms_a_climb_at_its_entry_band() {
        let cs = climbs(&[(1000, 3000)]);
        assert_eq!(resolve_active_climb(&cs, 800, None), None);
        // Just outside the entry band (base 1000 − 50 = 950): still nothing.
        assert_eq!(resolve_active_climb(&cs, 949, None), None);
        // Inside the entry band, before the base: armed early (the point of the enter margin).
        assert_eq!(resolve_active_climb(&cs, 960, None), Some(0));
        assert_eq!(resolve_active_climb(&cs, 2000, None), Some(0));
    }

    /// Exit: while on a climb it's *held* past the summit by `CLIMB_EXIT_MARGIN_M`, then disarms.
    #[test]
    fn resolve_holds_past_the_summit_then_exits() {
        let cs = climbs(&[(1000, 3000)]);
        assert_eq!(resolve_active_climb(&cs, 3000, Some(0)), Some(0));
        // Within the exit band (summit 3000 + 30 = 3030): held.
        assert_eq!(resolve_active_climb(&cs, 3025, Some(0)), Some(0));
        // Past the exit band: disarmed (no next climb to take over).
        assert_eq!(resolve_active_climb(&cs, 3040, Some(0)), None);
    }

    /// The flap guard: jitter around the base boundary (the matcher wobbling progress a few metres
    /// either way of the entry point) must not toggle the active climb once it's armed.
    #[test]
    fn resolve_does_not_flap_at_a_boundary() {
        let cs = climbs(&[(1000, 3000)]);
        let mut active = resolve_active_climb(&cs, 1000, None);
        assert_eq!(active, Some(0));
        // Progress jitters back below the base, inside the entry band, so the climb stays armed.
        for p in [995u32, 980, 970, 990, 1005, 998] {
            active = resolve_active_climb(&cs, p, active);
            assert_eq!(active, Some(0), "jitter around the base must not drop the active climb");
        }
        // …and jitter around the *summit* likewise doesn't flap (held by the exit band).
        active = resolve_active_climb(&cs, 3000, active);
        for p in [3005u32, 2998, 3010, 2995, 3020] {
            active = resolve_active_climb(&cs, p, active);
            assert_eq!(active, Some(0), "jitter around the summit must not drop the active climb");
        }
    }

    /// Leaving climb 0's exit band hands straight over to climb 1, whose entry band already contains
    /// progress: one clean transition, never a gap of `None`.
    #[test]
    fn resolve_hands_over_between_adjacent_climbs() {
        let cs = climbs(&[(1000, 3000), (3000, 5000)]);
        assert_eq!(resolve_active_climb(&cs, 3010, Some(0)), Some(0));
        // Past climb 0's exit band: re-arms straight onto climb 1.
        assert_eq!(resolve_active_climb(&cs, 3040, Some(0)), Some(1));
    }

    /// A stale index (the list shrank under the previous active climb) does not strand the resolver:
    /// it re-arms from scratch.
    #[test]
    fn resolve_recovers_from_a_stale_index() {
        let cs = climbs(&[(1000, 3000)]);
        // prev = 5, but only one climb exists and progress is nowhere near it.
        assert_eq!(resolve_active_climb(&cs, 200, Some(5)), None);
    }

    /// The index advances at exactly `dist + WAYPOINT_LINGER_M` and not one metre before.
    #[test]
    fn resolve_next_advances_exactly_at_the_linger() {
        let w = wpts(&[(1_000, "A"), (2_000, "B")]);
        // Before A, and anywhere in A's linger band [1000, 1100): A is next.
        assert_eq!(resolve_next_waypoint(&w, 0, None), Some(0));
        assert_eq!(resolve_next_waypoint(&w, 1_000, None), Some(0));
        assert_eq!(resolve_next_waypoint(&w, 1_099, Some(0)), Some(0));
        // Exactly at dist + 100: A's band closes, B is next.
        assert_eq!(resolve_next_waypoint(&w, 1_100, Some(0)), Some(1));
    }

    /// Jitter around a waypoint's own position never flaps the index.
    #[test]
    fn resolve_next_does_not_flap_around_a_waypoint() {
        let w = wpts(&[(1_000, "A"), (2_000, "B")]);
        let mut next = resolve_next_waypoint(&w, 970, None);
        assert_eq!(next, Some(0));
        for p in [1_005u32, 980, 1_030, 995, 1_020, 970] {
            next = resolve_next_waypoint(&w, p, next);
            assert_eq!(next, Some(0), "jitter around A's position must not advance the index");
        }
        // …and a dip back below the advance boundary after passing it doesn't regress the index.
        next = resolve_next_waypoint(&w, 1_100, next);
        assert_eq!(next, Some(1));
        for p in [1_080u32, 1_060, 1_090] {
            next = resolve_next_waypoint(&w, p, next);
            assert_eq!(next, Some(1), "a progress dip must not step back onto a passed waypoint");
        }
    }

    /// Past the last waypoint's linger the index is `None` — the chip / fields go empty.
    #[test]
    fn resolve_next_is_none_past_the_last() {
        let w = wpts(&[(1_000, "A"), (2_000, "B")]);
        assert_eq!(resolve_next_waypoint(&w, 2_099, Some(1)), Some(1));
        // Past B + 100: nothing ahead.
        assert_eq!(resolve_next_waypoint(&w, 2_100, Some(1)), None);
        assert_eq!(resolve_next_waypoint(&w, 9_999, Some(1)), None);
    }

    /// A fresh route with no prior index starts at the first waypoint ahead.
    #[test]
    fn resolve_next_fresh_route_starts_at_the_first_ahead() {
        let w = wpts(&[(1_000, "A"), (2_000, "B"), (3_000, "C")]);
        assert_eq!(resolve_next_waypoint(&w, 0, None), Some(0));
        // Starting past A's linger picks B (the first still-ahead), not A.
        assert_eq!(resolve_next_waypoint(&w, 1_500, None), Some(1));
        assert_eq!(resolve_next_waypoint(&Waypoints::new(), 0, None), None);
    }
}
