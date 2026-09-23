use obc_ports::{InputClock, RideClock, Sensors};
use obc_route::RouteReader;

use crate::catalog_state::CatalogIntent;
use crate::device_core::connections::{ActiveRouteRemoved, RideFinalized};
use crate::dirty::Dirty;
use crate::input::Gesture;
use crate::render_key::Repaint;
use crate::App;

use super::connections::Connections;
use super::derived::{DerivedInputs, DerivedNeeds, DerivedTargets};
use super::{
    Capabilities, DeviceFacts, EffectSlots, ExternalFacts, OutcomeSlots, PlatformSupport, SlotFull, StoreRevision,
    TransferState, UpdateResult,
};

/// The pass stages, in the order [`App::run_pass`] runs them. Each runs exactly once.
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
    Catalog,
    Recorder,
    Navigator,
    Settings,

    /// Advance `DfuState`, `BondState` and `StorageInfo`.
    Platform,
    /// Admit heavy work through `CoreMode` and recalculate [`Capabilities`].
    Admission,
    Faults,
    /// Calculate render work, needs, effects and the next wake.
    Plan,
}

impl PassStage {
    pub const ORDER: [PassStage; 12] = [
        PassStage::Outcomes,
        PassStage::Facts,
        PassStage::Input,
        PassStage::Ui,
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

/// The pass's two clocks. They are the same value on the board. In the simulator
/// [`ride`](Self::ride) is GPX-playback time and [`ui`](Self::ui) is wall time, so a replayed ride's
/// moving time is not scaled by the replay speed while a hold still charges in real seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PassClock {
    pub ride: RideClock,
    pub ui: InputClock,
}

/// Everything one pass reads. Values and pull-only ports: nothing here can reach back into
/// DeviceCore while the pass runs.
///
/// [`outcomes`](Self::outcomes) and [`facts`](Self::facts) are borrowed rather than owned, so the
/// pass consumes only what it has an owner for and leaves the rest where the executor put it.
pub struct PassInputs<'a> {
    pub now: PassClock,
    /// The gestures recognised since the last pass, in the order they happened.
    pub gestures: &'a [Gesture],
    pub sensors: Sensors<'a>,
    pub route: Option<&'a RouteReader<'a>>,

    /// What this firmware image and its hardware implement at all. Constant for a boot.
    pub support: PlatformSupport,
    /// What the platform finished since the last pass.
    pub outcomes: &'a mut OutcomeSlots,
    /// What changed underneath DeviceCore that nobody asked for.
    pub facts: &'a mut ExternalFacts,
    /// Keyed answers to the derived needs of the previous plan.
    pub derived: DerivedInputs,
    pub targets: DerivedTargets<'a>,
}

/// What the platform must read for the frame the pass just planned. A level, recalculated every
/// pass: a host that cannot open a source sees the need again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SourceNeeds {
    pub map: bool,
    pub route: bool,
}

/// What one pass decided: the render work, when to come back, what to read, and the bounded
/// physical work per domain.
#[derive(Debug, PartialEq, Eq)]
pub struct PassPlan {
    pub render: Dirty,
    /// Millis until the pass must run again, or `None` to sleep until an event. Planned before the
    /// frame is drawn: a host that renders in the same pass reads
    /// [`App::ms_until_next_wake`](crate::App::ms_until_next_wake) after the draw, since a draw can
    /// arm a wake of its own.
    pub next_wake_ms: Option<u32>,
    pub derived_needs: DerivedNeeds,
    pub sources: SourceNeeds,
    /// One bounded operation per domain.
    pub effects: EffectSlots,
    /// A later-to-earlier value is waiting: run another pass before sleeping.
    pub immediate: bool,
}

/// The overlay plane's live content and animation phase: the hold bulge and the planning banner.
/// The bulge repaints for one more frame after it goes quiet, so the last bulge is cleared off the
/// layer. The banner repaints when it appears, disappears, or advances its activity phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OverlayKey {
    pub(crate) hold: bool,
    /// Zero when absent, otherwise the banner's current activity phase.
    pub(crate) banner: u8,
}

impl OverlayKey {
    const QUIET: OverlayKey = OverlayKey { hold: false, banner: 0 };

    fn dirty_against(self, previous: OverlayKey) -> bool {
        (self.hold || previous.hold) || (self.banner != previous.banner)
    }
}

/// The coordinator's own resident state: the connections, the levels a stage compares against to
/// find an edge, the current capabilities, and the re-entrancy guard. No domain state lives here.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct PassState {
    pub(crate) connections: Connections,
    /// The newest store revision seen: the level the facts stage detects a commit against.
    store: Option<StoreRevision>,
    link: Option<crate::ble::BleStatus>,
    /// The active route's durable identity as of the last pass: Navigator's activation edge.
    active_route: Option<crate::CatalogObjectId>,
    /// What the device can currently do, recalculated every pass.
    pub(crate) capabilities: Capabilities,
    /// The overlay plane's levels at the last drain. See [`OverlayKey`].
    overlay: OverlayKey,
    /// Whether a pass is running. The push doors refuse while it is set.
    in_pass: bool,
    #[cfg(test)]
    trace: heapless::Vec<PassStage, 16>,
}

impl PassState {
    pub(crate) const fn new() -> Self {
        PassState {
            connections: Connections::new(),
            store: None,
            link: None,
            active_route: None,
            capabilities: Capabilities::NONE,
            overlay: OverlayKey::QUIET,
            in_pass: false,
            #[cfg(test)]
            trace: heapless::Vec::new(),
        }
    }

    pub(crate) fn in_pass(&self) -> bool {
        self.in_pass
    }

    /// Open a pass. Re-entry is a caller bug: a platform callback that reached here would be
    /// mutating DeviceCore in the middle of a pass.
    fn enter(&mut self) {
        debug_assert!(!self.in_pass, "a pass cannot run inside a pass");
        self.in_pass = true;
        #[cfg(test)]
        self.trace.clear();
    }

    fn leave(&mut self) {
        self.in_pass = false;
    }

    /// Fold this frame's overlay levels against the last drain's, then remember them. Call exactly
    /// once per frame: the bulge's trailing clear is tracked across calls, so a second drain in one
    /// frame swallows it.
    pub(crate) fn overlay_repaint(&mut self, now: OverlayKey) -> bool {
        now.dirty_against(core::mem::replace(&mut self.overlay, now))
    }

    /// Note that a stage ran. Free outside tests.
    #[inline]
    fn record(&mut self, stage: PassStage) {
        #[cfg(test)]
        self.trace.push(stage).expect("the trace holds every stage of one pass");
        #[cfg(not(test))]
        let _ = stage;
    }
}

// Tripwire: a growth here means domain state drifted into the sequencer.
const _: () = assert!(core::mem::size_of::<PassState>() <= 344, "connections, a few levels, the test recorder");

impl App {
    /// Run one DeviceCore pass: every stage once, in [`PassStage::ORDER`]. The whole product frame
    /// in one call, returning the bounded work the platform must perform.
    pub fn run_pass(&mut self, inputs: PassInputs<'_>) -> PassPlan {
        let PassInputs { now, gestures, sensors, route, support, outcomes, facts, derived, targets } = inputs;
        self.pass.enter();
        // The visible screens' exact facts, before any stage runs. The twin after the last stage
        // closes the comparison.
        let key_before = self.render_key();

        self.stage_outcomes(outcomes, now.ui.0);
        self.stage_facts(facts, derived, targets);
        self.stage_input(now, gestures, sensors, route);
        self.stage_ui(now);

        let mut effects = EffectSlots::new();
        self.stage_catalog(&mut effects);
        self.stage_metadata(&mut effects);
        self.stage_recorder(&mut effects);
        self.stage_navigator(&mut effects);
        self.stage_settings(&mut effects);

        self.stage_platform(&mut effects, support);
        self.stage_admission(support);
        self.stage_faults();
        // Every stage has run, so what the visible screens draw is final for this frame. A moved
        // key is a repaint, folded in before the plan stage drains the demand.
        match self.render_key().repaint_since(&key_before) {
            Repaint::Nothing => {}
            Repaint::GaugeBand => {
                let (w, h) = (self.ui.frame_size.0 as i32, self.ui.frame_size.1 as i32);
                self.ui.request_region(crate::screen::gauge_region(w, h), true);
            }
            Repaint::Full => self.ui.map_dirty = true,
        }
        let plan = self.stage_plan(now, effects);

        self.pass.leave();
        plan
    }

    fn stage_outcomes(&mut self, outcomes: &mut OutcomeSlots, now_ms: u32) {
        self.pass.record(PassStage::Outcomes);
        if let Some(outcome) = outcomes.catalog.take() {
            if self.catalogs.accepts(outcome) {
                if self.catalogs.cleanup_running() {
                    for screen in self.ui.stack.iter_mut() {
                        if let crate::screen::Screen::RouteCleanup(screen) = screen {
                            screen.progress(outcome);
                        }
                    }
                    self.ui.map_dirty = true;
                }
                match outcome {
                    crate::catalog_state::CatalogOutcome::Failed {
                        error: crate::catalog_state::CatalogError::Unreadable,
                        ..
                    } => self.catalogs.defer_read(now_ms),
                    crate::catalog_state::CatalogOutcome::Failed {
                        error: crate::catalog_state::CatalogError::Unsupported,
                        ..
                    } => {}
                    _ => {}
                }
            }
            self.catalogs.apply_outcome(outcome);
        }
        if let Some(outcome) = outcomes.metadata.take() {
            if self.metadata.apply_outcome(outcome) {
                self.assistant_checkpoint_answer(outcome);
                use crate::metadata::{MetadataError, MetadataOutcome};
                match outcome {
                    MetadataOutcome::Failed { error: MetadataError::RemountRequired, .. } => {
                        self.catalogs.loaded_scope = None;
                        self.catalogs.remount_required = true;
                    }
                    // An unsubmitted edit waits for a fresh scope before it is offered again.
                    MetadataOutcome::Cancelled { .. } if self.navigator.checkpoint_change().is_some() => {
                        self.catalogs.loaded_scope = None;
                        self.catalogs.note_store_moved();
                    }
                    MetadataOutcome::CheckpointWritten { .. } | MetadataOutcome::ProgressWritten { .. } => {
                        self.catalogs.loaded_scope = None;
                        self.catalogs.note_store_moved();
                    }
                    _ => {}
                }
            }
        }
        if let Some(outcome) = outcomes.recorder.take() {
            // A committed ride tells the catalog later in this pass. A failure raises the recording
            // warning and changes nothing else: the ride is still on the store, so the close stays
            // pending and re-offers.
            match self.recorder.apply_outcome_at(outcome, now_ms) {
                crate::recorder::RecorderVerdict::Saved(ride) => {
                    let _ = self.pass.connections.ride_finalized.try_put(RideFinalized { ride });
                    self.note_trip_finish();
                    let finished = self.recorder.ride_stats();
                    self.end_ride_session();
                    self.land_day_done(ride, &finished);
                }
                crate::recorder::RecorderVerdict::Dropped => self.end_ride_session(),
                crate::recorder::RecorderVerdict::Failed => {
                    self.pass.connections.faults.raise(crate::screen::WarningFlags::REC_ERROR);
                }
                // The typed card is the explanation, so no `REC_ERROR` is raised beside it: that
                // warning means a ride log is now incomplete, and no ride is being logged.
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
                // pass reaches the rider together, on one card.
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
                    if self.pass.store.is_some() {
                        self.catalogs.change_store();
                        for screen in self.ui.stack.iter_mut() {
                            if let crate::screen::Screen::RouteCleanup(screen) = screen {
                                screen.cancel();
                            }
                        }
                    }
                    self.metadata.reset_store();
                    self.navigator.review_store_changed(store.store);
                    self.catalogs.remount_required = false;
                    self.catalogs.loaded_scope = self.catalogs.loaded_scope.filter(|scope| scope.store == store.store);
                }
                self.pass.store = Some(store);
                // The fact says the store moved; it does not order a re-read. The catalog domain
                // owes the read from here on, so a commit that arrives while the catalog is busy
                // costs a delay rather than a missed rescan.
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
    /// A `Hold` or `BackHold` already in the batch behind a stack-changing gesture is dropped rather
    /// than delivered to the screen that replaced its target. The hold-cancel latch is deliberately
    /// not drained here: it belongs to the board's input plane, which drains it between passes.
    fn stage_input(
        &mut self,
        now: PassClock,
        gestures: &[Gesture],
        sensors: Sensors<'_>,
        route: Option<&RouteReader<'_>>,
    ) {
        self.pass.record(PassStage::Input);
        self.ui.now_ms = now.ui.0;
        // Recorder's session edge, before the world is applied: a ride asked for between two passes
        // must be open before this pass integrates a fix into it.
        self.advance_recorder_session();
        self.apply_gesture_batch(gestures);
        self.advance_inputs(now.ride, sensors, route);
    }

    /// Stage 4 — advance `UiRuntime`, then collect the typed intents the rider produced.
    ///
    /// The UI reaches a domain by naming what it wants, never by performing the work: a delete is a
    /// [`CatalogIntent`], not a store operation. Every intent is offered into a slot that is checked
    /// first, so an intent that cannot be delivered leaves the rider's one-shot where it was.
    fn stage_ui(&mut self, now: PassClock) {
        self.pass.record(PassStage::Ui);
        self.advance_animations(now.ui);

        if self.pass.connections.ui_catalog.is_empty() {
            // A vanished subject consumes the request and yields nothing, which is why the index is
            // resolved to a durable id here.
            let intent = if let Some(idx) = self.activity.take_route_delete() {
                self.catalogs.route_id_at(idx).map(|id| CatalogIntent::DeleteRoute { id })
            } else if let Some(idx) = self.activity.take_ride_delete() {
                self.catalogs.ride_entry(idx).map(|entry| CatalogIntent::DeleteRide { id: entry.id })
            } else {
                // The trip delete already is a durable id, so there is nothing to resolve here.
                self.activity
                    .take_trip_delete()
                    .map(|id| CatalogIntent::DeleteTrip { id })
                    .or_else(|| self.activity.cleanup_routes.take())
            };
            if let Some(intent) = intent {
                let _ = self.pass.connections.ui_catalog.try_put(intent);
            }
        }
    }
    /// Invalidates admission before any catalog feeder mutates the resident projection.
    pub fn begin_catalog_refresh(&mut self) {
        self.catalogs.loaded_scope = None;
    }
    fn stage_catalog(&mut self, effects: &mut EffectSlots) {
        self.pass.record(PassStage::Catalog);
        if self.pass.connections.ride_finalized.take().is_some() {
            self.catalogs.note_ride_finalized();
        }
        // A refused intent goes back into the slot it came from: that slot is its producer's
        // pending state until the catalog has room, so a busy pass costs a delay, not a delete.
        if let Some(intent) = self.pass.connections.ui_catalog.take() {
            if let Err(full) = self.admit_catalog_intent(intent) {
                let _ = self.pass.connections.ui_catalog.try_put(full.rejected);
            }
        }
        // The store refuses a removal for an object the checkpoint names, and a refused removal is
        // not retried, so the rider's confirmed delete would vanish. Give up the checkpoint first
        // and leave the intent admitted: the admitted intent is what the domain would pull next, so
        // holding the pull holds only this removal.
        if self.checkpoint_blocks_removal() {
            self.navigator.clear_checkpoint_for_removal();
            return;
        }
        if let Some(effect) = self.catalogs.next_effect_at(self.ui.now_ms) {
            let _ = effects.catalog.try_put(effect);
        }
    }

    /// Hand one intent to the catalog domain, and — when what leaves is the route being followed —
    /// tell Navigator in this same pass.
    fn admit_catalog_intent(&mut self, intent: CatalogIntent) -> Result<(), SlotFull<CatalogIntent>> {
        self.catalogs.admit_intent(intent)?;
        if let CatalogIntent::DeleteRoute { id } = intent {
            if self.navigator.route_state().active_route.and_then(|idx| self.catalogs.route_id_at(idx)) == Some(id) {
                let _ = self.pass.connections.active_route_removed.try_put(ActiveRouteRemoved { route: id });
            }
        }
        Ok(())
    }

    fn stage_metadata(&mut self, effects: &mut EffectSlots) {
        let Some(scope) = self.catalogs.loaded_scope.filter(|_| effects.metadata.is_empty()) else {
            return;
        };
        if self.navigator.checkpoint_change().is_some() {
            if let Some(mut effect) = self.metadata.next_checkpoint_effect() {
                effect.bind(Some(scope));
                self.navigator.checkpoint_issued(effect.token());
                let _ = effects.metadata.try_put(effect);
            }
        } else if let Some(mut effect) = self.metadata.next_progress_effect() {
            effect.bind(Some(scope));
            let _ = effects.metadata.try_put(effect);
        }
    }

    /// Stage 7 — `RecorderMachine`'s one bounded operation: a journal checkpoint the cadence owes,
    /// or the close the rider named.
    ///
    /// Gated on [`Capabilities::recorder`](super::Capabilities), so a device with nowhere to put a
    /// ride does no recording work at all. [`Start`](crate::RecorderIntent::Start) is a state
    /// change, not an effect, so it lands in
    /// [`advance_recorder_session`](App::advance_recorder_session) instead.
    fn stage_recorder(&mut self, effects: &mut EffectSlots) {
        self.pass.record(PassStage::Recorder);
        let caps = self.pass.capabilities.recorder;
        // The wall-clock anchor goes with the offer, so the figures the executor writes belong to
        // the operation and not to whenever it gets to it.
        let clock = self.footer_clock();
        if effects.recorder.is_empty() {
            if let Some(effect) = self.recorder.next_effect(caps, clock) {
                let _ = effects.recorder.try_put(effect);
            }
        }
    }

    pub(crate) fn advance_recorder_session(&mut self) {
        match self.recorder.advance(self.pass.capabilities.recorder) {
            crate::recorder::RecorderAdvance::Opened(start) => self.begin_ride_session(start),
            // The rider asked to record and this device cannot. The request is kept, so a card that
            // mounts later still opens the ride — but they are told now, through the recording
            // warning. A riding view that quietly records nothing is what this raise prevents.
            crate::recorder::RecorderAdvance::Refused => {
                self.pass.connections.faults.raise(crate::screen::WarningFlags::REC_ERROR);
            }
            // A damaged recovered object is still standing, so no session opened. Put the decision
            // back rather than a warning: it is the one thing the rider can act on.
            crate::recorder::RecorderAdvance::RecoveryOwed => {
                self.activity.mode = crate::activity::Mode::Idle;
                self.raise_ride_recovery();
            }
            crate::recorder::RecorderAdvance::Nothing => {}
        }
    }

    /// A ride session opened: re-lock the matcher, restart the trail and the pace window, and — for
    /// a fresh ride, never a recovered continuation — zero the accumulators, record the ride's
    /// origin and drop any detour in flight.
    fn begin_ride_session(&mut self, start: crate::recorder::SessionStart) {
        self.navigator.relock_matcher();
        if start == crate::recorder::SessionStart::Fresh {
            self.navigator.reset_ride();
            self.recorder.reset_totals();
            self.recorder.set_origin(self.ride_origin());
            self.metadata.begin_ride();
            self.navigator.reset_detour();
            // Only a measured anchor re-joins the route. A plain route selection records no
            // progress, so re-anchoring a fresh ride to it would drag the matcher back to the route
            // start from wherever the rider actually is.
            if let Some(checkpoint) = self
                .assistant_checkpoint()
                .filter(|checkpoint| !checkpoint.selection)
                .filter(|_| self.assistant_review_status() == crate::navigator::ReviewStatus::Accepted)
            {
                if let Some(index) = self
                    .active_route_index()
                    .filter(|&index| self.route_ids().get(index) == Some(&checkpoint.route.object))
                {
                    self.navigator.request_seam(index, checkpoint.progress_m);
                }
            }
        }
        self.recorder.restart_buffers();
        self.ui.map_dirty = true;
    }

    /// The ride session closed, saved or discarded: the matcher, the totals and the trail all go
    /// back to their between-rides state, so nothing is left showing the ride that just ended.
    fn end_ride_session(&mut self) {
        self.navigator.relock_matcher();
        self.navigator.reset_ride();
        self.recorder.reset_totals();
        self.navigator.reset_detour();
        self.recorder.restart_buffers();
        self.ui.map_dirty = true;
    }

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
    /// A capability is a level, never latched: it is recomputed from what the image implements, what
    /// is currently true, and [`CoreMode`](crate::device_core::core_mode::CoreMode)'s heavy-work
    /// verdict. `store_writable` cannot come back down, because [`ExternalFacts`] has no unmount
    /// fact to retract it with.
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

    /// Stage 13 — deliver every notice raised this pass, together. Last, because every producer
    /// runs before it, so one card carries what several domains found.
    fn stage_faults(&mut self) {
        self.pass.record(PassStage::Faults);
        let flags = self.pass.connections.faults.take();
        if !flags.is_empty() {
            self.on_warning(flags);
        }
    }

    fn stage_plan(&mut self, now: PassClock, effects: EffectSlots) -> PassPlan {
        self.pass.record(PassStage::Plan);
        let render = self.take_dirty();
        let immediate = false;
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

    /// Stage a mounted card: the level that admits catalog mutation and ride recording, without
    /// driving a whole frame for it.
    #[cfg(test)]
    pub(crate) fn test_mount_store(&mut self) {
        self.pass.store =
            Some(StoreRevision { store: super::StoreIdentity::new(1), revision: crate::device_core::Revision::new(1) });
        self.catalogs.loaded_scope = self.pass.store;
        self.pass.capabilities.recorder = crate::device_core::RecorderCapabilities { record: true };
    }

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
    use crate::device_core::Revision;
    use crate::device_core::{StoreIdentity, TokenSource};
    use crate::route::RouteSummary;
    use crate::screen::WarningFlags;

    use crate::Screen;
    use obc_ports::{Fix, LocationSource};

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

    /// Every input the pass takes, for a test that must drive what the shorthands leave at `NONE`.
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

    fn quiet_without_metadata(app: &mut App, ms: u32) -> PassPlan {
        let mut facts = ExternalFacts::NONE;
        pass_supported(
            app,
            PlatformSupport { ..EVERYTHING },
            PassClock { ride: RideClock(ms), ui: InputClock(ms) },
            &[],
            &mut OutcomeSlots::new(),
            &mut facts,
            DerivedInputs::NONE,
            DerivedTargets::NONE,
        )
    }

    /// A terminal card pops itself on a press, but the scheduler sweep that re-delivers it is a
    /// later stage of the same pass. The card must stay popped for the rest of that pass, or the
    /// board's dismissal latch never fires and the rider cannot leave the screen.
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
            PlatformSupport { ..EVERYTHING },
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
            ..Default::default()
        }
    }
    fn committed(revision: u64) -> ExternalFacts {
        let mut facts = ExternalFacts::NONE;
        facts.note_store_revision(StoreRevision { store: StoreIdentity::new(1), revision: Revision::new(revision) });
        facts
    }

    /// A stage is a position, not a reaction: a quiet pass runs the same stages as a busy one.
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
    /// The rider is not left being guided along a route the device has decided to remove.
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

    /// Several producers coalesce onto one card rather than displacing each other.
    #[test]
    fn a_fault_raised_earlier_in_the_pass_reaches_the_rider_in_it() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        let mut facts = ExternalFacts::NONE;
        facts.raise_warnings(WarningFlags::NO_GPS);
        facts.raise_warnings(WarningFlags::STORAGE_ERROR);

        pass_with(&mut app, 10, &[], &mut OutcomeSlots::new(), &mut facts);
        assert!(app.pass.connections.faults.take().is_empty(), "delivered, not left pending");
        assert!(
            matches!(app.top_screen(), crate::Screen::Warning(w) if w.flags().contains(WarningFlags::NO_GPS)
                && w.flags().contains(WarningFlags::STORAGE_ERROR)),
            "both notices reached one card"
        );
    }
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

    /// A saved ride without a trip leaves the rider on Home. A saved trip day lands its card: DAY 1
    /// DONE, which reads Day 2's profile, and TRIP DONE after the last day.
    #[test]
    fn a_saved_trip_day_lands_its_card_and_other_rides_go_home() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.set_routes_with_ids(&[summary("Day 1"), summary("Day 2")], &[11, 22]);
        app.set_trips(&[crate::trip::TripInput { id: 1, key: 42, name: "Alps", start_date: 0, stage_ids: &[11, 22] }]);
        let ride = |app: &mut App, day: Option<u8>, ms: u32| {
            app.activity.mode = Mode::Riding;
            app.test_start_ride();
            let trip = day.and_then(|day| obc_formats::ride::TripRef::new(42, day, 2));
            app.recorder.set_origin(crate::RideOrigin { bike: obc_formats::bike::BikeType::Road, trip });
            // Finish on the Paused page.
            let paused = crate::screen::Transition::Push(Screen::RideControl(crate::screen::RideControl::new()));
            crate::screen::apply(&mut app.ui.stack, paused);
            app.apply_gesture(Gesture::Step(1));
            app.apply_gesture(Gesture::Hold);
            let token = quiet(app, ms).effects.recorder.take().expect("the close").token();
            let mut outcomes = OutcomeSlots::new();
            outcomes
                .recorder
                .try_put(crate::recorder::RecorderOutcome::Finalized { token, ride: u64::from(ms) })
                .unwrap();
            let mut facts = ExternalFacts::NONE;
            pass_with(app, ms + 10, &[], &mut outcomes, &mut facts);
        };

        ride(&mut app, None, 100);
        assert!(matches!(app.top_screen(), Screen::Home(_)), "no trip, no card");

        ride(&mut app, Some(0), 200);
        assert!(matches!(app.top_screen(), Screen::DayDone(card) if !card.trip_done()));
        assert_eq!(app.derived_needs().day_profile.map(|key| key.day), Some(22), "Day 2's profile is read");
        app.apply_gesture(Gesture::Press);
        assert!(matches!(app.top_screen(), Screen::Home(_)));
        assert!(app.derived_needs().day_profile.is_none(), "OK drops the need");

        ride(&mut app, Some(1), 300);
        assert!(matches!(app.top_screen(), Screen::DayDone(card) if card.trip_done()));
        assert!(app.derived_needs().day_profile.is_none(), "no day is left to read");
    }

    /// Both halves are the property. Dropping the gate starts a ride with nowhere to put it;
    /// keeping the gate but swallowing the refusal gives the rider a riding view that records
    /// nothing and never says so, which is the worse of the two.
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

        // The card mounts, and the request the rider already made still opens the ride.
        let mut facts = ExternalFacts::NONE;
        facts.note_store_revision(StoreRevision {
            store: StoreIdentity::new(1),
            revision: crate::device_core::Revision::new(1),
        });
        pass_with(&mut app, 40, &[], &mut OutcomeSlots::new(), &mut facts);
        quiet(&mut app, 50);
        assert!(app.recording(), "the kept request opened the ride the rider asked for");
    }

    /// A new session clears the trail and the pace window, and a fresh one zeroes the totals.
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

        app.recorder.assert_totals_are_zero();
        assert!(app.recorder.staged().is_empty(), "and owes no sample the previous ride never wrote");
    }

    /// The integration anchors are part of the reset: a new ride that kept them would credit itself
    /// with the step from where the last one ended, and its first sample would continue that ride's
    /// segment instead of opening its own.
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

    #[test]
    fn a_recovered_ride_continues_without_resetting_its_totals() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0));
        let restored = crate::RideContinuation { ridden_m: 12_345.0, moving_s: 2_700.0, ..Default::default() };
        assert!(app.offer_recovered_ride(restored));
        // The card's Continue: a session that keeps what the journal restored.
        app.pass.store =
            Some(StoreRevision { store: StoreIdentity::new(1), revision: crate::device_core::Revision::new(1) });
        app.pass.capabilities.recorder = crate::device_core::RecorderCapabilities { record: true };
        app.recorder.continue_recovered();
        app.advance_recorder_session();
        assert!(app.recording());
        assert_eq!(app.recorder.continuation(), restored, "recovery must not zero the ride it just restored");
    }
    /// The fact reports that the store moved and `CatalogState::note_store_moved` arms the one bit
    /// that orders the re-read, so one commit orders exactly one.
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

        // A busy catalog delays the refresh rather than losing it: the rider's delete goes first.
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

    /// The owed re-read is a bit and not a counter, so the board — which raises a revision for its
    /// own removals — gains no second rescan from this slice.
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

    /// Only a domain that owns a token source may consume at all: an outcome nobody can validate
    /// stays in its slot rather than being dropped or guessed at.
    #[test]
    fn an_outcome_is_consumed_by_its_owner_and_left_alone_without_one() {
        let mut app = navigating();
        quiet(&mut app, 10);

        let mut outcomes = OutcomeSlots::new();

        // Recorder owns a token source, so its answer is consumed — and refused, because the token
        // is one it never issued.
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

    /// One effect slot is enough for a queue of deletes, because the outcome frees the operation.
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

    /// A capability is a level, never latched.
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

    /// A live search is heavy too. The nav arm is one block, so a second plan started mid-search
    /// could only fail; withdrawing the capability stops it being offered at all.
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

    /// `run_pass` holds `&mut self` for its whole length, so no safe caller can reach a push door
    /// mid-pass and the flag has to be set by hand here. Reaching one anyway is a caller bug, so it
    /// is loud in debug and refused in release rather than quietly losing the answer.
    #[test]
    #[should_panic(expected = "cannot change DeviceCore during a pass")]
    fn a_callback_cannot_mutate_core_state_during_a_pass() {
        let mut app = navigating();
        app.set_rides(&[crate::RideEntry { id: 7, summary: ride_summary() }], &[]);
        app.activity.viewed_ride = Some(0);
        let plan = quiet(&mut app, 10);
        let key = plan.derived_needs.ride_track.expect("the open ride detail needs its track");
        app.pass.in_pass = true;
        app.apply_derived(
            DerivedInputs::ride_track(crate::device_core::DerivedInput::filled(key)),
            DerivedTargets::NONE,
        );
    }

    #[test]
    fn a_push_outside_a_pass_is_applied_normally() {
        let mut app = navigating();
        app.set_rides(&[crate::RideEntry { id: 7, summary: ride_summary() }], &[]);
        app.activity.viewed_ride = Some(0);
        let plan = quiet(&mut app, 10);
        let key = plan.derived_needs.ride_track.expect("the open ride detail needs its track");
        app.apply_derived(
            DerivedInputs::ride_track(crate::device_core::DerivedInput::filled(key)),
            DerivedTargets::NONE,
        );
        assert!(quiet(&mut app, 20).derived_needs.ride_track.is_none(), "the answer landed");
    }

    /// A pass neither reports hold cancellation nor drains it, so the board's input plane still
    /// finds the latch it must act on between passes.
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

    /// The pass's routing: every level lands with its owner, every one-shot is taken from the batch,
    /// and a keyed derived answer clears the need it answers. The two clocks differ here on purpose,
    /// so a stage that reads the ride clock where it owes the UI one trips
    /// [`App::ms_until_next_wake`]'s same-frame assertion.
    #[test]
    fn one_pass_routes_a_full_fact_batch_and_a_derived_answer() {
        let mut app = navigating();
        app.set_rides(&[crate::RideEntry { id: 7, summary: ride_summary() }], &[]);
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

    /// The banner repaints on the engaged level, not on the search's start edge: that edge fires
    /// under the opaque planning spinner, where nothing freezes.
    #[test]
    fn the_overlay_key_repaints_on_the_engaged_level_and_the_bulge_trailing_edge() {
        let mut pass = PassState::new();
        let quiet = OverlayKey { hold: false, banner: 0 };
        assert!(!pass.overlay_repaint(quiet), "at rest there is nothing to repaint");

        // A search under the opaque spinner: a chrome base, so nothing is engaged.
        assert!(!pass.overlay_repaint(OverlayKey { hold: false, banner: 0 }));
        // The spinner goes and a map base is back — no search edge, but *this* is the freeze.
        assert!(pass.overlay_repaint(OverlayKey { hold: false, banner: 1 }), "the banner appears");
        assert!(
            !pass.overlay_repaint(OverlayKey { hold: false, banner: 1 }),
            "a level, so one repaint — not one per ride-loop pass"
        );
        assert!(pass.overlay_repaint(OverlayKey { hold: false, banner: 2 }), "activity advances");
        assert!(!pass.overlay_repaint(OverlayKey { hold: false, banner: 2 }), "activity is throttled");
        assert!(pass.overlay_repaint(quiet), "and one more to take the banner off");
        assert!(!pass.overlay_repaint(quiet));

        // The bulge: live while it animates, plus exactly one trailing frame to clear it.
        let bulge = OverlayKey { hold: true, banner: 0 };
        assert!(pass.overlay_repaint(bulge), "a charging bulge paints");
        assert!(pass.overlay_repaint(bulge), "…every frame it is live");
        assert!(pass.overlay_repaint(quiet), "the trailing clear frame");
        assert!(!pass.overlay_repaint(quiet), "and then quiet");
    }
}
