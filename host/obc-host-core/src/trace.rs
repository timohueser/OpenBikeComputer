//! Typed, in-memory behavior traces for the DeviceCore ownership move (#1434).
//!
//! This module deliberately knows nothing about the executor's dispatch policy. A trace harness
//! supplies input application, one bounded pass, outcome delivery, and a normalized visible-state
//! snapshot through [`TraceHarness`]. [`run_scenario`] only controls *when* completed outcomes are
//! delivered. That makes the same scenario usable against the legacy app/host protocol today and
//! DeviceCore later without copying either implementation's ordering rules into the runner.
//!
//! The small [`reconcile_fixture_pass`] and [`reconcile_fixture_to_completion`] helpers at the end
//! are shared setup for fixture-backed board-parity tests. They are adapters around `LegacyLoop`, not
//! part of the trace schema or runner.

use std::collections::BTreeMap;

use obc_app::{
    App, CatalogObjectId, DetourRequest, DfuAction, DfuFailure, DfuInstallError, DfuScanError, DfuScanReport,
    HostCommand, HostEvent, NavRequest, TrackAction, WarningFlags,
};
use obc_ports::SettingsSaveError;
use obc_route::NavError;

use crate::{LegacyLoop, MemRideStore, MemRouteStore, MemTrackStore, PlanHold};

/// Stable identity assigned by first observation, scoped by [`ObjectKind`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectKey(pub u16);

/// Stable revision assigned by first observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RevisionKey(pub u16);

/// Stable timestamp assigned by first observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TimeKey(pub u16);

/// The catalog namespace in which a durable object identity is meaningful.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ObjectKind {
    Route,
    Ride,
    Trip,
}

/// Normalizes values whose concrete representation is incidental to a behavior scenario.
#[derive(Debug, Default)]
pub struct Normalizer {
    objects: BTreeMap<(ObjectKind, CatalogObjectId), ObjectKey>,
    revisions: BTreeMap<u16, RevisionKey>,
    times: BTreeMap<u32, TimeKey>,
}

/// Explicit aliases for values known when a fixture is constructed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NormalizationSeed {
    pub objects: Vec<(ObjectKind, CatalogObjectId, ObjectKey)>,
    pub revisions: Vec<(u16, RevisionKey)>,
    pub times: Vec<(u32, TimeKey)>,
}

impl NormalizationSeed {
    fn apply(&self, normalizer: &mut Normalizer) {
        for &(kind, id, key) in &self.objects {
            normalizer.seed_object(kind, id, key);
        }
        for &(revision, key) in &self.revisions {
            normalizer.seed_revision(revision, key);
        }
        for &(utc, key) in &self.times {
            normalizer.seed_time(utc, key);
        }
    }
}

impl Normalizer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn object(&mut self, kind: ObjectKind, id: CatalogObjectId) -> ObjectKey {
        let next = self.next_object_key();
        *self.objects.entry((kind, id)).or_insert(next)
    }

    /// Bind a fixture identity to an explicit stable alias before the run. This prevents delivery
    /// reordering from changing aliases when outcomes under comparison mention objects in a
    /// different order.
    pub fn seed_object(&mut self, kind: ObjectKind, id: CatalogObjectId, key: ObjectKey) {
        assert!(
            self.objects.iter().all(|(raw, existing)| *raw == (kind, id) || *existing != key),
            "trace object key is already bound to a different identity"
        );
        if let Some(previous) = self.objects.insert((kind, id), key) {
            assert_eq!(previous, key, "trace identity is already bound to a different object key");
        }
    }

    pub fn revision(&mut self, revision: u16) -> RevisionKey {
        let next = self.next_revision_key();
        *self.revisions.entry(revision).or_insert(next)
    }

    pub fn seed_revision(&mut self, revision: u16, key: RevisionKey) {
        assert!(
            self.revisions.iter().all(|(raw, existing)| *raw == revision || *existing != key),
            "trace revision key is already bound to a different revision"
        );
        if let Some(previous) = self.revisions.insert(revision, key) {
            assert_eq!(previous, key, "trace revision is already bound to a different key");
        }
    }

    pub fn time(&mut self, utc: u32) -> TimeKey {
        let next = self.next_time_key();
        *self.times.entry(utc).or_insert(next)
    }

    pub fn seed_time(&mut self, utc: u32, key: TimeKey) {
        assert!(
            self.times.iter().all(|(raw, existing)| *raw == utc || *existing != key),
            "trace time key is already bound to a different timestamp"
        );
        if let Some(previous) = self.times.insert(utc, key) {
            assert_eq!(previous, key, "trace timestamp is already bound to a different key");
        }
    }

    fn next_object_key(&self) -> ObjectKey {
        (0..=u16::MAX)
            .map(ObjectKey)
            .find(|candidate| !self.objects.values().any(|key| key == candidate))
            .expect("trace object-key space exhausted")
    }

    fn next_revision_key(&self) -> RevisionKey {
        (0..=u16::MAX)
            .map(RevisionKey)
            .find(|candidate| !self.revisions.values().any(|key| key == candidate))
            .expect("trace revision-key space exhausted")
    }

    fn next_time_key(&self) -> TimeKey {
        (0..=u16::MAX)
            .map(TimeKey)
            .find(|candidate| !self.times.values().any(|key| key == candidate))
            .expect("trace time-key space exhausted")
    }

    pub fn command(&mut self, command: &HostCommand) -> NormalizedCommand {
        match command {
            HostCommand::RescanStore { commits } => NormalizedCommand::RescanStore { commits: *commits },
            HostCommand::CancelRoutePlan => NormalizedCommand::CancelRoutePlan,
            HostCommand::CancelDetour => NormalizedCommand::CancelDetour,
            HostCommand::DeleteRoute { id } => {
                NormalizedCommand::DeleteRoute { id: self.object(ObjectKind::Route, *id) }
            }
            HostCommand::DeleteTrip { id } => NormalizedCommand::DeleteTrip { id: self.object(ObjectKind::Trip, *id) },
            HostCommand::DeleteRide { id } => NormalizedCommand::DeleteRide { id: self.object(ObjectKind::Ride, *id) },
            HostCommand::StampRouteUsed { id, utc } => {
                NormalizedCommand::StampRouteUsed { id: self.object(ObjectKind::Route, *id), utc: self.time(*utc) }
            }
            HostCommand::StampRideSynced { id, utc } => {
                NormalizedCommand::StampRideSynced { id: self.object(ObjectKind::Ride, *id), utc: self.time(*utc) }
            }
            HostCommand::FinishTrack(action) => NormalizedCommand::FinishTrack(*action),
            HostCommand::PlanRoute(request) => NormalizedCommand::PlanRoute(NormalizedNavRequest::from(request)),
            HostCommand::PlanDetour(request) => NormalizedCommand::PlanDetour(NormalizedDetourRequest::from(request)),
            HostCommand::CommitDetour => NormalizedCommand::CommitDetour,
            HostCommand::Dfu(action) => NormalizedCommand::Dfu(*action),
            HostCommand::ForgetBond => NormalizedCommand::ForgetBond,
            HostCommand::PersistSettings { revision } => {
                NormalizedCommand::PersistSettings { revision: self.revision(*revision) }
            }
            HostCommand::ScanCardFree => NormalizedCommand::ScanCardFree,
            HostCommand::LoadRideTrack { id } => {
                NormalizedCommand::LoadRideTrack { id: self.object(ObjectKind::Ride, *id) }
            }
            HostCommand::RefreshNavPreview => NormalizedCommand::RefreshNavPreview,
        }
    }

    pub fn event(&mut self, event: &HostEvent) -> NormalizedEvent {
        match event {
            HostEvent::StoreChanged => NormalizedEvent::StoreChanged,
            HostEvent::RouteUploaded { id, replaced, elevation } => NormalizedEvent::RouteUploaded {
                id: self.object(ObjectKind::Route, *id),
                replaced: *replaced,
                elevation: elevation.map(|sparkline| sparkline.to_vec()),
            },
            HostEvent::TripUploaded { id, replaced } => {
                NormalizedEvent::TripUploaded { id: self.object(ObjectKind::Trip, *id), replaced: *replaced }
            }
            HostEvent::Warning(flags) => NormalizedEvent::Warning(*flags),
            HostEvent::NavPlanned(result) => NormalizedEvent::NavPlanned(
                result
                    .as_ref()
                    .map(|id| self.object(ObjectKind::Route, *id))
                    .map_err(|error| NormalizedError::from(*error)),
            ),
            HostEvent::DetourPlanned(result) => {
                NormalizedEvent::DetourPlanned(result.as_ref().copied().map_err(|error| NormalizedError::from(*error)))
            }
            HostEvent::DetourCommitted(result) => NormalizedEvent::DetourCommitted(
                result
                    .as_ref()
                    .map(|id| self.object(ObjectKind::Route, *id))
                    .map_err(|error| NormalizedError::from(*error)),
            ),
            HostEvent::CardScanned { free_bytes } => NormalizedEvent::CardScanned { free_bytes: *free_bytes },
            HostEvent::DfuScanned(result) => NormalizedEvent::DfuScanned(
                result.as_ref().map(NormalizedDfuScanReport::from).map_err(|error| NormalizedError::from(*error)),
            ),
            HostEvent::DfuInstallFailed(error) => NormalizedEvent::DfuInstallFailed((*error).into()),
            HostEvent::DfuInstallBegan => NormalizedEvent::DfuInstallBegan,
            HostEvent::UpdateConfirmed(version) => NormalizedEvent::UpdateConfirmed(version.as_str().to_owned()),
            HostEvent::UpdateFailed { why, staged } => NormalizedEvent::UpdateFailed {
                why: *why,
                staged: staged.as_ref().map(|version| version.as_str().to_owned()),
            },
            HostEvent::SettingsPersisted { revision } => {
                NormalizedEvent::SettingsPersisted { revision: self.revision(*revision) }
            }
            HostEvent::SettingsPersistFailed { revision, error } => {
                NormalizedEvent::SettingsPersistFailed { revision: self.revision(*revision), error: (*error).into() }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedNavRequest {
    pub from: (i32, i32),
    pub to: (i32, i32),
    pub name: String,
}

impl From<&NavRequest> for NormalizedNavRequest {
    fn from(value: &NavRequest) -> Self {
        Self { from: value.from, to: value.to, name: value.name().to_owned() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NormalizedDetourRequest {
    pub route_index: u16,
    pub from: (i32, i32),
    pub progress_m: u32,
    pub target_m: u32,
}

impl From<&DetourRequest> for NormalizedDetourRequest {
    fn from(value: &DetourRequest) -> Self {
        Self {
            route_index: value.route.try_into().expect("trace route index does not fit u16"),
            from: value.from,
            progress_m: value.progress_m,
            target_m: value.target_m,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedDfuScanReport {
    pub installed: String,
    pub staged: String,
    pub first_install: bool,
}

impl From<&DfuScanReport> for NormalizedDfuScanReport {
    fn from(value: &DfuScanReport) -> Self {
        Self {
            installed: value.installed.as_str().to_owned(),
            staged: value.staged.as_str().to_owned(),
            first_install: value.first_install,
        }
    }
}

/// Stable, path-free error vocabulary used by trace records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NormalizedError {
    NavNoPath,
    NavExhausted,
    DfuScanNotFound,
    DfuScanUnreadable,
    DfuScanDamaged,
    DfuScanTooLarge,
    DfuScanTooFragmented,
    DfuScanUntrusted,
    DfuInstallRecording,
    DfuInstallNoCard,
    DfuInstallScanNotFound,
    DfuInstallScanUnreadable,
    DfuInstallScanDamaged,
    DfuInstallScanTooLarge,
    DfuInstallScanTooFragmented,
    DfuInstallScanUntrusted,
    DfuInstallSnapshotFailed,
    DfuInstallStateWriteFailed,
    SettingsBackend,
}

impl From<NavError> for NormalizedError {
    fn from(value: NavError) -> Self {
        match value {
            NavError::NoPath => Self::NavNoPath,
            NavError::Exhausted => Self::NavExhausted,
        }
    }
}

impl From<DfuScanError> for NormalizedError {
    fn from(value: DfuScanError) -> Self {
        match value {
            DfuScanError::NotFound => Self::DfuScanNotFound,
            DfuScanError::Unreadable => Self::DfuScanUnreadable,
            DfuScanError::Damaged => Self::DfuScanDamaged,
            DfuScanError::TooLarge => Self::DfuScanTooLarge,
            DfuScanError::TooFragmented => Self::DfuScanTooFragmented,
            DfuScanError::Untrusted => Self::DfuScanUntrusted,
        }
    }
}

impl From<DfuInstallError> for NormalizedError {
    fn from(value: DfuInstallError) -> Self {
        match value {
            DfuInstallError::Recording => Self::DfuInstallRecording,
            DfuInstallError::NoCard => Self::DfuInstallNoCard,
            DfuInstallError::Scan(DfuScanError::NotFound) => Self::DfuInstallScanNotFound,
            DfuInstallError::Scan(DfuScanError::Unreadable) => Self::DfuInstallScanUnreadable,
            DfuInstallError::Scan(DfuScanError::Damaged) => Self::DfuInstallScanDamaged,
            DfuInstallError::Scan(DfuScanError::TooLarge) => Self::DfuInstallScanTooLarge,
            DfuInstallError::Scan(DfuScanError::TooFragmented) => Self::DfuInstallScanTooFragmented,
            DfuInstallError::Scan(DfuScanError::Untrusted) => Self::DfuInstallScanUntrusted,
            DfuInstallError::SnapshotFailed => Self::DfuInstallSnapshotFailed,
            DfuInstallError::StateWriteFailed => Self::DfuInstallStateWriteFailed,
        }
    }
}

impl From<SettingsSaveError> for NormalizedError {
    fn from(value: SettingsSaveError) -> Self {
        match value {
            SettingsSaveError::Backend => Self::SettingsBackend,
        }
    }
}

/// Exhaustive stable tags for the legacy command vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CommandTag {
    RescanStore,
    CancelRoutePlan,
    CancelDetour,
    DeleteRoute,
    DeleteTrip,
    DeleteRide,
    StampRouteUsed,
    StampRideSynced,
    FinishTrack,
    PlanRoute,
    PlanDetour,
    CommitDetour,
    Dfu,
    ForgetBond,
    PersistSettings,
    ScanCardFree,
    LoadRideTrack,
    RefreshNavPreview,
}

pub const ALL_COMMAND_TAGS: [CommandTag; 18] = [
    CommandTag::RescanStore,
    CommandTag::CancelRoutePlan,
    CommandTag::CancelDetour,
    CommandTag::DeleteRoute,
    CommandTag::DeleteTrip,
    CommandTag::DeleteRide,
    CommandTag::StampRouteUsed,
    CommandTag::StampRideSynced,
    CommandTag::FinishTrack,
    CommandTag::PlanRoute,
    CommandTag::PlanDetour,
    CommandTag::CommitDetour,
    CommandTag::Dfu,
    CommandTag::ForgetBond,
    CommandTag::PersistSettings,
    CommandTag::ScanCardFree,
    CommandTag::LoadRideTrack,
    CommandTag::RefreshNavPreview,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NormalizedCommand {
    RescanStore { commits: u32 },
    CancelRoutePlan,
    CancelDetour,
    DeleteRoute { id: ObjectKey },
    DeleteTrip { id: ObjectKey },
    DeleteRide { id: ObjectKey },
    StampRouteUsed { id: ObjectKey, utc: TimeKey },
    StampRideSynced { id: ObjectKey, utc: TimeKey },
    FinishTrack(TrackAction),
    PlanRoute(NormalizedNavRequest),
    PlanDetour(NormalizedDetourRequest),
    CommitDetour,
    Dfu(DfuAction),
    ForgetBond,
    PersistSettings { revision: RevisionKey },
    ScanCardFree,
    LoadRideTrack { id: ObjectKey },
    RefreshNavPreview,
}

impl NormalizedCommand {
    pub const fn tag(&self) -> CommandTag {
        match self {
            Self::RescanStore { .. } => CommandTag::RescanStore,
            Self::CancelRoutePlan => CommandTag::CancelRoutePlan,
            Self::CancelDetour => CommandTag::CancelDetour,
            Self::DeleteRoute { .. } => CommandTag::DeleteRoute,
            Self::DeleteTrip { .. } => CommandTag::DeleteTrip,
            Self::DeleteRide { .. } => CommandTag::DeleteRide,
            Self::StampRouteUsed { .. } => CommandTag::StampRouteUsed,
            Self::StampRideSynced { .. } => CommandTag::StampRideSynced,
            Self::FinishTrack(_) => CommandTag::FinishTrack,
            Self::PlanRoute(_) => CommandTag::PlanRoute,
            Self::PlanDetour(_) => CommandTag::PlanDetour,
            Self::CommitDetour => CommandTag::CommitDetour,
            Self::Dfu(_) => CommandTag::Dfu,
            Self::ForgetBond => CommandTag::ForgetBond,
            Self::PersistSettings { .. } => CommandTag::PersistSettings,
            Self::ScanCardFree => CommandTag::ScanCardFree,
            Self::LoadRideTrack { .. } => CommandTag::LoadRideTrack,
            Self::RefreshNavPreview => CommandTag::RefreshNavPreview,
        }
    }
}

/// Exhaustive stable tags for the legacy event vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EventTag {
    StoreChanged,
    RouteUploaded,
    TripUploaded,
    Warning,
    NavPlanned,
    DetourPlanned,
    DetourCommitted,
    CardScanned,
    DfuScanned,
    DfuInstallFailed,
    DfuInstallBegan,
    UpdateConfirmed,
    UpdateFailed,
    SettingsPersisted,
    SettingsPersistFailed,
}

pub const ALL_EVENT_TAGS: [EventTag; 15] = [
    EventTag::StoreChanged,
    EventTag::RouteUploaded,
    EventTag::TripUploaded,
    EventTag::Warning,
    EventTag::NavPlanned,
    EventTag::DetourPlanned,
    EventTag::DetourCommitted,
    EventTag::CardScanned,
    EventTag::DfuScanned,
    EventTag::DfuInstallFailed,
    EventTag::DfuInstallBegan,
    EventTag::UpdateConfirmed,
    EventTag::UpdateFailed,
    EventTag::SettingsPersisted,
    EventTag::SettingsPersistFailed,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NormalizedEvent {
    StoreChanged,
    RouteUploaded { id: ObjectKey, replaced: bool, elevation: Option<Vec<u8>> },
    TripUploaded { id: ObjectKey, replaced: bool },
    Warning(WarningFlags),
    NavPlanned(Result<ObjectKey, NormalizedError>),
    DetourPlanned(Result<obc_app::DetourPreview, NormalizedError>),
    DetourCommitted(Result<ObjectKey, NormalizedError>),
    CardScanned { free_bytes: Option<u64> },
    DfuScanned(Result<NormalizedDfuScanReport, NormalizedError>),
    DfuInstallFailed(NormalizedError),
    DfuInstallBegan,
    UpdateConfirmed(String),
    UpdateFailed { why: DfuFailure, staged: Option<String> },
    SettingsPersisted { revision: RevisionKey },
    SettingsPersistFailed { revision: RevisionKey, error: NormalizedError },
}

impl NormalizedEvent {
    pub const fn tag(&self) -> EventTag {
        match self {
            Self::StoreChanged => EventTag::StoreChanged,
            Self::RouteUploaded { .. } => EventTag::RouteUploaded,
            Self::TripUploaded { .. } => EventTag::TripUploaded,
            Self::Warning(_) => EventTag::Warning,
            Self::NavPlanned(_) => EventTag::NavPlanned,
            Self::DetourPlanned(_) => EventTag::DetourPlanned,
            Self::DetourCommitted(_) => EventTag::DetourCommitted,
            Self::CardScanned { .. } => EventTag::CardScanned,
            Self::DfuScanned(_) => EventTag::DfuScanned,
            Self::DfuInstallFailed(_) => EventTag::DfuInstallFailed,
            Self::DfuInstallBegan => EventTag::DfuInstallBegan,
            Self::UpdateConfirmed(_) => EventTag::UpdateConfirmed,
            Self::UpdateFailed { .. } => EventTag::UpdateFailed,
            Self::SettingsPersisted { .. } => EventTag::SettingsPersisted,
            Self::SettingsPersistFailed { .. } => EventTag::SettingsPersistFailed,
        }
    }
}

/// Delivery cadence for outcomes completed by a trace harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunnerMode {
    Immediate,
    OnePassDelayed,
    /// Delay of each successive outcome, in passes. The script repeats when more outcomes are
    /// produced than entries. A zero entry is immediate. An empty script is invalid.
    ScriptedDelay(&'static [u8]),
}

impl RunnerMode {
    fn delay(self, outcome_index: usize) -> Option<u8> {
        match self {
            Self::Immediate => Some(0),
            Self::OnePassDelayed => Some(1),
            Self::ScriptedDelay(script) => (!script.is_empty()).then(|| script[outcome_index % script.len()]),
        }
    }
}

/// One normalized source action in the scenario definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceInput {
    Named(&'static str),
    Gesture(obc_app::Gesture),
    Time(TimeKey),
    External {
        kind: &'static str,
        data: DataKey,
    },
    Command(NormalizedCommand),
    Event(NormalizedEvent),
    /// A runner-added pass used only to deliver delayed outcomes after the scripted inputs end.
    RunnerPass,
}

/// Stable identity of bulk fixture data. The bytes stay in their owning fixture.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DataKey {
    Static(&'static str),
    Object { scope: &'static str, kind: ObjectKind, id: ObjectKey },
    Revision { scope: &'static str, revision: RevisionKey },
}

impl DataKey {
    pub const fn new(value: &'static str) -> Self {
        Self::Static(value)
    }

    pub const fn object(scope: &'static str, kind: ObjectKind, id: ObjectKey) -> Self {
        Self::Object { scope, kind, id }
    }

    pub const fn revision(scope: &'static str, revision: RevisionKey) -> Self {
        Self::Revision { scope, revision }
    }
}

impl From<&'static str> for DataKey {
    fn from(value: &'static str) -> Self {
        Self::Static(value)
    }
}

/// Typed feeder names for bulk app data that intentionally does not ride in `HostEvent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FeederKind {
    RouteCatalog,
    TripCatalog,
    RideCatalog,
    RouteRetention,
    RideRetention,
    RideProfile,
    RidePreview,
    NavPreview,
    DetourPreview,
    Settings,
    WeatherSnapshot,
    RainView,
    WeatherFeedChanged,
}

/// Complete stable vocabulary of bulk feeder call sites.
pub const ALL_FEEDER_KINDS: [FeederKind; 13] = [
    FeederKind::RouteCatalog,
    FeederKind::TripCatalog,
    FeederKind::RideCatalog,
    FeederKind::RouteRetention,
    FeederKind::RideRetention,
    FeederKind::RideProfile,
    FeederKind::RidePreview,
    FeederKind::NavPreview,
    FeederKind::DetourPreview,
    FeederKind::Settings,
    FeederKind::WeatherSnapshot,
    FeederKind::RainView,
    FeederKind::WeatherFeedChanged,
];

/// One ordered bulk feeder call, recording only stable data identity and length.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeederCall {
    pub feeder: FeederKind,
    pub data: DataKey,
    pub len: usize,
}

impl FeederCall {
    pub fn new(feeder: FeederKind, data: impl Into<DataKey>, len: usize) -> Self {
        Self { feeder, data: data.into(), len }
    }
}

/// A complete typed behavior trace. `S` is the scenario suite's normalized visible state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trace<S> {
    pub scenario: &'static str,
    pub runner_mode: RunnerMode,
    pub initial_state: S,
    pub steps: Vec<TraceStep<S>>,
    pub final_state: S,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceStep<S> {
    pub input: TraceInput,
    /// The authoritative cross-category ordering of every output in this step.
    pub timeline: Vec<TraceOutput>,
    /// Category views kept for concise coverage assertions.
    pub commands: Vec<NormalizedCommand>,
    pub events: Vec<NormalizedEvent>,
    pub feeder_calls: Vec<FeederCall>,
    pub visible_state: S,
}

/// One item in a step's unified ordered output timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceOutput {
    Command(NormalizedCommand),
    Event(NormalizedEvent),
    Feeder(FeederCall),
}

struct OpenStep {
    input: TraceInput,
    timeline: Vec<TraceOutput>,
    commands: Vec<NormalizedCommand>,
    events: Vec<NormalizedEvent>,
    feeder_calls: Vec<FeederCall>,
}

/// Incremental recorder used by [`TraceHarness`] implementations.
pub struct TraceRecorder<S> {
    scenario: &'static str,
    runner_mode: RunnerMode,
    initial_state: S,
    steps: Vec<TraceStep<S>>,
    open: Option<OpenStep>,
    normalizer: Normalizer,
}

/// Observation seam for a legacy dispatcher adapter. Dispatch policy remains in the dispatcher;
/// this sink only records values at the real command/event/feeder call sites.
pub trait TraceSink {
    fn command(&mut self, command: &HostCommand);
    fn event(&mut self, event: &HostEvent);
    fn feeder(&mut self, call: FeederCall);

    /// Record feeder data keyed to a raw catalog identity. The recording sink normalizes it;
    /// `NoTrace` discards the raw value without constructing a key.
    fn feeder_object(
        &mut self,
        feeder: FeederKind,
        scope: &'static str,
        kind: ObjectKind,
        id: CatalogObjectId,
        len: usize,
    );

    /// Revision-keyed twin of [`TraceSink::feeder_object`].
    fn feeder_revision(&mut self, feeder: FeederKind, scope: &'static str, revision: u16, len: usize);
}

/// No-op observer used by ordinary production reconciliation.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoTrace;

impl TraceSink for NoTrace {
    fn command(&mut self, _command: &HostCommand) {}
    fn event(&mut self, _event: &HostEvent) {}
    fn feeder(&mut self, _call: FeederCall) {}
    fn feeder_object(
        &mut self,
        _feeder: FeederKind,
        _scope: &'static str,
        _kind: ObjectKind,
        _id: CatalogObjectId,
        _len: usize,
    ) {
    }
    fn feeder_revision(&mut self, _feeder: FeederKind, _scope: &'static str, _revision: u16, _len: usize) {}
}

impl<S> TraceSink for TraceRecorder<S> {
    fn command(&mut self, command: &HostCommand) {
        self.record_command(command);
    }

    fn event(&mut self, event: &HostEvent) {
        self.record_event(event);
    }

    fn feeder(&mut self, call: FeederCall) {
        self.record_feeder(call);
    }

    fn feeder_object(
        &mut self,
        feeder: FeederKind,
        scope: &'static str,
        kind: ObjectKind,
        id: CatalogObjectId,
        len: usize,
    ) {
        let id = self.normalizer.object(kind, id);
        self.record_feeder(FeederCall::new(feeder, DataKey::object(scope, kind, id), len));
    }

    fn feeder_revision(&mut self, feeder: FeederKind, scope: &'static str, revision: u16, len: usize) {
        let revision = self.normalizer.revision(revision);
        self.record_feeder(FeederCall::new(feeder, DataKey::revision(scope, revision), len));
    }
}

impl<S> TraceRecorder<S> {
    pub fn new(scenario: &'static str, runner_mode: RunnerMode, initial_state: S) -> Self {
        Self { scenario, runner_mode, initial_state, steps: Vec::new(), open: None, normalizer: Normalizer::new() }
    }

    pub fn normalizer(&mut self) -> &mut Normalizer {
        &mut self.normalizer
    }

    pub fn begin_step(&mut self, input: TraceInput) {
        assert!(self.open.is_none(), "finish the open trace step before beginning another");
        self.open = Some(OpenStep {
            input,
            timeline: Vec::new(),
            commands: Vec::new(),
            events: Vec::new(),
            feeder_calls: Vec::new(),
        });
    }

    pub fn record_command(&mut self, command: &HostCommand) {
        let normalized = self.normalizer.command(command);
        self.record_normalized_command(normalized);
    }

    pub fn record_normalized_command(&mut self, command: NormalizedCommand) {
        let open = self.open_mut();
        open.timeline.push(TraceOutput::Command(command.clone()));
        open.commands.push(command);
    }

    pub fn record_event(&mut self, event: &HostEvent) {
        let normalized = self.normalizer.event(event);
        self.record_normalized_event(normalized);
    }

    pub fn record_normalized_event(&mut self, event: NormalizedEvent) {
        let open = self.open_mut();
        open.timeline.push(TraceOutput::Event(event.clone()));
        open.events.push(event);
    }

    pub fn record_feeder(&mut self, call: FeederCall) {
        let open = self.open_mut();
        open.timeline.push(TraceOutput::Feeder(call.clone()));
        open.feeder_calls.push(call);
    }

    pub fn finish_step(&mut self, visible_state: S) {
        let open = self.open.take().expect("begin a trace step before finishing it");
        self.steps.push(TraceStep {
            input: open.input,
            timeline: open.timeline,
            commands: open.commands,
            events: open.events,
            feeder_calls: open.feeder_calls,
            visible_state,
        });
    }

    pub fn finish(self, final_state: S) -> Trace<S> {
        assert!(self.open.is_none(), "finish the open trace step before finishing the trace");
        Trace {
            scenario: self.scenario,
            runner_mode: self.runner_mode,
            initial_state: self.initial_state,
            steps: self.steps,
            final_state,
        }
    }

    fn open_mut(&mut self) -> &mut OpenStep {
        self.open.as_mut().expect("begin a trace step before recording output")
    }
}

/// One labeled input plus an executor-owned action payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScenarioStep<I> {
    pub input: TraceInput,
    pub action: I,
}

impl<I> ScenarioStep<I> {
    pub fn new(input: TraceInput, action: I) -> Self {
        Self { input, action }
    }
}

/// A scenario definition that can be run unchanged with different delivery modes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceScenario<I> {
    pub name: &'static str,
    pub steps: Vec<ScenarioStep<I>>,
}

/// Adapter implemented by the system under trace.
pub trait TraceHarness<I> {
    type State: Clone;
    type Outcome;

    fn snapshot(&self) -> Self::State;
    fn apply_input(&mut self, input: &I, trace: &mut TraceRecorder<Self::State>);
    fn run_pass(&mut self, trace: &mut TraceRecorder<Self::State>) -> Vec<Self::Outcome>;
    fn deliver(&mut self, outcome: Self::Outcome, trace: &mut TraceRecorder<Self::State>);
}

struct Delayed<T> {
    due_pass: u64,
    value: T,
}

/// Policy-free pass-delay queue, also exposed for custom runners.
pub struct DelayedQueue<T> {
    mode: RunnerMode,
    pass: u64,
    outcome_index: usize,
    queued: Vec<Delayed<T>>,
}

impl<T> DelayedQueue<T> {
    pub fn new(mode: RunnerMode) -> Result<Self, TraceRunError> {
        if mode.delay(0).is_none() {
            return Err(TraceRunError::EmptyDelayScript);
        }
        Ok(Self { mode, pass: 0, outcome_index: 0, queued: Vec::new() })
    }

    /// Queue an outcome, returning it when this mode makes it immediate.
    pub fn push(&mut self, value: T) -> Option<T> {
        let delay = self.mode.delay(self.outcome_index).expect("delay mode was validated at construction");
        self.outcome_index += 1;
        if delay == 0 {
            Some(value)
        } else {
            self.queued.push(Delayed { due_pass: self.pass + u64::from(delay), value });
            None
        }
    }

    pub fn take_due(&mut self) -> Vec<T> {
        let mut due = Vec::new();
        let mut pending = Vec::with_capacity(self.queued.len());
        for item in self.queued.drain(..) {
            if item.due_pass <= self.pass {
                due.push(item.value);
            } else {
                pending.push(item);
            }
        }
        self.queued = pending;
        due
    }

    pub fn finish_pass(&mut self) {
        self.pass += 1;
    }

    pub fn is_empty(&self) -> bool {
        self.queued.is_empty()
    }

    pub fn len(&self) -> usize {
        self.queued.len()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceRunError {
    EmptyDelayScript,
    FlushLimit { pending: usize },
}

/// Safety bound for runner-added tail passes. A scenario that needs more should make those passes
/// explicit, which keeps runaway level-triggered work visible rather than hanging the test.
pub const MAX_TRACE_FLUSH_PASSES: usize = 64;

pub fn run_scenario<I, H>(
    scenario: &TraceScenario<I>,
    mode: RunnerMode,
    harness: &mut H,
) -> Result<Trace<H::State>, TraceRunError>
where
    H: TraceHarness<I>,
{
    run_scenario_seeded(scenario, mode, &NormalizationSeed::default(), harness)
}

/// Run with explicit fixture aliases so traces remain stable even when a delivery mode changes
/// the order in which independently completed outcomes become visible.
pub fn run_scenario_seeded<I, H>(
    scenario: &TraceScenario<I>,
    mode: RunnerMode,
    seed: &NormalizationSeed,
    harness: &mut H,
) -> Result<Trace<H::State>, TraceRunError>
where
    H: TraceHarness<I>,
{
    let mut recorder = TraceRecorder::new(scenario.name, mode, harness.snapshot());
    seed.apply(recorder.normalizer());
    let mut delayed = DelayedQueue::new(mode)?;

    for step in &scenario.steps {
        recorder.begin_step(step.input.clone());
        deliver_due(harness, &mut delayed, &mut recorder);
        harness.apply_input(&step.action, &mut recorder);
        run_one_pass(harness, &mut delayed, &mut recorder);
        recorder.finish_step(harness.snapshot());
    }

    for _ in 0..MAX_TRACE_FLUSH_PASSES {
        if delayed.is_empty() {
            return Ok(recorder.finish(harness.snapshot()));
        }
        recorder.begin_step(TraceInput::RunnerPass);
        deliver_due(harness, &mut delayed, &mut recorder);
        run_one_pass(harness, &mut delayed, &mut recorder);
        recorder.finish_step(harness.snapshot());
    }

    Err(TraceRunError::FlushLimit { pending: delayed.len() })
}

fn deliver_due<I, H>(harness: &mut H, delayed: &mut DelayedQueue<H::Outcome>, recorder: &mut TraceRecorder<H::State>)
where
    H: TraceHarness<I>,
{
    for outcome in delayed.take_due() {
        harness.deliver(outcome, recorder);
    }
}

fn run_one_pass<I, H>(harness: &mut H, delayed: &mut DelayedQueue<H::Outcome>, recorder: &mut TraceRecorder<H::State>)
where
    H: TraceHarness<I>,
{
    for outcome in harness.run_pass(recorder) {
        if let Some(immediate) = delayed.push(outcome) {
            harness.deliver(immediate, recorder);
        }
    }
    delayed.finish_pass();
}

/// Run one `LegacyLoop` pass with the lightweight store setup used by board-parity fixtures.
pub fn reconcile_fixture_pass(
    host: &mut LegacyLoop,
    app: &mut App,
    routes: &mut MemRouteStore,
    map: &[u8],
) -> Result<(), obc_reader::Error> {
    with_map_reader(map, |reader| {
        let mut rides = MemRideStore::new(Vec::new());
        let mut tracks = MemTrackStore::new();
        let mut no_trips = ();
        let mut elev = obc_route::NullElevation;
        host.reconcile(app, routes, &mut rides, &mut tracks, &mut no_trips, reader, &mut elev, |_app, _cmd| {});
    })
}

/// Run the fixture-backed scripted-host shape without yielding between planner steps.
pub fn reconcile_fixture_to_completion(
    host: &mut LegacyLoop,
    app: &mut App,
    routes: &mut MemRouteStore,
    map: &[u8],
) -> Result<(), obc_reader::Error> {
    with_map_reader(map, |reader| {
        let mut rides = MemRideStore::new(Vec::new());
        let mut no_trips = ();
        let mut elev = obc_route::NullElevation;
        host.reconcile_to_completion(
            app,
            routes,
            &mut rides,
            &mut no_trips,
            reader,
            &mut elev,
            PlanHold::NONE,
            |_app, _cmd| {},
        );
    })
}

fn with_map_reader<R>(
    map: &[u8],
    use_reader: impl FnOnce(&obc_reader::Reader<'_>) -> R,
) -> Result<R, obc_reader::Error> {
    let source = obc_reader::SliceSource(map);
    let tables = obc_reader::MapTables::parse(&source)?;
    let cache = obc_reader::MapCache::new();
    let reader = obc_reader::Reader::new(&source, &tables, &cache);
    Ok(use_reader(&reader))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TinyHarness {
        requested: bool,
        visible: u8,
    }

    #[derive(Clone, Copy)]
    enum BoundaryAction {
        Request,
        Observe,
    }

    struct BoundaryHarness {
        requested: bool,
        delivered: bool,
        observed_before_action: bool,
    }

    impl TraceHarness<BoundaryAction> for BoundaryHarness {
        type State = (bool, bool);
        type Outcome = ();

        fn snapshot(&self) -> Self::State {
            (self.delivered, self.observed_before_action)
        }

        fn apply_input(&mut self, input: &BoundaryAction, _trace: &mut TraceRecorder<Self::State>) {
            match input {
                BoundaryAction::Request => self.requested = true,
                BoundaryAction::Observe => self.observed_before_action = self.delivered,
            }
        }

        fn run_pass(&mut self, _trace: &mut TraceRecorder<Self::State>) -> Vec<Self::Outcome> {
            if self.requested {
                self.requested = false;
                vec![()]
            } else {
                Vec::new()
            }
        }

        fn deliver(&mut self, (): Self::Outcome, _trace: &mut TraceRecorder<Self::State>) {
            self.delivered = true;
        }
    }

    impl TraceHarness<()> for TinyHarness {
        type State = u8;
        type Outcome = u8;

        fn snapshot(&self) -> Self::State {
            self.visible
        }

        fn apply_input(&mut self, _input: &(), _trace: &mut TraceRecorder<Self::State>) {
            self.requested = true;
        }

        fn run_pass(&mut self, trace: &mut TraceRecorder<Self::State>) -> Vec<Self::Outcome> {
            if self.requested {
                self.requested = false;
                trace.record_command(&HostCommand::ScanCardFree);
                vec![7]
            } else {
                Vec::new()
            }
        }

        fn deliver(&mut self, outcome: Self::Outcome, trace: &mut TraceRecorder<Self::State>) {
            self.visible = outcome;
            trace.record_event(&HostEvent::CardScanned { free_bytes: Some(u64::from(outcome)) });
        }
    }

    #[test]
    fn normalization_is_stable_by_namespace_and_first_observation() {
        let mut normalizer = Normalizer::new();
        normalizer.seed_object(ObjectKind::Route, 500, ObjectKey(9));
        normalizer.seed_revision(88, RevisionKey(4));
        normalizer.seed_time(1_800_000_000, TimeKey(7));
        assert_eq!(normalizer.object(ObjectKind::Route, 500), ObjectKey(9));
        assert_eq!(normalizer.revision(88), RevisionKey(4));
        assert_eq!(normalizer.time(1_800_000_000), TimeKey(7));
        assert_eq!(normalizer.object(ObjectKind::Route, 99), ObjectKey(0));
        assert_eq!(normalizer.object(ObjectKind::Route, 7), ObjectKey(1));
        assert_eq!(normalizer.object(ObjectKind::Route, 99), ObjectKey(0));
        assert_eq!(normalizer.object(ObjectKind::Ride, 99), ObjectKey(2));
        assert_eq!(normalizer.revision(42), RevisionKey(0));
        assert_eq!(normalizer.revision(42), RevisionKey(0));
        assert_eq!(normalizer.time(1_900_000_000), TimeKey(0));
    }

    #[test]
    fn object_keyed_feeders_use_the_trace_normalizer() {
        let mut recorder = TraceRecorder::new("feeder.object-key", RunnerMode::Immediate, ());
        recorder.normalizer().seed_object(ObjectKind::Ride, 700, ObjectKey(4));
        recorder.begin_step(TraceInput::Named("fill ride"));
        recorder.feeder_object(FeederKind::RidePreview, "requested-ride", ObjectKind::Ride, 700, 12);
        recorder.finish_step(());
        let trace = recorder.finish(());
        assert_eq!(
            trace.steps[0].feeder_calls[0].data,
            DataKey::object("requested-ride", ObjectKind::Ride, ObjectKey(4))
        );
    }

    #[test]
    fn delayed_queue_supports_all_runner_modes() {
        let mut immediate = DelayedQueue::new(RunnerMode::Immediate).unwrap();
        assert_eq!(immediate.push("now"), Some("now"));

        let mut delayed = DelayedQueue::new(RunnerMode::OnePassDelayed).unwrap();
        assert_eq!(delayed.push("later"), None);
        assert!(delayed.take_due().is_empty());
        delayed.finish_pass();
        assert_eq!(delayed.take_due(), vec!["later"]);

        let mut scripted = DelayedQueue::new(RunnerMode::ScriptedDelay(&[2, 0])).unwrap();
        assert_eq!(scripted.push("two"), None);
        assert_eq!(scripted.push("zero"), Some("zero"));
        scripted.finish_pass();
        assert!(scripted.take_due().is_empty());
        scripted.finish_pass();
        assert_eq!(scripted.take_due(), vec!["two"]);

        let mut reordered = DelayedQueue::new(RunnerMode::ScriptedDelay(&[2, 1])).unwrap();
        assert_eq!(reordered.push("second pass"), None);
        assert_eq!(reordered.push("first pass"), None);
        reordered.finish_pass();
        assert_eq!(reordered.take_due(), vec!["first pass"]);
        reordered.finish_pass();
        assert_eq!(reordered.take_due(), vec!["second pass"]);
        assert!(matches!(
            DelayedQueue::<()>::new(RunnerMode::ScriptedDelay(&[])),
            Err(TraceRunError::EmptyDelayScript)
        ));
    }

    #[test]
    fn one_scenario_runs_immediate_and_delayed_with_an_ordered_timeline() {
        let scenario = TraceScenario {
            name: "runner.same-definition",
            steps: vec![ScenarioStep::new(TraceInput::Named("request card scan"), ())],
        };
        let run = |mode| {
            run_scenario(&scenario, mode, &mut TinyHarness { requested: false, visible: 0 }).expect("trace runs")
        };

        let immediate = run(RunnerMode::Immediate);
        let delayed = run(RunnerMode::OnePassDelayed);
        assert_eq!(immediate.final_state, 7);
        assert_eq!(delayed.final_state, immediate.final_state);
        assert_eq!(immediate.steps.len(), 1);
        assert_eq!(delayed.steps.len(), 2);
        assert!(matches!(immediate.steps[0].timeline[0], TraceOutput::Command(_)));
        assert!(matches!(immediate.steps[0].timeline[1], TraceOutput::Event(_)));
        assert!(matches!(delayed.steps[1].input, TraceInput::RunnerPass));
    }

    #[test]
    fn delayed_outcomes_land_before_the_next_pass_action() {
        let scenario = TraceScenario {
            name: "runner.pass-boundary-order",
            steps: vec![
                ScenarioStep::new(TraceInput::Named("request"), BoundaryAction::Request),
                ScenarioStep::new(TraceInput::Named("observe"), BoundaryAction::Observe),
            ],
        };
        for mode in [RunnerMode::Immediate, RunnerMode::OnePassDelayed, RunnerMode::ScriptedDelay(&[1])] {
            let mut harness = BoundaryHarness { requested: false, delivered: false, observed_before_action: false };
            let trace = run_scenario(&scenario, mode, &mut harness).expect("trace runs");
            assert_eq!(trace.final_state, (true, true), "{mode:?} delivers before the next action");
        }
    }

    #[test]
    fn tag_tables_are_exhaustive_and_ordered() {
        assert_eq!(ALL_COMMAND_TAGS.len(), obc_app::HOST_COMMAND_CLASSES);
        assert_eq!(ALL_EVENT_TAGS.len(), 15);
        assert_eq!(ALL_FEEDER_KINDS.len(), 13);
    }
}
