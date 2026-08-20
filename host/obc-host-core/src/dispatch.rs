//! The shared command/event dispatcher for the frame-stepped hosts (the desktop sim GUI, the sim's
//! headless replay, and the web demo). One [`HostLoop`] owns the caller's [`HostMailbox`], the
//! in-flight resumable [`NavPlan`], and the resident [`ActiveRouteSession`]; [`HostLoop::reconcile`]
//! drains the typed protocol once per pass and applies the repository commands in the canonical
//! order the board runs asynchronously — deletes and their catalog re-feeds, the nav plan lifecycle,
//! the ride-track fill, and the track lifecycle. The handful of genuinely host-specific commands
//! (card scan, forget-bond, settings persistence, DFU) go to a caller closure, so a host without one
//! of them simply ignores it.
//!
//! What stays with the caller, by design: input/gesture application, the sensor tick, and rendering
//! (the overlay/input plane never routes through the mailbox), plus opening the active-route
//! [`RouteReader`] for that tick+render (a borrow the caller must own across both). The caller opens
//! it from [`HostLoop::session`] right after `reconcile` returns — no per-frame reparse.

use obc_app::{App, DrainStatus, HostCommand, HostMailbox, TrackAction};

use crate::nav::{finish_detour_commit, finish_detour_plan, DetourPlan, DetourReady};
use crate::{
    finish_nav_plan, ActiveRouteSession, NavPlan, RideRepository, RouteRepository, TrackRepository, TripCatalog,
};

/// Feed the app the route catalog **with** its retention metas (epic #638, S3) — the shared re-feed
/// after a scan/delete so the auto-expiry sweep always reads device-truth retention alongside the
/// summaries. A retention-less repository returns empty metas → every route reads `Never`.
fn feed_routes(app: &mut App, routes: &dyn RouteRepository) {
    app.set_routes_with_meta(routes.catalog(), routes.ids(), &routes.retention_metas());
}

/// The one in-flight plan a host steps — a POI route plan or a detour plan (#882). One enum slot
/// instead of two `Option`s: the two flows can never run concurrently **by construction** (the UI
/// can't reach both planning screens at once, and one slot makes the exclusion structural), and
/// only one large scratch/tile frame is alive at a time (the stack rule below).
pub enum InflightPlan {
    Nav(NavPlan),
    Detour(DetourPlan),
}

/// Plan requests a host deliberately consumes without starting. This is only needed by deterministic
/// hosts that must freeze a planning screen (for example the simulator's `--hold nav` snapshots);
/// ordinary frame loops use [`PlanHold::NONE`]. Cancels and every non-planning command still drain.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlanHold {
    route: bool,
    detour: bool,
}

impl PlanHold {
    /// Run every plan request.
    pub const NONE: Self = Self { route: false, detour: false };

    /// Hold route and/or detour requests while the rest of the canonical mailbox drains.
    pub const fn new(route: bool, detour: bool) -> Self {
        Self { route, detour }
    }
}

/// The shared host-loop state: the drain mailbox, the in-flight plan (stepped once per pass), a
/// planned-but-uncommitted detour, and the resident active-route parse. A host owns one for its
/// lifetime.
#[derive(Default)]
pub struct HostLoop {
    mailbox: HostMailbox,
    plan: Option<InflightPlan>,
    /// A planned detour's bytes + frozen splice context (#882), held from `DetourPlanned` until
    /// the rider commits or cancels.
    detour_ready: Option<DetourReady>,
    /// The resident active-route parse — the caller reads it (via [`session`](HostLoop::session))
    /// to open the tick+render [`RouteReader`](obc_route::RouteReader) after [`reconcile`](HostLoop::reconcile).
    pub session: ActiveRouteSession,
}

impl HostLoop {
    /// A fresh host loop (empty mailbox, no plan, nothing parsed).
    pub fn new() -> Self {
        HostLoop { mailbox: HostMailbox::new(), plan: None, detour_ready: None, session: ActiveRouteSession::new() }
    }

    /// Whether a plan (route or detour) is computing (the planning-spinner state).
    pub fn is_planning(&self) -> bool {
        self.plan.is_some()
    }

    /// Drain every pending host command in the canonical [`HostCommand::DRAIN_ORDER`], apply the
    /// repository ones here, then step an in-flight plan once and reconcile the ride recorder to the
    /// app's session. Host-specific commands ([`ScanCardFree`](HostCommand::ScanCardFree),
    /// [`ForgetBond`](HostCommand::ForgetBond), [`PersistSettings`](HostCommand::PersistSettings),
    /// [`Dfu`](HostCommand::Dfu)) are handed to `host` — pass `|_, _| {}` for a host that has none.
    ///
    /// `reader` is the map reader the resumable planner steps against, and `elev` the map's
    /// terrain (EL7 — [`crate::terrain`]; `&mut NullElevation` for a host without one). The
    /// active-route [`RefreshNavPreview`](HostCommand::RefreshNavPreview) cue is intentionally
    /// *not* answered in the drain (the route isn't open yet); the caller answers it with
    /// [`fill_nav_preview`](crate::fill_nav_preview) once it opens the route after this returns.
    #[allow(clippy::too_many_arguments)]
    pub fn reconcile(
        &mut self,
        app: &mut App,
        routes: &mut dyn RouteRepository,
        rides: &mut dyn RideRepository,
        tracks: &mut dyn TrackRepository,
        trips: &mut dyn TripCatalog,
        reader: &obc_reader::Reader,
        elev: &mut dyn obc_route::ElevationSource,
        host: impl FnMut(&mut App, HostCommand),
    ) {
        // The three phases run as **separate calls** on purpose: `dispatch_commands` reserves the
        // fresh `NavPlan` (its ~4 KB inline tile cache) and `step_plan` reaches `finish_nav_plan`'s
        // ~8 KB `RouteIndex` parse — nesting them in one frame stacked both and overflowed the deep
        // sim tour test's thread stack. Sequential calls keep only one large frame live at a time.
        let finish = self.dispatch_commands(app, routes, rides, trips, PlanHold::NONE, host);
        self.step_plan(app, routes, reader, elev);
        reconcile_track(app, rides, tracks, finish);
    }

    /// The run-to-completion counterpart to [`reconcile`](Self::reconcile): drain the same mailbox
    /// through the same dispatcher, then step the same resident plan until it reaches a terminal
    /// result. Scripted/headless hosts use this when there is no display frame to yield between
    /// bounded planner steps.
    ///
    /// `hold` is the small deterministic-harness escape hatch: selected plan requests are consumed
    /// but not started, while all other commands retain their canonical order and behavior. A
    /// drained track finish is returned because a headless host can physically open its track store
    /// later than its scripted command pass; frame loops use [`reconcile`](Self::reconcile) to apply
    /// the finish immediately.
    #[allow(clippy::too_many_arguments)]
    pub fn reconcile_to_completion(
        &mut self,
        app: &mut App,
        routes: &mut dyn RouteRepository,
        rides: &mut dyn RideRepository,
        trips: &mut dyn TripCatalog,
        reader: &obc_reader::Reader,
        elev: &mut dyn obc_route::ElevationSource,
        hold: PlanHold,
        host: impl FnMut(&mut App, HostCommand),
    ) -> Option<TrackAction> {
        // Keep these calls separate for the same stack-frame reason documented in `reconcile`.
        let finish = self.dispatch_commands(app, routes, rides, trips, hold, host);
        while self.plan.is_some() {
            self.step_plan(app, routes, reader, elev);
        }
        finish
    }

    /// Phase 1 — drain the typed protocol once in canonical order and apply each command. Returns the
    /// drained [`FinishTrack`](HostCommand::FinishTrack) action (reconciled in phase 3). Generic over
    /// the host closure; `#[inline(never)]` so its `NavPlan` reservation doesn't bleed into the
    /// caller's frame.
    #[inline(never)]
    fn dispatch_commands(
        &mut self,
        app: &mut App,
        routes: &mut dyn RouteRepository,
        rides: &mut dyn RideRepository,
        trips: &mut dyn TripCatalog,
        hold: PlanHold,
        mut host: impl FnMut(&mut App, HostCommand),
    ) -> Option<TrackAction> {
        // The mailbox is popped empty at the end of every pass and sized `HOST_COMMAND_CLASSES`
        // (the `HostMailbox` default), so a full drain is guaranteed by construction — keep the
        // invariant loud rather than suppressing the drain's `#[must_use]` status.
        let status = app.drain_host_commands(&mut self.mailbox);
        debug_assert_eq!(status, DrainStatus::Complete, "a canonical-capacity mailbox always drains completely");
        let mut finish: Option<TrackAction> = None;
        while let Some(cmd) = self.mailbox.pop() {
            match cmd {
                HostCommand::RescanStore { .. } => {
                    feed_routes(app, routes);
                    trips.rescan();
                    trips.refeed(app); // after the routes, so stage ids resolve
                    rides.refresh();
                    app.set_rides(rides.catalog(), rides.ids());
                }
                HostCommand::DeleteRoute { id } => {
                    if routes.delete_by_id(id) {
                        feed_routes(app, routes);
                    }
                }
                HostCommand::DeleteRide { id } => {
                    if rides.delete_by_id(id) {
                        app.set_rides(rides.catalog(), rides.ids());
                    }
                }
                HostCommand::DeleteTrip { id } => {
                    // Cascade: the trip's member routes, then its `.obt`. Re-feed routes first so the
                    // regrouped trip list resolves against the surviving catalog.
                    for rid in trips.member_route_ids(id) {
                        routes.delete_by_id(rid);
                    }
                    if trips.delete_by_id(id) {
                        feed_routes(app, routes);
                        trips.refeed(app);
                    }
                }
                // Auto-expiry sidecar stamps (epic #638, S3): apply to the host's retention store —
                // the app already mirrored the value optimistically, so no re-feed is needed here.
                HostCommand::StampRouteUsed { id, utc } => routes.stamp_route_used(id, utc),
                HostCommand::StampRideSynced { id, utc } => rides.stamp_synced_at(id, utc),
                HostCommand::CancelRoutePlan => {
                    if matches!(self.plan, Some(InflightPlan::Nav(_))) {
                        self.plan = None;
                    }
                }
                HostCommand::PlanRoute(req) => {
                    if !hold.route {
                        self.plan = Some(InflightPlan::Nav(NavPlan::start(&req, app.settings().bike_profile_idx)));
                    }
                }
                HostCommand::CancelDetour => {
                    // Drop both the in-flight detour plan and any planned-but-uncommitted bytes.
                    if matches!(self.plan, Some(InflightPlan::Detour(_))) {
                        self.plan = None;
                    }
                    self.detour_ready = None;
                }
                HostCommand::PlanDetour(req) => {
                    if !hold.detour {
                        self.detour_ready = None;
                        let started = self.session.index().and_then(|index| {
                            let src = routes.active_source()?;
                            let orig = obc_route::RouteReader::new(index, &src);
                            DetourPlan::start(&req, app.settings().bike_profile_idx, &orig)
                        });
                        match started {
                            Some(plan) => self.plan = Some(InflightPlan::Detour(plan)),
                            // The active route vanished / can't resolve the rejoin — answer now.
                            None => {
                                app.apply_event(obc_app::HostEvent::DetourPlanned(Err(obc_route::NavError::NoPath)))
                            }
                        }
                    }
                }
                HostCommand::CommitDetour => {
                    let ready = self.detour_ready.take();
                    finish_detour_commit(app, routes, self.session.index(), ready);
                }
                HostCommand::FinishTrack(action) => finish = Some(action),
                HostCommand::LoadRideTrack { id } => {
                    app.set_ride_profile(rides.profile_by_id(id));
                    app.set_ride_preview(&rides.preview_by_id(id));
                }
                // Answered post-reconcile by the caller's `fill_nav_preview` (the route isn't open here).
                HostCommand::RefreshNavPreview => {}
                other => host(app, other),
            }
        }
        finish
    }

    /// Phase 2 — step an in-flight plan once (the board's one-step-per-pass shape) and, on a terminal
    /// outcome, commit + answer through [`finish_nav_plan`] / [`finish_detour_plan`]. Non-generic and
    /// `#[inline(never)]` so the `RouteIndex` parse inside the finish tails never coexists with
    /// phase 1's plan-reservation frame.
    #[inline(never)]
    fn step_plan(
        &mut self,
        app: &mut App,
        routes: &mut dyn RouteRepository,
        reader: &obc_reader::Reader,
        elev: &mut dyn obc_route::ElevationSource,
    ) {
        // Compute the outcome before `take`-ing, so the terminal-outcome commit doesn't overlap the
        // step borrow.
        let outcome = match self.plan.as_mut() {
            None => return,
            Some(InflightPlan::Nav(plan)) => plan.step(reader, elev),
            Some(InflightPlan::Detour(plan)) => plan.step(reader, elev),
        };
        let terminal = match outcome {
            obc_route::Step::Running => return,
            obc_route::Step::Done(stats) => Ok(stats),
            obc_route::Step::Failed(e) => Err(e),
        };
        match self.plan.take().expect("just stepped it") {
            InflightPlan::Nav(plan) => {
                finish_nav_plan(app, routes, terminal, plan.bytes(), plan.tile_stats());
            }
            InflightPlan::Detour(plan) => {
                // The detour is NOT committed here — the bytes park until the preview's Press
                // drains `CommitDetour` (or `CancelDetour` drops them). Hand `finish` the resident
                // original route so it can trim the rejoin to first tail contact (#882); the source
                // binding must outlive the call, so it's bound here rather than inside a closure.
                let src = routes.active_source();
                let orig =
                    self.session.index().zip(src.as_ref()).map(|(index, s)| obc_route::RouteReader::new(index, s));
                self.detour_ready = finish_detour_plan(app, terminal, plan, orig.as_ref());
            }
        }
    }
}

/// Phase 3 — reconcile the ride recorder: the drained finish action + the live session, with the save
/// name (the active route) and totals (for a `Save`). Refresh + re-feed the simulator catalog so a
/// saved ride appears without a relaunch.
fn reconcile_track(
    app: &mut App,
    rides: &mut dyn RideRepository,
    tracks: &mut dyn TrackRepository,
    finish: Option<TrackAction>,
) {
    // The save name is only consumed when a ride is opened or finalised, both of which need a
    // session or a drained action — skip the small String copy on the idle no-ride path. During an
    // active ride it's one ≤48-byte name clone per pass, deliberately not cached: the active route
    // (and thus the save name a mid-ride swap would freeze) can change between passes.
    let name = (app.activity.session().is_some() || finish.is_some()).then(|| active_route_name(app)).flatten();
    let stats = matches!(finish, Some(TrackAction::Save)).then(|| app.ride_stats());
    tracks.reconcile(finish, app.activity.session(), name.as_deref(), stats);
    if matches!(finish, Some(TrackAction::Save)) {
        rides.refresh();
        app.set_rides(rides.catalog(), rides.ids());
    }
}

/// The active route's catalog name (the ride-log save filename), or `None` when nothing is active.
fn active_route_name(app: &App) -> Option<String> {
    let i = app.active_route_index()?;
    app.routes().get(i).map(|r| r.name.as_str().to_string())
}
