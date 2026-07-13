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

use crate::{
    finish_nav_plan, ActiveRouteSession, NavPlan, RideRepository, RouteRepository, TrackRepository, TripCatalog,
};

/// The shared host-loop state: the drain mailbox, the in-flight route plan (stepped once per pass),
/// and the resident active-route parse. A host owns one for its lifetime.
#[derive(Default)]
pub struct HostLoop {
    mailbox: HostMailbox,
    nav: Option<NavPlan>,
    /// The resident active-route parse — the caller reads it (via [`session`](HostLoop::session))
    /// to open the tick+render [`RouteReader`](obc_route::RouteReader) after [`reconcile`](HostLoop::reconcile).
    pub session: ActiveRouteSession,
}

impl HostLoop {
    /// A fresh host loop (empty mailbox, no plan, nothing parsed).
    pub fn new() -> Self {
        HostLoop { mailbox: HostMailbox::new(), nav: None, session: ActiveRouteSession::new() }
    }

    /// Whether a route plan is computing (the planning-spinner state).
    pub fn is_planning(&self) -> bool {
        self.nav.is_some()
    }

    /// Drain every pending host command in the canonical [`HostCommand::DRAIN_ORDER`], apply the
    /// repository ones here, then step an in-flight plan once and reconcile the track log to the
    /// app's session. Host-specific commands ([`ScanCardFree`](HostCommand::ScanCardFree),
    /// [`ForgetBond`](HostCommand::ForgetBond), [`PersistSettings`](HostCommand::PersistSettings),
    /// [`Dfu`](HostCommand::Dfu)) are handed to `host` — pass `|_, _| {}` for a host that has none.
    ///
    /// `reader` is the map reader the resumable planner steps against. The active-route
    /// [`RefreshNavPreview`](HostCommand::RefreshNavPreview) cue is intentionally *not* answered in
    /// the drain (the route isn't open yet); the caller answers it with
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
        host: impl FnMut(&mut App, HostCommand),
    ) {
        // The three phases run as **separate calls** on purpose: `dispatch_commands` reserves the
        // fresh `NavPlan` (its ~4 KB inline tile cache) and `step_plan` reaches `finish_nav_plan`'s
        // ~8 KB `RouteIndex` parse — nesting them in one frame stacked both and overflowed the deep
        // sim tour test's thread stack. Sequential calls keep only one large frame live at a time.
        let finish = self.dispatch_commands(app, routes, rides, tracks, trips, host);
        self.step_plan(app, routes, reader);
        reconcile_track(app, rides, tracks, finish);
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
        tracks: &mut dyn TrackRepository,
        trips: &mut dyn TripCatalog,
        mut host: impl FnMut(&mut App, HostCommand),
    ) -> Option<TrackAction> {
        // Reserved for future track commands; keeps the repository set uniform per phase.
        let _ = tracks;

        // The mailbox is popped empty at the end of every pass and sized `HOST_COMMAND_CLASSES`
        // (the `HostMailbox` default), so a full drain is guaranteed by construction — keep the
        // invariant loud rather than suppressing the drain's `#[must_use]` status.
        let status = app.drain_host_commands(&mut self.mailbox);
        debug_assert_eq!(status, DrainStatus::Complete, "a canonical-capacity mailbox always drains completely");
        let mut finish: Option<TrackAction> = None;
        while let Some(cmd) = self.mailbox.pop() {
            match cmd {
                HostCommand::RescanStore { .. } => {
                    app.set_routes_with_ids(routes.catalog(), routes.ids());
                    trips.rescan();
                    trips.refeed(app); // after the routes, so stage ids resolve
                    rides.refresh();
                    app.set_rides(rides.catalog(), rides.ids());
                }
                HostCommand::DeleteRoute { id } => {
                    if routes.delete_by_id(id) {
                        app.set_routes_with_ids(routes.catalog(), routes.ids());
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
                        app.set_routes_with_ids(routes.catalog(), routes.ids());
                        trips.refeed(app);
                    }
                }
                HostCommand::CancelRoutePlan => self.nav = None,
                HostCommand::PlanRoute(req) => {
                    self.nav = Some(NavPlan::start(&req, app.settings().bike_profile_idx));
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
    /// outcome, commit + answer through [`finish_nav_plan`]. Non-generic and `#[inline(never)]` so the
    /// `RouteIndex` parse inside `finish_nav_plan` never coexists with phase 1's `NavPlan` frame.
    #[inline(never)]
    fn step_plan(&mut self, app: &mut App, routes: &mut dyn RouteRepository, reader: &obc_reader::Reader) {
        // Compute the outcome before `take`-ing, so the terminal-outcome commit doesn't overlap the
        // step borrow.
        let outcome = self.nav.as_mut().map(|plan| plan.step(reader));
        match outcome {
            None | Some(obc_route::Step::Running) => {}
            Some(obc_route::Step::Done(stats)) => {
                let plan = self.nav.take().expect("just stepped it");
                finish_nav_plan(app, routes, Ok(stats), plan.bytes(), plan.tile_stats());
            }
            Some(obc_route::Step::Failed(e)) => {
                let plan = self.nav.take().expect("just stepped it");
                finish_nav_plan(app, routes, Err(e), plan.bytes(), plan.tile_stats());
            }
        }
    }
}

/// Phase 3 — reconcile the track log: the drained finish action + the live session, with the save
/// name (the active route) and totals (for a `Save`). A `Save` writes a fresh `RD{id}.ORD`, so
/// refresh + re-feed the ride catalog so it appears without a relaunch.
fn reconcile_track(
    app: &mut App,
    rides: &mut dyn RideRepository,
    tracks: &mut dyn TrackRepository,
    finish: Option<TrackAction>,
) {
    // The save name is only consumed when a log is (re)opened or finalised, both of which need a
    // session or a drained action — skip the small String copy on the idle no-ride path. During an
    // active ride it's one ≤48-byte name clone per pass, deliberately not cached: the active route
    // (and thus the save name a mid-ride swap would freeze) can change between passes.
    let name = (app.activity.session.is_some() || finish.is_some()).then(|| active_route_name(app)).flatten();
    let stats = matches!(finish, Some(TrackAction::Save)).then(|| app.ride_stats());
    tracks.reconcile(finish, app.activity.session, name.as_deref(), stats);
    if matches!(finish, Some(TrackAction::Save)) {
        rides.refresh();
        app.set_rides(rides.catalog(), rides.ids());
    }
}

/// The active route's catalog name (the ride-log save filename), or `None` when nothing is active.
fn active_route_name(app: &App) -> Option<String> {
    let i = app.activity.active_route?;
    app.routes().get(i).map(|r| r.name.as_str().to_string())
}
