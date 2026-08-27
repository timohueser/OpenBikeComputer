//! The shared DeviceCore behaviour corpus (#1434 DC1, #1440 DC7).
//!
//! One scenario table, one set of fixtures and one [`CorpusState`], shared by every runner in
//! `device_core_conformance`. Keeping the corpus here is what makes "the same scenario, a different
//! runner" a fact about the code rather than about hand-kept copies of a table.
//!
//! Nothing here decides policy: [`CorpusState::apply_input`] applies real `App` operations and real
//! gestures, and the runner in `obc_host_core::trace` only controls *when* completed outcomes are
//! delivered.

// A shared test corpus is compiled into every binary that includes it, and each uses a subset.
#![allow(dead_code)]

use obc_app::device_core::ModeState;
use obc_app::device_core::{
    DataIdentity, DerivedInputs, ExternalFacts, NavigatorTag, OperationToken, OutcomeSlots, Revision, RouteUpload,
    SettingsTag, StoreIdentity, StoreRevision, TripUpload, UpdateResult, WeatherData,
};
use obc_app::dfu::{clamp, DfuFailure, DfuInstallError, DfuScanError, DfuScanReport};
use obc_app::navigator::NavigatorOutcome;
use obc_app::screen::Screen;
use obc_app::{
    App, AppState, DetourPreview, Gesture, Mode, RideRetentionRecord, RideSummary, RouteSummary, TrackAction,
    TripInput, WarningFlags,
};
use obc_formats::io::{ByteSink, SliceSource};
use obc_host_core::trace::{
    FeederCall, FeederKind, NormalizationSeed, ObjectKey, ObjectKind, RevisionKey, ScenarioStep, TimeKey, Trace,
    TraceInput, TraceRecorder, TraceScenario, TraceSink,
};
use obc_host_core::{RideRepository, RouteRepository, TripCatalog};
use obc_map_scene::BBox;
use obc_ports::{Fix, InputClock, LocationSource, RideClock, Sensors};
use obc_route::{gpx_to_obcr, NavError, RouteIndex, RouteReader};

/// Every behavior row locked by DC1.  Keeping the inventory typed makes adding a scenario without
/// the corresponding acceptance row (or silently dropping a row during later refactors) fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Requirement {
    CatalogStoreChange,
    CatalogRefresh,
    CatalogRouteDelete,
    CatalogRideDelete,
    CatalogTripCascade,
    /// A completed removal is followed by the re-read the **domain** orders (#1541). The executor
    /// re-feeds nothing, so the rider's menu loses the row only when that read lands.
    CatalogDeleteOrdersRefresh,
    /// A read the store could not answer is re-offered until it lands (#1541) — the retry the board
    /// used to keep privately, now every host's.
    CatalogRefreshRetry,
    CatalogUploadOrder,
    CatalogIdentityRemap,
    NavigationPlan,
    NavigationCancel,
    NavigationLateResult,
    NavigationReplacement,
    NavigationDetourPlan,
    NavigationDetourCancel,
    NavigationDetourCommit,
    NavigationNoPath,
    RecorderStart,
    RecorderSave,
    RecorderDiscard,
    RecorderFinalizeFailure,
    RecorderSessionReplacement,
    SettingsDirtyRevision,
    SettingsSuccess,
    SettingsStaleResult,
    SettingsFailure,
    SettingsRetry,
    RetentionRouteUseStamp,
    RetentionRideSyncStamp,
    RetentionExpiryDelete,
    RetentionRetry,
    RetentionTrustedClockGate,
    /// An expiry candidate is retired by the catalog's removal verdict (#1548), in the pass that
    /// answer lands. The re-read the removal ordered can be slower than the delete backstop, and a
    /// second removal for an object the store has already reported gone is what that would cost.
    RetentionCandidateRetiredByVerdict,
    DfuScanSuccess,
    DfuScanFailure,
    DfuInstallStart,
    DfuInstallRefusal,
    DfuConfirmedUpdate,
    DfuFailedUpdate,
    PlatformForgetBond,
    PlatformCardSpaceScan,
    DerivedRideTrackRepeatedUntilFill,
    DerivedNavPreviewRepeatedUntilFill,
    WeatherRefreshState,
    WeatherInstalledDataChange,
    WeatherStaleData,
    WeatherAlertDelivery,
}

/// The corpus's one installed weather product.
pub const WEATHER_PRODUCT: u64 = 4;

/// A sampled bundle: `frames` dry rain frames 15 minutes apart from `now`, no rain grid.
pub fn weather_bundle(now: i64, frames: usize) -> obc_app::WeatherSnapshot {
    let mut table = heapless::Vec::new();
    for index in 0..frames {
        table
            .push(obc_app::weather::FrameSample {
                valid_at: now + index as i64 * 900,
                intensity: 0,
                lat: 0,
                lon: 0,
                past_route_end: false,
                spread_uncertain: false,
            })
            .expect("the synthetic table fits");
    }
    obc_app::WeatherSnapshot {
        generated_at: now,
        valid_from: now - 3_600,
        valid_until: now + 24 * 3_600,
        hourly: [obc_formats::obcw::HourlyRecord {
            valid_time_offset_s: 0,
            temperature_deci_c: 150,
            precipitation_tenth_mm: 0,
            precipitation_probability_pct: 0,
            condition: obc_formats::obcw::CONDITION_CLEAR,
            wind_from_deg: 200,
            wind_speed_deci_ms: 20,
            wind_gust_deci_ms: 30,
            flags: 0,
        }; obc_formats::obcw::HOURLY_COUNT],
        frames: table,
        frame_cap_s: 900,
        sampled_at: Some((0, 0)),
        pos_in_grid: true,
        current_pos_in_grid: true,
        projected: false,
        frames_truncated: false,
        rain_grid: None,
    }
}

/// The corpus's one trip folder, and the two routes it groups — one id space, as every store the
/// namespace-free `CatalogEffect::RemoveObject` runs against has.
pub const TRIP: u64 = 50;

pub const ALL_REQUIREMENTS: &[Requirement] = &[
    Requirement::CatalogStoreChange,
    Requirement::CatalogRefresh,
    Requirement::CatalogRouteDelete,
    Requirement::CatalogRideDelete,
    Requirement::CatalogTripCascade,
    Requirement::CatalogDeleteOrdersRefresh,
    Requirement::CatalogRefreshRetry,
    Requirement::CatalogUploadOrder,
    Requirement::CatalogIdentityRemap,
    Requirement::NavigationPlan,
    Requirement::NavigationCancel,
    Requirement::NavigationLateResult,
    Requirement::NavigationReplacement,
    Requirement::NavigationDetourPlan,
    Requirement::NavigationDetourCancel,
    Requirement::NavigationDetourCommit,
    Requirement::NavigationNoPath,
    Requirement::RecorderStart,
    Requirement::RecorderSave,
    Requirement::RecorderDiscard,
    Requirement::RecorderFinalizeFailure,
    Requirement::RecorderSessionReplacement,
    Requirement::SettingsDirtyRevision,
    Requirement::SettingsSuccess,
    Requirement::SettingsStaleResult,
    Requirement::SettingsFailure,
    Requirement::SettingsRetry,
    Requirement::RetentionRouteUseStamp,
    Requirement::RetentionRideSyncStamp,
    Requirement::RetentionExpiryDelete,
    Requirement::RetentionRetry,
    Requirement::RetentionTrustedClockGate,
    Requirement::RetentionCandidateRetiredByVerdict,
    Requirement::DfuScanSuccess,
    Requirement::DfuScanFailure,
    Requirement::DfuInstallStart,
    Requirement::DfuInstallRefusal,
    Requirement::DfuConfirmedUpdate,
    Requirement::DfuFailedUpdate,
    Requirement::PlatformForgetBond,
    Requirement::PlatformCardSpaceScan,
    Requirement::DerivedRideTrackRepeatedUntilFill,
    Requirement::DerivedNavPreviewRepeatedUntilFill,
    Requirement::WeatherRefreshState,
    Requirement::WeatherInstalledDataChange,
    Requirement::WeatherStaleData,
    Requirement::WeatherAlertDelivery,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Settle,
    StoreChanged,
    RefreshCatalogs,
    UploadFirstRoute,
    UploadSecondRoute,
    UploadTrip,
    RemapCatalogIdentity,
    DeleteRoute,
    DeleteRide,
    CascadeDeleteTrip,
    RenameRouteBehindAFailingRead,
    StartRoutePlan,
    CancelRoutePlan,
    DeliverLateRouteResult,
    ReplaceRoutePlan,
    PlanDetour,
    CancelDetour,
    CommitDetour,
    RouteNoPath,
    StartRecorder,
    SaveRecorder,
    DiscardRecorder,
    FailRecorderFinalize,
    ReplaceRecorderSession,
    DirtySettings,
    PersistSettings,
    DeliverStaleSettingsResult,
    DeliverMatchingSettingsResult,
    FailSettingsPersist,
    RetrySettingsPersist,
    StampRouteUse,
    StampRideSync,
    DeleteExpiredObject,
    RetryExpiredDelete,
    SleepPastDeleteBackoff,
    GateExpiryUntilClockTrusted,
    ScanDfuSuccess,
    ScanDfuFailure,
    StartDfuInstall,
    AdmitDfuInstall,
    RefuseDfuInstall,
    ConfirmUpdate,
    FailUpdate,
    ForgetBond,
    ScanCardSpace,
    NeedRideTrack,
    RemapRideIdentity,
    ReplaceRideTrackNeed,
    FillRideTrack,
    NeedNavPreview,
    ReplaceNavPreviewNeed,
    FillNavPreview,
    RefreshWeather,
    InstallWeatherData,
    MarkWeatherStale,
    DeliverWeatherAlert,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenState {
    Home,
    Menu,
    Routes,
    RouteOverview,
    Rides,
    RideDetail,
    Map,
    Detour,
    Planning,
    DetourPreview,
    DfuCheck,
    DfuConfirm,
    DfuProgress,
    DfuInstalling,
    DfuError,
    Warning,
    WeatherAlert,
    /// Any screen the projection does not name, carrying its variant name so two runners resting on
    /// *different* unnamed screens still compare unequal.
    Other(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleState {
    pub screen: ScreenState,
    pub stack_depth: usize,
    pub mode: Mode,
    pub route_names: Vec<String>,
    pub route_ids: Vec<ObjectKey>,
    pub ride_names: Vec<String>,
    pub ride_ids: Vec<ObjectKey>,
    pub trip_names: Vec<String>,
    pub trip_ids: Vec<ObjectKey>,
    pub active_route_name: Option<String>,
    pub active_route_id: Option<ObjectKey>,
    pub requested_ride_id: Option<ObjectKey>,
    pub recording: bool,
    pub clock_trusted: bool,
    /// The rain map's step range, as the **domain** derived it from the frame's sampled snapshot
    /// (#1549) — no host computes this any more, so the two cadences agreeing on it is a statement
    /// about `WeatherDomain`, not about a setter both of them happened to call.
    pub rain_steps_ahead: u8,
    /// Which weather product the device believes is installed, and at what revision.
    pub weather_installed: Option<obc_app::device_core::WeatherData>,
    /// How many refresh requests actually went out to the executor — one radio trip each.
    pub weather_refreshes: u16,
    pub settings_revision: Option<RevisionKey>,
    pub settings_utc_offset_min: i16,
    pub nav_preview_missing: bool,
    pub warning: Option<WarningFlags>,
    pub retention_delete_attempts: u16,
}

#[derive(Debug, Clone, Copy)]
pub enum PendingSettingsResult {
    PersistLatest,
    FailLatest,
    PersistRevision(u16),
}

/// The corpus's shared state: the `App` every runner drives, the repositories behind it, and the
/// scripted completions a runner answers with.
///
/// Inputs are real public app operations or UI gestures. Planner, detour-commit,
/// recorder-finalize and derived-fill completions are *scripted* here rather than computed, because
/// this is a fast behaviour corpus and not a second planner: the real planner/repository path is
/// exercised by the fixture-backed suites and by the simulator.
pub struct CorpusState {
    pub app: App,
    pub routes: Vec<RouteSummary>,
    pub route_ids: Vec<u64>,
    pub rides: Vec<RideSummary>,
    pub ride_ids: Vec<u64>,
    pub trip_stage_ids: Vec<u64>,
    pub trip_present: bool,
    pub nav_generation: u16,
    pub fail_next_finalize: bool,
    pub commit_success_pending: bool,
    pub pending_nav_plan: Option<Result<u64, NavError>>,
    pub pending_dfu_scan: Option<Result<DfuScanReport, DfuScanError>>,
    pub pending_dfu_install: Option<Result<(), DfuInstallError>>,
    pub pending_settings_result: Option<PendingSettingsResult>,
    pub settings_revision: u16,
    pub route_delete_fail_once: bool,
    /// The next catalog read does not answer. One shot, so the retry that follows it succeeds and
    /// the scenario has a rider-visible difference between "re-offered" and "lost".
    pub catalog_read_fail_once: bool,
    pub retention_delete_attempts: u16,
    /// How many weather refreshes the executor has raised — the counter behind
    /// [`VisibleState::weather_refreshes`].
    pub weather_refreshes: u16,
    /// The host's sampled weather snapshot, handed to every pass as `PassInputs::weather` — the
    /// borrow stage 10 derives the rain view state and the alert decision from.
    pub weather: Option<obc_app::WeatherSnapshot>,
    /// The resample counter reported as `ExternalFacts::note_weather_sample`.
    pub weather_sample: u64,
    pub settings_retry_requested: bool,
    /// What the executor has handed back, waiting for the next pass to read it.
    pub facts: ExternalFacts,
    pub outcomes: OutcomeSlots,
    pub derived: DerivedInputs,
    /// The store revision this fixture's repositories report. They have none of their own, so the
    /// executor mints a monotonic one per commit and per read — exactly what `HostLoop` does.
    pub store_revision: u64,
    /// The navigation operation the executor is holding. The corpus scripts a detour search's answer
    /// and the splice's answer at the *action* that produces them rather than at the request, so
    /// they are built against whatever is actually running.
    pub nav_token: Option<OperationToken<NavigatorTag>>,
    /// The settings write the executor is holding, for the same reason: a scripted answer may be a
    /// *stale* ack for a revision a newer edit already superseded.
    pub settings_token: Option<OperationToken<SettingsTag>>,
}

impl CorpusState {
    pub fn new() -> Self {
        let routes = vec![route("Alpha"), route("Beta"), route("Gamma")];
        let route_ids = vec![10, 20, 30];
        let rides = vec![ride("Morning"), ride("Evening")];
        let ride_ids = vec![70, 90];
        let trip_stage_ids = vec![10, 20];
        let mut app = App::new_idle(AppState::new(8_330_000, 46_570_000, 1.0));
        app.set_routes_with_ids(&routes, &route_ids);
        app.set_rides(&rides, &ride_ids);
        app.set_trips(&[TripInput { id: TRIP, name: "Alps", stage_ids: &trip_stage_ids }]);
        Self {
            app,
            routes,
            route_ids,
            rides,
            ride_ids,
            trip_stage_ids,
            trip_present: true,
            nav_generation: 0,
            fail_next_finalize: false,
            commit_success_pending: false,
            pending_nav_plan: None,
            pending_dfu_scan: None,
            pending_dfu_install: None,
            pending_settings_result: None,
            settings_revision: 0,
            route_delete_fail_once: false,
            catalog_read_fail_once: false,
            retention_delete_attempts: 0,
            weather_refreshes: 0,
            weather: None,
            weather_sample: 0,
            settings_retry_requested: false,
            facts: ExternalFacts::NONE,
            outcomes: OutcomeSlots::new(),
            derived: DerivedInputs::NONE,
            store_revision: 0,
            nav_token: None,
            settings_token: None,
        }
    }

    /// Whether a search is running — the executor answers it on the pass after the action that
    /// starts it, so an action that wants a fresh plan waits for the last one to land.
    pub fn planning(&self) -> bool {
        self.app.core_mode() == ModeState::Searching
    }

    /// The store moved underneath the executor — the level a commit reports.
    pub fn note_store_commit(&mut self) {
        self.store_revision += 1;
        self.facts.note_store_revision(StoreRevision {
            store: StoreIdentity::new(1),
            revision: Revision::new(self.store_revision),
        });
    }

    /// Answer the navigation operation the executor is holding. Nothing to answer means the search
    /// this action scripts a result for was never handed out.
    pub fn answer_nav(&mut self, build: impl FnOnce(OperationToken<NavigatorTag>) -> NavigatorOutcome) {
        if let Some(token) = self.nav_token.take() {
            let _ = self.outcomes.navigator.try_put(build(token));
        }
    }

    pub fn feed_routes(&mut self, key: &'static str, trace: &mut TraceRecorder<VisibleState>) {
        self.app.set_routes_with_ids(&self.routes, &self.route_ids);
        trace.record_feeder(FeederCall::new(FeederKind::RouteCatalog, key, self.routes.len()));
    }

    pub fn feed_rides(&mut self, key: &'static str, trace: &mut TraceRecorder<VisibleState>) {
        self.app.set_rides(&self.rides, &self.ride_ids);
        trace.record_feeder(FeederCall::new(FeederKind::RideCatalog, key, self.rides.len()));
    }

    pub fn feed_trips(&mut self, key: &'static str, trace: &mut TraceRecorder<VisibleState>) {
        if self.trip_present {
            self.app.set_trips(&[TripInput { id: TRIP, name: "Alps", stage_ids: &self.trip_stage_ids }]);
        } else {
            self.app.set_trips(&[]);
        }
        trace.record_feeder(FeederCall::new(FeederKind::TripCatalog, key, usize::from(self.trip_present)));
    }

    fn reset_to_riding_map(&mut self) {
        self.app = App::new_idle(AppState::new(7_500_000, 43_500_000, 1.0));
        self.app.set_routes_with_ids(&self.routes, &self.route_ids);
        self.app.set_rides(&self.rides, &self.ride_ids);
        self.app.set_map_nav_graph(true);
        self.app.state.user_fix = Some(road_fix(0.0));
        self.app.apply_gesture(Gesture::BackHold);
        self.app.apply_gesture(Gesture::Press);
        // The first row is the trip folder; enter it, open the first stage, then start the ride.
        self.app.apply_gesture(Gesture::Press);
        self.app.apply_gesture(Gesture::Press);
        self.app.apply_gesture(Gesture::Press);
        let bytes = road_obcr();
        let source = SliceSource(&bytes);
        let index = RouteIndex::read(&source).unwrap();
        let reader = RouteReader::new(&index, &source);
        struct OneFix(Option<Fix>);
        impl LocationSource for OneFix {
            fn poll(&mut self) -> Option<Fix> {
                self.0.take()
            }
        }
        let mut location = OneFix(Some(road_fix(0.31)));
        self.app.tick(RideClock(0), Sensors::new(&mut location), Some(&reader));
    }

    fn tick_without_fix(&mut self) {
        struct NoFix;
        impl LocationSource for NoFix {
            fn poll(&mut self) -> Option<obc_ports::Fix> {
                None
            }
        }
        let mut location = NoFix;
        self.app.tick(RideClock(0), Sensors::new(&mut location), None);
    }

    fn open_detour_plan(&mut self) {
        if !matches!(self.app.top_screen(), Screen::Detour(_)) {
            self.reset_to_riding_map();
            self.app.apply_gesture(Gesture::BackHold);
            self.app.apply_gesture(Gesture::Step(1));
            self.app.apply_gesture(Gesture::Press);
        }
        self.app.apply_gesture(Gesture::Press);
    }

    /// Apply one rider input. Shared by every runner: the corpus's inputs are real public app
    /// operations and UI gestures, so what a runner *is* differs only in how the work they produce
    /// is served.
    pub fn apply_input(&mut self, action: &Action, trace: &mut TraceRecorder<VisibleState>) {
        match action {
            Action::Settle => {}
            Action::StoreChanged => self.note_store_commit(),
            Action::RefreshCatalogs => {}
            // The burst is three actions, not one, and that is the whole point of the row: the
            // upload slot is single and most-recent-wins, so two uploads reported before one pass
            // consumes it would leave the first one unobserved and the *order* untested.
            Action::UploadFirstRoute => {
                self.feed_routes("upload.routes", trace);
                self.facts.note_route_upload(RouteUpload { id: 10, replaced: false, elevation: None });
            }
            Action::UploadSecondRoute => {
                self.facts.note_route_upload(RouteUpload { id: 20, replaced: false, elevation: None });
            }
            Action::UploadTrip => {
                self.feed_trips("upload.trip", trace);
                self.facts.note_trip_upload(TripUpload { id: TRIP, replaced: false });
            }
            Action::RemapCatalogIdentity => {
                self.routes.swap(0, 2);
                self.route_ids.swap(0, 2);
                self.feed_routes("catalog.remap", trace);
                self.feed_trips("catalog.remap-trips", trace);
            }
            Action::DeleteRoute => {
                self.app.apply_gesture(Gesture::Press);
                self.app.apply_gesture(Gesture::Press);
                // The trip folders are first. Open the first stage, then its first route overview.
                self.app.apply_gesture(Gesture::Press);
                self.app.apply_gesture(Gesture::Press);
                self.app.apply_gesture(Gesture::Step(1));
                self.app.apply_gesture(Gesture::Hold);
            }
            Action::DeleteRide => {
                self.app.apply_gesture(Gesture::Press);
                self.app.apply_gesture(Gesture::Step(1));
                self.app.apply_gesture(Gesture::Press);
                self.app.apply_gesture(Gesture::Press);
                self.app.apply_gesture(Gesture::Hold);
            }
            // The store moved and the *first* read of it will not answer. The rename is what makes
            // the retry rider-visible: until a read lands, the menu still shows the old name, so a
            // re-offer that never happened is a scenario that settles on stale rows.
            Action::RenameRouteBehindAFailingRead => {
                self.routes[2] = route("Delta");
                self.catalog_read_fail_once = true;
                self.note_store_commit();
            }
            Action::CascadeDeleteTrip => {
                self.app.apply_gesture(Gesture::Press);
                self.app.apply_gesture(Gesture::Press);
                self.app.apply_gesture(Gesture::Hold);
                self.app.apply_gesture(Gesture::Step(1));
                self.app.apply_gesture(Gesture::Hold);
            }
            Action::StartRoutePlan => {
                assert!(self.app.debug_start_nav((0, 0), (1_000, 1_000), "First plan"));
            }
            Action::CancelRoutePlan => self.app.apply_gesture(Gesture::Back),
            Action::DeliverLateRouteResult => {
                self.feed_routes("nav.old-publication", trace);
                // The operation the rider walked away from, answered late: Navigator refuses a
                // token it no longer holds, which is the whole point of the scenario.
                self.answer_nav(|token| NavigatorOutcome::PlanFinished { token, route: 10 });
            }
            Action::ReplaceRoutePlan => {
                assert!(self.app.debug_start_nav((0, 0), (2_000, 2_000), "Replacement"));
            }
            Action::PlanDetour => self.open_detour_plan(),
            Action::CancelDetour => self.app.apply_gesture(Gesture::Back),
            Action::CommitDetour => {
                self.app.set_detour_preview(&[(0, 0), (100, 100)]);
                trace.record_feeder(FeederCall::new(FeederKind::DetourPreview, "detour.preview", 2));
                self.answer_nav(|token| NavigatorOutcome::DetourFinished {
                    token,
                    preview: DetourPreview {
                        cost_delta_m: 100,
                        total_distance_m: 900,
                        rejoin_m: 1_000,
                        ascent_m: Some(30),
                    },
                });
                self.app.apply_gesture(Gesture::Press);
                self.commit_success_pending = true;
            }
            Action::RouteNoPath => {
                assert!(self.app.debug_start_nav((0, 0), (1_000, 1_000), "No path"));
                self.pending_nav_plan = Some(Err(NavError::NoPath));
            }
            Action::StartRecorder => self.app.activity.start_session(),
            Action::SaveRecorder => {
                self.app.activity.end_session();
                self.app.activity.request_track(TrackAction::Save);
            }
            Action::DiscardRecorder => {
                self.app.activity.end_session();
                self.app.activity.request_track(TrackAction::Discard);
            }
            Action::FailRecorderFinalize => {
                self.app.activity.request_track(TrackAction::Save);
                // Compatibility limitation: the legacy vocabulary has no recorder-finalize outcome;
                // hosts report the failure through the generic warning event.
                self.fail_next_finalize = true;
            }
            Action::ReplaceRecorderSession => {
                self.app.activity.end_session();
                self.app.activity.start_session();
            }
            Action::DirtySettings => {
                let settings = *self.app.settings();
                self.app.set_settings(settings);
                trace.feeder_revision(FeederKind::Settings, "settings.boot", 0, 1);
                self.app.stamp_clock_ble(1_720_000_000, 60);
                self.settings_revision = 1;
            }
            Action::PersistSettings => {}
            Action::DeliverStaleSettingsResult => {
                self.app.stamp_clock_ble(1_720_000_060, 120);
                self.settings_revision = 2;
                self.pending_settings_result = Some(PendingSettingsResult::PersistRevision(1));
            }
            Action::DeliverMatchingSettingsResult => {
                self.pending_settings_result = Some(PendingSettingsResult::PersistRevision(self.settings_revision));
            }
            Action::FailSettingsPersist => self.pending_settings_result = Some(PendingSettingsResult::FailLatest),
            Action::RetrySettingsPersist => {
                // The failed revision is retained through the real bounded retry window.
                self.settings_retry_requested = true;
                self.app.advance_animations(InputClock(4_002));
                self.pending_settings_result = Some(PendingSettingsResult::PersistLatest);
            }
            Action::StampRouteUse => {
                self.app.stamp_clock_ble(1_720_000_000, 60);
                self.facts.note_route_upload(RouteUpload { id: 10, replaced: false, elevation: None });
            }
            Action::StampRideSync => {
                self.app.stamp_clock_ble(1_720_000_000, 60);
                let mut stamped = self.rides[0].clone();
                stamped.synced = true;
                stamped.synced_at_utc = 0;
                self.rides[0] = stamped;
                self.feed_rides("retention.synced", trace);
                self.app.set_ride_retention_inventory(&[RideRetentionRecord {
                    id: self.ride_ids[0],
                    synced: true,
                    synced_at_utc: 0,
                }]);
                trace.record_feeder(FeederCall::new(FeederKind::RideRetention, "retention.rides", 1));
                self.app.force_retention_sweep();
                self.tick_without_fix();
            }
            Action::DeleteExpiredObject => {
                self.app.stamp_clock_ble(1_720_000_000, 60);
                self.app.set_route_meta(&[
                    obc_app::RouteRetentionMeta::new(obc_app::Retention::Day1, 1),
                    obc_app::RouteRetentionMeta::new(obc_app::Retention::Never, 0),
                    obc_app::RouteRetentionMeta::new(obc_app::Retention::Never, 0),
                ]);
                trace.record_feeder(FeederCall::new(FeederKind::RouteRetention, "retention.routes", 3));
                self.route_delete_fail_once = true;
                self.app.force_retention_sweep();
                self.tick_without_fix();
            }
            Action::RetryExpiredDelete => {
                // The original failed candidate owns its retry; no new discovery sweep is needed.
                self.app.advance_animations(InputClock(5_002));
                self.tick_without_fix();
            }
            Action::SleepPastDeleteBackoff => {
                // The removal was answered, and the device slept past the delete backstop before
                // the re-read that answer ordered re-fed the catalogs — the board's ordinary
                // cadence. Nothing may order a second removal for an object already gone (#1548).
                self.app.advance_animations(InputClock(9_002));
                self.tick_without_fix();
            }
            Action::GateExpiryUntilClockTrusted => {
                assert!(!self.app.clock_trusted());
                self.app.force_retention_sweep();
                self.tick_without_fix();
            }
            Action::ScanDfuSuccess => {
                assert!(self.app.open_remote_dfu_check());
                self.pending_dfu_scan = Some(Ok(DfuScanReport::new("v1", "v2", false)));
            }
            Action::ScanDfuFailure => {
                // Finish the preceding flow, then open a fresh real scan wait.
                self.app.apply_gesture(Gesture::Back);
                assert!(self.app.open_remote_dfu_check());
                self.pending_dfu_scan = Some(Err(DfuScanError::Damaged));
            }
            Action::StartDfuInstall => {
                assert!(self.app.open_remote_dfu_check());
                self.pending_dfu_scan = Some(Ok(DfuScanReport::new("v1", "v2", false)));
            }
            Action::AdmitDfuInstall => {
                self.app.apply_gesture(Gesture::Press);
                self.pending_dfu_install = Some(Ok(()));
            }
            Action::RefuseDfuInstall => {
                self.app.apply_gesture(Gesture::Press);
                self.pending_dfu_install = Some(Err(DfuInstallError::Recording));
            }
            Action::ConfirmUpdate => {
                self.facts.note_update_result(UpdateResult::Confirmed(clamp("v2"))).expect("no verdict pending");
            }
            Action::FailUpdate => self
                .facts
                .note_update_result(UpdateResult::Failed { why: DfuFailure::Reverted, staged: Some(clamp("v3")) })
                .expect("no verdict pending"),
            Action::ForgetBond => self.app.state.ble_forget_pending = true,
            Action::ScanCardSpace => {
                self.app.apply_gesture(Gesture::Press);
                self.app.apply_gesture(Gesture::Step(-1));
                self.app.apply_gesture(Gesture::Press);
                self.app.apply_gesture(Gesture::Step(-1));
                self.app.apply_gesture(Gesture::Press);
                self.app.apply_gesture(Gesture::Step(3));
                self.app.apply_gesture(Gesture::Press);
            }
            Action::NeedRideTrack => {
                if !matches!(self.app.top_screen(), Screen::RideDetail(_)) {
                    self.app.apply_gesture(Gesture::Press);
                    self.app.apply_gesture(Gesture::Step(1));
                    self.app.apply_gesture(Gesture::Press);
                    self.app.apply_gesture(Gesture::Press);
                }
            }
            Action::RemapRideIdentity => {
                self.rides.swap(0, 1);
                self.ride_ids.swap(0, 1);
                self.feed_rides("ride.remap", trace);
            }
            Action::ReplaceRideTrackNeed => {
                self.app.apply_gesture(Gesture::Back);
                self.app.apply_gesture(Gesture::Step(1));
                self.app.apply_gesture(Gesture::Press);
            }
            Action::FillRideTrack => {}
            // A computed overview is what wants a preview, and a plan is how one gets there. The
            // executor answers the search on the pass that follows this action, so a plan that is
            // still running is not started again.
            Action::NeedNavPreview => {
                if !self.app.nav_preview_missing() && !self.planning() {
                    assert!(self.app.debug_start_nav((0, 0), (1_000, 1_000), "Preview"));
                    self.feed_routes("nav.preview-route", trace);
                    self.pending_nav_plan = Some(Ok(10));
                    self.nav_generation = self.nav_generation.wrapping_add(1);
                }
            }
            Action::ReplaceNavPreviewNeed => {
                self.app.apply_gesture(Gesture::Back);
                if !self.planning() {
                    assert!(self.app.debug_start_nav((0, 0), (2_000, 2_000), "New preview"));
                    self.feed_routes("nav.preview-replacement", trace);
                    self.pending_nav_plan = Some(Ok(10));
                    self.nav_generation = self.nav_generation.wrapping_add(1);
                }
            }
            Action::FillNavPreview => {}
            // The rider opens the Weather dashboard from the Menu — `menu.rs`'s row is the intent's
            // one producer, and a companion is what makes the request serviceable, so the link
            // level goes in with it. What must follow is exactly one `WeatherEffect::RequestRefresh`.
            Action::RefreshWeather => {
                self.facts.note_link(obc_app::BleStatus {
                    link: obc_app::BleLink::Connected,
                    paired: true,
                    passkey: None,
                });
                self.app.apply_gesture(Gesture::Press); // Home → Menu
                for _ in 0..4 {
                    self.app.apply_gesture(Gesture::Step(1));
                }
                self.app.apply_gesture(Gesture::Press); // → the Weather dashboard
            }
            // A bundle lands: the platform reports the installed identity, and the host resamples
            // it. Five frames, so four lie ahead of NOW — a step range no host computed.
            Action::InstallWeatherData => {
                self.facts.note_weather_data(WeatherData {
                    data: DataIdentity::new(WEATHER_PRODUCT),
                    revision: Revision::new(1),
                });
                self.resample(Some(5), trace);
            }
            // The bundle ages out from under the rider: the next resample finds nothing current, so
            // the step range collapses — and the installed identity does **not**, because a stale
            // sample is not an uninstall.
            Action::MarkWeatherStale => self.resample(None, trace),
            Action::DeliverWeatherAlert => {
                assert!(self.app.show_weather_alert(obc_app::WeatherAlertKind::Storm, 12));
            }
        }
    }

    /// The host resampled: swap the borrow the next pass reads and report the new revision. `None`
    /// is a sample that found nothing current — a stale or unreadable bundle.
    fn resample(&mut self, frames: Option<usize>, trace: &mut TraceRecorder<VisibleState>) {
        let now = self.app.wall_unix_now() as i64;
        self.weather = frames.map(|count| weather_bundle(now, count));
        self.weather_sample += 1;
        self.facts.note_weather_sample(Revision::new(self.weather_sample));
        trace.record_feeder(FeederCall::new(FeederKind::WeatherSnapshot, "weather.sample", frames.unwrap_or(0)));
    }

    pub fn snapshot_state(&self) -> VisibleState {
        visible_state(&self.app, self.settings_revision, self.retention_delete_attempts, self.weather_refreshes)
    }
}

/// The normalized rider-visible state every runner is compared on.
pub fn visible_state(
    app: &App,
    settings_revision: u16,
    retention_delete_attempts: u16,
    weather_refreshes: u16,
) -> VisibleState {
    let screen = match app.top_screen() {
        Screen::Home(_) => ScreenState::Home,
        Screen::Menu(_) => ScreenState::Menu,
        Screen::RouteMenu(_) => ScreenState::Routes,
        Screen::RouteOverview(_) => ScreenState::RouteOverview,
        Screen::Rides(_) => ScreenState::Rides,
        Screen::RideDetail(_) => ScreenState::RideDetail,
        Screen::Map(_) => ScreenState::Map,
        Screen::Detour(_) => ScreenState::Detour,
        Screen::NavPlanning(_) => ScreenState::Planning,
        Screen::DetourPreview(_) => ScreenState::DetourPreview,
        Screen::DfuCheck(_) => ScreenState::DfuCheck,
        Screen::DfuConfirm(_) => ScreenState::DfuConfirm,
        Screen::DfuProgress(_) => ScreenState::DfuProgress,
        Screen::DfuInstalling(_) => ScreenState::DfuInstalling,
        Screen::DfuError(_) => ScreenState::DfuError,
        Screen::Warning(_) => ScreenState::Warning,
        Screen::WeatherAlert(_) => ScreenState::WeatherAlert,
        other => ScreenState::Other(other.name()),
    };
    VisibleState {
        screen,
        stack_depth: app.debug_stack_len(),
        mode: app.mode(),
        route_names: app.routes().iter().map(|item| item.name.as_str().to_owned()).collect(),
        route_ids: app.route_ids().iter().map(|&id| fixture_object_key(ObjectKind::Route, id)).collect(),
        ride_names: app.rides().iter().map(|item| item.name.as_str().to_owned()).collect(),
        ride_ids: app.ride_ids().iter().map(|&id| fixture_object_key(ObjectKind::Ride, id)).collect(),
        trip_names: app.trips().iter().map(|item| item.name.as_str().to_owned()).collect(),
        trip_ids: app.trips().iter().map(|trip| fixture_object_key(ObjectKind::Trip, trip.id)).collect(),
        active_route_name: app
            .active_route_index()
            .and_then(|index| app.routes().get(index))
            .map(|item| item.name.as_str().to_owned()),
        active_route_id: app
            .active_route_index()
            .and_then(|index| app.route_ids().get(index))
            .map(|&id| fixture_object_key(ObjectKind::Route, id)),
        requested_ride_id: app.derived_needs().ride_track.map(|key| fixture_object_key(ObjectKind::Ride, key.ride)),
        recording: app.activity.is_tracking(),
        clock_trusted: app.clock_trusted(),
        rain_steps_ahead: app.weather().steps_ahead(),
        weather_installed: app.weather().installed(),
        weather_refreshes,
        settings_revision: match settings_revision {
            0 => None,
            1 => Some(RevisionKey(0)),
            2 => Some(RevisionKey(1)),
            other => panic!("unseeded fixture settings revision {other}"),
        },
        settings_utc_offset_min: app.settings().utc_offset_min,
        nav_preview_missing: app.nav_preview_missing(),
        warning: match app.top_screen() {
            Screen::Warning(card) => Some(card.flags()),
            _ => None,
        },
        retention_delete_attempts,
    }
}

struct BorrowedRoutes<'a> {
    catalog: &'a mut Vec<RouteSummary>,
    ids: &'a mut Vec<u64>,
    fail_delete_once: &'a mut bool,
    delete_attempts: &'a mut u16,
}

impl RouteRepository for BorrowedRoutes<'_> {
    fn catalog(&self) -> &[RouteSummary] {
        self.catalog
    }

    fn ids(&self) -> &[u64] {
        self.ids
    }

    fn delete_by_id(&mut self, id: u64) -> bool {
        *self.delete_attempts = self.delete_attempts.saturating_add(1);
        if std::mem::take(self.fail_delete_once) {
            return false;
        }
        let Some(index) = self.ids.iter().position(|candidate| *candidate == id) else { return false };
        self.ids.remove(index);
        self.catalog.remove(index);
        true
    }

    fn write_nav_route(&mut self, _bytes: &[u8]) -> Option<u64> {
        None
    }

    fn sync_active(&mut self, _want: Option<usize>) -> bool {
        false
    }

    fn active_source(&self) -> Option<SliceSource<'_>> {
        None
    }

    fn invalidate_active(&mut self) {}
}

struct BorrowedRides<'a> {
    catalog: &'a mut Vec<RideSummary>,
    ids: &'a mut Vec<u64>,
}

impl RideRepository for BorrowedRides<'_> {
    fn catalog(&self) -> &[RideSummary] {
        self.catalog
    }

    fn ids(&self) -> &[u64] {
        self.ids
    }

    fn delete_by_id(&mut self, id: u64) -> bool {
        let Some(index) = self.ids.iter().position(|candidate| *candidate == id) else { return false };
        self.ids.remove(index);
        self.catalog.remove(index);
        true
    }

    fn profile_by_id(&self, _id: u64) -> Option<obc_route::Profile> {
        None
    }

    fn preview_by_id(&self, _id: u64) -> Vec<(i32, i32)> {
        vec![(0, 0), (1, 1)]
    }
}

struct BorrowedTrips<'a> {
    present: &'a mut bool,
    stage_ids: &'a [u64],
}

impl TripCatalog for BorrowedTrips<'_> {
    fn delete_by_id(&mut self, id: u64) -> bool {
        if *self.present && id == TRIP {
            *self.present = false;
            true
        } else {
            false
        }
    }

    fn refeed(&self, app: &mut App) {
        if *self.present {
            app.set_trips(&[TripInput { id: TRIP, name: "Alps", stage_ids: self.stage_ids }]);
        } else {
            app.set_trips(&[]);
        }
    }
}

fn ride_delivery_key(requested: u64, current: Option<u64>) -> &'static str {
    match (requested, current) {
        (70, Some(70)) => "ride.track.requested-morning.current-morning",
        (70, Some(90)) => "ride.track.requested-morning.current-evening",
        (90, Some(70)) => "ride.track.requested-evening.current-morning",
        (90, Some(90)) => "ride.track.requested-evening.current-evening",
        (70, None) => "ride.track.requested-morning.current-none",
        (90, None) => "ride.track.requested-evening.current-none",
        (_, Some(70)) => "ride.track.requested-other.current-morning",
        (_, Some(90)) => "ride.track.requested-other.current-evening",
        (_, Some(_)) => "ride.track.requested-other.current-other",
        (_, None) => "ride.track.requested-other.current-none",
    }
}

fn nav_delivery_key(requested: u16, current: u16) -> &'static str {
    match (requested, current) {
        (0, 0) => "nav.preview.requested-g0.current-g0",
        (0, 1) => "nav.preview.requested-g0.current-g1",
        (0, 2) => "nav.preview.requested-g0.current-g2",
        (1, 1) => "nav.preview.requested-g1.current-g1",
        (1, 2) => "nav.preview.requested-g1.current-g2",
        (2, 2) => "nav.preview.requested-g2.current-g2",
        _ if requested == current => "nav.preview.requested-other.current-matching",
        _ => "nav.preview.requested-other.current-replacement",
    }
}

pub fn fixture_object_key(kind: ObjectKind, id: u64) -> ObjectKey {
    match (kind, id) {
        (ObjectKind::Route, 10) => ObjectKey(0),
        (ObjectKind::Route, 20) => ObjectKey(1),
        (ObjectKind::Route, 30) => ObjectKey(2),
        (ObjectKind::Ride, 70) => ObjectKey(3),
        (ObjectKind::Ride, 90) => ObjectKey(4),
        (ObjectKind::Trip, TRIP) => ObjectKey(5),
        _ => panic!("unseeded fixture identity {kind:?}:{id}"),
    }
}

pub fn route(name: &str) -> RouteSummary {
    let mut summary = RouteSummary {
        name: Default::default(),
        distance_km: 10,
        climb_m: 100,
        bbox: BBox { min_lon: 0, min_lat: 0, max_lon: 1_000, max_lat: 1_000 },
        start_lon: 0,
        start_lat: 0,
    };
    summary.name.push_str(name).unwrap();
    summary
}

pub fn ride(name: &str) -> RideSummary {
    let mut summary = RideSummary {
        name: Default::default(),
        start_time: 1_720_000_000,
        distance_m: 1_000,
        moving_time_s: 600,
        climb_m: 10,
        synced: false,
        synced_at_utc: 0,
    };
    summary.name.push_str(name).unwrap();
    summary
}

#[derive(Default)]
struct TestSink(Vec<u8>);

impl ByteSink for TestSink {
    fn write(&mut self, bytes: &[u8]) -> Result<(), obc_formats::io::Error> {
        self.0.extend_from_slice(bytes);
        Ok(())
    }

    fn patch_at(&mut self, offset: u32, bytes: &[u8]) -> Result<(), obc_formats::io::Error> {
        let start = offset as usize;
        self.0[start..start + bytes.len()].copy_from_slice(bytes);
        Ok(())
    }
}

pub fn road_obcr() -> Vec<u8> {
    let mut gpx = String::from("<gpx><trk><trkseg>");
    for index in 0..=10 {
        let lon = 7.50 + 0.04 * index as f64 / 10.0;
        gpx.push_str(&format!("<trkpt lat=\"43.5000000\" lon=\"{lon:.7}\"><ele>100</ele></trkpt>"));
    }
    gpx.push_str("</trkseg></trk></gpx>");
    let mut sink = TestSink::default();
    gpx_to_obcr(&SliceSource(gpx.as_bytes()), "Trace road", &mut sink).unwrap();
    sink.0
}

pub fn road_fix(fraction: f64) -> Fix {
    Fix::at(43_500_000, ((7.50 + 0.04 * fraction) * 1e6) as i32)
}

pub struct Scenario {
    pub name: &'static str,
    pub requirements: &'static [Requirement],
    pub actions: &'static [Action],
}

pub const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "catalog.refresh-upload-remap",
        requirements: &[
            Requirement::CatalogStoreChange,
            Requirement::CatalogRefresh,
            Requirement::CatalogUploadOrder,
            Requirement::CatalogIdentityRemap,
        ],
        actions: &[
            Action::StoreChanged,
            Action::RefreshCatalogs,
            Action::UploadFirstRoute,
            Action::UploadSecondRoute,
            Action::UploadTrip,
            Action::RemapCatalogIdentity,
        ],
    },
    Scenario {
        name: "catalog.route-delete",
        requirements: &[Requirement::CatalogRouteDelete, Requirement::CatalogDeleteOrdersRefresh],
        actions: &[Action::DeleteRoute],
    },
    Scenario {
        name: "catalog.ride-delete",
        requirements: &[Requirement::CatalogRideDelete],
        actions: &[Action::DeleteRide],
    },
    Scenario {
        name: "catalog.trip-cascade",
        requirements: &[Requirement::CatalogTripCascade, Requirement::CatalogDeleteOrdersRefresh],
        actions: &[Action::CascadeDeleteTrip],
    },
    Scenario {
        name: "catalog.refresh-retry",
        requirements: &[Requirement::CatalogRefreshRetry],
        actions: &[Action::RenameRouteBehindAFailingRead, Action::Settle],
    },
    Scenario {
        name: "navigation.plan-cancel-late-replacement",
        requirements: &[
            Requirement::NavigationPlan,
            Requirement::NavigationCancel,
            Requirement::NavigationLateResult,
            Requirement::NavigationReplacement,
        ],
        actions: &[
            Action::StartRoutePlan,
            Action::CancelRoutePlan,
            Action::ReplaceRoutePlan,
            Action::DeliverLateRouteResult,
        ],
    },
    Scenario {
        name: "navigation.detour-lifecycle",
        requirements: &[
            Requirement::NavigationDetourPlan,
            Requirement::NavigationDetourCancel,
            Requirement::NavigationDetourCommit,
        ],
        actions: &[Action::PlanDetour, Action::CancelDetour, Action::PlanDetour, Action::CommitDetour],
    },
    Scenario {
        name: "navigation.no-path",
        requirements: &[Requirement::NavigationNoPath],
        actions: &[Action::RouteNoPath],
    },
    Scenario {
        name: "recorder.start-save-discard",
        requirements: &[Requirement::RecorderStart, Requirement::RecorderSave, Requirement::RecorderDiscard],
        actions: &[Action::StartRecorder, Action::SaveRecorder, Action::StartRecorder, Action::DiscardRecorder],
    },
    Scenario {
        name: "recorder.failure-and-session-replacement",
        requirements: &[Requirement::RecorderFinalizeFailure, Requirement::RecorderSessionReplacement],
        actions: &[Action::StartRecorder, Action::FailRecorderFinalize, Action::ReplaceRecorderSession],
    },
    Scenario {
        name: "settings.revision-success-and-stale-result",
        requirements: &[
            Requirement::SettingsDirtyRevision,
            Requirement::SettingsSuccess,
            Requirement::SettingsStaleResult,
        ],
        actions: &[
            Action::DirtySettings,
            Action::PersistSettings,
            Action::DeliverStaleSettingsResult,
            Action::DeliverMatchingSettingsResult,
        ],
    },
    Scenario {
        name: "settings.failure-and-retry",
        requirements: &[Requirement::SettingsFailure, Requirement::SettingsRetry],
        actions: &[Action::DirtySettings, Action::FailSettingsPersist, Action::RetrySettingsPersist],
    },
    Scenario {
        name: "retention.route-and-ride-stamps",
        requirements: &[Requirement::RetentionRouteUseStamp, Requirement::RetentionRideSyncStamp],
        actions: &[Action::StampRouteUse, Action::StampRideSync],
    },
    Scenario {
        name: "retention.expiry-retry-and-trusted-clock",
        requirements: &[
            Requirement::RetentionExpiryDelete,
            Requirement::RetentionRetry,
            Requirement::RetentionTrustedClockGate,
            Requirement::RetentionCandidateRetiredByVerdict,
        ],
        actions: &[
            Action::GateExpiryUntilClockTrusted,
            Action::DeleteExpiredObject,
            Action::RetryExpiredDelete,
            Action::SleepPastDeleteBackoff,
        ],
    },
    Scenario {
        name: "dfu.scan-outcomes",
        requirements: &[Requirement::DfuScanSuccess, Requirement::DfuScanFailure],
        actions: &[Action::ScanDfuSuccess, Action::Settle, Action::Settle, Action::ScanDfuFailure],
    },
    Scenario {
        name: "dfu.install-start",
        requirements: &[Requirement::DfuInstallStart],
        actions: &[Action::StartDfuInstall, Action::Settle, Action::Settle, Action::AdmitDfuInstall],
    },
    Scenario {
        name: "dfu.install-refusal",
        requirements: &[Requirement::DfuInstallRefusal],
        actions: &[Action::StartDfuInstall, Action::Settle, Action::Settle, Action::RefuseDfuInstall],
    },
    Scenario {
        name: "dfu.boot-outcomes",
        requirements: &[Requirement::DfuConfirmedUpdate, Requirement::DfuFailedUpdate],
        actions: &[Action::ConfirmUpdate, Action::FailUpdate],
    },
    Scenario {
        name: "platform.bond-and-card-space",
        requirements: &[Requirement::PlatformForgetBond, Requirement::PlatformCardSpaceScan],
        actions: &[Action::ForgetBond, Action::ScanCardSpace],
    },
    Scenario {
        name: "derived-data.repeats-until-matching-fill",
        requirements: &[
            Requirement::DerivedRideTrackRepeatedUntilFill,
            Requirement::DerivedNavPreviewRepeatedUntilFill,
        ],
        actions: &[
            Action::NeedRideTrack,
            Action::RemapRideIdentity,
            Action::ReplaceRideTrackNeed,
            Action::FillRideTrack,
            Action::NeedNavPreview,
            Action::ReplaceNavPreviewNeed,
            Action::NeedNavPreview,
            Action::FillNavPreview,
        ],
    },
    Scenario {
        name: "weather.refresh-install-stale-alert",
        requirements: &[
            Requirement::WeatherRefreshState,
            Requirement::WeatherInstalledDataChange,
            Requirement::WeatherStaleData,
            Requirement::WeatherAlertDelivery,
        ],
        actions: &[
            Action::RefreshWeather,
            Action::InstallWeatherData,
            Action::MarkWeatherStale,
            Action::DeliverWeatherAlert,
        ],
    },
];

/// The animation clock a delivered settings failure moves the app to, so the bounded retry window
/// has elapsed by the time the retry action runs. Used by [`CorpusState::deliver`] and by any
/// runner that owns a pass clock of its own.
pub const SETTINGS_FAILURE_RETRY_MS: u32 = 6_003;

/// The `InputClock` an action advances the app's animation clock to, or `0` for one that does not.
///
/// The legacy harness has no clock of its own — a handful of actions drive the app's directly, to
/// step past a bounded retry window. A runner that *does* own a pass clock keeps it at or above
/// these marks, so the window it just stepped past cannot reopen behind it.
pub fn clock_watermark(action: Action) -> u32 {
    match action {
        Action::RetrySettingsPersist => 4_002,
        Action::RetryExpiredDelete => 5_002,
        Action::SleepPastDeleteBackoff => 9_002,
        _ => 0,
    }
}

pub fn action_name(action: Action) -> &'static str {
    match action {
        Action::Settle => "settle",
        Action::StoreChanged => "store-changed",
        Action::RefreshCatalogs => "refresh-catalogs",
        Action::UploadFirstRoute => "upload-first-route",
        Action::UploadSecondRoute => "upload-second-route",
        Action::UploadTrip => "upload-trip",
        Action::RemapCatalogIdentity => "remap-catalog-identity",
        Action::DeleteRoute => "delete-route",
        Action::DeleteRide => "delete-ride",
        Action::CascadeDeleteTrip => "cascade-delete-trip",
        Action::RenameRouteBehindAFailingRead => "rename-route-behind-a-failing-read",
        Action::StartRoutePlan => "start-route-plan",
        Action::CancelRoutePlan => "cancel-route-plan",
        Action::DeliverLateRouteResult => "deliver-old-route-result",
        Action::ReplaceRoutePlan => "start-replacement-route-plan",
        Action::PlanDetour => "plan-detour",
        Action::CancelDetour => "cancel-detour",
        Action::CommitDetour => "commit-detour",
        Action::RouteNoPath => "route-no-path",
        Action::StartRecorder => "start-recorder",
        Action::SaveRecorder => "save-recorder",
        Action::DiscardRecorder => "discard-recorder",
        Action::FailRecorderFinalize => "fail-recorder-finalize",
        Action::ReplaceRecorderSession => "replace-recorder-session",
        Action::DirtySettings => "dirty-settings",
        Action::PersistSettings => "persist-settings-pass",
        Action::DeliverStaleSettingsResult => "deliver-stale-settings-result",
        Action::DeliverMatchingSettingsResult => "deliver-matching-settings-result",
        Action::FailSettingsPersist => "fail-settings-persist",
        Action::RetrySettingsPersist => "retry-settings-persist",
        Action::StampRouteUse => "stamp-route-use",
        Action::StampRideSync => "stamp-ride-sync",
        Action::DeleteExpiredObject => "delete-expired-object",
        Action::RetryExpiredDelete => "retry-expired-delete",
        Action::SleepPastDeleteBackoff => "sleep-past-delete-backoff",
        Action::GateExpiryUntilClockTrusted => "gate-expiry-until-clock-trusted",
        Action::ScanDfuSuccess => "scan-dfu-success",
        Action::ScanDfuFailure => "scan-dfu-failure",
        Action::StartDfuInstall => "start-dfu-install",
        Action::AdmitDfuInstall => "admit-dfu-install",
        Action::RefuseDfuInstall => "refuse-dfu-install",
        Action::ConfirmUpdate => "confirm-update",
        Action::FailUpdate => "fail-update",
        Action::ForgetBond => "forget-bond",
        Action::ScanCardSpace => "scan-card-space",
        Action::NeedRideTrack => "need-ride-track",
        Action::RemapRideIdentity => "remap-ride-identity",
        Action::ReplaceRideTrackNeed => "replace-ride-track-need",
        Action::FillRideTrack => "fill-ride-track",
        Action::NeedNavPreview => "need-nav-preview",
        Action::ReplaceNavPreviewNeed => "replace-nav-preview-need",
        Action::FillNavPreview => "fill-nav-preview",
        Action::RefreshWeather => "refresh-weather",
        Action::InstallWeatherData => "install-weather-data",
        Action::MarkWeatherStale => "mark-weather-stale",
        Action::DeliverWeatherAlert => "deliver-weather-alert",
    }
}

pub fn definition(scenario: &Scenario) -> TraceScenario<Action> {
    TraceScenario {
        name: scenario.name,
        steps: scenario
            .actions
            .iter()
            .copied()
            .map(|action| ScenarioStep::new(TraceInput::Named(action_name(action)), action))
            .collect(),
    }
}

pub fn normalization_seed() -> NormalizationSeed {
    NormalizationSeed {
        objects: vec![
            (ObjectKind::Route, 10, ObjectKey(0)),
            (ObjectKind::Route, 20, ObjectKey(1)),
            (ObjectKind::Route, 30, ObjectKey(2)),
            (ObjectKind::Ride, 70, ObjectKey(3)),
            (ObjectKind::Ride, 90, ObjectKey(4)),
            (ObjectKind::Trip, 50, ObjectKey(5)),
        ],
        revisions: vec![(1, RevisionKey(0)), (2, RevisionKey(1))],
        times: vec![(1_720_000_000, TimeKey(0)), (1_720_000_060, TimeKey(1)), (1_720_000_120, TimeKey(2))],
    }
}

pub fn step<'a>(
    trace: &'a Trace<VisibleState>,
    name: &'static str,
) -> &'a obc_host_core::trace::TraceStep<VisibleState> {
    trace
        .steps
        .iter()
        .find(|step| step.input == TraceInput::Named(name))
        .unwrap_or_else(|| panic!("{} has no {name} step", trace.scenario))
}

pub fn feeder_count(trace: &Trace<VisibleState>, kind: FeederKind) -> usize {
    trace.steps.iter().flat_map(|step| &step.feeder_calls).filter(|call| call.feeder == kind).count()
}
