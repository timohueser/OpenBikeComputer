use obc_app::catalog_state::{CatalogEffect, CatalogError, CatalogOutcome};
use obc_app::device_core::derived::{DerivedInput, DerivedInputs, DerivedTargets};
use obc_app::device_core::storage_info::{StorageInfoEffect, StorageInfoError, StorageInfoOutcome};
use obc_app::device_core::{
    CatalogTag, ExternalFacts, NavigatorTag, OperationToken, OutcomeSlots, PassClock, PassInputs, PassPlan,
    PlatformSupport, Revision, StoreIdentity, StoreRevision,
};
use obc_app::dfu::{DfuEffect, DfuInstallError, DfuOutcome, DfuScanError, DfuScanReport};
use obc_app::metadata::{MetadataEffect, MetadataOutcome};
use obc_app::navigator::{NavigatorEffect, NavigatorError, NavigatorOutcome, PlanFamily, PlannerProgress, PlannerWork};
use obc_app::recorder::{RecorderEffect, RecorderError, RecorderOutcome, RideClose};
use obc_app::settings::{Settings, SettingsEffect, SettingsOutcome};

use obc_app::{App, Gesture};
use obc_ports::{Sensors, SettingsSaveError};

use crate::nav::{commit_detour, commit_nav_plan, plan_detour_preview, DetourPlan, DetourReady};
use crate::trace::{DataKey, FeederCall, FeederKind, NoTrace, TraceSink};
use crate::{ActiveRouteSession, NavPlan, RideRepository, RouteRepository, TrackRepository, TripCatalog};

pub(crate) fn feed_routes(app: &mut App, routes: &dyn RouteRepository, trace: &mut dyn TraceSink) {
    app.set_routes_with_ids(routes.catalog(), routes.ids());
    app.set_internal_routes(routes.internal_routes());
    app.set_unaccepted_routes(routes.unaccepted_routes());
    for (index, object) in routes.ids().iter().enumerate() {
        if !app.can_reconcile_reviews() {
            break;
        }
        if !app.route_unaccepted(index) {
            continue;
        }
        let Some(store) = routes.store_scope().map(|scope| scope.store) else { break };
        let Some(fingerprint) = routes.fingerprint(*object) else { continue };
        let source =
            obc_formats::obcr::RouteSourceKey { store: store.bytes(), object: *object, revision: fingerprint.revision };
        if app.retains_find_review(source) {
            continue;
        }
        if routes.pin_review(source).is_some_and(|bytes| {
            obc_route::RouteObjectInfo::read(&bytes).is_ok_and(|info| {
                info.assistant_candidate && info.attribution_map.is_some_and(|map| map.store == source.store)
            })
        }) {
            app.reconcile_review_candidate(source);
        }
    }
    trace.feeder(FeederCall::new(FeederKind::RouteCatalog, DataKey::from("host.routes"), routes.catalog().len()));
}

fn feed_rides(app: &mut App, rides: &dyn RideRepository, trace: &mut dyn TraceSink) {
    app.set_rides(rides.catalog());
    trace.feeder(FeederCall::new(FeederKind::RideCatalog, DataKey::from("host.rides"), rides.catalog().len()));
}

/// Remove one object from the store, and **nothing else**.
///
/// Execute only the family selected by CatalogMachine. Absence never probes another repository.
fn remove_object(
    token: OperationToken<CatalogTag>,
    object: u64,
    kind: obc_app::catalog_state::CatalogObjectKind,
    routes: &mut dyn RouteRepository,
    rides: &mut dyn RideRepository,
    trips: &mut dyn TripCatalog,
) -> CatalogOutcome {
    use obc_app::catalog_state::CatalogObjectKind;
    let result = match kind {
        CatalogObjectKind::Route => routes.delete_by_id(object),
        CatalogObjectKind::Ride => rides.delete_by_id(object),
        CatalogObjectKind::Trip => trips.delete_by_id(object),
    };
    match result {
        Ok(existed) => CatalogOutcome::ObjectRemoved { token, object, existed },
        Err(error) => CatalogOutcome::Failed { token, error },
    }
}

/// The one in-flight plan a host steps — a POI route plan or a detour plan. One enum slot instead
/// of two `Option`s: the two flows can never run concurrently by construction, because Navigator
/// hands out at most one operation at a time, and only one large scratch or tile frame is alive at
/// a time.
pub enum InflightPlan {
    Nav(NavPlan),
    Detour(DetourPlan),
    /// The rest of the day before a trip day: nothing to search, so it is ready at once.
    Rest(DetourReady),
    Ready(NavPlan, obc_route::RouteStats),
    Visit(Box<crate::nav_visit::VisitPlan>),
    VisitReady(Box<crate::nav_visit::VisitPlan>, obc_route::RouteStats),
}

/// Sources admitted together; the reader is always made by the leased FlatMap.
struct PlanSources {
    map: crate::flat_store::ObjectSource,
    map_fingerprint: obc_formats::assistant::PayloadFingerprint,
    original: Option<(crate::RouteLease, Box<obc_route::RouteIndex>)>,
}
impl PlanSources {
    fn current(&self, map: &crate::flat_map::FlatMap, routes: &dyn RouteRepository) -> bool {
        self.map.same_revision(&map.source())
            && self.map.is_current()
            && self.original.as_ref().is_none_or(|(held, _)| routes.pin_active().is_some_and(|now| held.matches(&now)))
    }
    fn rebind(
        &mut self,
        context: obc_app::navigator::ReviewContext,
        map: &crate::flat_map::FlatMap,
        routes: &dyn RouteRepository,
    ) -> bool {
        let current_map = map.source();
        if !current_map.is_current()
            || (obc_formats::obcr::RouteSourceKey {
                store: current_map.store_id().0,
                object: current_map.id().0,
                revision: current_map.revision().0,
            }) != context.map
            || current_map.fingerprint() != Some(self.map_fingerprint)
        {
            return false;
        }
        let original = match (context.original, self.original.as_ref()) {
            (None, None) => None,
            (Some(expected), Some(_)) => {
                let Some(crate::RouteLease::Flat(source)) = routes.pin_active() else { return false };
                if source.store_id().0 != context.store.bytes()
                    || !source.is_current()
                    || source.fingerprint() != Some(expected)
                {
                    return false;
                }
                Some(crate::RouteLease::Flat(source))
            }
            _ => return false,
        };
        self.map = current_map;
        if let (Some((held, _)), Some(current)) = (self.original.as_mut(), original) {
            *held = current;
        }
        true
    }
    fn original(&self) -> Option<obc_route::RouteReader<'_>> {
        self.original.as_ref().map(|(source, index)| obc_route::RouteReader::new(index, source))
    }
}
struct Preview {
    ready: DetourReady,
    sources: PlanSources,
}

/// Plan requests a host deliberately takes without starting. Only a deterministic host that must
/// freeze a planning screen needs this; ordinary frame loops use [`PlanHold::NONE`].
///
/// A hold is exactly "acquire the operation and run nothing": the token still comes back with the
/// effect, so a scripted answer is a real answer to a real operation rather than an event with
/// nothing behind it.
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
/// cannot. Every method has a default, so a host without one simply does not implement it; the web
/// demo implements none, and `()` is the whole platform it needs.
///
/// Each answer is a result, never an event: the executor attaches the operation token the domain
/// issued.
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

    /// Remove durable and host keys; report controller confirmation separately.
    fn forget_bond(&mut self) -> Result<obc_app::ble::ControllerClearance, obc_app::ble::BondError> {
        Err(obc_app::ble::BondError::Unsupported)
    }

    /// Validate the staged update package. `None` means this host does not answer — nothing
    /// re-polls the platform between passes, so the operation is never completed and the rider's
    /// next request mints a fresh one.
    fn scan_update(&mut self) -> Option<Result<DfuScanReport, DfuScanError>> {
        None
    }

    /// Arm the staged update. `None` means this host does not answer, as above, which is exactly
    /// what a progress spinner with no terminal swap behind it is.
    fn arm_install(&mut self) -> Option<Result<(), DfuInstallError>> {
        None
    }
}

/// A host with no platform work of its own.
impl HostPlatform for () {}

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
    trace: Option<Box<dyn TraceSink>>,
    inbox: Inbox,
    plan: Option<InflightPlan>,
    /// The operation the planner is running under — the token every planner answer carries back.
    /// Held even while a search is frozen (`PlanHold`), which is what lets a scripted failure answer
    /// the operation the rider actually started.
    plan_token: Option<OperationToken<NavigatorTag>>,
    /// A planned detour's bytes + frozen splice context, held from the search's answer until
    /// the rider commits or cancels.
    detour_ready: Option<Preview>,
    sources: Option<PlanSources>,
    publication: Option<crate::RoutePublication>,
    releasing: Option<NavigatorEffect>,
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
    /// commit it is told about — a boot scan, an import, an upload landing. A catalog read mints
    /// nothing: the domain owes its own re-reads, and inventing a revision per read would report
    /// the executor's own work back to it as a store that moved.
    revision: u64,
}

impl Default for HostLoop {
    /// A host loop starts by reporting the store it is built over.
    ///
    /// A `HostLoop` is constructed around the caller's repositories, so a store exists from the
    /// first pass — and `store_writable` is what admits a catalog mutation, a route plan and a ride
    /// recording. Without this the level never rises on a host that imports nothing. The board has
    /// no such gap: it reports its flat store's live sequence on every pass.
    fn default() -> Self {
        let mut host = HostLoop {
            trace: None,
            inbox: Inbox::default(),
            plan: None,
            plan_token: None,
            detour_ready: None,
            sources: None,
            publication: None,
            releasing: None,
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

    /// Attach an observer to this host session. It cannot change the pass or its results.
    pub fn set_trace(&mut self, trace: Box<dyn TraceSink>) {
        self.trace = Some(trace);
    }

    /// Take selected searches without starting them — the deterministic-harness freeze. Set once at
    /// startup; a frame loop leaves it at [`PlanHold::NONE`].
    pub fn set_plan_hold(&mut self, hold: PlanHold) {
        self.hold = hold;
    }

    /// Whether a plan (route or detour) is computing (the planning-spinner state).
    pub fn is_planning(&self) -> bool {
        self.plan.is_some() || self.releasing.is_some()
    }

    /// Navigator owns work or sources that must be released before replacing this host loop.
    pub fn owns_navigation(&self) -> bool {
        self.plan_token.is_some()
            || self.plan.is_some()
            || self.sources.is_some()
            || self.detour_ready.is_some()
            || self.publication.is_some()
            || self.releasing.is_some()
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

    /// Report that the object store moved underneath the executor — the host scanned it at boot,
    /// imported a file, or committed an upload into it.
    ///
    /// The fact does not order a re-read; the domain's owed refresh does. Never called for a change
    /// the executor made itself: it has already re-fed the catalogs, and announcing its own work
    /// would order a rescan of it.
    pub fn note_store_commit(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.inbox
            .facts
            .note_store_revision(StoreRevision { store: REPOSITORY_STORE, revision: Revision::new(self.revision) });
    }

    #[allow(clippy::too_many_arguments)]
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
        if let Some(trace) = &mut self.trace {
            trace.pass_input(now, gestures, outcomes, facts, app.top_screen().name());
        }
        let plan = app.run_pass(PassInputs {
            now,
            gestures,
            sensors,
            route,

            support,
            outcomes,
            facts,
            derived: *derived,
            targets: DerivedTargets { ride_preview: ride_preview.as_slice(), nav_preview: nav_preview.as_slice() },
        });
        if let Some(trace) = &mut self.trace {
            trace.pass_output(&plan, app.top_screen().name());
        }
        plan
    }

    /// Perform the plan's bounded work and leave token-carrying outcomes for the next pass.
    ///
    /// Acquisition, stepping and commit use separate shallow calls so their large parse and
    /// planner frames do not overlap. FlatMap binds the reader to its exact retained source.
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
        map: &crate::flat_map::FlatMap,
        elev: &mut dyn obc_route::ElevationSource,
        platform: &mut dyn HostPlatform,
    ) {
        // The keyed answers and their polylines were consumed by the pass that produced `plan`;
        // a later answer brings its own.
        let source = map.source();
        app.bind_place_map(Some(obc_formats::obcr::RouteSourceKey {
            store: source.store_id().0,
            object: source.id().0,
            revision: source.revision().0,
        }));
        self.inbox.derived = DerivedInputs::NONE;
        self.inbox.ride_preview.clear();
        self.inbox.nav_preview.clear();
        self.sync_recorder(app, tracks);
        let navigation = plan.effects.navigator.take().or_else(|| self.releasing.take());
        if app.assistant_review_status() == obc_app::navigator::ReviewStatus::Accepted && self.plan.is_none() {
            self.publication = None;
            self.sources = None;
        }
        if app.assistant_needs_recovery() {
            if let Ok(checkpoint) = routes.read_checkpoint() {
                if let Some(scope) = routes.store_scope() {
                    let recovering = app.assistant_review_status() == obc_app::navigator::ReviewStatus::Unresolved;
                    app.offer_assistant_checkpoint(scope.store, checkpoint);
                    if recovering && app.assistant_review_status() != obc_app::navigator::ReviewStatus::Unresolved {
                        if let Some(context) = app.assistant_review_context() {
                            if !self.sources.as_mut().is_some_and(|sources| sources.rebind(context, map, routes)) {
                                app.invalidate_assistant_preview();
                            }
                        }
                    }
                }
            }
        }
        if let Some(expected) = app.requested_assistant_resume() {
            let index = routes.ids().iter().position(|id| *id == expected.object);
            let exact = routes.fingerprint(expected.object) == Some(expected);
            let changed = routes.sync_active(index.filter(|_| exact));
            session.reparse(changed, routes);
            let reader = session
                .index()
                .zip(routes.active_source())
                .map(|(index, source)| obc_route::RouteReader::new(index, source));
            app.prepare_assistant_resume(reader.as_ref());
            session.sync(app, routes);
        }
        if let Some(id) = app.requested_route_checkpoint() {
            session.sync(app, routes);
            let source = routes.fingerprint(id).zip(session.index().zip(routes.active_source())).and_then(
                |(route, (index, bytes))| {
                    // The catalog revision and the entry head can differ; only bytes of the exact
                    // named length are the ones this length and flag were read from.
                    (bytes.len() == route.length).then(|| {
                        let reader = obc_route::RouteReader::new(index, bytes);
                        obc_app::navigator::RouteCheckpointSource {
                            route,
                            distance_m: reader.total_distance_m,
                            unresolved_avoidance: reader.has_unresolved_avoidance(),
                        }
                    })
                },
            );
            app.offer_route_checkpoint(id, source);
        }
        if let Some(effect @ MetadataEffect::WriteCheckpoint { token, scope }) =
            plan.effects.metadata.take_if(|effect| matches!(effect, MetadataEffect::WriteCheckpoint { .. }))
        {
            let resume = app.assistant_review_status() == obc_app::navigator::ReviewStatus::Saving
                && app.assistant_preview().is_none();
            let current_map = map.source();
            let clearing = app.assistant_checkpoint_payload(token).is_some_and(|change| change.next.is_none());
            let current = scope.is_some_and(|scope| app.assistant_store_matches(scope.store))
                && (clearing
                    || (!resume
                        || routes.resume_map_matches(current_map.is_current().then(|| {
                            obc_formats::obcr::RouteSourceKey {
                                store: current_map.store_id().0,
                                object: current_map.id().0,
                                revision: current_map.revision().0,
                            }
                        })))
                        && app.assistant_review_context().is_none_or(|context| {
                            self.sources.as_ref().is_some_and(|sources| sources.current(map, routes))
                                && context.profile == app.settings().bike_type
                                && context.map
                                    == (obc_formats::obcr::RouteSourceKey {
                                        store: current_map.store_id().0,
                                        object: current_map.id().0,
                                        revision: current_map.revision().0,
                                    })
                        }));
            if !current || !app.assistant_checkpoint_submission(token) {
                plan.effects.metadata.take();
                let outcome = if current {
                    MetadataOutcome::Cancelled { token }
                } else {
                    MetadataOutcome::Failed { token, error: obc_app::metadata::MetadataError::Stale }
                };
                deliver(&mut self.inbox.outcomes.metadata, outcome, "metadata");
            } else {
                let _ = plan.effects.metadata.try_put(effect);
            }
        }
        self.serve_effects(app, plan, routes, rides, trips, tracks, platform);
        if let Some(effect) = navigation {
            if let Some(outcome) = self.serve_navigator(app, effect, routes, trips, map, elev) {
                deliver(&mut self.inbox.outcomes.navigator, outcome, "navigator");
            }
        }
        self.serve_derived(app, plan, session, routes, rides);
        if let Some(scope) = routes.store_scope() {
            self.inbox.facts.note_store_revision(scope);
        }
        if let Some(trace) = &mut self.trace {
            trace.executed(&self.inbox.outcomes, app.top_screen().name());
        }
    }

    /// Serve every effect the plan carries, one per domain. `#[inline(never)]` so the `NavPlan`
    /// reservation doesn't bleed into the caller's frame.
    #[allow(clippy::too_many_arguments)]
    #[inline(never)]
    fn serve_effects(
        &mut self,
        app: &mut App,
        plan: &mut PassPlan,
        routes: &mut dyn RouteRepository,
        rides: &mut dyn RideRepository,
        trips: &mut dyn TripCatalog,
        tracks: &mut dyn TrackRepository,
        platform: &mut dyn HostPlatform,
    ) {
        routes.set_route_clock(app.clock_trusted().then(|| app.wall_unix_now()));
        if let Some(effect) = plan.effects.catalog.take() {
            let outcome = self.serve_catalog(app, effect, routes, rides, trips);
            deliver(&mut self.inbox.outcomes.catalog, outcome, "catalog");
        }
        if let Some(MetadataEffect::WriteProgress { token, scope }) =
            plan.effects.metadata.take_if(|effect| matches!(effect, MetadataEffect::WriteProgress { .. }))
        {
            let outcome = match app.trip_progress_payload(token) {
                Some(record) if scope.is_some() && scope.map(|s| s.store) == routes.store_scope().map(|s| s.store) => {
                    let keys: Vec<u64> = app.trips().iter().map(|t| t.key).collect();
                    match trips.write_progress(record.clone(), &keys) {
                        Ok(()) => MetadataOutcome::ProgressWritten { token },
                        Err(error) => MetadataOutcome::Failed { token, error },
                    }
                }
                _ => MetadataOutcome::Cancelled { token },
            };
            deliver(&mut self.inbox.outcomes.metadata, outcome, "metadata");
        }
        if let Some(MetadataEffect::WriteCheckpoint { token, scope }) = plan.effects.metadata.take() {
            let outcome = match scope.zip(app.assistant_checkpoint_payload(token)) {
                Some((scope, change))
                    if change.next.is_none()
                        && routes.store_scope().is_some_and(|current| {
                            scope.store == current.store && scope.revision != current.revision
                        }) =>
                {
                    MetadataOutcome::Cancelled { token }
                }
                Some((scope, change)) => match routes.write_checkpoint(scope, change) {
                    Ok(()) => MetadataOutcome::CheckpointWritten { token },
                    Err(error) => MetadataOutcome::Failed { token, error },
                },
                None => MetadataOutcome::Cancelled { token },
            };
            deliver(&mut self.inbox.outcomes.metadata, outcome, "metadata");
        }
        if let Some(effect) = plan.effects.recorder.take() {
            let opened = app.recorder.object_owed(self.opened_session).is_none();
            let outcome = serve_recorder(app, effect, tracks, opened);
            deliver(&mut self.inbox.outcomes.recorder, outcome, "recorder");
        }
        if let Some(effect) = plan.effects.settings.take() {
            let outcome = match effect {
                SettingsEffect::PersistRevision { token, revision } => {
                    match platform.persist_settings(app.settings(), revision) {
                        Ok(()) => SettingsOutcome::Persisted { token, revision },
                        Err(error) => SettingsOutcome::PersistFailed { token, revision, error },
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

        if let Some(obc_app::ble::BondEffect::Forget { token }) = plan.effects.bond.take() {
            let outcome = obc_app::ble::BondOutcome::from_result(token, platform.forget_bond());
            deliver(&mut self.inbox.outcomes.bond, outcome, "bond");
        }
        debug_assert!(!plan.effects.has_pending(), "every effect a host can be handed has an arm above");
    }

    /// The two store operations: read the catalogs, remove one object.
    ///
    /// A trip's cascade is not composed here. `CatalogMachine` owns that order and sends it as one
    /// removal per member and one for the folder, so this executor performs no ordering of its own.
    /// Neither is the re-read a removal implies: the domain owes it and orders it as its own
    /// `ReadCatalog`.
    fn serve_catalog(
        &mut self,
        app: &mut App,
        effect: CatalogEffect,
        routes: &mut dyn RouteRepository,
        rides: &mut dyn RideRepository,
        trips: &mut dyn TripCatalog,
    ) -> CatalogOutcome {
        match effect {
            CatalogEffect::RemoveReview { token, source } => {
                let publication = crate::RoutePublication {
                    store: Some(StoreIdentity::from_bytes(source.store)),
                    id: source.object,
                    revision: source.revision,
                };
                match routes.retract_nav_route(publication) {
                    Ok(()) | Err(CatalogError::Stale) => CatalogOutcome::ReviewRemoved { token, source },
                    Err(error) => CatalogOutcome::Failed { token, error },
                }
            }
            CatalogEffect::CleanupRoute { token, before_utc, store } => {
                let active = app.active_route_index().and_then(|i| app.route_ids().get(i).copied());
                match routes.cleanup_route(before_utc, store, active) {
                    Ok(Some(object)) => CatalogOutcome::ObjectRemoved { token, object, existed: true },
                    Ok(None) => CatalogOutcome::CleanupFinished { token },
                    Err(error) => CatalogOutcome::Failed { token, error },
                }
            }
            CatalogEffect::ReadCatalog { token } => {
                app.begin_catalog_refresh();
                let scope = match routes.refresh_metadata() {
                    Ok(scope) => scope,
                    Err(error) => {
                        return CatalogOutcome::Failed {
                            token,
                            error: if error == obc_app::metadata::MetadataError::RemountRequired {
                                CatalogError::RemountRequired
                            } else {
                                CatalogError::Unreadable
                            },
                        }
                    }
                };
                let ride_scope = match rides.refresh_metadata() {
                    Ok(scope) => scope,
                    Err(error) => return CatalogOutcome::Failed { token, error: catalog_metadata_error(error) },
                };
                if let Err(error) = trips.rescan() {
                    return CatalogOutcome::Failed { token, error };
                }
                if routes.store_scope() != scope
                    || rides.store_scope() != ride_scope
                    || ride_scope.is_some_and(|ride_scope| Some(ride_scope) != scope)
                    || trips.store_scope().is_some_and(|trip| Some(trip) != scope)
                {
                    return CatalogOutcome::Failed { token, error: CatalogError::Stale };
                }
                feed_routes(app, routes, self.trace.as_deref_mut().unwrap_or(&mut NoTrace));
                // After the routes, so the trips' stage ids resolve against the fresh catalog.
                trips.refeed(app);
                feed_rides(app, rides, self.trace.as_deref_mut().unwrap_or(&mut NoTrace));
                CatalogOutcome::CatalogRead { token, scope }
            }
            CatalogEffect::RemoveObject { token, object, kind } => {
                remove_object(token, object, kind, routes, rides, trips)
            }
        }
    }

    /// Perform only the physical operation Navigator requested in this pass.
    #[inline(never)]
    fn serve_navigator(
        &mut self,
        app: &mut App,
        effect: NavigatorEffect,
        routes: &mut dyn RouteRepository,
        trips: &dyn TripCatalog,
        map: &crate::flat_map::FlatMap,
        elev: &mut dyn obc_route::ElevationSource,
    ) -> Option<NavigatorOutcome> {
        let token = effect.token();
        self.plan_token = Some(token);
        let failed = |error| Some(NavigatorOutcome::Failed { token, error });
        match effect {
            NavigatorEffect::Acquire { work, .. } => self.acquire_plan(app, token, work, routes, trips, map),
            NavigatorEffect::Step { .. } => {
                if !self.sources.as_ref().is_some_and(|s| s.current(map, routes)) {
                    return failed(NavigatorError::SourceChanged);
                }
                self.step_plan(app, token, map, elev)
            }
            NavigatorEffect::CommitRoute { .. } => {
                if !self.sources.as_ref().is_some_and(|s| s.current(map, routes)) {
                    return failed(NavigatorError::SourceChanged);
                }
                let (bytes, stats, tile_stats) = match self.plan.as_ref() {
                    Some(InflightPlan::Ready(plan, stats)) => (plan.bytes(), stats, plan.tile_stats()),
                    Some(InflightPlan::VisitReady(plan, stats)) => (plan.bytes(), stats, Default::default()),
                    _ => return failed(NavigatorError::Workspace),
                };
                if let Some(context) = app.assistant_review_context() {
                    if context.purpose == obc_app::navigator::ReviewPurpose::Destination
                        && app.assistant_visit_target().is_some_and(|target| {
                            target.validate_destination(&obc_formats::io::SliceSource(bytes), context.profile).is_err()
                        })
                    {
                        return failed(NavigatorError::Unavailable);
                    }
                    return Some(match routes.publish_review_route(bytes) {
                        Ok(publication) => {
                            self.publication = Some(publication);
                            let Some(source) = routes.fingerprint(publication.id) else {
                                return failed(NavigatorError::DurabilityUnknown);
                            };
                            let preview = obc_app::navigator::ReviewedRoute::read(
                                source,
                                &obc_formats::io::SliceSource(bytes),
                                app.assistant_review_context().unwrap(),
                            );
                            feed_routes(app, routes, self.trace.as_deref_mut().unwrap_or(&mut NoTrace));
                            match preview {
                                Ok(preview) => {
                                    let outcome = app.assistant_preview_outcome(token, preview);
                                    let src = obc_formats::io::SliceSource(bytes);
                                    if let Ok(index) = obc_route::RouteIndex::read(&src) {
                                        let reader = obc_route::RouteReader::new(&index, &src);
                                        let points =
                                            match reader.assistant_preview_polyline::<{ obc_app::NAV_PREVIEW_MAX }>() {
                                                Ok(points) => points,
                                                Err(_) => return failed(NavigatorError::Unavailable),
                                            };
                                        if matches!(outcome, NavigatorOutcome::ReviewReady { .. })
                                            && !app.set_assistant_preview_shape(token, preview.source, &points)
                                        {
                                            return failed(NavigatorError::SourceChanged);
                                        }
                                        app.assistant_easier_bounds(preview.source, index.bbox);
                                    }
                                    outcome
                                }
                                Err(error) => NavigatorOutcome::Failed { token, error },
                            }
                        }
                        Err(error) => NavigatorOutcome::Failed { token, error },
                    });
                }
                Some(
                    match commit_nav_plan(
                        app,
                        routes,
                        Ok(*stats),
                        bytes,
                        tile_stats,
                        self.trace.as_deref_mut().unwrap_or(&mut NoTrace),
                    ) {
                        Ok(publication) => {
                            self.publication = Some(publication);
                            NavigatorOutcome::PlanFinished { token, route: publication.id }
                        }
                        Err(error) => NavigatorOutcome::Failed { token, error },
                    },
                )
            }
            NavigatorEffect::CommitDetour { .. } => {
                let Some(preview) = self.detour_ready.as_ref() else { return failed(NavigatorError::Workspace) };
                if !preview.sources.current(map, routes) {
                    return failed(NavigatorError::SourceChanged);
                }
                let Some(orig) = preview.sources.original() else { return failed(NavigatorError::Workspace) };
                Some(
                    match commit_detour(
                        app,
                        routes,
                        &orig,
                        &preview.ready,
                        self.trace.as_deref_mut().unwrap_or(&mut NoTrace),
                    ) {
                        Ok(publication) => {
                            self.publication = Some(publication);
                            NavigatorOutcome::DetourCommitted { token, route: publication.id }
                        }
                        Err(error) => NavigatorOutcome::Failed { token, error },
                    },
                )
            }
            NavigatorEffect::Release { family, retain_result, .. } => {
                let keep_preview = retain_result && self.publication.is_none();
                if !retain_result {
                    if let Some(publication) = self.publication.filter(|publication| {
                        !publication.store.is_some_and(|store| {
                            app.retains_find_review(obc_formats::obcr::RouteSourceKey {
                                store: store.bytes(),
                                object: publication.id,
                                revision: publication.revision,
                            })
                        })
                    }) {
                        match routes.retract_nav_route(publication) {
                            Ok(()) => feed_routes(app, routes, self.trace.as_deref_mut().unwrap_or(&mut NoTrace)),
                            Err(CatalogError::RemoveFailed) => {
                                self.releasing = Some(effect);
                                return None;
                            }
                            Err(CatalogError::Stale) => {}
                            Err(error) => {
                                eprintln!("nav release: unresolved publication ({error:?})");
                                self.plan = None;
                                return Some(NavigatorOutcome::ReleaseUnresolved { token });
                            }
                        }
                    }
                }
                let keep_review = retain_result
                    && family == PlanFamily::Route
                    && (app.assistant_preview().is_some()
                        || app.assistant_review_status() == obc_app::navigator::ReviewStatus::Unresolved);
                if !keep_review {
                    self.publication = None;
                    self.sources = None;
                }
                self.plan = None;
                if family == PlanFamily::Detour && !keep_preview {
                    self.detour_ready = None;
                }
                self.plan_token = None;
                Some(NavigatorOutcome::Released { token })
            }
        }
    }

    /// Keep the original-route parse frame separate from step and commit frames.
    #[inline(never)]
    fn acquire_plan(
        &mut self,
        app: &mut App,
        token: OperationToken<NavigatorTag>,
        work: PlannerWork,
        routes: &dyn RouteRepository,
        trips: &dyn TripCatalog,
        map: &crate::flat_map::FlatMap,
    ) -> Option<NavigatorOutcome> {
        let failed = |error| Some(NavigatorOutcome::Failed { token, error });
        if self.plan.is_some() || self.sources.is_some() {
            return failed(NavigatorError::Workspace);
        }
        if match work {
            PlannerWork::Route(_) | PlannerWork::AssistantRoute(_) | PlannerWork::RestoreReview(_) => self.hold.route,
            PlannerWork::Detour(_) => self.hold.detour,
        } {
            return None;
        }
        let source = map.source();
        if !source.is_current() {
            return failed(NavigatorError::SourceChanged);
        }
        let original = match work {
            PlannerWork::Route(request) => {
                self.plan = Some(InflightPlan::Nav(NavPlan::start(&request, app.settings().bike_type)));
                None
            }
            PlannerWork::AssistantRoute(_) | PlannerWork::RestoreReview(_) => {
                if app.assistant_visit_target().is_some()
                    || app
                        .assistant_review_context()
                        .is_some_and(|c| matches!(c.purpose, obc_app::navigator::ReviewPurpose::Easier(_)))
                {
                    let Some(scope) = routes.store_scope() else { return failed(NavigatorError::SourceChanged) };
                    let held = routes.pin_active();
                    let fingerprint = held.as_ref().and_then(|route| routes.fingerprint(route.id()));
                    let avoidance = held.as_ref().is_some_and(|route| {
                        obc_route::RouteObjectInfo::read(route).map_or(true, |info| info.unresolved_avoidance)
                    });
                    if !app.bind_visit_sources(scope, fingerprint, avoidance) {
                        return failed(NavigatorError::SourceChanged);
                    }
                }
                let Some(context) = app.assistant_review_context() else {
                    return failed(NavigatorError::Unavailable);
                };
                if context.map.store != source.store_id().0
                    || context.map.object != source.id().0
                    || context.map.revision != source.revision().0
                    || routes.store_scope().map(|scope| scope.store) != Some(context.store)
                    || context.profile != app.settings().bike_type
                {
                    return failed(NavigatorError::SourceChanged);
                }
                let original = if let Some(expected) = context.original {
                    if routes.fingerprint(expected.object) != Some(expected) {
                        return failed(NavigatorError::SourceChanged);
                    }
                    let Some(held) = routes.pin_active() else {
                        return failed(NavigatorError::SourceChanged);
                    };
                    if held.id() != expected.object {
                        return failed(NavigatorError::SourceChanged);
                    }
                    let Ok(index) = obc_route::RouteIndex::read(&held) else {
                        return failed(NavigatorError::Unavailable);
                    };
                    if !context.accepts_original(
                        app.active_route_index().and_then(|index| app.route_ids().get(index).copied()),
                        Some(expected),
                        index.has_unresolved_avoidance(),
                    ) {
                        return failed(NavigatorError::Unavailable);
                    }
                    Some((held, Box::new(index)))
                } else {
                    if !context.accepts_original(
                        app.active_route_index().and_then(|index| app.route_ids().get(index).copied()),
                        None,
                        false,
                    ) {
                        return failed(NavigatorError::SourceChanged);
                    }
                    None
                };
                if matches!(work, PlannerWork::RestoreReview(_)) {
                    original
                } else {
                    let PlannerWork::AssistantRoute(request) = work else { unreachable!() };
                    if matches!(
                        context.purpose,
                        obc_app::navigator::ReviewPurpose::Visit
                            | obc_app::navigator::ReviewPurpose::ReturnToRoute
                            | obc_app::navigator::ReviewPurpose::Easier(_)
                    ) {
                        let Some((source, index)) = original.as_ref() else {
                            return failed(NavigatorError::Unavailable);
                        };
                        let route = obc_route::RouteReader::new(index, source);
                        if !app.assistant_easier_original(context, &route) {
                            return failed(NavigatorError::Unavailable);
                        }
                        match crate::nav_visit::VisitPlan::start(context, app.assistant_visit_target(), &route) {
                            Ok(plan) => self.plan = Some(InflightPlan::Visit(Box::new(plan))),
                            Err(error) => return failed(error),
                        }
                    } else {
                        let mut plan = NavPlan::start(&request, context.profile);
                        plan.set_attribution_map(context.map);
                        plan.set_assistant_candidate();

                        self.plan = Some(InflightPlan::Nav(plan));
                    }
                    original
                }
            }
            PlannerWork::Detour(request) => {
                let Some(source) = routes.pin_active() else { return failed(NavigatorError::Workspace) };
                if app.route_ids().get(request.route) != Some(&source.id())
                    || !routes.pin_active().is_some_and(|current| source.matches(&current))
                {
                    return failed(NavigatorError::SourceChanged);
                }
                let Ok(index) = obc_route::RouteIndex::read(&source) else {
                    return failed(NavigatorError::Workspace);
                };
                let orig = obc_route::RouteReader::new(&index, &source);
                self.plan = Some(if matches!(request.leg, obc_route::Leg::Rest { .. }) {
                    let Some(ready) = crate::nav::rest_ready(app, &request, routes, trips) else {
                        return failed(NavigatorError::SourceChanged);
                    };
                    InflightPlan::Rest(ready)
                } else {
                    let Some(plan) = DetourPlan::start(&request, app.settings().bike_type, &orig) else {
                        return failed(NavigatorError::Plan(obc_route::NavError::NoPath));
                    };
                    InflightPlan::Detour(plan)
                });
                Some((source, Box::new(index)))
            }
        };
        let Some(map_fingerprint) = source.fingerprint() else { return failed(NavigatorError::SourceChanged) };
        self.sources = Some(PlanSources { map: source, map_fingerprint, original });
        if let PlannerWork::RestoreReview(source) = work {
            return Some(self.restore_review(app, token, source, routes));
        }
        Some(NavigatorOutcome::Acquired { token })
    }

    fn restore_review(
        &mut self,
        app: &mut App,
        token: OperationToken<NavigatorTag>,
        source: obc_formats::obcr::RouteSourceKey,
        routes: &dyn RouteRepository,
    ) -> NavigatorOutcome {
        let failed = |error| NavigatorOutcome::Failed { token, error };
        let Some(context) = app.assistant_review_context() else { return failed(NavigatorError::SourceChanged) };
        if source.store != context.store.bytes() {
            return failed(NavigatorError::SourceChanged);
        }
        if app.current_review_origin().is_none_or(|origin| !context.accepts_origin(app.settings().bike_type, origin)) {
            return failed(NavigatorError::Movement);
        }
        let Some(bytes) = routes.pin_review(source) else { return failed(NavigatorError::SourceChanged) };
        let Some(fingerprint) =
            routes.fingerprint(source.object).filter(|fingerprint| fingerprint.revision == source.revision)
        else {
            return failed(NavigatorError::SourceChanged);
        };
        let Some(target) = app.assistant_visit_target() else { return failed(NavigatorError::SourceChanged) };
        let Ok(info) = obc_route::RouteObjectInfo::read(&bytes) else { return failed(NavigatorError::Unavailable) };
        if let Some(visit) = info.visit {
            if visit.target_kind != (target.metadata.source.0 >> 62) as u8
                || visit.target_id != target.metadata.source.0 & ((1 << 62) - 1)
            {
                return failed(NavigatorError::SourceChanged);
            }
        } else if target.validate_destination(&bytes, context.profile).is_err() {
            return failed(NavigatorError::Unavailable);
        }
        let preview = match obc_app::navigator::ReviewedRoute::read(fingerprint, &bytes, context) {
            Ok(preview) => preview,
            Err(error) => return failed(error),
        };
        let Ok(index) = obc_route::RouteIndex::read(&bytes) else { return failed(NavigatorError::Unavailable) };
        let points = match obc_route::RouteReader::new(&index, &bytes)
            .assistant_preview_polyline::<{ obc_app::NAV_PREVIEW_MAX }>()
        {
            Ok(points) => points,
            Err(_) => return failed(NavigatorError::Unavailable),
        };
        self.publication =
            Some(crate::RoutePublication { store: Some(context.store), id: source.object, revision: source.revision });
        let outcome = app.assistant_preview_outcome(token, preview);
        if !app.set_assistant_preview_shape(token, preview.source, &points) {
            return failed(NavigatorError::SourceChanged);
        }
        outcome
    }

    #[inline(never)]
    fn step_plan(
        &mut self,
        app: &mut App,
        token: OperationToken<NavigatorTag>,
        map: &crate::flat_map::FlatMap,
        elev: &mut dyn obc_route::ElevationSource,
    ) -> Option<NavigatorOutcome> {
        if let Some(InflightPlan::Visit(plan)) = self.plan.as_mut() {
            let Some(original) = self.sources.as_ref().and_then(|s| s.original()) else {
                return Some(NavigatorOutcome::Failed { token, error: NavigatorError::SourceChanged });
            };
            let stats = match plan.step(&map.reader(), &original, elev) {
                Ok(None) => return Some(NavigatorOutcome::Stepped { token, progress: PlannerProgress::Searching }),
                Ok(Some(stats)) => stats,
                Err(error) => return Some(NavigatorOutcome::Failed { token, error }),
            };
            if app
                .assistant_review_context()
                .is_some_and(|context| context.purpose == obc_app::navigator::ReviewPurpose::Visit)
                && !app.assistant_visit_variant(token, plan.original_anchors())
            {
                return Some(NavigatorOutcome::Failed { token, error: NavigatorError::SourceChanged });
            }
            let Some(InflightPlan::Visit(plan)) = self.plan.take() else { unreachable!() };
            self.plan = Some(InflightPlan::VisitReady(plan, stats));
            return Some(NavigatorOutcome::Stepped { token, progress: PlannerProgress::Reached });
        }
        if let Some(InflightPlan::Rest(_)) = self.plan {
            let Some(InflightPlan::Rest(ready)) = self.plan.take() else { unreachable!() };
            let preview = ready.lead_preview();
            self.detour_ready = Some(Preview { ready, sources: self.sources.take().expect("admitted sources") });
            return Some(NavigatorOutcome::DetourFinished { token, preview });
        }
        let outcome = match self.plan.as_mut() {
            Some(InflightPlan::Nav(plan)) => plan.step(&map.reader(), elev),
            Some(InflightPlan::Detour(plan)) => plan.step(&map.reader(), elev),
            _ => return Some(NavigatorOutcome::Failed { token, error: NavigatorError::Workspace }),
        };
        let stats = match outcome {
            obc_route::Step::Running => {
                return Some(NavigatorOutcome::Stepped { token, progress: PlannerProgress::Searching })
            }
            obc_route::Step::Failed(error) => {
                return Some(NavigatorOutcome::Failed { token, error: NavigatorError::Plan(error) })
            }
            obc_route::Step::Done(stats) => stats,
        };
        Some(match self.plan.take().expect("just stepped it") {
            InflightPlan::Nav(plan) => {
                self.plan = Some(InflightPlan::Ready(plan, stats));
                NavigatorOutcome::Stepped { token, progress: PlannerProgress::Reached }
            }
            InflightPlan::Detour(plan) => {
                let sources = self.sources.take().expect("admitted sources");
                let (ready, result) = plan_detour_preview(
                    app,
                    Ok(stats),
                    plan,
                    sources.original().as_ref(),
                    self.trace.as_deref_mut().unwrap_or(&mut NoTrace),
                );
                self.detour_ready = ready.map(|ready| Preview { ready, sources });
                match result {
                    Ok(preview) => NavigatorOutcome::DetourFinished { token, preview },
                    Err(error) => NavigatorOutcome::Failed { token, error: NavigatorError::Plan(error) },
                }
            }
            InflightPlan::Ready(..)
            | InflightPlan::Visit(..)
            | InflightPlan::VisitReady(..)
            | InflightPlan::Rest(..) => {
                unreachable!("only an unfinished plan steps")
            }
        })
    }

    /// Open a ride object when Recorder owes one.
    ///
    /// The only thing this executor does about the ride lifecycle outside an effect, and it decides
    /// nothing: it opens the identity Recorder named. Closing is an effect, so a session that ended
    /// leaves nothing to do here.
    ///
    /// Asking by id rather than by "the session changed" is what makes this safe on an executor
    /// whose pass and effects run in either order: a close served before its verdict is applied
    /// still shows an open session, and only the id says the object for it is already served.
    ///
    /// Read the save name only while an open is owed. A failed open stays owed; success freezes the
    /// name, so a later route swap cannot rename a ride that is already recording.
    fn sync_recorder(&mut self, app: &mut App, tracks: &mut dyn TrackRepository) {
        let Some(id) = app.recorder.object_owed(self.opened_session) else { return };
        if tracks.open(id, app.ride_name(), app.recorder.now_ms()) {
            self.opened_session = Some(id);
        }
    }

    /// Answer the plan's keyed derived needs. A level, not an operation: the key is the guard, so an
    /// answer that lands after the subject moved is simply about something else and the pass drops
    /// it.
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
            // route that is perfectly readable a line later — and a failure is an answer, so the
            // overview would settle with no shape at all.
            session.sync(app, routes);
            let src = routes.active_source();
            let pts = session.index().zip(src).and_then(|(index, s)| {
                let reader = obc_route::RouteReader::new(index, s);
                if key.assistant {
                    reader.assistant_preview_polyline::<{ obc_app::NAV_PREVIEW_MAX }>().ok()
                } else {
                    Some(reader.preview_polyline::<{ obc_app::NAV_PREVIEW_MAX }>())
                }
            });
            // Answered either way, exactly as the ride-track arm above it is: a failure is an
            // answer, and a level left unanswered is what turns an unreadable route into a headless
            // driver settling for `MAX_SETTLE_PASSES`.
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
/// [`Slot::try_put`](obc_app::device_core::Slot) hands a refused value back so its owner can offer
/// it again, and this executor's owner is the pass: it drains every slot the executor writes at
/// stage 1, unconditionally, so a full slot here means two answers were produced for one domain
/// inside a single `execute`. That cannot happen today, and a change that made it happen would
/// otherwise lose the second answer silently.
fn deliver<T: core::fmt::Debug>(slot: &mut obc_app::device_core::Slot<T>, outcome: T, domain: &str) {
    let refused = slot.try_put(outcome);
    debug_assert!(refused.is_ok(), "{domain} answered twice in one execute: {refused:?}");
}

/// Report only the repository's typed persistence result.
fn catalog_metadata_error(error: obc_app::metadata::MetadataError) -> CatalogError {
    if error == obc_app::metadata::MetadataError::RemountRequired {
        CatalogError::RemountRequired
    } else {
        CatalogError::Unreadable
    }
}
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
        RecorderEffect::Checkpoint { token } => {
            match tracks.checkpoint(app.ride_stats(), app.recorder.checkpoint_context()) {
                Ok(status) => RecorderOutcome::Checkpointed { token, status },
                Err(error) => RecorderOutcome::Failed { token, error },
            }
        }
        // Recorder drains acknowledged samples before it admits this footer-only close.
        RecorderEffect::Finalize { token } => {
            match tracks.finalize(app.ride_stats()) {
                RideClose::Committed(ride) => RecorderOutcome::Finalized { token, ride },
                // The object was never created — a start this store refused, which already warned
                // the rider. There is nothing to save, and saying so is terminal: answering `Failed`
                // would retry a close against an object that does not exist, for ever.
                RideClose::Nothing => RecorderOutcome::Discarded { token },
                RideClose::Failed => RecorderOutcome::Failed { token, error: RecorderError::Write },
            }
        }
        RecorderEffect::Discard { token } => match tracks.discard() {
            Ok(()) => RecorderOutcome::Discarded { token },
            Err(error) => RecorderOutcome::Failed { token, error },
        },
        // The staged samples, in order, for as long as the medium keeps taking them. A short write
        // is answered honestly: Recorder keeps the tail staged and offers it again next pass, so a
        // refusal costs a delay rather than a hole in the ride log. Nothing written at all is a
        // failure, which is what raises the recording warning.
        RecorderEffect::Append { token, samples } => {
            let staged = app.recorder.staged();
            let want = (samples as usize).min(staged.len());
            match tracks.append_batch(&staged[..want], app.recorder.append_context(samples)) {
                Ok(crate::repo::AppendStatus::Accepted(samples)) => RecorderOutcome::Appended { token, samples },
                Ok(crate::repo::AppendStatus::Cancelled) => RecorderOutcome::Cancelled { token },
                Ok(crate::repo::AppendStatus::NeedsCheckpoint) => RecorderOutcome::NeedsCheckpoint { token },
                Err(error) => RecorderOutcome::Failed { token, error },
            }
        }
    }
}

/// The active route's catalog name (the ride-log save filename), or `None` when nothing is active.
#[cfg(test)]
mod tests {
    use super::*;
    use obc_app::recorder::{RecorderIntent, RideClose};
    use obc_app::AppState;
    use obc_ports::{Fix, InputClock, LocationSource, RideClock, Sensors, TrackPoint};
    use obc_route::RideStats;

    /// An ordinary imported route is offered for Resume after a restart, from the same durable
    /// checkpoint the Assistant uses. The checkpoint pins the exact bytes it names, and a restored
    /// route opens no ride of its own.
    #[test]
    fn an_imported_route_is_offered_for_resume_after_a_restart() {
        use obc_app::navigator::ReviewStatus;
        use obc_pack::nav::{Edge, NavGraph, Node};
        let bbox = (0, 0, 1_000_000, 1_000_000);
        let points = [(500_000, 500_000), (500_000, 520_000), (520_000, 520_000)];
        let graph = NavGraph {
            nodes: points.iter().enumerate().map(|(id, &coord)| Node { id: id as u32, coord }).collect(),
            edges: points
                .windows(2)
                .enumerate()
                .map(|(id, p)| Edge { a: id as u32, b: id as u32 + 1, polyline: p.to_vec(), length_m: 2222, kind: 0 })
                .collect(),
        };
        let lods = [obc_pack::LodLayer {
            max_mpp: None,
            chunk_size: 2048,
            root: obc_pack::Node::Leaf { bbox, features: vec![] },
        }];
        let profiles =
            [obc_pack::NavProfile { name: "Neutral".into(), highway: [16; 32], surface: [16; 8], climb_weight: 0 }];
        let map_bytes =
            obc_pack::serialize_lods(&lods, &[], 0, bbox, &[], &graph, &profiles, &mut obc_elevation::NullElevation).0;
        let owner = crate::flat_store::HostStore::memory().unwrap();
        let map = crate::flat_map::FlatMap::from_bytes_in(&owner, &map_bytes).unwrap();
        let mut sink = crate::VecSink::default();
        let gpx = br#"<gpx><trk><trkseg><trkpt lon="0.500" lat="0.500"/><trkpt lon="0.500" lat="0.520"/><trkpt lon="0.520" lat="0.520"/></trkseg></trk></gpx>"#;
        obc_route::gpx_to_obcr(&obc_formats::io::SliceSource(gpx), "Imported", &mut sink).unwrap();
        let mut routes = crate::FlatRouteStore::new(owner.clone(), &[sink.bytes()]).unwrap();
        let imported = routes.ids()[0];
        let mut host = HostLoop::new();
        let mut session = ActiveRouteSession::new();
        let mut rides = crate::MemRideStore::new(vec![]);
        let mut tracks = RecordingTrackStore::default();
        let mut now = 0u32;
        let mut frame = |host: &mut HostLoop, app: &mut App, routes: &mut crate::FlatRouteStore, fix| {
            now += 100;
            let mut loc = OneFix(fix);
            let mut plan = host.pass(
                app,
                PassClock { ride: RideClock(now), ui: InputClock(now) },
                &[],
                Sensors::new(&mut loc),
                None,
                SUPPORT,
            );
            host.execute(
                app,
                &mut plan,
                &mut session,
                routes,
                &mut rides,
                &mut tracks,
                &mut (),
                &map,
                &mut obc_route::NullElevation,
                &mut (),
            );
            app.advance_animations(InputClock(now));
        };
        // The map-first app is riding: only a route being followed owes a checkpoint.
        let mut app = App::new(AppState::new(500_000, 500_000, 10.0));
        feed_routes(&mut app, &routes, &mut NoTrace);
        app.activate_route(0);
        for _ in 0..8 {
            frame(&mut host, &mut app, &mut routes, None);
            if routes.read_checkpoint().unwrap().is_some() {
                break;
            }
        }
        let saved = routes.read_checkpoint().unwrap().expect("selecting a route is durable on its own");
        assert_eq!(Some(saved.route), routes.fingerprint(imported));
        assert_eq!((saved.original, saved.phase), (None, obc_formats::assistant::JourneyPhase::Following));
        assert_eq!((saved.progress_m, saved.lower_m), (0, 0), "an ordinary route carries no phase window");
        assert!(!app.recording(), "selecting a route is not starting a ride");
        assert!(routes.replace(imported, sink.bytes()).is_err(), "the checkpoint pins the bytes it names");

        let mut reboot = App::new_idle(AppState::new(500_000, 500_000, 10.0));
        feed_routes(&mut reboot, &routes, &mut NoTrace);
        frame(&mut host, &mut reboot, &mut routes, None);
        assert_eq!(reboot.assistant_review_status(), ReviewStatus::ResumeAvailable);
        assert!(reboot.active_route_index().is_none(), "a restart never restores guidance by itself");
        frame(&mut host, &mut reboot, &mut routes, Some(Fix::at(510_000, 500_000)));
        assert_eq!(reboot.top_screen().name(), "Journey");
        reboot.apply_gesture(obc_app::Gesture::Press);
        for _ in 0..12 {
            frame(&mut host, &mut reboot, &mut routes, None);
            if reboot.assistant_review_status() == ReviewStatus::Accepted {
                break;
            }
        }
        assert_eq!(reboot.assistant_review_status(), ReviewStatus::Accepted);
        assert_eq!(reboot.route_ids()[reboot.active_route_index().unwrap()], imported);
        assert!(reboot.assistant_checkpoint().unwrap().progress_m > 50, "guidance returns to the rider's position");
        assert!(!reboot.recording(), "navigation recovery is not recording recovery");
    }

    #[test]
    fn assistant_plans_reviews_and_accepts_exact_immutable_bytes_through_the_host_executor() {
        immutable_assistant_replay(false);
    }
    #[test]
    fn visit_accepts_complete_bytes_through_the_host_executor() {
        immutable_assistant_replay(true);
    }
    fn immutable_assistant_replay(visit: bool) {
        use obc_app::navigator::{ReviewContext, ReviewOrigin, ReviewPurpose, ReviewStatus, REVIEW_FACTS_POLICY};
        use obc_pack::nav::{Edge, NavGraph, Node};
        let bbox = (0, 0, 1_000_000, 1_000_000);
        let points = [(500_000, 500_000), (502_000, 500_000), (504_000, 500_000)];
        let graph = NavGraph {
            nodes: points.iter().enumerate().map(|(id, &coord)| Node { id: id as u32, coord }).collect(),
            edges: points
                .windows(2)
                .enumerate()
                .map(|(id, p)| Edge { a: id as u32, b: id as u32 + 1, polyline: p.to_vec(), length_m: 222, kind: 0 })
                .collect(),
        };
        let lods = [obc_pack::LodLayer {
            max_mpp: None,
            chunk_size: 2048,
            root: obc_pack::Node::Leaf { bbox, features: vec![] },
        }];
        let profiles =
            [obc_pack::NavProfile { name: "Neutral".into(), highway: [16; 32], surface: [16; 8], climb_weight: 0 }];
        let map_bytes =
            obc_pack::serialize_lods(&lods, &[], 0, bbox, &[], &graph, &profiles, &mut obc_elevation::NullElevation).0;
        let owner = crate::flat_store::HostStore::memory().unwrap();
        let map = crate::flat_map::FlatMap::from_bytes_in(&owner, &map_bytes).unwrap();
        let mut sink = crate::VecSink::default();
        let gpx = b"<gpx><trk><trkseg><trkpt lon=\"0.500\" lat=\"0.500\"/><trkpt lon=\"0.504\" lat=\"0.500\"/></trkseg></trk></gpx>";
        obc_route::gpx_to_obcr(&obc_formats::io::SliceSource(gpx), "Original", &mut sink).unwrap();
        let mut routes = crate::FlatRouteStore::new(owner.clone(), &[sink.bytes()]).unwrap();
        let original = routes.ids()[0];
        let mut app = App::new_idle(AppState::new(500_000, 500_000, 10.0));
        feed_routes(&mut app, &routes, &mut NoTrace);
        app.activate_route(0);
        routes.sync_active(Some(0));
        let mut host = HostLoop::new();
        let mut rides = crate::MemRideStore::new(vec![]);
        let mut tracks = RecordingTrackStore::default();
        let mut session = ActiveRouteSession::new();
        let concurrent_discard = std::cell::RefCell::new(None::<crate::FlatRideRecorder>);
        let clear_scope_changed = std::cell::Cell::new(false);
        let clear_attempts = std::cell::Cell::new(0);
        let mut now = 0;
        let mut frame = |host: &mut HostLoop, app: &mut App, routes: &mut crate::FlatRouteStore| {
            now += 100;
            let mut loc = OneFix(None);
            let mut plan = host.pass(
                app,
                PassClock { ride: RideClock(now), ui: InputClock(now) },
                &[],
                Sensors::new(&mut loc),
                None,
                SUPPORT,
            );
            if let Some(effect) = plan.effects.metadata.take() {
                if app.assistant_checkpoint_payload(effect.token()).is_some_and(|change| change.next.is_none()) {
                    clear_attempts.set(clear_attempts.get() + 1);
                    if let Some(mut recorder) = concurrent_discard.borrow_mut().take() {
                        recorder.discard().unwrap();
                        assert_ne!(effect.scope(), routes.store_scope(), "recording removal changes the issued scope");
                        clear_scope_changed.set(true);
                        if visit {
                            app.activate_route(routes.ids().iter().position(|&id| id == original).unwrap());
                        }
                    } else if clear_scope_changed.get() {
                        assert_eq!(effect.scope(), routes.store_scope(), "retry waits for the refreshed scope");
                    }
                }
                plan.effects.metadata.try_put(effect).unwrap();
            }
            host.execute(
                app,
                &mut plan,
                &mut session,
                routes,
                &mut rides,
                &mut tracks,
                &mut (),
                &map,
                &mut obc_route::NullElevation,
                &mut (),
            );
        };
        for _ in 0..4 {
            frame(&mut host, &mut app, &mut routes);
        }
        let map_source = map.source();
        let context = ReviewContext {
            purpose: if visit { ReviewPurpose::Visit } else { ReviewPurpose::Destination },
            map: obc_formats::obcr::RouteSourceKey {
                store: map_source.store_id().0,
                object: map_source.id().0,
                revision: map_source.revision().0,
            },
            store: routes.store_scope().unwrap().store,
            original: routes.fingerprint(original),
            origin: points[0],
            progress_m: 0,
            occurrence: 0,
            required_anchors_m: [0; 3],
            profile: app.settings().bike_type,
            facts_policy: REVIEW_FACTS_POLICY,
            unresolved_avoidance: false,
        };
        let active_request = obc_app::activity::NavRequest::new(context.origin, points[2], "Target");
        let mut tokens = obc_app::device_core::TokenSource::<obc_app::device_core::NavigatorTag>::new();
        let mut refused = App::new_idle(AppState::new(500_000, 500_000, 10.0));
        feed_routes(&mut refused, &routes, &mut NoTrace);
        refused.activate_route(0);
        refused.plan_assistant(active_request, ReviewContext { original: None, ..context });
        assert!(matches!(
            HostLoop::new().acquire_plan(
                &mut refused,
                tokens.issue(),
                PlannerWork::AssistantRoute(active_request),
                &routes,
                &(),
                &map
            ),
            Some(NavigatorOutcome::Failed { .. })
        ));
        if visit {
            use obc_formats::obcm::{PoiApproach, PoiMetadata, SourceId};
            assert!(app.plan_visit(
                obc_route::visit::VisitTarget {
                    map: context.map,
                    display: points[1],
                    metadata: PoiMetadata {
                        source: SourceId::osm(1, 99),
                        approach: Some(PoiApproach {
                            source: SourceId::osm(1, 100),
                            lon: points[1].0,
                            lat: points[1].1,
                            profile_mask: 1,
                        }),
                    },
                },
                context
            ));
        } else {
            app.plan_assistant(obc_app::NavRequest::new(points[0], points[2], "Candidate"), context);
        }
        for _ in 0..200 {
            frame(&mut host, &mut app, &mut routes);
            if app.assistant_review_status() == ReviewStatus::Preview && !host.is_planning() {
                break;
            }
        }
        assert_eq!(app.assistant_review_status(), ReviewStatus::Preview);
        let preview = app.assistant_preview().unwrap();
        assert_eq!(app.route_ids()[app.active_route_index().unwrap()], original);
        assert!(routes.read_checkpoint().unwrap().is_none());
        assert!(app.route_unaccepted(app.route_ids().iter().position(|id| *id == preview.source.object).unwrap()));
        let mut orphan_boot = App::new_idle(AppState::new(500_000, 500_000, 10.0));
        feed_routes(&mut orphan_boot, &routes, &mut NoTrace);
        let orphan = orphan_boot.route_ids().iter().position(|id| *id == preview.source.object).unwrap();
        assert!(orphan_boot.route_unaccepted(orphan));
        orphan_boot.activate_route(orphan);
        assert!(orphan_boot.active_route_index().is_none());
        let bytes = owner
            .open(
                obc_storage::flat::ObjectId(preview.source.object),
                obc_storage::flat::Revision(preview.source.revision),
            )
            .unwrap();
        let info = obc_route::RouteObjectInfo::read(&bytes).unwrap();
        assert_eq!(info.visit.is_some(), visit);
        assert_eq!(
            (preview.distance_m, preview.ascent_m, preview.descent_m),
            (info.distance_m, info.ascent_m, info.descent_m)
        );
        drop(bytes);
        // Consume the arena-release ACK before accepting the immutable candidate.
        frame(&mut host, &mut app, &mut routes);
        app.accept_assistant(ReviewOrigin {
            fix: context.origin,
            progress_m: 0,
            occurrence: 0,
            lateral_m: 0,
            trustworthy: true,
        });
        assert_eq!(app.route_ids()[app.active_route_index().unwrap()], original);
        for _ in 0..12 {
            frame(&mut host, &mut app, &mut routes);
            if app.assistant_review_status() == ReviewStatus::Accepted {
                break;
            }
        }
        assert_eq!(app.assistant_review_status(), ReviewStatus::Accepted);
        assert_eq!(app.route_ids()[app.active_route_index().unwrap()], preview.source.object);
        assert_eq!(routes.read_checkpoint().unwrap().unwrap().route, preview.source);
        assert!(!host.owns_navigation());
        assert!(!app.route_unaccepted(app.active_route_index().unwrap()));
        assert_ne!(
            routes.internal_routes() & (1 << app.active_route_index().unwrap()),
            0,
            "acceptance does not turn generated bytes into a saved route"
        );
        assert_eq!(routes.internal_routes() & (1 << routes.ids().iter().position(|id| *id == original).unwrap()), 0);
        let reopened = crate::FlatRouteStore::new(owner.clone(), &[]).unwrap();
        assert_eq!(reopened.internal_routes(), routes.internal_routes(), "the existing header flag survives reload");
        if visit {
            assert!(
                routes.delete_by_id(original).is_err(),
                "accepted original remains protected without a reader hold"
            );
        } else {
            assert!(routes.read_checkpoint().unwrap().unwrap().original.is_none());
        }
        let mut reboot = App::new_idle(AppState::new(500_000, 500_000, 10.0));
        reboot.offer_assistant_checkpoint(routes.store_scope().unwrap().store, routes.read_checkpoint().unwrap());
        assert_eq!(reboot.assistant_review_status(), ReviewStatus::ResumeAvailable);
        assert!(reboot.active_route_index().is_none());
        feed_routes(&mut reboot, &routes, &mut NoTrace);
        reboot.tick(
            RideClock(0),
            Sensors::new(&mut OneFix(Some(Fix::at(points[0].1, (points[0].0 + points[1].0) / 2)))),
            None,
        );
        reboot.advance_animations(InputClock(0));
        assert_eq!(reboot.top_screen().name(), "Journey");
        assert!(reboot.requested_assistant_resume().is_none());
        reboot.apply_gesture(obc_app::Gesture::Press);
        assert_eq!(reboot.requested_assistant_resume(), Some(preview.source));
        for _ in 0..12 {
            frame(&mut host, &mut reboot, &mut routes);
            if reboot.assistant_review_status() == ReviewStatus::Accepted {
                break;
            }
        }
        assert_eq!(reboot.assistant_review_status(), ReviewStatus::Accepted);
        assert_eq!(reboot.route_ids()[reboot.active_route_index().unwrap()], preview.source.object);
        assert!(!reboot.recording(), "recovered guidance does not open a ride of its own");
        assert!(!reboot.visit_arrival_pending());
        assert!(reboot.assistant_checkpoint().unwrap().progress_m > 50);
        assert_eq!(routes.read_checkpoint().unwrap(), reboot.assistant_checkpoint());
        app = reboot;
        let mut concurrent = crate::FlatRideRecorder::new(owner.clone()).unwrap();
        assert!(concurrent.open(17, Some("Concurrent recording"), 0));
        concurrent_discard.replace(Some(concurrent));
        for _ in 0..4 {
            frame(&mut host, &mut app, &mut routes);
        }
        app.activate_route(usize::MAX);
        for _ in 0..12 {
            frame(&mut host, &mut app, &mut routes);
            if app.assistant_checkpoint().is_none() {
                break;
            }
        }
        assert!(clear_scope_changed.get());
        assert_eq!(clear_attempts.get(), 2);
        assert!(app.assistant_checkpoint().is_none(), "recording removal must not lose the queued navigation stop");
        assert_eq!(
            app.active_route_index().map(|index| app.route_ids()[index]),
            visit.then_some(original),
            "the latest route selection survives the scope refusal"
        );
        let accepted_index = app.route_ids().iter().position(|id| *id == preview.source.object).unwrap();
        app.activate_route(accepted_index);
        assert_eq!(app.active_route_index(), Some(accepted_index));
        let mut avoided = sink.bytes().to_vec();
        avoided[5] |= obc_formats::obcr::FLAG_UNRESOLVED_AVOIDANCE;
        routes.replace(original, &avoided).unwrap();
        feed_routes(&mut app, &routes, &mut NoTrace);
        let original_index = app.route_ids().iter().position(|id| *id == original).unwrap();
        app.activate_route(original_index);
        routes.sync_active(Some(original_index));
        app.plan_assistant(active_request, ReviewContext { original: routes.fingerprint(original), ..context });
        assert!(
            matches!(
                HostLoop::new().acquire_plan(
                    &mut app,
                    tokens.issue(),
                    PlannerWork::AssistantRoute(active_request),
                    &routes,
                    &(),
                    &map
                ),
                Some(NavigatorOutcome::Failed { .. })
            ),
            "persisted avoidance overrides the caller's false flag"
        );
    }
    #[test]
    fn assistant_prior_head_remount_rebinds_only_unchanged_frozen_sources() {
        use crate::flat_store::HostStore;
        use obc_app::navigator::{ReviewContext, ReviewOrigin, ReviewPurpose, ReviewStatus, REVIEW_FACTS_POLICY};
        use obc_storage::flat::{DisplayName, ObjectKind};
        use std::sync::Arc;
        for (with_original, changed_map, changed_original) in [
            (true, false, false),
            (false, false, false),
            (true, true, false),
            (false, true, false),
            (true, false, true),
        ] {
            let card = HostStore::memory().unwrap();
            let map_bytes = super::planner_tests::map_bytes();
            let map = crate::flat_map::FlatMap::from_bytes_in(&card, &map_bytes).unwrap();
            let mut routes = crate::FlatRouteStore::new(card.clone(), &[]).unwrap();
            if with_original {
                let mut sink = crate::VecSink::default();
                let gpx = br#"<gpx><trk><trkseg><trkpt lon="0.5" lat="0.5"/><trkpt lon="0.52" lat="0.5"/></trkseg></trk></gpx>"#;
                obc_route::gpx_to_obcr(&obc_formats::io::SliceSource(gpx), "Original", &mut sink).unwrap();
                routes.import(sink.bytes()).unwrap();
            }
            let mut app = App::new_idle(AppState::new(500_000, 500_000, 10.0));
            feed_routes(&mut app, &routes, &mut NoTrace);
            if with_original {
                app.activate_route(0);
                routes.sync_active(Some(0));
            }
            let frozen = map.source();
            let context = ReviewContext {
                purpose: ReviewPurpose::Destination,
                map: obc_formats::obcr::RouteSourceKey {
                    store: frozen.store_id().0,
                    object: frozen.id().0,
                    revision: frozen.revision().0,
                },
                store: routes.store_scope().unwrap().store,
                original: if with_original { routes.fingerprint(routes.ids()[0]) } else { None },
                origin: (500_000, 500_000),
                progress_m: 0,
                occurrence: 0,
                required_anchors_m: [0; 3],
                profile: obc_route::BikeType::Road,
                facts_policy: REVIEW_FACTS_POLICY,
                unresolved_avoidance: false,
            };
            let origin =
                ReviewOrigin { fix: context.origin, progress_m: 0, occurrence: 0, lateral_m: 0, trustworthy: true };
            let mut host = HostLoop::new();
            let mut rides = crate::MemRideStore::new(vec![]);
            let mut tracks = RecordingTrackStore::default();
            let mut session = ActiveRouteSession::new();
            let mut now = 0;
            let mut frame = |host: &mut HostLoop,
                             app: &mut App,
                             routes: &mut crate::FlatRouteStore,
                             map: &crate::flat_map::FlatMap,
                             fail_checkpoint: bool| {
                now += 100;
                let mut fix = OneFix(None);
                let mut plan = host.pass(
                    app,
                    PassClock { ride: RideClock(now), ui: InputClock(now) },
                    &[],
                    Sensors::new(&mut fix),
                    None,
                    SUPPORT,
                );
                let mut failed = false;
                if fail_checkpoint {
                    if let Some(MetadataEffect::WriteCheckpoint { token, .. }) = plan.effects.metadata.take() {
                        assert!(app.assistant_checkpoint_submission(token));
                        assert!(host
                            .inbox
                            .outcomes
                            .metadata
                            .try_put(MetadataOutcome::Failed {
                                token,
                                error: obc_app::metadata::MetadataError::RemountRequired
                            })
                            .is_ok());
                        failed = true;
                    }
                }
                host.execute(
                    app,
                    &mut plan,
                    &mut session,
                    routes,
                    &mut rides,
                    &mut tracks,
                    &mut (),
                    map,
                    &mut obc_route::NullElevation,
                    &mut (),
                );
                failed
            };
            for _ in 0..4 {
                frame(&mut host, &mut app, &mut routes, &map, false);
            }
            app.plan_assistant(obc_app::NavRequest::new(context.origin, (520_000, 500_000), "Target"), context);
            for _ in 0..128 {
                frame(&mut host, &mut app, &mut routes, &map, false);
                if app.assistant_review_status() == ReviewStatus::Preview {
                    break;
                }
            }
            assert_eq!(app.assistant_review_status(), ReviewStatus::Preview);
            frame(&mut host, &mut app, &mut routes, &map, false);
            let preview = app.assistant_preview().unwrap();
            app.accept_assistant(origin);
            assert!(frame(&mut host, &mut app, &mut routes, &map, true));
            // Reopen the same exact media in a new FlatStore and Owner Arc. Old leases stay alive.
            let reopened = card.remount_memory_snapshot();
            assert!(!frozen.is_current(), "the old mount is fenced");
            assert!(!Arc::ptr_eq(&card.0, &reopened.0));
            assert_eq!(card.store_id().unwrap(), reopened.store_id().unwrap());
            if changed_map {
                reopened
                    .import(
                        ObjectKind::MapShard,
                        Some((frozen.id(), frozen.revision())),
                        &mut &map_bytes[..],
                        map_bytes.len() as u64,
                        DisplayName::default(),
                    )
                    .unwrap();
            }
            if changed_original {
                let original = context.original.unwrap();
                let id = obc_storage::flat::ObjectId(original.object);
                let revision = obc_storage::flat::Revision(original.revision);
                let source = reopened.open(id, revision).unwrap();
                let mut bytes = vec![0; original.length as usize];
                obc_formats::io::ByteSource::read_at(&source, 0, &mut bytes).unwrap();
                reopened
                    .import(
                        ObjectKind::Route,
                        Some((id, revision)),
                        &mut &bytes[..],
                        bytes.len() as u64,
                        DisplayName::default(),
                    )
                    .unwrap();
            }
            let new_map = crate::flat_map::FlatMap::open_only_in(&reopened).unwrap();
            let mut new_routes = crate::FlatRouteStore::new(reopened, &[]).unwrap();
            new_routes.sync_active(app.active_route_index());
            frame(&mut host, &mut app, &mut new_routes, &new_map, false);
            if changed_map || changed_original {
                assert_eq!(app.assistant_review_status(), ReviewStatus::Failed(NavigatorError::SourceChanged));
                assert!(host.sources.as_ref().unwrap().map.same_revision(&frozen), "failed rebind is atomic");
            } else {
                assert_eq!(app.assistant_review_status(), ReviewStatus::Preview);
                assert!(host.sources.as_ref().unwrap().current(&new_map, &new_routes));
            }
            app.accept_assistant(origin);
            for _ in 0..12 {
                frame(&mut host, &mut app, &mut new_routes, &new_map, false);
            }
            if changed_map || changed_original {
                assert!(new_routes.read_checkpoint().unwrap().is_none());
                assert_ne!(app.assistant_review_status(), ReviewStatus::Accepted);
            } else {
                assert_eq!(app.assistant_review_status(), ReviewStatus::Accepted);
                assert_eq!(new_routes.read_checkpoint().unwrap().unwrap().route, preview.source);
            }
        }
    }

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
                SUPPORT,
            );
            let effect = plan.effects.bond.take().unwrap();
            plan.effects.bond.try_put(effect).unwrap();
            let mut platform = Platform { result, calls: 0 };
            let mut routes = crate::FlatRouteStore::from_bytes(&[]).unwrap();
            let mut rides = crate::MemRideStore::new(vec![]);
            let mut tracks = RecordingTrackStore::default();
            for _ in 0..2 {
                host.serve_effects(&mut app, &mut plan, &mut routes, &mut rides, &mut (), &mut tracks, &mut platform);
            }
            assert_eq!(platform.calls, 1);
            assert_eq!(host.inbox.outcomes.bond.take(), Some(BondOutcome::from_result(effect.token(), result)));
        }
    }

    #[test]
    fn removal_preserves_token_and_never_probes_another_family() {
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
                panic!("another family must not be reached");
            }
        }

        let token = obc_app::device_core::TokenSource::<CatalogTag>::new().issue();
        const ROUTE: &[u8] = include_bytes!("../../../fixtures/sources/sim-grimsel/routes/grimsel-climb.obcr");
        let mut routes = crate::FlatRouteStore::from_bytes(&[ROUTE]).unwrap();
        let object = routes.ids()[0];
        assert_eq!(
            remove_object(
                token,
                object,
                obc_app::catalog_state::CatalogObjectKind::Ride,
                &mut routes,
                &mut FailedRide,
                &mut UnreachedTrip
            ),
            CatalogOutcome::Failed { token, error: CatalogError::RemoveFailed },
        );
        assert_eq!(
            remove_object(
                token,
                object,
                obc_app::catalog_state::CatalogObjectKind::Ride,
                &mut routes,
                &mut crate::MemRideStore::new(vec![]),
                &mut ()
            ),
            CatalogOutcome::ObjectRemoved { token, object, existed: false },
        );
        assert_eq!(routes.ids(), &[object], "same-numbered route is untouched by a ride request");
    }

    #[test]
    fn trip_refresh_failure_or_another_card_cannot_grant_catalog_scope() {
        use crate::flat_store::HostStore;
        use obc_storage::flat::{DisplayName, ObjectKind};
        let owner = HostStore::memory().unwrap();
        let mut routes = crate::FlatRouteStore::new(owner.clone(), &[]).unwrap();
        let mut trips = crate::FlatTripStore::new(owner.clone()).unwrap();
        let mut sink = crate::VecSink::default();
        obc_route::write_trip(1, "Kept", 0, &[], &mut sink).unwrap();
        let trip = trips.import(sink.bytes()).unwrap();
        let mut app = App::new_idle(obc_app::AppState::new(0, 0, 1.0));
        let mut host = HostLoop::new();
        let mut rides = crate::MemRideStore::new(vec![]);
        let mut tokens = obc_app::device_core::TokenSource::<CatalogTag>::new();
        let token = tokens.issue();
        assert!(matches!(
            host.serve_catalog(&mut app, CatalogEffect::ReadCatalog { token }, &mut routes, &mut rides, &mut trips),
            CatalogOutcome::CatalogRead { scope: Some(_), .. }
        ));
        assert_eq!(app.trips()[0].id, trip);
        owner.import(ObjectKind::Trip, None, &mut &b"bad"[..], 3, DisplayName::default()).unwrap();
        let token = tokens.issue();
        assert_eq!(
            host.serve_catalog(&mut app, CatalogEffect::ReadCatalog { token }, &mut routes, &mut rides, &mut trips),
            CatalogOutcome::Failed { token, error: CatalogError::Unreadable }
        );
        assert_eq!(app.trips()[0].id, trip, "failed refresh retains the previous projection");
        let mut other = crate::FlatTripStore::new(HostStore::memory().unwrap()).unwrap();
        let token = tokens.issue();
        assert_eq!(
            host.serve_catalog(&mut app, CatalogEffect::ReadCatalog { token }, &mut routes, &mut rides, &mut other),
            CatalogOutcome::Failed { token, error: CatalogError::Stale }
        );
    }

    /// A host that can do everything — none of it reached by the recording path under test.
    pub(super) const SUPPORT: PlatformSupport = PlatformSupport {
        detour: true,
        settings_persistence: true,
        dfu: true,

        bonding: true,
        storage_space_report: true,
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
    pub(super) struct RecordingTrackStore {
        open: bool,
        points: Vec<TrackPoint>,
        footer: Option<RideStats>,
        failed_opens: usize,
        open_attempts: usize,
        delta_limit: Option<usize>,
        delta_points: usize,
        failed_checkpoints: usize,
        unsupported_checkpoints: bool,
        failed_finalizes: usize,
    }

    impl TrackRepository for RecordingTrackStore {
        fn open(&mut self, _session: u32, _name: Option<&str>, _now_ms: u32) -> bool {
            self.open_attempts += 1;
            self.open = self.open_attempts > self.failed_opens;
            self.open
        }

        fn finalize(&mut self, stats: RideStats) -> RideClose {
            if !self.open {
                return RideClose::Nothing;
            }
            if self.failed_finalizes > 0 {
                self.failed_finalizes -= 1;
                return RideClose::Failed;
            }
            self.open = false;
            self.footer = Some(stats);
            RideClose::Committed(7)
        }

        fn discard(&mut self) -> Result<(), obc_app::recorder::RecorderError> {
            self.open = false;
            Ok(())
        }

        fn checkpoint(
            &mut self,
            _stats: RideStats,
            _continuation: Option<obc_app::RideContinuation>,
        ) -> Result<obc_app::recorder::CheckpointStatus, RecorderError> {
            assert!(self.open, "no checkpoint against an absent recorder");
            if self.failed_checkpoints > 0 {
                self.failed_checkpoints -= 1;
                return Err(RecorderError::Write);
            }
            self.delta_points = 0;
            Ok(if self.unsupported_checkpoints {
                obc_app::recorder::CheckpointStatus::Unsupported
            } else {
                obc_app::recorder::CheckpointStatus::Durable
            })
        }

        fn append(&mut self, point: TrackPoint) -> bool {
            assert!(self.open, "no append against an absent recorder");
            if self.delta_limit.is_some_and(|limit| self.delta_points == limit) {
                return false;
            }
            self.delta_points += 1;
            self.points.push(point);
            true
        }
    }

    /// A ride recording `fixes` samples, none of which any append has taken: every append this
    /// executor is offered is refused, which is what leaves a tail staged for the close to find.
    /// Fixes are one second and about 11 m apart, so each one is logged and none reads as a
    /// teleport.
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
                SUPPORT,
            );
            // Refuse every append, so the samples stay staged. A refusal is a delay, not a loss.
            if let Some(effect) = plan.effects.recorder.take() {
                assert!(matches!(effect, RecorderEffect::Append { .. }), "only appends here: {effect:?}");
                let refused = RecorderOutcome::Cancelled { token: effect.token() };
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
            let times = if !save && start == 11_000 {
                vec![
                    start,
                    start + 1_000,
                    start + 1_000 + obc_app::recorder::CHECKPOINT_RETRY_MS,
                    start + 32_001,
                    start + 32_002,
                ]
            } else {
                vec![start, start + 1_000, start + 2_000, start + 3_000]
            };
            for (step, now) in times.into_iter().enumerate() {
                let mut loc = OneFix(None);
                let mut plan = host.pass(
                    &mut app,
                    PassClock { ride: RideClock(now), ui: InputClock(now) },
                    &[],
                    Sensors::new(&mut loc),
                    None,
                    SUPPORT,
                );
                let first_token = (step == 0).then(|| {
                    let effect = plan.effects.recorder.take().unwrap();
                    assert!(match (save, start) {
                        (true, _) => matches!(effect, RecorderEffect::Append { .. }),
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
                    &map,
                    &mut obc_route::NullElevation,
                    &mut (),
                );
                if let Some(token) = first_token {
                    assert_eq!(host.opened_session, None, "a failed open stays owed");
                    assert!(store.points.is_empty());
                    assert_eq!(app.recorder.staged(), expected);
                    let outcome = host.inbox.outcomes.recorder.take().unwrap();
                    assert_eq!(outcome, RecorderOutcome::Failed { token, error: RecorderError::Write });
                    host.inbox.outcomes.recorder.try_put(outcome).unwrap();
                }
            }
            if save {
                assert_eq!(app.recorder.staged(), expected, "Save cannot drop samples whose open is still refused");
                assert_eq!(store.open_attempts, 4, "the same session still owes its object");
                assert!(app.recorder.session().is_some());
                assert!(store.points.is_empty());
                assert!(store.footer.is_none(), "no object was committed");
            } else {
                assert!(app.recorder.staged().is_empty());
                assert_eq!(store.open_attempts, 2, "one retry, then the session stays acknowledged");
                assert_eq!(host.opened_session, app.recorder.session());
                assert_eq!(store.points, expected, "the staged samples reach the object once, in order");
            }
        }
    }

    #[test]
    fn save_drains_a_short_append_and_retries_checkpoint_and_footer_without_duplicates() {
        for unsupported in [false, true] {
            let (mut app, mut host) = ride_with_a_staged_tail(4);
            let expected = app.recorder.staged().to_vec();
            let mut store = RecordingTrackStore {
                delta_limit: Some(2),
                failed_checkpoints: usize::from(!unsupported),
                unsupported_checkpoints: unsupported,
                failed_finalizes: 1,
                ..Default::default()
            };
            store.open(1, Some("ride"), 0);
            app.recorder.request(RecorderIntent::Save);
            let mut operations = Vec::new();
            let times = [9_000, 9_001, 9_002, 39_002, 39_003, 39_004, 39_005, 39_006];
            for now in times {
                let mut loc = OneFix(None);
                let mut plan = host.pass(
                    &mut app,
                    PassClock { ride: RideClock(now), ui: InputClock(now) },
                    &[],
                    Sensors::new(&mut loc),
                    None,
                    SUPPORT,
                );
                let Some(effect) = plan.effects.recorder.take() else { continue };
                operations.push(match effect {
                    RecorderEffect::Append { .. } => "append",
                    RecorderEffect::Checkpoint { .. } => "checkpoint",
                    RecorderEffect::Finalize { .. } => {
                        assert!(app.recorder.staged().is_empty());
                        assert_eq!(store.points, expected);
                        "finalize"
                    }
                    RecorderEffect::Discard { .. } => panic!("Save cannot discard"),
                });
                let outcome = serve_recorder(&app, effect, &mut store, true);
                if unsupported && matches!(effect, RecorderEffect::Checkpoint { .. }) {
                    assert_eq!(
                        outcome,
                        RecorderOutcome::Checkpointed {
                            token: effect.token(),
                            status: obc_app::recorder::CheckpointStatus::Unsupported,
                        }
                    );
                }
                host.inbox.outcomes.recorder.try_put(outcome).unwrap();
            }
            let expected_operations = if unsupported {
                vec!["append", "checkpoint", "append", "finalize", "finalize"]
            } else {
                vec!["append", "checkpoint", "checkpoint", "append", "finalize", "finalize"]
            };
            assert_eq!(operations, expected_operations);
            assert_eq!(store.points, expected, "each accepted sample occurs once");
            assert!(!app.recorder.recording());
            let footer = store.footer.expect("one successful close writes the footer");
            let last = store.points.last().unwrap();
            assert_eq!(last.t_ms, 6_000);
            assert!(footer.distance_m > 0);
            assert_eq!(footer.moving_time_s, (last.t_ms - store.points[0].t_ms) / 1_000);
        }
    }
}

#[cfg(test)]
mod planner_tests;

#[cfg(test)]
mod find_tests;
