//! DC7 — the DeviceCore Phase 1 conformance gate (#1440, epic #1433 §13).
//!
//! Every DC1 scenario, run through every runner, compared on what the rider can see:
//!
//! | Runner | Frame | Executor |
//! |---|---|---|
//! | `core-immediate` | [`App::run_pass`] | typed effects in, typed outcomes back, same call |
//! | `core-delayed` | the same | the same, on a scripted delay |
//! | `compatibility` | the same | [`LegacyAdapter`] — effects out as `HostCommand`s, `HostEvent`s back |
//!
//! The comparison is **rider-visible state**, not command sequences. `core-immediate` is the
//! baseline and `core-delayed` is what proves behaviour is independent of answer cadence — the
//! property most likely to regress, and the one the delayed runner exists for.
//!
//! ## Dispositions
//!
//! Every difference this file finds carries a [`Disposition::Accepted`] row with the reason and the
//! slice that removes it. A *blocking* conformance failure is not a row: it is this file failing,
//! because a difference with no approved row is one nothing may ship over.
//!
//! ## Two production defects came out of this gate
//!
//! **The rider's ride close was destroyed.** The pass took the finish one-shot at stage 4 and
//! dropped it at stage 7, where Recorder has no machine to act on it — so the ride was never
//! finalized and no executor was told. The fix was to delete the `UiRuntime` → `Recorder`
//! connection rather than document the loss: it provisioned for a lifecycle nobody owns, and its
//! only effect was destroying a rider request. The close is back on the legacy drain, where it is
//! performed, and [`a_ride_finalize_failure_after_the_last_checkpoint_reaches_the_rider`] runs the
//! mandated trace for real instead of pinning a gap.
//!
//! **A decided sidecar stamp must be mirrored into the resident view.** Retention re-derives its
//! candidates from that view — the eager ride stamp on every trusted tick — so an unmirrored stamp
//! is rediscovered and re-issued on every later pass.
//! [`a_stamp_that_was_answered_is_not_enqueued_again`] pins the ride arm.
//!
//! ## What DeviceCore owns here, and what it does not
//!
//! Seven domains have a state machine: the catalog, retention and weather from #1438, and the four
//! #1397 S2 added — Navigator, `SettingsMachine`, `DfuState` and `StorageInfo`. Six of them can be
//! reached from outside `obc-app` (weather's refresh intent has no public door until #1401 lands the
//! request cutover), so this executor serves those six and asserts the rest stays empty.
//!
//! **Two** domains still speak the legacy protocol: Recorder and Bond. Both are named in
//! [`LegacyOwned`] with the reason — a ride close is answered by a catalog re-feed rather than by a
//! ride identity, and a bond removal by a link-status fact rather than by a reply — and a domain
//! that cannot validate a token cannot own an outcome (epic §4.3). The residual drain asks for
//! exactly those classes by name, and every class still on the old protocol has a row naming the
//! slice that moves it.

mod device_core_corpus;

use std::collections::BTreeSet;

use obc_app::ble::BondEffect;
use obc_app::catalog_state::{CatalogEffect, CatalogError, CatalogOutcome};
use obc_app::device_core::compat::{event_reply, LegacyOwned, LegacyReply};
use obc_app::device_core::derived::{
    DerivedInput, DerivedInputs, DerivedNeeds, DerivedResult, DerivedTargets, NavPreviewKey, RideTrackKey,
};
use obc_app::device_core::storage_info::{StorageInfoEffect, StorageInfoOutcome};
use obc_app::device_core::{
    Capabilities, DeviceFacts, EffectSlots, LegacyAdapter, LegacyInputs, NavigatorTag, OperationToken, OutcomeSlots,
    PassClock, PassInputs, PassPlan, PlatformSupport, Revision, SettingsTag, StoreIdentity, StoreRevision, TokenSource,
    TransferState,
};
use obc_app::dfu::{DfuEffect, DfuOutcome};
use obc_app::navigator::{NavigatorEffect, NavigatorError, NavigatorOutcome, PlannerWork};
use obc_app::recorder::RecorderEffect;
use obc_app::retention::{Retention, RetentionEffect, RetentionError, RetentionOutcome, RouteRetentionMeta};
use obc_app::screen::Screen;
use obc_app::settings::{SettingsEffect, SettingsOutcome};
use obc_app::weather::WeatherEffect;
use obc_app::{
    App, AppState, DetourRequest, DfuAction, Gesture, HostCommand, HostEvent, HostMailbox, NavRequest,
    RideRetentionRecord, TrackAction, WarningFlags,
};
use obc_host_core::trace::{run_scenario_seeded, RunnerMode, Trace, TraceHarness, TraceInput, TraceRecorder};
use obc_ports::{Fix, InputClock, LocationSource, RideClock, Sensors, SettingsSaveError};
use obc_route::NavError;

use device_core_corpus::{
    clock_watermark, definition, normalization_seed, visible_state, Action, CorpusState, PendingSettingsResult,
    Scenario, VisibleState, SCENARIOS, SETTINGS_FAILURE_RETRY_MS,
};

// ==================== the runners ====================

/// One column of the conformance matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Runner {
    CoreImmediate,
    CoreDelayed,
    Compatibility,
}

impl Runner {
    const ALL: [Runner; 3] = [Runner::CoreImmediate, Runner::CoreDelayed, Runner::Compatibility];

    const fn name(self) -> &'static str {
        match self {
            Runner::CoreImmediate => "core-immediate",
            Runner::CoreDelayed => "core-delayed",
            Runner::Compatibility => "compatibility",
        }
    }

    /// When completed work is handed back. The scripted delay is deliberately uneven, so a runner
    /// that only ever sees one cadence cannot pass by accident.
    const fn mode(self) -> RunnerMode {
        match self {
            Runner::CoreImmediate | Runner::Compatibility => RunnerMode::Immediate,
            Runner::CoreDelayed => RunnerMode::ScriptedDelay(&[2, 0, 1]),
        }
    }

    /// Run one scenario, then let the runner settle, and report both.
    fn run(self, scenario: &Scenario) -> Run {
        let definition = definition(scenario);
        let executor = if self == Runner::Compatibility { Executor::Compatibility } else { Executor::Typed };
        let mut harness = CoreHarness::new(executor);
        let trace = run_scenario_seeded(&definition, self.mode(), &normalization_seed(), &mut harness);
        Run::finish(self, scenario, trace, &mut harness)
    }
}

/// One runner's result: the recorded trace, and the state it comes to rest in.
///
/// The two are different questions. A trace ends the moment the last delayed answer is delivered,
/// which for a *level* is one pass before the level can be consumed — so comparing traces at that
/// instant would compare delivery cadence rather than behaviour. The settled state is what "the same
/// device" means, and it is what the matrix compares (the same rule
/// `device_core_compat` follows: compare at rest, never mid-flight).
struct Run {
    trace: Trace<VisibleState>,
    settled: VisibleState,
}

impl Run {
    fn finish<H>(runner: Runner, scenario: &Scenario, trace: TraceResult, harness: &mut H) -> Run
    where
        H: TraceHarness<Action, State = VisibleState>,
    {
        let trace = trace.unwrap_or_else(|error| panic!("{} failed in {}: {error:?}", scenario.name, runner.name()));
        let mut recorder = recorder();
        for _ in 0..SETTLE_PASSES {
            for done in harness.run_pass(&mut recorder) {
                harness.deliver(done, &mut recorder);
            }
        }
        // At rest means at rest: a runner still producing work here would be compared mid-flight,
        // and the matrix would be reading a snapshot of a device that had not finished.
        let settled = harness.snapshot();
        assert!(
            harness.run_pass(&mut recorder).is_empty(),
            "{} has not settled in {} passes under {}",
            scenario.name,
            SETTLE_PASSES,
            runner.name()
        );
        assert_eq!(harness.snapshot(), settled, "and a quiet pass changes nothing");
        Run { settled, trace }
    }
}

type TraceResult = Result<Trace<VisibleState>, obc_host_core::trace::TraceRunError>;

/// Quiet passes after the scripted inputs end. Enough for a level to be answered and consumed, and
/// for a deferred value to reach the component behind it.
const SETTLE_PASSES: usize = 6;

/// A platform that implements everything, so no capability hides a path the matrix means to run.
const EVERYTHING: PlatformSupport = PlatformSupport {
    detour: true,
    settings_persistence: true,
    dfu: true,
    weather: true,
    bonding: true,
    storage_space_report: true,
    // The folder stores keep the retention sidecars beside their objects.
    retention_metadata: true,
};

/// The rider-visible projection two runners must agree on.
///
/// Two fields are dropped, and both for the same reason: they count *legacy* events rather than
/// anything the rider can see. `pending_host_command` asks the old protocol whether it has a command
/// queued, which a domain that no longer speaks it answers `false` to by construction.
/// `retention_delete_attempts` counts calls into `BorrowedRoutes::delete_by_id`, which counts every
/// call including one for an id already gone; the typed executor reaches the store by identity and
/// counts only the calls whose object is still catalogued, so the two count different events for the
/// same behaviour. Requiring either would be requiring the legacy command sequence.
fn rider_visible(mut state: VisibleState) -> VisibleState {
    state.pending_host_command = false;
    state.retention_delete_attempts = 0;
    state
}

// ==================== what the pass owns today ====================

/// The two derived cues, which are *levels* rather than one-shots: they are re-derived from state on
/// every drain, so they keep pending and a DeviceCore runner declines them every time — the plan's
/// keyed [`DerivedNeeds`] is what it answers instead (#1437).
fn derived_level(command: &HostCommand) -> Option<&'static str> {
    match command {
        HostCommand::LoadRideTrack { .. } => Some("LoadRideTrack"),
        HostCommand::RefreshNavPreview => Some("RefreshNavPreview"),
        _ => None,
    }
}

// ==================== the DeviceCore harness ====================

/// Which executor sits behind the pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Executor {
    /// Bounded effects in, typed token-carrying outcomes back. What #1397 S6 builds for real.
    Typed,
    /// The same effects through [`LegacyAdapter`], executed as `HostCommand`s and answered with
    /// `HostEvent`s. What a host that has not migrated yet can run today.
    Compatibility,
}

struct NoFix;
impl LocationSource for NoFix {
    fn poll(&mut self) -> Option<Fix> {
        None
    }
}

/// Which resident catalog a completed store operation changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refeed {
    None,
    Routes,
    Rides,
}

/// One thing the executor finished, on its way back into the next pass.
#[derive(Debug)]
enum Done {
    /// A typed catalog answer, plus the resident re-feed the store change implies. Bulk never
    /// enters the protocol, so the catalog arrives through the same feeders it always did.
    Catalog {
        outcome: CatalogOutcome,
        refeed: Refeed,
    },
    Retention(RetentionOutcome),
    /// A typed answer for one of the four domains #1397 S2 gave a machine.
    Navigator(NavigatorOutcome),
    Settings(SettingsOutcome),
    Dfu(DfuOutcome),
    Storage(StorageInfoOutcome),
    /// A legacy answer, for a domain whose machine has not landed.
    Event(HostEvent),
    /// The ride the recorder just finalized — answered, as the legacy protocol answers it, by a
    /// catalog re-feed rather than by a terminal ride identity (`LegacyOwned::RideCloseAck`).
    RideSaved,
    RideTrack(DerivedInput<RideTrackKey>),
    NavPreview(DerivedInput<NavPreviewKey>),
}

/// The pass, one executor, and the shared scenario fixture.
///
/// The fixture — the app, the catalogs and the scripted planner/DFU/settings answers — is
/// [`CorpusState`], unchanged: the two harnesses apply the *same* rider inputs and differ only in
/// what runs the frame. That is what makes a difference in the trace a difference in the runner.
struct CoreHarness {
    state: CorpusState,
    executor: Executor,
    adapter: LegacyAdapter,
    /// What the executor has handed back since the last pass.
    inbox: LegacyInputs,
    /// The pass's own monotonic clock. The legacy harness has none — its actions move the app's
    /// animation clock directly — so this stays at or above every mark those actions set.
    clock_ms: u32,
    /// The bounded polylines a derived answer carries beside its key.
    ride_preview: Vec<(i32, i32)>,
    nav_preview: Vec<(i32, i32)>,
    /// Legacy classes the pass took ownership of, so the run can prove it moved rather than dropped.
    moved: BTreeSet<&'static str>,
    /// Effects the executor served, by domain.
    served: BTreeSet<&'static str>,
    /// Effects the adapter could not express at all, by row.
    left: BTreeSet<LegacyOwned>,
    /// The store revision a catalog read reports. The old protocol had none
    /// (`LegacyOwned::StoreRevision`) and this fixture's repositories have none either, so the
    /// executor mints a monotonic one per read — exactly what `HostLoop` does.
    store_revision: u64,
    /// The settings write the typed executor is holding. A real asynchronous executor keeps the
    /// token of the operation it is performing; the corpus scripts some settings answers at the
    /// *action* rather than at the request, so they are built against whatever is actually running.
    settings_token: Option<OperationToken<SettingsTag>>,
}

impl CoreHarness {
    fn new(executor: Executor) -> Self {
        CoreHarness {
            state: CorpusState::new(),
            executor,
            adapter: LegacyAdapter::new(),
            inbox: LegacyInputs::new(),
            clock_ms: 0,
            ride_preview: Vec::new(),
            nav_preview: Vec::new(),
            moved: BTreeSet::new(),
            served: BTreeSet::new(),
            left: BTreeSet::new(),
            settings_token: None,
            store_revision: 0,
        }
    }

    fn app(&mut self) -> &mut App {
        &mut self.state.app
    }

    /// One DeviceCore frame: whatever the executor handed back, then fourteen stages, then a plan.
    ///
    /// The clock moves one millisecond per pass. The legacy harness has none — its actions drive the
    /// app's animation clock directly, and time otherwise stands still — so a runner that ran the
    /// clock faster would age cards and idle timers the legacy baseline never sees, and the matrix
    /// would compare elapsed time rather than behaviour. The marks the actions do set are followed
    /// exactly (see [`clock_watermark`]).
    fn pass(&mut self) -> PassPlan {
        self.clock_ms += 1;
        let mut inputs = std::mem::take(&mut self.inbox);
        let ride_preview = std::mem::take(&mut self.ride_preview);
        let nav_preview = std::mem::take(&mut self.nav_preview);
        let mut location = NoFix;
        let plan = self.state.app.run_pass(PassInputs {
            now: PassClock { ride: RideClock(self.clock_ms), ui: InputClock(self.clock_ms) },
            gestures: &[],
            sensors: Sensors::new(&mut location),
            route: None,
            support: EVERYTHING,
            outcomes: &mut inputs.outcomes,
            facts: &mut inputs.facts,
            derived: inputs.derived,
            targets: DerivedTargets { ride_preview: &ride_preview, nav_preview: &nav_preview },
        });
        // A derived answer was either accepted or was about something else; either way it is spent.
        // Outcomes and facts with no owner stay where the executor put them.
        inputs.derived = DerivedInputs::NONE;
        self.inbox = inputs;
        plan
    }

    // ---- the typed executor ----

    /// Serve what a host outside `obc-app` can actually cause.
    ///
    /// Six domains reach this executor: the catalog and retention from #1438, and the four #1397 S2
    /// gave a machine. Weather has a machine too, but its refresh intent has no public door yet
    /// (#1401 owns the request cutover), and Recorder and Bond have no machine at all — so nothing
    /// else may appear. Asserted rather than assumed: an effect this executor cannot serve turning
    /// up would be a silent change of who decides, and the run must stop rather than skip it.
    fn serve_typed(&mut self, effects: &mut EffectSlots, done: &mut Vec<Done>) {
        if let Some(effect) = effects.catalog.take() {
            self.served.insert("catalog");
            done.push(self.serve_catalog(effect));
        }
        if let Some(effect) = effects.retention.take() {
            self.served.insert("retention");
            done.push(Done::Retention(self.serve_retention(effect)));
        }
        if let Some(effect) = effects.navigator.take() {
            self.served.insert("navigator");
            if let Some(outcome) = self.serve_navigator(effect) {
                done.push(Done::Navigator(outcome));
            }
        }
        let mut persisted = None;
        if let Some(effect) = effects.settings.take() {
            self.served.insert("settings");
            let SettingsEffect::PersistRevision { token, revision } = effect;
            self.settings_token = Some(token);
            persisted = Some(revision);
        }
        if let Some(effect) = effects.dfu.take() {
            self.served.insert("dfu");
            if let Some(outcome) = self.serve_dfu(effect) {
                done.push(Done::Dfu(outcome));
            }
        }
        if let Some(effect) = effects.storage_info.take() {
            self.served.insert("storage");
            let StorageInfoEffect::MeasureFreeSpace { token } = effect;
            done.push(Done::Storage(StorageInfoOutcome::Measured { token, free_bytes: 8 * 1024 * 1024 }));
        }
        assert!(!effects.has_pending(), "recorder, weather and bond are the domains a host cannot reach in Phase 1");
        self.serve_scripted(persisted, done);
    }

    /// Serve one navigation operation from the corpus's scripted planner answers.
    ///
    /// `None` means "the executor is still working": the corpus scripts a **detour** search's answer
    /// at the action that opens the preview rather than at the request (the commit it leads to needs
    /// that preview to exist), so that one arrives through the app's own event door.
    fn serve_navigator(&mut self, effect: NavigatorEffect) -> Option<NavigatorOutcome> {
        let token = effect.token();
        match effect {
            NavigatorEffect::Acquire { work: PlannerWork::Route(_), .. } => {
                self.state.pending_nav_plan.take().map(|result| match result {
                    Ok(route) => NavigatorOutcome::PlanFinished { token, route },
                    Err(error) => NavigatorOutcome::Failed { token, error: NavigatorError::Plan(error) },
                })
            }
            NavigatorEffect::Acquire { work: PlannerWork::Detour(_), .. } => None,
            // The splice's answer is scripted at the action, like the detour search's — see
            // `serve_scripted`.
            NavigatorEffect::CommitDetour { .. } => None,
            NavigatorEffect::Release { .. } => Some(NavigatorOutcome::Released { token }),
            NavigatorEffect::Step { .. } | NavigatorEffect::CommitRoute { .. } => {
                panic!("one legacy request runs the whole search (LegacyOwned::PlannerPacing) — {effect:?}")
            }
        }
    }

    /// Serve one update operation from the corpus's scripted scan and install answers.
    fn serve_dfu(&mut self, effect: DfuEffect) -> Option<DfuOutcome> {
        match effect {
            DfuEffect::Scan { token } => self.state.pending_dfu_scan.take().map(|result| match result {
                Ok(report) => DfuOutcome::ScanFinished { token, report },
                Err(error) => DfuOutcome::ScanFailed { token, error },
            }),
            DfuEffect::ArmInstall { token } => self.state.pending_dfu_install.take().map(|result| match result {
                Ok(()) => DfuOutcome::InstallBegan { token },
                Err(error) => DfuOutcome::InstallFailed { token, error },
            }),
        }
    }

    fn serve_catalog(&mut self, effect: CatalogEffect) -> Done {
        match effect {
            CatalogEffect::ReadCatalog { token } => {
                // The re-read the store commit ordered (#1397 S6a). The fixture's catalogs are the
                // resident ones, so a refresh re-feeds exactly what it already holds and the outcome
                // reports only the revision it read at — bulk never enters the protocol.
                self.store_revision += 1;
                Done::Catalog {
                    outcome: CatalogOutcome::CatalogRead { token, revision: Revision::new(self.store_revision) },
                    refeed: Refeed::Routes,
                }
            }
            CatalogEffect::RemoveObject { token, object } => {
                if let Some(index) = self.state.route_ids.iter().position(|&id| id == object) {
                    self.state.retention_delete_attempts = self.state.retention_delete_attempts.saturating_add(1);
                    if std::mem::take(&mut self.state.route_delete_fail_once) {
                        // The store refused the removal. Not `existed: false` — the object is still
                        // there, which is what makes retention re-queue its candidate.
                        return Done::Catalog {
                            outcome: CatalogOutcome::Failed { token, error: CatalogError::RemoveFailed },
                            refeed: Refeed::None,
                        };
                    }
                    self.state.routes.remove(index);
                    self.state.route_ids.remove(index);
                    return Done::Catalog {
                        outcome: CatalogOutcome::ObjectRemoved { token, object, existed: true },
                        refeed: Refeed::Routes,
                    };
                }
                if let Some(index) = self.state.ride_ids.iter().position(|&id| id == object) {
                    self.state.rides.remove(index);
                    self.state.ride_ids.remove(index);
                    return Done::Catalog {
                        outcome: CatalogOutcome::ObjectRemoved { token, object, existed: true },
                        refeed: Refeed::Rides,
                    };
                }
                // The subject vanished before the commit — a success for the goal state, and the
                // one shape that must not read as a failure (epic §13).
                Done::Catalog {
                    outcome: CatalogOutcome::ObjectRemoved { token, object, existed: false },
                    refeed: Refeed::None,
                }
            }
            CatalogEffect::ReadTripMembers { .. } => {
                panic!("the trip cascade is refused at admission until #1397 lands the bounded member read")
            }
        }
    }

    /// The sidecar writes. The fixture keeps no durable sidecar, so the answer *is* the write —
    /// what matters here is that it carries the operation's token back, which the legacy protocol
    /// has no way to do (`LegacyOwned::SidecarAck`).
    fn serve_retention(&mut self, effect: RetentionEffect) -> RetentionOutcome {
        match effect {
            RetentionEffect::WriteRouteMetadata { token, id, .. } => {
                RetentionOutcome::RouteMetadataWritten { token, id }
            }
            RetentionEffect::WriteRideMetadata { token, id, .. } => RetentionOutcome::RideMetadataWritten { token, id },
        }
    }

    // ---- the compatibility executor ----

    /// The same effects, through the adapter and out as legacy commands.
    ///
    /// The two counts this leaves behind are the whole point of running it beside the typed
    /// executor: what the adapter *sent*, and what it could not express at all.
    fn serve_compat(
        &mut self,
        effects: &mut EffectSlots,
        needs: &DerivedNeeds,
        done: &mut Vec<Done>,
        trace: &mut TraceRecorder<VisibleState>,
    ) {
        let mut mail: HostMailbox = HostMailbox::new();
        let report = self.adapter.effects_to_commands(effects, &mut mail);
        for row in LegacyOwned::ALL {
            if report.owned.contains(row) {
                self.left.insert(row);
            }
        }
        self.adapter.needs_to_commands(needs, &mut mail);
        let mut persisted = None;
        while let Some(command) = mail.pop() {
            if let Some(level) = derived_level(&command) {
                // The adapter re-emits the two levels as their old cues; this runner answers them
                // from the plan's keys instead, which is the whole of #1437.
                self.moved.insert(level);
                continue;
            }
            if let HostCommand::PersistSettings { revision } = command {
                persisted = Some(revision);
            }
            self.served.insert("adapter");
            trace.record_command(&command);
            self.serve_legacy(command, done, trace);
        }
        self.serve_scripted(persisted, done);
    }

    // ---- the legacy half, for the two domains without a machine ----
    //
    // Recorder and Bond, and nothing else. Both are named in `LegacyOwned` with the reason they are
    // not here: the legacy ride close is answered by a catalog re-feed rather than by a ride
    // identity (`RideCloseAck`), and the legacy bond removal is confirmed by a link-status fact
    // rather than by a reply (`BondAck`). A domain that cannot validate a token cannot own an
    // outcome (epic §4.3), so both keep the old protocol until #1397 S6.

    /// Asked for **by name**: the three residual classes, and nothing else. A class DeviceCore owns
    /// is not filtered out of a full walk here — it is never drained, because the full walk *pulls*
    /// from each domain as it passes and would mint the operation the pass's own effect already
    /// carries. The harness is therefore unable to reach past its classes rather than asserting
    /// that it did not.
    fn serve_mailbox(&mut self, done: &mut Vec<Done>, trace: &mut TraceRecorder<VisibleState>) {
        let mut mail: HostMailbox = HostMailbox::new();
        let _ = self.state.app.drain_residual_commands(&mut mail);
        while let Some(command) = mail.pop() {
            trace.record_command(&command);
            self.serve_legacy(command, done, trace);
        }
    }

    /// The corpus's answers that are scripted at the **action** rather than at the request,
    /// delivered once per pass exactly as `CorpusState::run_pass` delivers them — so all three
    /// frames answer the same script.
    ///
    /// Two are not request-keyed and cannot be: the detour splice's answer is armed by the Press
    /// that asks for it and arrives without a command (like `DetourPlanned`, whose preview that
    /// Press happens on), and a settings answer may be a *stale* ack for a revision a newer edit
    /// has already superseded — the whole point of that scenario, and something no in-flight write
    /// of its own would carry.
    ///
    /// `persisted` is the revision a write went out for on this pass, when one did: the corpus's
    /// retry case is the one answer that must not precede its own request.
    fn serve_scripted(&mut self, persisted: Option<u16>, done: &mut Vec<Done>) {
        if std::mem::take(&mut self.state.commit_success_pending) {
            done.push(Done::Event(HostEvent::DetourCommitted(Ok(10))));
        }
        let Some((revision, failed)) = self.scripted_settings(persisted) else { return };
        // The typed executor answers with the token of the write it is holding, so the domain
        // checks the operation *and* the revision — two independent guards (#810). The
        // compatibility executor has no token to give: the adapter is holding it, and hands it back
        // itself when the legacy event arrives.
        let typed = matches!(self.executor, Executor::Typed).then_some(self.settings_token).flatten();
        done.push(match (typed, failed) {
            (Some(token), true) => {
                Done::Settings(SettingsOutcome::PersistFailed { token, revision, error: SettingsSaveError::Backend })
            }
            (Some(token), false) => Done::Settings(SettingsOutcome::Persisted { token, revision }),
            (None, true) => {
                Done::Event(HostEvent::SettingsPersistFailed { revision, error: SettingsSaveError::Backend })
            }
            (None, false) => Done::Event(HostEvent::SettingsPersisted { revision }),
        });
    }

    /// The corpus's scripted settings answer, if one is due this pass.
    ///
    /// `persisted` is the revision a write went out for on this pass, when one did — the corpus's
    /// retry case is the one answer that must not precede its own request.
    fn scripted_settings(&mut self, persisted: Option<u16>) -> Option<(u16, bool)> {
        let ready = !matches!(self.state.pending_settings_result, Some(PendingSettingsResult::PersistLatest))
            || persisted.is_some();
        if !ready {
            return None;
        }
        let result = self.state.pending_settings_result.take()?;
        let revision = match result {
            PendingSettingsResult::PersistRevision(revision) => revision,
            PendingSettingsResult::PersistLatest | PendingSettingsResult::FailLatest => {
                persisted.unwrap_or(self.state.settings_revision)
            }
        };
        Some((revision, matches!(result, PendingSettingsResult::FailLatest)))
    }

    fn serve_legacy(&mut self, command: HostCommand, done: &mut Vec<Done>, trace: &mut TraceRecorder<VisibleState>) {
        match command {
            HostCommand::RescanStore { .. } => {
                self.state.feed_routes("core.routes", trace);
                self.state.feed_trips("core.trips", trace);
                self.state.feed_rides("core.rides", trace);
            }
            HostCommand::DeleteTrip { id } => {
                // The legacy host runs the whole cascade inside one command
                // (`LegacyOwned::TripCascade`); the bounded member read arrives with #1397.
                if self.state.trip_present && id == 50 {
                    for member in self.state.trip_stage_ids.clone() {
                        if let Some(index) = self.state.route_ids.iter().position(|&id| id == member) {
                            self.state.routes.remove(index);
                            self.state.route_ids.remove(index);
                        }
                    }
                    self.state.trip_present = false;
                    self.state.feed_routes("core.cascade-routes", trace);
                    self.state.feed_trips("core.cascade-trips", trace);
                }
            }
            // The four domains #1397 S2 gave a machine reach this executor as *translated* effects,
            // so the corpus's scripted answers are keyed to the command that carries the request —
            // and go back through the adapter, which is holding the token.
            HostCommand::Dfu(DfuAction::Scan) => {
                if let Some(result) = self.state.pending_dfu_scan.take() {
                    done.push(Done::Event(HostEvent::DfuScanned(result)));
                }
            }
            HostCommand::Dfu(DfuAction::Install) => {
                if let Some(result) = self.state.pending_dfu_install.take() {
                    done.push(Done::Event(match result {
                        Ok(()) => HostEvent::DfuInstallBegan,
                        Err(error) => HostEvent::DfuInstallFailed(error),
                    }));
                }
            }
            HostCommand::ScanCardFree => {
                done.push(Done::Event(HostEvent::CardScanned { free_bytes: Some(8 * 1024 * 1024) }));
            }
            HostCommand::PlanRoute(_) => {
                if let Some(result) = self.state.pending_nav_plan.take() {
                    done.push(Done::Event(HostEvent::NavPlanned(result)));
                }
            }
            // The sidecar stamps are fire-and-forget in the old protocol: the write happens and
            // nothing acknowledges it (`LegacyOwned::SidecarAck`), which is exactly why
            // RetentionMachine stays in flight behind one under the compatibility executor.
            HostCommand::StampRouteUsed { .. } | HostCommand::StampRideSynced { .. } => {
                self.served.insert("legacy-stamp");
            }
            // The ride close: still a legacy command, because Recorder has no machine — and the
            // pass deliberately leaves the rider's one-shot here rather than taking it somewhere it
            // cannot be acted on.
            HostCommand::FinishTrack(TrackAction::Save) => {
                if std::mem::take(&mut self.state.fail_next_finalize) {
                    // The legacy vocabulary has no recorder-finalize outcome; a host reports the
                    // failure through the generic warning, which is DC1's own recorded limitation.
                    done.push(Done::Event(HostEvent::Warning(WarningFlags::REC_ERROR)));
                } else {
                    done.push(Done::RideSaved);
                }
            }
            HostCommand::FinishTrack(TrackAction::Discard) => {}
            // A cancellation is consumed without being started (the search it aborts was never run
            // here), the detour search's answer is scripted at the action that opens its preview,
            // and the legacy bond removal is confirmed by a link fact rather than by a reply
            // (`LegacyOwned::BondAck`).
            HostCommand::CancelRoutePlan
            | HostCommand::CancelDetour
            | HostCommand::PlanDetour(_)
            | HostCommand::CommitDetour
            | HostCommand::PersistSettings { .. }
            | HostCommand::ForgetBond => {}
            other => panic!("{other:?} is pass-owned and must not reach the legacy executor"),
        }
    }

    // ---- the two derived levels ----

    /// Answer each level with the key the need carried. Under a delayed runner the subject may have
    /// moved by the time this lands, and the pass drops it — which is the corrected defect.
    fn serve_derived(&mut self, needs: &DerivedNeeds, done: &mut Vec<Done>) {
        if let Some(key) = needs.ride_track {
            self.served.insert("derived.ride-track");
            done.push(Done::RideTrack(DerivedInput::filled(key)));
        }
        if let Some(key) = needs.nav_preview {
            self.served.insert("derived.nav-preview");
            done.push(Done::NavPreview(DerivedInput::filled(key)));
        }
    }

    /// Serve one plan and hand every answer straight back — the trace runner's frame, without the
    /// delivery scheduling.
    fn serve(&mut self, mut plan: PassPlan, trace: &mut TraceRecorder<VisibleState>) {
        let mut done = Vec::new();
        match self.executor {
            Executor::Typed => self.serve_typed(&mut plan.effects, &mut done),
            Executor::Compatibility => self.serve_compat(&mut plan.effects, &plan.derived_needs, &mut done, trace),
        }
        self.serve_mailbox(&mut done, trace);
        self.serve_derived(&plan.derived_needs, &mut done);
        for item in done {
            self.deliver(item, trace);
        }
    }

    fn refeed(&mut self, refeed: Refeed, trace: &mut TraceRecorder<VisibleState>) {
        match refeed {
            Refeed::None => {}
            Refeed::Routes => self.state.feed_routes("core.routes", trace),
            Refeed::Rides => self.state.feed_rides("core.rides", trace),
        }
    }
}

impl TraceHarness<Action> for CoreHarness {
    type State = VisibleState;
    type Outcome = Done;

    fn snapshot(&self) -> Self::State {
        visible_state(&self.state.app, self.state.settings_revision, self.state.retention_delete_attempts)
    }

    fn apply_input(&mut self, action: &Action, trace: &mut TraceRecorder<Self::State>) {
        // An action that moves the app's animation clock moves the pass's with it, so a bounded
        // retry window cannot reopen behind the action that just closed it.
        self.clock_ms = self.clock_ms.max(clock_watermark(*action));
        self.state.apply_input(action, trace);
    }

    fn run_pass(&mut self, trace: &mut TraceRecorder<Self::State>) -> Vec<Self::Outcome> {
        let mut plan = self.pass();
        let mut done = Vec::new();
        match self.executor {
            Executor::Typed => self.serve_typed(&mut plan.effects, &mut done),
            Executor::Compatibility => self.serve_compat(&mut plan.effects, &plan.derived_needs, &mut done, trace),
        }
        self.serve_mailbox(&mut done, trace);
        self.serve_derived(&plan.derived_needs, &mut done);
        done
    }

    fn deliver(&mut self, done: Self::Outcome, trace: &mut TraceRecorder<Self::State>) {
        match done {
            Done::Catalog { outcome, refeed } => {
                self.refeed(refeed, trace);
                let _ = self.inbox.outcomes.catalog.try_put(outcome);
            }
            Done::Retention(outcome) => {
                let _ = self.inbox.outcomes.retention.try_put(outcome);
            }
            Done::Navigator(outcome) => {
                let _ = self.inbox.outcomes.navigator.try_put(outcome);
            }
            Done::Settings(outcome) => {
                if matches!(outcome, SettingsOutcome::PersistFailed { .. }) && self.state.settings_retry_requested {
                    self.clock_ms = self.clock_ms.max(SETTINGS_FAILURE_RETRY_MS);
                }
                let _ = self.inbox.outcomes.settings.try_put(outcome);
            }
            Done::Dfu(outcome) => {
                let _ = self.inbox.outcomes.dfu.try_put(outcome);
            }
            Done::Storage(outcome) => {
                let _ = self.inbox.outcomes.storage_info.try_put(outcome);
            }
            Done::Event(event) => {
                trace.record_event(&event);
                let settings_failed = matches!(event, HostEvent::SettingsPersistFailed { .. });
                // The adapter answers what the *adapter* asked for. An event that answers a command
                // the App's own mailbox produced has no correlation slot — the domain that asked has
                // no machine and asked over the old protocol — so it goes back through the old door.
                // Handing it to the adapter would be an unrequested reply, which it refuses rather
                // than forging a token for.
                let correlated = matches!(self.executor, Executor::Compatibility)
                    && event_reply(&event).is_some_and(|class| self.adapter.pending().holds(class));
                if correlated {
                    self.adapter.event_to_inputs(event, &mut self.inbox).expect("the adapter asked for it");
                } else {
                    self.state.app.apply_event(event);
                }
                if settings_failed && self.state.settings_retry_requested {
                    self.clock_ms = self.clock_ms.max(SETTINGS_FAILURE_RETRY_MS);
                }
            }
            Done::RideSaved => self.state.feed_rides("core.recorder-saved", trace),
            Done::RideTrack(input) => {
                self.ride_preview = vec![(0, 0), (1, 1)];
                self.inbox.derived.ride_track = Some(input);
            }
            Done::NavPreview(input) => {
                self.nav_preview = vec![(0, 0), (1, 1)];
                self.inbox.derived.nav_preview = Some(input);
            }
        }
    }
}

// ==================== the disposition table ====================

/// What a difference between two runners means.
///
/// Only one disposition is ever *written down*: a blocking conformance failure is not a row someone
/// writes, it is [`every_scenario_agrees_or_has_an_approved_disposition`] failing on a difference
/// with no row. That is why there is no `Blocking` variant here — an unexplained difference cannot
/// be recorded and shrugged at, it fails the gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Disposition {
    /// A real difference this phase accepts, with the reason and the slice that removes it.
    Accepted,
}

/// One approved difference between the matrix's baseline runner and another.
///
/// `runners` is what makes the table exact at the level the result is reported at: a difference that
/// *moved* from one runner to another — the typed executor regressing into the compatibility
/// executor's shape, say — changes the set of `(scenario, runner)` cells even when the scenario list
/// and the cell count are unchanged.
#[derive(Debug, Clone, Copy)]
struct Difference {
    scenario: &'static str,
    /// Exactly the runners this difference appears in.
    runners: &'static [Runner],
    disposition: Disposition,
    /// The legacy row that owns the difference, when one does. Cross-checked against the rows the
    /// compatibility executor actually reports, so a citation cannot be prose that stopped being
    /// true. `None` says no legacy row owns it, and `why` then has to say what does.
    owner: Option<LegacyOwned>,
    /// What differs.
    what: &'static str,
    /// The reason and the later owner.
    why: &'static str,
}

impl Difference {
    /// The `(scenario, runner)` cells this row accounts for.
    fn cells(&self) -> impl Iterator<Item = (&'static str, &'static str)> + '_ {
        self.runners.iter().map(|runner| (self.scenario, runner.name()))
    }
}

/// Every difference this gate found, with its disposition and the cells it appears in.
const DIFFERENCES: &[Difference] = &[
    Difference {
        scenario: "catalog.route-delete",
        runners: &[Runner::Compatibility],
        disposition: Disposition::Accepted,
        owner: Some(LegacyOwned::ObjectNamespace),
        what: "the compatibility runner keeps the object",
        why: "CatalogEffect::RemoveObject is namespace-free because the flat store removes by \
              identity; the legacy deletes are namespaced, and the namespace cannot be recovered \
              from the effect. The adapter leaves it rather than guessing. #1397 S6a built the \
              typed store executor and moved every production host onto it, so this row is now the \
              cost of the *adapter* alone — the path a host runs only before its own executor \
              migrates (the board, at S6b). The typed runner removes the object, which is what the \
              production hosts now do.",
    },
    Difference {
        scenario: "catalog.ride-delete",
        runners: &[Runner::Compatibility],
        disposition: Disposition::Accepted,
        owner: Some(LegacyOwned::ObjectNamespace),
        what: "the compatibility runner keeps the object",
        why: "the same namespace-free removal, and the same row — carried by the adapter alone \
              since #1397 S6a moved the production hosts to the typed executor.",
    },
    Difference {
        scenario: "retention.expiry-retry-and-trusted-clock",
        runners: &[Runner::Compatibility],
        disposition: Disposition::Accepted,
        owner: Some(LegacyOwned::ObjectNamespace),
        what: "the compatibility runner expires nothing",
        why: "an expiry reaches the catalog as the same namespace-free removal, so it hits the same \
              row — and the catalog then stays in flight, because no legacy event can build a \
              CatalogOutcome. That is the documented one-operation cost of running the pass through \
              the adapter (#1439). #1397 S6a closed it for the simulator and the web demo by giving \
              them an executor that answers; the adapter keeps it until the board follows at S6b.",
    },
];

// ==================== the matrix ====================

/// Every applicable DC1 scenario, through every runner.
///
/// The gate: every runner reaches a rider-visible terminal state that either matches the baseline —
/// the pass answered immediately — or is named in [`DIFFERENCES`] with a disposition. Nothing may
/// differ silently.
#[test]
fn every_scenario_agrees_or_has_an_approved_disposition() {
    let mut compared = 0usize;
    let mut differing: BTreeSet<(&str, &str)> = BTreeSet::new();

    for scenario in SCENARIOS {
        let runs: Vec<_> = Runner::ALL.iter().map(|runner| (*runner, runner.run(scenario))).collect();
        let baseline = rider_visible(runs[0].1.settled.clone());

        for (runner, run) in &runs {
            assert_eq!(run.trace.scenario, scenario.name);
            assert!(!run.trace.steps.is_empty(), "{} produced no steps in {}", scenario.name, runner.name());
            compared += 1;
            if rider_visible(run.settled.clone()) != baseline {
                differing.insert((scenario.name, runner.name()));
            }
        }

        // Immediate and delayed reach the same place — the executor conformance rule of #1433 §13,
        // and the property the delayed runner exists for. A difference here would be timing
        // sensitivity, never policy.
        assert_eq!(
            rider_visible(runs[1].1.settled.clone()),
            baseline,
            "{}: DeviceCore changed terminal state under a scripted delay",
            scenario.name
        );
    }

    assert_eq!(compared, SCENARIOS.len() * Runner::ALL.len(), "every scenario runs in every runner");

    // Exact at the level the result is reported at: one approved `(scenario, runner)` cell per
    // difference. A difference that moves between runners inside a listed scenario changes this set
    // even though the scenario list and the cell count do not.
    let approved: BTreeSet<(&str, &str)> = DIFFERENCES.iter().flat_map(Difference::cells).collect();
    assert_eq!(
        differing, approved,
        "the disposition table is exact per cell: every difference is approved in the runner it \
         appears in, and no row documents one that no longer exists"
    );
}

/// Every row of the disposition table is usable, and its citation is checked rather than read.
///
/// An `owner` is not prose: for a compatibility-runner row it must be a row the compatibility
/// executor **actually reports leaving** on that scenario, so a citation that stopped being true
/// fails here. A row with no owner has to say what owns the difference instead.
#[test]
fn every_difference_carries_a_verified_disposition() {
    assert!(!DIFFERENCES.is_empty());
    let names: BTreeSet<&str> = SCENARIOS.iter().map(|scenario| scenario.name).collect();

    for row in DIFFERENCES {
        assert!(!row.what.is_empty() && !row.why.is_empty(), "{row:?}");
        assert!(names.contains(row.scenario), "{} is not a scenario", row.scenario);
        assert!(!row.runners.is_empty(), "a difference appears in at least one runner: {row:?}");
        assert!(row.why.contains('#'), "every disposition names the slice that owns its target: {row:?}");

        match (row.disposition, row.owner) {
            (Disposition::Accepted, Some(owner)) => {
                assert!(owner.deletes_in().starts_with('#'), "{owner:?} must name its deletion slice");
                if row.runners.contains(&Runner::Compatibility) {
                    let left = compatibility_rows_left(named(row.scenario));
                    assert!(
                        left.contains(&owner),
                        "{}: cites {owner:?}, but the compatibility executor reported {left:?}",
                        row.scenario
                    );
                }
            }
            (Disposition::Accepted, None) => assert!(
                row.why.contains("no legacy row owns"),
                "an accepted difference with no owning row has to say what owns it instead: {row:?}"
            ),
        }
    }
}

/// The `LegacyOwned` rows the compatibility executor reports leaving on one scenario — the evidence
/// an `owner` citation is checked against.
fn compatibility_rows_left(scenario: &Scenario) -> BTreeSet<LegacyOwned> {
    let mut harness = CoreHarness::new(Executor::Compatibility);
    run_scenario_seeded(&definition(scenario), RunnerMode::Immediate, &normalization_seed(), &mut harness)
        .expect("the scenario runs");
    harness.left
}

/// Every `LegacyOwned` row has a later owner and a deletion slice — the epic's Phase 1 gate on the
/// inventory itself, checked from outside `obc-app` so the table is a seam rather than an internal.
#[test]
fn every_legacy_owned_row_names_the_slice_that_deletes_it() {
    for row in LegacyOwned::ALL {
        let owner = row.deletes_in();
        assert!(owner.starts_with('#'), "{row:?} must name an issue");
        assert!(owner.contains('—'), "{row:?} must say what takes it over");
    }
}

/// The classes the pass took over really did move: a DeviceCore run observes them in the mailbox,
/// declines to execute them, and serves the same request as a typed effect or a keyed level.
#[test]
fn the_pass_owns_the_classes_it_took_over() {
    // A rider's delete: the pass takes the request at stage 4, so the legacy class never pends at
    // all, and the removal happens as one bounded catalog operation.
    let mut harness = CoreHarness::new(Executor::Typed);
    let trace = run_scenario_seeded(
        &definition(named("catalog.route-delete")),
        RunnerMode::Immediate,
        &normalization_seed(),
        &mut harness,
    )
    .expect("the scenario runs");
    assert_eq!(trace.final_state.route_ids.len(), 2, "the rider's delete removed one route");
    assert!(harness.served.contains("catalog"), "and it was the catalog effect that did it");
    // `serve_mailbox` never asks for the class, so the removal could only have come from the effect.

    // The retention stamps: the same, one layer down — the sweep's candidate leaves as a
    // `RetentionEffect`, so the legacy stamp class is not pending either.
    let mut harness = CoreHarness::new(Executor::Typed);
    run_scenario_seeded(
        &definition(named("retention.route-and-ride-stamps")),
        RunnerMode::Immediate,
        &normalization_seed(),
        &mut harness,
    )
    .expect("the scenario runs");
    assert!(harness.served.contains("retention"), "the stamps left as retention effects");
    assert!(
        !harness.moved.contains("StampRouteUsed") && !harness.moved.contains("StampRideSynced"),
        "and no legacy stamp was left pending beside them"
    );

    // An auto-expiry reaches the catalog by the same path a rider's delete does, and the legacy
    // delete class does not come back beside it.
    let mut harness = CoreHarness::new(Executor::Typed);
    run_scenario_seeded(
        &definition(named("retention.expiry-retry-and-trusted-clock")),
        RunnerMode::Immediate,
        &normalization_seed(),
        &mut harness,
    )
    .expect("the scenario runs");
    assert!(harness.served.contains("catalog"), "the expiry was served as one bounded removal");

    // The two derived fills are keyed needs on the plan, not commands: the pass raises them and the
    // executor answers the key it was handed.
    let mut harness = CoreHarness::new(Executor::Typed);
    run_scenario_seeded(
        &definition(named("derived-data.repeats-until-matching-fill")),
        RunnerMode::Immediate,
        &normalization_seed(),
        &mut harness,
    )
    .expect("the scenario runs");
    assert!(harness.served.contains("derived.ride-track"), "the ride-track fill is answered from the plan's key");
    assert!(harness.served.contains("derived.nav-preview"), "and so is the nav preview");

    // The eight classes #1397 S2 moved, each through the scenario that produces it: the rider's
    // request leaves as its domain's typed effect, and the residual drain — which every one of
    // these runs through, and which asks for three classes by name — is what makes the legacy class
    // unable to pend beside it. `Cancel*` rides its own family's scenario, and `PersistSettings`
    // the settings ones.
    for (scenario, domain) in [
        ("navigation.plan-cancel-late-replacement", "navigator"),
        ("navigation.detour-lifecycle", "navigator"),
        ("dfu.scan-outcomes", "dfu"),
        ("dfu.install-start", "dfu"),
        ("platform.bond-and-card-space", "storage"),
        ("settings.revision-success-and-stale-result", "settings"),
    ] {
        let mut harness = CoreHarness::new(Executor::Typed);
        run_scenario_seeded(&definition(named(scenario)), RunnerMode::Immediate, &normalization_seed(), &mut harness)
            .expect("the scenario runs");
        assert!(harness.served.contains(domain), "{scenario}: the rider's request left as a {domain} effect");
    }
}

fn named(name: &'static str) -> &'static Scenario {
    SCENARIOS.iter().find(|scenario| scenario.name == name).unwrap_or_else(|| panic!("no scenario {name}"))
}

/// What the compatibility executor cannot do, measured rather than asserted from the doc comment.
///
/// The adapter can build a navigator, settings, DFU or storage outcome, because those four legacy
/// commands have a terminal event. It can build no catalog, retention or weather outcome, because
/// the old protocol answers those with a bulk re-feed, a store-changed edge, or nothing at all — so
/// a domain that latches in flight when it emits stays latched. Two of the accepted rows above are
/// this fact reaching the rider.
#[test]
fn the_compatibility_executor_leaves_what_the_old_protocol_cannot_say() {
    let mut harness = CoreHarness::new(Executor::Compatibility);
    run_scenario_seeded(
        &definition(named("retention.expiry-retry-and-trusted-clock")),
        RunnerMode::Immediate,
        &normalization_seed(),
        &mut harness,
    )
    .expect("the scenario runs");
    assert!(
        harness.left.contains(&LegacyOwned::ObjectNamespace),
        "the removal has no legacy expression, and is left with its owner rather than dropped"
    );
    assert_eq!(harness.state.route_ids.len(), 3, "so nothing was removed, and nothing pretended it had been");

    // The same run under the typed executor completes it — the difference is the executor, and
    // nothing else about the device.
    let mut typed = CoreHarness::new(Executor::Typed);
    run_scenario_seeded(
        &definition(named("retention.expiry-retry-and-trusted-clock")),
        RunnerMode::Immediate,
        &normalization_seed(),
        &mut typed,
    )
    .expect("the scenario runs");
    assert_eq!(typed.state.route_ids.len(), 2, "the expired object is gone");
}

// ==================== the mandatory traces (#1440) ====================

/// One of the sixteen traces #1440 requires, bound to the test that runs it.
struct MandatoryTrace {
    /// The row, in the issue's words.
    row: &'static str,
    /// The `#[test]` that runs it.
    test: &'static str,
    /// Set when the test exercises a *different* situation than the row names, saying why the row's
    /// own situation is unreachable in Phase 1 and which slice makes it reachable. A row that
    /// silently tested something else would let the issue's checklist read as covered when it is not.
    substitution: Option<&'static str>,
}

/// All sixteen. The binding is checked rather than written down: the test below looks each name up
/// in this file's own source as a real `#[test]`, so a renamed, deleted or un-attributed trace fails
/// the gate instead of quietly leaving a row of the issue uncovered.
const MANDATORY_TRACES: [MandatoryTrace; 16] = [
    MandatoryTrace {
        row: "outcome after cancellation",
        test: "an_outcome_after_cancellation_changes_nothing",
        substitution: None,
    },
    MandatoryTrace {
        row: "outcome after a replacement request",
        test: "an_outcome_after_a_replacement_request_changes_nothing",
        substitution: None,
    },
    MandatoryTrace {
        row: "store change during catalog refresh",
        test: "a_store_change_during_a_catalog_operation_is_not_lost",
        substitution: None,
    },
    MandatoryTrace {
        row: "transfer start during route planning",
        test: "a_transfer_during_planning_withdraws_heavy_capability",
        substitution: None,
    },
    MandatoryTrace {
        row: "route-plan completion after active-route change",
        test: "a_route_plan_that_lands_after_the_active_route_changed_is_refused",
        substitution: None,
    },
    MandatoryTrace {
        row: "settings result with an old revision",
        test: "a_settings_result_with_an_old_revision_is_refused",
        substitution: None,
    },
    MandatoryTrace {
        row: "ride finalize failure after the last checkpoint",
        test: "a_ride_finalize_failure_after_the_last_checkpoint_reaches_the_rider",
        substitution: None,
    },
    MandatoryTrace {
        row: "trip member disappearance before delete commit",
        test: "an_object_that_vanished_before_the_commit_is_a_success",
        substitution: Some(
            "the trip cascade never becomes an effect — CatalogState::admit_intent refuses it              (LegacyOwned::TripCascade), so there is no bounded member read to race with. The trace              runs the same disappearance against a route removal, which is where the rule lives: an              object already gone is `existed: false`, a success, never a failure the rider sees. The              cascade's own member read lands with #1491.",
        ),
    },
    MandatoryTrace {
        row: "capability change after a new map mounts",
        test: "capabilities_follow_the_mounted_data_and_the_platform",
        substitution: None,
    },
    MandatoryTrace {
        row: "detour without a path",
        test: "a_detour_without_a_path_is_a_failure_and_not_an_absent_capability",
        substitution: None,
    },
    MandatoryTrace {
        row: "device without detour capability",
        test: "capabilities_follow_the_mounted_data_and_the_platform",
        substitution: None,
    },
    MandatoryTrace {
        row: "active-route deletion with same-pass Navigator delivery",
        test: "deleting_the_active_route_drops_it_in_the_same_pass",
        substitution: None,
    },
    MandatoryTrace {
        row: "Navigator activation with next-pass Retention delivery",
        test: "an_activation_reaches_retention_on_the_next_pass",
        substitution: None,
    },
    MandatoryTrace {
        row: "full effect slot and full outcome slot",
        test: "a_full_slot_preserves_work_on_both_sides_of_the_seam",
        substitution: None,
    },
    MandatoryTrace {
        row: "deferred slot which forces a pass before sleep",
        test: "a_deferred_value_forces_a_pass_before_sleep",
        substitution: None,
    },
    MandatoryTrace {
        row: "stale derived input after a subject change",
        test: "a_stale_derived_fill_is_dropped_and_the_level_asks_again",
        substitution: None,
    },
];

#[test]
fn every_mandatory_trace_has_a_test_that_runs_it() {
    let source = include_str!("device_core_conformance.rs");
    for trace in MANDATORY_TRACES {
        // `#[test]` and not merely `fn`: a private helper of the same name, or a trace that lost its
        // attribute, would otherwise satisfy the binding without ever running.
        assert!(
            source.contains(&format!("#[test]\nfn {}(", trace.test)),
            "{}: no `#[test] fn {}` in this file",
            trace.row,
            trace.test
        );
        if let Some(substitution) = trace.substitution {
            assert!(
                substitution.contains('#'),
                "{}: a substituted situation names the slice that makes the row literal",
                trace.row
            );
        }
    }
    let substituted = MANDATORY_TRACES.iter().filter(|trace| trace.substitution.is_some()).count();
    assert_eq!(
        substituted, 1,
        "the store-refresh row became literal at #1397 S6a; only the trip cascade is still \
         unreachable, and it says so"
    );
}

fn typed() -> CoreHarness {
    CoreHarness::new(Executor::Typed)
}

/// Three routes under a trusted clock, `expired` of them long past their deadline, so the retention
/// sweep decides to expire them and the catalog turns each into one bounded removal. This is the one
/// path that reaches `CatalogEffect` from a public app surface, which is why the catalog traces
/// below are written on it rather than on the rider's hold-to-delete.
fn expiring(expired: usize) -> CoreHarness {
    let mut harness = typed();
    harness.app().stamp_clock_ble(1_720_000_000, 60);
    let now = harness.state.app.wall_unix_now();
    let old = now.saturating_sub(30 * 24 * 3600);
    let meta: Vec<_> = (0..3)
        .map(|index| {
            if index < expired {
                RouteRetentionMeta::new(Retention::Week1, old)
            } else {
                RouteRetentionMeta::new(Retention::Never, 0)
            }
        })
        .collect();
    harness.app().set_route_meta(&meta);
    harness.app().force_retention_sweep();
    harness
}

impl CoreHarness {
    /// Apply one corpus action outside the scenario runner.
    fn apply(&mut self, action: Action) {
        let mut trace = recorder();
        self.clock_ms = self.clock_ms.max(clock_watermark(action));
        self.state.apply_input(&action, &mut trace);
    }

    /// Serve one catalog effect and hand the answer straight back.
    fn answer_catalog(&mut self, effect: CatalogEffect) -> CatalogOutcome {
        let Done::Catalog { outcome, refeed } = self.serve_catalog(effect) else { panic!("a catalog answer") };
        self.deliver(Done::Catalog { outcome, refeed }, &mut recorder());
        outcome
    }

    /// The next catalog effect, running passes until one appears.
    fn next_catalog_effect(&mut self) -> CatalogEffect {
        for _ in 0..8 {
            let mut plan = self.pass();
            if let Some(effect) = plan.effects.catalog.take() {
                return effect;
            }
        }
        panic!("no catalog effect within eight passes")
    }
}

/// **Outcome after cancellation.** A terminal answer invalidates the domain's token, so a repeat of
/// it is no longer current and starts nothing — the rule a cancellation applies, reached through the
/// one terminal event every operation ends with.
#[test]
fn an_outcome_after_cancellation_changes_nothing() {
    let mut harness = expiring(1);
    let effect = harness.next_catalog_effect();
    let outcome = harness.answer_catalog(effect);
    harness.pass();
    assert_eq!(harness.state.route_ids.len(), 2, "the expiry removed one route");

    // The same answer again: the operation is over, so nothing reopens and nothing is retried.
    let _ = harness.inbox.outcomes.catalog.try_put(outcome);
    let plan = harness.pass();
    assert!(plan.effects.catalog.is_empty(), "a repeat of a terminal answer starts no work");
    assert_eq!(harness.state.route_ids.len(), 2, "and removes nothing a second time");

    // The navigator half of the same rule, on the real machine (#1397 S2): the rider's Back
    // invalidates the operation, so the planner's answer commits no route at all.
    let mut harness = typed();
    assert!(harness.app().debug_start_nav((0, 0), (1_000, 1_000), "col"), "the plan is admitted");
    let mut plan = harness.pass();
    let effect = plan.effects.navigator.take().expect("and leaves as one bounded planner operation");
    harness.app().apply_gesture(Gesture::Back); // the rider walks away from the spinner
    let _ =
        harness.inbox.outcomes.navigator.try_put(NavigatorOutcome::PlanFinished { token: effect.token(), route: 10 });
    harness.pass();
    assert_eq!(harness.state.app.active_route_index(), None, "the cancelled plan adopted nothing");
    assert!(
        !matches!(harness.state.app.top_screen(), Screen::RouteOverview(_)),
        "and the rider is not shown an overview for a route they cancelled"
    );

    // The same rule seen from the adapter's side: it hands the stored token straight back, and it is
    // the *domain* that refuses it.
    let mut navigator: TokenSource<NavigatorTag> = TokenSource::new();
    let token = navigator.issue();
    let mut adapter = LegacyAdapter::new();
    let mut mail: HostMailbox = HostMailbox::new();
    let mut effects = EffectSlots::new();
    let work = PlannerWork::Route(NavRequest::new((0, 0), (1, 1), "goal"));
    effects.navigator.try_put(NavigatorEffect::Acquire { token, work }).unwrap();
    assert_eq!(adapter.effects_to_commands(&mut effects, &mut mail).translated, 1);
    navigator.invalidate(); // the rider pressed Back
    let mut inbox = LegacyInputs::new();
    adapter.event_to_inputs(HostEvent::NavPlanned(Ok(9)), &mut inbox).unwrap();
    let outcome = inbox.outcomes.navigator.take().expect("delivered, not swallowed");
    assert_eq!(outcome, NavigatorOutcome::PlanFinished { token, route: 9 });
    assert!(!navigator.is_current(outcome.token()), "the cancelled operation does not accept it");
}

/// **Outcome after a replacement request.** The next operation supersedes the last; an answer that
/// belongs to the finished one changes nothing about the live one.
#[test]
fn an_outcome_after_a_replacement_request_changes_nothing() {
    let mut harness = expiring(2);
    let first = harness.next_catalog_effect();
    harness.answer_catalog(first);
    let second = harness.next_catalog_effect();
    assert_ne!(first.token(), second.token(), "a new operation, a new token");

    // The finished operation's answer, arriving again against the live one.
    let _ = harness.inbox.outcomes.catalog.try_put(CatalogOutcome::ObjectRemoved {
        token: first.token(),
        object: 10,
        existed: true,
    });
    let plan = harness.pass();
    assert!(plan.effects.catalog.is_empty(), "the stale answer did not free the live operation");

    harness.answer_catalog(second);
    harness.pass();
    assert_eq!(harness.state.route_ids.len(), 1, "both removals landed, neither twice");
}

/// **A store change during a catalog refresh.** The commit is an edge the pass records once, the
/// domain keeps one operation in flight, and neither loses the other — the refresh the commit
/// ordered simply waits for the removal that is already running.
///
/// Literal since #1397 S6a: `ExternalFacts::store_revision` raises `CatalogIntent::Refresh`, so
/// there is a real catalog refresh for a real store change to race.
#[test]
fn a_store_change_during_a_catalog_operation_is_not_lost() {
    let mut harness = expiring(1);
    let effect = harness.next_catalog_effect();

    // The store moves underneath us while that removal is unanswered.
    harness.inbox.facts.note_store_revision(StoreRevision { store: StoreIdentity::new(1), revision: Revision::new(4) });
    let plan = harness.pass();
    assert!(plan.effects.catalog.is_empty(), "one catalog operation at a time");
    assert_eq!(harness.state.app.store_changed_pending(), 0, "and no legacy rescan cue behind it");

    // The same revision again is the same edge, not a second one: the refresh is owed once.
    harness.inbox.facts.note_store_revision(StoreRevision { store: StoreIdentity::new(1), revision: Revision::new(4) });
    harness.pass();

    // The removal's answer frees the domain, and the commit's own re-read is what goes out next —
    // once, not twice.
    harness.answer_catalog(effect);
    let refresh = harness.next_catalog_effect();
    assert!(matches!(refresh, CatalogEffect::RemoveObject { .. } | CatalogEffect::ReadCatalog { .. }));
    let refresh = match refresh {
        CatalogEffect::ReadCatalog { .. } => refresh,
        // The expiry sweep can re-offer its own removal first; the refresh is right behind it.
        other => {
            harness.answer_catalog(other);
            harness.next_catalog_effect()
        }
    };
    assert!(matches!(refresh, CatalogEffect::ReadCatalog { .. }), "the commit ordered exactly one re-read");
    harness.answer_catalog(refresh);
    let plan = harness.pass();
    assert!(plan.effects.catalog.is_empty(), "one commit, one refresh");
    assert_eq!(harness.state.route_ids.len(), 2, "and the removal completed");
}

/// **A transfer starts during route planning.** Heavy work is withdrawn while a transfer holds the
/// store, so a *new* plan is never started — and the one already running is not failed by it either.
///
/// The distinction is the rule: admission decides what may *begin*, and a capability going away is
/// not a reason to cancel an operation that is already owed an answer.
#[test]
fn a_transfer_during_planning_withdraws_heavy_capability() {
    // A plan is admitted and goes out under Navigator's live token.
    let mut navigator: TokenSource<NavigatorTag> = TokenSource::new();
    let token = navigator.issue();
    let mut adapter = LegacyAdapter::new();
    let mut mail: HostMailbox = HostMailbox::new();
    let mut effects = EffectSlots::new();
    let work = PlannerWork::Route(NavRequest::new((0, 0), (1, 1), "goal"));
    effects.navigator.try_put(NavigatorEffect::Acquire { token, work }).unwrap();
    adapter.effects_to_commands(&mut effects, &mut mail);
    assert!(matches!(mail.pop(), Some(HostCommand::PlanRoute(_))), "the planner was asked");
    assert!(adapter.pending().holds(LegacyReply::RoutePlan), "and is owed an answer");

    // The transfer starts. Admission is a level, recalculated from what is true now.
    let facts = |heavy| DeviceFacts {
        store_writable: true,
        nav_graph: true,
        weather_data: false,
        link_connected: true,
        ride_recording: false,
        heavy_operations: heavy,
    };
    let idle = Capabilities::calculate(EVERYTHING, facts(true));
    let streaming = Capabilities::calculate(EVERYTHING, facts(false));
    assert!(idle.navigator.plan_route && idle.navigator.plan_detour && idle.dfu.install);
    assert!(!streaming.navigator.plan_route, "a second route plan cannot start");
    assert!(!streaming.navigator.plan_detour, "nor a detour");
    assert!(!streaming.dfu.install, "nor an install");
    assert!(streaming.catalog.mutate, "but the store is still writable — this is admission, not a fault");

    // The pass sees the transfer and starts nothing; it also fails nothing.
    let mut harness = expiring(0);
    harness.inbox.facts.note_transfer(TransferState::Active);
    let mut plan = harness.pass();
    assert!(plan.effects.navigator.is_empty(), "no plan is started while the transfer holds the store");
    assert!(plan.effects.dfu.is_empty(), "nor an install");
    assert!(plan.effects.catalog.is_empty() && plan.effects.retention.is_empty(), "nor a store operation");
    // The settings write is not heavy and is not withdrawn: the trusted-clock stamp this fixture
    // makes is a rider edit like any other, and a transfer holding the *store* has nothing to say
    // about a settings revision. That distinction is what `Capabilities` is for — asserted, not
    // merely tolerated, so a settings write that quietly stopped going out fails here.
    assert!(plan.effects.settings.take().is_some(), "the settings write is unaffected by the transfer");
    assert!(adapter.pending().holds(LegacyReply::RoutePlan), "and the running plan is untouched");
    assert!(navigator.is_current(token), "its operation is still the current one");

    // So when the planner finally answers, it is still this operation's answer.
    let mut inbox = LegacyInputs::new();
    adapter.event_to_inputs(HostEvent::NavPlanned(Ok(10)), &mut inbox).unwrap();
    let outcome = inbox.outcomes.navigator.take().expect("the answer is delivered");
    assert!(navigator.is_current(outcome.token()), "a withdrawn capability never cancelled the running plan");

    // …and the capability comes straight back when the transfer ends.
    harness.inbox.facts.note_transfer(TransferState::Idle);
    harness.pass();
    assert!(Capabilities::calculate(EVERYTHING, facts(true)).navigator.plan_route);
}

/// **A route plan completes after the active route changed.** The answer carries the token the
/// request went out with; a Navigator that has moved on refuses it.
#[test]
fn a_route_plan_that_lands_after_the_active_route_changed_is_refused() {
    let mut navigator: TokenSource<NavigatorTag> = TokenSource::new();
    let token = navigator.issue();
    let mut adapter = LegacyAdapter::new();
    let mut mail: HostMailbox = HostMailbox::new();
    let mut effects = EffectSlots::new();
    let work = PlannerWork::Route(NavRequest::new((0, 0), (1, 1), "first"));
    effects.navigator.try_put(NavigatorEffect::Acquire { token, work }).unwrap();
    adapter.effects_to_commands(&mut effects, &mut mail);

    // The rider activates a different route: Navigator replaces its operation.
    navigator.invalidate();
    let replacement = navigator.issue();

    let mut inbox = LegacyInputs::new();
    adapter.event_to_inputs(HostEvent::NavPlanned(Ok(10)), &mut inbox).unwrap();
    let outcome = inbox.outcomes.navigator.take().expect("the answer is delivered");
    assert_eq!(outcome.token(), token, "carrying the token it went out with");
    assert!(!navigator.is_current(outcome.token()), "which is no longer the current operation");
    assert!(navigator.is_current(replacement), "the replacement is");
}

/// **A settings result with an old revision.** The token and the revision are independent guards,
/// and the rider-visible half is identical in the legacy and DeviceCore runners.
#[test]
fn a_settings_result_with_an_old_revision_is_refused() {
    let mut settings: TokenSource<SettingsTag> = TokenSource::new();
    let token = settings.issue();
    let mut adapter = LegacyAdapter::new();
    let mut mail: HostMailbox = HostMailbox::new();
    let mut effects = EffectSlots::new();
    effects.settings.try_put(SettingsEffect::PersistRevision { token, revision: 4 }).unwrap();
    adapter.effects_to_commands(&mut effects, &mut mail);
    assert_eq!(mail.pop(), Some(HostCommand::PersistSettings { revision: 4 }));

    settings.invalidate(); // the rider edited again — a newer revision is the latest now
    let mut inbox = LegacyInputs::new();
    adapter.event_to_inputs(HostEvent::SettingsPersisted { revision: 4 }, &mut inbox).unwrap();
    let outcome = inbox.outcomes.settings.take().expect("delivered, not swallowed");
    assert_eq!(outcome, SettingsOutcome::Persisted { token, revision: 4 });
    assert!(!settings.is_current(outcome.token()), "the superseded write does not clear the dirty state");

    // …and it is real behaviour rather than only a token: the stale ack leaves the newer content
    // pending at both answer cadences.
    let scenario = named("settings.revision-success-and-stale-result");
    let immediate = Runner::CoreImmediate.run(scenario);
    let delayed = Runner::CoreDelayed.run(scenario);
    assert_eq!(rider_visible(delayed.settled.clone()), rider_visible(immediate.settled.clone()));
}

/// **A ride finalize failure after the last checkpoint.** The ride close survives the pass, the
/// executor that performs it fails it, and the rider is told.
///
/// This trace is why the `ui_recorder` → `ride_closed` wiring is gone. It used to take the rider's
/// finish one-shot at stage 4 and drop it at stage 7, so the ride was never finalized and no
/// executor was told — a rider request destroyed to serve a lifecycle Recorder does not own yet.
/// The pass now leaves it alone, which is what "the close reaches the platform on the legacy path"
/// has to mean to be true.
#[test]
fn a_ride_finalize_failure_after_the_last_checkpoint_reaches_the_rider() {
    let mut harness = expiring(0);
    harness.app().activity.start_session();
    harness.pass();

    // The last checkpoint is behind us; the finalize is the one that fails.
    harness.state.fail_next_finalize = true;
    harness.app().activity.request_track(TrackAction::Save);
    let plan = harness.pass();
    assert!(plan.effects.recorder.is_empty(), "Recorder has no machine, so the pass emits no effect");
    assert!(harness.state.app.activity.has_track_action(), "and it leaves the rider's finish for the drain");

    let mut done = Vec::new();
    let mut trace = recorder();
    harness.serve_mailbox(&mut done, &mut trace);
    // The legacy vocabulary has no recorder-finalize outcome, so a host reports the failure through
    // the generic warning — DC1's own recorded compatibility limitation, unchanged here.
    assert!(
        matches!(done.as_slice(), [Done::Event(HostEvent::Warning(flags))] if flags.contains(WarningFlags::REC_ERROR)),
        "the finalize failed and said so: {done:?}"
    );
    for item in done {
        harness.deliver(item, &mut trace);
    }
    harness.pass();
    assert!(
        matches!(harness.state.app.top_screen(), Screen::Warning(card)
            if card.flags().contains(WarningFlags::REC_ERROR)),
        "and the rider is told rather than left believing the ride was saved"
    );

    // The typed replacement is already mapped: when Recorder emits the effect, the adapter knows
    // where it goes, so what is missing is one domain's machine and nothing in the protocol.
    let mut recorder_ops = TokenSource::new();
    let mut effects = EffectSlots::new();
    effects.recorder.try_put(RecorderEffect::Finalize { token: recorder_ops.issue() }).unwrap();
    let mut adapter = LegacyAdapter::new();
    let mut mail: HostMailbox = HostMailbox::new();
    assert_eq!(adapter.effects_to_commands(&mut effects, &mut mail).translated, 1);
    assert_eq!(mail.pop(), Some(HostCommand::FinishTrack(TrackAction::Save)));
    assert!(LegacyOwned::RideCloseAck.deletes_in().contains("#1398"), "the acknowledgement is still owed");
}

/// **A trip member disappears before the delete commit.** The goal state holds, so the removal is a
/// success with `existed: false` — never a failure the rider is shown.
#[test]
fn an_object_that_vanished_before_the_commit_is_a_success() {
    let mut harness = expiring(1);
    let effect = harness.next_catalog_effect();
    let CatalogEffect::RemoveObject { object, .. } = effect else { panic!("a removal") };

    // Something else removed it first.
    let index = harness.state.route_ids.iter().position(|&id| id == object).expect("still catalogued");
    harness.state.routes.remove(index);
    harness.state.route_ids.remove(index);

    let Done::Catalog { outcome, refeed } = harness.serve_catalog(effect) else { panic!() };
    assert_eq!(outcome, CatalogOutcome::ObjectRemoved { token: effect.token(), object, existed: false });
    assert_eq!(refeed, Refeed::None, "nothing changed, so nothing is re-fed");
    harness.deliver(Done::Catalog { outcome, refeed }, &mut recorder());
    let plan = harness.pass();
    assert!(plan.effects.catalog.is_empty(), "the operation is over — nothing retried, nothing failed");
}

/// **A capability changes after a new map mounts**, and **a device without the detour capability**.
///
/// A capability is a level recomputed from what the image implements and what is true now. A missing
/// graph or missing support withdraws the operation entirely, so "unsupported" never reaches the
/// rider as a planning failure.
#[test]
fn capabilities_follow_the_mounted_data_and_the_platform() {
    let facts = |nav_graph| DeviceFacts {
        store_writable: true,
        nav_graph,
        weather_data: false,
        link_connected: false,
        ride_recording: false,
        heavy_operations: true,
    };
    let before = Capabilities::calculate(EVERYTHING, facts(false));
    let after = Capabilities::calculate(EVERYTHING, facts(true));
    assert!(!before.navigator.plan_route && !before.navigator.plan_detour, "no graph, no planning");
    assert!(after.navigator.plan_route && after.navigator.plan_detour, "a mounted routing graph turns both on");
    assert!(before.catalog.mutate && after.catalog.mutate, "and the rest of the device is unaffected");

    // A device the detour was never built for.
    let limited = Capabilities::calculate(NO_DETOUR, facts(true));
    assert!(limited.navigator.plan_route, "route planning is unaffected");
    assert!(!limited.navigator.plan_detour && !limited.navigator.commit_detour, "the detour is simply absent");
}

/// A platform without the detour, for the capability traces.
const NO_DETOUR: PlatformSupport = PlatformSupport { detour: false, ..EVERYTHING };

/// **A detour without a path** is a planning failure, and it is a *different* value from the absence
/// of the capability — the epic's "unsupported must not appear as NoPath".
#[test]
fn a_detour_without_a_path_is_a_failure_and_not_an_absent_capability() {
    let mut navigator: TokenSource<NavigatorTag> = TokenSource::new();
    let token = navigator.issue();
    let mut adapter = LegacyAdapter::new();
    let mut mail: HostMailbox = HostMailbox::new();
    let mut effects = EffectSlots::new();
    let work = PlannerWork::Detour(DetourRequest { route: 0, from: (0, 0), progress_m: 0, target_m: 500 });
    effects.navigator.try_put(NavigatorEffect::Acquire { token, work }).unwrap();
    adapter.effects_to_commands(&mut effects, &mut mail);
    assert!(matches!(mail.pop(), Some(HostCommand::PlanDetour(_))), "a supported device asks the planner");

    let mut inbox = LegacyInputs::new();
    adapter.event_to_inputs(HostEvent::DetourPlanned(Err(NavError::NoPath)), &mut inbox).unwrap();
    let outcome = inbox.outcomes.navigator.take().expect("the failure is an answer");
    assert_eq!(outcome, NavigatorOutcome::Failed { token, error: NavigatorError::Plan(NavError::NoPath) });
    assert!(navigator.is_current(outcome.token()), "and it is this operation's answer");

    // The unsupported device never gets here: the capability is absent, so nothing is requested and
    // there is no failure to report at all.
    let facts = DeviceFacts {
        store_writable: true,
        nav_graph: true,
        weather_data: false,
        link_connected: false,
        ride_recording: false,
        heavy_operations: true,
    };
    assert!(!Capabilities::calculate(NO_DETOUR, facts).navigator.plan_detour);
}

/// **Deleting the active route, with same-pass Navigator delivery.** The rider is not left being
/// guided along a route the device has decided to remove.
#[test]
fn deleting_the_active_route_drops_it_in_the_same_pass() {
    let mut harness = typed();
    harness.app().activate_route(0);
    assert_eq!(harness.state.app.active_route_index(), Some(0));

    harness.apply(Action::DeleteRoute);
    let mut plan = harness.pass();
    assert!(
        matches!(plan.effects.catalog.take(), Some(CatalogEffect::RemoveObject { object: 10, .. })),
        "the rider's delete left as one bounded catalog operation"
    );
    assert_eq!(harness.state.app.active_route_index(), None, "and Navigator heard about it in that pass");
}

/// **Navigator activation, with next-pass Retention delivery.** Navigator runs after retention, so
/// the activation waits one pass — and the wait is bounded by the immediate wake, not by input.
#[test]
fn an_activation_reaches_retention_on_the_next_pass() {
    let mut harness = typed();
    harness.app().stamp_clock_ble(1_720_000_000, 60);
    let now = harness.state.app.wall_unix_now();
    // A fresh `last_used`, so the hourly sweep has nothing of its own to say and the only stamp in
    // this trace is the activation's.
    harness.app().set_route_meta(&[
        RouteRetentionMeta::new(Retention::Week1, now),
        RouteRetentionMeta::new(Retention::Never, 0),
        RouteRetentionMeta::new(Retention::Never, 0),
    ]);
    harness.app().activate_route(0);

    let mut plan = harness.pass();
    assert!(plan.immediate, "Navigator runs after retention, so the activation is deposited, not delivered");
    assert!(
        matches!(plan.effects.retention.take(), Some(RetentionEffect::WriteRouteMetadata { id: 10, .. })),
        "the active route's use stamp goes out"
    );

    let plan = harness.pass();
    assert!(!plan.immediate, "retention consumed it on the next pass, and nothing is left waiting");
    assert!(plan.effects.retention.is_empty(), "the delivery is idempotent — no second sidecar write");
}

/// **A full effect slot and a full outcome slot.** Both preserve the value already there, and a
/// refused one comes back to its owner rather than being dropped.
#[test]
fn a_full_slot_preserves_work_on_both_sides_of_the_seam() {
    // The effect side: two objects expire together, the domain admits one, and the other is not
    // queued in the slot — it stays with its producer and goes out once the answer frees the domain.
    let mut harness = expiring(2);
    let first = harness.next_catalog_effect();
    let plan = harness.pass();
    assert!(plan.effects.catalog.is_empty(), "one catalog operation in flight at a time");
    harness.answer_catalog(first);
    let second = harness.next_catalog_effect();
    assert_ne!(first.token(), second.token(), "nothing was lost to the busy pass");

    // The outcome side: a second answer cannot displace one the pass has not consumed yet.
    let mut outcomes = OutcomeSlots::new();
    let mut tokens = TokenSource::new();
    let held = CatalogOutcome::Cancelled { token: tokens.issue() };
    let intruder = CatalogOutcome::Cancelled { token: tokens.issue() };
    outcomes.catalog.try_put(held).unwrap();
    let refused = outcomes.catalog.try_put(intruder).expect_err("a full slot refuses");
    assert_eq!(refused.rejected, intruder, "and hands the value back to its owner");
    assert_eq!(outcomes.catalog.take(), Some(held), "the first answer is what the domain gets");
}

/// **A deferred slot forces a pass before sleep.** Work that is already decided must not sit until
/// the next rider input.
///
/// Written on the route activation, which is the deferred producer the wiring actually has:
/// Navigator runs after retention, so an activation cannot reach backwards and waits a pass.
#[test]
fn a_deferred_value_forces_a_pass_before_sleep() {
    let mut harness = typed();
    harness.app().activate_route(0);

    let plan = harness.pass();
    assert!(plan.immediate && plan.next_wake_ms == Some(0), "the runtime comes straight back");

    let plan = harness.pass();
    assert!(!plan.immediate && plan.next_wake_ms != Some(0), "consumed, and nothing is left to hurry for");

    harness.app().activate_route(1);
    let plan = harness.pass();
    assert!(plan.immediate, "a second activation is deposited just the same");
}

/// **A stale derived input after a subject change** — the corrected defect of this gate.
///
/// The legacy feeders carry no subject, so a delayed fill for the ride the rider *was* looking at
/// satisfies the need for the one they are looking at now. DeviceCore keys the read, so the same
/// delayed answer is dropped and the level asks again.
#[test]
fn a_stale_derived_fill_is_dropped_and_the_level_asks_again() {
    let mut harness = typed();
    open_ride_detail(&mut harness);
    let plan = harness.pass();
    let key = plan.derived_needs.ride_track.expect("the open detail wants its track");

    // An answer about a different ride is an answer to a question nobody asked.
    let other = RideTrackKey { ride: key.ride + 1, source: key.source, view: key.view };
    harness.inbox.derived.ride_track = Some(DerivedInput { key: other, result: DerivedResult::Filled });
    let plan = harness.pass();
    assert_eq!(plan.derived_needs.ride_track, Some(key), "the need is untouched");

    // A failure for the *right* key is an answer, so a dead source costs one read and not one per
    // pass.
    harness.inbox.derived.ride_track = Some(DerivedInput { key, result: DerivedResult::Failed });
    let plan = harness.pass();
    assert!(plan.derived_needs.ride_track.is_none(), "a failure answers the key");

    // And the scenario-level consequence: DeviceCore's terminal state stops depending on cadence.
    let scenario = named("derived-data.repeats-until-matching-fill");
    let immediate = Runner::CoreImmediate.run(scenario);
    let delayed = Runner::CoreDelayed.run(scenario);
    assert_eq!(
        rider_visible(delayed.settled.clone()),
        rider_visible(immediate.settled.clone()),
        "DeviceCore's terminal state no longer depends on delivery cadence"
    );
}

fn open_ride_detail(harness: &mut CoreHarness) {
    for gesture in [Gesture::Press, Gesture::Step(1), Gesture::Press, Gesture::Press] {
        harness.app().apply_gesture(gesture);
    }
}

/// A throwaway recorder for the direct pass-level traces, which assert on state rather than on a
/// trace timeline. One open step, never finished — the feeder calls a re-feed makes have somewhere
/// to land.
fn recorder() -> TraceRecorder<VisibleState> {
    let mut recorder = TraceRecorder::new("direct", RunnerMode::Immediate, blank_state());
    recorder.begin_step(TraceInput::Named("direct"));
    recorder
}

fn blank_state() -> VisibleState {
    visible_state(&App::new_idle(AppState::new(0, 0, 1.0)), 0, 0)
}

/// The one warning path the pass owns end to end: a fault raised by any producer reaches the rider
/// in the pass it was raised in, and several producers coalesce onto one card.
#[test]
fn a_fact_raised_this_pass_reaches_the_rider_in_it() {
    let mut harness = expiring(0);
    harness.inbox.facts.raise_warnings(WarningFlags::NO_GPS);
    harness.inbox.facts.raise_warnings(WarningFlags::MAP_SLOW);
    harness.pass();
    assert!(
        matches!(harness.state.app.top_screen(), Screen::Warning(card)
            if card.flags().contains(WarningFlags::NO_GPS) && card.flags().contains(WarningFlags::MAP_SLOW)),
        "both notices reached one card"
    );
}

/// A failed sidecar write re-queues its candidate — the retry the legacy protocol cannot express at
/// all, because it never acknowledges a stamp (`LegacyOwned::SidecarAck`).
#[test]
fn a_failed_retention_write_is_retried() {
    let mut harness = typed();
    harness.app().stamp_clock_ble(1_720_000_000, 60);
    let now = harness.state.app.wall_unix_now();
    harness.app().set_route_meta(&[
        RouteRetentionMeta::new(Retention::Week1, now),
        RouteRetentionMeta::new(Retention::Never, 0),
        RouteRetentionMeta::new(Retention::Never, 0),
    ]);
    harness.pass();
    harness.app().activate_route(0);

    let mut effect = None;
    for _ in 0..8 {
        let mut plan = harness.pass();
        if let Some(found) = plan.effects.retention.take() {
            effect = Some(found);
            break;
        }
    }
    let effect = effect.expect("the activation's use stamp goes out");
    let _ = harness
        .inbox
        .outcomes
        .retention
        .try_put(RetentionOutcome::Failed { token: effect.token(), error: RetentionError::WriteFailed });

    let mut retried = false;
    for _ in 0..16 {
        let mut plan = harness.pass();
        if plan.effects.retention.take().is_some() {
            retried = true;
            break;
        }
    }
    assert!(retried, "a failed write keeps its candidate and offers it again");
}

/// A decided sidecar stamp is mirrored into the resident view, so it is not rediscovered.
///
/// The eager ride stamp runs on every trusted tick and re-enqueues any resident ride that is
/// `synced` with a `synced_at` of 0; only the mirror clears that 0, so an unmirrored stamp comes
/// back on every pass after the executor answers it. Both the legacy drain
/// (`App::retention_stamp_command`) and the pass mirror into the full ride inventory as well as
/// the display catalog, because a ride outside the newest-32 menu re-enqueues just the same
/// (finding #876-2).
///
/// Without the mirror this fails on the first pass after the answer.
#[test]
fn a_stamp_that_was_answered_is_not_enqueued_again() {
    let mut harness = typed();
    harness.app().stamp_clock_ble(1_720_000_000, 60);
    let id = harness.state.ride_ids[0];
    harness.app().set_ride_retention_inventory(&[RideRetentionRecord { id, synced: true, synced_at_utc: 0 }]);
    harness.app().force_retention_sweep();

    let mut effect = None;
    for _ in 0..8 {
        let mut plan = harness.pass();
        if let Some(found) = plan.effects.retention.take() {
            effect = Some(found);
            break;
        }
        harness.serve(plan, &mut recorder());
    }
    let effect = effect.expect("an acked ride with no synced_at stamp gets one");
    let outcome = harness.serve_retention(effect);
    harness.deliver(Done::Retention(outcome), &mut recorder());

    for step in 0..8 {
        let mut plan = harness.pass();
        assert!(
            plan.effects.retention.take().is_none(),
            "the answered stamp came back on settle pass {step} — an endless sidecar write"
        );
        harness.serve(plan, &mut recorder());
    }
}

/// The conformance replay's wake profile and pass cost — #1440's last two resource rows.
///
/// The wake counts are deterministic, so they are asserted: a pass that starts polling, or a
/// deferred connection that stops settling, moves them and this fails. The times are
/// machine-dependent, so they are printed under `--nocapture` rather than gated. What the test
/// guarantees is that both figures are reproducible from one command, instead of from a measurement
/// harness someone ran once and deleted.
#[test]
fn the_conformance_replay_wake_profile_and_pass_cost() {
    let mut passes = 0u32;
    let mut immediate = 0u32;
    let mut timed = 0u32;
    let mut sleeps = 0u32;
    let mut total = std::time::Duration::ZERO;
    let mut worst = std::time::Duration::ZERO;

    for scenario in SCENARIOS {
        for executor in [Executor::Typed, Executor::Compatibility] {
            let mut harness = CoreHarness::new(executor);
            let mut trace = recorder();
            let actions = scenario.actions.iter().chain(std::iter::repeat_n(&Action::Settle, SETTLE_PASSES));
            for action in actions {
                harness.apply(*action);
                let start = std::time::Instant::now();
                let plan = harness.pass();
                let elapsed = start.elapsed();
                total += elapsed;
                worst = worst.max(elapsed);
                passes += 1;
                match plan.next_wake_ms {
                    Some(0) => immediate += 1,
                    Some(_) => timed += 1,
                    None => sleeps += 1,
                }
                harness.serve(plan, &mut trace);
            }
        }
    }

    println!("passes {passes}: {immediate} immediate, {timed} timed, {sleeps} sleep-until-event");
    println!(
        "pass time: mean {:.3} us, worst {:.3} us",
        total.as_secs_f64() * 1e6 / f64::from(passes),
        worst.as_secs_f64() * 1e6
    );

    assert_eq!(passes, WAKE_PROFILE.0, "the replay's pass count is fixed by the scenario table");
    assert_eq!(immediate, WAKE_PROFILE.1, "an immediate wake is decided work that has not reached its consumer");
    assert_eq!(timed, WAKE_PROFILE.2);
    assert_eq!(sleeps, WAKE_PROFILE.3);
    assert!(immediate * 10 < passes, "immediate wakes stay a small minority — nothing here polls");
}

/// `(passes, immediate, timed, sleep-until-event)` for the replay above. A ratchet, not a budget:
/// the numbers move when the pass's wake decisions do.
///
/// Two replay passes are *timed* rather than *sleep-until-event* because a settings write's
/// failure is raised into the fault connection at stage 1, so its card — and the wake the card's
/// 30 s timeout arms — lands at stage 13 of that pass rather than before stages 2-12 run. The two
/// gating figures — 366 passes and 6 immediate wakes — are the claim that matters: nothing here
/// polls.
const WAKE_PROFILE: (u32, u32, u32, u32) = (366, 6, 238, 122);

// ==================== the resource gate ====================

/// The pass protocol's size budget, re-asserted from outside `obc-app`.
///
/// The compile-time assertions inside the crate gate the same values; this one exists because a
/// *host* is what puts both structs on its stack every pass, and because the gate's resource table
/// has to be reproducible from a command anyone can run.
#[test]
fn the_pass_protocol_stays_within_its_budget() {
    use std::mem::size_of;

    assert!(size_of::<EffectSlots>() <= 160, "nine bounded effects: {}", size_of::<EffectSlots>());
    assert!(size_of::<OutcomeSlots>() <= 224, "nine bounded outcomes: {}", size_of::<OutcomeSlots>());
    assert!(size_of::<DerivedNeeds>() <= 64);
    assert!(size_of::<DerivedInputs>() <= 80);

    // The largest single message per direction — what a payload creeping into the protocol would
    // show up as first.
    let largest_effect = [
        size_of::<CatalogEffect>(),
        size_of::<RetentionEffect>(),
        size_of::<RecorderEffect>(),
        size_of::<NavigatorEffect>(),
        size_of::<SettingsEffect>(),
        size_of::<WeatherEffect>(),
        size_of::<DfuEffect>(),
        size_of::<BondEffect>(),
        size_of::<StorageInfoEffect>(),
    ]
    .into_iter()
    .max()
    .unwrap();
    assert!(largest_effect <= 96, "the planner request is the largest effect: {largest_effect}");
}
