//! [`RideEngine`] — the ride-domain component behind the [`App`](crate::App) façade.
//!
//! Owns everything derived from the sensors and the active route while riding: the live
//! route-matcher and its lock key, the per-session breadcrumb and its session key, the
//! once-per-load route caches (elevation profile, detected climbs, named waypoints) with their
//! build keys, the single resident climb detail buffer, and the tick-edge state the repaint
//! economy hangs off (fix freshness, the no-fix banner edge, the sensor-tile edge, the battery
//! poll cadence, the ambient temperature).
//!
//! `App::tick` stays the per-frame orchestrator — it owns the camera ([`AppState`]), the screen
//! stack gates (`shows_live_data`, the Home battery gate), the wall clock, and the dirty flag —
//! and calls in here for every ride-domain decision. Methods take
//! [`Activity`] explicitly: the activity is `App`'s public façade field (hosts read it), while
//! this component owns the caches keyed on it.
//!
//! [`AppState`]: crate::AppState

use obc_route::{ClimbProfile, Climbs, Profile, RouteMatch, RouteReader, Waypoints};

use crate::activity::Activity;
use crate::breadcrumb::Breadcrumb;
use crate::settings::Settings;

/// A fix older than this (map-plane millis) means "no current GPS fix". The window is the larger of
/// this floor and a few fix intervals (see [`RideEngine::no_fix_window_ms`]), so a long configured
/// interval doesn't false-trip the banner between its own expected fixes.
const NO_FIX_FLOOR_MS: u32 = 5_000;
/// How many configured fix intervals of silence count as "lost" before the floor takes over.
const NO_FIX_INTERVALS: u32 = 3;

/// How often the tick reads the battery [`FuelGauge`](obc_ports::FuelGauge). Charge drifts over
/// minutes, so a ~30 s cadence keeps the Home gauge fresh while reading the PMIC a few times a
/// minute at most. Independent of redraws: an unchanged reading repaints nothing.
const BATTERY_POLL_MS: u32 = 30_000;

/// Enter/exit hysteresis for [`RideEngine::update_active_climb`] — the margins that turn the raw
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
/// `RideEngine::update_active_climb` wrapper only adds the off-route freeze and the once-per-entry
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
/// [`update_next_waypoint`](RideEngine::update_next_waypoint) wrapper adds the off-route freeze and
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

/// The ride-domain state + logic component. See the module docs; field-level invariants are on
/// each field.
pub(crate) struct RideEngine {
    /// The active route's resident elevation profile, rebuilt on route load (it streams every
    /// chunk, so never per frame). `None` when no route is loaded;
    /// [`profile_route`](RideEngine::profile_route) tracks which route it was built for.
    pub(crate) profile: Option<Profile>,
    /// The `active_route` the cached [`profile`](RideEngine::profile) was built for, so a route
    /// change triggers exactly one rebuild.
    pub(crate) profile_route: Option<usize>,
    /// The active route's detected climbs, segmented once on route load (one streaming chunk
    /// sweep, so never per frame). Empty when no route is loaded;
    /// [`climbs_route`](RideEngine::climbs_route) tracks which route the list was built for. The
    /// riding views query it (with hysteresis, via
    /// [`update_active_climb`](RideEngine::update_active_climb)) to decide "am I on a climb now?".
    pub(crate) climbs: Climbs,
    /// The `active_route` the cached [`climbs`](RideEngine::climbs) list was built for, so a route
    /// change triggers exactly one re-segmentation. Kept apart from
    /// [`profile_route`](RideEngine::profile_route) even though they change together, so each
    /// cache states its own build key.
    pub(crate) climbs_route: Option<usize>,
    /// The active route's resident named-waypoint table, loaded once on route load (it streams the
    /// stored waypoint section, so never per frame) — the waypoint twin of
    /// [`climbs`](RideEngine::climbs). Empty when no route is loaded; the riding views read it and
    /// [`Activity::next_waypoint`] indexes it. A [`truncated`](obc_route::Waypoints::truncated)
    /// table is re-windowed forward in [`update_next_waypoint`](RideEngine::update_next_waypoint)
    /// once the rider passes its tail.
    pub(crate) waypoints: Waypoints,
    /// The `active_route` the cached [`waypoints`](RideEngine::waypoints) table was loaded for —
    /// its own build key, alongside [`climbs_route`](RideEngine::climbs_route). A re-window leaves
    /// it pointed at the same route (it reloads a *later* window of the same route, not a new
    /// route), so only an actual route change reloads from the start.
    pub(crate) waypoints_route: Option<usize>,
    /// The **single** resident detail profile for the currently-active climb — one buffer refilled
    /// in place only when [`Activity::active_climb`] transitions to a new `Some(i)`, never per
    /// frame (the fill streams the climb's chunks; ~400 B, held resident to keep it off the ~36 KB
    /// device stack). Meaningless (a flat base line) while no climb is active; the
    /// [`Render`](crate::screen::Render) surface only hands it out alongside a `Some`
    /// `active_climb`, so a stale buffer is never drawn.
    pub(crate) climb_profile: ClimbProfile,
    /// Test-only tally of [`ClimbProfile::fill`] calls, so a test can assert the detail buffer is
    /// rebuilt **exactly** on climb-entry transitions — never per fix on the same climb. Not
    /// compiled into the firmware.
    #[cfg(test)]
    pub(crate) climb_fill_count: u32,
    /// The live route-matcher (snaps each GPS fix to the active route → progress / off-route).
    /// Reset on route change; runs in the tick, result stored on [`Activity`].
    route_match: RouteMatch,
    /// The `active_route` the **matcher** was last reset for, so changing the navigated route — a
    /// load *or* a "Swap route only" — re-locks it once.
    pub(crate) matched_route: Option<usize>,
    /// The [`session`](Activity::session) the **ride accumulators + breadcrumb** were last reset
    /// for, so a new tracking session (load from Idle / "Save & start new") restarts them once,
    /// while a swap (same session) leaves them running.
    pub(crate) ride_session: Option<u32>,
    /// The travelled-path breadcrumb (RAM, bounded), fed each logged fix and drawn on the Map;
    /// cleared when [`ride_session`](RideEngine::ride_session) changes.
    pub(crate) breadcrumb: Breadcrumb,
    /// Millis of the last battery [`FuelGauge`](obc_ports::FuelGauge) poll, or `None` before the
    /// first. Read on a slow cadence ([`BATTERY_POLL_MS`]) — *not* every tick — so a real PMIC
    /// read never spins the I²C bus at the frame rate.
    last_battery_poll_ms: Option<u32>,
    /// Last ambient temperature (°C), or `None` before the first sample / no thermometer. Held
    /// across ticks. No screen consumes it yet, so it lives **off**
    /// [`AppState`](crate::AppState) — storing it there would gate a needless map redraw on every
    /// reading, breaking render-on-demand. No screen or public app façade consumes the cached
    /// sample yet; the tick path only updates this ride-owned state.
    pub(crate) temp_c: Option<f32>,
    /// Map-plane millis of the last accepted GPS fix, or `None` before the first ever. Drives the
    /// "No GPS Fix" banner via [`has_live_fix`](RideEngine::has_live_fix). Lives **off**
    /// [`AppState`](crate::AppState) — like [`temp_c`](RideEngine::temp_c) — so advancing it on
    /// every fix (incl. a stationary one) never trips the `state != state_before` redraw gate; the
    /// banner's own repaint edge comes from the end-of-tick flip below.
    pub(crate) last_fix_ms: Option<u32>,
    /// The coordinate `(lat, lon)` of the freshest fix that has **not yet** been handed a terrain
    /// sample (EL8, epic #1068), or `None` once one has been (or before the first fix).
    ///
    /// This one-shot *is* the sampling cadence: `tick` arms it only on a fresh fix, and
    /// [`App::sample_terrain`](crate::App::sample_terrain) disarms it, so a host that calls the
    /// sampler every frame still reads at most one terrain tile per fix — the whole point, since a
    /// per-frame sample would be an SD read on the render path. Lives **off**
    /// [`AppState`](crate::AppState) for the same reason as
    /// [`last_fix_ms`](RideEngine::last_fix_ms): it must never trip the redraw gate.
    pub(crate) pending_terrain: Option<(i32, i32)>,
    /// The no-fix state at the previous tick's end, so the timer edge that flips the "No GPS Fix"
    /// banner dirties the live-data views exactly once. Starts `true` — no fix at boot.
    pub(crate) prev_no_fix: bool,
    /// The sensor-tile display values `(hr, power, cadence)` at the previous tick's end, so a
    /// fresh BLE sample — or the 5 s staleness gate expiring one into `--` — repaints the riding
    /// views exactly once (the sensor twin of [`prev_no_fix`](RideEngine::prev_no_fix)). The
    /// samples land in [`Activity`], which the `state != state_before` redraw gate never compares,
    /// so without this edge a live tile only repainted when something *else* happened to dirty the
    /// frame — frozen solid on an indoor bench with no fix (epic #744, SR3).
    pub(crate) prev_live_sensors: (Option<u16>, Option<u16>, Option<u8>),
    /// The rider's **travel direction** (degrees CW from north) for the route-relative wind
    /// arrows (WX12, #1197): the active route's general heading ahead of the matched progress
    /// while on-route (held while stopped — the wind question at a rest stop is about the ride
    /// ahead), else `None` — the arrows then render neutral, never a fabricated head/tail.
    ///
    /// **Route or nothing** (owner tuning round). The momentary GPS course used to stand in
    /// without a route, and it is not a claim the panel can keep honest: a rider stopped at a
    /// junction, turning the bars to show a partner the screen, would repaint every arrow against
    /// a direction they aren't going. A planned route is the one direction that survives standing
    /// still. Updated per fresh fix in `App::tick`; cleared with the route-derived state.
    pub(crate) travel_deg: Option<f32>,
    /// The progress the route heading in [`travel_deg`](RideEngine::travel_deg) was computed at —
    /// the recompute hysteresis key, so the two `position_at` chunk decodes run only when the
    /// rider has actually moved along the route (≥ [`HEADING_MOVE_M`]), not per fix.
    pub(crate) travel_at_m: Option<u32>,
    /// The bounded recent moving-speed window feeding the weather ride projection (WX12) — see
    /// [`SpeedWindow`](crate::weather::SpeedWindow). Cleared with the session.
    pub(crate) speed_win: crate::weather::SpeedWindow,
}

/// Progress the rider must cover before the route heading is re-derived (two chunk decodes).
/// Sized against [`TRAVEL_CHORD_M`](crate::weather::TRAVEL_CHORD_M): a kilometre-long chord cannot
/// swing within a few tens of metres, so anything finer only re-reads the card — and a stationary
/// rider's GPS jitter must never do that at all.
pub(crate) const HEADING_MOVE_M: u32 = 50;

impl RideEngine {
    /// The boot state: no route caches, no session, nothing sensed yet.
    pub(crate) fn new() -> Self {
        RideEngine {
            profile: None,
            profile_route: None,
            climbs: Climbs::new(),
            climbs_route: None,
            waypoints: Waypoints::new(),
            waypoints_route: None,
            climb_profile: ClimbProfile::new(),
            #[cfg(test)]
            climb_fill_count: 0,
            route_match: RouteMatch::new(),
            matched_route: None,
            ride_session: None,
            breadcrumb: Breadcrumb::new(),
            last_battery_poll_ms: None,
            temp_c: None,
            last_fix_ms: None,
            pending_terrain: None,
            prev_no_fix: true,
            prev_live_sensors: (None, None, None),
            travel_deg: None,
            travel_at_m: None,
            speed_win: crate::weather::SpeedWindow::new(),
        }
    }

    /// Initialize `slot` **in place** to the [`new`](RideEngine::new) state — the placement path
    /// (the waypoint table, climb caches, and breadcrumb are KB-scale; nothing here may form a
    /// by-value `RideEngine` on the stack). Same field-by-field `addr_of_mut!` discipline as
    /// [`App::init_idle`](crate::App::init_idle), with the same trailing exhaustiveness guard.
    ///
    /// # Safety
    /// `slot` must be valid, aligned, exclusively owned, and writable for a full `RideEngine`.
    pub(crate) unsafe fn init_in_place(slot: *mut Self) {
        use core::ptr::addr_of_mut;
        // SAFETY: caller's contract; every field is written exactly once before any read.
        unsafe {
            addr_of_mut!((*slot).profile).write(None);
            addr_of_mut!((*slot).profile_route).write(None);
            // The climb caches mirror the profile: an empty list + a zeroed detail buffer
            // (`Climbs::new`/`ClimbProfile::new` are const, so no large temporary is formed here).
            addr_of_mut!((*slot).climbs).write(Climbs::new());
            addr_of_mut!((*slot).climbs_route).write(None);
            // The waypoint table mirrors the climbs list: an empty table (~1.3 KB) written straight
            // into the slot, keyed to no route until the first load.
            addr_of_mut!((*slot).waypoints).write(Waypoints::new());
            addr_of_mut!((*slot).waypoints_route).write(None);
            addr_of_mut!((*slot).climb_profile).write(ClimbProfile::new());
            #[cfg(test)]
            addr_of_mut!((*slot).climb_fill_count).write(0);
            addr_of_mut!((*slot).route_match).write(RouteMatch::new());
            addr_of_mut!((*slot).matched_route).write(None);
            addr_of_mut!((*slot).ride_session).write(None);
            addr_of_mut!((*slot).breadcrumb).write(Breadcrumb::new());
            addr_of_mut!((*slot).last_battery_poll_ms).write(None);
            addr_of_mut!((*slot).temp_c).write(None);
            addr_of_mut!((*slot).last_fix_ms).write(None);
            addr_of_mut!((*slot).pending_terrain).write(None);
            addr_of_mut!((*slot).prev_no_fix).write(true);
            addr_of_mut!((*slot).prev_live_sensors).write((None, None, None));
            addr_of_mut!((*slot).travel_deg).write(None);
            addr_of_mut!((*slot).travel_at_m).write(None);
            addr_of_mut!((*slot).speed_win).write(crate::weather::SpeedWindow::new());
            // Exhaustiveness guard: a field added to `RideEngine` fails to compile here until its
            // `addr_of_mut!(...).write(...)` is added above (see `App::init_idle`).
            let RideEngine {
                profile: _,
                profile_route: _,
                climbs: _,
                climbs_route: _,
                waypoints: _,
                waypoints_route: _,
                climb_profile: _,
                #[cfg(test)]
                    climb_fill_count: _,
                route_match: _,
                matched_route: _,
                ride_session: _,
                breadcrumb: _,
                last_battery_poll_ms: _,
                temp_c: _,
                last_fix_ms: _,
                pending_terrain: _,
                prev_no_fix: _,
                prev_live_sensors: _,
                travel_deg: _,
                travel_at_m: _,
                speed_win: _,
            } = &*slot;
        }
    }

    /// The once-per-load route/session sync, run at the top of every tick. Returns whether the map
    /// must repaint (a route line appeared/vanished, the breadcrumb cleared, the matcher re-locked).
    ///
    /// - The **matcher** follows the *navigated route*: a load or a "Swap route only" re-locks it.
    /// - The **accumulators + breadcrumb** follow the *tracking session*: a new session restarts
    ///   them, while a swap (which keeps the session) leaves them running.
    /// - `route_total_m` mirrors the active route's length for the riding views (0 when none
    ///   loaded). A change here means the *drawable* route appeared or vanished — a load, or a
    ///   transient SD glitch recovering where the geometry becomes streamable a frame or two later.
    /// - The **climbs** and **waypoints** caches build once per load — climbs here in the tick
    ///   (not render) because [`update_active_climb`](RideEngine::update_active_climb) needs the
    ///   list before the fix is matched. Only advance a build key when the geometry is actually
    ///   streamable: a `None` route (idle, or a transient SD glitch) leaves the old state in place
    ///   and retries next tick, rather than latching an empty result for the route.
    pub(crate) fn sync_route_state(&mut self, activity: &mut Activity, route: Option<&RouteReader>) -> bool {
        let mut dirty = false;
        if activity.active_route != self.matched_route {
            // Deliberately do NOT clear a pending seam re-anchor here: a detour commit queues it
            // for the *just-adopted* spliced route, so this route-change edge is exactly the tick
            // it must survive into. Stale seams die on the request's own route-key check.
            self.route_match.reset();
            self.matched_route = activity.active_route;
            // The old route's tangent means nothing on the new line (WX12) — neutral until the
            // next fix matches.
            self.travel_deg = None;
            self.travel_at_m = None;
            dirty = true; // route load / swap repaints the route line + recenters
        }
        if activity.session != self.ride_session {
            // A new tracking session on the same route is a new navigation pass too: discard a
            // previous session's floor before the first match.
            self.route_match.reset();
            if !activity.take_resume_session() {
                activity.reset_ride();
            }
            self.breadcrumb.clear();
            // A new session is a new pace too (WX12's projection window restarts with the ride).
            self.speed_win.clear();
            self.ride_session = activity.session;
            dirty = true; // the breadcrumb cleared — the map's travelled trail changed
        }
        let route_total_before = activity.route_total_m;
        activity.route_total_m = route.map_or(0, |r| r.total_distance_m);
        if activity.route_total_m != route_total_before {
            dirty = true;
        }

        // Segment the route's climbs once per load — the twin of the elevation-profile rebuild.
        if activity.active_route != self.climbs_route {
            match (activity.active_route, route) {
                (Some(_), Some(r)) => {
                    self.climbs = r.detect_climbs();
                    self.climbs_route = activity.active_route;
                    activity.active_climb = None; // a fresh list — re-derive the active climb on the next match
                }
                (None, _) => {
                    // The route unloaded: drop the climbs and the on-climb state.
                    self.climbs = Climbs::new();
                    self.climbs_route = None;
                    activity.active_climb = None;
                }
                (Some(_), None) => { /* geometry not yet streamable — keep the old state, retry next tick */ }
            }
        }

        // Load the route's named waypoints once per load, alongside the climbs above and on the
        // same streamable-geometry guard. Loaded from the route start (`min_dist_m = 0`); a
        // truncated table is slid forward later, in `update_next_waypoint`, not here.
        if activity.active_route != self.waypoints_route {
            match (activity.active_route, route) {
                (Some(_), Some(r)) => {
                    self.waypoints = r.load_waypoints(0);
                    self.waypoints_route = activity.active_route;
                    activity.next_waypoint = None; // a fresh table — re-derive the next waypoint on the next match
                }
                (None, _) => {
                    // The route unloaded: drop the table and the next-waypoint state.
                    self.waypoints = Waypoints::new();
                    self.waypoints_route = None;
                    activity.next_waypoint = None;
                }
                (Some(_), None) => { /* geometry not yet streamable — keep the old table, retry next tick */ }
            }
        }
        activity.waypoint_count = self.waypoints.len();
        dirty
    }

    /// Whether the ~30 s battery-poll cadence is due at `now_ms` — and if so, stamp it consumed.
    /// The caller (the tick) does the actual [`FuelGauge`](obc_ports::FuelGauge) read + the Home-only
    /// repaint gate.
    pub(crate) fn battery_poll_due(&mut self, now_ms: u32) -> bool {
        let due = self.last_battery_poll_ms.is_none_or(|last| now_ms.wrapping_sub(last) >= BATTERY_POLL_MS);
        if due {
            self.last_battery_poll_ms = Some(now_ms);
        }
        due
    }

    /// Snap a fresh fix onto the active route: run the matcher and store the result on
    /// [`Activity`]. Called once per fresh fix (never on a dropout, so progress isn't re-derived
    /// from a stale position).
    pub(crate) fn match_fix(&mut self, activity: &mut Activity, fix: obc_ports::Fix, route: &RouteReader) {
        let m = self.route_match.update(fix.lon, fix.lat, route);
        activity.apply_match(m);
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
    pub(crate) fn apply_pending_seam(&mut self, activity: &mut Activity, route: Option<&RouteReader>) -> bool {
        let Some(req) = activity.pending_seam() else { return false };
        if activity.active_route != Some(req.route) {
            activity.clear_seam();
            return false;
        }
        let Some(route) = route else { return false };
        if let Some(pos) = self.route_match.set_progress_floor(route, req.anchor_m) {
            activity.clear_seam();
            activity.apply_match(obc_route::Match { progress_m: pos.progress_m, off_route: false, dist_m: 0 });
            activity.active_climb = None;
            activity.next_waypoint = None;
            true
        } else {
            // A transient decode failure is retryable. Keep both the request and the old visible
            // progress; clearing one without moving the matcher would split the two anchors.
            false
        }
    }

    /// Recompute [`Activity::active_climb`] from the freshly-matched `progress_m`, applying
    /// enter/exit hysteresis over the raw [`Climbs::active_at`] lookup, and refill the resident
    /// [`climb_profile`](RideEngine::climb_profile) detail buffer **only on a new climb entry**
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
    pub(crate) fn update_active_climb(
        &mut self,
        activity: &mut Activity,
        route: &RouteReader,
    ) -> Option<(Option<usize>, Option<usize>)> {
        // Off-route freezes the cursor, so keep whatever climb we were on — don't recompute against
        // a stale progress. `apply_match` leaves `progress_m` frozen while off-route.
        if activity.off_route {
            return None;
        }
        let prev = activity.active_climb;
        let next = resolve_active_climb(&self.climbs, activity.progress_m, prev);
        if next == prev {
            return None; // unchanged — no refill, no SD read.
        }
        activity.active_climb = next;
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

    /// Recompute [`Activity::next_waypoint`] from the freshly-matched `progress_m` via the pure
    /// [`resolve_next_waypoint`], and slide a truncated table's window forward when the rider passes
    /// its tail — the waypoint twin of [`update_active_climb`](RideEngine::update_active_climb).
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
    pub(crate) fn update_next_waypoint(&mut self, activity: &mut Activity, route: &RouteReader) -> bool {
        // Off-route freezes progress, so the resolved index freezes with it — keep what we had.
        if activity.off_route {
            return false;
        }
        // Slide a truncated window forward once its whole resident span (last entry + linger) is
        // behind the rider — see the re-window note above.
        if self.waypoints.truncated {
            if let Some(last) = self.waypoints.as_slice().last() {
                if activity.progress_m >= last.dist_along_m.saturating_add(WAYPOINT_LINGER_M) {
                    self.waypoints = route.load_waypoints(activity.progress_m);
                    activity.next_waypoint = None; // the window slid — re-derive against it below
                }
            }
        }
        activity.waypoint_count = self.waypoints.len();
        let prev = activity.next_waypoint;
        let next = resolve_next_waypoint(&self.waypoints, activity.progress_m, prev);
        if next != prev {
            activity.next_waypoint = next;
            return true; // the next waypoint changed — the chip / fields must repaint
        }
        false
    }

    /// Rebuild the cached elevation profile when the active route changed — it streams every
    /// chunk, so it's built once on load, never per frame; clears when no route is loaded. Run at
    /// render (the one place the host guarantees a live reader for the frame).
    pub(crate) fn refresh_route_profile(&mut self, active_route: Option<usize>, route: Option<&RouteReader>) {
        if active_route != self.profile_route {
            self.profile = route.map(|r| r.elevation_profile());
            self.profile_route = active_route;
        }
    }

    /// Drop **everything derived from the active route's geometry** — matcher lock, elevation
    /// profile, climbs (+ on-climb state), waypoints (+ next-waypoint), and the match-derived
    /// readouts on [`Activity`] — so the next tick/render re-derives all of it from the reopened
    /// geometry. The forced-adoption discipline shared by a committed route plan (new bytes under
    /// the reserved nav id) and an active-replace upload (new bytes under a kept id): the same-id
    /// remap deliberately preserves same-id state, and these are exactly the cases where that
    /// preservation would carry stale state onto new geometry. The recording session is untouched.
    pub(crate) fn drop_route_derived_state(&mut self, activity: &mut Activity) {
        // `reset` also clears any wide re-lock armed by a freeze (`note_unmatched_fix`), and should:
        // an unstarted matcher scans the whole route on its next fix, which is wider still. That is
        // why the wide window is only ever spent on a freeze that ended without new geometry.
        self.route_match.reset();
        self.matched_route = None; // tick re-locks the matcher from the current fix
        self.profile = None;
        self.profile_route = None; // the next render rebuilds from the reopened geometry
        self.climbs = Climbs::new();
        self.climbs_route = None; // the next tick re-segments from the reopened geometry
        activity.active_climb = None;
        self.waypoints = Waypoints::new();
        self.waypoints_route = None; // the next tick re-loads from the reopened geometry
        activity.next_waypoint = None;
        activity.waypoint_count = 0;
        activity.progress_m = 0;
        activity.off_route = false;
        activity.dist_to_route_m = 0;
        activity.clear_seam();
        // The route tangent was measured on the old geometry — neutral until the next fix
        // re-derives it (WX12). The speed window survives: the rider's pace is route-agnostic.
        self.travel_deg = None;
        self.travel_at_m = None;
    }

    /// Update the WX12 travel direction from this tick's fresh fix (see
    /// [`travel_deg`](RideEngine::travel_deg) — the route's general heading, or neutral). Runs
    /// after the matcher, so `activity` carries this fix's match. The heading recomputes only when
    /// the rider moved ≥ [`HEADING_MOVE_M`] along the route (two `position_at` chunk decodes,
    /// fix-cadence-bounded).
    pub(crate) fn update_travel(&mut self, activity: &Activity, route: Option<&RouteReader>) {
        let on_route = route.is_some() && activity.active_route.is_some() && self.started() && !activity.off_route;
        if on_route {
            let route = route.unwrap();
            let moved = self.travel_at_m.is_none_or(|at| activity.progress_m.abs_diff(at) >= HEADING_MOVE_M);
            if !moved && self.travel_deg.is_some() {
                return; // held heading (stopped, or sub-hysteresis creep)
            }
            if let Some(deg) = crate::weather::route_heading_deg(route, activity.progress_m) {
                self.travel_deg = Some(deg);
                self.travel_at_m = Some(activity.progress_m);
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
    /// derived [`Activity`] state hanging off it). The `active_route` remap itself lives here too,
    /// so the matcher reset on a vanished navigated route can't be forgotten by a caller.
    pub(crate) fn remap_route_keys(&mut self, activity: &mut Activity, remap: &dyn Fn(usize) -> Option<usize>) {
        // The navigated route + the caches keyed on it. When the identity survives, all move
        // together, so nothing resets (no matcher re-lock, no profile rebuild). When it vanished,
        // navigation unloads and the stale per-route state is dropped with it.
        let old_active = activity.active_route;
        activity.active_route = old_active.and_then(remap);
        // A queued seam re-anchor (one tick between detour commit and geometry) and a queued
        // detour-plan request both follow the same durable route identity as `active_route`, or
        // are cancelled if that route vanished.
        activity.remap_seam_route(remap);
        activity.remap_detour_route(remap);
        if old_active.is_some() && activity.active_route.is_none() {
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
            activity.active_climb = None;
        }
        // The waypoint table follows that same identity — remapped across a rescan, dropped (with
        // the next-waypoint index) when the navigated route vanishes.
        let old_wpts = self.waypoints_route;
        self.waypoints_route = old_wpts.and_then(remap);
        if old_wpts.is_some() && self.waypoints_route.is_none() {
            self.waypoints = Waypoints::new();
            activity.next_waypoint = None;
            activity.waypoint_count = 0;
        }
    }

    /// The fix-staleness window (map-plane millis): the larger of [`NO_FIX_FLOOR_MS`] and a few
    /// configured fix intervals, so a long interval doesn't flag "no fix" in the normal gap between
    /// its own fixes. A 1 s interval gives the 5 s floor; a 30 s interval gives 90 s.
    fn no_fix_window_ms(&self, settings: &Settings) -> u32 {
        (settings.fix_interval_s as u32 * 1000 * NO_FIX_INTERVALS).max(NO_FIX_FLOOR_MS)
    }

    /// Whether there's a **current** GPS fix at `now_ms`: a fix has been accepted and is no older
    /// than [`no_fix_window_ms`](RideEngine::no_fix_window_ms). `false` before the first fix
    /// (acquiring) and once the signal drops (lost) — exactly when the "No GPS Fix" banner shows.
    pub(crate) fn has_live_fix(&self, now_ms: u32, settings: &Settings) -> bool {
        self.last_fix_ms.is_some_and(|t| now_ms.wrapping_sub(t) <= self.no_fix_window_ms(settings))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::support::wpts;

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
    // route-swap clear — rides the same `tick`/`Activity` machinery the climb wiring does.)

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
