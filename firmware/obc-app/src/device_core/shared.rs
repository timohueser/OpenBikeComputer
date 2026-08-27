//! The shared DeviceCore vocabulary: operation tokens, capabilities, and external facts (#1435).
//!
//! Nothing here performs work. These are the three small contracts every domain state machine and
//! every platform executor agrees on:
//!
//! | Contract | Question it answers |
//! |---|---|
//! | [`OperationToken`] | Is this result still the one I asked for? |
//! | [`Capabilities`] | Can this device do this operation *at all*? |
//! | [`ExternalFacts`] | What changed underneath me that nobody asked for? |
//!
//! Every type here is bounded and free of platform handles, paths and unbounded collections — a
//! value in this module can cross the DeviceCore ↔ executor seam on any platform. All of them are
//! `Copy` except [`UpdateResult`] and the [`ExternalFacts`] that holds it: an update result carries
//! `Version` strings ([`heapless::String`], fixed 32-byte buffers), which are bounded but not
//! `Copy`. Those two are `Clone` instead — bounded either way, and neither ever allocates.

use core::marker::PhantomData;

use crate::ble::BleStatus;
use crate::dfu::{DfuFailure, Version};
use crate::screen::WarningFlags;
use crate::CatalogObjectId;

// ==================== operation tokens ====================

// The domain tags an `OperationToken` is typed by — one per domain that owns asynchronous work
// (the effect/outcome slots of epic #1433 §5). They are uninhabited: a tag is a *name at the type
// level*, never a value, so no trait impl on one could ever be called. `UiRuntime`, `CoreMode` and
// `FaultState` have no tag because they issue no effects and therefore own no operation.

/// Catalog revisions, refresh, deletion, and the trip cascade.
pub enum CatalogTag {}
/// Route-use and ride-sync stamps, and expiry metadata writes.
pub enum RetentionTag {}
/// Ride samples, checkpoints, finalize and discard.
pub enum RecorderTag {}
/// Route planning, detour planning, preview and commit.
pub enum NavigatorTag {}
/// The settings persist handshake.
pub enum SettingsTag {}
/// Weather refresh and installed weather data.
pub enum WeatherTag {}
/// Firmware-update scan and install arming.
pub enum DfuTag {}
/// Bond removal.
pub enum BondTag {}
/// Free-space measurement.
pub enum StorageInfoTag {}

/// The identity of one in-flight operation, typed by its owning domain.
///
/// Every effect carries the token its domain issued, and every outcome carries it back. The domain
/// owner accepts an outcome only while [`TokenSource::is_current`] holds — that single equality is
/// how a *superseded* result (cancelled, replaced, or belonging to an operation the domain has
/// since moved past) is rejected without the executor knowing any product rule.
///
/// **What equality alone does not reject.** The generation keeps standing after the operation ends,
/// so a duplicate or post-terminal outcome carrying the same token still compares current. Closing
/// that is the domain owner's obligation, and it is one line: **invalidate when you accept a
/// terminal outcome** ([`TokenSource::invalidate`]), exactly as cancellation and replacement do.
/// An owner that skips it will reprocess a repeated result, and the bug will look like a state
/// machine bug rather than the contract gap it is.
///
/// The generation is private and never zero, so a token cannot be forged, minted by an executor, or
/// confused with "no operation". `PhantomData<fn() -> Tag>` keeps the tag invariant-free (the token
/// stays `Copy` and `Send` regardless of the tag) while making a token of one domain unusable as a
/// token of another:
///
/// ```compile_fail
/// use obc_app::device_core::{CatalogTag, NavigatorTag, OperationToken, TokenSource};
///
/// let mut catalog: TokenSource<CatalogTag> = TokenSource::new();
/// // A catalog token can never stand in for a navigator one.
/// let navigator: OperationToken<NavigatorTag> = catalog.issue();
/// ```
pub struct OperationToken<Tag> {
    generation: u32,
    tag: PhantomData<fn() -> Tag>,
}

impl<Tag> Clone for OperationToken<Tag> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<Tag> Copy for OperationToken<Tag> {}

impl<Tag> PartialEq for OperationToken<Tag> {
    fn eq(&self, other: &Self) -> bool {
        self.generation == other.generation
    }
}

impl<Tag> Eq for OperationToken<Tag> {}

impl<Tag> core::fmt::Debug for OperationToken<Tag> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "OperationToken({})", self.generation)
    }
}

/// The single minter of one domain's [`OperationToken`]s — a domain state machine owns exactly one
/// and no executor ever holds it.
///
/// Generation `0` is the boot state "nothing was ever issued", so a token can never equal it: the
/// wrap in [`issue`](Self::issue) skips back to `1`. Wrapping is sound because only the *latest*
/// generation is ever current — a stale token would have to survive 2³² further operations of the
/// same domain to be mistaken for it.
pub struct TokenSource<Tag> {
    generation: u32,
    tag: PhantomData<fn() -> Tag>,
}

impl<Tag> TokenSource<Tag> {
    /// A source that has issued nothing.
    pub const fn new() -> Self {
        TokenSource { generation: 0, tag: PhantomData }
    }

    /// Start an operation: invalidate every outstanding token and mint the new one.
    #[must_use = "the issued token identifies the new operation; use `invalidate` to only cancel"]
    pub fn issue(&mut self) -> OperationToken<Tag> {
        self.generation = match self.generation.wrapping_add(1) {
            0 => 1,
            next => next,
        };
        OperationToken { generation: self.generation, tag: PhantomData }
    }

    /// Cancel, replace, or close out a finished operation without starting new work: outstanding
    /// tokens stop being current, so their outcomes are rejected when they finally land. A domain
    /// owner calls this on cancellation, on replacement, **and** when it accepts a terminal outcome
    /// — see [`OperationToken`] for why the last one is not optional.
    pub fn invalidate(&mut self) {
        let _ = self.issue();
    }

    /// Whether `token` identifies the operation this source last issued — the domain owner's accept
    /// test for an incoming outcome. It answers "not superseded", not "not yet answered": an owner
    /// that has accepted the terminal outcome must have called [`invalidate`](Self::invalidate).
    pub fn is_current(&self, token: OperationToken<Tag>) -> bool {
        token.generation == self.generation
    }
}

impl<Tag> Default for TokenSource<Tag> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Tag> core::fmt::Debug for TokenSource<Tag> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "TokenSource({})", self.generation)
    }
}

// ==================== capabilities ====================

/// What this firmware image and its hardware implement **at all** — constant for a boot.
///
/// Producer: the platform executor, once at start-up. Consumer: [`Capabilities::calculate`].
///
/// This is where "the board has no detour planner" and "the web demo has no persistent store" are
/// stated honestly, instead of surfacing later as a routing failure or a save that silently never
/// happens.
///
/// There is deliberately no `route_planning` field: #1433 §7.2 requires the same `obc-route`
/// algorithms on every platform, so route planning rests on live facts (a routing graph, a store to
/// commit into, admission) alone. Detour is the one navigation split that actually exists today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PlatformSupport {
    /// The detour planner and the splice-commit path are present.
    pub detour: bool,
    /// A durable settings store exists (the web demo has none).
    pub settings_persistence: bool,
    /// A firmware-update path exists.
    pub dfu: bool,
    /// Weather data can be requested and installed.
    pub weather: bool,
    /// A radio with a bond store exists.
    pub bonding: bool,
    /// Free space on the storage medium can be measured.
    pub storage_space_report: bool,
    /// A durable place to keep per-object retention metadata exists — the route-use stamp and the
    /// ride-sync stamp.
    ///
    /// False on the board: FS7/FS8 removed the FAT sidecars deliberately and #1398 supplies the
    /// ObjectId-keyed replacement, so a stamp there is mirrored in the resident view and is never
    /// durable. Stating that here is what stops
    /// [`stage_retention`](crate::device_core::pass) emitting a
    /// [`RetentionEffect`](crate::retention::RetentionEffect) nobody can answer: an unanswered write
    /// parks `inflight_write` forever, and answering `…Written` would claim durability that does not
    /// exist. Absence is the third option, and the honest one.
    pub retention_metadata: bool,
}

/// The live facts capabilities depend on — mounted data and heavy-operation admission.
///
/// Producer: DeviceCore, from [`ExternalFacts`], the mounted map/store and `CoreMode`. Consumer:
/// [`Capabilities::calculate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DeviceFacts {
    /// A writable object store is mounted — the precondition for every catalog mutation, ride
    /// recording and route commit.
    pub store_writable: bool,
    /// The mounted map carries a routing graph.
    pub nav_graph: bool,
    /// Weather data is installed on the device.
    pub weather_data: bool,
    /// A companion link is connected — the only weather-refresh source.
    pub link_connected: bool,
    /// A ride is being recorded. Arming an install ends in a reboot, which would lose the live ride
    /// — the shipping refusal in [`DfuInstallError`](crate::dfu::DfuInstallError) and the remote-DFU
    /// door in [`App::open_remote_dfu_check`](crate::App::open_remote_dfu_check).
    ///
    /// `RetentionMachine` also defers its expiry deletes while a ride records (together with the
    /// trusted-clock gate). That stays a domain policy rather than a capability: the device *can*
    /// delete, it simply waits — and a dimmed menu entry would be the wrong way to say so.
    pub ride_recording: bool,
    /// [`CoreMode`](crate::device_core::core_mode::CoreMode)'s verdict on heavy work — a transfer
    /// holding the store, or a planner run holding the nav arm. This field carries the verdict, not
    /// that list: read `CoreMode` for the current conditions rather than re-deriving them here.
    pub heavy_operations: bool,
}

/// Whether the catalog may change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CatalogCapabilities {
    /// Routes, trips and rides can be deleted and re-committed.
    pub mutate: bool,
}

/// Whether a ride can be recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RecorderCapabilities {
    /// A ride can be started and its samples persisted.
    pub record: bool,
}

/// What navigation work this device can start. Absence is a *level*, not a failure: a device
/// without [`plan_detour`](Self::plan_detour) never enters the planning path at all, so the rider
/// is never told "no path" about a route the device never tried to find.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NavigatorCapabilities {
    /// A route can be planned from the mounted map.
    pub plan_route: bool,
    /// A detour around the route ahead can be planned.
    pub plan_detour: bool,
    /// A planned detour can be spliced and committed.
    pub commit_detour: bool,
}

/// Whether settings edits can be made durable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SettingsCapabilities {
    /// A settings revision can be written to durable storage.
    pub persist: bool,
}

/// What the weather domain can offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WeatherCapabilities {
    /// A refresh can be requested.
    pub refresh: bool,
    /// Installed weather data can be shown.
    pub installed_data: bool,
}

/// What the firmware-update domain can offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DfuCapabilities {
    /// A staged image can be scanned and reported.
    pub scan: bool,
    /// An install can be armed.
    pub install: bool,
}

/// Whether the paired phone can be forgotten.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BondCapabilities {
    /// The bond store can be cleared.
    pub remove: bool,
}

/// Whether storage occupancy can be reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StorageInfoCapabilities {
    /// Free space can be measured.
    pub report_free_space: bool,
}

/// Everything this device can currently do, one nested value per domain.
///
/// Producer: DeviceCore, by [`calculate`](Self::calculate) on every pass whose inputs changed.
/// Consumer: `UiRuntime` — a screen hides or dims an operation whose capability is absent, and
/// reads a named field rather than testing a bit number.
///
/// A capability is a *level*, recalculated from the current inputs; it is never latched, and an
/// executor must never report an unsupported operation as [`NoPath`](obc_route::nav::NavError) or
/// any other normal failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub catalog: CatalogCapabilities,
    pub recorder: RecorderCapabilities,
    pub navigator: NavigatorCapabilities,
    pub settings: SettingsCapabilities,
    pub weather: WeatherCapabilities,
    pub dfu: DfuCapabilities,
    pub bond: BondCapabilities,
    pub storage_info: StorageInfoCapabilities,
}

impl Capabilities {
    /// Every capability absent — the boot value, before the first calculation.
    pub const NONE: Capabilities = Capabilities {
        catalog: CatalogCapabilities { mutate: false },
        recorder: RecorderCapabilities { record: false },
        navigator: NavigatorCapabilities { plan_route: false, plan_detour: false, commit_detour: false },
        settings: SettingsCapabilities { persist: false },
        weather: WeatherCapabilities { refresh: false, installed_data: false },
        dfu: DfuCapabilities { scan: false, install: false },
        bond: BondCapabilities { remove: false },
        storage_info: StorageInfoCapabilities { report_free_space: false },
    };

    /// Recalculate every capability from what the platform implements and what is currently true of
    /// the device. Pure: identical inputs give identical output, on every platform.
    ///
    /// The rules, and why each precondition is real:
    ///
    /// - Catalog mutation and ride recording need a writable store.
    /// - Route planning needs a routing graph, a store to commit into, and admission.
    /// - Detour planning needs the detour planner, a graph and admission; committing one needs the
    ///   planner and a writable store (the commit itself is not heavy).
    /// - Weather refresh needs a connected companion; installed weather data needs data installed.
    /// - An install is heavy *and* reboots, so it also needs no ride recording; a scan is neither,
    ///   so it rests on the image alone.
    pub const fn calculate(support: PlatformSupport, facts: DeviceFacts) -> Capabilities {
        Capabilities {
            catalog: CatalogCapabilities { mutate: facts.store_writable },
            recorder: RecorderCapabilities { record: facts.store_writable },
            navigator: NavigatorCapabilities {
                plan_route: facts.nav_graph && facts.store_writable && facts.heavy_operations,
                plan_detour: support.detour && facts.nav_graph && facts.heavy_operations,
                commit_detour: support.detour && facts.store_writable,
            },
            settings: SettingsCapabilities { persist: support.settings_persistence },
            weather: WeatherCapabilities {
                refresh: support.weather && facts.link_connected,
                installed_data: support.weather && facts.weather_data,
            },
            dfu: DfuCapabilities {
                scan: support.dfu,
                install: support.dfu && facts.heavy_operations && !facts.ride_recording,
            },
            bond: BondCapabilities { remove: support.bonding },
            storage_info: StorageInfoCapabilities { report_free_space: support.storage_space_report },
        }
    }
}

impl Default for Capabilities {
    fn default() -> Self {
        Capabilities::NONE
    }
}

// ==================== external facts ====================

/// The opaque identity of a mounted store. DeviceCore compares it; only the executor knows what it
/// names, so no path or storage handle crosses the seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreIdentity(u64);

impl StoreIdentity {
    /// Name a store. The executor mints this from its own mount identity.
    pub const fn new(raw: u64) -> Self {
        StoreIdentity(raw)
    }
}

/// The opaque identity of an installed data set (currently weather products).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DataIdentity(u64);

impl DataIdentity {
    /// Name a data set. The executor mints this from its own product identity.
    pub const fn new(raw: u64) -> Self {
        DataIdentity(raw)
    }
}

/// A monotonic revision of a store or data set — the flat store's `u64` width, so no identity has
/// to be narrowed to reach DeviceCore.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Revision(u64);

impl Revision {
    /// The revision of a store that has committed nothing.
    pub const ZERO: Revision = Revision(0);

    /// Name a revision.
    pub const fn new(raw: u64) -> Self {
        Revision(raw)
    }

    /// The next revision — how a DeviceCore-side generation (a derived view, a locally-known commit)
    /// moves forward. Saturating rather than wrapping: a revision is compared with `>` as well as
    /// `==`, so a wrap would let an ancient value read as newer. 2⁶⁴ commits is not a reachable
    /// device lifetime, and saturating simply stops the counter instead of lying about order.
    pub const fn next(self) -> Revision {
        Revision(self.0.saturating_add(1))
    }
}

/// The mounted store moved. Producer: the store executor on every commit or delete. Consumer:
/// `CatalogMachine`, which decides *when* to refresh — the fact itself never orders one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreRevision {
    /// Which store this revision belongs to; a different identity is a different mount.
    pub store: StoreIdentity,
    /// The store's revision after the commit.
    pub revision: Revision,
}

/// Installed weather data. Producer: the platform weather task after it installs a product.
/// Consumer: `WeatherDomain`, which owns visible freshness and alert policy — never the task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WeatherData {
    /// Which product set is installed.
    pub data: DataIdentity,
    /// Its revision.
    pub revision: Revision,
}

/// Whether a bulk transfer is streaming. Producer: the link and USB control planes. Consumer:
/// [`CoreMode`](crate::device_core::core_mode::CoreMode), which withdraws heavy-operation admission
/// while one is in flight.
///
/// **The known gap, stated rather than papered over:** today the only transfer that reports itself
/// is the **map** upload, through
/// [`App::set_map_transfer`](crate::App::set_map_transfer)'s card level. A route, trip or weather
/// upload streams without one, so `CoreMode`'s transfer level does not see it. That is a gap in the
/// *fact* — the flat engine knows the truth — and #1397 S6 closes it by feeding
/// [`note_transfer`](ExternalFacts::note_transfer) from the engine. A fourth derivation here would
/// be a second copy of the level, which is exactly what S5 deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferState {
    /// No transfer holds the store.
    Idle,
    /// A transfer is streaming.
    Active,
}

/// A route upload committed to the store. Producer: the upload executor, after the catalog already
/// saw the commit. Consumer: `CatalogMachine` (identity remap) and `UiRuntime` (the received card).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteUpload {
    /// The committed route's durable object identity.
    pub id: CatalogObjectId,
    /// The upload replaced the bytes of a stored route.
    pub replaced: bool,
    /// The commit-time mini elevation sparkline, or `None` when the route carries no elevation.
    /// Bounded by construction and already the card's content — it is not a preview polyline.
    pub elevation: Option<[u8; obc_route::SPARKLINE_BUCKETS]>,
}

/// A trip upload committed to the store. Producer and consumers as [`RouteUpload`]; a trip always
/// arrives after its member routes, which is why one slot serves both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TripUpload {
    /// The committed trip's durable object identity.
    pub id: CatalogObjectId,
    /// The upload replaced a stored trip at the same identity.
    pub replaced: bool,
}

/// What this boot has to say about the previous firmware update. Producer: the boot path, exactly
/// once. Consumer: `DfuState`, which turns it into the post-update toast or the failure card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateResult {
    /// A freshly installed image is running; the version is the running one.
    Confirmed(Version),
    /// The update did not take: the typed verdict, plus the staged version when the arm marker
    /// survived.
    Failed { why: DfuFailure, staged: Option<Version> },
}

/// Why a fact could not be merged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactMergeError {
    /// A second boot update result arrived while the first was still unconsumed. There is one boot
    /// per boot: this is a producer bug, and dropping it silently would hide it.
    UpdateResultUnconsumed,
}

/// Everything that changed underneath DeviceCore without anyone asking — never an answer to an
/// effect, so no field carries an [`OperationToken`].
///
/// The same type serves as the executor's per-pass batch and as DeviceCore's accumulator: an
/// executor fills a [`NONE`](Self::NONE) batch through the `note_*` methods, and DeviceCore folds
/// it in with [`merge`](Self::merge), which applies exactly those same per-field rules.
///
/// Field shapes and merge rules:
///
/// | Field | Shape | Merge rule |
/// |---|---|---|
/// | [`store_revision`](Self::store_revision) | latest level | newest identity and revision wins |
/// | [`transfer`](Self::transfer) | latest level | newest state replaces |
/// | [`link`](Self::link) | latest level | newest state replaces |
/// | [`warnings`](Self::take_warnings) | bit set | OR until DeviceCore consumes them |
/// | [`route_upload`](Self::take_route_upload) | bounded latest slot | most recent commit wins |
/// | [`trip_upload`](Self::take_trip_upload) | bounded latest slot | most recent commit wins |
/// | [`update_result`](Self::take_update_result) | one-shot slot | a second unconsumed one is rejected |
/// | [`weather_data`](Self::weather_data) | latest level | newest identity and revision wins |
/// | [`weather_sample`](Self::weather_sample) | latest level | newest revision wins |
/// | [`weather_refreshing`](Self::weather_refreshing) | latest level | newest state replaces |
///
/// Levels are *read* (they describe the world and stay true); the bit set and the slots are
/// *taken* (they describe something that happened once). Consuming one never touches another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalFacts {
    store_revision: Option<StoreRevision>,
    transfer: Option<TransferState>,
    link: Option<BleStatus>,
    weather_data: Option<WeatherData>,
    weather_sample: Option<Revision>,
    weather_refreshing: Option<bool>,
    warnings: WarningFlags,
    route_upload: Option<RouteUpload>,
    trip_upload: Option<TripUpload>,
    update_result: Option<UpdateResult>,
}

impl ExternalFacts {
    /// Nothing reported and nothing pending — the boot accumulator and every executor's empty
    /// batch.
    pub const NONE: ExternalFacts = ExternalFacts {
        store_revision: None,
        transfer: None,
        link: None,
        weather_data: None,
        weather_sample: None,
        weather_refreshing: None,
        warnings: WarningFlags::NONE,
        route_upload: None,
        trip_upload: None,
        update_result: None,
    };

    /// Fold `incoming` in, field by field, under the rules documented on this type. The only
    /// rejection is a second unconsumed [`UpdateResult`]; every other field merges, so a partial
    /// failure cannot lose a warning or an upload.
    pub fn merge(&mut self, incoming: ExternalFacts) -> Result<(), FactMergeError> {
        if let Some(fact) = incoming.store_revision {
            self.note_store_revision(fact);
        }
        if let Some(state) = incoming.transfer {
            self.note_transfer(state);
        }
        if let Some(status) = incoming.link {
            self.note_link(status);
        }
        if let Some(fact) = incoming.weather_data {
            self.note_weather_data(fact);
        }
        if let Some(sample) = incoming.weather_sample {
            self.note_weather_sample(sample);
        }
        if let Some(fetching) = incoming.weather_refreshing {
            self.note_weather_refreshing(fetching);
        }
        self.raise_warnings(incoming.warnings);
        if let Some(upload) = incoming.route_upload {
            self.note_route_upload(upload);
        }
        if let Some(upload) = incoming.trip_upload {
            self.note_trip_upload(upload);
        }
        match incoming.update_result {
            Some(result) => self.note_update_result(result),
            None => Ok(()),
        }
    }

    /// The store moved. Same store: the newer revision wins, so a reordered report cannot walk the
    /// level backwards. Different store: it replaces, because revisions of two different stores have
    /// no order to compare.
    ///
    /// **Producer obligation.** Because a different identity always wins, an executor must not
    /// report a store it has unmounted. A late report from the previous mount would otherwise
    /// overwrite the live one and leave [`store_revision`](Self::store_revision) naming a dead
    /// store. The executor is the only party that can honour this — it is the one that knows the
    /// unmount happened — so it drains or drops its pending store reports before it reports the new
    /// mount.
    pub fn note_store_revision(&mut self, fact: StoreRevision) {
        let keep =
            matches!(self.store_revision, Some(have) if have.store == fact.store && have.revision > fact.revision);
        if !keep {
            self.store_revision = Some(fact);
        }
    }

    /// Weather data was installed. Same identity/newer revision rule as
    /// [`note_store_revision`](Self::note_store_revision).
    pub fn note_weather_data(&mut self, fact: WeatherData) {
        let keep = matches!(self.weather_data, Some(have) if have.data == fact.data && have.revision > fact.revision);
        if !keep {
            self.weather_data = Some(fact);
        }
    }

    /// The host resampled the weather snapshot at this revision — the repaint edge for the screens
    /// that draw it. A monotone level, so a reordered report cannot walk it backwards.
    ///
    /// **Why it is not folded into [`weather_data`](Self::weather_data).** A resample at a new rider
    /// position, or a new minute of the route projection, changes what the card says under an
    /// entirely unchanged installed revision. One counter cannot carry both.
    pub fn note_weather_sample(&mut self, sample: Revision) {
        if self.weather_sample.is_none_or(|have| sample > have) {
            self.weather_sample = Some(sample);
        }
    }

    /// The provider plane started or stopped fetching; the newest report is the truth. Reported as
    /// a level and not as an operation's answer, because its own cadence raises fetches nobody
    /// ordered — and the rider is owed the UPDATING cue for those too.
    pub fn note_weather_refreshing(&mut self, fetching: bool) {
        self.weather_refreshing = Some(fetching);
    }

    /// The transfer state changed; the newest report is the truth.
    pub fn note_transfer(&mut self, state: TransferState) {
        self.transfer = Some(state);
    }

    /// The link state changed; the newest report is the truth.
    pub fn note_link(&mut self, status: BleStatus) {
        self.link = Some(status);
    }

    /// Raise warning flags. They accumulate — a second warning in the same pass never displaces the
    /// first, and nothing clears until DeviceCore takes them.
    pub fn raise_warnings(&mut self, flags: WarningFlags) {
        self.warnings |= flags;
    }

    /// A route upload committed; the most recent commit is the one worth announcing.
    pub fn note_route_upload(&mut self, upload: RouteUpload) {
        self.route_upload = Some(upload);
    }

    /// A trip upload committed; the most recent commit is the one worth announcing.
    pub fn note_trip_upload(&mut self, upload: TripUpload) {
        self.trip_upload = Some(upload);
    }

    /// Report this boot's update result. Fails with
    /// [`UpdateResultUnconsumed`](FactMergeError::UpdateResultUnconsumed) when one is still
    /// pending: overwriting it would drop the verdict the rider is owed.
    pub fn note_update_result(&mut self, result: UpdateResult) -> Result<(), FactMergeError> {
        if self.update_result.is_some() {
            return Err(FactMergeError::UpdateResultUnconsumed);
        }
        self.update_result = Some(result);
        Ok(())
    }

    /// The newest store identity and revision, or `None` when no store has reported yet.
    pub fn store_revision(&self) -> Option<StoreRevision> {
        self.store_revision
    }

    /// The newest transfer state, or `None` when none was reported yet.
    pub fn transfer(&self) -> Option<TransferState> {
        self.transfer
    }

    /// The newest link state, or `None` when none was reported yet.
    pub fn link(&self) -> Option<BleStatus> {
        self.link
    }

    /// The installed weather data, or `None` when none is installed.
    pub fn weather_data(&self) -> Option<WeatherData> {
        self.weather_data
    }

    /// The newest weather-sample revision, or `None` when the host has not resampled yet.
    pub fn weather_sample(&self) -> Option<Revision> {
        self.weather_sample
    }

    /// Whether the provider plane is fetching, or `None` when it has not reported yet.
    pub fn weather_refreshing(&self) -> Option<bool> {
        self.weather_refreshing
    }

    /// Take the accumulated warning flags, clearing them.
    pub fn take_warnings(&mut self) -> WarningFlags {
        core::mem::replace(&mut self.warnings, WarningFlags::NONE)
    }

    /// Take the pending route upload, clearing the slot.
    pub fn take_route_upload(&mut self) -> Option<RouteUpload> {
        self.route_upload.take()
    }

    /// Take the pending trip upload, clearing the slot.
    pub fn take_trip_upload(&mut self) -> Option<TripUpload> {
        self.trip_upload.take()
    }

    /// Take this boot's update result, clearing the one-shot slot.
    pub fn take_update_result(&mut self) -> Option<UpdateResult> {
        self.update_result.take()
    }
}

impl Default for ExternalFacts {
    fn default() -> Self {
        ExternalFacts::NONE
    }
}

// ==================== layout tripwires ====================
//
// Sizes are 64-bit host ceilings (the device's 32-bit `usize` makes the `Version`-carrying values
// smaller, never larger). A growth here is a design change, not an accident: every value below
// crosses the DeviceCore ↔ executor seam and, for `ExternalFacts`, stays resident.
const _: () = assert!(core::mem::size_of::<OperationToken<CatalogTag>>() == 4, "a token is one generation");
const _: () = assert!(core::mem::size_of::<TokenSource<CatalogTag>>() == 4, "a token source is one generation");
const _: () = assert!(core::mem::size_of::<PlatformSupport>() <= 8, "platform support is a handful of bools");
const _: () = assert!(core::mem::size_of::<DeviceFacts>() <= 8, "device facts are a handful of bools");
const _: () = assert!(core::mem::size_of::<Capabilities>() <= 16, "capabilities are bools, never payloads");
const _: () = assert!(core::mem::size_of::<StoreIdentity>() <= 8, "an opaque identity, nothing more");
const _: () = assert!(core::mem::size_of::<DataIdentity>() <= 8, "an opaque identity, nothing more");
const _: () = assert!(core::mem::size_of::<Revision>() <= 8, "the flat store's revision width");
const _: () = assert!(core::mem::size_of::<StoreRevision>() <= 16, "an identity and a revision");
const _: () = assert!(core::mem::size_of::<WeatherData>() <= 16, "an identity and a revision");
const _: () = assert!(core::mem::size_of::<FactMergeError>() <= 1, "a fieldless reason");
const _: () = assert!(core::mem::size_of::<TransferState>() <= 1, "a two-state level");
const _: () = assert!(core::mem::size_of::<RouteUpload>() <= 80, "id + flag + the fixed sparkline");
const _: () = assert!(core::mem::size_of::<TripUpload>() <= 16, "id + flag");
const _: () = assert!(core::mem::size_of::<UpdateResult>() <= 56, "two fixed version strings at most");
const _: () = assert!(core::mem::size_of::<ExternalFacts>() <= 240, "the fact slots stay pocket-sized");

#[cfg(test)]
mod tests {
    use super::*;

    fn store(revision: u64) -> StoreRevision {
        StoreRevision { store: StoreIdentity::new(1), revision: Revision::new(revision) }
    }

    /// The whole point of a token: only the newest operation's result is accepted. Issuing,
    /// cancelling and replacing all move the generation, so every earlier token goes stale.
    #[test]
    fn tokens_go_stale_on_issue_cancel_and_replacement() {
        let mut nav: TokenSource<NavigatorTag> = TokenSource::new();

        let planning = nav.issue();
        assert!(nav.is_current(planning));

        nav.invalidate(); // cancellation
        assert!(!nav.is_current(planning), "a cancelled operation's outcome is rejected");

        let replanning = nav.issue(); // replacement
        assert!(nav.is_current(replanning));
        assert_ne!(planning, replanning);

        let again = nav.issue();
        assert!(!nav.is_current(replanning), "the replaced token is no longer current");
        assert!(nav.is_current(again));
    }

    /// Wrapping is fine, but generation `0` means "nothing issued" — a wrapped token must never
    /// collide with the boot state of a fresh source.
    #[test]
    fn generation_skips_zero_on_wrap() {
        let mut source: TokenSource<CatalogTag> = TokenSource::new();
        source.generation = u32::MAX - 1;

        let before = source.issue(); // the last generation before the wrap
        assert_eq!(source.generation, u32::MAX);
        let wrapped = source.issue();
        assert_eq!(source.generation, 1);
        assert_ne!(before, wrapped, "the wrap still moves the generation");
        assert!(!source.is_current(before), "the pre-wrap token is stale like any other");

        let fresh: TokenSource<CatalogTag> = TokenSource::new();
        assert!(!fresh.is_current(wrapped), "a wrapped token cannot pass as a never-issued one");
    }

    /// Warnings are the one accumulating field: two producers in the same pass both survive.
    #[test]
    fn warnings_accumulate_without_loss() {
        let mut facts = ExternalFacts::NONE;
        facts.raise_warnings(WarningFlags::NO_GPS);

        let mut batch = ExternalFacts::NONE;
        batch.raise_warnings(WarningFlags::MAP_SLOW);
        facts.merge(batch).unwrap();
        facts.raise_warnings(WarningFlags::REC_ERROR);

        let taken = facts.take_warnings();
        assert!(taken.contains(WarningFlags::NO_GPS));
        assert!(taken.contains(WarningFlags::MAP_SLOW));
        assert!(taken.contains(WarningFlags::REC_ERROR));
        assert!(facts.take_warnings().is_empty(), "taking clears the set");
    }

    /// Levels describe the world: the newest report wins, except that a store cannot walk its own
    /// revision backwards. A remount (a different identity) always replaces.
    #[test]
    fn latest_levels_replace_older_values() {
        let mut facts = ExternalFacts::NONE;

        facts.note_store_revision(store(7));
        facts.note_store_revision(store(4));
        assert_eq!(facts.store_revision().unwrap().revision, Revision::new(7), "a stale revision cannot win");

        let other = StoreRevision { store: StoreIdentity::new(2), revision: Revision::ZERO };
        facts.note_store_revision(other);
        assert_eq!(facts.store_revision(), Some(other), "a different store is a remount, not a stale report");

        facts.note_transfer(TransferState::Active);
        facts.note_transfer(TransferState::Idle);
        assert_eq!(facts.transfer(), Some(TransferState::Idle));

        facts.note_link(BleStatus::DISCONNECTED);
        let connected = BleStatus { link: crate::ble::BleLink::Connected, ..BleStatus::DISCONNECTED };
        facts.note_link(connected);
        assert_eq!(facts.link(), Some(connected));

        let installed = WeatherData { data: DataIdentity::new(9), revision: Revision::new(3) };
        facts.note_weather_data(installed);
        facts.note_weather_data(WeatherData { revision: Revision::new(2), ..installed });
        assert_eq!(facts.weather_data(), Some(installed));
    }

    /// There is one boot per boot. A second unconsumed result is a producer bug and says so.
    #[test]
    fn second_unconsumed_update_result_is_rejected() {
        let mut facts = ExternalFacts::NONE;
        let confirmed = UpdateResult::Confirmed(crate::dfu::clamp("v1.2.3"));
        facts.note_update_result(confirmed.clone()).unwrap();

        let second = UpdateResult::Failed { why: DfuFailure::Reverted, staged: None };
        assert_eq!(facts.note_update_result(second.clone()), Err(FactMergeError::UpdateResultUnconsumed));
        assert_eq!(facts.take_update_result(), Some(confirmed), "the rejected report never displaced the first");

        facts.note_update_result(second.clone()).unwrap();
        assert_eq!(facts.take_update_result(), Some(second), "the slot is free once consumed");
    }

    /// A batch merges field by field, under exactly the rules the `note_*` methods apply — and an
    /// absent field in the batch changes nothing, so an executor reporting one fact cannot wipe a
    /// level it knows nothing about.
    #[test]
    fn merge_folds_every_field_and_leaves_absent_ones_alone() {
        let installed = WeatherData { data: DataIdentity::new(4), revision: Revision::new(2) };
        let mut facts = ExternalFacts::NONE;
        facts.note_store_revision(store(5));
        facts.note_transfer(TransferState::Idle);
        facts.note_link(BleStatus::DISCONNECTED);
        facts.note_weather_data(installed);
        facts.raise_warnings(WarningFlags::NO_GPS);

        let connected = BleStatus { link: crate::ble::BleLink::Connected, ..BleStatus::DISCONNECTED };
        let route = RouteUpload { id: 21, replaced: false, elevation: None };
        let trip = TripUpload { id: 22, replaced: false };
        let mut batch = ExternalFacts::NONE;
        batch.note_store_revision(store(9));
        batch.note_transfer(TransferState::Active);
        batch.note_link(connected);
        batch.note_weather_data(WeatherData { revision: Revision::new(7), ..installed });
        batch.raise_warnings(WarningFlags::MAP_SLOW);
        batch.note_route_upload(route);
        batch.note_trip_upload(trip);
        batch.note_update_result(UpdateResult::Confirmed(crate::dfu::clamp("v2"))).unwrap();

        facts.merge(batch).unwrap();

        assert_eq!(facts.store_revision(), Some(store(9)));
        assert_eq!(facts.transfer(), Some(TransferState::Active));
        assert_eq!(facts.link(), Some(connected));
        assert_eq!(facts.weather_data().unwrap().revision, Revision::new(7));
        assert_eq!(facts.take_route_upload(), Some(route));
        assert_eq!(facts.take_trip_upload(), Some(trip));
        assert!(facts.take_update_result().is_some());
        let warnings = facts.take_warnings();
        assert!(warnings.contains(WarningFlags::NO_GPS) && warnings.contains(WarningFlags::MAP_SLOW));

        // A stale batch loses on the level fields and cannot clear the ones it omits.
        facts.note_store_revision(store(9));
        let mut stale = ExternalFacts::NONE;
        stale.note_store_revision(store(6));
        facts.merge(stale).unwrap();
        assert_eq!(facts.store_revision(), Some(store(9)), "merge applies the same stale-revision rule");
        assert_eq!(facts.transfer(), Some(TransferState::Active), "an absent field in the batch changes nothing");
        assert_eq!(facts.link(), Some(connected));
    }

    /// The documented partial-failure guarantee: a batch whose update result is rejected still
    /// delivers every other fact, so the rejection cannot cost a warning or an upload.
    #[test]
    fn a_rejected_update_result_does_not_lose_the_rest_of_the_batch() {
        let first = UpdateResult::Confirmed(crate::dfu::clamp("v1"));
        let mut facts = ExternalFacts::NONE;
        facts.note_update_result(first.clone()).unwrap();

        let route = RouteUpload { id: 31, replaced: true, elevation: None };
        let mut batch = ExternalFacts::NONE;
        batch.raise_warnings(WarningFlags::REC_ERROR);
        batch.note_route_upload(route);
        batch.note_store_revision(store(3));
        batch.note_update_result(UpdateResult::Failed { why: DfuFailure::NotStarted, staged: None }).unwrap();

        assert_eq!(facts.merge(batch), Err(FactMergeError::UpdateResultUnconsumed));

        assert!(facts.take_warnings().contains(WarningFlags::REC_ERROR), "the warning survived the rejection");
        assert_eq!(facts.take_route_upload(), Some(route), "the upload survived the rejection");
        assert_eq!(facts.store_revision(), Some(store(3)));
        assert_eq!(facts.take_update_result(), Some(first), "the unconsumed result is still the one held");
    }

    /// Each field is consumed on its own: a pass that takes the uploads must not lose the warnings
    /// or the levels it did not look at.
    #[test]
    fn consuming_one_fact_leaves_the_others() {
        let mut facts = ExternalFacts::NONE;
        facts.note_store_revision(store(1));
        facts.raise_warnings(WarningFlags::NO_COMPASS);
        facts.note_route_upload(RouteUpload { id: 11, replaced: false, elevation: None });
        facts.note_trip_upload(TripUpload { id: 12, replaced: true });
        facts.note_update_result(UpdateResult::Confirmed(crate::dfu::clamp("v9"))).unwrap();

        assert_eq!(facts.take_route_upload(), Some(RouteUpload { id: 11, replaced: false, elevation: None }));
        assert!(facts.take_route_upload().is_none(), "the slot is one-shot");

        assert_eq!(facts.take_trip_upload(), Some(TripUpload { id: 12, replaced: true }));
        assert_eq!(facts.store_revision(), Some(store(1)));
        assert!(facts.take_warnings().contains(WarningFlags::NO_COMPASS));
        assert!(facts.take_update_result().is_some());
    }

    fn board() -> PlatformSupport {
        PlatformSupport {
            detour: true,
            settings_persistence: true,
            dfu: true,
            weather: true,
            bonding: true,
            storage_space_report: true,
            retention_metadata: true,
        }
    }

    fn mounted() -> DeviceFacts {
        DeviceFacts {
            store_writable: true,
            nav_graph: true,
            weather_data: true,
            link_connected: true,
            ride_recording: false,
            heavy_operations: true,
        }
    }

    /// Capabilities are a pure level of their inputs — every field, written out, so a rule flipped
    /// in `calculate` fails here rather than passing a "same inputs, same output" tautology.
    #[test]
    fn capabilities_are_the_written_out_level_of_their_inputs() {
        assert_eq!(
            Capabilities::calculate(board(), mounted()),
            Capabilities {
                catalog: CatalogCapabilities { mutate: true },
                recorder: RecorderCapabilities { record: true },
                navigator: NavigatorCapabilities { plan_route: true, plan_detour: true, commit_detour: true },
                settings: SettingsCapabilities { persist: true },
                weather: WeatherCapabilities { refresh: true, installed_data: true },
                dfu: DfuCapabilities { scan: true, install: true },
                bond: BondCapabilities { remove: true },
                storage_info: StorageInfoCapabilities { report_free_space: true },
            }
        );

        // The web demo: no durable store, no radio, no update path, no weather.
        assert_eq!(
            Capabilities::calculate(PlatformSupport { detour: true, ..PlatformSupport::default() }, mounted()),
            Capabilities {
                catalog: CatalogCapabilities { mutate: true },
                recorder: RecorderCapabilities { record: true },
                navigator: NavigatorCapabilities { plan_route: true, plan_detour: true, commit_detour: true },
                ..Capabilities::NONE
            }
        );
    }

    /// Nothing latches: a changed live fact gives the answer that fact implies, and giving it back
    /// restores the previous level exactly.
    #[test]
    fn capabilities_recalculate_from_mounted_data_and_transfer_state() {
        let full = Capabilities::calculate(board(), mounted());

        // A map without a routing graph mounts: planning goes, the rest stays.
        let graphless = Capabilities::calculate(board(), DeviceFacts { nav_graph: false, ..mounted() });
        assert!(!graphless.navigator.plan_route && !graphless.navigator.plan_detour);
        assert!(graphless.navigator.commit_detour, "committing a planned detour needs no graph read");
        assert!(graphless.catalog.mutate && graphless.recorder.record);

        // A transfer starts: CoreMode withdraws heavy admission.
        let transferring = Capabilities::calculate(board(), DeviceFacts { heavy_operations: false, ..mounted() });
        assert!(!transferring.navigator.plan_route && !transferring.navigator.plan_detour);
        assert!(!transferring.dfu.install, "an install reboots — never mid-transfer");
        assert!(transferring.dfu.scan, "a scan is not heavy");
        assert!(transferring.catalog.mutate);

        // A ride records: the install that would reboot away the live ride is gone, planning is not.
        let riding = Capabilities::calculate(board(), DeviceFacts { ride_recording: true, ..mounted() });
        assert!(!riding.dfu.install, "arming an install mid-ride would lose the ride");
        assert!(riding.dfu.scan && riding.navigator.plan_route && riding.navigator.plan_detour);

        // Each fact comes back: the level does too, with no residue from the interim states.
        assert_eq!(Capabilities::calculate(board(), mounted()), full);
    }

    /// An unsupported detour is a missing capability, not a planning failure: no combination of
    /// live facts can turn it on, so the UI hides it and the rider is never told "no path" about a
    /// search the device cannot run.
    #[test]
    fn unsupported_detour_never_enters_the_planning_path() {
        let support = PlatformSupport { detour: false, ..board() };
        for bits in 0u8..64 {
            let facts = DeviceFacts {
                store_writable: bits & 1 != 0,
                nav_graph: bits & 2 != 0,
                weather_data: bits & 4 != 0,
                link_connected: bits & 8 != 0,
                ride_recording: bits & 16 != 0,
                heavy_operations: bits & 32 != 0,
            };
            let caps = Capabilities::calculate(support, facts);
            assert!(!caps.navigator.plan_detour, "no live fact can supply a planner the image lacks");
            assert!(!caps.navigator.commit_detour);
        }
        assert_eq!(Capabilities::NONE.navigator, NavigatorCapabilities::default());
    }
}
