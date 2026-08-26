//! DC7 — the DeviceCore Phase 1 conformance gate (#1440, epic #1433 §13).
//!
//! Every DC1 scenario, run through every runner, compared on what the rider can see:
//!
//! | Runner | Frame | Executor |
//! |---|---|---|
//! | `core-immediate` | [`App::run_pass`] | typed effects in, typed outcomes back, same call |
//! | `core-delayed` | the same | the same, on a scripted delay |
//!
//! The comparison is **rider-visible state**, not command sequences. `core-immediate` is the
//! baseline and `core-delayed` is what proves behaviour is independent of answer cadence — the
//! property most likely to regress, and the one the delayed runner exists for.
//!
//! ## A difference is a failure, not a row
//!
//! There is no disposition table and no way to record a difference and move on: the runners must
//! reach the same rider-visible state, and [`every_scenario_agrees_in_every_runner`] failing is what
//! a difference looks like. Beside it, three coverage gates say the corpus is still reaching what it
//! claims to — every DC1 behaviour row is claimed by a scenario, every bulk feeder is exercised by a
//! real call, and each upload in a burst reaches the rider.
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
//! **Two** domains still speak the legacy protocol: Recorder and Bond. A ride close is answered by
//! a catalog re-feed rather than by a ride identity, and a bond removal by a link-status fact rather
//! than by a reply — and a domain that cannot validate a token cannot own an outcome (epic §4.3).
//! The residual drain asks for exactly those classes by name (`device_core::residual`), which is
//! what makes running it between two passes safe.

mod device_core_corpus;

use std::collections::BTreeSet;

use obc_app::ble::BondEffect;
use obc_app::catalog_state::{CatalogEffect, CatalogError, CatalogOutcome};
use obc_app::device_core::derived::{
    DerivedInput, DerivedInputs, DerivedNeeds, DerivedResult, DerivedTargets, NavPreviewKey, RideTrackKey,
};
use obc_app::device_core::storage_info::{StorageInfoEffect, StorageInfoOutcome};
use obc_app::device_core::{
    Capabilities, DeviceFacts, EffectSlots, ModeState, OutcomeSlots, PassClock, PassInputs, PassPlan, PlatformSupport,
    Revision, StoreIdentity, StoreRevision, TokenSource, TransferState,
};
use obc_app::dfu::{DfuEffect, DfuOutcome};
use obc_app::navigator::{NavigatorEffect, NavigatorError, NavigatorOutcome, PlannerWork};
use obc_app::recorder::RecorderEffect;
use obc_app::retention::{Retention, RetentionEffect, RetentionError, RetentionOutcome, RouteRetentionMeta};
use obc_app::screen::Screen;
use obc_app::settings::{SettingsEffect, SettingsOutcome};
use obc_app::weather::WeatherEffect;
use obc_app::{App, AppState, Gesture, HostCommand, HostMailbox, RideRetentionRecord, TrackAction, WarningFlags};
use obc_host_core::trace::{
    run_scenario_seeded, FeederCall, FeederKind, RunnerMode, Trace, TraceHarness, TraceInput, TraceRecorder,
    ALL_FEEDER_KINDS,
};
use obc_ports::{Fix, InputClock, LocationSource, RideClock, Sensors, SettingsSaveError};
use obc_route::NavError;

use device_core_corpus::{
    clock_watermark, definition, normalization_seed, visible_state, Action, CorpusState, PendingSettingsResult,
    Scenario, ScreenState, VisibleState, ALL_REQUIREMENTS, SCENARIOS, SETTINGS_FAILURE_RETRY_MS, TRIP,
};

// ==================== the runners ====================

/// One column of the conformance matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Runner {
    CoreImmediate,
    CoreDelayed,
}

impl Runner {
    const ALL: [Runner; 2] = [Runner::CoreImmediate, Runner::CoreDelayed];

    const fn name(self) -> &'static str {
        match self {
            Runner::CoreImmediate => "core-immediate",
            Runner::CoreDelayed => "core-delayed",
        }
    }

    /// When completed work is handed back. The scripted delay is deliberately uneven, so a runner
    /// that only ever sees one cadence cannot pass by accident.
    const fn mode(self) -> RunnerMode {
        match self {
            Runner::CoreImmediate => RunnerMode::Immediate,
            Runner::CoreDelayed => RunnerMode::ScriptedDelay(&[2, 0, 1]),
        }
    }

    /// Run one scenario, then let the runner settle, and report both.
    fn run(self, scenario: &Scenario) -> Run {
        let definition = definition(scenario);
        let mut harness = CoreHarness::new();
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

/// The rider-visible projection every runner must agree on.
///
/// `retention_delete_attempts` is dropped: it counts calls into `BorrowedRoutes::delete_by_id`,
/// including one for an id already gone, and an executor that reaches the store by identity counts
/// only the calls whose object is still catalogued — two counts of different events for the same
/// behaviour, and not something a rider can see.
fn rider_visible(mut state: VisibleState) -> VisibleState {
    state.retention_delete_attempts = 0;
    state
}

// ==================== the DeviceCore harness ====================

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
    Trips,
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
    /// A warning raised by an executor that has no domain to answer to — the recorder-finalize
    /// failure, which Recorder cannot report as an outcome until #1398.
    Warning(WarningFlags),
    /// The ride the recorder just finalized — answered by a catalog re-feed rather than by a
    /// terminal ride identity, because Recorder has no machine to validate one (#1398).
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
    /// The pass's own monotonic clock, at or above every mark the corpus's actions set.
    clock_ms: u32,
    /// The bounded polylines a derived answer carries beside its key.
    ride_preview: Vec<(i32, i32)>,
    nav_preview: Vec<(i32, i32)>,
    /// Effects the executor served, by domain.
    served: BTreeSet<&'static str>,
    /// Every settings revision the executor was asked to write, in order.
    settings_writes: Vec<u16>,
}

impl CoreHarness {
    fn new() -> Self {
        CoreHarness {
            state: CorpusState::new(),
            clock_ms: 0,
            ride_preview: Vec::new(),
            nav_preview: Vec::new(),
            served: BTreeSet::new(),
            settings_writes: Vec::new(),
        }
    }

    fn app(&mut self) -> &mut App {
        &mut self.state.app
    }

    /// One DeviceCore frame: whatever the executor handed back, then fourteen stages, then a plan.
    ///
    /// The clock moves one millisecond per pass — the corpus's actions drive the app's animation
    /// clock directly and time otherwise stands still, so a runner that ran the clock faster would
    /// age cards and idle timers, and the matrix would compare elapsed time rather than behaviour.
    /// The marks the actions do set are followed exactly (see [`clock_watermark`]).
    fn pass(&mut self) -> PassPlan {
        self.clock_ms += 1;
        let ride_preview = std::mem::take(&mut self.ride_preview);
        let nav_preview = std::mem::take(&mut self.nav_preview);
        let mut location = NoFix;
        let clock = PassClock { ride: RideClock(self.clock_ms), ui: InputClock(self.clock_ms) };
        let state = &mut self.state;
        // A derived answer is spent whether it was accepted or was about something else. Outcomes
        // and facts with no owner stay where the executor put them.
        let derived = std::mem::replace(&mut state.derived, DerivedInputs::NONE);
        state.app.run_pass(PassInputs {
            now: clock,
            gestures: &[],
            sensors: Sensors::new(&mut location),
            route: None,
            support: EVERYTHING,
            outcomes: &mut state.outcomes,
            facts: &mut state.facts,
            derived,
            targets: DerivedTargets { ride_preview: &ride_preview, nav_preview: &nav_preview },
        })
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
            self.state.settings_token = Some(token);
            self.settings_writes.push(revision);
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
                panic!("one request runs the whole search here; stepped pacing is #1400's — {effect:?}")
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
                self.state.store_revision += 1;
                Done::Catalog {
                    outcome: CatalogOutcome::CatalogRead { token, revision: Revision::new(self.state.store_revision) },
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
                // The folder's own object — the cascade's last step, decided by the domain and not
                // composed here.
                if self.state.trip_present && object == TRIP {
                    self.state.trip_present = false;
                    return Done::Catalog {
                        outcome: CatalogOutcome::ObjectRemoved { token, object, existed: true },
                        refeed: Refeed::Trips,
                    };
                }
                // The subject vanished before the commit — a success for the goal state, and the
                // one shape that must not read as a failure (epic §13).
                Done::Catalog {
                    outcome: CatalogOutcome::ObjectRemoved { token, object, existed: false },
                    refeed: Refeed::None,
                }
            }
        }
    }

    /// The sidecar writes. The fixture keeps no durable sidecar, so the answer *is* the write —
    /// what matters here is that it carries the operation's token back, which the fire-and-forget
    /// legacy stamp had no way to do.
    fn serve_retention(&mut self, effect: RetentionEffect) -> RetentionOutcome {
        match effect {
            RetentionEffect::WriteRouteMetadata { token, id, .. } => {
                RetentionOutcome::RouteMetadataWritten { token, id }
            }
            RetentionEffect::WriteRideMetadata { token, id, .. } => RetentionOutcome::RideMetadataWritten { token, id },
        }
    }

    // ---- the residual half, for the domains without a machine ----
    //
    // Recorder and Bond, and nothing else. The ride close is answered by a catalog re-feed rather
    // than by a ride identity (#1398) and the bond removal by a link-status fact rather than by a
    // reply (#1400). A domain that cannot validate a token cannot own an outcome (epic §4.3).

    /// Asked for **by name**: the two residual classes, and nothing else. A class DeviceCore owns
    /// is not filtered out of a full walk here — it is never drained, because the full walk *pulls*
    /// from each domain as it passes and would mint the operation the pass's own effect already
    /// carries. The harness is therefore unable to reach past its classes rather than asserting
    /// that it did not.
    fn serve_mailbox(&mut self, done: &mut Vec<Done>, trace: &mut TraceRecorder<VisibleState>) {
        let mut mail: HostMailbox = HostMailbox::new();
        let _ = self.state.app.drain_residual_commands(&mut mail);
        while let Some(command) = mail.pop() {
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
            self.state.answer_nav(|token| NavigatorOutcome::DetourCommitted { token, route: 10 });
        }
        // The answer carries the token of the write the executor is holding, so the domain checks
        // the operation *and* the revision — two independent guards (#810). The token is read
        // **first**: with no write in flight there is nothing for this script to answer, and
        // `scripted_settings` consumes the script, so asking it before knowing that would spend the
        // scenario's only ack on a pass that could not deliver it.
        let Some(token) = self.state.settings_token else { return };
        let Some((revision, failed)) = self.scripted_settings(persisted) else { return };
        done.push(if failed {
            Done::Settings(SettingsOutcome::PersistFailed { token, revision, error: SettingsSaveError::Backend })
        } else {
            Done::Settings(SettingsOutcome::Persisted { token, revision })
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
        let _ = trace;
        match command {
            // The ride close: still a legacy command, because Recorder has no machine — and the
            // pass deliberately leaves the rider's one-shot here rather than taking it somewhere it
            // cannot be acted on.
            HostCommand::FinishTrack(TrackAction::Save) => {
                if std::mem::take(&mut self.state.fail_next_finalize) {
                    // Recorder has no outcome to validate, so the failure reaches the rider as the
                    // generic warning a host raises — the limitation #1398 clears.
                    done.push(Done::Warning(WarningFlags::REC_ERROR));
                } else {
                    done.push(Done::RideSaved);
                }
            }
            HostCommand::FinishTrack(TrackAction::Discard) => {}
            // The bond removal is confirmed by a link fact rather than by a reply, so there is
            // nothing to answer here.
            HostCommand::ForgetBond => {}
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
        self.serve_typed(&mut plan.effects, &mut done);
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
            Refeed::Trips => self.state.feed_trips("core.trips", trace),
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
        self.serve_typed(&mut plan.effects, &mut done);
        self.serve_mailbox(&mut done, trace);
        self.serve_derived(&plan.derived_needs, &mut done);
        done
    }

    fn deliver(&mut self, done: Self::Outcome, trace: &mut TraceRecorder<Self::State>) {
        match done {
            Done::Catalog { outcome, refeed } => {
                self.refeed(refeed, trace);
                let _ = self.state.outcomes.catalog.try_put(outcome);
            }
            Done::Retention(outcome) => {
                let _ = self.state.outcomes.retention.try_put(outcome);
            }
            Done::Navigator(outcome) => {
                let _ = self.state.outcomes.navigator.try_put(outcome);
            }
            Done::Settings(outcome) => {
                if matches!(outcome, SettingsOutcome::PersistFailed { .. }) && self.state.settings_retry_requested {
                    self.clock_ms = self.clock_ms.max(SETTINGS_FAILURE_RETRY_MS);
                }
                let _ = self.state.outcomes.settings.try_put(outcome);
            }
            Done::Dfu(outcome) => {
                let _ = self.state.outcomes.dfu.try_put(outcome);
            }
            Done::Storage(outcome) => {
                let _ = self.state.outcomes.storage_info.try_put(outcome);
            }
            Done::Warning(flags) => self.state.facts.raise_warnings(flags),
            Done::RideSaved => self.state.feed_rides("core.recorder-saved", trace),
            // A keyed ride-track answer fills two resident buffers — the elevation profile and the
            // preview polyline — and a nav-preview answer fills one. They are recorded as the bulk
            // feeder calls they are: the data still crosses the seam, it just carries a key now
            // instead of arriving on a `set_*` method of its own.
            Done::RideTrack(input) => {
                self.ride_preview = vec![(0, 0), (1, 1)];
                trace.record_feeder(FeederCall::new(FeederKind::RideProfile, "core.ride-track", 1));
                trace.record_feeder(FeederCall::new(
                    FeederKind::RidePreview,
                    "core.ride-track",
                    self.ride_preview.len(),
                ));
                self.state.derived.ride_track = Some(input);
            }
            Done::NavPreview(input) => {
                self.nav_preview = vec![(0, 0), (1, 1)];
                trace.record_feeder(FeederCall::new(
                    FeederKind::NavPreview,
                    "core.nav-preview",
                    self.nav_preview.len(),
                ));
                self.state.derived.nav_preview = Some(input);
            }
        }
    }
}

// ==================== the matrix ====================

/// Every applicable DC1 scenario, through every runner.
///
/// The gate: every runner reaches the same rider-visible terminal state. There is no disposition
/// table any more and no way to record a difference and move on — the last three approved cells were
/// the compatibility executor's (a namespace-free `RemoveObject` the adapter could not express, and
/// the retention expiry that stayed in flight behind it), and both sides of that comparison retired
/// with the adapter.
#[test]
fn every_scenario_agrees_in_every_runner() {
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
    assert!(differing.is_empty(), "the runners disagree, and nothing may ship over that: {differing:?}");
}

/// **The DC1 checklist is still a checklist.** Every behaviour row #1434 locked is claimed by at
/// least one scenario, the table's names are stable unique trace keys, and no scenario is empty.
///
/// This gate lived in `device_core_legacy_traces.rs` and is corpus-side, not runner-side: it is a
/// property of `SCENARIOS` × [`ALL_REQUIREMENTS`] and has nothing to do with which frame runs them.
/// It is here because the file that held it retired with the legacy runner it was written against.
#[test]
fn every_dc1_requirement_is_claimed_by_a_scenario() {
    let names: BTreeSet<_> = SCENARIOS.iter().map(|scenario| scenario.name).collect();
    assert_eq!(names.len(), SCENARIOS.len(), "scenario names are stable unique trace keys");
    assert!(SCENARIOS.iter().all(|scenario| !scenario.actions.is_empty()), "a scenario with no actions runs nothing");

    let covered: BTreeSet<_> = SCENARIOS.iter().flat_map(|scenario| scenario.requirements.iter().copied()).collect();
    let required: BTreeSet<_> = ALL_REQUIREMENTS.iter().copied().collect();
    assert_eq!(covered, required, "every DC1 behaviour row must be claimed by a scenario");
}

/// **Every bulk feeder is exercised by a real call.** The feeders outlived the protocol — a typed
/// executor fills the resident catalogs through them — so the loop that proves the corpus reaches
/// all of them has to outlive it too, and it moves here from the retired legacy traces (#1516's
/// open question 5). A feeder no scenario touches is a seam nothing is checking.
#[test]
fn every_feeder_kind_is_exercised_by_a_real_call() {
    let mut seen: BTreeSet<FeederKind> = BTreeSet::new();
    for scenario in SCENARIOS {
        for runner in Runner::ALL {
            let trace = runner.run(scenario).trace;
            seen.extend(trace.steps.iter().flat_map(|step| &step.feeder_calls).map(|call| call.feeder));
        }
    }
    assert_eq!(seen, ALL_FEEDER_KINDS.into_iter().collect(), "every feeder must be reached by a real call");
}

/// **The upload burst reaches the rider in order.** `Requirement::CatalogUploadOrder`'s claim is that
/// each member route's commit is announced, each popup replacing the last, and that the trip object —
/// which always lands after its members — replaces the final route popup with one card.
///
/// The upload fact slot is single and most-recent-wins, so this is only observable when each commit
/// gets its own pass: two uploads reported before one pass consumes the slot leave the first
/// unobserved, and the *order* untested. That is why the burst is three scenario actions.
#[test]
fn each_upload_in_a_burst_reaches_the_rider_and_the_trip_card_lands_last() {
    for runner in Runner::ALL {
        let trace = runner.run(named("catalog.refresh-upload-remap")).trace;
        let screens: Vec<ScreenState> = trace.steps.iter().map(|step| step.visible_state.screen).collect();
        let received = screens.iter().filter(|s| **s == ScreenState::Other("RouteReceived")).count();
        assert_eq!(received, 2, "{}: both member routes were announced, not just the last: {screens:?}", runner.name());
        let trip = screens.iter().rposition(|s| *s == ScreenState::Other("TripReceived"));
        let last_route = screens.iter().rposition(|s| *s == ScreenState::Other("RouteReceived"));
        assert!(
            matches!((trip, last_route), (Some(trip), Some(route)) if trip > route),
            "{}: the trip card replaces the burst's last route popup: {screens:?}",
            runner.name()
        );
    }
}

fn named(name: &'static str) -> &'static Scenario {
    SCENARIOS.iter().find(|scenario| scenario.name == name).unwrap_or_else(|| panic!("no scenario {name}"))
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
        test: "a_trip_member_that_vanished_before_the_commit_is_a_success",
        substitution: None,
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
        substituted, 0,
        "the store-refresh row became literal at #1397 S6a and the trip cascade at #1491 — every \
         row now runs the situation it names"
    );
}

fn typed() -> CoreHarness {
    CoreHarness::new()
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
    let _ = harness.state.outcomes.catalog.try_put(outcome);
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
        harness.state.outcomes.navigator.try_put(NavigatorOutcome::PlanFinished { token: effect.token(), route: 10 });
    harness.pass();
    assert_eq!(harness.state.app.active_route_index(), None, "the cancelled plan adopted nothing");
    assert!(
        !matches!(harness.state.app.top_screen(), Screen::RouteOverview(_)),
        "and the rider is not shown an overview for a route they cancelled"
    );

    // And it was *delivered*, not swallowed on the way: the slot the executor put it in is empty
    // after the pass, so what refused it is the domain and not the plumbing losing it.
    assert!(harness.state.outcomes.navigator.is_empty(), "the pass took the answer and refused it");
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
    let _ = harness.state.outcomes.catalog.try_put(CatalogOutcome::ObjectRemoved {
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
    harness.state.facts.note_store_revision(StoreRevision { store: StoreIdentity::new(1), revision: Revision::new(4) });
    let plan = harness.pass();
    assert!(plan.effects.catalog.is_empty(), "one catalog operation at a time");

    // The same revision again is the same edge, not a second one: the refresh is owed once.
    harness.state.facts.note_store_revision(StoreRevision { store: StoreIdentity::new(1), revision: Revision::new(4) });
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
    let mut harness = typed();
    assert!(harness.app().debug_start_nav((0, 0), (1_000, 1_000), "goal"), "the plan is admitted");
    let mut plan = harness.pass();
    let running = plan.effects.navigator.take().expect("the planner was asked").token();

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
    let mut fixture = expiring(0);
    fixture.state.facts.note_transfer(TransferState::Active);
    let mut plan = fixture.pass();
    assert!(plan.effects.navigator.is_empty(), "no plan is started while the transfer holds the store");
    assert!(plan.effects.dfu.is_empty(), "nor an install");
    assert!(plan.effects.catalog.is_empty() && plan.effects.retention.is_empty(), "nor a store operation");
    // The settings write is not heavy and is not withdrawn: the trusted-clock stamp this fixture
    // makes is a rider edit like any other, and a transfer holding the *store* has nothing to say
    // about a settings revision. That distinction is what `Capabilities` is for — asserted, not
    // merely tolerated, so a settings write that quietly stopped going out fails here.
    assert!(plan.effects.settings.take().is_some(), "the settings write is unaffected by the transfer");

    // The plan already running is untouched by the withdrawal: its answer still lands.
    harness.state.facts.note_transfer(TransferState::Active);
    harness.pass();
    let _ = harness.state.outcomes.navigator.try_put(NavigatorOutcome::PlanFinished { token: running, route: 10 });
    harness.pass();
    assert!(
        matches!(harness.state.app.top_screen(), Screen::RouteOverview(_)),
        "a withdrawn capability never cancelled the running plan — its answer still lands"
    );

    // The withdrawal above is derived from one level, so the fixture is what has to show that level
    // arriving — and the pure `Capabilities` verdicts asserted at the top are what it means.
    assert_eq!(
        fixture.state.app.core_mode(),
        ModeState::Transferring,
        "the fact reached the level the withdrawal reads"
    );

    // …and the capability comes straight back when the transfer ends. This is the restoration claim,
    // so it is asserted on the fixture after the fact *and* the pass that consumes it: recomputing
    // `Capabilities::calculate` here would depend on neither.
    fixture.state.facts.note_transfer(TransferState::Idle);
    fixture.pass();
    assert_eq!(fixture.state.app.core_mode(), ModeState::Free, "the transfer ended, so heavy work is admissible again");
}

/// **A route plan completes after the active route changed.** The answer carries the token the
/// request went out with; a Navigator that has moved on refuses it.
#[test]
fn a_route_plan_that_lands_after_the_active_route_changed_is_refused() {
    let mut harness = typed();
    assert!(harness.app().debug_start_nav((0, 0), (1_000, 1_000), "first"), "the first plan is admitted");
    let mut plan = harness.pass();
    let first = plan.effects.navigator.take().expect("and goes out as one operation").token();

    // The rider walks away and asks again: Navigator replaces its operation.
    harness.app().apply_gesture(Gesture::Back);
    harness.pass();
    assert!(harness.app().debug_start_nav((0, 0), (2_000, 2_000), "second"), "the replacement is admitted");
    let mut plan = harness.pass();
    let second = plan.effects.navigator.take().expect("and goes out too").token();
    assert_ne!(first, second, "a replacement is a different operation");

    // The first plan's answer arrives late, carrying the token it went out with.
    let _ = harness.state.outcomes.navigator.try_put(NavigatorOutcome::PlanFinished { token: first, route: 10 });
    harness.pass();
    assert_eq!(harness.state.app.active_route_index(), None, "the superseded answer adopted nothing");
    assert!(
        matches!(harness.state.app.top_screen(), Screen::NavPlanning(_)),
        "and the replacement is still the plan the rider is waiting on"
    );
}

/// **A settings result with an old revision.** The token and the revision are independent guards,
/// and what a *host* can observe of them is what this row pins: the rider's second edit is written
/// after the first write's stale answer lands, rather than being swallowed by it.
///
/// The token guard's own refusal is [`an_outcome_after_cancellation_changes_nothing`], where a
/// superseded answer visibly adopts nothing. The revision guard sits behind it — a superseding edit
/// leaves `Awaiting` for `Dirty` before any stale answer can arrive — so it is belt-and-braces and
/// is pinned in `obc-app`'s own suite rather than claimed here.
#[test]
fn a_settings_result_with_an_old_revision_is_refused() {
    // The rider edits, the write goes out, the rider edits again, and the *first* write's answer
    // lands — for a superseded revision. What must not happen is the second edit being lost, so the
    // observation is the sequence of revisions the executor was actually asked to write.
    let mut harness = typed();
    let mut trace = recorder();
    for action in [Action::DirtySettings, Action::PersistSettings, Action::DeliverStaleSettingsResult] {
        harness.apply(action);
        let plan = harness.pass();
        harness.serve(plan, &mut trace);
    }
    for _ in 0..SETTLE_PASSES {
        let plan = harness.pass();
        harness.serve(plan, &mut trace);
    }
    assert_eq!(
        harness.settings_writes,
        [1, 2],
        "the rider's second edit is written after the first one's stale answer, not swallowed by it"
    );

    // …and the whole row settles the same way at both answer cadences.
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
        matches!(done.as_slice(), [Done::Warning(flags)] if flags.contains(WarningFlags::REC_ERROR)),
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
}

/// **A trip member disappears before the delete commit.** The rider holds to delete a folder, and
/// one of its member routes is gone by the time that member's removal commits. The goal state holds,
/// so the step is a success with `existed: false` — never a failure the rider is shown — and the
/// cascade walks on to the remaining member and then the folder.
///
/// The literal situation, on the domain's own cascade (#1491). The generic rule — an object already
/// absent is a success — is the trace below it.
#[test]
fn a_trip_member_that_vanished_before_the_commit_is_a_success() {
    let mut harness = typed();
    harness.apply(Action::CascadeDeleteTrip);

    // The first member's removal is decided; something else takes the file first.
    let first = harness.next_catalog_effect();
    let CatalogEffect::RemoveObject { object: member, .. } = first else { panic!("a removal") };
    assert!(harness.state.trip_stage_ids.contains(&member), "the cascade starts with a member, not the folder");
    let index = harness.state.route_ids.iter().position(|&id| id == member).expect("still catalogued");
    harness.state.routes.remove(index);
    harness.state.route_ids.remove(index);

    let outcome = harness.answer_catalog(first);
    assert_eq!(
        outcome,
        CatalogOutcome::ObjectRemoved { token: first.token(), object: member, existed: false },
        "a member that vanished first is a success for the goal state"
    );

    // The walk is unaffected: the remaining member, then the folder itself.
    let mut removed = vec![member];
    for _ in 0..2 {
        let effect = harness.next_catalog_effect();
        let CatalogEffect::RemoveObject { object, .. } = effect else { panic!("a removal") };
        removed.push(object);
        harness.answer_catalog(effect);
    }
    assert_eq!(removed.last(), Some(&TRIP), "the folder is removed last, after every member had its turn");
    assert!(!harness.state.trip_present, "and the folder is gone from the store");
    assert!(harness.state.app.trips().is_empty(), "and from the rider's Route menu");

    let plan = harness.pass();
    assert!(plan.effects.catalog.is_empty(), "the cascade is over — nothing retried, nothing failed");
}

/// An object that vanished before its removal commits is a success with `existed: false`, whichever
/// delete decided it — here the retention sweep's.
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
    // A supported device asks the planner for real, and its "no path" is an *answer* to that
    // operation: the rider is shown the generic failure card, and not the range tier.
    let mut harness = typed();
    harness.apply(Action::PlanDetour);
    let mut plan = harness.pass();
    let effect = plan.effects.navigator.take().expect("the detour search leaves as one operation");
    assert!(
        matches!(effect, NavigatorEffect::Acquire { work: PlannerWork::Detour(_), .. }),
        "and it is a detour search: {effect:?}"
    );
    let _ = harness
        .state
        .outcomes
        .navigator
        .try_put(NavigatorOutcome::Failed { token: effect.token(), error: NavigatorError::Plan(NavError::NoPath) });
    harness.pass();
    match harness.state.app.top_screen() {
        Screen::NavFail(card) => {
            assert!(!card.shows_too_far(), "NoPath is the generic tier — the range tier is Exhausted");
        }
        other => panic!("the planner's failure must reach the rider, not {}", other.name()),
    }

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
    harness.state.derived.ride_track = Some(DerivedInput { key: other, result: DerivedResult::Filled });
    let plan = harness.pass();
    assert_eq!(plan.derived_needs.ride_track, Some(key), "the need is untouched");

    // A failure for the *right* key is an answer, so a dead source costs one read and not one per
    // pass.
    harness.state.derived.ride_track = Some(DerivedInput { key, result: DerivedResult::Failed });
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
    harness.state.facts.raise_warnings(WarningFlags::NO_GPS);
    harness.state.facts.raise_warnings(WarningFlags::MAP_SLOW);
    harness.pass();
    assert!(
        matches!(harness.state.app.top_screen(), Screen::Warning(card)
            if card.flags().contains(WarningFlags::NO_GPS) && card.flags().contains(WarningFlags::MAP_SLOW)),
        "both notices reached one card"
    );
}

/// A failed sidecar write re-queues its candidate — the retry the legacy protocol cannot express at
/// all, because a fire-and-forget legacy stamp never acknowledged one.
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
        .state
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
/// back on every pass after the executor answers it. The pass mirrors a decided stamp into the full
/// ride inventory as well as the display catalog, because a ride outside the newest-32 menu
/// re-enqueues just the same (finding #876-2).
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
        {
            let mut harness = CoreHarness::new();
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
/// The two gating figures — 185 passes and 3 immediate wakes — are the claim that matters: nothing
/// here polls. Both are exactly the typed executor's own contribution to the pre-S6c figures; what
/// halved is the runner count, not the work per pass.
///
/// The upload burst is three actions rather than one — the upload slot is single and
/// most-recent-wins, so reporting two uploads before a pass consumes it left the first unobserved.
/// The two extra passes are two extra upload-popup frames, each arming that popup's 30 s auto-close:
/// +2 passes, +2 timed, immediate and sleep unchanged.
///
/// Three further cells moved with S6c's corpus port, and none of them is a pass decision:
/// `catalog.refresh-upload-remap` gained an immediate wake because a store commit is now reported as
/// a *revision fact*, which raises a refresh intent the next pass consumes;
/// `navigation.plan-cancel-late-replacement` lost one because a late answer now carries the
/// abandoned operation's token and Navigator simply refuses it, leaving nothing deferred; and
/// `recorder.failure-and-session-replacement` turned one sleep into a timed wake because the
/// finalize failure arrives as a warning fact the next pass consumes, so the card's 30 s timeout
/// arms one pass later.
const WAKE_PROFILE: (u32, u32, u32, u32) = (185, 3, 122, 60);

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
