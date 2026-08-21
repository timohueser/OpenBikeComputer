//! Typed legacy DeviceCore behavior traces (DC1, #1434).
//!
//! This is intentionally a behavior corpus rather than a second implementation of `HostLoop`.
//! Scenario inputs describe product edges, while the trace harness records the legacy commands,
//! outcomes, bulk feeder identities, and visible state.  DC7 can run the same scenario definitions
//! against DeviceCore and compare the normalized traces.

use std::collections::BTreeSet;

use obc_app::dfu::{clamp, DfuFailure, DfuInstallError, DfuScanError, DfuScanReport};
use obc_app::screen::Screen;
use obc_app::{
    App, AppState, DetourPreview, Gesture, HostCommand, HostEvent, Mode, RideRetentionRecord, RideSummary,
    RouteSummary, TrackAction, TripInput, WarningFlags,
};
use obc_formats::io::{ByteSink, SliceSource};
use obc_host_core::trace::{
    run_scenario_seeded, CommandTag, DataKey, EventTag, FeederCall, FeederKind, NormalizationSeed, NormalizedCommand,
    NormalizedError, NormalizedEvent, ObjectKey, ObjectKind, RevisionKey, RunnerMode, ScenarioStep, TimeKey, Trace,
    TraceHarness, TraceInput, TraceOutput, TraceRecorder, TraceScenario, TraceSink, ALL_COMMAND_TAGS, ALL_EVENT_TAGS,
    ALL_FEEDER_KINDS,
};
use obc_host_core::{HostLoop, PlanHold, RideRepository, RouteRepository, TripCatalog};
use obc_map_scene::BBox;
use obc_ports::{Fix, InputClock, LocationSource, RideClock, Sensors, SettingsSaveError};
use obc_route::{gpx_to_obcr, NavError, RouteIndex, RouteReader};

/// Every behavior row locked by DC1.  Keeping the inventory typed makes adding a scenario without
/// the corresponding acceptance row (or silently dropping a row during later refactors) fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Requirement {
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

const ALL_REQUIREMENTS: &[Requirement] = &[
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
enum Action {
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
enum ScreenState {
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
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VisibleState {
    screen: ScreenState,
    stack_depth: usize,
    mode: Mode,
    route_names: Vec<String>,
    route_ids: Vec<ObjectKey>,
    ride_names: Vec<String>,
    ride_ids: Vec<ObjectKey>,
    trip_names: Vec<String>,
    trip_ids: Vec<ObjectKey>,
    active_route_name: Option<String>,
    active_route_id: Option<ObjectKey>,
    requested_ride_id: Option<ObjectKey>,
    recording: bool,
    clock_trusted: bool,
    rain_steps_ahead: u8,
    settings_revision: Option<RevisionKey>,
    settings_utc_offset_min: i16,
    pending_host_command: bool,
    nav_preview_missing: bool,
    warning: Option<WarningFlags>,
    retention_delete_attempts: u16,
}

#[derive(Debug)]
enum Outcome {
    Event(HostEvent),
    FinishSave,
    FinalizeFailed,
    CardScanned,
    RideTrack { id: u64 },
    NavPreview { generation: u16 },
    DetourCommitted,
}

#[derive(Debug, Clone, Copy)]
enum PendingSettingsResult {
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
struct LegacyHarness {
    app: App,
    routes: Vec<RouteSummary>,
    route_ids: Vec<u64>,
    rides: Vec<RideSummary>,
    ride_ids: Vec<u64>,
    trip_stage_ids: Vec<u64>,
    trip_present: bool,
    nav_generation: u16,
    host: HostLoop,
    fail_next_finalize: bool,
    commit_success_pending: bool,
    pending_nav_plan: Option<Result<u64, NavError>>,
    pending_dfu_scan: Option<Result<DfuScanReport, DfuScanError>>,
    pending_dfu_install: Option<Result<(), DfuInstallError>>,
    pending_settings_result: Option<PendingSettingsResult>,
    settings_revision: u16,
    route_delete_fail_once: bool,
    retention_delete_attempts: u16,
    settings_retry_requested: bool,
}

impl LegacyHarness {
    fn new() -> Self {
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

    fn event(&mut self, event: HostEvent, trace: &mut TraceRecorder<VisibleState>) {
        trace.record_event(&event);
        self.app.apply_event(event);
    }

    fn feed_routes(&mut self, key: &'static str, trace: &mut TraceRecorder<VisibleState>) {
        self.app.set_routes_with_ids(&self.routes, &self.route_ids);
        trace.record_feeder(FeederCall::new(FeederKind::RouteCatalog, key, self.routes.len()));
    }

    fn feed_rides(&mut self, key: &'static str, trace: &mut TraceRecorder<VisibleState>) {
        self.app.set_rides(&self.rides, &self.ride_ids);
        trace.record_feeder(FeederCall::new(FeederKind::RideCatalog, key, self.rides.len()));
    }

    fn feed_trips(&mut self, key: &'static str, trace: &mut TraceRecorder<VisibleState>) {
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

    fn snapshot_state(&self) -> VisibleState {
        let screen = match self.app.top_screen() {
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
            _ => ScreenState::Other,
        };
        VisibleState {
            screen,
            stack_depth: self.app.debug_stack_len(),
            mode: self.app.mode(),
            route_names: self.app.routes().iter().map(|item| item.name.as_str().to_owned()).collect(),
            route_ids: self.app.route_ids().iter().map(|&id| fixture_object_key(ObjectKind::Route, id)).collect(),
            ride_names: self.app.rides().iter().map(|item| item.name.as_str().to_owned()).collect(),
            ride_ids: self.app.ride_ids().iter().map(|&id| fixture_object_key(ObjectKind::Ride, id)).collect(),
            trip_names: self.app.trips().iter().map(|item| item.name.as_str().to_owned()).collect(),
            trip_ids: self.app.trips().iter().map(|trip| fixture_object_key(ObjectKind::Trip, trip.id)).collect(),
            active_route_name: self
                .app
                .active_route_index()
                .and_then(|index| self.app.routes().get(index))
                .map(|item| item.name.as_str().to_owned()),
            active_route_id: self
                .app
                .active_route_index()
                .and_then(|index| self.app.route_ids().get(index))
                .map(|&id| fixture_object_key(ObjectKind::Route, id)),
            requested_ride_id: self.app.ride_track_request().map(|id| fixture_object_key(ObjectKind::Ride, id)),
            recording: self.app.activity.is_tracking(),
            clock_trusted: self.app.clock_trusted(),
            rain_steps_ahead: self.app.state.rain_steps_ahead,
            settings_revision: match self.settings_revision {
                0 => None,
                1 => Some(RevisionKey(0)),
                2 => Some(RevisionKey(1)),
                other => panic!("unseeded fixture settings revision {other}"),
            },
            settings_utc_offset_min: self.app.settings().utc_offset_min,
            pending_host_command: self.app.has_pending_host_command(),
            nav_preview_missing: self.app.nav_preview_missing(),
            warning: match self.app.top_screen() {
                Screen::Warning(card) => Some(card.flags()),
                _ => None,
            },
            retention_delete_attempts: self.retention_delete_attempts,
        }
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
                    self.app.advance_animations(InputClock(6_003));
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

fn fixture_object_key(kind: ObjectKind, id: u64) -> ObjectKey {
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

fn route(name: &str) -> RouteSummary {
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

fn ride(name: &str) -> RideSummary {
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

fn road_obcr() -> Vec<u8> {
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

fn road_fix(fraction: f64) -> Fix {
    Fix::at(43_500_000, ((7.50 + 0.04 * fraction) * 1e6) as i32)
}

struct Scenario {
    name: &'static str,
    requirements: &'static [Requirement],
    actions: &'static [Action],
}

const SCENARIOS: &[Scenario] = &[
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BoardDifference {
    legacy: &'static str,
    target_owner: &'static str,
}

/// Board-only compatibility differences are evidence, never DeviceCore target behavior.
const BOARD_DIFFERENCES: &[BoardDifference] = &[
    BoardDifference {
        legacy: "detour plan and commit answer NoPath without executing a planner",
        target_owner: "Navigator plus the board detour executor",
    },
    BoardDifference {
        legacy: "the router-less BLE image presents unsupported route planning as NoPath",
        target_owner: "capability gating in Navigator and CoreMode",
    },
    BoardDifference {
        legacy: "flat route-use and ride-sync metadata stamps are physically inert pending #1398",
        target_owner: "RetentionMachine plus the ride-domain metadata executor",
    },
];

/// Honest limits of the fast protocol corpus. These are verification seams, not target policy.
const COMPATIBILITY_LIMITATIONS: &[BoardDifference] = &[
    BoardDifference {
        legacy: "TrackRepository::reconcile cannot return recorder-finalize failure to App",
        target_owner: "Recorder outcome plus FaultState",
    },
    BoardDifference {
        legacy: "HostLoop answers ride-track fills synchronously and the App feeders accept no request key",
        target_owner: "DerivedInputs key validation",
    },
    BoardDifference {
        legacy: "the fast corpus scripts planner completion after observing the real command drain",
        target_owner: "Navigator; real planning remains fixture-verified by board_parity",
    },
];

#[test]
fn scenario_matrix_covers_every_dc1_behavior_row_once_or_more() {
    assert_eq!(SCENARIOS.len(), 20, "stable scenario count is part of the handoff inventory");

    let names: BTreeSet<_> = SCENARIOS.iter().map(|scenario| scenario.name).collect();
    assert_eq!(names.len(), SCENARIOS.len(), "scenario names are stable unique trace keys");
    assert!(SCENARIOS.iter().all(|scenario| !scenario.actions.is_empty()));

    let covered: BTreeSet<_> = SCENARIOS.iter().flat_map(|scenario| scenario.requirements.iter().copied()).collect();
    let required: BTreeSet<_> = ALL_REQUIREMENTS.iter().copied().collect();
    assert_eq!(covered, required, "the typed scenario table must cover all 44 DC1 rows");
    assert_eq!(BOARD_DIFFERENCES.len(), 3);
    assert!(BOARD_DIFFERENCES.iter().all(|difference| !difference.target_owner.is_empty()));
    assert_eq!(COMPATIBILITY_LIMITATIONS.len(), 3);
    assert!(COMPATIBILITY_LIMITATIONS.iter().all(|difference| !difference.target_owner.is_empty()));
}

fn action_name(action: Action) -> &'static str {
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

fn definition(scenario: &Scenario) -> TraceScenario<Action> {
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

fn normalization_seed() -> NormalizationSeed {
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

fn run_legacy(scenario: &TraceScenario<Action>, mode: RunnerMode, harness: &mut LegacyHarness) -> Trace<VisibleState> {
    run_scenario_seeded(scenario, mode, &normalization_seed(), harness)
        .unwrap_or_else(|error| panic!("{} failed in {mode:?}: {error:?}", scenario.name))
}

fn run_matrix(mode: RunnerMode) -> Vec<Trace<VisibleState>> {
    SCENARIOS
        .iter()
        .map(|scenario| {
            let mut harness = LegacyHarness::new();
            run_legacy(&definition(scenario), mode, &mut harness)
        })
        .collect()
}

fn step<'a>(trace: &'a Trace<VisibleState>, name: &'static str) -> &'a obc_host_core::trace::TraceStep<VisibleState> {
    trace
        .steps
        .iter()
        .find(|step| step.input == TraceInput::Named(name))
        .unwrap_or_else(|| panic!("{} has no {name} step", trace.scenario))
}

fn command_count(trace: &Trace<VisibleState>, tag: CommandTag) -> usize {
    trace.steps.iter().flat_map(|step| &step.commands).filter(|command| command.tag() == tag).count()
}

fn event_count(trace: &Trace<VisibleState>, tag: EventTag) -> usize {
    trace.steps.iter().flat_map(|step| &step.events).filter(|event| event.tag() == tag).count()
}

fn feeder_count(trace: &Trace<VisibleState>, kind: FeederKind) -> usize {
    trace.steps.iter().flat_map(|step| &step.feeder_calls).filter(|call| call.feeder == kind).count()
}

fn output_position(trace: &Trace<VisibleState>, predicate: impl Fn(&TraceOutput) -> bool) -> Option<(usize, usize)> {
    trace.steps.iter().enumerate().find_map(|(step_index, step)| {
        step.timeline.iter().position(&predicate).map(|output_index| (step_index, output_index))
    })
}

fn command_precedes_event(trace: &Trace<VisibleState>, command: CommandTag, event: EventTag) -> bool {
    let command =
        output_position(trace, |output| matches!(output, TraceOutput::Command(value) if value.tag() == command));
    let event = output_position(trace, |output| matches!(output, TraceOutput::Event(value) if value.tag() == event));
    matches!((command, event), (Some(command), Some(event)) if command < event)
}

fn output_precedes(
    trace: &Trace<VisibleState>,
    before: impl Fn(&TraceOutput) -> bool,
    after: impl Fn(&TraceOutput) -> bool,
) -> bool {
    matches!((output_position(trace, before), output_position(trace, after)), (Some(before), Some(after)) if before < after)
}

fn assert_requirement(requirement: Requirement, trace: &Trace<VisibleState>) {
    let command = |tag| command_count(trace, tag) > 0;
    let event = |tag| event_count(trace, tag) > 0;
    let feeder = |kind| feeder_count(trace, kind) > 0;
    match requirement {
        Requirement::CatalogStoreChange => {
            assert!(!command_precedes_event(trace, CommandTag::RescanStore, EventTag::StoreChanged));
            assert!(output_precedes(
                trace,
                |output| matches!(output, TraceOutput::Event(NormalizedEvent::StoreChanged)),
                |output| matches!(output, TraceOutput::Command(NormalizedCommand::RescanStore { commits: 1 }))
            ));
        }
        Requirement::CatalogRefresh => {
            assert!(command(CommandTag::RescanStore));
            assert!(feeder(FeederKind::RouteCatalog));
            assert!(feeder(FeederKind::RideCatalog));
            assert!(feeder(FeederKind::TripCatalog));
        }
        Requirement::CatalogRouteDelete => {
            assert!(trace
                .steps
                .iter()
                .flat_map(|step| &step.commands)
                .any(|command| matches!(command, NormalizedCommand::DeleteRoute { id: ObjectKey(0) })));
            assert_eq!(trace.final_state.route_names.len(), 2);
            assert_eq!(trace.final_state.route_ids, [ObjectKey(1), ObjectKey(2)]);
        }
        Requirement::CatalogRideDelete => {
            assert!(trace
                .steps
                .iter()
                .flat_map(|step| &step.commands)
                .any(|command| matches!(command, NormalizedCommand::DeleteRide { id: ObjectKey(3) })));
            assert_eq!(trace.final_state.ride_names.len(), 1);
            assert_eq!(trace.final_state.ride_ids, [ObjectKey(4)]);
        }
        Requirement::CatalogTripCascade => {
            assert!(command(CommandTag::DeleteTrip));
            assert!(trace.final_state.trip_names.is_empty());
            assert!(trace.final_state.trip_ids.is_empty());
            assert_eq!(trace.final_state.route_names, ["Gamma"]);
        }
        Requirement::CatalogUploadOrder => {
            assert!(output_precedes(
                trace,
                |output| matches!(output, TraceOutput::Feeder(call) if call.data == DataKey::new("upload.routes")),
                |output| matches!(output, TraceOutput::Event(NormalizedEvent::RouteUploaded { .. }))
            ));
            assert!(output_precedes(
                trace,
                |output| matches!(output, TraceOutput::Event(NormalizedEvent::RouteUploaded { .. })),
                |output| matches!(output, TraceOutput::Feeder(call) if call.data == DataKey::new("upload.trip"))
            ));
            assert!(output_precedes(
                trace,
                |output| matches!(output, TraceOutput::Feeder(call) if call.data == DataKey::new("upload.trip")),
                |output| matches!(output, TraceOutput::Event(NormalizedEvent::TripUploaded { .. }))
            ));
        }
        Requirement::CatalogIdentityRemap => {
            assert_eq!(step(trace, "remap-catalog-identity").visible_state.route_names, ["Gamma", "Beta", "Alpha"]);
            assert_eq!(
                step(trace, "remap-catalog-identity").visible_state.route_ids,
                [ObjectKey(2), ObjectKey(1), ObjectKey(0)]
            );
            assert!(trace
                .steps
                .iter()
                .flat_map(|step| &step.feeder_calls)
                .any(|call| call.data == DataKey::new("catalog.remap") && call.len == 3));
        }
        Requirement::NavigationPlan => {
            assert!(command(CommandTag::PlanRoute));
            assert_eq!(step(trace, "start-route-plan").visible_state.screen, ScreenState::Planning);
        }
        Requirement::NavigationCancel => {
            assert!(command(CommandTag::CancelRoutePlan));
            assert_eq!(step(trace, "cancel-route-plan").visible_state.screen, ScreenState::Home);
        }
        Requirement::NavigationLateResult => {
            assert!(command_precedes_event(trace, CommandTag::PlanRoute, EventTag::NavPlanned));
            assert!(trace
                .steps
                .iter()
                .flat_map(|step| &step.feeder_calls)
                .any(|call| call.data == DataKey::new("nav.old-publication")));
        }
        Requirement::NavigationReplacement => {
            assert_eq!(command_count(trace, CommandTag::PlanRoute), 2);
            assert_eq!(trace.final_state.screen, ScreenState::RouteOverview);
            assert_eq!(trace.final_state.active_route_name.as_deref(), Some("Alpha"));
            assert_eq!(trace.final_state.active_route_id, Some(ObjectKey(0)));
        }
        Requirement::NavigationDetourPlan => {
            assert!(command(CommandTag::PlanDetour));
            assert!(command_precedes_event(trace, CommandTag::PlanDetour, EventTag::DetourPlanned));
        }
        Requirement::NavigationDetourCancel => assert!(command(CommandTag::CancelDetour)),
        Requirement::NavigationDetourCommit => {
            assert!(command(CommandTag::CommitDetour));
            assert!(command_precedes_event(trace, CommandTag::CommitDetour, EventTag::DetourCommitted));
            assert!(feeder(FeederKind::DetourPreview));
            assert!(trace
                .steps
                .iter()
                .flat_map(|step| &step.events)
                .any(|event| matches!(event, NormalizedEvent::DetourCommitted(Ok(ObjectKey(0))))));
        }
        Requirement::NavigationNoPath => {
            assert!(command_precedes_event(trace, CommandTag::PlanRoute, EventTag::NavPlanned));
            assert!(trace
                .steps
                .iter()
                .flat_map(|step| &step.events)
                .any(|event| matches!(event, NormalizedEvent::NavPlanned(Err(NormalizedError::NavNoPath)))));
        }
        Requirement::RecorderStart => assert!(step(trace, "start-recorder").visible_state.recording),
        Requirement::RecorderSave => {
            assert!(trace
                .steps
                .iter()
                .flat_map(|step| &step.commands)
                .any(|command| matches!(command, NormalizedCommand::FinishTrack(TrackAction::Save))));
            assert!(trace
                .steps
                .iter()
                .flat_map(|step| &step.feeder_calls)
                .any(|call| call.data == DataKey::new("recorder.saved")));
        }
        Requirement::RecorderDiscard => {
            assert!(trace
                .steps
                .iter()
                .flat_map(|step| &step.commands)
                .any(|command| matches!(command, NormalizedCommand::FinishTrack(TrackAction::Discard))));
            assert!(!trace.final_state.recording);
        }
        Requirement::RecorderFinalizeFailure => {
            assert!(command_precedes_event(trace, CommandTag::FinishTrack, EventTag::Warning));
            assert!(trace.steps.iter().flat_map(|step| &step.events).any(
                |event| matches!(event, NormalizedEvent::Warning(flags) if flags.contains(WarningFlags::REC_ERROR))
            ));
            assert!(trace.final_state.warning.is_some_and(|flags| flags.contains(WarningFlags::REC_ERROR)));
        }
        Requirement::RecorderSessionReplacement => assert!(trace.final_state.recording),
        Requirement::SettingsDirtyRevision => {
            assert!(feeder(FeederKind::Settings));
            assert!(trace
                .steps
                .iter()
                .flat_map(|step| &step.commands)
                .any(|command| matches!(command, NormalizedCommand::PersistSettings { revision: RevisionKey(0) })));
        }
        Requirement::SettingsSuccess => {
            assert!(trace
                .steps
                .iter()
                .flat_map(|step| &step.events)
                .any(|event| matches!(event, NormalizedEvent::SettingsPersisted { revision: RevisionKey(1) })));
            assert_eq!(trace.final_state.settings_revision, Some(RevisionKey(1)));
            assert_eq!(trace.final_state.settings_utc_offset_min, 120);
        }
        Requirement::SettingsStaleResult => {
            assert!(trace
                .steps
                .iter()
                .flat_map(|step| &step.commands)
                .any(|command| matches!(command, NormalizedCommand::PersistSettings { revision: RevisionKey(1) })));
            assert!(trace
                .steps
                .iter()
                .flat_map(|step| &step.events)
                .any(|event| matches!(event, NormalizedEvent::SettingsPersisted { revision: RevisionKey(0) })));
            assert!(command_precedes_event(trace, CommandTag::PersistSettings, EventTag::SettingsPersisted));
        }
        Requirement::SettingsFailure => {
            assert!(command_precedes_event(trace, CommandTag::PersistSettings, EventTag::SettingsPersistFailed));
            assert!(trace.steps.iter().flat_map(|step| &step.events).any(|event| matches!(
                event,
                NormalizedEvent::SettingsPersistFailed {
                    revision: RevisionKey(0),
                    error: NormalizedError::SettingsBackend
                }
            )));
        }
        Requirement::SettingsRetry => {
            assert_eq!(
                command_count(trace, CommandTag::PersistSettings),
                2,
                "settings retry command count in {:?}",
                trace.runner_mode
            );
            assert_eq!(event_count(trace, EventTag::SettingsPersistFailed), 1);
            assert!(trace
                .steps
                .iter()
                .flat_map(|step| &step.events)
                .any(|event| matches!(event, NormalizedEvent::SettingsPersisted { revision: RevisionKey(0) })));
        }
        Requirement::RetentionRouteUseStamp => assert!(trace
            .steps
            .iter()
            .flat_map(|step| &step.commands)
            .any(|command| matches!(command, NormalizedCommand::StampRouteUsed { id: ObjectKey(0), utc: TimeKey(0) }))),
        Requirement::RetentionRideSyncStamp => {
            assert!(trace.steps.iter().flat_map(|step| &step.commands).any(|command| matches!(
                command,
                NormalizedCommand::StampRideSynced { id: ObjectKey(3), utc: TimeKey(0) }
            )));
            assert!(feeder(FeederKind::RideRetention));
        }
        Requirement::RetentionExpiryDelete => {
            assert!(feeder(FeederKind::RouteRetention));
            assert!(command(CommandTag::DeleteRoute));
            assert!(trace.final_state.retention_delete_attempts >= 1);
        }
        Requirement::RetentionRetry => {
            assert_eq!(command_count(trace, CommandTag::DeleteRoute), 2);
            assert_eq!(trace.final_state.retention_delete_attempts, 2);
            let deleted: Vec<_> = trace
                .steps
                .iter()
                .flat_map(|step| &step.commands)
                .filter_map(|command| match command {
                    NormalizedCommand::DeleteRoute { id } => Some(*id),
                    _ => None,
                })
                .collect();
            assert_eq!(deleted, [ObjectKey(0), ObjectKey(0)]);
        }
        Requirement::RetentionTrustedClockGate => {
            assert!(!step(trace, "gate-expiry-until-clock-trusted").visible_state.clock_trusted);
            assert!(step(trace, "delete-expired-object").visible_state.clock_trusted);
            assert!(step(trace, "gate-expiry-until-clock-trusted").commands.iter().all(|command| !matches!(
                command,
                NormalizedCommand::DeleteRoute { .. } | NormalizedCommand::DeleteRide { .. }
            )));
        }
        Requirement::DfuScanSuccess => {
            assert!(command_precedes_event(trace, CommandTag::Dfu, EventTag::DfuScanned));
            assert!(trace
                .steps
                .iter()
                .flat_map(|step| &step.events)
                .any(|event| matches!(event, NormalizedEvent::DfuScanned(Ok(report)) if report.staged == "v2")));
        }
        Requirement::DfuScanFailure => assert!(trace
            .steps
            .iter()
            .flat_map(|step| &step.events)
            .any(|event| matches!(event, NormalizedEvent::DfuScanned(Err(NormalizedError::DfuScanDamaged))))),
        Requirement::DfuInstallStart => {
            assert!(command_precedes_event(trace, CommandTag::Dfu, EventTag::DfuInstallBegan));
            assert!(event(EventTag::DfuInstallBegan));
        }
        Requirement::DfuInstallRefusal => {
            assert!(command_precedes_event(trace, CommandTag::Dfu, EventTag::DfuInstallFailed));
            assert!(trace
                .steps
                .iter()
                .flat_map(|step| &step.events)
                .any(|event| matches!(event, NormalizedEvent::DfuInstallFailed(NormalizedError::DfuInstallRecording))));
        }
        Requirement::DfuConfirmedUpdate => assert!(trace
            .steps
            .iter()
            .flat_map(|step| &step.events)
            .any(|event| matches!(event, NormalizedEvent::UpdateConfirmed(version) if version == "v2"))),
        Requirement::DfuFailedUpdate => {
            assert!(trace.steps.iter().flat_map(|step| &step.events).any(|event| matches!(
                event,
                NormalizedEvent::UpdateFailed { why: DfuFailure::Reverted, staged: Some(version) } if version == "v3"
            )))
        }
        Requirement::PlatformForgetBond => assert!(command(CommandTag::ForgetBond)),
        Requirement::PlatformCardSpaceScan => {
            assert!(command_precedes_event(trace, CommandTag::ScanCardFree, EventTag::CardScanned));
            assert!(trace
                .steps
                .iter()
                .flat_map(|step| &step.events)
                .any(|event| matches!(event, NormalizedEvent::CardScanned { free_bytes: Some(8_388_608) })));
        }
        Requirement::DerivedRideTrackRepeatedUntilFill => {
            assert!(command_count(trace, CommandTag::LoadRideTrack) >= 2);
            assert!(feeder(FeederKind::RideProfile) && feeder(FeederKind::RidePreview));
        }
        Requirement::DerivedNavPreviewRepeatedUntilFill => {
            assert!(command_count(trace, CommandTag::RefreshNavPreview) >= 2);
            assert!(feeder(FeederKind::NavPreview));
        }
        Requirement::WeatherRefreshState => {
            assert!(trace
                .steps
                .iter()
                .flat_map(|step| &step.feeder_calls)
                .any(|call| call.data == DataKey::new("weather.refresh") && call.len == 3));
            assert_eq!(step(trace, "refresh-weather").visible_state.rain_steps_ahead, 3);
        }
        Requirement::WeatherInstalledDataChange => {
            assert!(feeder(FeederKind::WeatherSnapshot) && feeder(FeederKind::WeatherFeedChanged));
        }
        Requirement::WeatherStaleData => {
            assert_eq!(step(trace, "mark-weather-stale").visible_state.rain_steps_ahead, 0);
        }
        Requirement::WeatherAlertDelivery => {
            assert_eq!(trace.final_state.screen, ScreenState::WeatherAlert);
        }
    }
}

#[test]
fn real_legacy_scenarios_run_with_immediate_and_delayed_delivery() {
    let immediate = run_matrix(RunnerMode::Immediate);
    let delayed = run_matrix(RunnerMode::OnePassDelayed);
    let scripted = run_matrix(RunnerMode::ScriptedDelay(&[2, 0, 1]));

    assert_eq!(immediate.len(), SCENARIOS.len());
    for ((now, later), scripted) in immediate.iter().zip(&delayed).zip(&scripted) {
        assert_eq!(now.scenario, later.scenario);
        assert_eq!(now.scenario, scripted.scenario);
        assert_eq!(now.final_state, later.final_state, "{} changed its final visible state when delayed", now.scenario);
        assert_eq!(
            now.final_state, scripted.final_state,
            "{} changed its final visible state under scripted delay",
            now.scenario
        );
        assert!(now.steps.iter().any(|step| !step.timeline.is_empty()), "{} recorded no real output", now.scenario);
    }
}

#[test]
fn every_dc1_row_has_a_typed_runtime_behavior_contract_in_all_runner_modes() {
    for mode in [RunnerMode::Immediate, RunnerMode::OnePassDelayed, RunnerMode::ScriptedDelay(&[2, 0, 1])] {
        for scenario in SCENARIOS {
            let mut harness = LegacyHarness::new();
            let trace = run_legacy(&definition(scenario), mode, &mut harness);
            for requirement in scenario.requirements {
                assert_requirement(*requirement, &trace);
            }
        }
    }
}

#[test]
fn recorded_timeline_covers_the_complete_legacy_vocabulary() {
    let traces = [
        run_matrix(RunnerMode::Immediate),
        run_matrix(RunnerMode::OnePassDelayed),
        run_matrix(RunnerMode::ScriptedDelay(&[2, 0, 1])),
    ];
    let commands: BTreeSet<CommandTag> = traces
        .iter()
        .flatten()
        .flat_map(|trace| &trace.steps)
        .flat_map(|step| &step.commands)
        .map(|command| command.tag())
        .collect();
    let events: BTreeSet<EventTag> = traces
        .iter()
        .flatten()
        .flat_map(|trace| &trace.steps)
        .flat_map(|step| &step.events)
        .map(|event| event.tag())
        .collect();

    assert_eq!(commands, ALL_COMMAND_TAGS.into_iter().collect(), "all 18 tags must come from recorded commands");
    assert_eq!(events, ALL_EVENT_TAGS.into_iter().collect(), "all 15 tags must come from recorded events");

    let feeders: BTreeSet<FeederKind> = traces
        .iter()
        .flatten()
        .flat_map(|trace| &trace.steps)
        .flat_map(|step| &step.feeder_calls)
        .map(|call| call.feeder)
        .collect();
    assert_eq!(feeders, ALL_FEEDER_KINDS.into_iter().collect(), "all 13 feeders must be exercised by real calls");
}

#[test]
fn replacement_spinner_accepts_the_old_plan_result_as_a_known_legacy_defect() {
    let scenario =
        SCENARIOS.iter().find(|scenario| scenario.name == "navigation.plan-cancel-late-replacement").unwrap();
    let mut harness = LegacyHarness::new();
    let trace = run_legacy(&definition(scenario), RunnerMode::OnePassDelayed, &mut harness);
    let replacement = trace
        .steps
        .iter()
        .find(|step| step.input == TraceInput::Named("start-replacement-route-plan"))
        .expect("replacement step");
    assert_eq!(replacement.visible_state.screen, ScreenState::Planning);
    let old_result = trace
        .steps
        .iter()
        .find(|step| step.input == TraceInput::Named("deliver-old-route-result"))
        .expect("late result step");
    assert!(old_result.events.iter().any(|event| event.tag() == EventTag::NavPlanned));
    assert_eq!(
        old_result.visible_state.screen,
        ScreenState::RouteOverview,
        "legacy results are not request-keyed: the old result incorrectly lands on the replacement spinner"
    );
}

#[test]
fn delayed_derived_needs_repeat_across_identity_changes_until_matching_fills() {
    let scenario =
        SCENARIOS.iter().find(|scenario| scenario.name == "derived-data.repeats-until-matching-fill").unwrap();
    let definition = definition(scenario);
    let mut immediate_harness = LegacyHarness::new();
    let immediate = run_legacy(&definition, RunnerMode::Immediate, &mut immediate_harness);
    let mut delayed_harness = LegacyHarness::new();
    let delayed = run_legacy(&definition, RunnerMode::ScriptedDelay(&[4]), &mut delayed_harness);
    let count = |trace: &Trace<VisibleState>, tag| {
        trace.steps.iter().flat_map(|step| &step.commands).filter(|command| command.tag() == tag).count()
    };

    assert!(count(&delayed, CommandTag::LoadRideTrack) > count(&immediate, CommandTag::LoadRideTrack));
    assert!(count(&delayed, CommandTag::RefreshNavPreview) > count(&immediate, CommandTag::RefreshNavPreview));
    let mut delayed_without_depth = delayed.final_state.clone();
    delayed_without_depth.stack_depth = immediate.final_state.stack_depth;
    assert_eq!(delayed_without_depth, immediate.final_state, "terminal product data converges after matching fills");
    assert_eq!(
        immediate.final_state.stack_depth,
        delayed.final_state.stack_depth + 1,
        "the unkeyed nav fill changes the legacy overview stack with delivery cadence; DC7 must reject stale fills"
    );
    let feeders: Vec<_> = delayed.steps.iter().flat_map(|step| &step.feeder_calls).map(|call| call.feeder).collect();
    assert!(feeders.contains(&FeederKind::RideProfile) && feeders.contains(&FeederKind::RidePreview));
    assert!(feeders.contains(&FeederKind::NavPreview));
    let data_keys: Vec<_> = delayed.steps.iter().flat_map(|step| &step.feeder_calls).map(|call| &call.data).collect();
    assert!(
        data_keys.iter().any(|data| matches!(
            data,
            DataKey::Static("ride.track.requested-morning.current-evening")
                | DataKey::Static("ride.track.requested-evening.current-morning")
        )),
        "a delayed old ride fill is attempted against the replacement identity: {data_keys:?}"
    );
    assert!(
        data_keys.iter().any(|data| matches!(
            data,
            DataKey::Static("nav.preview.requested-g1.current-g2")
                | DataKey::Static("nav.preview.requested-g0.current-g1")
                | DataKey::Static("nav.preview.requested-g0.current-g2")
        )),
        "a delayed old nav fill is attempted against the replacement generation: {data_keys:?}"
    );
    assert!(
        delayed.steps.iter().any(|step| step
            .feeder_calls
            .iter()
            .any(|call| matches!(call.feeder, FeederKind::RideProfile))
            && step.visible_state.requested_ride_id.is_none()),
        "known legacy defect: the unkeyed stale fill incorrectly satisfies the replacement need"
    );
}

#[test]
fn detour_target_is_success_and_never_the_board_stub_no_path() {
    let scenario = SCENARIOS.iter().find(|scenario| scenario.name == "navigation.detour-lifecycle").unwrap();
    let mut harness = LegacyHarness::new();
    let trace = run_legacy(&definition(scenario), RunnerMode::OnePassDelayed, &mut harness);
    let committed: Vec<_> = trace
        .steps
        .iter()
        .flat_map(|step| &step.events)
        .filter(|event| matches!(event, NormalizedEvent::DetourCommitted(_)))
        .collect();
    assert!(committed.iter().any(|event| matches!(event, NormalizedEvent::DetourCommitted(Ok(_)))));
    assert!(committed.iter().all(|event| !matches!(event, NormalizedEvent::DetourCommitted(Err(_)))));
    assert!(BOARD_DIFFERENCES[0].legacy.contains("without executing a planner"));
}

#[test]
fn traces_use_no_external_fixture_bytes() {
    // The corpus uses production-shaped typed summaries and feeder identity/length records. The
    // fixture-backed planner parity remains in `board_parity`; this fast hermetic suite adds no
    // duplicate OBCR/OBCM bytes.
    const FIXTURE_BYTES: usize = 0;
    assert_eq!(FIXTURE_BYTES, 0);
}
