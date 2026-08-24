//! The **bulk feeder inventory** (#1437): every public way a host pushes data into `App`, and where
//! that data goes once DeviceCore owns it.
//!
//! The legacy protocol is two enums and one table ([`migration`](super::migration)). The feeders are
//! not: they are 27 free-standing `pub fn` methods on `App` that grew one at a time, each with its
//! own shape, and *that* is why they need an inventory at all. Every one of them has exactly one new
//! home here, and nothing is left unclassified.
//!
//! **This is documentation and test data, not a dispatcher.** Nothing in a runtime path calls it.
//! It dies with the pass cutover (#1397 S6), together with the wrappers it describes.
//!
//! ## Reading a row
//!
//! | Column | What it says |
//! |---|---|
//! | [`kind`](FeederMigration::kind) | What the data *is* once it crosses the seam. |
//! | [`owner`](FeederMigration::owner) | Which DeviceCore component owns it afterwards. |
//! | [`home`](FeederMigration::home) | Where it lands, as prose — not a compiler-checked symbol. |
//! | [`deletes_in`](FeederMigration::deletes_in) | The slice that removes the method. |
//!
//! `home` is prose for the same reason [`LegacyMigration::home`](super::migration::LegacyMigration)
//! is: a third of the destinations name fields that do not exist yet, and inventing placeholder
//! types for them is the speculative structure this repository bans.
//!
//! ## What is actually guarded, and what is not
//!
//! Two of the three things this table could get wrong are structural:
//!
//! - **A variant cannot be missing from [`Feeder::ALL`], and cannot be listed twice.** The enum,
//!   `ALL` and [`COUNT`](Feeder::COUNT) are all generated from one list by [`feeders!`], and a
//!   repeated name would not be a legal enum. No test is needed, and none is written.
//! - **A variant cannot be missing a row.** [`feeder_migration`] is an exhaustive match.
//!
//! The third is **not** guarded and is stated here rather than implied: nothing ties this enum to
//! `App`'s actual method surface, so a twenty-eighth public feeder *method* could land with no
//! variant. [`migration`](super::migration) has an anchor for its half — `HostCommand::DRAIN_ORDER`
//! — and the feeders, being free-standing methods, have no such registry to anchor on. The list was
//! enumerated by hand against `App` when this slice landed: 24 `pub fn set_*` plus
//! `begin_ride_profile_fill`, `finish_ride_profile_fill` and `weather_feed_changed`. Anyone adding a
//! feeder before DC6 #1439 deletes this file adds its row here too.

use super::migration::LegacyOwner;

/// Declare the feeder vocabulary once: the enum, [`Feeder::ALL`] and [`Feeder::COUNT`] all come out
/// of this single list, so a variant cannot exist outside `ALL` and `COUNT` cannot drift from
/// either. That is the completeness guard this table's whole value rests on, made structural
/// instead of asserted — a hand-written `ALL` beside a hand-written `COUNT` would let a new variant
/// be invisible to every test in this file.
macro_rules! feeders {
    ($( $(#[$meta:meta])* $variant:ident ),+ $(,)?) => {
        /// One public bulk feeder on `App`, named after the method.
        ///
        /// The list is the complete public feeding surface: every `App::set_*`, plus the in-place
        /// ride profile fill pair and the weather snapshot pulse, which feed data without being
        /// named `set_`.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Feeder {
            $( $(#[$meta])* $variant, )+
        }

        impl Feeder {
            /// Every public bulk feeder, in declaration order.
            pub const ALL: &'static [Feeder] = &[ $( Feeder::$variant, )+ ];

            /// How many public bulk feeders exist. The migration is complete when only the
            /// [`DeletingSlice::Kept`] rows are left.
            pub const COUNT: usize = Feeder::ALL.len();
        }
    };
}

feeders! {
    /// `App::set_nav_profiles`
    NavProfiles,
    /// `App::set_fw_version`
    FwVersion,
    /// `App::set_map_info`
    MapInfo,
    /// `App::set_map_mpp`
    MapMpp,
    /// `App::set_map_nav_graph`
    MapNavGraph,
    /// `App::set_routes`
    Routes,
    /// `App::set_routes_with_ids`
    RoutesWithIds,
    /// `App::set_routes_with_meta`
    RoutesWithMeta,
    /// `App::set_route_meta`
    RouteMeta,
    /// `App::set_trips`
    Trips,
    /// `App::set_rides`
    Rides,
    /// `App::set_ride_retention_inventory`
    RideRetentionInventory,
    /// `App::set_ride_profile`
    RideProfile,
    /// `App::begin_ride_profile_fill`
    RideProfileFillBegin,
    /// `App::finish_ride_profile_fill`
    RideProfileFillFinish,
    /// `App::set_ride_preview`
    RidePreview,
    /// `App::set_nav_preview`
    NavPreview,
    /// `App::set_detour_preview`
    DetourPreview,
    /// `App::set_settings`
    Settings,
    /// `App::set_ble_status`
    BleStatus,
    /// `App::set_map_transfer`
    MapTransfer,
    /// `App::set_sensor_status`
    SensorStatus,
    /// `App::set_sensor_scan_hits`
    SensorScanHits,
    /// `App::set_rain_view`
    RainView,
    /// `App::weather_feed_changed`
    WeatherFeed,
    /// `App::set_hold_progress`
    HoldProgress,
    /// `App::set_render_clip`
    RenderClip,
}

/// What a feeder's data becomes on the DeviceCore side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeederKind {
    /// The result of a catalog refresh — the executor filling a domain-owned resident catalog.
    RefreshOutcome,
    /// A keyed derived answer, guarded by its key rather than a token.
    DerivedInput,
    /// A bounded result target a domain owns and an executor fills in place.
    BoundedTarget,
    /// A fact nobody asked for: no token, no key.
    ExternalFact,
    /// Data supplied once at boot before any pass runs.
    BootInput,
    /// Render-plane or input-plane state, applied by the runtime around a pass rather than owned by
    /// a domain at all.
    PlatformState,
}

/// Which slice removes the method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeletingSlice {
    /// #1397 S6 — every host runs `App::run_pass`, so the legacy protocol and its feeders go with
    /// the frame methods they belong to. DC6 #1439 built the replacement path
    /// ([`compat`](super::compat)) but deliberately left the production call sites alone, so the
    /// methods themselves survive until the cutover.
    PassCutover,
    /// #1401 — the weather storage and request cutover, after FS7.
    WeatherCutover,
    /// The method survives the migration: it is not a legacy shim but a real runtime seam.
    Kept,
}

/// One feeder's new home.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeederMigration {
    /// What the data becomes.
    pub kind: FeederKind,
    /// Which component owns it afterwards.
    pub owner: LegacyOwner,
    /// Where it lands, as display prose for a human reading the plan.
    pub home: &'static str,
    /// The slice that deletes the method.
    pub deletes_in: DeletingSlice,
}

const fn row(kind: FeederKind, owner: LegacyOwner, home: &'static str, deletes_in: DeletingSlice) -> FeederMigration {
    FeederMigration { kind, owner, home, deletes_in }
}

/// Where `feeder`'s data goes. Exhaustive by construction: a twenty-eighth feeder does not compile
/// until it has a row here.
pub fn feeder_migration(feeder: Feeder) -> FeederMigration {
    use DeletingSlice as When;
    use FeederKind as Kind;
    use LegacyOwner as Own;
    match feeder {
        // ---- catalog refresh outcomes: the executor fills the resident catalogs, the outcome
        // reports only the revision it read at. Three route feeders exist because the id column and
        // the retention column were added one at a time; they collapse into one refresh.
        Feeder::Routes | Feeder::RoutesWithIds => {
            row(Kind::RefreshOutcome, Own::Catalog, "CatalogOutcome::CatalogRead", When::PassCutover)
        }
        Feeder::RoutesWithMeta => row(
            Kind::RefreshOutcome,
            Own::Catalog,
            "CatalogOutcome::CatalogRead + RetentionMachine metadata",
            When::PassCutover,
        ),
        Feeder::Trips | Feeder::Rides => {
            row(Kind::RefreshOutcome, Own::Catalog, "CatalogOutcome::CatalogRead", When::PassCutover)
        }
        Feeder::RouteMeta => {
            row(Kind::RefreshOutcome, Own::Retention, "RetentionMachine route metadata column", When::PassCutover)
        }
        Feeder::RideRetentionInventory => row(
            Kind::RefreshOutcome,
            Own::Retention,
            "CatalogMachine inventory + RetentionMachine input",
            When::PassCutover,
        ),

        // ---- keyed derived data: one need, one key, one answer (this slice).
        Feeder::RideProfile | Feeder::RideProfileFillBegin | Feeder::RideProfileFillFinish | Feeder::RidePreview => {
            row(Kind::DerivedInput, Own::Derived, "DerivedInputs::ride_track", When::PassCutover)
        }
        Feeder::NavPreview => row(Kind::DerivedInput, Own::Derived, "DerivedInputs::nav_preview", When::PassCutover),

        // ---- bounded targets a domain owns. The detour preview is *not* a derived level: it is the
        // result of a planning operation the Navigator asked for, and it carries that operation's
        // token rather than a key.
        Feeder::DetourPreview => {
            row(Kind::BoundedTarget, Own::Navigator, "Navigator detour preview target", When::PassCutover)
        }

        // ---- external facts: nobody asked for these.
        Feeder::BleStatus => row(Kind::ExternalFact, Own::Fault, "ExternalFacts::link", When::PassCutover),
        Feeder::MapTransfer => row(Kind::ExternalFact, Own::Fault, "ExternalFacts::transfer", When::PassCutover),
        Feeder::SensorStatus | Feeder::SensorScanHits => {
            row(Kind::ExternalFact, Own::Fault, "PassInputs::sensors", When::PassCutover)
        }
        Feeder::WeatherFeed => {
            row(Kind::ExternalFact, Own::Weather, "ExternalFacts::weather_data", When::WeatherCutover)
        }
        // Two halves with two homes: the step range and zoom floor are weather view state, but the
        // method also re-clamps the map camera into the product's regime — UI-plane work no
        // `WeatherVisible` field models, and the awkward half of this row to move.
        Feeder::RainView => row(
            Kind::ExternalFact,
            Own::Weather,
            "WeatherDomain visible view state + a UiRuntime rain-zoom clamp",
            When::WeatherCutover,
        ),

        // ---- boot inputs: supplied once, before any pass.
        Feeder::Settings => row(Kind::BootInput, Own::Settings, "SettingsMachine boot input", When::PassCutover),
        Feeder::NavProfiles | Feeder::MapInfo | Feeder::MapNavGraph => {
            row(Kind::BootInput, Own::Catalog, "mounted-map facts at map open", When::PassCutover)
        }
        Feeder::FwVersion => row(Kind::BootInput, Own::Dfu, "DfuState running version", When::PassCutover),

        // ---- platform state around a pass. These are not legacy shims: the runtime owns the
        // display and the input plane on every platform, and says so through these.
        Feeder::RenderClip => row(Kind::PlatformState, Own::Fault, "UiRuntime render state", When::Kept),
        // Kept, but not ordinary render state: this is the USB-CDC `Z` command's hook into the
        // strippable render-instrumentation seam, and the table is only worth having if it says so.
        Feeder::MapMpp => {
            row(Kind::PlatformState, Own::Fault, "UiRuntime render-instrumentation seam (debug)", When::Kept)
        }
        Feeder::HoldProgress => row(Kind::PlatformState, Own::Fault, "UiRuntime input plane", When::Kept),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every row names a real home and a real deleting slice — an inventory is only useful while no
    /// row is a placeholder.
    #[test]
    fn every_row_names_a_home_and_a_deleting_slice() {
        for &feeder in Feeder::ALL {
            let row = feeder_migration(feeder);
            assert!(!row.home.is_empty(), "{feeder:?} has no home");
            // A feeder that survives must be platform state; anything a domain owns is a shim.
            if row.deletes_in == DeletingSlice::Kept {
                assert_eq!(row.kind, FeederKind::PlatformState, "{feeder:?} outlives the migration without reason");
            }
        }
    }

    /// The classifications worth stating out loud, because getting them wrong is expensive.
    #[test]
    fn the_locked_classifications_hold() {
        // The four ride-track feeders are one need, not four: they are fed together and answer the
        // same key, which is why merging them costs nothing.
        for feeder in
            [Feeder::RideProfile, Feeder::RideProfileFillBegin, Feeder::RideProfileFillFinish, Feeder::RidePreview]
        {
            let row = feeder_migration(feeder);
            assert_eq!(row.kind, FeederKind::DerivedInput);
            assert_eq!(row.home, "DerivedInputs::ride_track");
        }

        // The detour preview looks like the nav preview and is not: it answers an operation.
        assert_eq!(feeder_migration(Feeder::DetourPreview).kind, FeederKind::BoundedTarget);
        assert_eq!(feeder_migration(Feeder::NavPreview).kind, FeederKind::DerivedInput);

        // Weather feeders wait for #1401, not DC6.
        assert_eq!(feeder_migration(Feeder::WeatherFeed).deletes_in, DeletingSlice::WeatherCutover);
        assert_eq!(feeder_migration(Feeder::RainView).deletes_in, DeletingSlice::WeatherCutover);

        // Nothing a domain owns survives the migration.
        let kept = Feeder::ALL.iter().filter(|&&f| feeder_migration(f).deletes_in == DeletingSlice::Kept).count();
        assert_eq!(kept, 3, "only the render/input-plane seams are kept");
    }
}
