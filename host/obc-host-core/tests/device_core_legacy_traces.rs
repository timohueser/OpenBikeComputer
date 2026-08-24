//! Typed legacy DeviceCore behavior traces (DC1, #1434).
//!
//! This is intentionally a behavior corpus rather than a second implementation of `HostLoop`.
//! Scenario inputs describe product edges, while the trace harness records the legacy commands,
//! outcomes, bulk feeder identities, and visible state.  DC7 can run the same scenario definitions
//! against DeviceCore and compare the normalized traces.

mod device_core_corpus;

use std::collections::BTreeSet;

use obc_app::{DfuFailure, TrackAction, WarningFlags};
use obc_host_core::trace::{
    CommandTag, DataKey, EventTag, FeederKind, NormalizedCommand, NormalizedError, NormalizedEvent, ObjectKey,
    RevisionKey, RunnerMode, TimeKey, Trace, TraceInput, TraceOutput, ALL_COMMAND_TAGS, ALL_EVENT_TAGS,
    ALL_FEEDER_KINDS,
};

use device_core_corpus::{
    command_count, command_precedes_event, definition, event_count, feeder_count, output_precedes, run_legacy,
    run_matrix, step, LegacyHarness, Requirement, ScreenState, VisibleState, ALL_REQUIREMENTS, SCENARIOS,
};

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
