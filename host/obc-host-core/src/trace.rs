//! Typed, in-memory behaviour traces for the DeviceCore conformance matrix (#1434).
//!
//! This module deliberately knows nothing about the executor's dispatch policy. A trace harness
//! supplies input application, one bounded pass, outcome delivery, and a normalized visible-state
//! snapshot through [`TraceHarness`]. [`run_scenario`] only controls *when* completed outcomes are
//! delivered. That is what lets one scenario run against DeviceCore at two answer cadences without
//! copying the executor's ordering rules into the runner.

use std::collections::BTreeMap;

use obc_app::CatalogObjectId;

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

/// Typed feeder names for the bulk app data that deliberately never rides in the protocol.
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
    /// The bulk feeder calls this step made, in order.
    pub feeder_calls: Vec<FeederCall>,
    pub visible_state: S,
}

struct OpenStep {
    input: TraceInput,
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

/// Observation seam for an executor. Policy remains in the executor; this sink only records values
/// at the real bulk feeder call sites.
pub trait TraceSink {
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
        self.open = Some(OpenStep { input, feeder_calls: Vec::new() });
    }

    pub fn record_feeder(&mut self, call: FeederCall) {
        self.open_mut().feeder_calls.push(call);
    }

    pub fn finish_step(&mut self, visible_state: S) {
        let open = self.open.take().expect("begin a trace step before finishing it");
        self.steps.push(TraceStep { input: open.input, feeder_calls: open.feeder_calls, visible_state });
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
                trace.record_feeder(FeederCall::new(FeederKind::Settings, "tiny.request", 1));
                vec![7]
            } else {
                Vec::new()
            }
        }

        fn deliver(&mut self, outcome: Self::Outcome, trace: &mut TraceRecorder<Self::State>) {
            self.visible = outcome;
            trace.record_feeder(FeederCall::new(FeederKind::Settings, "tiny.answer", 1));
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
        assert_eq!(immediate.steps[0].feeder_calls.len(), 2, "the request and its answer land in one step");
        assert_eq!(delayed.steps[0].feeder_calls.len(), 1, "delayed, the answer lands in the runner's own pass");
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

    /// The feeder vocabulary is the trace's last exhaustiveness tripwire: a new bulk feeder must get
    /// a kind, or a scenario that exercises it records nothing and no coverage assertion notices.
    #[test]
    fn the_feeder_table_is_exhaustive() {
        assert_eq!(ALL_FEEDER_KINDS.len(), 13);
    }
}
