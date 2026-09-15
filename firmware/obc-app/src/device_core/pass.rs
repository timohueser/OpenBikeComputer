use obc_ports::{InputClock, RideClock, Sensors};
use obc_route::RouteReader;

use crate::catalog_state::CatalogIntent;
use crate::device_core::connections::{
    ActiveRouteRemoved, CatalogIdentityChanged, CatalogRemoval, RideFinalized, RouteActivated,
};
use crate::dirty::Dirty;
use crate::input::Gesture;
use crate::App;

use super::connections::Connections;
use super::derived::{DerivedInputs, DerivedNeeds, DerivedTargets};
use super::{
    Capabilities, DeviceFacts, EffectSlots, ExternalFacts, OutcomeSlots, PlatformSupport, Revision, SlotFull,
    StoreRevision, TransferState, UpdateResult,
};

/// The thirteen stages, in the order [`App::run_pass`] runs them. Each runs exactly once, and each
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
    pub const ORDER: [PassStage; 13] = [
        PassStage::Outcomes,
        PassStage::Facts,
        PassStage::Input,
        PassStage::Ui,
        PassStage::Retention,
        PassStage::Catalog,
        PassStage::Recorder,
        PassStage::Navigator,
        PassStage::Settings,
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

/// The **overlay plane's** two levels, as one value (#1447).
///
/// The overlay is the cheap transient layer: the hold bulge and the Recalculating banner. Its
/// repaint rule is not the map's — it is two rules over two levels, which used to be three separate
/// level-to-edge converters, one on the input plane, one on the UI runtime and one on the mode
/// machine, producing a single boolean per frame between them.
///
/// - The **bulge** repaints while it is live, plus exactly one trailing frame after it goes quiet,
///   so the last bulge is cleared off the layer rather than left painted over an unchanged map.
/// - The **banner** repaints when the freeze's *engaged* level flips, either way. It is a level and
///   not the search's own start edge: a plan begun under the opaque planning spinner engages
///   nothing, and the pass that puts a map base back under a still-running search raises no search
///   edge at all. Keyed on the start edge, the banner would be spent on a chrome frame and the
///   rider would then look at a frozen screen with no explanation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OverlayKey {
    /// The hold bulge is charging, popping or retracting.
    pub(crate) hold: bool,
    /// A search holds the nav arm *and* the base screen would draw a map.
    pub(crate) freeze: bool,
}

impl OverlayKey {
    /// The boot level: nothing charging, nothing frozen.
    const QUIET: OverlayKey = OverlayKey { hold: false, freeze: false };

    /// Whether the overlay plane must be repainted, given the level at the previous drain.
    fn dirty_against(self, previous: OverlayKey) -> bool {
        (self.hold || previous.hold) || (self.freeze != previous.freeze)
    }
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
    /// The active route's durable identity as of the last pass — Navigator's activation edge.
    active_route: Option<crate::CatalogObjectId>,
    /// What the device can currently do, recalculated at stage 12.
    pub(crate) capabilities: Capabilities,
    /// The overlay plane's levels at the last drain — the one converter behind
    /// [`Dirty::overlay`](crate::Dirty::overlay). See [`OverlayKey`].
    overlay: OverlayKey,
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
            active_route: None,
            capabilities: Capabilities::NONE,
            overlay: OverlayKey::QUIET,
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

    /// Fold this frame's overlay levels against the last drain's, then remember them. Call exactly
    /// once per frame: the bulge's trailing clear is tracked across calls, so a second drain in one
    /// frame swallows it.
    pub(crate) fn overlay_repaint(&mut self, now: OverlayKey) -> bool {
        now.dirty_against(core::mem::replace(&mut self.overlay, now))
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
const _: () = assert!(core::mem::size_of::<PassState>() <= 344, "connections, a few levels, the test recorder");

impl App {
    /// Run one DeviceCore pass: thirteen stages, once each, in [`PassStage::ORDER`].
    ///
    /// The whole product frame in one call — what completed, what changed, what the rider did, and
    /// what every domain decides about it — returning the bounded work the platform must perform.
    ///
    /// Public from #1439 on, so the compatibility adapter and the conformance tests can drive it.
    /// The existing frame methods stay the *production* hosts' entry points until #1397 S6 migrates
    /// them; both compositions call the same per-domain entry points, so there is one implementation
    /// of each and only the order differs.
    pub fn run_pass(&mut self, inputs: PassInputs<'_>) -> PassPlan {
        let PassInputs { now, gestures, sensors, route, support, outcomes, facts, derived, targets } = inputs;
        self.pass.enter();
        // The visible screens' exact facts, as they are *before* any stage runs (#1447). Held on
        // this frame's stack and nowhere else: a resident copy would be one more mirror of the state
        // it is describing. The twin below closes the comparison — see `crate::render_key`.
        let key_before = self.render_key();
        // The previous pass's later-to-earlier deposits become visible here — before any new
        // outcome, fact or gesture, so an earlier component acts on them ahead of new user input.
        self.pass.connections.promote_deferred();

        self.stage_outcomes(outcomes, now.ui.0);
        self.stage_facts(facts, derived, targets);
        self.stage_input(now, gestures, sensors, route);
        self.stage_ui(now);

        let mut effects = EffectSlots::new();
        self.stage_retention(&mut effects, support);
        self.stage_catalog(&mut effects);
        self.stage_recorder(&mut effects);
        self.stage_navigator(&mut effects);
        self.stage_settings(&mut effects);

        self.stage_platform(&mut effects, support);
        self.stage_admission(support);
        self.stage_faults();
        // Every stage has run: whatever the visible screens draw is now final for this frame. A
        // moved key is a repaint, folded in *before* stage 14 drains the demand, so the explicit
        // requests the stages made and the region-scoped tick demand fold together exactly as they
        // always have (a full-frame demand still overrides a region — over-redraw stays safe).
        if self.render_key() != key_before {
            self.ui.map_dirty = true;
        }
        let plan = self.stage_plan(now, effects);

        self.pass.leave();
        plan
    }

    /// Stage 1 — validate and consume each domain's outcome slot.
    ///
    fn stage_outcomes(&mut self, outcomes: &mut OutcomeSlots, now_ms: u32) {
        self.pass.record(PassStage::Outcomes);
        if let Some(outcome) = outcomes.catalog.take() {
            if self.catalogs.accepts(outcome) {
                match outcome {
                    crate::catalog_state::CatalogOutcome::Failed {
                        error: crate::catalog_state::CatalogError::Unreadable,
                        ..
                    } => self.catalogs.defer_read(now_ms),
                    crate::catalog_state::CatalogOutcome::Failed {
                        error: crate::catalog_state::CatalogError::Unsupported,
                        ..
                    } => self.retention.reject_expiry(),
                    _ => {}
                }
            }
            // The catalog's verdict on a removal is retention's: the expiry candidate for an object
            // the store no longer holds is retired at stage 5 of this pass, rather than surviving
            // until the re-read the removal ordered lands (#1548).
            if let Some(object) = self.catalogs.apply_outcome(outcome) {
                let _ = self.pass.connections.catalog_removal.try_put(CatalogRemoval { object });
            }
        }
        if let Some(outcome) = outcomes.retention.take() {
            if self.retention.apply_outcome(outcome) {
                self.assistant_checkpoint_answer(outcome);
                use crate::retention::{RetentionError, RetentionOutcome};
                match outcome {
                    RetentionOutcome::Failed { error: RetentionError::RemountRequired, .. } => {
                        self.catalogs.loaded_scope = None;
                        self.catalogs.remount_required = true;
                    }
                    RetentionOutcome::Failed { error: RetentionError::Unsupported, .. } => {}
                    RetentionOutcome::Failed { error: RetentionError::WriteFailed | RetentionError::Busy, .. }
                    | RetentionOutcome::Cancelled { .. } => {
                        self.retention.defer_write(now_ms);
                    }
                    _ => {
                        self.catalogs.loaded_scope = None;
                        self.catalogs.note_store_moved();
                    }
                }
            }
        }
        if let Some(outcome) = outcomes.recorder.take() {
            // Recorder's verdict on the close. A committed ride tells the catalog at stage 6 of this
            // pass — the same owed bit a removal and a store commit arm, so one saved ride is one
            // re-read. A failure raises the recording warning through the fault connection and
            // changes nothing else: the ride is still on the store, so the close stays pending and
            // re-offers.
            match self.recorder.apply_outcome(outcome) {
                crate::recorder::RecorderVerdict::Saved(ride) => {
                    let _ = self.pass.connections.ride_finalized.try_put(RideFinalized { ride });
                    self.end_ride_session();
                }
                crate::recorder::RecorderVerdict::Dropped => self.end_ride_session(),
                crate::recorder::RecorderVerdict::Failed => {
                    self.pass.connections.faults.raise(crate::screen::WarningFlags::REC_ERROR);
                }
                // The rider's one repair attempt on a damaged recovered object is over and Recorder
                // has latched what it came to. Re-raise the card so its new mode is what the rider
                // sees — the typed card **is** the explanation, so no `REC_ERROR` is raised beside
                // it: that warning means a ride log is now incomplete, and no ride is being logged.
                crate::recorder::RecorderVerdict::RecoveryLatched => {
                    self.raise_ride_recovery();
                }
                crate::recorder::RecorderVerdict::Nothing => {}
            }
        }

        if let Some(outcome) = outcomes.navigator.take() {
            self.apply_navigator_outcome(outcome);
        }
        if let Some(outcome) = outcomes.settings.take() {
            if self.apply_settings_outcome(outcome) {
                // Through the fault connection, not straight to a card: every notice raised in a
                // pass reaches the rider together at stage 13, so a failed save shares the card
                // with whatever else this pass found rather than displacing it.
                self.pass.connections.faults.raise(crate::screen::WarningFlags::SETTINGS_ERROR);
            }
        }
        if let Some(outcome) = outcomes.bond.take() {
            if self.bond.apply_outcome(outcome) {
                self.state.bond_status = self.bond.status();
                self.ui.map_dirty = true;
            }
        }
        if let Some(outcome) = outcomes.dfu.take() {
            self.apply_dfu_outcome(outcome);
        }
        if let Some(outcome) = outcomes.storage_info.take() {
            if self.storage.apply_outcome(outcome) {
                self.ui.map_dirty = true;
            }
        }
    }

    fn stage_facts(&mut self, facts: &mut ExternalFacts, derived: DerivedInputs, targets: DerivedTargets<'_>) {
        self.pass.record(PassStage::Facts);
        if let Some(store) = facts.store_revision() {
            if self.pass.store != Some(store) {
                if self.pass.store.is_none_or(|old| old.store != store.store) {
                    self.retention.reset_store();
                    self.navigator.review_store_changed(store.store);
                    if self.pass.store.is_some() {
                        self.catalogs.change_store();
                    }
                    self.catalogs.remount_required = false;
                    let _ = self.pass.connections.catalog_removal.take();
                    self.catalogs.loaded_scope = self.catalogs.loaded_scope.filter(|scope| scope.store == store.store);
                    let _ = self.pass.connections.expiry.take();
                }
                self.pass.store = Some(store);
                // The fact says the store moved; it does not order a re-read. The **domain** owes
                // the read from here on, and stage 6 admits it — so a commit that arrives while the
                // catalog is busy costs a delay rather than a missed rescan, and it coalesces with
                // the reads a completed removal or a failed read owe.
                self.catalogs.note_store_moved();
            }
        }
        if let Some(state) = facts.transfer() {
            self.mode.note_transfer(matches!(state, TransferState::Active));
        }
        if let Some(link) = facts.link() {
            if self.pass.link != Some(link) {
                self.pass.link = Some(link);
                self.set_ble_status(link);
            }
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
    /// Gestures land in the order they were recognised, and a `Hold`/`BackHold` already in the batch
    /// behind a stack-changing gesture is dropped rather than delivered to the screen that replaced
    /// its target (#480). The hold-cancel latch a stack change arms is
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
        // Recorder's session edge, before the world is applied: a host that asked for a ride between
        // two passes must have it open before this pass integrates a fix into it, or the ride would
        // start by discarding its own first frame of motion.
        self.advance_recorder_session();
        self.apply_gesture_batch(gestures);
        self.advance_inputs(now.ride, sensors, route);
    }

    /// Stage 4 — advance `UiRuntime`, then collect the typed intents the rider produced.
    ///
    /// The UI reaches a domain by naming what it wants, never by performing the work: a delete is a
    /// [`CatalogIntent`], not a store operation. Every intent is offered into a slot that is
    /// **checked first** — an intent that cannot be delivered leaves the rider's one-shot exactly
    /// where it was, so nothing is lost by a busy pass.
    ///
    /// The ride close, navigation, update and free-space requests are not taken here: their domains
    /// exist, so the screens name them directly through `Ctx` and there is nothing left in
    /// `Activity` to collect (see the module docs).
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
                // The trip delete already *is* a durable id — the confirm dialog holds the folder's
                // identity rather than a menu row — so there is nothing to resolve here.
                self.activity.take_trip_delete().map(|id| CatalogIntent::DeleteTrip { id })
            };
            if let Some(intent) = intent {
                let _ = self.pass.connections.ui_catalog.try_put(intent);
            }
        }
    }

    /// Retention decisions use only the catalog snapshot that completed its metadata read.
    fn stage_retention(&mut self, effects: &mut EffectSlots, support: PlatformSupport) {
        self.pass.record(PassStage::Retention);
        if let Some(removal) = self.pass.connections.catalog_removal.take() {
            self.retention.note_object_removed(removal.object);
            // The expiry slot can still hold an intent the catalog had no room for last pass. It is
            // a copy of a candidate the verdict has just retired, so admitting it would be a second
            // removal for an object already gone — the rider deleting a route the sweep had queued
            // is exactly that race. Anything about another object still waits its turn.
            if let Some(parked) = self.pass.connections.expiry.take() {
                let stale = matches!(
                    parked,
                    CatalogIntent::DeleteRoute { id } | CatalogIntent::DeleteRide { id } | CatalogIntent::ExpireObject { id, .. } if id == removal.object
                );
                if !stale {
                    let _ = self.pass.connections.expiry.try_put(parked);
                }
            }
        }
        if let Some(activated) = self.pass.connections.route_activated.take() {
            self.with_retention(|retention, view| retention.note_route_activated(activated.route, view));
        }
        if self.pass.connections.catalog_identity.take().is_some() {
            self.retention.note_catalog_changed();
        }
        let Some(scope) = self.catalogs.loaded_scope.filter(|scope| Some(*scope) == self.pass.store) else { return };
        if !support.retention_metadata {
            return;
        }
        self.retention_tick();
        if self.pass.connections.expiry.is_empty() {
            if let Some(intent) = self.with_retention(|retention, view| retention.next_expiry(view)) {
                use crate::catalog_state::CatalogObjectKind;
                let (id, kind) = match intent {
                    CatalogIntent::DeleteRoute { id } => (id, CatalogObjectKind::Route),
                    CatalogIntent::DeleteRide { id } => (id, CatalogObjectKind::Ride),
                    _ => unreachable!("retention expires routes and rides only"),
                };
                let _ = self.pass.connections.expiry.try_put(CatalogIntent::ExpireObject { id, kind, scope });
            }
        }

        if effects.retention.is_empty() {
            let checkpoint = self.navigator.checkpoint_change();
            let effect = checkpoint.and_then(|_| self.retention.next_checkpoint_effect());
            if let Some(effect) = effect {
                self.navigator.checkpoint_issued(effect.token());
            }
            if let Some(mut effect) =
                effect.or_else(|| self.with_retention(|retention, view| retention.next_metadata_effect(view)))
            {
                effect.bind(scope);
                let _ = effects.retention.try_put(effect);
            }
        }
    }

    /// Invalidates admission before any catalog feeder mutates the resident projection.
    pub fn begin_catalog_refresh(&mut self) {
        self.catalogs.loaded_scope = None;
    }

    /// The existing retention owner rechecks volatile policy before the executor queues expiry.
    pub fn retention_expiry_due(
        &mut self,
        id: crate::CatalogObjectId,
        kind: crate::catalog_state::CatalogObjectKind,
        scope: super::StoreRevision,
    ) -> bool {
        self.catalogs.loaded_scope == Some(scope)
            && self.pass.store == Some(scope)
            && self.with_retention(|retention, view| retention.object_due(id, kind, view))
    }

    /// Stage 6 — advance `CatalogMachine`.
    ///
    /// The rider's own request outranks an expiry, and both outrank the store's own re-read, exactly
    /// as the legacy drain has it: a hold-to-delete is something someone is watching happen. Only
    /// the two deletions are *admitted* here — the re-read is owed inside `CatalogMachine` and taken
    /// by [`next_effect`](crate::catalog_state::CatalogState::next_effect_at) when nothing else is
    /// pending, which is that same priority without a second copy of the refresh to lose or double.
    ///
    /// An admitted deletion of the **followed** route reaches Navigator in this pass — the rider is
    /// not left being guided along a route the device has decided to remove — and a store commit is
    /// announced to retention for the next one.
    ///
    /// Recorder's committed ride arrives first, and it orders nothing of its own: it arms the same
    /// owed bit a removal and a store commit arm, so a save that also moved the store revision costs
    /// one read rather than two.
    fn stage_catalog(&mut self, effects: &mut EffectSlots) {
        self.pass.record(PassStage::Catalog);
        if self.pass.connections.ride_finalized.take().is_some() {
            self.catalogs.note_ride_finalized();
        }
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
                self.pass.connections.catalog_identity.defer(CatalogIdentityChanged { revision: store.revision });
            }
        }
        if let Some(effect) = self.catalogs.next_effect_at(self.ui.now_ms) {
            let _ = effects.catalog.try_put(effect);
        }
    }

    /// Hand one intent to the catalog domain, and — when what leaves is the route being followed —
    /// tell Navigator in this same pass. The refusal is handed back to the caller unchanged.
    fn admit_catalog_intent(&mut self, intent: CatalogIntent) -> Result<(), SlotFull<CatalogIntent>> {
        self.catalogs.admit_intent(intent)?;
        if let CatalogIntent::DeleteRoute { id } = intent {
            if self.navigator.route_state().active_route.and_then(|idx| self.catalogs.route_id_at(idx)) == Some(id) {
                let _ = self.pass.connections.active_route_removed.try_put(ActiveRouteRemoved { route: id });
            }
        }
        Ok(())
    }

    /// Stage 7 — `RecorderMachine`'s one bounded operation: a journal checkpoint the cadence owes,
    /// or the close the rider named.
    ///
    /// Gated on [`Capabilities::recorder`](super::Capabilities) — the last pass's level, the same
    /// pattern stage 10 uses. A device with nowhere to put a ride does no recording work at all,
    /// rather than starting operations that fail on their first write.
    ///
    /// The rider's [`Start`](crate::RecorderIntent::Start) is not taken here: opening a session is a
    /// state change, not an effect, and everything from stage 3 on has to see it — see
    /// [`advance_recorder_session`](App::advance_recorder_session) for where it lands and why.
    fn stage_recorder(&mut self, effects: &mut EffectSlots) {
        self.pass.record(PassStage::Recorder);
        let caps = self.pass.capabilities.recorder;
        // The footer's wall-clock anchor goes with the offer, so the figures the executor writes
        // belong to the operation it is about to perform rather than to whenever it gets to it.
        let clock = self.footer_clock();
        if effects.recorder.is_empty() {
            if let Some(effect) = self.recorder.next_effect(caps, clock) {
                let _ = effects.recorder.try_put(effect);
            }
        }
    }

    /// Recorder's session edge — run wherever the rider's Start can have arrived, and idempotent
    /// when none has.
    ///
    /// Two call sites, each with its own reason. Stage 3 runs it **before** the world is applied,
    /// because a host that asked for a ride between two passes must have it open before this pass
    /// integrates a fix into it. [`apply_gesture`](App::apply_gesture) runs it after each gesture,
    /// because a pass applies a **batch**: without it the second gesture of a batch would still read
    /// "not recording" after the first one started the ride. One implementation, two call sites —
    /// the shape `advance_inputs` and `retention_tick` already use.
    pub(crate) fn advance_recorder_session(&mut self) {
        match self.recorder.advance(self.pass.capabilities.recorder) {
            crate::recorder::RecorderAdvance::Opened(start) => self.begin_ride_session(start),
            // The rider asked to record and this device cannot. The request is kept, so a card that
            // mounts later still opens the ride they asked for — but they are told now, through the
            // same recording warning a refused first write raises. A riding view that quietly
            // records nothing is the failure this raise exists to prevent.
            crate::recorder::RecorderAdvance::Refused => {
                self.pass.connections.faults.raise(crate::screen::WarningFlags::REC_ERROR);
            }
            // A damaged recovered object is still standing, so no session opened. Put the decision
            // back rather than a warning: it is the one thing between the rider and recording, and
            // it is the thing they can act on.
            crate::recorder::RecorderAdvance::RecoveryOwed => {
                self.activity.mode = crate::activity::Mode::Idle;
                self.raise_ride_recovery();
            }
            crate::recorder::RecorderAdvance::Nothing => {}
        }
    }

    /// A ride session opened: re-lock the matcher, restart the trail and the pace window, and — for
    /// a fresh ride, never a recovered continuation — zero the accumulators and drop any detour in
    /// flight.
    ///
    /// Recorder is the only thing that knows a session opened, and it is the only thing that knows
    /// whether it continues a recovered one.
    fn begin_ride_session(&mut self, start: crate::recorder::SessionStart) {
        self.navigator.relock_matcher();
        if start == crate::recorder::SessionStart::Fresh {
            self.navigator.reset_ride();
            self.recorder.reset_totals();
            self.navigator.reset_detour();
        }
        self.recorder.restart_buffers();
        self.ui.map_dirty = true;
    }

    /// The ride session closed, saved or discarded: the matcher, the totals and the trail all go
    /// back to their between-rides state, so nothing is left showing the ride that just ended.
    ///
    /// The trail restarts on **both** edges, and neither is the other's spare. This one is "the
    /// ride the trail belonged to is over"; [`begin_ride_session`](App::begin_ride_session)'s is "a
    /// new ride starts with an empty trail", which is also what a recovered continuation needs —
    /// it keeps the totals the journal restored and still opens on a clean trail.
    fn end_ride_session(&mut self) {
        self.navigator.relock_matcher();
        self.navigator.reset_ride();
        self.recorder.reset_totals();
        self.navigator.reset_detour(); // the ride the detour was planned for is over
        self.recorder.restart_buffers();
        self.ui.map_dirty = true;
    }

    /// Stage 8 — advance `Navigator`.
    ///
    /// Consumes the catalog's [`ActiveRouteRemoved`] in the same pass it was sent, and reports an
    /// activation to retention in the next one — an active route must not expire underneath the ride
    /// it is guiding. Then hands the executor at most one planning operation.
    ///
    /// There is no `UiRuntime` → `Navigator` connection to drain: a planning screen names its
    /// request to Navigator as it happens (`Ctx::navigator`), so the rider's plan is already with
    /// its owner before stage 1 of this pass — earlier than a slot could deliver it, and in the one
    /// place that also serves a seam running between two passes.
    fn stage_navigator(&mut self, effects: &mut EffectSlots) {
        self.pass.record(PassStage::Navigator);
        if let Some(removed) = self.pass.connections.active_route_removed.take() {
            if self.navigator.route_state().active_route.and_then(|idx| self.catalogs.route_id_at(idx))
                == Some(removed.route)
            {
                self.navigator.set_active_route(None);
                self.drop_route_derived_state();
                self.ui.map_dirty = true;
            }
        }
        let active = self.navigator.route_state().active_route.and_then(|idx| self.catalogs.route_id_at(idx));
        if active != self.pass.active_route {
            self.pass.active_route = active;
            if let Some(route) = active {
                self.pass.connections.route_activated.defer(RouteActivated { route });
            }
        }
        if effects.navigator.is_empty() {
            if let Some(effect) = self.navigator.next_effect(&mut self.mode) {
                let _ = effects.navigator.try_put(effect);
            }
        }
    }

    fn stage_settings(&mut self, effects: &mut EffectSlots) {
        self.pass.record(PassStage::Settings);
        if effects.settings.is_empty() {
            if let Some(effect) = self.next_settings_effect() {
                let _ = effects.settings.try_put(effect);
            }
        }
    }

    /// Offer the pending settings write after the rider leaves the settings subtree.
    pub(crate) fn next_settings_effect(&mut self) -> Option<crate::settings::SettingsEffect> {
        let (in_subtree, now_ms) = (self.ui.top_is_settings(), self.ui.now_ms);
        self.settings_ops.next_effect(in_subtree, now_ms)
    }

    /// Consume a settings write result and report whether the rider must see a failure.
    pub(crate) fn apply_settings_outcome(&mut self, outcome: crate::settings::SettingsOutcome) -> bool {
        let now_ms = self.ui.now_ms;
        self.settings_ops.apply_outcome(outcome, now_ms)
    }

    /// Stage 11 — advance `DfuState`, `BondState` and `StorageInfo`.
    ///
    fn stage_platform(&mut self, effects: &mut EffectSlots, support: PlatformSupport) {
        self.pass.record(PassStage::Platform);
        if core::mem::take(&mut self.state.ble_forget_requested) {
            self.bond.request(support.bonding, self.state.device.ble_paired);
        }
        if let Some(effect) = self.bond.next_effect() {
            let _ = effects.bond.try_put(effect);
        }
        if self.state.bond_status != self.bond.status() {
            self.state.bond_status = self.bond.status();
            self.ui.map_dirty = true;
        }
        if effects.dfu.is_empty() {
            if let Some(effect) = self.dfu.next_effect() {
                let _ = effects.dfu.try_put(effect);
            }
        }
        if effects.storage_info.is_empty() {
            if let Some(effect) = self.storage.next_effect() {
                let _ = effects.storage_info.try_put(effect);
            }
        }
    }

    /// Stage 12 — `CoreMode`: recalculate what this device can do at all.
    ///
    /// A capability is a level, never latched: it is recomputed from what the image implements and
    /// what is currently true (a mounted store, a routing graph, a recording ride) and from
    /// [`CoreMode`](crate::device_core::core_mode::CoreMode)'s heavy-work verdict. Heavy work is
    /// withdrawn while a transfer holds the store **or a planner run holds the nav arm** — which is
    /// what stops a second plan or an install from starting, rather than letting one start and
    /// fail. The mode is the only store of either level; this stage re-derives nothing.
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

            link_connected: matches!(self.state.device.ble_link, crate::ble::BleLink::Connected),
            ride_recording: self.recorder.recording(),
            heavy_operations: self.mode.admits_heavy(),
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
            sources: SourceNeeds {
                map: self.base_needs_reader(),
                route: self.navigator.route_state().active_route.is_some(),
            },
            effects,
            immediate,
        }
    }

    /// Stage a mounted card, as stage 2 and stage 12 would: the level that admits catalog mutation
    /// and ride recording, without driving a whole frame for it.
    #[cfg(test)]
    pub(crate) fn test_mount_store(&mut self) {
        self.pass.store = Some(StoreRevision { store: super::StoreIdentity::new(1), revision: Revision::new(1) });
        self.catalogs.loaded_scope = self.pass.store;
        self.pass.capabilities.recorder = crate::device_core::RecorderCapabilities { record: true };
    }

    /// Stage a mounted card and an open ride, as stage 12 and stage 7 would.
    #[cfg(test)]
    pub(crate) fn test_start_ride(&mut self) {
        self.test_mount_store();
        self.recorder.request(crate::RecorderIntent::Start);
        self.advance_recorder_session();
        assert!(self.recorder.recording(), "a mounted store admits the ride");
    }

    /// Close the staged ride the way a store's `Discarded` verdict does, without an executor.
    #[cfg(test)]
    pub(crate) fn test_end_ride(&mut self) {
        self.recorder.request(crate::RecorderIntent::Discard);
        let effect = self
            .recorder
            .next_effect(self.pass.capabilities.recorder, self.footer_clock())
            .expect("an open ride offers its close");
        let verdict =
            self.recorder.apply_outcome(crate::recorder::RecorderOutcome::Discarded { token: effect.token() });
        assert_eq!(verdict, crate::recorder::RecorderVerdict::Dropped);
        self.end_ride_session();
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
    use crate::catalog_state::{CatalogEffect, CatalogError, CatalogOutcome};
    use crate::device_core::{StoreIdentity, TokenSource};
    use crate::retention::SweepKind;
    use crate::retention::{Retention, RouteRetentionMeta};
    use crate::route::RouteSummary;
    use crate::screen::WarningFlags;

    use crate::Screen;
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
        pass_supported(app, EVERYTHING, now, gestures, outcomes, facts, derived, targets)
    }

    /// Every capability this crate's own tests assume unless one is named.
    const EVERYTHING: PlatformSupport = PlatformSupport {
        detour: true,
        settings_persistence: true,
        dfu: true,

        bonding: true,
        storage_space_report: true,
        retention_metadata: true,
    };

    #[allow(clippy::too_many_arguments)]
    fn pass_supported(
        app: &mut App,
        support: PlatformSupport,
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

            support,
            outcomes,
            facts,
            derived,
            targets,
        })
    }

    /// One quiet pass on a platform with no durable retention metadata — the board.
    fn quiet_without_metadata(app: &mut App, ms: u32) -> PassPlan {
        let mut facts = ExternalFacts::NONE;
        pass_supported(
            app,
            PlatformSupport { retention_metadata: false, ..EVERYTHING },
            PassClock { ride: RideClock(ms), ui: InputClock(ms) },
            &[],
            &mut OutcomeSlots::new(),
            &mut facts,
            DerivedInputs::NONE,
            DerivedTargets::NONE,
        )
    }

    /// A **dismissed terminal map-transfer card must stay dismissed.**
    ///
    /// The card is a *level* family: the scheduler re-delivers it every sweep for as long as the
    /// desired state is `Some`. A terminal card pops itself on a press, but the press and the sweep
    /// that re-lands it are stages of the **same** pass — so the card is back before the pass ends,
    /// and the board's dismissal latch (which clears the published state only when it observes a
    /// card that *was* up and no longer is) never gets to fire. The card then outlives every
    /// dismissal and the rider cannot leave it.
    ///
    /// `card_scheduler::map_transfer_card_opens_updates_and_closes` misses this because it asserts
    /// right after `apply_gesture` and never runs another pass.
    #[test]
    fn a_dismissed_terminal_map_transfer_card_does_not_come_back() {
        use crate::screen::MapTransfer;
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));

        app.set_map_transfer(Some(MapTransfer::Installed));
        quiet_without_metadata(&mut app, 1_000);
        assert!(app.map_transfer_card_up(), "a terminal state raises the card");

        let mut facts = ExternalFacts::NONE;
        pass_supported(
            &mut app,
            PlatformSupport { retention_metadata: false, ..EVERYTHING },
            PassClock { ride: RideClock(2_000), ui: InputClock(2_000) },
            &[Gesture::Press],
            &mut OutcomeSlots::new(),
            &mut facts,
            DerivedInputs::NONE,
            DerivedTargets::NONE,
        );
        assert!(
            !app.map_transfer_card_up(),
            "the press that pops a terminal card must leave it popped for the rest of that pass"
        );

        quiet_without_metadata(&mut app, 3_000);
        assert!(
            !app.map_transfer_card_up(),
            "the dismissed card came back on the next pass — the rider cannot get off this screen"
        );
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
        app.test_mount_store();
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
            matches!(effects.catalog.take(), Some(CatalogEffect::ExpireObject { object: 22, .. })),
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

    #[test]
    fn unsupported_metadata_never_mirrors_or_authorizes_expiry() {
        let (mut app, _) = expiring_app();
        let before = app.route_metas().to_vec();
        for ms in [10, 20, 30, 3_600_010] {
            let plan = quiet_without_metadata(&mut app, ms);
            assert!(plan.effects.retention.is_empty());
            assert!(plan.effects.catalog.is_empty());
            assert_eq!(app.route_metas(), before);
        }
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
    /// Written on the activation, which is the deferred producer this wiring actually has: Navigator
    /// runs after retention, so an activation cannot reach backwards and waits a pass.
    #[test]
    fn a_deferred_value_forces_another_pass_before_sleep() {
        let mut app = navigating(); // route 0 is active before the first pass
        app.activity.mode = Mode::Riding;

        let plan = quiet(&mut app, 10);
        assert!(app.pass.connections.route_activated.is_pending(), "the activation waits for retention");
        assert!(plan.immediate && plan.next_wake_ms == Some(0), "so the runtime comes straight back");

        // The next pass consumes it before anything else, and then there is nothing to hurry for.
        let plan = quiet(&mut app, 20);
        assert!(!app.pass.connections.route_activated.is_pending());
        assert!(!plan.immediate && plan.next_wake_ms != Some(0));

        // A second activation right behind the first is deposited just the same.
        app.activate_route(1);
        let plan = quiet(&mut app, 30);
        assert!(app.pass.connections.route_activated.is_pending() && plan.immediate);
    }

    /// The rider's Save becomes a `Finalize` effect in the **same** pass that applied the gesture.
    ///
    /// Routed through a stage-4 slot instead, the close would wait a pass — reinstating the wake gap
    /// would otherwise defer the work until the next external event.
    #[test]
    fn the_riders_save_becomes_a_finalize_effect_in_the_same_pass() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.activity.mode = Mode::Riding;
        app.test_start_ride();
        quiet(&mut app, 10);

        app.recorder.request(crate::RecorderIntent::Save);
        let plan = quiet(&mut app, 20);
        let mut effects = plan.effects;
        let effect = effects.recorder.take().expect("the close left as a bounded operation");
        assert!(matches!(effect, crate::recorder::RecorderEffect::Finalize { .. }), "{effect:?}");
        assert!(app.recording(), "and the ride stays open until the store answers");

        // The verdict closes it, and orders exactly one catalog read for the committed ride.
        let mut outcomes = OutcomeSlots::new();
        outcomes
            .recorder
            .try_put(crate::recorder::RecorderOutcome::Finalized { token: effect.token(), ride: 77 })
            .unwrap();
        let mut facts = ExternalFacts::NONE;
        let plan = pass_with(&mut app, 30, &[], &mut outcomes, &mut facts);
        assert!(!app.recording(), "the store's verdict is what closes the ride");
        let mut effects = plan.effects;
        assert!(
            matches!(effects.catalog.take(), Some(CatalogEffect::ReadCatalog { .. })),
            "the committed ride ordered the catalog's own re-read"
        );
    }

    /// A ride is refused without a writable store — `Capabilities::recorder.record` is the gate and
    /// stage 7 its reader — and the rider is **told**, through the whole chain from the absent fact
    /// to the card on glass.
    ///
    /// Both halves are the property. Dropping the gate starts a ride with nowhere to put it; keeping
    /// the gate but swallowing the refusal gives the rider a riding view that records nothing and
    /// never says so, which is the worse of the two.
    #[test]
    fn recording_is_refused_without_a_writable_store() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.activity.mode = Mode::Riding;
        quiet(&mut app, 10); // no store fact has ever been reported

        app.recorder.request(crate::RecorderIntent::Start);
        let plan = quiet(&mut app, 20);
        assert!(!app.recording(), "no store, no ride");
        assert!(plan.effects.recorder.is_empty(), "and nothing physical is offered for it");
        assert!(
            matches!(app.top_screen(), Screen::Warning(card) if card.flags().contains(WarningFlags::REC_ERROR)),
            "the rider is told, rather than left on a riding view that records nothing"
        );
        // One card per refusal, not one per pass: the request stays pending, so a re-raise every
        // pass would be a warning the rider cannot get out of.
        app.apply_gesture(Gesture::Press); // dismiss it
        quiet(&mut app, 30);
        assert!(!matches!(app.top_screen(), Screen::Warning(_)), "the card does not come back");

        // The card mounts. The request the rider already made is still theirs, and it opens the ride
        // — nothing was destroyed by a device that could not serve it yet.
        let mut facts = ExternalFacts::NONE;
        facts.note_store_revision(StoreRevision { store: StoreIdentity::new(1), revision: Revision::new(1) });
        pass_with(&mut app, 40, &[], &mut OutcomeSlots::new(), &mut facts);
        quiet(&mut app, 50);
        assert!(app.recording(), "the kept request opened the ride the rider asked for");
    }

    /// A new session clears the trail and the pace window, and a fresh one zeroes the totals.
    ///
    /// The reset is re-derived by Recorder's own session edge; leaving it in `sync_route_state`
    /// against a mirrored session id is what let the previous ride's trail survive.
    #[test]
    fn a_new_session_clears_every_accumulator() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.activity.mode = Mode::Riding;
        app.test_start_ride();
        app.recorder.breadcrumb.push(1_000, 2_000);

        app.recorder.record_fix(obc_ports::Fix::at(0, 0), 0, true);
        app.recorder.record_fix(obc_ports::Fix::at(100, 0), 1_000, true);
        assert!(!app.recorder.breadcrumb.is_empty());
        assert!(app.recorder.ridden_m() > 0.0 && !app.recorder.staged().is_empty(), "the ride accumulated");

        app.test_end_ride();
        assert!(app.recorder.breadcrumb.is_empty(), "the ride the trail belonged to is over");

        // …and the open edge restarts them too, which is what a recovered continuation needs: it
        // keeps the totals the journal restored and still opens on a clean trail.
        app.recorder.breadcrumb.push(1_000, 2_000);

        app.test_start_ride();
        assert!(app.recorder.breadcrumb.is_empty(), "a new ride starts with an empty trail");

        app.recorder.assert_totals_are_zero(); // a fresh ride starts at zero
        assert!(app.recorder.staged().is_empty(), "and owes no sample the previous ride never wrote");
    }

    /// **"Save & start new" gives ride two nothing of ride one's**, and both halves of the gesture
    /// run in one pass: the verdict lands at stage 1 and the fresh ride opens at stage 3. That is
    /// the shape that makes the integration anchors part of the reset and not an afterthought — a
    /// new ride that kept them would credit itself with the step from where the last one ended, and
    /// its first sample would continue that ride's segment instead of opening its own.
    #[test]
    fn save_and_start_new_gives_the_second_ride_nothing_of_the_first() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.activity.mode = Mode::Riding;
        app.test_start_ride();
        app.recorder.record_fix(obc_ports::Fix::at(0, 0), 0, true);
        app.recorder.record_fix(obc_ports::Fix::at(0, 100), 1_000, true);
        assert!(app.recorder.ridden_m() > 0.0, "ride one covered ground");

        // The gesture, exactly as the pass serves it.
        app.recorder.save_and_restart();
        let effect =
            app.recorder.next_effect(app.pass.capabilities.recorder, app.footer_clock()).expect("the close goes first");
        let verdict =
            app.recorder.apply_outcome(crate::recorder::RecorderOutcome::Finalized { token: effect.token(), ride: 1 });
        assert_eq!(verdict, crate::recorder::RecorderVerdict::Saved(1));
        app.end_ride_session(); // stage 1
        app.advance_recorder_session(); // stage 3
        assert!(app.recording(), "the second half of the gesture opened the new ride");
        app.recorder.assert_totals_are_zero();

        // Ride two's first fix, a second later and ~11 m on from where ride one ended.
        app.recorder.record_fix(obc_ports::Fix::at(0, 200), 2_000, true);
        assert_eq!(app.recorder.ridden_m(), 0.0, "the step between the two rides belongs to neither");
        assert_eq!(app.recorder.moving_s(), 0.0, "and it books no moving time either");
        assert!(app.recorder.staged()[0].segment_start, "ride two's first sample opens ride two's segment");
    }

    /// A recovered ride continues without resetting the totals the journal restored.
    #[test]
    fn a_recovered_ride_continues_without_resetting_its_totals() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let restored = crate::RideContinuation { ridden_m: 12_345.0, moving_s: 2_700.0, ..Default::default() };
        assert!(app.offer_recovered_ride(restored));
        // The card's Continue: a session that keeps what the journal restored.
        app.pass.store = Some(StoreRevision { store: StoreIdentity::new(1), revision: Revision::new(1) });
        app.pass.capabilities.recorder = crate::device_core::RecorderCapabilities { record: true };
        app.recorder.continue_recovered();
        app.advance_recorder_session();
        assert!(app.recording());
        assert_eq!(app.recorder.continuation(), restored, "recovery must not zero the ride it just restored");
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
            matches!(effects.catalog.take(), Some(CatalogEffect::ExpireObject { object: 22, .. })),
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

    /// A store commit arms the domain's owed refresh, not a rescan cue: the fact reports that
    /// the store moved, `CatalogState::note_store_moved` arms the one bit that orders the
    /// re-read, and one commit orders exactly one.
    #[test]
    fn a_store_commit_raises_one_catalog_refresh() {
        let mut app = navigating();
        quiet(&mut app, 10);

        let mut facts = committed(4);
        let plan = pass_with(&mut app, 20, &[], &mut OutcomeSlots::new(), &mut facts);
        let mut effects = plan.effects;
        let effect = effects.catalog.take().expect("the commit ordered a re-read");
        assert!(matches!(effect, CatalogEffect::ReadCatalog { .. }));

        // The same revision again is the same edge, and the answered read starts nothing new.
        let mut outcomes = OutcomeSlots::new();
        outcomes.catalog.try_put(CatalogOutcome::CatalogRead { token: effect.token(), scope: None }).unwrap();
        let mut same = committed(4);
        let plan = pass_with(&mut app, 30, &[], &mut outcomes, &mut same);
        assert!(plan.effects.catalog.is_empty(), "one commit, one refresh");

        // A busy catalog delays the refresh rather than losing it: the rider's delete goes first
        // and the commit's own re-read follows on a later pass.
        app.activity.request_route_delete(1);
        let mut next = committed(5);
        let plan = pass_with(&mut app, 40, &[], &mut OutcomeSlots::new(), &mut next);
        let mut effects = plan.effects;
        let delete = effects.catalog.take().expect("the rider outranks the re-read");
        assert!(matches!(delete, CatalogEffect::RemoveObject { object: 22, .. }));

        let mut outcomes = OutcomeSlots::new();
        outcomes
            .catalog
            .try_put(CatalogOutcome::ObjectRemoved { token: delete.token(), object: 22, existed: true })
            .unwrap();
        let mut none = ExternalFacts::NONE;
        let plan = pass_with(&mut app, 50, &[], &mut outcomes, &mut none);
        let mut effects = plan.effects;
        assert!(
            matches!(effects.catalog.take(), Some(CatalogEffect::ReadCatalog { .. })),
            "the delayed refresh went out once the domain had room"
        );
    }

    /// An unreadable catalog waits thirty seconds before retrying, without requesting busy passes.
    #[test]
    fn a_failed_read_waits_for_the_retry_deadline() {
        let mut app = navigating();
        quiet(&mut app, 10);
        quiet(&mut app, 20); // the boot activation's deferred value, consumed

        let mut facts = committed(4);
        let plan = pass_with(&mut app, 30, &[], &mut OutcomeSlots::new(), &mut facts);
        let mut effects = plan.effects;
        let first = effects.catalog.take().expect("the commit ordered a re-read");

        // The card did not answer. The domain is free — but the read is still owed.
        let mut outcomes = OutcomeSlots::new();
        outcomes
            .catalog
            .try_put(CatalogOutcome::Failed { token: first.token(), error: CatalogError::Unreadable })
            .unwrap();
        let mut none = ExternalFacts::NONE;
        let plan = pass_with(&mut app, 40, &[], &mut outcomes, &mut none);
        assert!(!plan.immediate, "the retry is per wake — arming never asks for an immediate pass");
        let mut effects = plan.effects;
        assert!(effects.catalog.take().is_none(), "failure waits instead of polling the card each pass");
        let mut effects = quiet(&mut app, 30_040).effects;
        let second = effects.catalog.take().expect("the failed read is re-offered after its deadline");
        assert!(matches!(second, CatalogEffect::ReadCatalog { .. }));
        assert_ne!(first.token(), second.token(), "a new operation, not the old one resurrected");

        // …and once it lands, nothing is owed.
        let mut outcomes = OutcomeSlots::new();
        outcomes.catalog.try_put(CatalogOutcome::CatalogRead { token: second.token(), scope: None }).unwrap();
        let mut none = ExternalFacts::NONE;
        let plan = pass_with(&mut app, 30_050, &[], &mut outcomes, &mut none);
        assert!(plan.effects.catalog.is_empty(), "the read landed, so the store is no longer ahead of us");
    }

    /// The store-revision edge and a completed delete **coalesce into one read**. The owed re-read
    /// is a bit and not a counter, so the board — which raises a revision for its own removals —
    /// gains no second rescan from this slice.
    #[test]
    fn a_store_edge_and_a_completed_delete_coalesce_into_one_read() {
        let mut app = navigating();
        quiet(&mut app, 10);

        app.activity.request_route_delete(1);
        let plan = quiet(&mut app, 20);
        let mut effects = plan.effects;
        let delete = effects.catalog.take().expect("the rider's delete");

        // The executor performs it, and the store's revision moves with it: two reasons, one read.
        let mut outcomes = OutcomeSlots::new();
        outcomes
            .catalog
            .try_put(CatalogOutcome::ObjectRemoved { token: delete.token(), object: 22, existed: true })
            .unwrap();
        let mut facts = committed(4);
        let plan = pass_with(&mut app, 30, &[], &mut outcomes, &mut facts);
        let mut effects = plan.effects;
        let read = effects.catalog.take().expect("the removal ordered a re-read");
        assert!(matches!(read, CatalogEffect::ReadCatalog { .. }));

        let mut outcomes = OutcomeSlots::new();
        outcomes.catalog.try_put(CatalogOutcome::CatalogRead { token: read.token(), scope: None }).unwrap();
        let mut none = ExternalFacts::NONE;
        let plan = pass_with(&mut app, 40, &[], &mut outcomes, &mut none);
        assert!(plan.effects.catalog.is_empty(), "one read, not one per reason");
    }

    /// The owed re-read is spent **after** the rider's intent, in the same pass that owes both: a
    /// hold-to-delete is something someone is watching happen, and bookkeeping waits behind it.
    #[test]
    fn an_owed_refresh_yields_to_the_riders_delete() {
        let mut app = navigating();
        quiet(&mut app, 10);

        app.activity.request_route_delete(1);
        let mut facts = committed(4);
        let plan = pass_with(&mut app, 20, &[], &mut OutcomeSlots::new(), &mut facts);
        let mut effects = plan.effects;
        assert!(
            matches!(effects.catalog.take(), Some(CatalogEffect::RemoveObject { object: 22, .. })),
            "the rider's delete goes first, and the owed read is still owed"
        );
    }

    // ==================== outcomes ====================

    /// A domain consumes its own outcome and rejects one it has moved past. Only a domain that owns
    /// a token source may consume at all — an outcome nobody can validate stays in its slot rather
    /// than being dropped or guessed at.
    #[test]
    fn an_outcome_is_consumed_by_its_owner_and_left_alone_without_one() {
        let mut app = navigating();
        quiet(&mut app, 10);

        let mut outcomes = OutcomeSlots::new();

        // Recorder owns a token source too, so its answer is consumed — and refused, because the
        // token is one it never issued.
        let mut recorder_ops: TokenSource<crate::device_core::RecorderTag> = TokenSource::new();
        app.test_start_ride();
        outcomes.recorder.try_put(crate::recorder::RecorderOutcome::Discarded { token: recorder_ops.issue() }).unwrap();

        // An unissued bond result is consumed and rejected.
        let mut bond_ops: TokenSource<crate::device_core::BondTag> = TokenSource::new();
        let bond = crate::ble::BondOutcome::KeysRemoved {
            token: bond_ops.issue(),
            controller: crate::ble::ControllerClearance::Confirmed,
        };
        outcomes.bond.try_put(bond).unwrap();

        let mut none = ExternalFacts::NONE;
        pass_with(&mut app, 20, &[], &mut outcomes, &mut none);

        assert!(app.recording(), "and neither is a recorder token Recorder never issued");
        assert!(outcomes.recorder.is_empty(), "the owner consumed it, which is what refusing it means");
        assert!(outcomes.bond.is_empty(), "the bond owner rejects an unissued result");
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
        assert!(app.pass.capabilities.catalog.mutate, "a mounted store may be mutated");

        let mut streaming = ExternalFacts::NONE;
        streaming.note_transfer(TransferState::Active);
        pass_with(&mut app, 20, &[], &mut OutcomeSlots::new(), &mut streaming);
        assert!(!app.pass.capabilities.dfu.install, "an install is heavy — never while a transfer streams");

        let mut idle = ExternalFacts::NONE;
        idle.note_transfer(TransferState::Idle);
        pass_with(&mut app, 30, &[], &mut OutcomeSlots::new(), &mut idle);
        assert!(app.pass.capabilities.dfu.install, "and it comes straight back");
    }

    /// The axis this stage did not have before #1397 S5: a **live search** is heavy too. The nav
    /// arm is one block, so a second plan started mid-search could only fail — withdrawing the
    /// capability is what stops it being offered rather than offered and refused.
    #[test]
    fn admission_withdraws_heavy_work_while_a_search_holds_the_nav_arm() {
        let mut app = navigating();
        app.state.has_nav_graph = true; // planning needs a graph before admission can be the deciding axis
        let mut facts = committed(1);
        pass_with(&mut app, 10, &[], &mut OutcomeSlots::new(), &mut facts);
        let caps = app.pass.capabilities;
        assert!(caps.navigator.plan_route && caps.navigator.plan_detour && caps.dfu.install);

        app.debug_set_plan_live(true);
        let mut none = ExternalFacts::NONE;
        pass_with(&mut app, 20, &[], &mut OutcomeSlots::new(), &mut none);
        let caps = app.pass.capabilities;
        assert!(!caps.navigator.plan_route, "a second route plan cannot start while one runs");
        assert!(!caps.navigator.plan_detour, "nor a detour");
        assert!(!caps.dfu.install, "nor an install — it reboots, and the search would go with it");
        assert!(caps.navigator.commit_detour, "but a splice is a write, not a search: still offered");
        assert!(caps.catalog.mutate, "and the store is still writable — this is admission, not a fault");

        app.debug_set_plan_live(false);
        let mut none = ExternalFacts::NONE;
        pass_with(&mut app, 30, &[], &mut OutcomeSlots::new(), &mut none);
        assert!(app.pass.capabilities.navigator.plan_route, "the answer hands the arm back");
    }

    /// A platform callback cannot change DeviceCore in the middle of a pass. `run_pass` holds
    /// `&mut self` for its whole length, so no safe caller can reach a push door mid-pass at all —
    /// the flag has to be set by hand here. Reaching one anyway is a caller bug, so it is loud in
    /// debug and refused in release rather than quietly losing the answer.
    ///
    /// One door is left to check it on: with the legacy event door gone, `apply_derived` is the only
    /// remaining way into DeviceCore that is not `run_pass` itself.
    #[test]
    #[should_panic(expected = "cannot change DeviceCore during a pass")]
    fn a_callback_cannot_mutate_core_state_during_a_pass() {
        let mut app = navigating();
        app.set_rides(&[crate::RideEntry { id: 7, summary: ride_summary() }]);
        app.activity.viewed_ride = Some(0);
        let plan = quiet(&mut app, 10);
        let key = plan.derived_needs.ride_track.expect("the open ride detail needs its track");
        app.pass.in_pass = true;
        app.apply_derived(
            DerivedInputs::ride_track(crate::device_core::DerivedInput::filled(key)),
            DerivedTargets::NONE,
        );
    }

    /// The same door, outside a pass: it works exactly as it always did.
    #[test]
    fn a_push_outside_a_pass_is_applied_normally() {
        let mut app = navigating();
        app.set_rides(&[crate::RideEntry { id: 7, summary: ride_summary() }]);
        app.activity.viewed_ride = Some(0);
        let plan = quiet(&mut app, 10);
        let key = plan.derived_needs.ride_track.expect("the open ride detail needs its track");
        app.apply_derived(
            DerivedInputs::ride_track(crate::device_core::DerivedInput::filled(key)),
            DerivedTargets::NONE,
        );
        assert!(quiet(&mut app, 20).derived_needs.ride_track.is_none(), "the answer landed");
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
        app.set_rides(&[crate::RideEntry { id: 7, summary: ride_summary() }]);
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

        let mut facts = committed(9);
        facts.note_link(connected);

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

        assert_eq!(app.state.device.ble_link, crate::ble::BleLink::Connected, "the link state reached the UI");

        // One-shots were consumed rather than left for a second delivery.
        assert!(facts.take_route_upload().is_none() && facts.take_trip_upload().is_none());
        assert!(facts.take_update_result().is_none() && facts.take_warnings().is_empty());
        assert!(app.debug_stack_len() > 1, "and what they post reached the screen");

        // The keyed answer cleared the need it answered.
        assert!(plan.derived_needs.ride_track.is_none(), "the ride track is answered");
    }

    /// The overlay converter's two rules, on the levels themselves (#1447).
    ///
    /// The **freeze** half is the named regression the engaged level exists for: the search's own
    /// start edge fires under the planning spinner, where nothing freezes — and the pass that puts a
    /// map base back under the still-live search raises no search edge at all. A host keyed on the
    /// start edge would never be told to paint the banner for the whole of that search.
    #[test]
    fn the_overlay_key_repaints_on_the_engaged_level_and_the_bulge_trailing_edge() {
        let mut pass = PassState::new();
        let quiet = OverlayKey { hold: false, freeze: false };
        assert!(!pass.overlay_repaint(quiet), "at rest there is nothing to repaint");

        // A search under the opaque spinner: a chrome base, so nothing is engaged.
        assert!(!pass.overlay_repaint(OverlayKey { hold: false, freeze: false }));
        // The spinner goes and a map base is back — no search edge, but *this* is the freeze.
        assert!(pass.overlay_repaint(OverlayKey { hold: false, freeze: true }), "the banner appears");
        assert!(
            !pass.overlay_repaint(OverlayKey { hold: false, freeze: true }),
            "a level, so one repaint — not one per ride-loop pass"
        );
        assert!(pass.overlay_repaint(quiet), "and one more to take the banner off");
        assert!(!pass.overlay_repaint(quiet));

        // The bulge: live while it animates, plus exactly one trailing frame to clear it.
        let bulge = OverlayKey { hold: true, freeze: false };
        assert!(pass.overlay_repaint(bulge), "a charging bulge paints");
        assert!(pass.overlay_repaint(bulge), "…every frame it is live");
        assert!(pass.overlay_repaint(quiet), "the trailing clear frame");
        assert!(!pass.overlay_repaint(quiet), "and then quiet");
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
    #[test]
    fn durable_ride_overlay_covers_the_full_inventory_and_expiry_rechecks_live_policy() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let entries: std::vec::Vec<_> =
            (1..=crate::UI_RIDES_CAP as u64).map(|id| crate::RideEntry { id, summary: ride_summary() }).collect();
        let records: std::vec::Vec<_> = (1..=crate::MAX_RIDES as u64)
            .map(|id| crate::RideRetentionRecord { id, synced: false, synced_at_utc: 0 })
            .collect();
        app.set_rides(&entries);
        app.set_ride_retention_inventory(&records);
        app.set_ride_archive_proof(1, 0);
        app.set_ride_archive_proof(128, 1_600_000_000);
        app.set_ride_archive_proof(129, 1_600_000_000);
        assert!(app.rides()[0].summary.synced);
        assert_eq!(app.catalogs.ride_records().len(), 128);
        assert!(app.catalogs.ride_records()[127].synced);
        app.test_mount_store();
        let scope = app.pass.store.unwrap();
        assert!(
            !app.retention_expiry_due(128, crate::catalog_state::CatalogObjectKind::Ride, scope),
            "unknown clock protects even a nonzero proof"
        );
        trust_clock(&mut app);
        assert!(
            !app.retention_expiry_due(1, crate::catalog_state::CatalogObjectKind::Ride, scope),
            "zero stamp starts a clock, never expires"
        );
        assert!(
            !app.retention_expiry_due(2, crate::catalog_state::CatalogObjectKind::Ride, scope),
            "unsynced is protected"
        );
        assert!(app.retention_expiry_due(128, crate::catalog_state::CatalogObjectKind::Ride, scope));
        app.test_start_ride();
        assert!(
            !app.retention_expiry_due(128, crate::catalog_state::CatalogObjectKind::Ride, scope),
            "recording blocks expiry"
        );
        app.test_end_ride();
        assert!(app.retention_expiry_due(128, crate::catalog_state::CatalogObjectKind::Ride, scope));
        app.begin_catalog_refresh();
        assert!(
            !app.retention_expiry_due(128, crate::catalog_state::CatalogObjectKind::Ride, scope),
            "partial refresh has no authority"
        );
    }
}
