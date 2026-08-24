//! The shared DeviceCore behaviour corpus (#1434 DC1, #1440 DC7).
//!
//! One scenario table, one set of fixtures and one legacy harness, used by two test binaries:
//! `device_core_legacy_traces` pins what the legacy protocol does with them, and
//! `device_core_conformance` runs the same definitions through five runners. Keeping the corpus
//! here is what makes "the same scenario, a different runner" a fact about the code rather than
//! about two hand-kept copies of a table.
//!
//! Nothing here decides policy: the harness applies real `App` operations and real gestures, and the
//! runner in `obc_host_core::trace` only controls *when* completed outcomes are delivered.

// A shared test corpus is compiled into every binary that includes it, and each uses a subset.
#![allow(dead_code)]

use obc_app::dfu::{clamp, DfuFailure, DfuInstallError, DfuScanError, DfuScanReport};
use obc_app::screen::Screen;
use obc_app::{
    App, AppState, DetourPreview, Gesture, HostCommand, HostEvent, Mode, RideRetentionRecord, RideSummary,
    RouteSummary, TrackAction, TripInput, WarningFlags,
};
use obc_formats::io::{ByteSink, SliceSource};
use obc_host_core::trace::{
    run_scenario_seeded, CommandTag, EventTag, FeederCall, FeederKind, NormalizationSeed, ObjectKey, ObjectKind,
    RevisionKey, RunnerMode, ScenarioStep, TimeKey, Trace, TraceHarness, TraceInput, TraceOutput, TraceRecorder,
    TraceScenario, TraceSink,
};
use obc_host_core::{HostLoop, PlanHold, RideRepository, RouteRepository, TripCatalog};
use obc_map_scene::BBox;
use obc_ports::{Fix, InputClock, LocationSource, RideClock, Sensors, SettingsSaveError};
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

pub const ALL_REQUIREMENTS: &[Requirement] = &[
    Requirement::CatalogStoreChange,
    Requirement::CatalogRefresh,
    Requirement::CatalogRouteDelete,
    Requirement::CatalogRideDelete,
    Requirement::CatalogTripCascade,
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
    UploadRoutesThenTrip,
    RemapCatalogIdentity,
    DeleteRoute,
    DeleteRide,
    CascadeDeleteTrip,
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
    pub rain_steps_ahead: u8,
    pub settings_revision: Option<RevisionKey>,
    pub settings_utc_offset_min: i16,
    pub pending_host_command: bool,
    pub nav_preview_missing: bool,
    pub warning: Option<WarningFlags>,
    pub retention_delete_attempts: u16,
}

#[derive(Debug)]
pub enum Outcome {
    Event(HostEvent),
    FinishSave,
    FinalizeFailed,
    CardScanned,
    RideTrack { id: u64 },
    NavPreview { generation: u16 },
    DetourCommitted,
}

#[derive(Debug, Clone, Copy)]
pub enum PendingSettingsResult {
    PersistLatest,
    FailLatest,
    PersistRevision(u16),
}

/// The legacy adapter uses the real `App` protocol doors and `HostLoop`'s passive trace observer.
/// Inputs are real public app operations or UI gestures; each pass runs the real command dispatcher;
/// deliveries call the real `set_*` feeders and `apply_event`. Planner, detour-commit, recorder-finalize,
/// and derived-fill completions are scripted at that protocol boundary because the legacy interfaces
/// do not expose a deterministic completion seam. The fixture-backed `board_parity` suite separately
/// exercises the real planner/repository path; this fast corpus must not be read as a second planner.
pub struct LegacyHarness {
    pub app: App,
    pub routes: Vec<RouteSummary>,
    pub route_ids: Vec<u64>,
    pub rides: Vec<RideSummary>,
    pub ride_ids: Vec<u64>,
    pub trip_stage_ids: Vec<u64>,
    pub trip_present: bool,
    pub nav_generation: u16,
    pub host: HostLoop,
    pub fail_next_finalize: bool,
    pub commit_success_pending: bool,
    pub pending_nav_plan: Option<Result<u64, NavError>>,
    pub pending_dfu_scan: Option<Result<DfuScanReport, DfuScanError>>,
    pub pending_dfu_install: Option<Result<(), DfuInstallError>>,
    pub pending_settings_result: Option<PendingSettingsResult>,
    pub settings_revision: u16,
    pub route_delete_fail_once: bool,
    pub retention_delete_attempts: u16,
    pub settings_retry_requested: bool,
}

impl LegacyHarness {
    pub fn new() -> Self {
        let routes = vec![route("Alpha"), route("Beta"), route("Gamma")];
        let route_ids = vec![10, 20, 30];
        let rides = vec![ride("Morning"), ride("Evening")];
        let ride_ids = vec![70, 90];
        let trip_stage_ids = vec![10, 20];
        let mut app = App::new_idle(AppState::new(8_330_000, 46_570_000, 1.0));
        app.set_routes_with_ids(&routes, &route_ids);
        app.set_rides(&rides, &ride_ids);
        app.set_trips(&[TripInput { id: 50, name: "Alps", stage_ids: &trip_stage_ids }]);
        Self {
            app,
            routes,
            route_ids,
            rides,
            ride_ids,
            trip_stage_ids,
            trip_present: true,
            nav_generation: 0,
            host: HostLoop::new(),
            fail_next_finalize: false,
            commit_success_pending: false,
            pending_nav_plan: None,
            pending_dfu_scan: None,
            pending_dfu_install: None,
            pending_settings_result: None,
            settings_revision: 0,
            route_delete_fail_once: false,
            retention_delete_attempts: 0,
            settings_retry_requested: false,
        }
    }

    pub fn event(&mut self, event: HostEvent, trace: &mut TraceRecorder<VisibleState>) {
        trace.record_event(&event);
        self.app.apply_event(event);
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
            self.app.set_trips(&[TripInput { id: 50, name: "Alps", stage_ids: &self.trip_stage_ids }]);
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

    pub fn snapshot_state(&self) -> VisibleState {
        visible_state(&self.app, self.settings_revision, self.retention_delete_attempts)
    }
}

/// The normalized rider-visible state every runner is compared on.
pub fn visible_state(app: &App, settings_revision: u16, retention_delete_attempts: u16) -> VisibleState {
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
        requested_ride_id: app.ride_track_request().map(|id| fixture_object_key(ObjectKind::Ride, id)),
        recording: app.activity.is_tracking(),
        clock_trusted: app.clock_trusted(),
        rain_steps_ahead: app.state.rain_steps_ahead,
        settings_revision: match settings_revision {
            0 => None,
            1 => Some(RevisionKey(0)),
            2 => Some(RevisionKey(1)),
            other => panic!("unseeded fixture settings revision {other}"),
        },
        settings_utc_offset_min: app.settings().utc_offset_min,
        pending_host_command: app.has_pending_host_command(),
        nav_preview_missing: app.nav_preview_missing(),
        warning: match app.top_screen() {
            Screen::Warning(card) => Some(card.flags()),
            _ => None,
        },
        retention_delete_attempts,
    }
}

impl TraceHarness<Action> for LegacyHarness {
    type State = VisibleState;
    type Outcome = Outcome;

    fn snapshot(&self) -> Self::State {
        self.snapshot_state()
    }

    fn apply_input(&mut self, action: &Action, trace: &mut TraceRecorder<Self::State>) {
        match action {
            Action::Settle => {}
            Action::StoreChanged => self.event(HostEvent::StoreChanged, trace),
            Action::RefreshCatalogs => {}
            Action::UploadRoutesThenTrip => {
                self.feed_routes("upload.routes", trace);
                self.event(HostEvent::RouteUploaded { id: 10, replaced: false, elevation: None }, trace);
                self.event(HostEvent::RouteUploaded { id: 20, replaced: false, elevation: None }, trace);
                self.feed_trips("upload.trip", trace);
                self.event(HostEvent::TripUploaded { id: 50, replaced: false }, trace);
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
                self.event(HostEvent::NavPlanned(Ok(10)), trace);
            }
            Action::ReplaceRoutePlan => {
                assert!(self.app.debug_start_nav((0, 0), (2_000, 2_000), "Replacement"));
            }
            Action::PlanDetour => self.open_detour_plan(),
            Action::CancelDetour => self.app.apply_gesture(Gesture::Back),
            Action::CommitDetour => {
                self.app.set_detour_preview(&[(0, 0), (100, 100)]);
                trace.record_feeder(FeederCall::new(FeederKind::DetourPreview, "detour.preview", 2));
                self.event(
                    HostEvent::DetourPlanned(Ok(DetourPreview {
                        cost_delta_m: 100,
                        total_distance_m: 900,
                        rejoin_m: 1_000,
                        ascent_m: Some(30),
                    })),
                    trace,
                );
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
                self.event(HostEvent::RouteUploaded { id: 10, replaced: false, elevation: None }, trace);
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
            Action::ConfirmUpdate => self.event(HostEvent::UpdateConfirmed(clamp("v2")), trace),
            Action::FailUpdate => {
                self.event(HostEvent::UpdateFailed { why: DfuFailure::Reverted, staged: Some(clamp("v3")) }, trace)
            }
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
            Action::NeedNavPreview => {
                if !self.app.nav_preview_missing() {
                    assert!(self.app.debug_start_nav((0, 0), (1_000, 1_000), "Preview"));
                    self.feed_routes("nav.preview-route", trace);
                    self.event(HostEvent::NavPlanned(Ok(10)), trace);
                    self.nav_generation = self.nav_generation.wrapping_add(1);
                }
            }
            Action::ReplaceNavPreviewNeed => {
                self.app.apply_gesture(Gesture::Back);
                assert!(self.app.debug_start_nav((0, 0), (2_000, 2_000), "New preview"));
                self.feed_routes("nav.preview-replacement", trace);
                self.event(HostEvent::NavPlanned(Ok(10)), trace);
                self.nav_generation = self.nav_generation.wrapping_add(1);
            }
            Action::FillNavPreview => {}
            Action::RefreshWeather => {
                self.app.set_rain_view(3, 0.5);
                trace.record_feeder(FeederCall::new(FeederKind::RainView, "weather.refresh", 3));
            }
            Action::InstallWeatherData => {
                self.app.weather_feed_changed();
                trace.record_feeder(FeederCall::new(FeederKind::WeatherSnapshot, "weather.installed", 1));
                trace.record_feeder(FeederCall::new(FeederKind::WeatherFeedChanged, "weather.installed", 1));
            }
            Action::MarkWeatherStale => {
                self.app.set_rain_view(0, 0.0);
                trace.record_feeder(FeederCall::new(FeederKind::RainView, "weather.stale", 0));
            }
            Action::DeliverWeatherAlert => {
                assert!(self.app.show_weather_alert(obc_app::WeatherAlertKind::Storm, 12));
            }
        }
    }

    fn run_pass(&mut self, trace: &mut TraceRecorder<Self::State>) -> Vec<Self::Outcome> {
        let ride_track_need = self.app.ride_track_request();
        let mut route_repo = BorrowedRoutes {
            catalog: &mut self.routes,
            ids: &mut self.route_ids,
            fail_delete_once: &mut self.route_delete_fail_once,
            delete_attempts: &mut self.retention_delete_attempts,
        };
        let mut ride_repo = BorrowedRides { catalog: &mut self.rides, ids: &mut self.ride_ids };
        let mut trip_repo = BorrowedTrips { present: &mut self.trip_present, stage_ids: &self.trip_stage_ids };
        let mut platform = Vec::new();
        let finish = self.host.reconcile_commands_traced(
            &mut self.app,
            &mut route_repo,
            &mut ride_repo,
            &mut trip_repo,
            PlanHold::new(true, true),
            true,
            true,
            |_app, command| platform.push(command),
            trace,
        );
        let mut outcomes = Vec::new();
        if let Some(result) = self.pending_nav_plan.take() {
            outcomes.push(Outcome::Event(HostEvent::NavPlanned(result)));
        }
        if matches!(finish, Some(TrackAction::Save)) {
            if std::mem::take(&mut self.fail_next_finalize) {
                outcomes.push(Outcome::FinalizeFailed);
            } else {
                outcomes.push(Outcome::FinishSave);
            }
        }
        if platform.iter().any(|command| matches!(command, HostCommand::ScanCardFree)) {
            outcomes.push(Outcome::CardScanned);
        }
        let persisted_revision = platform.iter().find_map(|command| match command {
            HostCommand::PersistSettings { revision } => Some(*revision),
            _ => None,
        });
        let ready_settings_result = !matches!(self.pending_settings_result, Some(PendingSettingsResult::PersistLatest))
            || persisted_revision.is_some();
        if ready_settings_result {
            if let Some(result) = self.pending_settings_result.take() {
                let revision = match result {
                    PendingSettingsResult::PersistRevision(revision) => revision,
                    PendingSettingsResult::PersistLatest | PendingSettingsResult::FailLatest => {
                        persisted_revision.unwrap_or(self.settings_revision)
                    }
                };
                outcomes.push(Outcome::Event(match result {
                    PendingSettingsResult::FailLatest => {
                        HostEvent::SettingsPersistFailed { revision, error: SettingsSaveError::Backend }
                    }
                    PendingSettingsResult::PersistLatest | PendingSettingsResult::PersistRevision(_) => {
                        HostEvent::SettingsPersisted { revision }
                    }
                }));
            }
        }
        for command in platform {
            match command {
                HostCommand::Dfu(obc_app::DfuAction::Scan) => {
                    if let Some(result) = self.pending_dfu_scan.take() {
                        outcomes.push(Outcome::Event(HostEvent::DfuScanned(result)));
                    }
                }
                HostCommand::Dfu(obc_app::DfuAction::Install) => {
                    if let Some(result) = self.pending_dfu_install.take() {
                        outcomes.push(Outcome::Event(match result {
                            Ok(()) => HostEvent::DfuInstallBegan,
                            Err(error) => HostEvent::DfuInstallFailed(error),
                        }));
                    }
                }
                _ => {}
            }
        }
        if let Some(id) = ride_track_need {
            outcomes.push(Outcome::RideTrack { id });
        }
        if self.app.nav_preview_missing() {
            outcomes.push(Outcome::NavPreview { generation: self.nav_generation });
        }
        if std::mem::take(&mut self.commit_success_pending) {
            outcomes.push(Outcome::DetourCommitted);
        }
        outcomes
    }

    fn deliver(&mut self, outcome: Self::Outcome, trace: &mut TraceRecorder<Self::State>) {
        match outcome {
            Outcome::Event(event) => {
                let settings_failed = matches!(event, HostEvent::SettingsPersistFailed { .. });
                self.event(event, trace);
                if settings_failed && self.settings_retry_requested {
                    self.app.advance_animations(InputClock(SETTINGS_FAILURE_RETRY_MS));
                }
            }
            Outcome::FinishSave => self.feed_rides("recorder.saved", trace),
            Outcome::FinalizeFailed => self.event(HostEvent::Warning(WarningFlags::REC_ERROR), trace),
            Outcome::CardScanned => self.event(HostEvent::CardScanned { free_bytes: Some(8 * 1024 * 1024) }, trace),
            Outcome::RideTrack { id } => {
                let current = self.app.ride_track_request();
                let data = ride_delivery_key(id, current);
                // Legacy bulk feeders are not request-keyed. Record and apply even stale attempts;
                // accepting this fill for a replacement view is a characterized compatibility defect.
                self.app.set_ride_profile(None);
                self.app.set_ride_preview(&[(0, 0), (1, 1)]);
                trace.record_feeder(FeederCall::new(FeederKind::RideProfile, data, 0));
                trace.record_feeder(FeederCall::new(FeederKind::RidePreview, data, 2));
            }
            Outcome::NavPreview { generation } => {
                let data = nav_delivery_key(generation, self.nav_generation);
                // The legacy preview feeder has no generation argument, so delayed old data is
                // attempted against the current request and may satisfy it incorrectly.
                self.app.set_nav_preview(&[(0, 0), (1, 1)]);
                trace.record_feeder(FeederCall::new(FeederKind::NavPreview, data, 2));
            }
            Outcome::DetourCommitted => {
                self.feed_routes("detour.commit", trace);
                self.event(HostEvent::DetourCommitted(Ok(10)), trace);
            }
        }
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
    fn member_route_ids(&self, id: u64) -> Vec<u64> {
        if *self.present && id == 50 {
            self.stage_ids.to_vec()
        } else {
            Vec::new()
        }
    }

    fn delete_by_id(&mut self, id: u64) -> bool {
        if *self.present && id == 50 {
            *self.present = false;
            true
        } else {
            false
        }
    }

    fn refeed(&self, app: &mut App) {
        if *self.present {
            app.set_trips(&[TripInput { id: 50, name: "Alps", stage_ids: self.stage_ids }]);
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
        (ObjectKind::Trip, 50) => ObjectKey(5),
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
            Action::UploadRoutesThenTrip,
            Action::RemapCatalogIdentity,
        ],
    },
    Scenario {
        name: "catalog.route-delete",
        requirements: &[Requirement::CatalogRouteDelete],
        actions: &[Action::DeleteRoute],
    },
    Scenario {
        name: "catalog.ride-delete",
        requirements: &[Requirement::CatalogRideDelete],
        actions: &[Action::DeleteRide],
    },
    Scenario {
        name: "catalog.trip-cascade",
        requirements: &[Requirement::CatalogTripCascade],
        actions: &[Action::CascadeDeleteTrip],
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
        ],
        actions: &[Action::GateExpiryUntilClockTrusted, Action::DeleteExpiredObject, Action::RetryExpiredDelete],
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
/// has elapsed by the time the retry action runs. Used by [`LegacyHarness::deliver`] and by any
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
        _ => 0,
    }
}

pub fn action_name(action: Action) -> &'static str {
    match action {
        Action::Settle => "settle",
        Action::StoreChanged => "store-changed",
        Action::RefreshCatalogs => "refresh-catalogs",
        Action::UploadRoutesThenTrip => "upload-routes-then-trip",
        Action::RemapCatalogIdentity => "remap-catalog-identity",
        Action::DeleteRoute => "delete-route",
        Action::DeleteRide => "delete-ride",
        Action::CascadeDeleteTrip => "cascade-delete-trip",
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

pub fn run_legacy(
    scenario: &TraceScenario<Action>,
    mode: RunnerMode,
    harness: &mut LegacyHarness,
) -> Trace<VisibleState> {
    run_scenario_seeded(scenario, mode, &normalization_seed(), harness)
        .unwrap_or_else(|error| panic!("{} failed in {mode:?}: {error:?}", scenario.name))
}

pub fn run_matrix(mode: RunnerMode) -> Vec<Trace<VisibleState>> {
    SCENARIOS
        .iter()
        .map(|scenario| {
            let mut harness = LegacyHarness::new();
            run_legacy(&definition(scenario), mode, &mut harness)
        })
        .collect()
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

pub fn command_count(trace: &Trace<VisibleState>, tag: CommandTag) -> usize {
    trace.steps.iter().flat_map(|step| &step.commands).filter(|command| command.tag() == tag).count()
}

pub fn event_count(trace: &Trace<VisibleState>, tag: EventTag) -> usize {
    trace.steps.iter().flat_map(|step| &step.events).filter(|event| event.tag() == tag).count()
}

pub fn feeder_count(trace: &Trace<VisibleState>, kind: FeederKind) -> usize {
    trace.steps.iter().flat_map(|step| &step.feeder_calls).filter(|call| call.feeder == kind).count()
}

pub fn output_position(
    trace: &Trace<VisibleState>,
    predicate: impl Fn(&TraceOutput) -> bool,
) -> Option<(usize, usize)> {
    trace.steps.iter().enumerate().find_map(|(step_index, step)| {
        step.timeline.iter().position(&predicate).map(|output_index| (step_index, output_index))
    })
}

pub fn command_precedes_event(trace: &Trace<VisibleState>, command: CommandTag, event: EventTag) -> bool {
    let command =
        output_position(trace, |output| matches!(output, TraceOutput::Command(value) if value.tag() == command));
    let event = output_position(trace, |output| matches!(output, TraceOutput::Event(value) if value.tag() == event));
    matches!((command, event), (Some(command), Some(event)) if command < event)
}

pub fn output_precedes(
    trace: &Trace<VisibleState>,
    before: impl Fn(&TraceOutput) -> bool,
    after: impl Fn(&TraceOutput) -> bool,
) -> bool {
    matches!((output_position(trace, before), output_position(trace, after)), (Some(before), Some(after)) if before < after)
}
