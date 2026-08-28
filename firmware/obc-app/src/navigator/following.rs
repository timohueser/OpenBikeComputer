//! Active-route state and route-following policy owned by [`NavigatorMachine`](super::NavigatorMachine).

use core::num::NonZeroUsize;

use obc_route::{Climbs, RouteReader, Waypoints};

use super::NavigatorMachine;

/// A route-catalog index stored as `index + 1`. Catalog indices are bounded by
/// [`crate::MAX_ROUTES`], so the nonzero representation preserves every valid value and gives
/// [`Option`] a compact empty state.
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

    fn apply_match(&mut self, result: obc_route::Match) {
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

/// Enter/exit hysteresis for [`NavigatorMachine::update_active_climb`] — the margins that turn the raw
/// interval lookup ([`Climbs::active_at`], exact detected geometry, no slack) into a flap-free "on
/// a climb now" state.
///
/// **Enter early, exit late.** The raw intervals are the detected trough→summit, but the matched
/// `progress_m` jitters a few metres either way of the true position each fix (matcher snap +
/// smoothing). Without slack a rider straddling the base or the summit would toggle the Climb
/// screen on and off between consecutive fixes. So we **arm** the climb once progress reaches
/// [`CLIMB_ENTER_MARGIN_M`] *before* the base and **hold** it until progress passes
/// [`CLIMB_EXIT_MARGIN_M`] *past* the summit — an on-then-off band wider than the jitter, biased so
/// the panel appears slightly ahead of the ramp (useful) and lingers slightly past the crest
/// (avoids a premature dismissal on the false-flat over the top).
///
/// The margins are asymmetric on purpose: showing the climb a touch early is welcome, and holding a
/// touch past the crest reads better than snapping away the instant `progress == end_m`. Both are
/// well under [`obc_route::MIN_LEN`] (400 m), so they can't make one climb's exit band overlap the
/// next climb's entry band on any kept climb.
const CLIMB_ENTER_MARGIN_M: u32 = 50;
/// Distance (m) past a climb's summit the "on climb" state is held before it disarms — see
/// [`CLIMB_ENTER_MARGIN_M`].
const CLIMB_EXIT_MARGIN_M: u32 = 30;

/// The active-climb hysteresis, as a **pure** function of the climbs list, the matched progress,
/// and the previous active index — the whole flap-guard policy in one testable place (the
/// `NavigatorMachine::update_active_climb` wrapper only adds the off-route freeze and the once-per-entry
/// refill).
///
/// While *on* climb `prev`, hold it until `progress` passes its summit + [`CLIMB_EXIT_MARGIN_M`]
/// (or the index went stale — a shrunk list after a swap); otherwise re-arm. To *arm* a climb,
/// `progress` must have reached within [`CLIMB_ENTER_MARGIN_M`] of its base and not yet passed its
/// summit — the first such climb in route order (they're non-overlapping and the margins are far
/// under [`obc_route::MIN_LEN`], so the bands can't collide on kept climbs). The exit band is wider
/// on the far side and the entry band on the near side, so a rider straddling a boundary can't
/// toggle the state between consecutive fixes.
fn resolve_active_climb(climbs: &Climbs, progress: u32, prev: Option<usize>) -> Option<usize> {
    // While committed to a climb, hold it across its exit band before reconsidering.
    if let Some(i) = prev {
        if let Some(seg) = climbs.as_slice().get(i) {
            if progress <= seg.end_m.saturating_add(CLIMB_EXIT_MARGIN_M) {
                return Some(i);
            }
        }
    }
    // Not held: arm the first climb whose entry band (base − enter margin ..= summit) contains
    // progress.
    climbs
        .as_slice()
        .iter()
        .position(|c| progress >= c.start_m.saturating_sub(CLIMB_ENTER_MARGIN_M) && progress <= c.end_m)
}

/// Distance (m) a passed waypoint **lingers** as "next" before the index advances — distance
/// hysteresis, not time. GPS jitter around a waypoint's position stays inside this band, so the
/// resolved index can't flap there; the shown distance-to-go clamps to 0 through the linger. Matches
/// the epic's 100 m pass-linger.
pub(crate) const WAYPOINT_LINGER_M: u32 = 100;

/// The next-waypoint index as a **pure** function of the resident table, the matched progress, and
/// the previously-resolved index — the waypoint sibling of [`resolve_active_climb`]. The
/// [`update_next_waypoint`](NavigatorMachine::update_next_waypoint) wrapper adds the off-route freeze and
/// the re-window; this is the whole "which waypoint is next?" policy in one testable place.
///
/// The next waypoint is the **first entry still ahead**: one whose linger band is open,
/// `progress < dist_along_m + WAYPOINT_LINGER_M`. A passed waypoint therefore lingers
/// [`WAYPOINT_LINGER_M`] before the index moves on, so jitter around a waypoint can't flap it. `prev`
/// only keeps the index from *regressing* on a progress dip (jitter at the far, advance edge of the
/// band) — it never steps back onto a waypoint already passed while progress oscillates. `None` once
/// the rider is past every waypoint's linger.
fn resolve_next_waypoint(wpts: &Waypoints, progress_m: u32, prev: Option<usize>) -> Option<usize> {
    let ahead = wpts.as_slice().iter().position(|w| progress_m < w.dist_along_m.saturating_add(WAYPOINT_LINGER_M));
    match ahead {
        // Past every waypoint's linger — the chip / fields go empty even if one was held.
        None => None,
        // Hold the furthest-reached index against a jittering cursor (never un-pass a waypoint);
        // otherwise take the first still-ahead one. A stale `prev` (≥ len, after a table shrink)
        // falls through to `a`.
        Some(a) => match prev {
            Some(p) if p > a && p < wpts.len() => Some(p),
            _ => Some(a),
        },
    }
}

/// Progress the rider must cover before the route heading is re-derived (two chunk decodes).
/// Sized against [`TRAVEL_CHORD_M`](crate::weather::TRAVEL_CHORD_M): a kilometre-long chord cannot
/// swing within a few tens of metres, so anything finer only re-reads the card — and a stationary
/// rider's GPS jitter must never do that at all.
pub(crate) const HEADING_MOVE_M: u32 = 50;

impl NavigatorMachine {
    /// The live route-following view screens render.
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

    #[cfg(test)]
    pub(crate) fn set_travel_deg_for_test(&mut self, travel_deg: Option<f32>) {
        self.travel_deg = travel_deg;
    }

    /// Select or clear the active catalog route. Route-keyed caches reconcile on the next tick.
    pub(crate) fn set_active_route(&mut self, route: Option<usize>) {
        self.following.active_route = route;
    }

    /// Select a route and return the previous selection.
    pub(crate) fn replace_active_route(&mut self, route: usize) -> Option<usize> {
        self.following.active_route.replace(route)
    }

    /// Queue a seam re-anchor for the newly committed route.
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

    #[cfg(test)]
    pub(crate) fn waypoints_mut(&mut self) -> &mut Waypoints {
        &mut self.waypoints
    }

    pub(crate) fn travel_deg(&self) -> Option<f32> {
        self.travel_deg
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
    /// repaint (a route line appeared/vanished, the matcher re-locked).
    ///
    /// A new **ride session** re-locks the matcher too, and that is Recorder's edge rather than a
    /// key held here — see [`reset_ride`](NavigatorMachine::reset_ride).
    ///
    /// - The **matcher** follows the *navigated route*: a load or a "Swap route only" re-locks it.
    /// - The **accumulators, trail and pace window** follow the *ride session*, which is Recorder's:
    ///   the pass applies them on its session edge. A swap keeps the session, so it keeps them.
    /// - `route_total_m` mirrors the active route's length for the riding views (0 when none
    ///   loaded). A change here means the *drawable* route appeared or vanished — a load, or a
    ///   transient SD glitch recovering where the geometry becomes streamable a frame or two later.
    /// - The **climbs** and **waypoints** caches build once per load — climbs here in the tick
    ///   (not render) because [`update_active_climb`](NavigatorMachine::update_active_climb) needs the
    ///   list before the fix is matched. Only advance a build key when the geometry is actually
    ///   streamable: a `None` route (idle, or a transient SD glitch) leaves the old state in place
    ///   and retries next tick, rather than latching an empty result for the route.
    pub(crate) fn sync_route_state(&mut self, route: Option<&RouteReader>) -> bool {
        let mut dirty = false;
        if self.following.active_route != self.matched_route {
            // Deliberately do NOT clear a pending seam re-anchor here: a detour commit queues it
            // for the *just-adopted* spliced route, so this route-change edge is exactly the tick
            // it must survive into. Stale seams die on the request's own route-key check.
            self.route_match.reset();
            self.matched_route = self.following.active_route;
            // The old route's tangent means nothing on the new line (WX12) — neutral until the
            // next fix matches.
            self.travel_deg = None;
            self.travel_at_m = None;
            dirty = true; // route load / swap repaints the route line + recenters
        }
        let route_total_before = self.following.route_total_m;
        self.following.route_total_m = route.map_or(0, |r| r.total_distance_m);
        if self.following.route_total_m != route_total_before {
            dirty = true;
        }

        // Segment the route's climbs once per load — the twin of the elevation-profile rebuild.
        if self.following.active_route != self.climbs_route {
            match (self.following.active_route, route) {
                (Some(_), Some(r)) => {
                    self.climbs = r.detect_climbs();
                    self.climbs_route = self.following.active_route;
                    self.following.active_climb = None; // a fresh list — re-derive the active climb on the next match
                }
                (None, _) => {
                    // The route unloaded: drop the climbs and the on-climb state.
                    self.climbs = Climbs::new();
                    self.climbs_route = None;
                    self.following.active_climb = None;
                }
                (Some(_), None) => { /* geometry not yet streamable — keep the old state, retry next tick */ }
            }
        }

        // Load the route's named waypoints once per load, alongside the climbs above and on the
        // same streamable-geometry guard. Loaded from the route start (`min_dist_m = 0`); a
        // truncated table is slid forward later, in `update_next_waypoint`, not here.
        if self.following.active_route != self.waypoints_route {
            match (self.following.active_route, route) {
                (Some(_), Some(r)) => {
                    self.waypoints = r.load_waypoints(0);
                    self.waypoints_route = self.following.active_route;
                    self.following.next_waypoint = None; // a fresh table — re-derive the next waypoint on the next match
                }
                (None, _) => {
                    // The route unloaded: drop the table and the next-waypoint state.
                    self.waypoints = Waypoints::new();
                    self.waypoints_route = None;
                    self.following.next_waypoint = None;
                }
                (Some(_), None) => { /* geometry not yet streamable — keep the old table, retry next tick */ }
            }
        }
        self.following.waypoint_count = self.waypoints.len();
        dirty
    }

    /// Snap a fresh fix onto the active route: run the matcher and store the result on
    /// [`RouteState`]. Called once per fresh fix (never on a dropout, so progress is not re-derived
    /// from a stale position).
    pub(crate) fn match_fix(&mut self, fix: obc_ports::Fix, route: &RouteReader) {
        let m = self.route_match.update(fix.lon, fix.lat, route);
        self.following.apply_match(m);
    }

    /// A fresh fix went **unmatched** — the Recalculating freeze (#1146 P2) holds the matcher for
    /// the length of a route search, which is seconds, not one fix. Arm a one-shot wide re-lock so
    /// the first match after the freeze reaches wherever the rider actually got to: the tight
    /// on-route window is sized for one fix's travel, and a rider who rode past it would otherwise
    /// come out of the freeze with a false off-route chip and frozen progress.
    ///
    /// **The freezes this covers are the ones that end *without* new geometry** — a cancel, a
    /// `NoPath`/`Exhausted` answer, a detour's terminal edge. A search that *succeeds* never needs
    /// it: committing the result runs `drop_route_derived_state`, and its `RouteMatch::reset`
    /// clears this flag along with the rest of the lock, leaving the matcher unstarted so the next
    /// fix scans the whole new route regardless. This is for the rider who was shown
    /// "Recalculating" and then handed back the route they were already riding.
    pub(crate) fn note_unmatched_fix(&mut self) {
        self.route_match.relock_wide();
    }

    /// Apply a queued seam re-anchor (#882) once matching route geometry is available: install
    /// matcher progress + the forward-only floor at the splice seam. Returns `true` when the
    /// matcher/progress floor moved; a transient `None` reader leaves the request queued, while a
    /// route-key mismatch drops it rather than applying the distance to different geometry.
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

    /// Recompute the active climb from the freshly-matched progress, applying
    /// enter/exit hysteresis over the raw [`Climbs::active_at`] lookup, and refill the resident
    /// [`climb_profile`](NavigatorMachine::climb_profile) detail buffer **only on a new climb entry**
    /// (never per frame — the fill streams the climb's chunks).
    ///
    /// **Hysteresis.** The raw intervals carry no slack, so this widens them per the current state:
    /// while *off* a climb, a climb arms once progress reaches within [`CLIMB_ENTER_MARGIN_M`] of
    /// its base; while *on* a climb, it stays that climb until progress passes
    /// [`CLIMB_EXIT_MARGIN_M`] past its summit (or the rider has clearly moved onto a *different*
    /// climb's core interval). That asymmetric band is wider than the matcher's per-fix jitter, so
    /// straddling a boundary can't flap the on-climb state between consecutive fixes.
    ///
    /// **Off-route.** A stale match freezes `progress_m` (the matcher holds it while off-route), so
    /// leaving the route mid-climb *keeps* the current climb rather than snapping it away on a
    /// frozen cursor — the panel stays put until the rider rejoins and progress moves again. Only an
    /// explicit clear path (route swap/unload/replace) drops it.
    ///
    /// Called on each matched fix with the live route reader (the source the refill reads); a
    /// no-op that touches no SD when the active climb is unchanged. Returns the `(prev, next)`
    /// transition when the active climb **changed** (the caller repaints and runs the C5
    /// auto-switch off the same edge), `None` when unchanged.
    pub(crate) fn update_active_climb(&mut self, route: &RouteReader) -> Option<(Option<usize>, Option<usize>)> {
        // Off-route freezes the cursor, so keep whatever climb we were on — don't recompute against
        // a stale progress. `apply_match` leaves `progress_m` frozen while off-route.
        if self.following.off_route {
            return None;
        }
        let prev = self.following.active_climb;
        let next = resolve_active_climb(&self.climbs, self.following.progress_m, prev);
        if next == prev {
            return None; // unchanged — no refill, no SD read.
        }
        self.following.active_climb = next;
        // Refill the single resident detail buffer for the new climb — only here, on the transition,
        // so a fix that stays on the same climb never re-reads the card.
        if let Some(seg) = next.and_then(|i| self.climbs.as_slice().get(i)) {
            self.climb_profile.fill(route, seg);
            #[cfg(test)]
            {
                self.climb_fill_count += 1;
            }
        }
        Some((prev, next))
    }

    /// Recompute the next waypoint from the freshly-matched progress via the pure
    /// [`resolve_next_waypoint`], and slide a truncated table's window forward when the rider passes
    /// its tail — the waypoint twin of [`update_active_climb`](NavigatorMachine::update_active_climb).
    ///
    /// **Off-route.** `apply_match` freezes `progress_m` off-route, so the index self-freezes; like
    /// the climb resolver, just don't fight that — return and hold whatever was next. (The chip is
    /// hidden off-route anyway; the along-route distance is meaningless there.)
    ///
    /// **Re-window on exhaustion.** A file with more than [`MAX_WAYPOINTS`](obc_route::MAX_WAYPOINTS)
    /// named waypoints loads only the first window and flags [`truncated`](obc_route::Waypoints).
    /// Once the rider has passed the resident tail (its linger included), reload from the current
    /// progress so the far waypoints keep tracking. Gated on `truncated`, so a normal route never
    /// re-streams; and the reload starts strictly past the old window (all its entries sit at
    /// `dist < progress`), so it can't re-fire on the next tick.
    ///
    /// Called on each matched fix; touches SD only on the rare re-window. Returns whether the next
    /// waypoint changed (the caller repaints the chip / fields).
    pub(crate) fn update_next_waypoint(&mut self, route: &RouteReader) -> bool {
        // Off-route freezes progress, so the resolved index freezes with it — keep what we had.
        if self.following.off_route {
            return false;
        }
        // Slide a truncated window forward once its whole resident span (last entry + linger) is
        // behind the rider — see the re-window note above.
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
            return true; // the next waypoint changed — the chip / fields must repaint
        }
        false
    }

    /// Rebuild the cached elevation profile when the active route changed — it streams every
    /// chunk, so it's built once on load, never per frame; clears when no route is loaded. Run at
    /// render (the one place the host guarantees a live reader for the frame).
    pub(crate) fn refresh_route_profile(&mut self, route: Option<&RouteReader>) {
        if self.following.active_route != self.profile_route {
            self.profile = route.map(|r| r.elevation_profile());
            self.profile_route = self.following.active_route;
        }
    }

    /// Drop **everything derived from the active route's geometry** — matcher lock, elevation
    /// profile, climbs (+ on-climb state), waypoints (+ next-waypoint), and the match-derived
    /// readouts in [`RouteState`] — so the next tick/render re-derives all of it from the reopened
    /// geometry. The forced-adoption discipline shared by a committed route plan (new bytes under
    /// the reserved nav id) and an active-replace upload (new bytes under a kept id): the same-id
    /// remap deliberately preserves same-id state, and these are exactly the cases where that
    /// preservation would carry stale state onto new geometry. The recording session is untouched.
    pub(crate) fn drop_route_derived_state(&mut self) {
        // `reset` also clears any wide re-lock armed by a freeze (`note_unmatched_fix`), and should:
        // an unstarted matcher scans the whole route on its next fix, which is wider still. That is
        // why the wide window is only ever spent on a freeze that ended without new geometry.
        self.route_match.reset();
        self.matched_route = None; // tick re-locks the matcher from the current fix
        self.profile = None;
        self.profile_route = None; // the next render rebuilds from the reopened geometry
        self.climbs = Climbs::new();
        self.climbs_route = None; // the next tick re-segments from the reopened geometry
        self.following.active_climb = None;
        self.waypoints = Waypoints::new();
        self.waypoints_route = None; // the next tick re-loads from the reopened geometry
        self.following.next_waypoint = None;
        self.following.waypoint_count = 0;
        self.following.progress_m = 0;
        self.following.off_route = false;
        self.following.dist_to_route_m = 0;
        self.following.seam_request = None;
        // The route tangent was measured on the old geometry — neutral until the next fix
        // re-derives it (WX12). The speed window survives: the rider's pace is route-agnostic.
        self.travel_deg = None;
        self.travel_at_m = None;
    }

    /// Update the WX12 travel direction from this tick's fresh fix (see
    /// [`travel_deg`](NavigatorMachine::travel_deg) — the route's general heading, or neutral). Runs
    /// after the matcher, so [`RouteState`] carries this fix's match. The heading recomputes only when
    /// the rider moved ≥ [`HEADING_MOVE_M`] along the route (two `position_at` chunk decodes,
    /// fix-cadence-bounded).
    pub(crate) fn update_travel(&mut self, route: Option<&RouteReader>) {
        let on_route =
            route.is_some() && self.following.active_route.is_some() && self.started() && !self.following.off_route;
        if on_route {
            let route = route.unwrap();
            let moved = self.travel_at_m.is_none_or(|at| self.following.progress_m.abs_diff(at) >= HEADING_MOVE_M);
            if !moved && self.travel_deg.is_some() {
                return; // held heading (stopped, or sub-hysteresis creep)
            }
            if let Some(deg) = crate::weather::route_heading_deg(route, self.following.progress_m) {
                self.travel_deg = Some(deg);
                self.travel_at_m = Some(self.following.progress_m);
                return;
            }
            // Undecodable geometry: neutral, like having no route at all.
        }
        // Off-route, no route, or no readable geometry: neutral — the momentary heading is not a
        // direction the panel can stand behind (see `travel_deg`).
        self.travel_deg = None;
        self.travel_at_m = None;
    }

    /// Whether the route matcher has locked onto the active route at least once this load.
    pub(crate) fn started(&self) -> bool {
        self.route_match.started()
    }

    /// Re-point every route-keyed cache after a catalog replacement (#450): each build key follows
    /// its route's identity through `remap`; a key whose route vanished drops its cache (and the
    /// derived [`RouteState`] hanging off it). The active-route remap itself lives here too,
    /// so the matcher reset on a vanished navigated route can't be forgotten by a caller.
    pub(crate) fn remap_route_keys(&mut self, remap: &dyn Fn(usize) -> Option<usize>) {
        // The navigated route + the caches keyed on it. When the identity survives, all move
        // together, so nothing resets (no matcher re-lock, no profile rebuild). When it vanished,
        // navigation unloads and the stale per-route state is dropped with it.
        let old_active = self.following.active_route;
        self.following.active_route = old_active.and_then(remap);
        // A queued seam re-anchor (one tick between detour commit and geometry) follows the same
        // durable route identity as `active_route`, or is cancelled if that route vanished. So does
        // Navigator's undelivered detour request, which its owner remaps beside this call.
        self.following.remap_seam(remap);
        if old_active.is_some() && self.following.active_route.is_none() {
            self.route_match.reset(); // drop stale progress/off-route from the vanished route
        }
        self.matched_route = self.matched_route.and_then(remap);
        let old_profile = self.profile_route;
        self.profile_route = old_profile.and_then(remap);
        if old_profile.is_some() && self.profile_route.is_none() {
            self.profile = None;
        }
        // The climbs cache follows the same identity: it survives a rescan that keeps the route
        // (same-id remap), and drops when the navigated route vanishes. Clearing the active-climb
        // state too keeps a stale "on climb" flag from stranding the rider on a gone route.
        let old_climbs = self.climbs_route;
        self.climbs_route = old_climbs.and_then(remap);
        if old_climbs.is_some() && self.climbs_route.is_none() {
            self.climbs = Climbs::new();
            self.following.active_climb = None;
        }
        // The waypoint table follows that same identity — remapped across a rescan, dropped (with
        // the next-waypoint index) when the navigated route vanishes.
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

    // --- the pure climb resolver (C3, #509) ---
    //
    // `resolve_active_climb` is pinned directly over a hand-built `Climbs` list — enter, exit,
    // and the flap-guard — with no reader. The App-side wiring (build-on-load, clear-on-unload,
    // the once-per-entry `ClimbProfile::fill`, the C5 auto-switch) stays pinned end-to-end in
    // `app.rs` over the committed Grimsel fixture.

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

    /// A `Climbs` list from `(start, end)` pairs.
    fn climbs(spans: &[(u32, u32)]) -> Climbs {
        let mut c = Climbs::new();
        for &(s, e) in spans {
            c.0.push(seg(s, e)).unwrap();
        }
        c
    }

    /// Enter: below a climb's entry band there's no active climb; once progress reaches within
    /// `CLIMB_ENTER_MARGIN_M` of the base the climb arms (slightly *before* the base), and it stays
    /// armed through the interval.
    #[test]
    fn resolve_arms_a_climb_at_its_entry_band() {
        let cs = climbs(&[(1000, 3000)]);
        // Well before the entry band (base 1000 − 50 = 950): nothing.
        assert_eq!(resolve_active_climb(&cs, 800, None), None);
        // Just outside the band: still nothing.
        assert_eq!(resolve_active_climb(&cs, 949, None), None);
        // Inside the entry band, before the base: armed early (the point of the enter margin).
        assert_eq!(resolve_active_climb(&cs, 960, None), Some(0));
        // Mid-climb: on it.
        assert_eq!(resolve_active_climb(&cs, 2000, None), Some(0));
    }

    /// Exit: while on a climb it's *held* past the summit by `CLIMB_EXIT_MARGIN_M`, then disarms.
    #[test]
    fn resolve_holds_past_the_summit_then_exits() {
        let cs = climbs(&[(1000, 3000)]);
        // At the summit: still on it.
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
        // Arm at the base.
        let mut active = resolve_active_climb(&cs, 1000, None);
        assert_eq!(active, Some(0));
        // Progress jitters back a few metres below the base across several fixes — inside the entry
        // band, so the climb stays armed every time (no off→on→off flapping).
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

    /// Back-to-back climbs (the Grimsel shape): leaving climb 0's exit band hands straight over to
    /// climb 1 whose entry band it's already inside — one clean transition, never a gap of `None`.
    #[test]
    fn resolve_hands_over_between_adjacent_climbs() {
        let cs = climbs(&[(1000, 3000), (3000, 5000)]);
        // On climb 0 at its summit, held through the exit band.
        assert_eq!(resolve_active_climb(&cs, 3010, Some(0)), Some(0));
        // Past climb 0's exit band: re-arms, and climb 1's entry band already contains progress →
        // straight onto climb 1.
        assert_eq!(resolve_active_climb(&cs, 3040, Some(0)), Some(1));
    }

    /// A stale index (the list shrank under the previous active climb, e.g. a swap to a flatter
    /// route) doesn't strand the resolver — it re-arms from scratch (here: nothing).
    #[test]
    fn resolve_recovers_from_a_stale_index() {
        let cs = climbs(&[(1000, 3000)]);
        // prev = 5, but only one climb exists and progress is nowhere near it.
        assert_eq!(resolve_active_climb(&cs, 200, Some(5)), None);
    }

    // --- next-waypoint tracking (#569) ---
    //
    // The pure resolver `resolve_next_waypoint` is pinned directly over a hand-built `Waypoints`
    // table: the linger advance, the anti-flap jitter guard, the past-the-last `None`, and a fresh
    // route starting at index 0. (The App-side wiring — build-on-load, off-route freeze, re-window,
    // route-swap clear — rides the same tick/Navigator machinery the climb wiring does.)

    /// The index advances at exactly `dist + WAYPOINT_LINGER_M`, and not one metre before — the
    /// passed waypoint lingers the whole 100 m band.
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

    /// Jitter around a waypoint's own position (progress wobbling ±30 m across A's `dist`) never
    /// flaps the index — the linger band absorbs it.
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
        // Inside B's band: still B.
        assert_eq!(resolve_next_waypoint(&w, 2_099, Some(1)), Some(1));
        // Past B + 100: nothing ahead.
        assert_eq!(resolve_next_waypoint(&w, 2_100, Some(1)), None);
        assert_eq!(resolve_next_waypoint(&w, 9_999, Some(1)), None);
    }

    /// A fresh route (no prior index) starts at the first waypoint ahead — index 0 from progress 0,
    /// or the first still-ahead one when the rider starts mid-route.
    #[test]
    fn resolve_next_fresh_route_starts_at_the_first_ahead() {
        let w = wpts(&[(1_000, "A"), (2_000, "B"), (3_000, "C")]);
        assert_eq!(resolve_next_waypoint(&w, 0, None), Some(0));
        // Starting past A's linger picks B (the first still-ahead), not A.
        assert_eq!(resolve_next_waypoint(&w, 1_500, None), Some(1));
        // An empty table is always `None`.
        assert_eq!(resolve_next_waypoint(&Waypoints::new(), 0, None), None);
    }
}
