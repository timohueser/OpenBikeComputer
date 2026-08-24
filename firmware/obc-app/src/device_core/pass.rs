//! The **pass** — DeviceCore's one deterministic frame (#1438, epic #1433 §6).
//!
//! One entry point, [`App::run_pass`], runs fourteen named stages **once each, in a fixed order**,
//! and returns a bounded [`PassPlan`]. There is no loop that drains mailboxes until they empty, no
//! re-entry, and no component that calls another: the same inputs produce the same stages, the same
//! deliveries and the same plan, on the board, in the simulator and in the web demo.
//!
//! ## The order, and why it is this order
//!
//! | # | Stage | What it is for |
//! |---|---|---|
//! | 1 | [`Outcomes`](PassStage::Outcomes) | Answers to work *we* asked for come first: a domain must know what completed before it decides anything else. |
//! | 2 | [`Facts`](PassStage::Facts) | What changed underneath us that nobody asked for, plus the keyed derived reads that answer a need. |
//! | 3 | [`Input`](PassStage::Input) | The rider and the world: gestures, sensors, time. |
//! | 4 | [`Ui`](PassStage::Ui) | `UiRuntime` advances and turns what the rider did into **typed intents** for their owning domain. It runs before every domain so an intent lands in the same pass. |
//! | 5 | [`Retention`](PassStage::Retention) | Before the catalog, so an expiry it decides reaches `CatalogMachine` in this pass rather than the next. |
//! | 6 | [`Catalog`](PassStage::Catalog) | Before Navigator, so a route being deleted reaches the component following it in this pass. |
//! | 7 | [`Recorder`](PassStage::Recorder) | |
//! | 8 | [`Navigator`](PassStage::Navigator) | |
//! | 9 | [`Settings`](PassStage::Settings) | |
//! | 10 | [`Weather`](PassStage::Weather) | |
//! | 11 | [`Platform`](PassStage::Platform) | DFU, bond and storage information — the domains whose work is purely physical. |
//! | 12 | [`Admission`](PassStage::Admission) | `CoreMode` recalculates what this device can do at all, from what the platform implements and what is currently true. |
//! | 13 | [`Faults`](PassStage::Faults) | Every domain has spoken, so every fault notice raised this pass reaches the rider together. |
//! | 14 | [`Plan`](PassStage::Plan) | What the executor must do: render, wake, read, and the bounded effects. |
//!
//! ## The two delivery rules
//!
//! Both follow from the order rather than from a policy each connection chooses:
//!
//! - **Earlier → later is same-pass.** The producer fills a named slot; the consumer's stage takes
//!   it a few stages on.
//! - **Later → earlier is next-pass.** It cannot reach backwards, so it waits in a
//!   [`Deferred`](super::connections::Deferred) slot, which
//!   [`promote_deferred`](super::connections::Connections::promote_deferred) makes visible at the
//!   top of the next pass — *before* any new gesture, sensor reading or fact. A deferred value
//!   still in flight at the end of a pass folds into an **immediate** next wake, so decided work
//!   never waits for the next rider input.
//!
//! [`connections`](super::connections) lists every connection, its type, its capacity and its merge
//! rule.
//!
//! ## What is deliberately not here
//!
//! **Hold cancellation.** A stack change invalidates a hold that is charging *right now* on the
//! board's high-priority input plane, and that plane runs between passes. It stays the direct
//! one-shot latch [`App::take_hold_cancel`], drained by the board before it cancels its recognizer.
//! Routing it through a plan the board reads at the end of a pass would make a rider's finger wait
//! for a frame — see [`host`](crate::host) for the seam.
//!
//! **Callbacks.** Nothing in [`PassInputs`] can call back into DeviceCore: the sensor ports are
//! *pull* ports that return values and hold no path to the `App`, and the two push doors an
//! executor answers through ([`App::apply_event`], [`App::apply_derived`]) refuse while a pass is in
//! flight. The next pass consumes what an executor completed; nothing mutates mid-pass.
//!
//! ## What this slice does not yet do
//!
//! Three domains own an operation token today — retention, weather and the catalog — and those are
//! exactly the three whose outcomes a pass may consume: **a domain that cannot validate a token
//! cannot be the owner of an outcome** (epic §4.3). An outcome for a domain whose state machine has
//! not landed is therefore *left in its slot*, not dropped and not guessed at. The stage where that
//! machine will advance already exists and already runs, because the order is what this slice pins.

// The pass has no production caller yet, by design: #1438 pins the order and the delivery rules,
// DC6 #1439's compatibility adapter is its first caller, and #1397 S6 makes it the hosts' entry
// point and deletes the frame methods it replaces. Until then the tests below are what exercise it.
#![allow(dead_code)]

use obc_ports::{InputClock, RideClock, Sensors};
use obc_route::RouteReader;

use crate::catalog_state::CatalogIntent;
use crate::device_core::connections::{ActiveRouteRemoved, CatalogIdentityChanged, RideClosed, RouteActivated};
use crate::dirty::Dirty;
use crate::input::Gesture;
use crate::recorder::RecorderIntent;
use crate::retention::SweepKind;
use crate::App;

use super::connections::Connections;
use super::derived::{DerivedInputs, DerivedNeeds, DerivedTargets};
use super::{
    Capabilities, DeviceFacts, EffectSlots, ExternalFacts, OutcomeSlots, PlatformSupport, Revision, SlotFull,
    StoreRevision, TransferState, UpdateResult,
};

/// The fourteen stages, in the order [`App::run_pass`] runs them. Each runs exactly once, and each
/// advances exactly one component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassStage {
    /// Validate and consume each domain's outcome slot.
    Outcomes,
    /// Consume external facts and the matching keyed derived inputs.
    Facts,
    /// Apply gestures, sensors and time.
    Input,
    /// Advance `UiRuntime` and collect the typed intents it produced.
    Ui,
    /// Advance `RetentionMachine`.
    Retention,
    /// Advance `CatalogMachine`.
    Catalog,
    /// Advance `Recorder`.
    Recorder,
    /// Advance `Navigator`.
    Navigator,
    /// Advance `SettingsMachine`.
    Settings,
    /// Advance `WeatherDomain`.
    Weather,
    /// Advance `DfuState`, `BondState` and `StorageInfo`.
    Platform,
    /// Admit heavy work through `CoreMode` and recalculate [`Capabilities`].
    Admission,
    /// Advance `FaultState`.
    Faults,
    /// Calculate render work, needs, effects and the next wake.
    Plan,
}

impl PassStage {
    /// The fixed order, as one value — what the order test compares against, and the only place the
    /// sequence is written down besides [`App::run_pass`] itself.
    pub const ORDER: [PassStage; 14] = [
        PassStage::Outcomes,
        PassStage::Facts,
        PassStage::Input,
        PassStage::Ui,
        PassStage::Retention,
        PassStage::Catalog,
        PassStage::Recorder,
        PassStage::Navigator,
        PassStage::Settings,
        PassStage::Weather,
        PassStage::Platform,
        PassStage::Admission,
        PassStage::Faults,
        PassStage::Plan,
    ];
}

/// The pass's two clocks. They are the same value on the board (one monotonic `now` drives the whole
/// loop) and differ in the simulator, where [`ride`](Self::ride) is GPX-playback time and
/// [`ui`](Self::ui) is wall time — so a replayed ride's moving time is not scaled by the replay
/// speed while a hold still charges in real seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PassClock {
    /// Fix-consistent millis: ride accumulators, sensor freshness, the recorded track.
    pub ride: RideClock,
    /// Monotonic wall millis: holds, animations, idle return, the next wake.
    pub ui: InputClock,
}

/// Everything one pass reads. Values and pull-only ports — nothing here can reach back into
/// DeviceCore while the pass runs.
///
/// [`outcomes`](Self::outcomes) and [`facts`](Self::facts) are borrowed rather than owned so the
/// pass can consume *what it has an owner for* and leave the rest where the executor put it: a
/// value with no owner is never silently dropped.
pub struct PassInputs<'a> {
    /// This pass's clocks.
    pub now: PassClock,
    /// The gestures recognised since the last pass, in the order they happened.
    pub gestures: &'a [Gesture],
    /// The platform's sensor ports.
    pub sensors: Sensors<'a>,
    /// The active route's reader, when the platform has one open.
    pub route: Option<&'a RouteReader<'a>>,
    /// What this firmware image and its hardware implement at all — constant for a boot.
    pub support: PlatformSupport,
    /// What the platform finished since the last pass.
    pub outcomes: &'a mut OutcomeSlots,
    /// What changed underneath DeviceCore that nobody asked for.
    pub facts: &'a mut ExternalFacts,
    /// Keyed answers to the derived needs of the previous plan.
    pub derived: DerivedInputs,
    /// The bounded polylines a derived answer carries beside its key.
    pub targets: DerivedTargets<'a>,
}

/// What the platform must read for the frame the pass just planned. A *level*, recalculated every
/// pass: a host that cannot open a source simply sees the need again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SourceNeeds {
    /// The map reader — the base screen draws the map.
    pub map: bool,
    /// The active route's reader — a route is loaded.
    pub route: bool,
}

/// What one pass decided: the render work, when to come back, what to read, and the bounded
/// physical work per domain.
#[derive(Debug, PartialEq, Eq)]
pub struct PassPlan {
    /// Which display planes changed.
    pub render: Dirty,
    /// Millis until the pass must run again, or `None` to sleep until an event. `Some(0)` when
    /// [`immediate`](Self::immediate) holds.
    pub next_wake_ms: Option<u32>,
    /// The keyed derived reads DeviceCore still needs.
    pub derived_needs: DerivedNeeds,
    /// The sources the next frame needs open.
    pub sources: SourceNeeds,
    /// One bounded operation per domain.
    pub effects: EffectSlots,
    /// A later-to-earlier value is waiting: run another pass **before sleeping**. The work is
    /// already decided; it has simply not reached its consumer yet.
    pub immediate: bool,
}

/// The coordinator's own resident state: the connections, the levels a stage compares against to
/// find an edge, the current capabilities, and the re-entrancy guard.
///
/// Deliberately small and deliberately *not* domain state — nothing here decides a product rule.
/// Each field is either a wire ([`Connections`]) or the previous value of something a stage must
/// detect a change in.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct PassState {
    /// Every cross-domain connection.
    pub(crate) connections: Connections,
    /// The newest store revision seen — the level stage 2 detects a commit against.
    store: Option<StoreRevision>,
    /// The store revision the catalog last announced to retention, so one commit is announced once.
    announced: Option<Revision>,
    /// The newest link state seen, so an unchanged level does not re-run the link's card sweep.
    link: Option<crate::ble::BleStatus>,
    /// Whether a bulk transfer is streaming — `CoreMode`'s heavy-work verdict rests on it.
    transfer: TransferState,
    /// The active route's durable identity as of the last pass — Navigator's activation edge.
    active_route: Option<crate::CatalogObjectId>,
    /// What the device can currently do, recalculated at stage 12.
    capabilities: Capabilities,
    /// Whether a pass is running. The push doors refuse while it is set.
    in_pass: bool,
    /// The stages this pass ran, in order.
    #[cfg(test)]
    trace: heapless::Vec<PassStage, 16>,
}

impl PassState {
    /// The boot state: nothing wired, no level seen, no capability.
    pub(crate) const fn new() -> Self {
        PassState {
            connections: Connections::new(),
            store: None,
            announced: None,
            link: None,
            transfer: TransferState::Idle,
            active_route: None,
            capabilities: Capabilities::NONE,
            in_pass: false,
            #[cfg(test)]
            trace: heapless::Vec::new(),
        }
    }

    /// Whether a pass is currently running — the guard the push doors read.
    pub(crate) fn in_pass(&self) -> bool {
        self.in_pass
    }

    /// Open a pass. Re-entry is a caller bug: a platform callback that reached here would be
    /// mutating DeviceCore in the middle of one.
    fn enter(&mut self) {
        debug_assert!(!self.in_pass, "a pass cannot run inside a pass");
        self.in_pass = true;
        #[cfg(test)]
        self.trace.clear();
    }

    /// Close the pass.
    fn leave(&mut self) {
        self.in_pass = false;
    }

    /// Note that a stage ran. Free outside tests: the recorder exists so the *order* can be
    /// asserted, not to be read at runtime.
    #[inline]
    fn record(&mut self, stage: PassStage) {
        #[cfg(test)]
        self.trace.push(stage).expect("the trace holds every stage of one pass");
        #[cfg(not(test))]
        let _ = stage;
    }
}

// Layout tripwire: the coordinator's own state is wires and levels — a growth here means domain
// state drifted into the sequencer.
const _: () = assert!(core::mem::size_of::<PassState>() <= 288, "connections, a few levels, the test recorder");

impl App {
    /// Run one DeviceCore pass: fourteen stages, once each, in [`PassStage::ORDER`].
    ///
    /// The whole product frame in one call — what completed, what changed, what the rider did, and
    /// what every domain decides about it — returning the bounded work the platform must perform.
    ///
    /// Private to `obc-app` for Phase 1. The existing frame methods stay the hosts' entry points
    /// until #1397 S6 migrates them; both compositions call the same per-domain entry points, so
    /// there is one implementation of each and only the order differs.
    pub(crate) fn run_pass(&mut self, inputs: PassInputs<'_>) -> PassPlan {
        let PassInputs { now, gestures, sensors, route, support, outcomes, facts, derived, targets } = inputs;
        self.pass.enter();
        // The previous pass's later-to-earlier deposits become visible here — before any new
        // outcome, fact or gesture, so an earlier component acts on them ahead of new user input.
        self.pass.connections.promote_deferred();

        self.stage_outcomes(outcomes);
        self.stage_facts(facts, derived, targets);
        self.stage_input(now, gestures, sensors, route);
        self.stage_ui(now);

        let mut effects = EffectSlots::new();
        self.stage_retention(&mut effects);
        self.stage_catalog(&mut effects);
        self.stage_recorder();
        self.stage_navigator();
        self.stage_settings();
        self.stage_weather(&mut effects);
        self.stage_platform();
        self.stage_admission(support);
        self.stage_faults();
        let plan = self.stage_plan(now, effects);

        self.pass.leave();
        plan
    }

    /// Stage 1 — validate and consume each domain's outcome slot.
    ///
    /// A domain accepts an outcome only while its own [`OperationToken`](super::OperationToken) is
    /// current, which is why only a domain that *owns a token source* may consume one. The rest
    /// stay in their slots: an outcome nobody can validate is not something to guess at, and the
    /// slot's capacity of one is the executor's backpressure until the owner lands.
    fn stage_outcomes(&mut self, outcomes: &mut OutcomeSlots) {
        self.pass.record(PassStage::Outcomes);
        if let Some(outcome) = outcomes.catalog.take() {
            self.catalogs.apply_outcome(outcome);
        }
        if let Some(outcome) = outcomes.retention.take() {
            self.retention.apply_outcome(outcome);
        }
        if let Some(outcome) = outcomes.weather.take() {
            self.weather.apply_outcome(outcome);
        }
    }

    /// Stage 2 — consume external facts and the derived inputs that answer a need.
    ///
    /// Levels ([`store_revision`](ExternalFacts::store_revision), transfer, link, installed weather)
    /// are read and compared against what the coordinator last saw, so one commit is one cue. The
    /// one-shots (uploads, warnings, this boot's update result) are taken. A warning goes to the
    /// fault connection rather than straight to a card: every fault raised in a pass reaches the
    /// rider together at stage 13.
    fn stage_facts(&mut self, facts: &mut ExternalFacts, derived: DerivedInputs, targets: DerivedTargets<'_>) {
        self.pass.record(PassStage::Facts);
        if let Some(store) = facts.store_revision() {
            if self.pass.store != Some(store) {
                self.pass.store = Some(store);
                self.host.note_store_changed();
            }
        }
        if let Some(state) = facts.transfer() {
            self.pass.transfer = state;
        }
        if let Some(link) = facts.link() {
            if self.pass.link != Some(link) {
                self.pass.link = Some(link);
                self.set_ble_status(link);
            }
        }
        if let Some(installed) = facts.weather_data() {
            self.weather.note_installed(installed);
        }
        if let Some(upload) = facts.take_route_upload() {
            self.on_route_uploaded(upload.id, upload.replaced, upload.elevation);
        }
        if let Some(upload) = facts.take_trip_upload() {
            self.on_trip_uploaded(upload.id, upload.replaced);
        }
        let warnings = facts.take_warnings();
        if !warnings.is_empty() {
            self.pass.connections.faults.raise(warnings);
        }
        if let Some(result) = facts.take_update_result() {
            let update = match result {
                UpdateResult::Confirmed(version) => crate::card_scheduler::BootUpdate::Confirmed(version),
                UpdateResult::Failed { why, staged } => crate::card_scheduler::BootUpdate::Failed(why, staged),
            };
            self.post_boot_update(update);
        }
        self.accept_derived(derived, targets);
    }

    /// Stage 3 — apply gestures, sensors and time.
    ///
    /// Gestures land in the order they were recognised. The hold-cancel latch a stack change arms is
    /// deliberately **not** drained here: it belongs to the board's input plane, which drains it
    /// between passes (see the module docs).
    fn stage_input(
        &mut self,
        now: PassClock,
        gestures: &[Gesture],
        sensors: Sensors<'_>,
        route: Option<&RouteReader<'_>>,
    ) {
        self.pass.record(PassStage::Input);
        self.ui.now_ms = now.ui.0;
        for &gesture in gestures {
            self.apply_gesture(gesture);
        }
        self.advance_inputs(now.ride, sensors, route);
    }

    /// Stage 4 — advance `UiRuntime`, then collect the typed intents the rider produced.
    ///
    /// The UI reaches a domain by naming what it wants, never by performing the work: a delete is a
    /// [`CatalogIntent`], not a store operation. Every intent is offered into a slot that is
    /// **checked first** — an intent that cannot be delivered leaves the rider's one-shot exactly
    /// where it was, so nothing is lost by a busy pass.
    fn stage_ui(&mut self, now: PassClock) {
        self.pass.record(PassStage::Ui);
        self.advance_animations(now.ui);

        if self.pass.connections.ui_catalog.is_empty() {
            // A vanished subject consumes the request and yields nothing — the same rule the
            // legacy drain applies, and the reason the index is resolved to a durable id here.
            let intent = if let Some(idx) = self.activity.take_route_delete() {
                self.catalogs.route_id_at(idx).map(|id| CatalogIntent::DeleteRoute { id })
            } else if let Some(idx) = self.activity.take_ride_delete() {
                self.catalogs.ride_entry(idx).map(|entry| CatalogIntent::DeleteRide { id: entry.id })
            } else {
                None
            };
            if let Some(intent) = intent {
                let _ = self.pass.connections.ui_catalog.try_put(intent);
            }
        }
        if self.pass.connections.ui_recorder.is_empty() {
            if let Some(action) = self.activity.take_track_action() {
                let intent = match action {
                    crate::TrackAction::Save => RecorderIntent::Save,
                    crate::TrackAction::Discard => RecorderIntent::Discard,
                };
                let _ = self.pass.connections.ui_recorder.try_put(intent);
            }
        }
    }

    /// Stage 5 — advance `RetentionMachine`.
    ///
    /// Its deferred inbox first: what Navigator, Recorder and the catalog decided *after* it ran
    /// last pass. Then the domain's own advance, then the one expiry intent and the one sidecar
    /// write it may have this pass. The expiry goes into a same-pass slot because the catalog runs
    /// next — an auto-expired object leaves by exactly the path a rider-deleted one does.
    ///
    /// Each inbox value goes to a domain **entry point that re-derives its own rule**: a delivered
    /// id is a pass old, and only the domain can say whether it still qualifies. An activation in
    /// particular must not stamp a route that has no expiry clock, and must queue nothing at all
    /// without a trusted clock — the two guards
    /// [`note_route_activated`](crate::retention::RetentionMachine::note_route_activated) applies,
    /// which are the same ones the sweep applies.
    fn stage_retention(&mut self, effects: &mut EffectSlots) {
        self.pass.record(PassStage::Retention);
        if let Some(activated) = self.pass.connections.route_activated.take() {
            self.with_retention(|retention, view| retention.note_route_activated(activated.route, view));
        }
        if self.pass.connections.ride_closed.take().is_some() {
            self.retention.force_next_sweep();
        }
        if self.pass.connections.catalog_identity.take().is_some() {
            self.retention.force_next_sweep();
        }
        self.retention_tick();

        if self.pass.connections.expiry.is_empty() {
            for kind in [SweepKind::DeleteRoute, SweepKind::DeleteRide] {
                if let Some(intent) = self.with_retention(|retention, view| retention.next_expiry(kind, view)) {
                    let _ = self.pass.connections.expiry.try_put(intent);
                    break;
                }
            }
        }
        if effects.retention.is_empty() {
            for kind in [SweepKind::StampRoute, SweepKind::StampRide] {
                if let Some(effect) = self.with_retention(|retention, view| retention.next_metadata_effect(kind, view))
                {
                    let _ = effects.retention.try_put(effect);
                    break;
                }
            }
        }
    }

    /// Stage 6 — advance `CatalogMachine`.
    ///
    /// The rider's own request outranks an expiry, exactly as the legacy drain has it: a hold-to-
    /// delete is something someone is watching happen. An admitted deletion of the **followed**
    /// route reaches Navigator in this pass — the rider is not left being guided along a route the
    /// device has decided to remove — and a store commit is announced to retention for the next one.
    fn stage_catalog(&mut self, effects: &mut EffectSlots) {
        self.pass.record(PassStage::Catalog);
        // A refused intent goes back into the slot it came from: that slot is its producer's pending
        // state until the catalog has room, and putting it back is what makes a busy pass cost a
        // delay rather than a delete.
        if let Some(intent) = self.pass.connections.ui_catalog.take() {
            if let Err(full) = self.admit_catalog_intent(intent) {
                let _ = self.pass.connections.ui_catalog.try_put(full.rejected);
            }
        }
        if let Some(intent) = self.pass.connections.expiry.take() {
            if let Err(full) = self.admit_catalog_intent(intent) {
                let _ = self.pass.connections.expiry.try_put(full.rejected);
            }
        }
        if let Some(store) = self.pass.store {
            if self.pass.announced != Some(store.revision) {
                self.pass.announced = Some(store.revision);
                let _ =
                    self.pass.connections.catalog_identity.defer(CatalogIdentityChanged { revision: store.revision });
            }
        }
        if let Some(effect) = self.catalogs.next_effect() {
            let _ = effects.catalog.try_put(effect);
        }
    }

    /// Hand one intent to the catalog domain, and — when what leaves is the route being followed —
    /// tell Navigator in this same pass. The refusal is handed back to the caller unchanged.
    fn admit_catalog_intent(&mut self, intent: CatalogIntent) -> Result<(), SlotFull<CatalogIntent>> {
        self.catalogs.admit_intent(intent)?;
        if let CatalogIntent::DeleteRoute { id } = intent {
            if self.activity.active_route.and_then(|idx| self.catalogs.route_id_at(idx)) == Some(id) {
                let _ = self.pass.connections.active_route_removed.try_put(ActiveRouteRemoved { route: id });
            }
        }
        Ok(())
    }

    /// Stage 7 — advance `Recorder`.
    ///
    /// The ride session itself accumulates with the fix in stage 3, and the close reaches the
    /// platform on the legacy path until Recorder's machine lands (#1397). What the pass owns is the
    /// connection: the ride inventory is about to change and retention hears about it next pass. A
    /// full deferred slot leaves the intent where it was, and the immediate next wake is what brings
    /// it back.
    fn stage_recorder(&mut self) {
        self.pass.record(PassStage::Recorder);
        let Some(intent) = self.pass.connections.ui_recorder.take() else { return };
        let closed = RideClosed { discarded: matches!(intent, RecorderIntent::Discard) };
        if self.pass.connections.ride_closed.defer(closed).is_err() {
            let _ = self.pass.connections.ui_recorder.try_put(intent);
        }
    }

    /// Stage 8 — advance `Navigator`.
    ///
    /// Consumes the catalog's [`ActiveRouteRemoved`] in the same pass it was sent, and reports an
    /// activation to retention in the next one — an active route must not expire underneath the ride
    /// it is guiding.
    fn stage_navigator(&mut self) {
        self.pass.record(PassStage::Navigator);
        if let Some(removed) = self.pass.connections.active_route_removed.take() {
            if self.activity.active_route.and_then(|idx| self.catalogs.route_id_at(idx)) == Some(removed.route) {
                self.activity.active_route = None;
                self.drop_route_derived_state();
                self.ui.map_dirty = true;
            }
        }
        let active = self.activity.active_route.and_then(|idx| self.catalogs.route_id_at(idx));
        if active != self.pass.active_route {
            self.pass.active_route = active;
            if let Some(route) = active {
                let _ = self.pass.connections.route_activated.defer(RouteActivated { route });
            }
        }
    }

    /// Stage 9 — advance `SettingsMachine`.
    ///
    /// The dirty revision, the debounce, the retry backoff and the stale-ack rule live in
    /// [`HostPending`](crate::host::HostPending) and advance on the frame clock stage 3 set. The
    /// write itself is still owed through the legacy protocol: an effect needs an operation token,
    /// and the token needs the owner that #1397 lands.
    fn stage_settings(&mut self) {
        self.pass.record(PassStage::Settings);
    }

    /// Stage 10 — advance `WeatherDomain`: one refresh at a time, and only while the device can
    /// actually reach a companion. The capability is the level stage 12 calculated last pass — a
    /// refresh the link cannot serve is not started at all rather than failing.
    fn stage_weather(&mut self, effects: &mut EffectSlots) {
        self.pass.record(PassStage::Weather);
        if let Some(effect) = self.weather.next_effect(self.pass.capabilities.weather) {
            let _ = effects.weather.try_put(effect);
        }
    }

    /// Stage 11 — advance `DfuState`, `BondState` and `StorageInfo`.
    ///
    /// The three domains whose product state is a scan result, a bond and a number of free bytes.
    /// Their machines arrive with their cutovers (#1397); the stage is where they advance.
    fn stage_platform(&mut self) {
        self.pass.record(PassStage::Platform);
    }

    /// Stage 12 — `CoreMode`: recalculate what this device can do at all.
    ///
    /// A capability is a level, never latched: it is recomputed from what the image implements and
    /// what is currently true (a mounted store, a routing graph, a streaming transfer, a recording
    /// ride). Heavy work is withdrawn while a transfer holds the store — which is what stops a plan
    /// or an install from starting, rather than letting one start and fail.
    ///
    /// **One axis cannot yet come back down.** `store_writable` reads "a store has reported a
    /// revision", and [`ExternalFacts`] has no unmount fact to retract it with, so a pulled card
    /// leaves catalog mutation asserted. That is a gap in the fact vocabulary rather than in this
    /// stage: the level is honest the moment an unmount can be reported.
    fn stage_admission(&mut self, support: PlatformSupport) {
        self.pass.record(PassStage::Admission);
        let facts = DeviceFacts {
            store_writable: self.pass.store.is_some(),
            nav_graph: self.state.has_nav_graph,
            weather_data: self.weather.installed().is_some(),
            link_connected: matches!(self.state.device.ble_link, crate::ble::BleLink::Connected),
            ride_recording: self.activity.is_tracking(),
            heavy_operations: matches!(self.pass.transfer, TransferState::Idle),
        };
        self.pass.capabilities = Capabilities::calculate(support, facts);
    }

    /// Stage 13 — advance `FaultState`: deliver every notice raised this pass, together. Last,
    /// because every producer runs before it, so one card carries what several domains found.
    fn stage_faults(&mut self) {
        self.pass.record(PassStage::Faults);
        let flags = self.pass.connections.faults.take();
        if !flags.is_empty() {
            self.on_warning(flags);
        }
    }

    /// Stage 14 — calculate the plan: render work, the next wake, the derived and source needs, and
    /// the bounded effects.
    ///
    /// A deferred connection still in flight folds into an immediate wake: the runtime must run one
    /// more pass before it sleeps, or work that is already decided would sit until the next input.
    fn stage_plan(&mut self, now: PassClock, effects: EffectSlots) -> PassPlan {
        self.pass.record(PassStage::Plan);
        let render = self.take_dirty();
        let immediate = self.pass.connections.has_deferred();
        let next_wake_ms = if immediate { Some(0) } else { self.ms_until_next_wake(now.ui.0) };
        PassPlan {
            render,
            next_wake_ms,
            derived_needs: self.derived_needs(),
            sources: SourceNeeds { map: self.base_needs_reader(), route: self.activity.active_route.is_some() },
            effects,
            immediate,
        }
    }

    /// What this device can currently do, as of the last pass's admission stage.
    pub(crate) fn capabilities(&self) -> Capabilities {
        self.pass.capabilities
    }

    /// The stages the last pass ran, in order.
    #[cfg(test)]
    pub(crate) fn pass_trace(&self) -> &[PassStage] {
        &self.pass.trace
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Mode;
    use crate::app::AppState;
    use crate::catalog_state::{CatalogEffect, CatalogOutcome};
    use crate::device_core::{DataIdentity, StoreIdentity, TokenSource, WeatherData};
    use crate::retention::{Retention, RouteRetentionMeta};
    use crate::route::RouteSummary;
    use crate::screen::WarningFlags;
    use crate::weather::WeatherOutcome;
    use obc_ports::{Fix, LocationSource};

    /// A location port that never has a fix — the pass's sensor input in every test that is not
    /// about the fix path.
    struct NoFix;
    impl LocationSource for NoFix {
        fn poll(&mut self) -> Option<Fix> {
            None
        }
    }

    /// One pass with nothing to report: no gesture, no fix, no outcome, no fact.
    fn quiet(app: &mut App, ms: u32) -> PassPlan {
        let mut facts = ExternalFacts::NONE;
        pass_with(app, ms, &[], &mut OutcomeSlots::new(), &mut facts)
    }

    fn pass_with(
        app: &mut App,
        ms: u32,
        gestures: &[Gesture],
        outcomes: &mut OutcomeSlots,
        facts: &mut ExternalFacts,
    ) -> PassPlan {
        pass_full(
            app,
            PassClock { ride: RideClock(ms), ui: InputClock(ms) },
            gestures,
            outcomes,
            facts,
            DerivedInputs::NONE,
            DerivedTargets::NONE,
        )
    }

    /// Every input the pass takes, so one test can drive the halves the shorthands leave at `NONE`.
    fn pass_full(
        app: &mut App,
        now: PassClock,
        gestures: &[Gesture],
        outcomes: &mut OutcomeSlots,
        facts: &mut ExternalFacts,
        derived: DerivedInputs,
        targets: DerivedTargets<'_>,
    ) -> PassPlan {
        let mut loc = NoFix;
        app.run_pass(PassInputs {
            now,
            gestures,
            sensors: Sensors::new(&mut loc),
            route: None,
            support: PlatformSupport {
                detour: true,
                settings_persistence: true,
                dfu: true,
                weather: true,
                bonding: true,
                storage_space_report: true,
            },
            outcomes,
            facts,
            derived,
            targets,
        })
    }

    fn summary(name: &str) -> RouteSummary {
        RouteSummary {
            name: heapless::String::try_from(name).unwrap(),
            distance_km: 10,
            climb_m: 100,
            bbox: obc_map_scene::BBox { min_lon: 0, min_lat: 0, max_lon: 1000, max_lat: 1000 },
            start_lon: 100,
            start_lat: 100,
        }
    }

    /// An app with two routes catalogued and the first one active.
    fn navigating() -> App {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.set_routes_with_ids(&[summary("alpha"), summary("beta")], &[11, 22]);
        app.activate_route(0);
        app
    }

    fn ride_summary() -> crate::ride::RideSummary {
        crate::ride::RideSummary {
            name: heapless::String::try_from("First").unwrap(),
            start_time: 1_720_000_000,
            distance_m: 1_000,
            moving_time_s: 600,
            climb_m: 10,
            synced: false,
            synced_at_utc: 0,
        }
    }

    fn expiring(retention: Retention, last_used_utc: u32) -> RouteRetentionMeta {
        RouteRetentionMeta::new(retention, last_used_utc)
    }

    /// A store fact at `revision`, the level a commit reports.
    fn committed(revision: u64) -> ExternalFacts {
        let mut facts = ExternalFacts::NONE;
        facts.note_store_revision(StoreRevision { store: StoreIdentity::new(1), revision: Revision::new(revision) });
        facts
    }

    // ==================== the order ====================

    /// The fixed order, in full: every stage runs, each exactly once, in exactly this sequence.
    /// A quiet pass runs the same stages as a busy one — a stage is a *position*, not a reaction.
    #[test]
    fn every_stage_runs_exactly_once_in_the_fixed_order() {
        let mut app = navigating();
        quiet(&mut app, 10);
        assert_eq!(app.pass_trace(), PassStage::ORDER, "the quiet pass runs the whole order");

        let mut outcomes = OutcomeSlots::new();
        let mut facts = committed(3);
        pass_with(&mut app, 20, &[Gesture::Step(1)], &mut outcomes, &mut facts);
        assert_eq!(app.pass_trace(), PassStage::ORDER, "and so does a pass with work in every stage");

        for stage in PassStage::ORDER {
            assert_eq!(app.pass_trace().iter().filter(|&&s| s == stage).count(), 1, "{stage:?} advances once");
        }
    }

    /// The order is not iterated to a fixed point: a second pass is a second pass, never a hidden
    /// loop inside the first.
    #[test]
    fn a_pass_never_iterates_until_the_slots_empty() {
        let mut app = navigating();
        quiet(&mut app, 10);
        assert_eq!(app.pass_trace().len(), PassStage::ORDER.len(), "one advance per component, whatever is pending");
    }

    // ==================== earlier → later, in the same pass ====================

    /// The rider's delete: `UiRuntime` produces the intent at stage 4 and `CatalogMachine` has it at
    /// stage 6 — one pass, not two — and it leaves as one bounded effect.
    #[test]
    fn a_ui_delete_reaches_the_catalog_in_the_same_pass() {
        let mut app = navigating();
        quiet(&mut app, 10); // settle the boot pass
        app.activity.request_route_delete(1);

        let plan = quiet(&mut app, 20);
        let mut effects = plan.effects;
        assert!(
            matches!(effects.catalog.take(), Some(CatalogEffect::RemoveObject { object: 22, .. })),
            "the rider's delete became this pass's catalog operation"
        );
        assert!(app.pass.connections.ui_catalog.is_empty(), "the intent was consumed, not queued");
    }

    /// Retention's expiry is the *same* intent a rider's delete is, delivered in the same pass —
    /// an auto-expired object leaves by exactly the path a deleted one does.
    #[test]
    fn a_retention_expiry_reaches_the_catalog_in_the_same_pass() {
        let (mut app, now) = expiring_app();
        let plan = quiet(&mut app, now);

        let mut effects = plan.effects;
        assert!(
            matches!(effects.catalog.take(), Some(CatalogEffect::RemoveObject { object: 22, .. })),
            "the expired route left as a catalog removal in the pass retention decided it"
        );
    }

    /// Deleting the route being followed reaches Navigator in the same pass: the rider is not left
    /// being guided along a route the device has decided to remove.
    #[test]
    fn deleting_the_active_route_reaches_navigator_in_the_same_pass() {
        let mut app = navigating();
        quiet(&mut app, 10);
        assert_eq!(app.active_route_index(), Some(0));

        app.activity.request_route_delete(0);
        quiet(&mut app, 20);

        assert_eq!(app.active_route_index(), None, "Navigator dropped the route in the delete's own pass");
        assert!(app.pass.connections.active_route_removed.is_empty(), "the notice was consumed");
    }

    /// A fault raised by an earlier stage reaches `FaultState` in the same pass, and several
    /// producers coalesce onto one card rather than displacing each other.
    #[test]
    fn a_fault_raised_earlier_in_the_pass_reaches_the_rider_in_it() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        let mut facts = ExternalFacts::NONE;
        facts.raise_warnings(WarningFlags::NO_GPS);
        facts.raise_warnings(WarningFlags::MAP_SLOW);

        pass_with(&mut app, 10, &[], &mut OutcomeSlots::new(), &mut facts);
        assert!(app.pass.connections.faults.is_empty(), "delivered, not left pending");
        assert!(
            matches!(app.top_screen(), crate::Screen::Warning(w) if w.flags().contains(WarningFlags::NO_GPS)
                && w.flags().contains(WarningFlags::MAP_SLOW)),
            "both notices reached one card"
        );
    }

    // ==================== later → earlier, in the next pass ====================

    /// Navigator runs after retention, so an activation cannot reach it in the same pass. It waits
    /// in a deferred slot and lands *before any new input* on the next one.
    #[test]
    fn an_activation_reaches_retention_on_the_next_pass() {
        let mut app = navigating();
        trust_clock(&mut app);
        // A fresh `last_used` so the hourly sweep has nothing of its own to say: the only stamp in
        // this test is the activation's.
        let now = app.wall_unix_now();
        app.set_route_meta(&[expiring(Retention::Week1, now), expiring(Retention::Never, 0)]);

        let plan = quiet(&mut app, 10);
        assert!(
            app.pass.connections.route_activated.is_pending(),
            "the activation is deposited, not delivered — retention already ran"
        );
        // The stamp itself is retention's own, from the view it reads every advance, so it is
        // already out; the connection delivers the same fact by the same rule one pass later.
        let mut effects = plan.effects;
        assert!(
            matches!(
                effects.retention.take(),
                Some(crate::retention::RetentionEffect::WriteRouteMetadata { id: 11, .. })
            ),
            "the active route's use stamp goes out"
        );

        let plan = quiet(&mut app, 20);
        assert!(!app.pass.connections.route_activated.is_pending(), "consumed by retention's stage");
        assert!(!plan.immediate, "and nothing is left waiting");
        assert!(plan.effects.retention.is_empty(), "the delivery is idempotent — no second sidecar write");
        assert!(!app.retention.has(SweepKind::StampRoute), "and no second candidate either");
    }

    /// The delivered id is a pass old, so the domain re-derives the rule rather than trusting it:
    /// a route with no expiry clock is never stamped, and an untrusted clock queues nothing at all.
    ///
    /// Both are retention's own invariants, and a delivery that reached past them would write a
    /// sidecar for a countdown that does not exist — or put a candidate in the bounded queue on a
    /// boot where "nothing runs" is the whole safety core.
    #[test]
    fn an_activation_stamps_only_a_route_that_can_expire_under_a_trusted_clock() {
        let mut never = navigating();
        never.set_route_meta(&[expiring(Retention::Never, 0), expiring(Retention::Never, 0)]);
        trust_clock(&mut never);
        quiet(&mut never, 10); // stage 8 defers the activation
        let plan = quiet(&mut never, 20); // …and this is the pass that delivers it
        assert!(plan.effects.retention.is_empty(), "a route with no expiry clock has no `last_used` to write");
        assert!(!never.retention.has(SweepKind::StampRoute), "and no candidate is queued for one");

        let mut untrusted = navigating();
        untrusted.set_route_meta(&[expiring(Retention::Week1, 0), expiring(Retention::Never, 0)]);
        quiet(&mut untrusted, 10);
        let plan = quiet(&mut untrusted, 20);
        assert!(
            plan.effects.retention.is_empty() && !untrusted.retention.has(SweepKind::StampRoute),
            "no trusted clock this boot: no stamp, no sweep, no candidate"
        );
    }

    /// A deferred value in flight makes the pass ask for another one **before sleep**: the work is
    /// already decided, and parking on it would leave it sitting until the next rider input.
    ///
    /// The other half of the rule — a *full* slot handing the value back so its producer keeps it —
    /// is the [`Deferred`](super::super::connections::Deferred) contract, exercised in
    /// [`connections`](super::super::connections). It cannot arise from this wiring, because every
    /// consumer's stage runs on every pass.
    #[test]
    fn a_deferred_value_forces_another_pass_before_sleep() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.activity.mode = Mode::Riding;
        quiet(&mut app, 10);

        app.activity.request_track(crate::TrackAction::Save);
        let plan = quiet(&mut app, 20);
        assert!(app.pass.connections.ride_closed.is_pending(), "the close waits for retention");
        assert!(plan.immediate && plan.next_wake_ms == Some(0), "so the runtime comes straight back");

        // The next pass consumes it before anything else, and then there is nothing to hurry for.
        let plan = quiet(&mut app, 30);
        assert!(!app.pass.connections.ride_closed.is_pending());
        assert!(!plan.immediate && plan.next_wake_ms != Some(0));

        // A second close right behind the first is delivered just the same — nothing was lost to
        // the pass that was already carrying one.
        app.activity.request_track(crate::TrackAction::Discard);
        let plan = quiet(&mut app, 40);
        assert!(app.pass.connections.ride_closed.is_pending() && plan.immediate);
        assert!(app.pass.connections.ui_recorder.is_empty(), "the intent reached its domain");
    }

    /// The backpressure rule, end to end: two intents reach the catalog in one pass, it can admit
    /// one, and the refused one goes **back into the slot it came from** rather than being dropped —
    /// so a busy pass costs a delay, never a delete.
    #[test]
    fn a_refused_intent_goes_back_to_its_producer_and_lands_later() {
        let (mut app, ms) = expiring_app();
        app.activity.request_route_delete(0); // the rider deletes one route in the pass an expiry fires

        let plan = quiet(&mut app, ms);
        let mut effects = plan.effects;
        let first = effects.catalog.take().expect("the rider's delete outranks the expiry");
        assert!(matches!(first, CatalogEffect::RemoveObject { object: 11, .. }));
        assert!(!app.pass.connections.expiry.is_empty(), "the refused expiry is back with its producer");

        // Next pass: the catalog admits it, but its one operation is still in flight.
        let plan = quiet(&mut app, ms + 10);
        assert!(app.pass.connections.expiry.is_empty(), "delivered on the pass after the refusal");
        assert!(plan.effects.catalog.is_empty(), "one catalog operation at a time");

        // The answer frees the domain, and the expiry that waited two passes goes out unchanged.
        let mut outcomes = OutcomeSlots::new();
        outcomes
            .catalog
            .try_put(CatalogOutcome::ObjectRemoved { token: first.token(), object: 11, existed: true })
            .unwrap();
        let mut none = ExternalFacts::NONE;
        let plan = pass_with(&mut app, ms + 20, &[], &mut outcomes, &mut none);
        let mut effects = plan.effects;
        assert!(
            matches!(effects.catalog.take(), Some(CatalogEffect::RemoveObject { object: 22, .. })),
            "nothing was lost to the busy pass"
        );
    }

    /// A store commit is announced to retention once, from the stage that owns the catalog — and
    /// because the catalog runs *after* retention, next pass.
    #[test]
    fn a_catalog_identity_change_reaches_retention_on_the_next_pass() {
        let mut app = navigating();
        let mut facts = committed(4);
        pass_with(&mut app, 10, &[], &mut OutcomeSlots::new(), &mut facts);
        assert!(app.pass.connections.catalog_identity.is_pending());

        quiet(&mut app, 20);
        assert!(!app.pass.connections.catalog_identity.is_pending(), "retention consumed it");

        // The same revision is not announced twice.
        let mut same = committed(4);
        pass_with(&mut app, 30, &[], &mut OutcomeSlots::new(), &mut same);
        assert!(!app.pass.connections.catalog_identity.is_pending(), "one commit, one announcement");
    }

    // ==================== outcomes ====================

    /// A domain consumes its own outcome and rejects one it has moved past. Only a domain that owns
    /// a token source may consume at all — an outcome nobody can validate stays in its slot rather
    /// than being dropped or guessed at.
    #[test]
    fn an_outcome_is_consumed_by_its_owner_and_left_alone_without_one() {
        let mut app = navigating();
        quiet(&mut app, 10);

        let installed = WeatherData { data: DataIdentity::new(2), revision: Revision::new(1) };
        let mut outcomes = OutcomeSlots::new();
        let mut stale: TokenSource<crate::device_core::WeatherTag> = TokenSource::new();
        outcomes
            .weather
            .try_put(WeatherOutcome::Refreshed {
                token: stale.issue(),
                data: installed.data,
                revision: installed.revision,
            })
            .unwrap();
        // No machine owns these two, so nothing may act on them.
        let mut recorder_ops: TokenSource<crate::device_core::RecorderTag> = TokenSource::new();
        let recorder = crate::recorder::RecorderOutcome::Discarded { token: recorder_ops.issue() };
        outcomes.recorder.try_put(recorder).unwrap();

        let mut none = ExternalFacts::NONE;
        pass_with(&mut app, 20, &[], &mut outcomes, &mut none);
        assert!(app.weather.installed().is_none(), "a token the domain never issued is not an answer");
        assert_eq!(outcomes.recorder.take(), Some(recorder), "an outcome with no owner is left, never dropped");
    }

    /// The catalog's own outcome frees its operation, so the next intent can go out — the loop that
    /// makes one effect slot enough for a queue of deletes.
    #[test]
    fn a_catalog_outcome_frees_the_next_operation() {
        let mut app = navigating();
        quiet(&mut app, 10);

        app.activity.request_route_delete(1);
        let plan = quiet(&mut app, 20);
        let mut effects = plan.effects;
        let first = effects.catalog.take().expect("the delete went out");

        // A second delete while the first is unanswered: it waits, and no second effect is issued.
        app.activity.request_route_delete(0);
        let plan = quiet(&mut app, 30);
        assert!(plan.effects.catalog.is_empty(), "one catalog operation in flight at a time");

        let mut outcomes = OutcomeSlots::new();
        outcomes
            .catalog
            .try_put(CatalogOutcome::ObjectRemoved { token: first.token(), object: 22, existed: true })
            .unwrap();
        let mut none = ExternalFacts::NONE;
        let plan = pass_with(&mut app, 40, &[], &mut outcomes, &mut none);
        let mut effects = plan.effects;
        assert!(
            matches!(effects.catalog.take(), Some(CatalogEffect::RemoveObject { object: 11, .. })),
            "the answer frees the domain at stage 1, so the retained delete goes out at stage 6 of that pass"
        );

        // A repeat of the same answer is no longer current, so it frees nothing a second time.
        let mut repeat = OutcomeSlots::new();
        repeat
            .catalog
            .try_put(CatalogOutcome::ObjectRemoved { token: first.token(), object: 22, existed: true })
            .unwrap();
        let mut none = ExternalFacts::NONE;
        let plan = pass_with(&mut app, 50, &[], &mut repeat, &mut none);
        assert!(plan.effects.catalog.is_empty(), "nothing was owed, and a stale answer starts nothing");
    }

    // ==================== the plan, the guard, and the latch ====================

    /// Capabilities are recalculated every pass from what the platform implements and what is
    /// currently true — a level, never latched.
    #[test]
    fn admission_recalculates_capabilities_every_pass() {
        let mut app = navigating();
        let mut facts = committed(1);
        pass_with(&mut app, 10, &[], &mut OutcomeSlots::new(), &mut facts);
        assert!(app.capabilities().catalog.mutate, "a mounted store may be mutated");

        let mut streaming = ExternalFacts::NONE;
        streaming.note_transfer(TransferState::Active);
        pass_with(&mut app, 20, &[], &mut OutcomeSlots::new(), &mut streaming);
        assert!(!app.capabilities().dfu.install, "an install is heavy — never while a transfer streams");

        let mut idle = ExternalFacts::NONE;
        idle.note_transfer(TransferState::Idle);
        pass_with(&mut app, 30, &[], &mut OutcomeSlots::new(), &mut idle);
        assert!(app.capabilities().dfu.install, "and it comes straight back");
    }

    /// A platform callback cannot change DeviceCore in the middle of a pass. `run_pass` holds
    /// `&mut self` for its whole length, so no safe caller can reach a push door mid-pass at all —
    /// the flag has to be set by hand here. Reaching one anyway is a caller bug, so it is loud in
    /// debug and refused in release rather than quietly losing the event.
    #[test]
    #[should_panic(expected = "cannot change DeviceCore during a pass")]
    fn a_callback_cannot_mutate_core_state_during_a_pass() {
        let mut app = navigating();
        quiet(&mut app, 10);
        app.pass.in_pass = true;
        app.apply_event(crate::HostEvent::StoreChanged);
    }

    /// The same door, outside a pass: it works exactly as it always did.
    #[test]
    fn a_push_outside_a_pass_is_applied_normally() {
        let mut app = navigating();
        quiet(&mut app, 10);
        let before = app.store_changed_pending();
        app.apply_event(crate::HostEvent::StoreChanged);
        assert_ne!(app.store_changed_pending(), before);
    }

    /// Hold cancellation stays off the pass entirely: a pass neither reports it nor drains it, so
    /// the board's input plane still finds the latch it must act on between passes.
    #[test]
    fn hold_cancellation_stays_independent_of_the_pass() {
        let mut app = navigating();
        quiet(&mut app, 10);

        // A gesture that changes the stack arms the latch.
        let mut none = ExternalFacts::NONE;
        pass_with(&mut app, 20, &[Gesture::Press], &mut OutcomeSlots::new(), &mut none);
        assert!(app.debug_stack_len() > 1, "the press opened a screen");

        // Another pass runs without touching it — the latch is still there for the board.
        quiet(&mut app, 30);
        assert!(app.take_hold_cancel(), "the pass left the latch for its owner");
        assert!(!app.take_hold_cancel(), "and it is a one-shot");
    }

    /// The plan is the whole answer the executor needs: what to repaint, when to come back, what to
    /// read, and the bounded work per domain.
    #[test]
    fn the_plan_reports_render_wake_needs_and_effects() {
        let mut app = navigating();
        let plan = quiet(&mut app, 10);
        assert!(plan.render.map, "the boot pass has a frame to draw");
        assert!(plan.sources.map, "the map base needs its reader");
        assert!(plan.sources.route, "and a route is loaded");

        let plan = quiet(&mut app, 20);
        assert!(!plan.render.map, "a quiet pass repaints nothing");
        assert!(plan.derived_needs.is_empty(), "no detail is open");
        assert!(!plan.effects.has_pending(), "and nothing physical is owed");
    }

    /// Stage 2 with a full batch: every level lands with its owner, every one-shot is **taken** from
    /// the batch (which is what "the pass consumed it" means), and a keyed derived answer clears the
    /// need it answers. The handlers themselves are pinned by the legacy-protocol tests; what this
    /// covers is the pass's routing to them.
    ///
    /// The two clocks are deliberately different here — the board runs them equal, the simulator does
    /// not — so a stage that read the ride clock where it owes the UI one trips
    /// [`App::ms_until_next_wake`]'s same-frame assertion.
    #[test]
    fn one_pass_routes_a_full_fact_batch_and_a_derived_answer() {
        let mut app = navigating();
        app.set_rides(&[ride_summary()], &[7]);
        app.activity.viewed_ride = Some(0);
        let mut quiet_facts = ExternalFacts::NONE;
        let plan = pass_full(
            &mut app,
            PassClock { ride: RideClock(5_000), ui: InputClock(9_000) },
            &[],
            &mut OutcomeSlots::new(),
            &mut quiet_facts,
            DerivedInputs::NONE,
            DerivedTargets::NONE,
        );
        let key = plan.derived_needs.ride_track.expect("the open ride detail needs its track");

        let connected =
            crate::ble::BleStatus { link: crate::ble::BleLink::Connected, ..crate::ble::BleStatus::DISCONNECTED };
        let installed = WeatherData { data: DataIdentity::new(7), revision: Revision::new(2) };
        let mut facts = committed(9);
        facts.note_link(connected);
        facts.note_weather_data(installed);
        facts.note_route_upload(crate::device_core::RouteUpload { id: 33, replaced: false, elevation: None });
        facts.note_trip_upload(crate::device_core::TripUpload { id: 44, replaced: false });
        facts.note_update_result(UpdateResult::Confirmed(crate::dfu::clamp("v9"))).unwrap();

        let plan = pass_full(
            &mut app,
            PassClock { ride: RideClock(6_000), ui: InputClock(10_000) },
            &[],
            &mut OutcomeSlots::new(),
            &mut facts,
            DerivedInputs::ride_track(crate::device_core::DerivedInput::filled(key)),
            DerivedTargets { ride_preview: &[(1, 2), (3, 4)], nav_preview: &[] },
        );

        // Levels reached their owners.
        assert_eq!(app.weather.installed(), Some(installed), "the installed data reached WeatherDomain");
        assert_eq!(app.state.device.ble_link, crate::ble::BleLink::Connected, "the link state reached the UI");
        assert!(app.store_changed_pending() > 0, "the commit became the catalog's refresh cue");

        // One-shots were consumed rather than left for a second delivery.
        assert!(facts.take_route_upload().is_none() && facts.take_trip_upload().is_none());
        assert!(facts.take_update_result().is_none() && facts.take_warnings().is_empty());
        assert!(app.debug_stack_len() > 1, "and what they post reached the screen");

        // The keyed answer cleared the need it answered.
        assert!(plan.derived_needs.ride_track.is_none(), "the ride track is answered");
    }

    // ---- helpers that need a trusted clock ----

    fn trust_clock(app: &mut App) {
        app.stamp_clock_ble(1_700_000_000, 0);
    }

    /// An app whose second route is long expired under a trusted clock, with the first one active.
    fn expiring_app() -> (App, u32) {
        let mut app = navigating();
        trust_clock(&mut app);
        let now = app.wall_unix_now();
        app.set_route_meta(&[
            RouteRetentionMeta::new(Retention::Never, 0),
            RouteRetentionMeta::new(Retention::Week1, now.saturating_sub(30 * 24 * 3600)),
        ]);
        app.force_retention_sweep();
        (app, 10)
    }
}
