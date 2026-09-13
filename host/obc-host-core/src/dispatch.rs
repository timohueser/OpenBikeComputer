//! The shared **typed executor** for the frame-stepped hosts (the desktop sim GUI, the sim's
//! headless driver, and the web demo) — #1397 S6a.
//!
//! One [`HostLoop`] owns everything a host needs between two DeviceCore passes: the outcomes and
//! facts the next pass reads, the in-flight resumable planner, a planned-but-uncommitted detour,
//! and the resident active-route parse. A frame is two calls:
//!
//! ```text
//!   let plan = host.pass(app, now, gestures, sensors, route, weather, support); // one App::run_pass
//!   host.execute(app, &mut plan, routes, rides, tracks, trips, reader, elev, platform, trace);
//! ```
//!
//! [`pass`](HostLoop::pass) hands `App` this frame's inputs and returns its [`PassPlan`];
//! [`execute`](HostLoop::execute) performs the plan's **bounded effects** against the caller's
//! repositories and leaves token-carrying outcomes for the next pass. There is one arm per domain
//! effect, and the executor performs no product policy: no ordering decision, no cascade, no
//! replacement rule — those belong to the domain that decided the effect.
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
    CatalogTag, ExternalFacts, NavigatorTag, OperationToken, OutcomeSlots, PassClock, PassInputs, PassPlan,
    PlatformSupport, Revision, StoreIdentity, StoreRevision,
};
use obc_app::dfu::{DfuEffect, DfuInstallError, DfuOutcome, DfuScanError, DfuScanReport};
use obc_app::navigator::{NavigatorEffect, NavigatorError, NavigatorOutcome, PlannerWork};
use obc_app::recorder::{RecorderEffect, RecorderError, RecorderOutcome, RideClose};
use obc_app::retention::{RetentionEffect, RetentionOutcome};
use obc_app::settings::{Settings, SettingsEffect, SettingsOutcome};
use obc_app::weather::{WeatherEffect, WeatherError, WeatherOutcome};
use obc_app::weather_alerts::AlertMarks;
use obc_app::{App, Gesture};
use obc_ports::{Sensors, SettingsSaveError};

use crate::nav::{commit_detour, commit_nav_plan, plan_detour_preview, DetourPlan, DetourReady};
use crate::trace::{DataKey, FeederCall, FeederKind, NoTrace, TraceSink};
use crate::{ActiveRouteSession, NavPlan, RideRepository, RouteRepository, TrackRepository, TripCatalog};

/// Feed the app the route catalog **with** its retention metas (epic #638, S3) — the shared re-feed
/// after a scan/delete so the auto-expiry sweep always reads device-truth retention alongside the
/// summaries. A retention-less repository returns empty metas → every route reads `Never`.
///
/// Bulk enters neither protocol: the executor fills the resident catalogs through the feeders they
/// always used, and the *outcome* reports only that the operation is over.
pub(crate) fn feed_routes(app: &mut App, routes: &dyn RouteRepository, trace: &mut dyn TraceSink) {
    let metas = routes.retention_metas();
    app.set_routes_with_meta(routes.catalog(), routes.ids(), &metas);
    trace.feeder(FeederCall::new(FeederKind::RouteCatalog, DataKey::from("host.routes"), routes.catalog().len()));
    trace.feeder(FeederCall::new(FeederKind::RouteRetention, DataKey::from("host.route-retention"), metas.len()));
}

fn feed_rides(app: &mut App, rides: &dyn RideRepository, trace: &mut dyn TraceSink) {
    app.set_rides(rides.catalog());
    trace.feeder(FeederCall::new(FeederKind::RideCatalog, DataKey::from("host.rides"), rides.catalog().len()));
}

/// Remove one object from the store, and **nothing else** (#1541).
///
/// The removal is namespace-free by design — routes, rides and trips are all objects to the store —
/// so the identity is probed against the three repositories in a fixed order. An object none of
/// them has vanished before the commit: a success for the goal state, and the one shape that must
/// not read as a failure (#1433 §13).
///
/// Free-standing, and **without an `&mut App`**: the re-read a removal implies is `CatalogMachine`'s
/// to order, and a function that cannot reach the app cannot compose one. That is the guard, and it
/// is structural rather than a grep.
fn remove_object(
    token: OperationToken<CatalogTag>,
    object: u64,
    routes: &mut dyn RouteRepository,
    rides: &mut dyn RideRepository,
    trips: &mut dyn TripCatalog,
) -> CatalogOutcome {
    let result = routes
        .delete_by_id(object)
        .and_then(|existed| if existed { Ok(true) } else { rides.delete_by_id(object) })
        .and_then(|existed| if existed { Ok(true) } else { trips.delete_by_id(object) });
    match result {
        Ok(existed) => CatalogOutcome::ObjectRemoved { token, object, existed },
        Err(error) => CatalogOutcome::Failed { token, error },
    }
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

    /// Persist the weather alert-mark record as `revision` — the second durable record (#1542), on
    /// the same terms as [`persist_settings`](HostPlatform::persist_settings) and defaulted for the
    /// same reason: a host with no durable store has nothing that can fail, and an unanswered write
    /// would park the handshake.
    fn persist_alert_marks(&mut self, marks: &AlertMarks, revision: u16) -> Result<(), SettingsSaveError> {
        let _ = (marks, revision);
        Ok(())
    }

    /// Bytes still free on the mounted medium, or the reason there is no figure.
    fn measure_free_space(&mut self) -> Result<u64, StorageInfoError> {
        Err(StorageInfoError::NotMounted)
    }

    /// Remove durable and host keys; report controller confirmation separately.
    fn forget_bond(&mut self) -> Result<obc_app::ble::ControllerClearance, obc_app::ble::BondError> {
        Err(obc_app::ble::BondError::Unsupported)
    }

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

    /// **Raise** a weather refresh with whatever plane schedules the radio, and report whether
    /// there was anything to raise it with. `false` — the default, because a host with no companion
    /// has nothing to ask — is answered as [`WeatherError::LinkLost`]. What comes back, and when,
    /// arrives as the installed-data fact; this call never waits for a bundle.
    fn request_weather_refresh(&mut self) -> bool {
        false
    }
}

/// A host with no platform work of its own.
impl HostPlatform for () {}

/// Legacy folder repositories use a session-local fallback. FlatRouteStore supplies the complete
/// physical card scope and is the only repository that admits durable retention work.
const REPOSITORY_STORE: StoreIdentity = StoreIdentity::new(1);

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
/// planned-but-uncommitted detour, and the resident active-route
/// parse. A host owns one for its lifetime.
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
    /// Which searches this host takes without starting (`--hold nav`); [`PlanHold::NONE`] for a
    /// normal frame loop.
    hold: PlanHold,
    /// The ride session this executor has opened an object for. Never cleared by a close: a session
    /// that has been served is served, whatever became of its object. See
    /// [`RecorderMachine::object_owed`](obc_app::RecorderMachine::object_owed) for why this is an id
    /// rather than "is anything recording".
    opened_session: Option<u32>,
    /// The store revision this host reports through [`note_store_commit`](HostLoop::note_store_commit).
    /// An in-process repository has none of its own, so the executor mints a monotonic one per
    /// commit it is *told* about — a boot scan, an import, an upload landing. A catalog **read**
    /// mints nothing: the domain owes its own re-reads, and inventing a revision per read would
    /// have reported the executor's own work back to it as a store that moved.
    revision: u64,
}

impl Default for HostLoop {
    /// A host loop starts by reporting the store it is built over.
    ///
    /// A `HostLoop` is constructed around the caller's repositories, so a store exists from the
    /// first pass — and `store_writable` is what admits a catalog mutation, a route plan and a ride
    /// recording. Without this the level never rose on a host that imports nothing, and every one of
    /// those capabilities read absent for the whole run. The board has no such gap: it reports its
    /// flat store's live sequence on every pass.
    fn default() -> Self {
        let mut host = HostLoop {
            inbox: Inbox::default(),
            plan: None,
            plan_token: None,
            detour_ready: None,
            hold: PlanHold::NONE,
            opened_session: None,
            revision: 0,
        };
        host.inbox.facts.note_store_revision(StoreRevision { store: REPOSITORY_STORE, revision: Revision::new(0) });
        host
    }
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
    /// The fact does not order a re-read; the domain's owed refresh does — the pass arms it via
    /// `CatalogState::note_store_moved` (`catalog_state.rs`'s own rule). Never called for a change
    /// the executor made itself: it has already re-fed the catalogs, and announcing its own work
    /// would order a rescan of it.
    pub fn note_store_commit(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.inbox
            .facts
            .note_store_revision(StoreRevision { store: REPOSITORY_STORE, revision: Revision::new(self.revision) });
    }

    /// Run **one** DeviceCore pass: whatever the executor handed back, this frame's input, and the
    /// fourteen stages.
    ///
    /// `route` is the active route opened over the host's [`ActiveRouteSession`] — the same reader
    /// the caller's render uses, so the map-matcher and the map draw agree about the geometry;
    /// `weather` is the frame's sampled weather snapshot on the same terms.
    /// The lifetime is one region covering the whole frame: `Sensors` is invariant, so the pass's
    /// borrows — this loop's inbox, the frame's gestures, the sensor ports and the open route —
    /// have to be the *same* region. Every host already holds them as sibling fields, which is what
    /// makes that free.
    #[allow(clippy::too_many_arguments)]
    pub fn pass<'a>(
        &'a mut self,
        app: &mut App,
        now: PassClock,
        gestures: &'a [Gesture],
        sensors: Sensors<'a>,
        route: Option<&'a obc_route::RouteReader<'a>>,
        weather: Option<&'a obc_app::WeatherSnapshot>,
        support: PlatformSupport,
    ) -> PassPlan {
        let Inbox { outcomes, facts, derived, ride_preview, nav_preview } = &mut self.inbox;
        app.run_pass(PassInputs {
            now,
            gestures,
            sensors,
            route,
            weather,
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
        self.sync_recorder(app, tracks);
        self.serve_effects(app, plan, session, routes, rides, trips, tracks, platform);
        self.step_plan(app, session, routes, reader, elev);
        self.serve_derived(app, plan, session, routes, rides);
        if let Some(scope) = routes.store_scope() {
            self.inbox.facts.note_store_revision(scope);
        }
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
        tracks: &mut dyn TrackRepository,
        platform: &mut dyn HostPlatform,
    ) {
        if let Some(effect) = plan.effects.catalog.take() {
            let outcome = self.serve_catalog(app, effect, routes, rides, trips);
            deliver(&mut self.inbox.outcomes.catalog, outcome, "catalog");
        }
        if let Some(effect) = plan.effects.retention.take() {
            let outcome = serve_retention(effect, routes);
            deliver(&mut self.inbox.outcomes.retention, outcome, "retention");
        }
        if let Some(effect) = plan.effects.recorder.take() {
            let opened = app.recorder.object_owed(self.opened_session).is_none();
            let outcome = serve_recorder(app, effect, tracks, opened);
            deliver(&mut self.inbox.outcomes.recorder, outcome, "recorder");
        }
        if let Some(effect) = plan.effects.navigator.take() {
            if let Some(outcome) = self.serve_navigator(app, effect, session, routes) {
                deliver(&mut self.inbox.outcomes.navigator, outcome, "navigator");
            }
        }
        if let Some(effect) = plan.effects.settings.take() {
            let outcome = match effect {
                SettingsEffect::PersistRevision { token, revision } => {
                    match platform.persist_settings(app.settings(), revision) {
                        Ok(()) => SettingsOutcome::Persisted { token, revision },
                        Err(error) => SettingsOutcome::PersistFailed { token, revision, error },
                    }
                }
                SettingsEffect::PersistAlertMarks { token, revision } => {
                    match platform.persist_alert_marks(app.alert_marks(), revision) {
                        Ok(()) => SettingsOutcome::MarksPersisted { token, revision },
                        Err(error) => SettingsOutcome::MarksPersistFailed { token, revision, error },
                    }
                }
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
        if let Some(WeatherEffect::RequestRefresh { token }) = plan.effects.weather.take() {
            let outcome = match platform.request_weather_refresh() {
                true => WeatherOutcome::Raised { token },
                false => WeatherOutcome::Failed { token, error: WeatherError::LinkLost },
            };
            deliver(&mut self.inbox.outcomes.weather, outcome, "weather");
        }
        if let Some(obc_app::ble::BondEffect::Forget { token }) = plan.effects.bond.take() {
            let outcome = obc_app::ble::BondOutcome::from_result(token, platform.forget_bond());
            deliver(&mut self.inbox.outcomes.bond, outcome, "bond");
        }
        debug_assert!(!plan.effects.has_pending(), "every effect a host can be handed has an arm above");
    }

    /// The two store operations: read the catalogs, remove one object.
    ///
    /// The removal is **namespace-free** by design — routes, rides and trips are all objects to the
    /// store — so the executor resolves the identity against the repositories in a fixed order.
    /// That is only unambiguous while the three families number their objects out of one space; the
    /// flat store does (FS7 #1389), and the simulator's folder stores do through
    /// [`RIDE_ID_BASE`](crate::RIDE_ID_BASE) and [`TRIP_ID_BASE`](crate::TRIP_ID_BASE).
    ///
    /// A trip's *cascade* is not composed here. `CatalogMachine` owns that order and sends it as one
    /// removal per member and one for the folder (#1491), so this executor performs no ordering of
    /// its own — the rule the module header states.
    ///
    /// Neither is the **re-read a removal implies** (#1541). The domain owes it and orders it as its
    /// own `ReadCatalog`, so a removal here is a removal and nothing more.
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
                app.begin_catalog_refresh();
                let scope = match routes.refresh_metadata() {
                    Ok(scope) => scope,
                    Err(error) => {
                        return CatalogOutcome::Failed {
                            token,
                            error: if error == obc_app::retention::RetentionError::RemountRequired {
                                CatalogError::RemountRequired
                            } else {
                                CatalogError::Unreadable
                            },
                        }
                    }
                };
                rides.refresh();
                trips.rescan();
                feed_routes(app, routes, &mut NoTrace);
                // After the routes, so the trips' stage ids resolve against the fresh catalog.
                trips.refeed(app);
                feed_rides(app, rides, &mut NoTrace);
                CatalogOutcome::CatalogRead { token, scope }
            }
            CatalogEffect::ExpireObject { token, object, scope } => {
                if !app.route_ids().contains(&object) {
                    return CatalogOutcome::Failed { token, error: CatalogError::Unsupported };
                }
                if !app.retention_expiry_due(object, scope) {
                    return CatalogOutcome::Failed { token, error: CatalogError::Stale };
                }
                match routes.expire_route(object, scope) {
                    Ok(existed) => CatalogOutcome::ObjectRemoved { token, object, existed },
                    Err(error) => CatalogOutcome::Failed { token, error },
                }
            }
            CatalogEffect::RemoveObject { token, object } => remove_object(token, object, routes, rides, trips),
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
                    let orig = obc_route::RouteReader::new(index, src);
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
            // One request runs the whole search here (#1400); stepped pacing
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
                let orig = session.index().zip(src).map(|(i, s)| obc_route::RouteReader::new(i, s));
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

    /// Open a ride object when Recorder owes one.
    ///
    /// The **only** thing this executor does about the ride lifecycle outside an effect, and it
    /// decides nothing: it opens the identity Recorder named. Closing is an effect, so a session
    /// that ended leaves nothing to do here — the operation that ended it already finalized or
    /// discarded the object.
    ///
    /// Asking by **id** rather than by "the session changed" is what makes this safe on an executor
    /// whose pass and effects run in either order: a close served before its verdict is applied
    /// still shows an open session, and only the id says the object for it is already served. This
    /// host runs its pass first, so it never sees that window — the board does, and both ask the
    /// same question of the same domain rather than each keeping a rule.
    ///
    /// Read the save name only while an open is owed. A failed open stays owed; success freezes
    /// the name, so a later route swap cannot rename a ride that is already recording.
    fn sync_recorder(&mut self, app: &mut App, tracks: &mut dyn TrackRepository) {
        let Some(id) = app.recorder.object_owed(self.opened_session) else { return };
        if tracks.open(id, active_route_name(app).as_deref()) {
            self.opened_session = Some(id);
        }
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
            // Filling invalidates the view, so answer with its post-fill key below.
            let preview = rides.fill_track(key.ride, app.begin_ride_profile_fill());
            let filled = preview.is_some();
            self.inbox.ride_preview = preview.unwrap_or_default();
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
            let pts = session.index().zip(src).map(|(index, s)| {
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

/// Report only the repository's typed persistence result.
fn serve_retention(effect: RetentionEffect, routes: &mut dyn RouteRepository) -> RetentionOutcome {
    let token = effect.token();
    match (effect, routes.write_metadata(effect)) {
        (RetentionEffect::WriteRouteMetadata { id, .. }, Ok(())) => {
            RetentionOutcome::RouteMetadataWritten { token, id }
        }
        (_, Err(error)) => RetentionOutcome::Failed { token, error },
        _ => RetentionOutcome::Failed { token, error: obc_app::retention::RetentionError::Unsupported },
    }
}

/// One recording operation, beside [`serve_retention`].
///
/// **Nothing is re-fed here.** A committed ride is a store change, and the re-read it implies is
/// `CatalogMachine`'s to order — Recorder tells it through the `RideFinalized` connection, so one
/// saved ride is one catalog read (#1541's rule, applied to the last producer that kept its own copy
/// of it).
fn serve_recorder(
    app: &App,
    effect: RecorderEffect,
    tracks: &mut dyn TrackRepository,
    opened: bool,
) -> RecorderOutcome {
    if !opened && matches!(effect, RecorderEffect::Append { .. } | RecorderEffect::Checkpoint { .. }) {
        return RecorderOutcome::Failed { token: effect.token(), error: RecorderError::Write };
    }
    match effect {
        RecorderEffect::Checkpoint { token } => match tracks.checkpoint() {
            true => RecorderOutcome::Checkpointed { token },
            false => RecorderOutcome::Failed { token, error: RecorderError::Write },
        },
        // The samples this ride staged and no append has taken yet go in **before** the footer:
        // they belong to the ride being saved, and the totals the footer carries already count the
        // distance and the moving time they cover. This is inside the close's own service, so it
        // orders nothing against the append rank.
        //
        // The footer facts come from Recorder, which stamped its wall-clock anchor as it minted
        // this close — nothing assembles them a second time on the way out.
        RecorderEffect::Finalize { token } => {
            if opened {
                for point in app.recorder.staged() {
                    if !tracks.append(*point) {
                        break; // the medium refused; the footer is still the honest total
                    }
                }
            }
            match tracks.finalize(app.recorder.ride_stats()) {
                RideClose::Committed(ride) => RecorderOutcome::Finalized { token, ride },
                // The object was never created — a start this store refused, which already warned
                // the rider. There is nothing to save, and saying so is **terminal**: answering
                // `Failed` would retry a close against an object that does not exist, for ever.
                RideClose::Nothing => RecorderOutcome::Discarded { token },
                RideClose::Failed => RecorderOutcome::Failed { token, error: RecorderError::Write },
            }
        }
        RecorderEffect::Discard { token } => match tracks.discard() {
            true => RecorderOutcome::Discarded { token },
            false => RecorderOutcome::Failed { token, error: RecorderError::Write },
        },
        // The staged samples, in order, for as long as the medium keeps taking them. A short write
        // is answered honestly: Recorder keeps the tail staged and offers it again next pass, so a
        // refusal costs a delay rather than a hole in the ride log. Nothing written at all is a
        // failure, which is what raises the recording warning.
        RecorderEffect::Append { token, samples } => {
            let staged = app.recorder.staged();
            let want = (samples as usize).min(staged.len());
            let written = staged[..want].iter().take_while(|point| tracks.append(**point)).count() as u16;
            match written {
                0 if want > 0 => RecorderOutcome::Failed { token, error: RecorderError::Write },
                _ => RecorderOutcome::Appended { token, samples: written },
            }
        }
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
    use obc_app::recorder::{RecorderIntent, RideClose};
    use obc_app::AppState;
    use obc_ports::{Fix, InputClock, LocationSource, RideClock, Sensors, TrackPoint};
    use obc_route::RideStats;

    #[test]
    fn bond_platform_results_return_once_with_the_admitted_token() {
        use obc_app::ble::{BondError, BondOutcome, ControllerClearance};
        struct Platform {
            result: Result<ControllerClearance, BondError>,
            calls: usize,
        }
        impl HostPlatform for Platform {
            fn forget_bond(&mut self) -> Result<ControllerClearance, BondError> {
                self.calls += 1;
                self.result
            }
        }
        for result in [
            Ok(ControllerClearance::Confirmed),
            Ok(ControllerClearance::Unconfirmed),
            Err(BondError::StoreWriteFailed),
            Err(BondError::StoreVerifyFailed),
            Err(BondError::HostKeysRemoveFailed),
            Err(BondError::QueueFull),
        ] {
            let mut app = App::new_idle(AppState::new(0, 0, 1.0));
            let mut host = HostLoop::new();
            app.state.device.ble_paired = true;
            app.state.ble_forget_requested = true;
            let mut loc = OneFix(None);
            let mut plan = host.pass(
                &mut app,
                PassClock { ride: RideClock(0), ui: InputClock(0) },
                &[],
                Sensors::new(&mut loc),
                None,
                None,
                SUPPORT,
            );
            let effect = plan.effects.bond.take().unwrap();
            plan.effects.bond.try_put(effect).unwrap();
            let mut platform = Platform { result, calls: 0 };
            let mut routes = crate::FlatRouteStore::from_bytes(&[]).unwrap();
            let mut rides = crate::MemRideStore::new(vec![]);
            let mut tracks = RecordingTrackStore::default();
            for _ in 0..2 {
                host.serve_effects(
                    &mut app,
                    &mut plan,
                    &ActiveRouteSession::new(),
                    &mut routes,
                    &mut rides,
                    &mut (),
                    &mut tracks,
                    &mut platform,
                );
            }
            assert_eq!(platform.calls, 1);
            assert_eq!(host.inbox.outcomes.bond.take(), Some(BondOutcome::from_result(effect.token(), result)));
        }
    }

    #[test]
    fn failed_removal_preserves_the_token_and_stops_repository_fallback() {
        use obc_app::catalog_state::CatalogError;

        struct FailedRide;
        impl RideRepository for FailedRide {
            fn catalog(&self) -> &[obc_app::RideEntry] {
                &[]
            }
            fn delete_by_id(&mut self, _: u64) -> Result<bool, CatalogError> {
                Err(CatalogError::RemoveFailed)
            }
            fn fill_track(&self, _: u64, _: &mut obc_route::Profile) -> Option<Vec<(i32, i32)>> {
                None
            }
        }
        struct UnreachedTrip;
        impl TripCatalog for UnreachedTrip {
            fn delete_by_id(&mut self, _: u64) -> Result<bool, CatalogError> {
                panic!("failure must stop the namespace probe");
            }
        }

        let token = obc_app::device_core::TokenSource::<CatalogTag>::new().issue();
        let mut routes = crate::FlatRouteStore::from_bytes(&[]).unwrap();
        assert_eq!(
            remove_object(token, 7, &mut routes, &mut FailedRide, &mut UnreachedTrip),
            CatalogOutcome::Failed { token, error: CatalogError::RemoveFailed },
        );
        assert_eq!(
            remove_object(token, 7, &mut routes, &mut crate::MemRideStore::new(vec![]), &mut ()),
            CatalogOutcome::ObjectRemoved { token, object: 7, existed: false },
        );
    }

    /// A host that can do everything — none of it reached by the recording path under test.
    const SUPPORT: PlatformSupport = PlatformSupport {
        detour: true,
        settings_persistence: true,
        dfu: true,
        weather: true,
        bonding: true,
        storage_space_report: true,
        retention_metadata: true,
    };

    /// One scripted fix, taken once — the pass's location port.
    struct OneFix(Option<Fix>);
    impl LocationSource for OneFix {
        fn poll(&mut self) -> Option<Fix> {
            self.0.take()
        }
    }

    /// A ride object that keeps what it was handed: every appended sample, in order, and the footer
    /// the close wrote. Enough to ask whether the samples reached the object the rider saved.
    #[derive(Default)]
    struct RecordingTrackStore {
        open: bool,
        points: Vec<TrackPoint>,
        footer: Option<RideStats>,
        failed_opens: usize,
        open_attempts: usize,
    }

    impl TrackRepository for RecordingTrackStore {
        fn open(&mut self, _session: u32, _name: Option<&str>) -> bool {
            self.open_attempts += 1;
            self.open = self.open_attempts > self.failed_opens;
            self.open
        }

        fn finalize(&mut self, stats: RideStats) -> RideClose {
            if !self.open {
                return RideClose::Nothing;
            }
            self.open = false;
            self.footer = Some(stats);
            RideClose::Committed(7)
        }

        fn discard(&mut self) -> bool {
            self.open = false;
            true
        }

        fn checkpoint(&mut self) -> bool {
            assert!(self.open, "no checkpoint against an absent recorder");
            true
        }

        fn append(&mut self, point: TrackPoint) -> bool {
            assert!(self.open, "no append against an absent recorder");
            self.points.push(point);
            true
        }
    }

    /// A ride recording `fixes` samples, none of which any append has taken: every append this
    /// executor is offered is refused, which is what leaves a tail staged for the close to find.
    /// Fixes are one second and ~11 m apart, so each one is logged and none reads as a teleport.
    fn ride_with_a_staged_tail(fixes: u32) -> (App, HostLoop) {
        let mut app = App::new(AppState::new(0, 0, 1.0)); // the map-first app is `Mode::Riding`
        let mut host = HostLoop::new();
        host.note_store_commit(); // a ride needs somewhere to put it
        for step in 0..=fixes + 2 {
            if step == 2 {
                app.recorder.request(RecorderIntent::Start);
            }
            let fix = (step > 2).then(|| Fix::at(0, 100 * step as i32));
            let mut loc = OneFix(fix);
            let now = step * 1_000;
            let mut plan = host.pass(
                &mut app,
                PassClock { ride: RideClock(now), ui: InputClock(now) },
                &[],
                Sensors::new(&mut loc),
                None,
                None,
                SUPPORT,
            );
            // Refuse every append, so the samples stay staged. A refusal is a delay, not a loss.
            if let Some(effect) = plan.effects.recorder.take() {
                assert!(matches!(effect, RecorderEffect::Append { .. }), "only appends here: {effect:?}");
                let refused = RecorderOutcome::Failed { token: effect.token(), error: RecorderError::Write };
                let _ = host.inbox.outcomes.recorder.try_put(refused);
            }
        }
        assert_eq!(app.recorder.staged().len() as u32, fixes, "the ride is holding its whole tail");
        (app, host)
    }

    #[test]
    fn failed_open_retries_before_serving_samples_or_checkpoints() {
        let bytes = obcm_testkit::build_file(
            (0, 0, 4000, 4000),
            &[],
            &[obcm_testkit::LodSpec { max_mpp: f32::INFINITY, index: vec![], chunks: vec![], chunk_size: 4096 }],
        );
        let map = crate::flat_map::FlatMap::from_bytes(&bytes).unwrap();
        // Exercise both the append and checkpoint ranks, then the unchanged never-opened Save.
        for (start, save) in [(7_000, false), (11_000, false), (7_000, true)] {
            let (mut app, mut host) = ride_with_a_staged_tail(4);
            let expected = app.recorder.staged().to_vec();
            let mut store =
                RecordingTrackStore { failed_opens: if save { usize::MAX } else { 1 }, ..Default::default() };
            let mut routes = crate::FlatRouteStore::from_bytes(&[]).unwrap();
            let mut rides = crate::MemRideStore::new(vec![]);
            let mut session = ActiveRouteSession::new();
            if save {
                app.recorder.request(RecorderIntent::Save);
            }
            for step in 0..4 {
                let now = start + step * 1_000;
                let mut loc = OneFix(None);
                let mut plan = host.pass(
                    &mut app,
                    PassClock { ride: RideClock(now), ui: InputClock(now) },
                    &[],
                    Sensors::new(&mut loc),
                    None,
                    None,
                    SUPPORT,
                );
                let first_token = (step == 0).then(|| {
                    let effect = plan.effects.recorder.take().unwrap();
                    assert!(match (save, start) {
                        (true, _) => matches!(effect, RecorderEffect::Finalize { .. }),
                        (_, 11_000) => matches!(effect, RecorderEffect::Checkpoint { .. }),
                        _ => matches!(effect, RecorderEffect::Append { .. }),
                    });
                    plan.effects.recorder.try_put(effect).unwrap();
                    effect.token()
                });
                host.execute(
                    &mut app,
                    &mut plan,
                    &mut session,
                    &mut routes,
                    &mut rides,
                    &mut store,
                    &mut (),
                    &map.reader(),
                    &mut obc_route::NullElevation,
                    &mut (),
                );
                if let Some(token) = first_token {
                    assert_eq!(host.opened_session, None, "a failed open stays owed");
                    assert!(store.points.is_empty());
                    assert_eq!(app.recorder.staged(), expected);
                    let outcome = host.inbox.outcomes.recorder.take().unwrap();
                    assert_eq!(
                        outcome,
                        if save {
                            RecorderOutcome::Discarded { token }
                        } else {
                            RecorderOutcome::Failed { token, error: RecorderError::Write }
                        }
                    );
                    host.inbox.outcomes.recorder.try_put(outcome).unwrap();
                }
            }
            assert!(app.recorder.staged().is_empty());
            if save {
                assert_eq!(store.open_attempts, 1, "a terminal close does not reopen");
                assert_eq!(app.recorder.session(), None);
                assert!(store.points.is_empty());
                assert!(store.footer.is_none(), "no object was committed");
            } else {
                assert_eq!(store.open_attempts, 2, "one retry, then the session stays acknowledged");
                assert_eq!(host.opened_session, app.recorder.session());
                assert_eq!(store.points, expected, "the staged samples reach the object once, in order");
            }
        }
    }

    /// **The close writes the samples it was holding.** A ride whose tail no append reached is
    /// saved with that tail in the ride object, ahead of the footer — because the footer's distance
    /// and moving time already count the ground those samples cover, and an object whose figures
    /// describe track it does not contain is the inconsistency this exists to prevent.
    #[test]
    fn a_saved_ride_writes_the_samples_the_close_was_holding() {
        let (mut app, mut host) = ride_with_a_staged_tail(4);
        let mut store = RecordingTrackStore::default();
        store.open(1, Some("ride"));

        app.recorder.request(RecorderIntent::Save);
        let mut loc = OneFix(None);
        let mut plan = host.pass(
            &mut app,
            PassClock { ride: RideClock(9_000), ui: InputClock(9_000) },
            &[],
            Sensors::new(&mut loc),
            None,
            None,
            SUPPORT,
        );
        let effect = plan.effects.recorder.take().expect("the rider's Save is a close");
        assert!(matches!(effect, RecorderEffect::Finalize { .. }), "{effect:?}");

        let outcome = serve_recorder(&app, effect, &mut store, true);
        assert!(matches!(outcome, RecorderOutcome::Finalized { .. }), "{outcome:?}");

        assert_eq!(store.points.len(), 4, "every staged sample reached the ride object");
        let footer = store.footer.expect("the close wrote a footer");
        let last = store.points.last().expect("…and the object is not empty");
        assert_eq!(last.t_ms, 6_000, "the object's last sample is the ride's last fix");
        assert!(footer.distance_m > 0, "the footer counts ground the object now contains");
        assert_eq!(
            footer.moving_time_s,
            (last.t_ms - store.points[0].t_ms) / 1_000,
            "and the moving time it reports is the span the samples cover"
        );
    }
}
