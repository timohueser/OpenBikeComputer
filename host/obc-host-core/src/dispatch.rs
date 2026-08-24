//! The shared **typed executor** for the frame-stepped hosts (the desktop sim GUI, the sim's
//! headless driver, and the web demo) — #1397 S6a.
//!
//! One [`HostLoop`] owns everything a host needs between two DeviceCore passes: the outcomes and
//! facts the next pass reads, the in-flight resumable planner, a planned-but-uncommitted detour,
//! and the resident active-route parse. A frame is two calls:
//!
//! ```text
//!   let plan = host.pass(app, now, gestures, sensors, support, routes);   // one App::run_pass
//!   host.execute(app, &mut plan, routes, rides, tracks, trips, reader, elev, platform, trace);
//! ```
//!
//! [`pass`](HostLoop::pass) hands `App` this frame's inputs and returns its [`PassPlan`];
//! [`execute`](HostLoop::execute) performs the plan's **bounded effects** against the caller's
//! repositories and leaves token-carrying outcomes for the next pass. There is one arm per domain
//! effect, and the executor performs no product policy: no ordering decision, no cascade, no
//! replacement rule — those belong to the domain that decided the effect.
//!
//! ## What is still on the legacy protocol here, and why
//!
//! Exactly three commands reach [`drain_residual`](HostLoop::execute)'s mailbox:
//!
//! | Command | Why | Retires in |
//! |---|---|---|
//! | `FinishTrack` | Recorder has no machine — the close is answered by a catalog re-feed, not a ride identity (`LegacyOwned::RideCloseAck`) | #1398 |
//! | `ForgetBond` | The removal is confirmed by a link-status fact, not by a reply (`LegacyOwned::BondAck`) | #1398/#1400 |
//! | `DeleteTrip` | `CatalogState::admit_intent` **refuses** a trip cascade: the bounded member read does not exist, and the sim's folder stores number routes and trips from separate counters, so a namespace-free `RemoveObject` could not tell them apart (`LegacyOwned::TripCascade` + `ObjectNamespace`) | `LegacyOwned::TripCascade::deletes_in` |
//!
//! [`RESIDUAL`] is that list as data, and [`assert_residual`] is the production assertion that
//! nothing else comes back.
//!
//! ## What stays with the caller
//!
//! Input recognition, rendering, the frame's own clock — and the [`ActiveRouteSession`]. The
//! resident route parse lives with the *host*, not in this struct, because the
//! [`RouteReader`](obc_route::RouteReader) built over it is borrowed **across** the pass and the
//! render, which a `&mut self` executor call cannot straddle. The host opens it once per frame with
//! [`ActiveRouteSession::sync`] and lends it to both.

use obc_app::catalog_state::{CatalogEffect, CatalogError, CatalogOutcome};
use obc_app::device_core::derived::{DerivedInput, DerivedInputs, DerivedTargets};
use obc_app::device_core::storage_info::{StorageInfoEffect, StorageInfoError, StorageInfoOutcome};
use obc_app::device_core::{
    ExternalFacts, NavigatorTag, OperationToken, OutcomeSlots, PassClock, PassInputs, PassPlan, PlatformSupport,
    Revision, StoreIdentity, StoreRevision,
};
use obc_app::dfu::{DfuEffect, DfuInstallError, DfuOutcome, DfuScanError, DfuScanReport};
use obc_app::navigator::{NavigatorEffect, NavigatorError, NavigatorOutcome, PlannerWork};
use obc_app::retention::{RetentionEffect, RetentionOutcome};
use obc_app::settings::{Settings, SettingsEffect, SettingsOutcome};
use obc_app::{App, DrainStatus, Gesture, HostCommand, HostMailbox, TrackAction};
use obc_ports::{Sensors, SettingsSaveError};

use crate::nav::{commit_detour, commit_nav_plan, plan_detour_preview, DetourPlan, DetourReady};
use crate::trace::{DataKey, FeederCall, FeederKind, NoTrace, TraceSink};
use crate::{ActiveRouteSession, NavPlan, RideRepository, RouteRepository, TrackRepository, TripCatalog};

/// Feed the app the route catalog **with** its retention metas (epic #638, S3) — the shared re-feed
/// after a scan/delete so the auto-expiry sweep always reads device-truth retention alongside the
/// summaries. A retention-less repository returns empty metas → every route reads `Never`.
///
/// Bulk enters neither protocol: the executor fills the resident catalogs through the feeders they
/// always used, and the *outcome* reports only the revision it read at.
pub(crate) fn feed_routes(app: &mut App, routes: &dyn RouteRepository, trace: &mut dyn TraceSink) {
    let metas = routes.retention_metas();
    app.set_routes_with_meta(routes.catalog(), routes.ids(), &metas);
    trace.feeder(FeederCall::new(FeederKind::RouteCatalog, DataKey::from("host.routes"), routes.catalog().len()));
    trace.feeder(FeederCall::new(FeederKind::RouteRetention, DataKey::from("host.route-retention"), metas.len()));
}

fn feed_rides(app: &mut App, rides: &dyn RideRepository, trace: &mut dyn TraceSink) {
    app.set_rides(rides.catalog(), rides.ids());
    trace.feeder(FeederCall::new(FeederKind::RideCatalog, DataKey::from("host.rides"), rides.catalog().len()));
}

/// The one in-flight plan a host steps — a POI route plan or a detour plan (#882). One enum slot
/// instead of two `Option`s: the two flows can never run concurrently **by construction** (Navigator
/// hands out at most one operation at a time), and only one large scratch/tile frame is alive at a
/// time (the stack rule below).
pub enum InflightPlan {
    Nav(NavPlan),
    Detour(DetourPlan),
}

/// Plan requests a host deliberately takes without starting. This is only needed by deterministic
/// hosts that must freeze a planning screen (for example the simulator's `--hold nav` snapshots);
/// ordinary frame loops use [`PlanHold::NONE`].
///
/// Under the typed executor a hold is exactly "acquire the operation and run nothing": the token
/// still comes back with the effect, so a scripted answer (`--inject nav-fail=…`) is a real answer
/// to a real operation rather than an event with nothing behind it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlanHold {
    pub(crate) route: bool,
    pub(crate) detour: bool,
}

impl PlanHold {
    /// Run every plan request.
    pub const NONE: Self = Self { route: false, detour: false };

    /// Hold route and/or detour searches: the operation is acquired, but no search is started.
    pub const fn new(route: bool, detour: bool) -> Self {
        Self { route, detour }
    }
}

/// The bounded platform work only a specific host can do — everything the shared repositories
/// cannot. Every method has a default, so a host without one simply does not implement it (the web
/// demo implements none, and `()` is the whole platform it needs).
///
/// Each answer is a *result*, never an event: the executor attaches the operation token the domain
/// issued, which is the field the legacy protocol had no room for.
pub trait HostPlatform {
    /// Persist `settings` as `revision`. The default acknowledges the write, because a host with no
    /// durable store has nothing that can fail — leaving it unanswered would park the handshake.
    fn persist_settings(&mut self, settings: &Settings, revision: u16) -> Result<(), SettingsSaveError> {
        let _ = (settings, revision);
        Ok(())
    }

    /// Bytes still free on the mounted medium, or the reason there is no figure.
    fn measure_free_space(&mut self) -> Result<u64, StorageInfoError> {
        Err(StorageInfoError::NotMounted)
    }

    /// Remove the bond with the paired phone. Confirmed by a link fact, never by a reply.
    fn forget_bond(&mut self) {}

    /// Validate the staged update package. `None` = **this host does not answer** — nothing
    /// re-polls the platform between passes, so the operation is simply never completed and the
    /// rider's next request mints a fresh one.
    fn scan_update(&mut self) -> Option<Result<DfuScanReport, DfuScanError>> {
        None
    }

    /// Arm the staged update. `None` = this host does not answer, as above — which is exactly what
    /// a progress spinner with no terminal swap behind it is.
    fn arm_install(&mut self) -> Option<Result<(), DfuInstallError>> {
        None
    }
}

/// A host with no platform work of its own.
impl HostPlatform for () {}

/// The one store a frame-stepped host mounts. These hosts have exactly one set of repositories for
/// their whole life, so the identity half of a [`StoreRevision`] is a constant and only the
/// revision moves.
const HOST_STORE: StoreIdentity = StoreIdentity::new(1);

/// The legacy classes these hosts still drain, and nothing else — the named residual of #1397 S6.
/// Prose for [`assert_residual`]'s message; [`residual`] is what actually decides, and
/// `the_residual_table_names_exactly_what_the_predicate_admits` pins the two together.
const RESIDUAL: [&str; 3] = ["FinishTrack", "ForgetBond", "DeleteTrip"];

/// Whether `command` is one of the three the typed executor deliberately leaves on the old
/// protocol. Anything else in the mailbox is a class DeviceCore already owns, and running it beside
/// the effect that carries it would perform the same work twice.
fn residual(command: &HostCommand) -> bool {
    matches!(command, HostCommand::FinishTrack(_) | HostCommand::ForgetBond | HostCommand::DeleteTrip { .. })
}

/// The two derived cues are **levels**, not one-shots: they are re-derived on every drain, so they
/// keep pending and this executor declines them every time — the plan's keyed
/// [`DerivedNeeds`](obc_app::device_core::DerivedNeeds) is what it answers instead (#1437).
fn derived_level(command: &HostCommand) -> bool {
    matches!(command, HostCommand::LoadRideTrack { .. } | HostCommand::RefreshNavPreview)
}

/// Everything the executor leaves for the next pass: the domain outcome slots, the external facts,
/// the keyed derived answers, and the bounded polylines a derived answer carries beside its key.
#[derive(Default)]
struct Inbox {
    outcomes: OutcomeSlots,
    facts: ExternalFacts,
    derived: DerivedInputs,
    ride_preview: Vec<(i32, i32)>,
    nav_preview: Vec<(i32, i32)>,
}

/// The shared host loop: the next pass's inbox, the in-flight plan (stepped once per pass), a
/// planned-but-uncommitted detour, the residual legacy mailbox, and the resident active-route
/// parse. A host owns one for its lifetime.
#[derive(Default)]
pub struct HostLoop {
    inbox: Inbox,
    plan: Option<InflightPlan>,
    /// The operation the planner is running under — the token every planner answer carries back.
    /// Held even while a search is *frozen* (`PlanHold`), which is what lets a scripted failure
    /// answer the operation the rider actually started.
    plan_token: Option<OperationToken<NavigatorTag>>,
    /// A planned detour's bytes + frozen splice context (#882), held from the search's answer until
    /// the rider commits or cancels.
    detour_ready: Option<DetourReady>,
    /// The residual legacy mailbox — [`RESIDUAL`] and nothing else.
    mailbox: HostMailbox,
    /// Which searches this host takes without starting (`--hold nav`); [`PlanHold::NONE`] for a
    /// normal frame loop.
    hold: PlanHold,
    /// The store revision a catalog read reports. The old protocol had none
    /// (`LegacyOwned::StoreRevision`); an in-process repository has none either, so the executor
    /// mints a monotonic one per read. It deliberately never becomes an
    /// [`ExternalFacts::note_store_revision`] fact: nothing changes these stores behind the
    /// executor's back, so a commit it made itself must not order a re-read of its own work.
    revision: u64,
}

impl HostLoop {
    /// A fresh host loop (nothing owed, no plan, nothing parsed).
    pub fn new() -> Self {
        HostLoop::default()
    }

    /// Take selected searches without starting them — the deterministic-harness freeze. Set once at
    /// startup; a frame loop leaves it at [`PlanHold::NONE`].
    pub fn set_plan_hold(&mut self, hold: PlanHold) {
        self.hold = hold;
    }

    /// Whether a plan (route or detour) is computing (the planning-spinner state).
    pub fn is_planning(&self) -> bool {
        self.plan.is_some()
    }

    /// The operation a frozen or running search is holding, for a host that scripts its answer.
    pub fn plan_token(&self) -> Option<OperationToken<NavigatorTag>> {
        self.plan_token
    }

    /// Offer one outcome to the next pass. The host-specific injection door (the simulator's
    /// `--inject` / `--dfu` seeds); production work reaches the inbox from [`execute`](Self::execute).
    pub fn outcomes(&mut self) -> &mut OutcomeSlots {
        &mut self.inbox.outcomes
    }

    /// Report a fact to the next pass — something that changed underneath DeviceCore that nobody
    /// asked for (an upload landing, a warning, this boot's update result).
    pub fn facts(&mut self) -> &mut ExternalFacts {
        &mut self.inbox.facts
    }

    /// Report that the object store moved **underneath** the executor — the host scanned it at
    /// boot, imported a file, or committed an upload into it.
    ///
    /// The fact does not order a re-read; `CatalogIntent::Refresh` does, and the pass raises it
    /// (`catalog_state.rs`'s own rule). Never called for a change the executor made itself: it has
    /// already re-fed the catalogs, and announcing its own work would order a rescan of it.
    pub fn note_store_commit(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.inbox
            .facts
            .note_store_revision(StoreRevision { store: HOST_STORE, revision: Revision::new(self.revision) });
    }

    /// Run **one** DeviceCore pass: whatever the executor handed back, this frame's input, and the
    /// fourteen stages.
    ///
    /// `route` is the active route opened over the host's [`ActiveRouteSession`] — the same reader
    /// the caller's render uses, so the map-matcher and the map draw agree about the geometry.
    /// The lifetime is one region covering the whole frame: `Sensors` is invariant, so the pass's
    /// borrows — this loop's inbox, the frame's gestures, the sensor ports and the open route —
    /// have to be the *same* region. Every host already holds them as sibling fields, which is what
    /// makes that free.
    pub fn pass<'a>(
        &'a mut self,
        app: &mut App,
        now: PassClock,
        gestures: &'a [Gesture],
        sensors: Sensors<'a>,
        route: Option<&'a obc_route::RouteReader<'a>>,
        support: PlatformSupport,
    ) -> PassPlan {
        let Inbox { outcomes, facts, derived, ride_preview, nav_preview } = &mut self.inbox;
        app.run_pass(PassInputs {
            now,
            gestures,
            sensors,
            route,
            support,
            outcomes,
            facts,
            derived: *derived,
            targets: DerivedTargets { ride_preview: ride_preview.as_slice(), nav_preview: nav_preview.as_slice() },
        })
    }

    /// Perform the plan's bounded work and leave token-carrying outcomes for the next pass.
    ///
    /// The phases run as **separate calls** on purpose: `serve_effects` reserves the fresh
    /// [`NavPlan`] (its ~4 KB inline tile cache) and `step_plan` reaches the ~8 KB `RouteIndex`
    /// parse in the finish tails — nesting them in one frame stacked both and overflowed the deep
    /// sim tour test's thread stack. Sequential calls keep only one large frame live at a time.
    #[allow(clippy::too_many_arguments)]
    pub fn execute(
        &mut self,
        app: &mut App,
        plan: &mut PassPlan,
        session: &mut ActiveRouteSession,
        routes: &mut dyn RouteRepository,
        rides: &mut dyn RideRepository,
        tracks: &mut dyn TrackRepository,
        trips: &mut dyn TripCatalog,
        reader: &obc_reader::Reader,
        elev: &mut dyn obc_route::ElevationSource,
        platform: &mut dyn HostPlatform,
    ) {
        // The keyed answers and their polylines were consumed by the pass that produced `plan`;
        // a later answer brings its own.
        self.inbox.derived = DerivedInputs::NONE;
        self.inbox.ride_preview.clear();
        self.inbox.nav_preview.clear();
        self.serve_effects(app, plan, session, routes, rides, trips, platform);
        self.step_plan(app, session, routes, reader, elev);
        let finish = self.drain_residual(app, trips, routes, platform);
        reconcile_track(app, rides, tracks, finish, &mut NoTrace);
        self.serve_derived(app, plan, session, routes, rides);
    }

    // ---- one arm per domain effect ----

    /// Serve every effect the plan carries, one per domain. `#[inline(never)]` so the `NavPlan`
    /// reservation doesn't bleed into the caller's frame.
    #[allow(clippy::too_many_arguments)]
    #[inline(never)]
    fn serve_effects(
        &mut self,
        app: &mut App,
        plan: &mut PassPlan,
        session: &ActiveRouteSession,
        routes: &mut dyn RouteRepository,
        rides: &mut dyn RideRepository,
        trips: &mut dyn TripCatalog,
        platform: &mut dyn HostPlatform,
    ) {
        if let Some(effect) = plan.effects.catalog.take() {
            let outcome = self.serve_catalog(app, effect, routes, rides, trips);
            deliver(&mut self.inbox.outcomes.catalog, outcome, "catalog");
        }
        if let Some(effect) = plan.effects.retention.take() {
            let outcome = serve_retention(effect, routes, rides);
            deliver(&mut self.inbox.outcomes.retention, outcome, "retention");
        }
        if let Some(effect) = plan.effects.navigator.take() {
            if let Some(outcome) = self.serve_navigator(app, effect, session, routes) {
                deliver(&mut self.inbox.outcomes.navigator, outcome, "navigator");
            }
        }
        if let Some(SettingsEffect::PersistRevision { token, revision }) = plan.effects.settings.take() {
            let outcome = match platform.persist_settings(app.settings(), revision) {
                Ok(()) => SettingsOutcome::Persisted { token, revision },
                Err(error) => SettingsOutcome::PersistFailed { token, revision, error },
            };
            deliver(&mut self.inbox.outcomes.settings, outcome, "settings");
        }
        if let Some(effect) = plan.effects.dfu.take() {
            let answer = match effect {
                DfuEffect::Scan { token } => platform.scan_update().map(|result| match result {
                    Ok(report) => DfuOutcome::ScanFinished { token, report },
                    Err(error) => DfuOutcome::ScanFailed { token, error },
                }),
                DfuEffect::ArmInstall { token } => platform.arm_install().map(|result| match result {
                    Ok(()) => DfuOutcome::InstallBegan { token },
                    Err(error) => DfuOutcome::InstallFailed { token, error },
                }),
            };
            if let Some(outcome) = answer {
                deliver(&mut self.inbox.outcomes.dfu, outcome, "dfu");
            }
        }
        if let Some(StorageInfoEffect::MeasureFreeSpace { token }) = plan.effects.storage_info.take() {
            let outcome = match platform.measure_free_space() {
                Ok(free_bytes) => StorageInfoOutcome::Measured { token, free_bytes },
                Err(error) => StorageInfoOutcome::Failed { token, error },
            };
            deliver(&mut self.inbox.outcomes.storage_info, outcome, "storage");
        }
        if plan.effects.bond.take().is_some() {
            // Bond has no machine, so nothing produces a `BondEffect` and the removal arrives as
            // the residual `ForgetBond` command instead (`LegacyOwned::BondAck`). Performing it
            // here as well would forget the bond **twice in one execute** — the exact
            // double-execution `assert_residual` exists to prevent — so this refuses like every
            // other never-produced arm, and the domain that starts emitting one has to move its
            // removal off the residual command in the same change.
            debug_assert!(false, "BondEffect has no producer: the bond removal is the residual ForgetBond command");
        }
        debug_assert!(
            !plan.effects.has_pending(),
            "recorder and weather are the domains a host outside obc-app cannot reach yet"
        );
    }

    /// The three store operations: read the catalogs, remove one object, read a trip's members.
    ///
    /// The removal is **namespace-free** by design — routes and rides are all objects to the store —
    /// so the executor resolves the identity against the repositories in a fixed order. That is only
    /// unambiguous while the two families number their objects out of one space; the flat store does
    /// (FS7 #1389), and the simulator's folder stores do since this slice (see
    /// [`crate::RIDE_ID_BASE`]).
    fn serve_catalog(
        &mut self,
        app: &mut App,
        effect: CatalogEffect,
        routes: &mut dyn RouteRepository,
        rides: &mut dyn RideRepository,
        trips: &mut dyn TripCatalog,
    ) -> CatalogOutcome {
        match effect {
            CatalogEffect::ReadCatalog { token } => {
                rides.refresh();
                trips.rescan();
                feed_routes(app, routes, &mut NoTrace);
                // After the routes, so the trips' stage ids resolve against the fresh catalog.
                trips.refeed(app);
                feed_rides(app, rides, &mut NoTrace);
                self.revision = self.revision.wrapping_add(1);
                CatalogOutcome::CatalogRead { token, revision: Revision::new(self.revision) }
            }
            CatalogEffect::RemoveObject { token, object } => {
                if routes.delete_by_id(object) {
                    feed_routes(app, routes, &mut NoTrace);
                    return CatalogOutcome::ObjectRemoved { token, object, existed: true };
                }
                if rides.delete_by_id(object) {
                    feed_rides(app, rides, &mut NoTrace);
                    return CatalogOutcome::ObjectRemoved { token, object, existed: true };
                }
                // The subject vanished before the commit — a success for the goal state, and the
                // one shape that must not read as a failure (#1433 §13).
                CatalogOutcome::ObjectRemoved { token, object, existed: false }
            }
            // Unreachable: `CatalogState::admit_intent` refuses a trip cascade, so no member read is
            // ever decided. Answered as a failure rather than dropped, so a domain that starts
            // producing one cannot wedge behind an executor that ignored it.
            CatalogEffect::ReadTripMembers { token, .. } => {
                debug_assert!(false, "the trip cascade is refused at admission — no member read exists yet");
                CatalogOutcome::Failed { token, error: CatalogError::Unreadable }
            }
        }
    }

    /// One navigation operation. `None` means the executor is still working — a search runs across
    /// frames, and its answer arrives from [`step_plan`](Self::step_plan).
    fn serve_navigator(
        &mut self,
        app: &mut App,
        effect: NavigatorEffect,
        session: &ActiveRouteSession,
        routes: &mut dyn RouteRepository,
    ) -> Option<NavigatorOutcome> {
        let token = effect.token();
        match effect {
            NavigatorEffect::Acquire { work: PlannerWork::Route(request), .. } => {
                self.plan_token = Some(token);
                if !self.hold.route {
                    self.plan = Some(InflightPlan::Nav(NavPlan::start(&request, app.settings().bike_profile_idx)));
                }
                None
            }
            NavigatorEffect::Acquire { work: PlannerWork::Detour(request), .. } => {
                self.plan_token = Some(token);
                self.detour_ready = None;
                if self.hold.detour {
                    return None;
                }
                let started = session.index().and_then(|index| {
                    let src = routes.active_source()?;
                    let orig = obc_route::RouteReader::new(index, &src);
                    DetourPlan::start(&request, app.settings().bike_profile_idx, &orig)
                });
                match started {
                    Some(plan) => {
                        self.plan = Some(InflightPlan::Detour(plan));
                        None
                    }
                    // The active route vanished / can't resolve the rejoin — answer now.
                    None => Some(NavigatorOutcome::Failed {
                        token,
                        error: NavigatorError::Plan(obc_route::NavError::NoPath),
                    }),
                }
            }
            NavigatorEffect::CommitDetour { .. } => {
                let ready = self.detour_ready.take();
                let result = commit_detour(app, routes, session.index(), ready, &mut NoTrace);
                Some(match result {
                    Ok(route) => NavigatorOutcome::DetourCommitted { token, route },
                    Err(_) => NavigatorOutcome::Failed { token, error: NavigatorError::Store },
                })
            }
            // A release is Navigator telling the executor the rider walked away: drop whatever this
            // host is holding for that family. `next_release` only issues one when the cancelled
            // family's own operation was the live one (or nothing was), so there is never another
            // family's search to protect here.
            NavigatorEffect::Release { .. } => {
                match self.plan.take() {
                    Some(InflightPlan::Nav(_)) => {}
                    // A detour search, or a preview with nothing running behind it.
                    Some(InflightPlan::Detour(_)) | None => self.detour_ready = None,
                }
                self.plan_token = None;
                Some(NavigatorOutcome::Released { token })
            }
            // One request runs the whole search here (`LegacyOwned::PlannerPacing`); stepped pacing
            // is #1400's, with the board's typed effect staging.
            NavigatorEffect::Step { .. } | NavigatorEffect::CommitRoute { .. } => {
                debug_assert!(false, "the executor paces the search: {effect:?} has no producer yet");
                None
            }
        }
    }

    /// Step an in-flight plan **once** (the board's one-step-per-pass shape) and, on a terminal
    /// outcome, commit and answer. Non-generic and `#[inline(never)]` so the `RouteIndex` parse
    /// inside the finish tails never coexists with the plan-reservation frame above.
    #[inline(never)]
    fn step_plan(
        &mut self,
        app: &mut App,
        session: &ActiveRouteSession,
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
        let Some(token) = self.plan_token else {
            debug_assert!(false, "a plan runs under the operation that started it");
            self.plan = None;
            return;
        };
        let answer = match self.plan.take().expect("just stepped it") {
            InflightPlan::Nav(plan) => {
                let result = commit_nav_plan(app, routes, terminal, plan.bytes(), plan.tile_stats(), &mut NoTrace);
                match result {
                    Ok(route) => NavigatorOutcome::PlanFinished { token, route },
                    Err(error) => NavigatorOutcome::Failed { token, error: NavigatorError::Plan(error) },
                }
            }
            InflightPlan::Detour(plan) => {
                // The detour is NOT committed here — the bytes park until the rider's commit (or a
                // cancellation's `Release` drops them). Hand the finish the resident original route
                // so it can trim the rejoin to first tail contact (#882); the source binding must
                // outlive the call, so it's bound here rather than inside a closure.
                let src = routes.active_source();
                let orig = session.index().zip(src.as_ref()).map(|(i, s)| obc_route::RouteReader::new(i, s));
                let (ready, result) = plan_detour_preview(app, terminal, plan, orig.as_ref(), &mut NoTrace);
                self.detour_ready = ready;
                match result {
                    Ok(preview) => NavigatorOutcome::DetourFinished { token, preview },
                    Err(error) => NavigatorOutcome::Failed { token, error: NavigatorError::Plan(error) },
                }
            }
        };
        self.plan_token = None;
        deliver(&mut self.inbox.outcomes.navigator, answer, "navigator");
    }

    // ---- the residual legacy half, for the classes with no domain executor ----

    /// Drain the residual mailbox: [`RESIDUAL`] and nothing else. Returns the drained ride-close
    /// action, which phase 3 reconciles against the live session.
    fn drain_residual(
        &mut self,
        app: &mut App,
        trips: &mut dyn TripCatalog,
        routes: &mut dyn RouteRepository,
        platform: &mut dyn HostPlatform,
    ) -> Option<TrackAction> {
        let status = app.drain_host_commands(&mut self.mailbox);
        debug_assert_eq!(status, DrainStatus::Complete, "a canonical-capacity mailbox always drains completely");
        let mut finish = None;
        while let Some(command) = self.mailbox.pop() {
            if derived_level(&command) {
                continue;
            }
            assert_residual(&command);
            match command {
                HostCommand::FinishTrack(action) => finish = Some(action),
                HostCommand::ForgetBond => platform.forget_bond(),
                HostCommand::DeleteTrip { id } => {
                    // The cascade: the trip's member routes, then its `.obt`. Re-feed routes first
                    // so the regrouped trip list resolves against the surviving catalog.
                    for rid in trips.member_route_ids(id) {
                        routes.delete_by_id(rid);
                    }
                    if trips.delete_by_id(id) {
                        feed_routes(app, routes, &mut NoTrace);
                        trips.refeed(app);
                    }
                }
                _ => unreachable!("assert_residual already refused it"),
            }
        }
        finish
    }

    // ---- the two derived levels ----

    /// Answer the plan's keyed derived needs. A *level*, not an operation: the key is the guard, so
    /// an answer that lands after the subject moved is simply about something else and the pass
    /// drops it (#1437).
    fn serve_derived(
        &mut self,
        app: &mut App,
        plan: &PassPlan,
        session: &mut ActiveRouteSession,
        routes: &mut dyn RouteRepository,
        rides: &mut dyn RideRepository,
    ) {
        if let Some(key) = plan.derived_needs.ride_track {
            let profile = rides.profile_by_id(key.ride);
            let filled = profile.is_some();
            // The ~5 KB profile stays DeviceCore-owned and is filled **in place**, which
            // invalidates the view — so the key the answer must carry is the one the need has
            // *after* the fill, not the one it had before.
            match profile {
                Some(profile) => *app.begin_ride_profile_fill() = profile,
                None => {
                    app.begin_ride_profile_fill();
                }
            }
            self.inbox.ride_preview = rides.preview_by_id(key.ride);
            if let Some(key) = app.derived_needs().ride_track {
                let input = if filled { DerivedInput::filled(key) } else { DerivedInput::failed(key) };
                self.inbox.derived.ride_track = Some(input);
            }
        }
        if let Some(key) = plan.derived_needs.nav_preview {
            // Re-sync first: a plan or a splice this very `execute` committed replaced the active
            // route's bytes, and the resident parse is still the old route's until the store is
            // pointed at the new one. Reading before that would answer the level `Failed` for a
            // route that is perfectly readable a line later — and a failure *is* an answer, so the
            // overview would settle with no shape at all.
            session.sync(app, routes);
            let src = routes.active_source();
            let pts = session.index().zip(src.as_ref()).map(|(index, s)| {
                obc_route::RouteReader::new(index, s).preview_polyline::<{ obc_app::NAV_PREVIEW_MAX }>()
            });
            // Answered either way, exactly as the ride-track arm above it is: a failure *is* an
            // answer (`derived.rs`'s "a dead file must cost one read, not one read per pass"), and
            // a level left unanswered is what turns an unreadable route into a headless driver
            // settling for `MAX_SETTLE_PASSES`.
            self.inbox.derived.nav_preview = Some(match pts {
                Some(pts) => {
                    self.inbox.nav_preview = pts.iter().copied().collect();
                    DerivedInput::filled(key)
                }
                None => DerivedInput::failed(key),
            });
        }
    }
}

/// Hand one outcome to its domain's slot.
///
/// [`Slot::try_put`](obc_app::device_core::Slot) hands a refused value **back** so its owner can
/// offer it again, and this executor's owner is the pass: it drains every slot the executor writes
/// at stage 1, unconditionally, so a full slot here means two answers were produced for one domain
/// inside a single `execute`. That cannot happen today — each arm serves at most one effect — and a
/// change that made it happen would otherwise lose the second answer silently.
fn deliver<T: core::fmt::Debug>(slot: &mut obc_app::device_core::Slot<T>, outcome: T, domain: &str) {
    let refused = slot.try_put(outcome);
    debug_assert!(refused.is_ok(), "{domain} answered twice in one execute: {refused:?}");
}

/// The sidecar writes. A repository stamp cannot report a failure, so the answer *is* the write —
/// what matters is that it carries the operation's token back, which the legacy protocol had no way
/// to do (`LegacyOwned::SidecarAck`). A host whose sidecar *can* fail answers
/// [`RetentionOutcome::Failed`] instead and the domain re-queues the candidate.
fn serve_retention(
    effect: RetentionEffect,
    routes: &mut dyn RouteRepository,
    rides: &mut dyn RideRepository,
) -> RetentionOutcome {
    match effect {
        RetentionEffect::WriteRouteMetadata { token, id, meta } => {
            routes.stamp_route_used(id, meta.last_used_utc);
            RetentionOutcome::RouteMetadataWritten { token, id }
        }
        RetentionEffect::WriteRideMetadata { token, id, synced_at } => {
            rides.stamp_synced_at(id, synced_at);
            RetentionOutcome::RideMetadataWritten { token, id }
        }
    }
}

/// The production assertion behind [`RESIDUAL`]: a class DeviceCore owns must never come back on
/// the old protocol, because executing it beside the effect that carries it would plan, install or
/// delete twice — and a class that quietly reappeared would be the migration coming undone.
fn assert_residual(command: &HostCommand) {
    assert!(
        residual(command),
        "{command:?} is DeviceCore's now — running it here would repeat the effect that carries it \
         (the residual is {RESIDUAL:?})"
    );
}

/// Phase 3 — reconcile the ride recorder: the drained finish action + the live session, with the
/// save name (the active route) and totals (for a `Save`). Refresh + re-feed the catalog so a saved
/// ride appears without a relaunch — the legacy ride close is answered by exactly that re-feed
/// rather than by a ride identity (`LegacyOwned::RideCloseAck`).
pub(crate) fn reconcile_track(
    app: &mut App,
    rides: &mut dyn RideRepository,
    tracks: &mut dyn TrackRepository,
    finish: Option<TrackAction>,
    trace: &mut dyn TraceSink,
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
        feed_rides(app, rides, trace);
    }
}

/// The active route's catalog name (the ride-log save filename), or `None` when nothing is active.
pub(crate) fn active_route_name(app: &App) -> Option<String> {
    let i = app.active_route_index()?;
    app.routes().get(i).map(|r| r.name.as_str().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_app::{DfuAction, TrackAction};

    /// [`RESIDUAL`] is the prose an `assert_residual` failure prints and [`residual`] is what
    /// actually decides — so they have to name the same three classes. A class added to one and not
    /// the other would either fail with a message that lists the wrong residual, or quietly widen
    /// the residual without anyone reading the list noticing.
    #[test]
    fn the_residual_table_names_exactly_what_the_predicate_admits() {
        let admitted = [
            ("FinishTrack", HostCommand::FinishTrack(TrackAction::Save)),
            ("FinishTrack", HostCommand::FinishTrack(TrackAction::Discard)),
            ("ForgetBond", HostCommand::ForgetBond),
            ("DeleteTrip", HostCommand::DeleteTrip { id: 7 }),
        ];
        for (name, command) in admitted {
            assert!(residual(&command), "{command:?} is in the residual and the predicate refuses it");
            assert!(RESIDUAL.contains(&name), "{name} is admitted but not in the printed table");
        }
        assert_eq!(RESIDUAL.len(), 3, "three classes, and the table says which");

        // Everything DeviceCore took over is refused, including the two derived levels — those are
        // declined earlier, by `derived_level`, and must never reach the assertion at all.
        for command in [
            HostCommand::RescanStore { commits: 1 },
            HostCommand::DeleteRoute { id: 1 },
            HostCommand::DeleteRide { id: 1 },
            HostCommand::StampRouteUsed { id: 1, utc: 2 },
            HostCommand::StampRideSynced { id: 1, utc: 2 },
            HostCommand::CancelRoutePlan,
            HostCommand::CancelDetour,
            HostCommand::CommitDetour,
            HostCommand::Dfu(DfuAction::Scan),
            HostCommand::PersistSettings { revision: 1 },
            HostCommand::ScanCardFree,
        ] {
            assert!(!residual(&command), "{command:?} is DeviceCore's — the executor must refuse it");
        }
        for command in [HostCommand::LoadRideTrack { id: 1 }, HostCommand::RefreshNavPreview] {
            assert!(derived_level(&command), "{command:?} is a level the plan's keys answer");
        }
    }
}
