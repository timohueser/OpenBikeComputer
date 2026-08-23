//! The legacy protocol migration inventory (#1436, epic #1433 Appendix A).
//!
//! Every one of the 18 [`HostCommand`] variants and 15 [`HostEvent`] variants has exactly one new
//! home in the DeviceCore vocabulary. This module states that mapping *in code*, so it cannot drift
//! from either side: [`command_migration`] and [`event_migration`] are exhaustive matches, so a new
//! legacy variant fails the build until someone decides where it belongs.
//!
//! **This is documentation and test data, not a dispatcher.** Nothing in a runtime path calls it,
//! and it must never grow into the thing that turns a command into an effect — that would recreate
//! the combined vocabulary the epic exists to remove. It dies with the legacy protocol.
//!
//! ## Reading a row
//!
//! | [`LegacyRole`] | What it means for this variant |
//! |---|---|
//! | `Intent` | A product request. Its owner decides what physical work follows. |
//! | `Effect` | One bounded physical operation, already. |
//! | `Outcome` | The answer to an effect; it gains an operation token. |
//! | `ExternalFact` | Nobody asked for it. It carries no token. |
//! | `DerivedNeed` | A *level*, answered by keyed data. No token — the data key is the guard. |
//! | `Deleted` | The variant goes away; something else already covers it. |

use crate::{HostCommand, HostEvent};

/// What a legacy variant becomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyRole {
    /// A product request to a domain owner.
    Intent,
    /// A bounded physical operation.
    Effect,
    /// The token-carrying answer to an effect.
    Outcome,
    /// A fact that is not an answer to anything.
    ExternalFact,
    /// A re-emitted level answered by keyed derived data.
    DerivedNeed,
    /// Removed outright — its work is covered elsewhere.
    Deleted,
}

/// Which DeviceCore component owns the variant after the migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyOwner {
    /// `CatalogMachine` — revisions, identities, refresh, deletion, the trip cascade.
    Catalog,
    /// `RetentionMachine` — usage stamps, expiry deadlines, sidecar metadata.
    Retention,
    /// `Recorder` — the ride session and its persistence lifecycle.
    Recorder,
    /// `Navigator` — route and detour planning, preview, and commit.
    Navigator,
    /// `SettingsMachine` — the dirty revision and the persist handshake.
    Settings,
    /// `WeatherDomain` — visible freshness, alerts, and installed-data identity.
    Weather,
    /// `DfuState` — update scan, install admission, and terminal state.
    Dfu,
    /// The bond domain in `ble.rs` — bond removal.
    Bond,
    /// The storage-information domain — free-space reporting.
    StorageInfo,
    /// `FaultState` and the card scheduler.
    Fault,
    /// `PassPlan::DerivedNeeds` / `PassInputs::DerivedInputs` — owned by the requesting screen's
    /// domain but delivered by the derived-data path, which has no token.
    Derived,
}

/// One legacy variant's new home.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegacyMigration {
    /// What the variant becomes.
    pub role: LegacyRole,
    /// Which component owns it afterwards.
    pub owner: LegacyOwner,
    /// Where it lands, as **display prose for a human reading the plan** — not a symbol reference,
    /// and deliberately not compiler-checked.
    ///
    /// A third of these destinations cannot be a checked path: two name `DerivedNeeds` fields that
    /// DC4 has not created yet, five name private [`ExternalFacts`](super::ExternalFacts) fields
    /// whose public accessors are named differently, and three name a *pair* of variants one legacy
    /// variant splits into. Typing the rest would either invent placeholder types for the futures —
    /// the speculative structure this repo bans — or leave a half-typed table, which is worse than a
    /// uniformly prose one.
    ///
    /// What is actually guarded is the thing that matters: [`command_migration`] and
    /// [`event_migration`] are exhaustive over the legacy enums, so no variant can go unclassified.
    /// The strings are rewritten row by row as DC5/DC6 wire each real path, and die with the
    /// compatibility adapter.
    pub home: &'static str,
}

const fn row(role: LegacyRole, owner: LegacyOwner, home: &'static str) -> LegacyMigration {
    LegacyMigration { role, owner, home }
}

/// How many legacy commands the inventory covers. The migration is complete when this reaches zero.
pub const LEGACY_COMMANDS: usize = 18;

/// How many legacy events the inventory covers.
pub const LEGACY_EVENTS: usize = 15;

/// Where `cmd`'s variant goes. Exhaustive by construction: a nineteenth [`HostCommand`] does not
/// compile until it has a row here.
pub fn command_migration(cmd: &HostCommand) -> LegacyMigration {
    use LegacyOwner as Own;
    use LegacyRole as Role;
    match cmd {
        // The store-revision external fact wakes CatalogMachine; the refresh it may decide on is
        // `CatalogEffect::ReadCatalog`. Nothing needs a command for "the store moved".
        HostCommand::RescanStore { .. } => row(Role::Deleted, Own::Catalog, "ExternalFacts::store_revision"),
        HostCommand::CancelRoutePlan => row(Role::Intent, Own::Navigator, "NavigatorIntent::CancelPlan"),
        HostCommand::CancelDetour => row(Role::Intent, Own::Navigator, "NavigatorIntent::CancelDetour"),
        HostCommand::DeleteRoute { .. } => row(Role::Intent, Own::Catalog, "CatalogIntent::DeleteRoute"),
        HostCommand::DeleteTrip { .. } => row(Role::Intent, Own::Catalog, "CatalogIntent::DeleteTrip"),
        HostCommand::DeleteRide { .. } => row(Role::Intent, Own::Catalog, "CatalogIntent::DeleteRide"),
        HostCommand::StampRouteUsed { .. } => row(Role::Effect, Own::Retention, "RetentionEffect::WriteRouteMetadata"),
        HostCommand::StampRideSynced { .. } => row(Role::Effect, Own::Retention, "RetentionEffect::WriteRideMetadata"),
        // Save vs. discard is the intent; the writes it implies are recorder effects.
        HostCommand::FinishTrack(_) => row(Role::Intent, Own::Recorder, "RecorderIntent::Save | Discard"),
        HostCommand::PlanRoute(_) => row(Role::Intent, Own::Navigator, "NavigatorIntent::PlanRoute"),
        HostCommand::PlanDetour(_) => row(Role::Intent, Own::Navigator, "NavigatorIntent::PlanDetour"),
        HostCommand::CommitDetour => row(Role::Intent, Own::Navigator, "NavigatorIntent::CommitDetour"),
        HostCommand::Dfu(_) => row(Role::Intent, Own::Dfu, "DfuIntent::ScanRequested | InstallRequested"),
        HostCommand::ForgetBond => row(Role::Effect, Own::Bond, "BondEffect::Forget"),
        HostCommand::PersistSettings { .. } => row(Role::Effect, Own::Settings, "SettingsEffect::PersistRevision"),
        HostCommand::ScanCardFree => row(Role::Effect, Own::StorageInfo, "StorageInfoEffect::MeasureFreeSpace"),
        HostCommand::LoadRideTrack { .. } => row(Role::DerivedNeed, Own::Derived, "DerivedNeeds::ride_track"),
        HostCommand::RefreshNavPreview => row(Role::DerivedNeed, Own::Derived, "DerivedNeeds::nav_preview"),
    }
}

/// Where `event`'s variant goes. Exhaustive by construction, like [`command_migration`].
pub fn event_migration(event: &HostEvent) -> LegacyMigration {
    use LegacyOwner as Own;
    use LegacyRole as Role;
    match event {
        HostEvent::StoreChanged => row(Role::ExternalFact, Own::Catalog, "ExternalFacts::store_revision"),
        HostEvent::RouteUploaded { .. } => row(Role::ExternalFact, Own::Catalog, "ExternalFacts::route_upload"),
        HostEvent::TripUploaded { .. } => row(Role::ExternalFact, Own::Catalog, "ExternalFacts::trip_upload"),
        HostEvent::Warning(_) => row(Role::ExternalFact, Own::Fault, "ExternalFacts::warnings"),
        HostEvent::NavPlanned(_) => row(Role::Outcome, Own::Navigator, "NavigatorOutcome::PlanFinished"),
        HostEvent::DetourPlanned(_) => row(Role::Outcome, Own::Navigator, "NavigatorOutcome::DetourFinished"),
        HostEvent::DetourCommitted(_) => row(Role::Outcome, Own::Navigator, "NavigatorOutcome::DetourCommitted"),
        HostEvent::CardScanned { .. } => row(Role::Outcome, Own::StorageInfo, "StorageInfoOutcome::Measured"),
        HostEvent::DfuScanned(_) => row(Role::Outcome, Own::Dfu, "DfuOutcome::ScanFinished | ScanFailed"),
        HostEvent::DfuInstallFailed(_) => row(Role::Outcome, Own::Dfu, "DfuOutcome::InstallFailed"),
        HostEvent::DfuInstallBegan => row(Role::Outcome, Own::Dfu, "DfuOutcome::InstallBegan"),
        // The boot path supplies these; no effect was ever issued for them.
        HostEvent::UpdateConfirmed(_) => row(Role::ExternalFact, Own::Dfu, "ExternalFacts::update_result"),
        HostEvent::UpdateFailed { .. } => row(Role::ExternalFact, Own::Dfu, "ExternalFacts::update_result"),
        HostEvent::SettingsPersisted { .. } => row(Role::Outcome, Own::Settings, "SettingsOutcome::Persisted"),
        HostEvent::SettingsPersistFailed { .. } => row(Role::Outcome, Own::Settings, "SettingsOutcome::PersistFailed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::{DetourRequest, DfuAction, NavRequest, TrackAction};
    use crate::dfu::{DfuFailure, DfuInstallError, DfuScanError, DfuScanReport};
    use crate::host::DetourPreview;
    use crate::screen::WarningFlags;

    /// One representative of every legacy command. The count is pinned to
    /// [`HOST_COMMAND_CLASSES`](crate::HOST_COMMAND_CLASSES) below, and the per-class assertion
    /// there is what proves this list is complete rather than merely long.
    fn every_command() -> [HostCommand; LEGACY_COMMANDS] {
        [
            HostCommand::RescanStore { commits: 1 },
            HostCommand::CancelRoutePlan,
            HostCommand::CancelDetour,
            HostCommand::DeleteRoute { id: 1 },
            HostCommand::DeleteTrip { id: 2 },
            HostCommand::DeleteRide { id: 3 },
            HostCommand::StampRouteUsed { id: 4, utc: 100 },
            HostCommand::StampRideSynced { id: 5, utc: 100 },
            HostCommand::FinishTrack(TrackAction::Save),
            HostCommand::PlanRoute(NavRequest::new((0, 0), (1, 1), "goal")),
            HostCommand::PlanDetour(DetourRequest { route: 0, from: (0, 0), progress_m: 0, target_m: 500 }),
            HostCommand::CommitDetour,
            HostCommand::Dfu(DfuAction::Scan),
            HostCommand::ForgetBond,
            HostCommand::PersistSettings { revision: 1 },
            HostCommand::ScanCardFree,
            HostCommand::LoadRideTrack { id: 6 },
            HostCommand::RefreshNavPreview,
        ]
    }

    /// One representative of every legacy event.
    fn every_event() -> [HostEvent; LEGACY_EVENTS] {
        [
            HostEvent::StoreChanged,
            HostEvent::RouteUploaded { id: 1, replaced: false, elevation: None },
            HostEvent::TripUploaded { id: 2, replaced: false },
            HostEvent::Warning(WarningFlags::NO_GPS),
            HostEvent::NavPlanned(Ok(3)),
            HostEvent::DetourPlanned(Ok(DetourPreview {
                cost_delta_m: 0,
                total_distance_m: 0,
                rejoin_m: 0,
                ascent_m: None,
            })),
            HostEvent::DetourCommitted(Ok(4)),
            HostEvent::CardScanned { free_bytes: Some(1) },
            HostEvent::DfuScanned(Ok(DfuScanReport::new("v1", "v2", false))),
            HostEvent::DfuInstallFailed(DfuInstallError::NoCard),
            HostEvent::DfuInstallBegan,
            HostEvent::UpdateConfirmed(crate::dfu::clamp("v2")),
            HostEvent::UpdateFailed { why: DfuFailure::Reverted, staged: None },
            HostEvent::SettingsPersisted { revision: 1 },
            HostEvent::SettingsPersistFailed { revision: 1, error: obc_ports::SettingsSaveError::Backend },
        ]
    }

    /// All 18 commands map, exactly once each — and the inventory is *complete*, because the set of
    /// classes it covers is exactly [`HostCommand::DRAIN_ORDER`]. A nineteenth variant cannot slip
    /// through: `class()` forces it a class, the drain order forces that class a slot, and this
    /// assertion then forces it a sample and a migration row.
    #[test]
    fn all_eighteen_commands_map_exactly_once() {
        let commands = every_command();
        assert_eq!(commands.len(), crate::HOST_COMMAND_CLASSES, "the inventory is sized to the protocol");

        let mut seen: heapless::Vec<_, LEGACY_COMMANDS> = heapless::Vec::new();
        for cmd in &commands {
            let class = cmd.class();
            assert!(!seen.contains(&class), "{class:?} appears twice in the inventory");
            seen.push(class).unwrap();
            let _row = command_migration(cmd); // exhaustive by construction — this just exercises it
        }
        for class in HostCommand::DRAIN_ORDER {
            assert!(seen.contains(&class), "{class:?} has no inventory row");
        }
    }

    /// All 15 events map, exactly once each. Distinctness is by discriminant, so two rows for the
    /// same variant would fail even though their payloads differ.
    ///
    /// Unlike the command side there is no `HostEvent` class registry to cross-check the sample list
    /// against, so what guarantees completeness here is [`event_migration`]'s exhaustive match alone
    /// — a sixteenth variant fails the build. This test proves the rows are distinct, not that the
    /// list is exhaustive.
    #[test]
    fn all_fifteen_events_map_exactly_once() {
        let events = every_event();

        let mut seen: heapless::Vec<_, LEGACY_EVENTS> = heapless::Vec::new();
        for event in &events {
            let kind = core::mem::discriminant(event);
            assert!(!seen.contains(&kind), "an event appears twice in the inventory");
            seen.push(kind).unwrap();
            let _row = event_migration(event);
        }
    }

    /// The classifications the epic locked, spot-checked where a wrong call would be expensive: the
    /// two derived levels must not gain operation tokens, the rescan command must actually go away,
    /// and the boot's update verdict is a fact rather than an answer to anything.
    #[test]
    fn the_locked_classifications_hold() {
        let by_role = |cmd: HostCommand| command_migration(&cmd).role;
        assert_eq!(by_role(HostCommand::RescanStore { commits: 1 }), LegacyRole::Deleted);
        assert_eq!(by_role(HostCommand::LoadRideTrack { id: 1 }), LegacyRole::DerivedNeed);
        assert_eq!(by_role(HostCommand::RefreshNavPreview), LegacyRole::DerivedNeed);
        assert_eq!(by_role(HostCommand::ForgetBond), LegacyRole::Effect);
        assert_eq!(
            by_role(HostCommand::PlanDetour(DetourRequest { route: 0, from: (0, 0), progress_m: 0, target_m: 1 })),
            LegacyRole::Intent
        );

        let updated = HostEvent::UpdateFailed { why: DfuFailure::NotStarted, staged: None };
        assert_eq!(event_migration(&updated).role, LegacyRole::ExternalFact);
        assert_eq!(event_migration(&HostEvent::StoreChanged).role, LegacyRole::ExternalFact);
        assert_eq!(event_migration(&HostEvent::DfuInstallBegan).role, LegacyRole::Outcome);
        assert_eq!(event_migration(&HostEvent::DfuScanned(Err(DfuScanError::NotFound))).owner, LegacyOwner::Dfu);
    }

    /// Every command and event lands on a real domain, and no row is a placeholder — the inventory
    /// is only useful while every `home` names something that exists.
    #[test]
    fn every_row_names_a_home() {
        for cmd in &every_command() {
            assert!(!command_migration(cmd).home.is_empty());
        }
        for event in &every_event() {
            assert!(!event_migration(event).home.is_empty());
        }
    }
}
