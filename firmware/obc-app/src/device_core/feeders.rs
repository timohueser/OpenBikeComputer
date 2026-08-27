//! The **bulk feeder inventory** (#1437): every public way a host pushes data into `App`, and where
//! that data goes once DeviceCore owns it.
//!
//! The feeders are free-standing `pub fn` methods on `App` that grew one at a time, each with its
//! own shape, and *that* is why they need an inventory at all. Every one of them has exactly one new
//! home here, and nothing is left unclassified.
//!
//! **This is documentation and test data, not a dispatcher.** Nothing in a runtime path calls it —
//! `git grep 'feeders::'` finds no importer — so the compiler will not catch a label that has gone
//! stale. Each row dies with the ownership cutover it names.
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
//! `home` is prose rather than a symbol: a third of the destinations name fields that do not exist
//! yet, and inventing placeholder types for them is the speculative structure this repository
//! bans.
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
//! `App`'s actual method surface, so a further public feeder *method* could land with no variant.
//! Being free-standing methods, the feeders have no registry to anchor on; the list was enumerated
//! by hand against `App`. Anyone adding a feeder adds its row here too.

/// Which DeviceCore component owns a feeder's data after the migration.
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
    /// The derived-data path, owned by the requesting screen's domain but delivered without a token.
    Derived,
}

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
    /// `App::begin_ride_profile_fill`
    RideProfileFillBegin,
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

/// Which cutover removes the method.
///
/// **Not the pass cutover.** #1397 S6 moved every host onto `App::run_pass` and deleted the legacy
/// protocol, and these feeders survived it deliberately: a bulk re-feed is how a *typed executor*
/// fills a resident catalog, and it stops being a shim only when the domain owns the catalog itself.
/// Each row therefore names the ownership cutover that really deletes it (#1448 Gate 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeletingSlice {
    /// #1400 — Navigator owns the preview it asked for, and the board's typed effect staging fills
    /// its bounded targets.
    NavigatorOwnership,
    /// Gate 4 — boot state and unrequested facts reach DeviceCore as `PassInputs`, so the boot
    /// feeders and the fact-shaped setters retire together.
    BootAndFacts,
    /// #1401 — the weather storage and request cutover, after FS7.
    WeatherCutover,
    /// The method survives the migration: it is not a shim but a real runtime seam.
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
        // reports only that the read is over. Three route feeders exist because the id column and
        // the retention column were added one at a time; they collapse into one refresh.
        //
        // These four are **not** waiting on refresh ownership (#1541): fill *order* is not policy.
        // `CatalogState::replace_routes` re-resolves every trip's stage ids on either ordering —
        // pinned by `a_catalog_re_feed_mid_cascade_does_not_move_the_cursor` — which is why the
        // board and the host already read in different orders and both are correct. What retires
        // them is the day a bulk fill arrives as `PassInputs` rather than as a `set_*` call.
        Feeder::RoutesWithIds => {
            row(Kind::RefreshOutcome, Own::Catalog, "CatalogOutcome::CatalogRead", When::BootAndFacts)
        }
        Feeder::RoutesWithMeta => row(
            Kind::RefreshOutcome,
            Own::Catalog,
            "CatalogOutcome::CatalogRead + RetentionMachine metadata",
            When::BootAndFacts,
        ),
        Feeder::Trips | Feeder::Rides => {
            row(Kind::RefreshOutcome, Own::Catalog, "CatalogOutcome::CatalogRead", When::BootAndFacts)
        }
        // These two are not waiting on retention ownership (#1548) either, for the same reason as
        // the four above: retention metadata arrives as part of a catalog read's fill, and *where
        // the column lives* is the record consolidation #1398 R4 owns. What retires them is the day
        // a bulk fill arrives as `PassInputs` rather than as a `set_*` call.
        Feeder::RouteMeta => {
            row(Kind::RefreshOutcome, Own::Retention, "RetentionMachine route metadata column", When::BootAndFacts)
        }
        Feeder::RideRetentionInventory => row(
            Kind::RefreshOutcome,
            Own::Retention,
            "CatalogMachine inventory + RetentionMachine input",
            When::BootAndFacts,
        ),

        // ---- keyed derived data: one need, one key, one answer. What is left is the in-place fill
        // the executor borrows: the answer itself is a `DerivedInput` already.
        // The in-place fill retires with its twin below, and for the same reason: what deletes both
        // is a typed executor staging a domain's bounded target, not who owns the catalog.
        Feeder::RideProfileFillBegin => {
            row(Kind::DerivedInput, Own::Derived, "DerivedInputs::ride_track", When::NavigatorOwnership)
        }
        Feeder::NavPreview => {
            row(Kind::DerivedInput, Own::Derived, "DerivedInputs::nav_preview", When::NavigatorOwnership)
        }

        // ---- bounded targets a domain owns. The detour preview is *not* a derived level: it is the
        // result of a planning operation the Navigator asked for, and it carries that operation's
        // token rather than a key.
        Feeder::DetourPreview => {
            row(Kind::BoundedTarget, Own::Navigator, "Navigator detour preview target", When::NavigatorOwnership)
        }

        // ---- external facts: nobody asked for these.
        Feeder::BleStatus => row(Kind::ExternalFact, Own::Fault, "ExternalFacts::link", When::BootAndFacts),
        Feeder::MapTransfer => row(Kind::ExternalFact, Own::Fault, "ExternalFacts::transfer", When::BootAndFacts),
        Feeder::SensorStatus | Feeder::SensorScanHits => {
            row(Kind::ExternalFact, Own::Fault, "PassInputs::sensors", When::BootAndFacts)
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
        Feeder::Settings => row(Kind::BootInput, Own::Settings, "SettingsMachine boot input", When::BootAndFacts),
        Feeder::NavProfiles | Feeder::MapInfo | Feeder::MapNavGraph => {
            row(Kind::BootInput, Own::Catalog, "mounted-map facts at map open", When::BootAndFacts)
        }
        Feeder::FwVersion => row(Kind::BootInput, Own::Dfu, "DfuState running version", When::BootAndFacts),

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
        // The ride-track fill is one need: the executor borrows the buffer and answers the key the
        // need has after the fill.
        let row = feeder_migration(Feeder::RideProfileFillBegin);
        assert_eq!(row.kind, FeederKind::DerivedInput);
        assert_eq!(row.home, "DerivedInputs::ride_track");

        // The detour preview looks like the nav preview and is not: it answers an operation.
        assert_eq!(feeder_migration(Feeder::DetourPreview).kind, FeederKind::BoundedTarget);
        assert_eq!(feeder_migration(Feeder::NavPreview).kind, FeederKind::DerivedInput);

        // Weather feeders wait for #1401.
        assert_eq!(feeder_migration(Feeder::WeatherFeed).deletes_in, DeletingSlice::WeatherCutover);
        assert_eq!(feeder_migration(Feeder::RainView).deletes_in, DeletingSlice::WeatherCutover);

        // Nothing a domain owns survives the migration.
        let kept = Feeder::ALL.iter().filter(|&&f| feeder_migration(f).deletes_in == DeletingSlice::Kept).count();
        assert_eq!(kept, 3, "only the render/input-plane seams are kept");
    }
}
