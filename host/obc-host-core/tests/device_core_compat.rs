//! The compatibility executor, driven from outside `obc-app` (DC6, #1439).
//!
//! `App::run_pass` produces bounded effects and keyed derived needs; every runtime host still
//! speaks `HostCommand` / `HostEvent`. This file is the host- and board-side proof that
//! [`LegacyAdapter`] is the only thing needed between them, and that it is a *translator*: the same
//! mapping table serves both shapes, a late answer is rejected by the domain rather than by the
//! adapter, and a level keeps asking until a matching key comes back.
//!
//! The production `HostLoop` and the board's `HostPass` are deliberately untouched — #1439 adds the
//! adapter, #1397 S6 moves the hosts onto it. The DC1 legacy behaviour traces therefore still
//! describe the shipping paths exactly as before.

use obc_app::device_core::compat::{
    event_reply, navigator_row, recorder_row, AnsweredRow, LegacyError, LegacyOwned, LegacyReport, UnansweredRow,
};
use obc_app::device_core::derived::{DerivedInputs, DerivedResult, DerivedTargets, RideTrackKey};
use obc_app::device_core::{
    EffectSlots, LegacyAdapter, LegacyInputs, LegacyReply, NavigatorTag, OperationToken, PassClock, PassInputs,
    PassPlan, PlatformSupport, SettingsTag, TokenSource,
};
use obc_app::dfu::DfuEffect;
use obc_app::navigator::{NavigatorEffect, NavigatorOutcome, PlannerWork};
use obc_app::recorder::RecorderEffect;
use obc_app::retention::{Retention, RouteRetentionMeta};
use obc_app::ride::RideSummary;
use obc_app::route::RouteSummary;
use obc_app::screen::Screen;
use obc_app::settings::{SettingsEffect, SettingsOutcome};
use obc_app::{App, AppState, HostCommand, HostEvent, HostMailbox, NavRequest, WarningFlags};
use obc_map_scene::BBox;
use obc_ports::{Fix, InputClock, LocationSource, RideClock, Sensors};

/// A platform that implements everything, so no capability hides a path this file means to exercise.
const EVERYTHING: PlatformSupport = PlatformSupport {
    detour: true,
    settings_persistence: true,
    dfu: true,
    weather: true,
    bonding: true,
    storage_space_report: true,
};

struct NoFix;
impl LocationSource for NoFix {
    fn poll(&mut self) -> Option<Fix> {
        None
    }
}

/// One host built out of the pieces DC6 provides: the real `App`, one [`LegacyAdapter`], and the
/// legacy mailbox the existing executors already pop from. There is nothing else between them —
/// which is the claim this file exists to make.
struct CompatHost {
    app: App,
    adapter: LegacyAdapter,
    mail: HostMailbox,
    /// What the adapter has been handed since the last pass.
    inbox: LegacyInputs,
    /// Every legacy command the adapter has emitted, in order.
    sent: Vec<HostCommand>,
}

impl CompatHost {
    fn new(app: App) -> Self {
        CompatHost {
            app,
            adapter: LegacyAdapter::new(),
            mail: HostMailbox::new(),
            inbox: LegacyInputs::new(),
            sent: Vec::new(),
        }
    }

    /// One frame: run the pass on what arrived, then translate what it decided.
    fn pass(&mut self, ms: u32) -> (PassPlan, LegacyReport) {
        let mut inputs = std::mem::take(&mut self.inbox);
        let mut location = NoFix;
        let mut plan = self.app.run_pass(PassInputs {
            now: PassClock { ride: RideClock(ms), ui: InputClock(ms) },
            gestures: &[],
            sensors: Sensors::new(&mut location),
            route: None,
            support: EVERYTHING,
            outcomes: &mut inputs.outcomes,
            facts: &mut inputs.facts,
            derived: inputs.derived,
            targets: DerivedTargets::NONE,
        });
        // Outcomes and facts the pass had no owner for stay where the executor put them; a derived
        // answer was either accepted or was about something else, and either way it is spent.
        inputs.derived = DerivedInputs::NONE;
        self.inbox = inputs;

        let report = self.adapter.effects_to_commands(&mut plan.effects, &mut self.mail);
        self.adapter.needs_to_commands(&plan.derived_needs, &mut self.mail);
        while let Some(command) = self.mail.pop() {
            self.sent.push(command);
        }
        (plan, report)
    }

    fn deliver(&mut self, event: HostEvent) -> Result<(), LegacyError> {
        let mut inbox = std::mem::take(&mut self.inbox);
        let result = self.adapter.event_to_inputs(event, &mut inbox);
        self.inbox = inbox;
        result
    }

    fn count(&self, want: &HostCommand) -> usize {
        self.sent.iter().filter(|command| *command == want).count()
    }

    /// The rider-visible state two runners have to agree on.
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            screen: screen_name(self.app.top_screen()),
            stack_depth: self.app.debug_stack_len(),
            route_ids: self.app.route_ids().to_vec(),
            ride_ids: self.app.ride_ids().to_vec(),
            wants_ride_track: self.app.ride_track_request(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Snapshot {
    screen: &'static str,
    stack_depth: usize,
    route_ids: Vec<u64>,
    ride_ids: Vec<u64>,
    wants_ride_track: Option<u64>,
}

fn screen_name(screen: &Screen) -> &'static str {
    match screen {
        Screen::Home(_) => "home",
        Screen::Menu(_) => "menu",
        Screen::Rides(_) => "rides",
        Screen::RideDetail(_) => "ride-detail",
        Screen::Warning(_) => "warning",
        _ => "other",
    }
}

fn route(name: &str) -> RouteSummary {
    let mut summary = RouteSummary {
        name: Default::default(),
        distance_km: 10,
        climb_m: 100,
        bbox: BBox { min_lon: 0, min_lat: 0, max_lon: 1_000, max_lat: 1_000 },
        start_lon: 100,
        start_lat: 100,
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

/// Two routes and two rides, with the second route long expired under a trusted clock — the one
/// scenario the current pass wiring drives all the way from a domain decision to a legacy command.
fn expiring_host() -> CompatHost {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    app.set_routes_with_ids(&[route("alpha"), route("beta")], &[11, 22]);
    app.set_rides(&[ride("morning"), ride("evening")], &[71, 72]);
    app.activate_route(0);
    app.stamp_clock_ble(1_700_000_000, 0);
    let now = app.wall_unix_now();
    // The active route carries a live expiry clock, so its activation is stamped; the other is a
    // month past its deadline, so the sweep decides to expire it. Both land in the same pass.
    app.set_route_meta(&[
        RouteRetentionMeta::new(Retention::Week1, now),
        RouteRetentionMeta::new(Retention::Week1, now.saturating_sub(30 * 24 * 3600)),
    ]);
    app.force_retention_sweep();
    CompatHost::new(app)
}

// ==================== the pass, end to end, over the legacy protocol ====================

/// The real `App` over the legacy protocol, and the exact price of the half-done migration.
///
/// Retention's use stamp for the active route has a legacy command; the expiry it decides on becomes
/// a namespace-free `RemoveObject`, which the old namespaced deletes cannot carry, so it is left
/// against [`LegacyOwned::ObjectNamespace`] rather than dropped.
///
/// Then the tail: **neither domain speaks again**. Both latch in-flight when they emit and clear
/// only on their own outcome, and this path can build neither a catalog nor a retention outcome —
/// so the successfully translated stamp is stuck exactly as hard as the untranslatable removal.
/// That is the cost of running the pass before the executors migrate, and it is a fact here rather
/// than a sentence in a doc comment.
#[test]
fn the_adapter_path_completes_one_operation_per_unanswerable_domain() {
    let mut host = expiring_host();

    let (_, report) = host.pass(10);
    // Two domains speak in the boot pass: retention's use stamp for the active route, and the
    // settings write the trusted-clock stamp made owed (#1397 S2 moved that handshake into
    // `SettingsMachine`, so the pass — not the drain — is what offers it).
    assert_eq!(report.translated, 2, "the use stamp and the settings write are both legacy commands");
    assert_eq!(report.left, 0);
    assert_eq!(
        host.sent,
        vec![
            HostCommand::StampRouteUsed { id: 11, utc: host.app.wall_unix_now() },
            HostCommand::PersistSettings { revision: 1 },
        ]
    );

    let (plan, report) = host.pass(20);
    assert_eq!(report.left, 1, "the expiry's removal has no legacy expression");
    assert_eq!(report.translated, 0);
    assert!(report.owned.contains(LegacyOwned::ObjectNamespace));
    let mut effects = plan.effects;
    assert!(
        matches!(effects.catalog.take(), Some(obc_app::catalog_state::CatalogEffect::RemoveObject { object: 22, .. })),
        "and it is still with the catalog, not lost"
    );

    // The row is not a loose end: it names the slice that closes it.
    assert!(LegacyOwned::ObjectNamespace.deletes_in().starts_with('#'));
    assert!(host.app.route_ids().contains(&22), "nothing pretended the object was gone");

    // …and this is what it costs until then, pinned rather than left as prose. Both domains hold an
    // in-flight latch that only their own outcome clears, and this path can build neither, so
    // neither speaks again — the *translated* stamp is as stuck as the untranslatable removal.
    for step in 0..4 {
        let (plan, report) = host.pass(30 + step * 10);
        assert_eq!(report, LegacyReport::default(), "no domain offers anything further");
        assert!(!plan.effects.has_pending(), "and the pass has nothing left to offer either");
    }
    assert_eq!(host.sent.len(), 2, "two commands in the device's whole life on this path");

    // The difference an *answerable* domain makes, in the same run: the settings write has a
    // terminal legacy event, so its answer reaches `SettingsMachine` and the domain speaks again.
    host.deliver(HostEvent::SettingsPersisted { revision: 1 }).unwrap();
    let (_, report) = host.pass(80);
    assert_eq!(report, LegacyReport::default(), "the write is done — nothing is owed");
    assert!(host.adapter.pending().is_empty(), "and its correlation slot is free again");
}

/// The fact half of the protocol, end to end: a store commit, an upload and a warning all reach the
/// rider through the pass, and none of them consumes a correlation slot.
#[test]
fn legacy_facts_reach_the_rider_through_the_pass() {
    let mut host = expiring_host();
    host.pass(10);

    host.deliver(HostEvent::StoreChanged).unwrap();
    host.deliver(HostEvent::Warning(WarningFlags::NO_GPS)).unwrap();
    host.deliver(HostEvent::RouteUploaded { id: 22, replaced: false, elevation: None }).unwrap();
    let owed: Vec<_> = LegacyReply::ALL.into_iter().filter(|class| host.adapter.pending().holds(*class)).collect();
    assert_eq!(
        owed,
        vec![LegacyReply::SettingsWrite],
        "a fact is nobody's answer — the only slot in flight is the boot pass's settings write"
    );

    host.pass(20);
    assert!(host.app.debug_stack_len() > 1, "the facts put something in front of the rider");
    // The commit becomes `CatalogIntent::Refresh` (#1397 S6a), not the legacy rescan cue: the fact
    // reports that the store moved, and the *intent* is what orders the re-read — which the adapter
    // then expresses as the one legacy command the old protocol has for it.
    assert_eq!(host.app.store_changed_pending(), 0, "no legacy rescan cue is latched behind it");
    assert_eq!(host.count(&HostCommand::RescanStore { commits: 1 }), 1, "one commit, one re-read");
}

// ==================== derived levels ====================

/// A derived level repeats until a **matching** key comes back: an answer about another ride is not
/// an answer, and the cue goes out again. This is the whole point of the feeder helper — the legacy
/// `set_ride_profile` / `set_ride_preview` pair carries no subject at all.
#[test]
fn a_derived_need_repeats_until_a_keyed_answer_arrives() {
    let mut host = expiring_host();
    host.pass(10);
    open_ride_detail(&mut host);

    let (plan, _) = host.pass(20);
    let key = plan.derived_needs.ride_track.expect("an open ride detail wants its track");
    assert_eq!(host.count(&HostCommand::LoadRideTrack { id: key.ride }), 1);

    // A pass with no answer sees the level again.
    host.pass(30);
    assert_eq!(host.count(&HostCommand::LoadRideTrack { id: key.ride }), 2, "the level re-emits");

    // An answer about a different ride is about a question nobody asked.
    let other = RideTrackKey { ride: key.ride + 1, source: key.source, view: key.view };
    host.adapter.feed_ride_track(other, DerivedResult::Filled, &mut host.inbox);
    let (plan, _) = host.pass(40);
    assert_eq!(plan.derived_needs.ride_track, Some(key), "a stale fill answers nothing");
    assert_eq!(host.count(&HostCommand::LoadRideTrack { id: key.ride }), 3);

    // The matching key ends it — and a failure would have ended it just the same.
    host.adapter.feed_ride_track(key, DerivedResult::Filled, &mut host.inbox);
    let (plan, _) = host.pass(50);
    assert!(plan.derived_needs.ride_track.is_none(), "the need is answered");
    host.pass(60);
    assert_eq!(host.count(&HostCommand::LoadRideTrack { id: key.ride }), 3, "and the cue stops");
}

fn open_ride_detail(host: &mut CompatHost) {
    for gesture in
        [obc_app::Gesture::Press, obc_app::Gesture::Step(1), obc_app::Gesture::Press, obc_app::Gesture::Press]
    {
        host.app.apply_gesture(gesture);
    }
    assert!(matches!(host.app.top_screen(), Screen::RideDetail(_)), "the ride detail is open");
}

// ==================== delivery timing ====================

/// The same scenario with immediate and with delayed delivery reaches the same terminal core state.
///
/// This is the executor-conformance rule of #1433 §13 in the compatibility shape: an adapter that
/// remembered anything timing-dependent — a retry, a replacement rule, a visible flag — would show
/// up here as two different devices.
#[test]
fn immediate_and_delayed_delivery_reach_the_same_core_state() {
    let run = |delay: u32| {
        let mut host = expiring_host();
        host.pass(10);
        open_ride_detail(&mut host);
        let (plan, _) = host.pass(20);
        let key = plan.derived_needs.ride_track.expect("the detail wants its track");

        // The executor's answers, offered `delay` passes after the work was asked for.
        for step in 0..=delay {
            if step == delay {
                host.deliver(HostEvent::StoreChanged).unwrap();
                host.deliver(HostEvent::Warning(WarningFlags::NO_GPS)).unwrap();
                host.adapter.feed_ride_track(key, DerivedResult::Filled, &mut host.inbox);
            }
            host.pass(30 + step * 10);
        }
        // Settle, so both runners are compared at rest rather than mid-flight.
        for step in 0..4 {
            host.pass(200 + step * 10);
        }
        host.snapshot()
    };

    let immediate = run(0);
    assert_eq!(immediate, run(2), "a two-pass delay changes when, never what");
    assert_eq!(immediate.wants_ride_track, None, "and both settled with the level answered");
}

// ==================== late answers ====================

/// A late answer is rejected by the **domain**, not by the adapter.
///
/// The adapter hands back exactly the token that went out; the domain's own
/// [`TokenSource`] — which cancellation and replacement invalidate — is what refuses it. The
/// alternative, an adapter that dropped late results itself, is the duplicate product policy this
/// epic exists to delete: it would have to know what "cancelled" means.
#[test]
fn a_late_answer_after_cancellation_is_rejected_by_the_domain() {
    let mut adapter = LegacyAdapter::new();
    let mut mail: HostMailbox = HostMailbox::new();
    let mut inbox = LegacyInputs::new();

    // A settings write goes out under the domain's live token.
    let mut settings: TokenSource<SettingsTag> = TokenSource::new();
    let token = settings.issue();
    let mut effects = EffectSlots::new();
    effects.settings.try_put(SettingsEffect::PersistRevision { token, revision: 4 }).unwrap();
    assert_eq!(adapter.effects_to_commands(&mut effects, &mut mail).translated, 1);

    // The rider edits again: the domain replaces the operation, so the old token is not current.
    settings.invalidate();

    // The old write's answer finally arrives.
    adapter.event_to_inputs(HostEvent::SettingsPersisted { revision: 4 }, &mut inbox).unwrap();
    let outcome = inbox.outcomes.settings.take().expect("the adapter still delivers it");
    assert_eq!(outcome, SettingsOutcome::Persisted { token, revision: 4 }, "carrying the original token");
    assert!(!settings.is_current(outcome.token()), "which the domain no longer recognises");

    // The same shape for a cancelled route plan, so the rule is not one domain's accident.
    let mut navigator: TokenSource<NavigatorTag> = TokenSource::new();
    let plan: OperationToken<NavigatorTag> = navigator.issue();
    let work = PlannerWork::Route(NavRequest::new((0, 0), (1, 1), "goal"));
    let mut effects = EffectSlots::new();
    effects.navigator.try_put(NavigatorEffect::Acquire { token: plan, work }).unwrap();
    assert_eq!(adapter.effects_to_commands(&mut effects, &mut mail).translated, 1);
    navigator.invalidate(); // the rider pressed Back
    adapter.event_to_inputs(HostEvent::NavPlanned(Ok(9)), &mut inbox).unwrap();
    let outcome = inbox.outcomes.navigator.take().expect("delivered, not swallowed");
    assert_eq!(outcome, NavigatorOutcome::PlanFinished { token: plan, route: 9 });
    assert!(!navigator.is_current(outcome.token()), "a cancelled plan does not accept its own result");
}

// ==================== one table, two shapes ====================

/// The host completes a whole batch in one call; the board stages effects one at a time before its
/// first await. Both go through the same table and must produce the same commands and the same
/// in-flight set — otherwise "the board and the host run the same core" would be a claim about
/// tests rather than about code.
#[test]
fn the_host_and_the_board_translate_one_batch_identically() {
    let build = || {
        let mut settings: TokenSource<SettingsTag> = TokenSource::new();
        let mut dfu = TokenSource::new();
        let mut recorder = TokenSource::new();
        let mut effects = EffectSlots::new();
        effects.settings.try_put(SettingsEffect::PersistRevision { token: settings.issue(), revision: 9 }).unwrap();
        effects.dfu.try_put(DfuEffect::Scan { token: dfu.issue() }).unwrap();
        effects.recorder.try_put(RecorderEffect::Finalize { token: recorder.issue() }).unwrap();
        effects
    };

    // The host shape: one call, the whole batch.
    let mut host_adapter = LegacyAdapter::new();
    let mut host_mail: HostMailbox = HostMailbox::new();
    let mut batch = build();
    let host_report = host_adapter.effects_to_commands(&mut batch, &mut host_mail);
    let mut host_sent = Vec::new();
    while let Some(command) = host_mail.pop() {
        host_sent.push(command);
    }

    // The board shape: one domain per staging step, before the first await.
    let mut board_adapter = LegacyAdapter::new();
    let mut board_mail: HostMailbox = HostMailbox::new();
    let mut board_report = LegacyReport::default();
    let source = build();
    // Staged in the adapter's own domain order, which is what the board's staging spine follows —
    // comparing against a different staging order would compare this test, not the table.
    for mut staged in [
        EffectSlots { recorder: source.recorder, ..EffectSlots::new() },
        EffectSlots { settings: source.settings, ..EffectSlots::new() },
        EffectSlots { dfu: source.dfu, ..EffectSlots::new() },
    ] {
        let step = board_adapter.effects_to_commands(&mut staged, &mut board_mail);
        board_report.translated += step.translated;
        board_report.left += step.left;
        assert!(!staged.has_pending(), "every staged effect was consumed");
    }
    let mut board_sent = Vec::new();
    while let Some(command) = board_mail.pop() {
        board_sent.push(command);
    }

    assert_eq!(host_report.translated, board_report.translated);
    assert_eq!(host_sent, board_sent, "the same batch, the same commands, in the same order");
    for class in LegacyReply::ALL {
        assert_eq!(
            host_adapter.pending().holds(class),
            board_adapter.pending().holds(class),
            "{class:?} differs between the two shapes"
        );
    }
}

/// The table is a public seam, not an adapter internal: a platform executor can read a row
/// directly to decide what physical work it owes, and every row it cannot serve names what still
/// owns the behaviour.
///
/// The row-by-row mapping is pinned exhaustively inside `obc-app`; what this adds is that the same
/// table is reachable and usable from an executor crate.
#[test]
fn the_mapping_table_is_a_public_seam() {
    let mut navigator: TokenSource<NavigatorTag> = TokenSource::new();
    let work = PlannerWork::Route(NavRequest::new((0, 0), (1, 1), "goal"));
    assert_eq!(
        navigator_row(NavigatorEffect::Acquire { token: navigator.issue(), work }),
        AnsweredRow::Command {
            command: HostCommand::PlanRoute(NavRequest::new((0, 0), (1, 1), "goal")),
            reply: LegacyReply::RoutePlan,
        }
    );
    let mut recorder = TokenSource::new();
    let absent = recorder_row(RecorderEffect::Checkpoint { token: recorder.issue() });
    assert!(matches!(absent, UnansweredRow::Absent(LegacyOwned::RecorderJournal)));
    assert!(absent.command().is_none(), "an executor is told there is no command, not left guessing");

    for row in LegacyOwned::ALL {
        assert!(row.deletes_in().starts_with('#'), "{row:?} must name the slice that removes it");
    }
    // Every reply class is one some real event answers — a class nothing answers would be a
    // correlation slot that fills and never drains.
    for class in LegacyReply::ALL {
        assert!(LEGACY_REPLIES.iter().any(|event| event_reply(event) == Some(class)), "{class:?} is unanswerable");
    }
}

/// One representative of every reply-producing legacy event.
const LEGACY_REPLIES: [HostEvent; 7] = [
    HostEvent::NavPlanned(Ok(1)),
    HostEvent::DetourPlanned(Err(obc_route::nav::NavError::NoPath)),
    HostEvent::DetourCommitted(Ok(2)),
    HostEvent::SettingsPersisted { revision: 1 },
    HostEvent::DfuScanned(Err(obc_app::dfu::DfuScanError::NotFound)),
    HostEvent::DfuInstallBegan,
    HostEvent::CardScanned { free_bytes: None },
];
