//! The shared DeviceCore vocabulary: operation tokens, capabilities, and external facts. Nothing
//! here performs work.
//!
//! Every type here is bounded and free of platform handles, paths and unbounded collections, so a
//! value in this module can cross the DeviceCore ↔ executor seam on any platform.

use core::marker::PhantomData;

use crate::ble::BleStatus;
use crate::dfu::{DfuFailure, Version};
use crate::Alerts;
use crate::CatalogObjectId;

// The domain tags an `OperationToken` is typed by, one per domain that owns asynchronous work.
// They are uninhabited: a tag is a name at the type level, never a value.

pub enum CatalogTag {}
/// Assistant checkpoint writes.
pub enum MetadataTag {}
pub enum RecorderTag {}
pub enum NavigatorTag {}
pub enum SettingsTag {}

pub enum DfuTag {}
pub enum BondTag {}
pub enum StorageInfoTag {}

/// The identity of one in-flight operation, typed by its owning domain.
///
/// Every effect carries the token its domain issued, and every outcome carries it back. The domain
/// owner accepts an outcome only while [`TokenSource::is_current`] holds, which is how a superseded
/// result is rejected without the executor knowing any product rule.
///
/// Equality alone does not reject a duplicate or post-terminal outcome, because the generation
/// keeps standing after the operation ends. The owner must call [`TokenSource::invalidate`] when it
/// accepts a terminal outcome, exactly as cancellation and replacement do.
///
/// The generation is private and never zero, so a token cannot be forged or confused with "no
/// operation". `PhantomData<fn() -> Tag>` makes a token of one domain unusable as a token of
/// another:
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

/// The single minter of one domain's [`OperationToken`]s. A domain state machine owns exactly one,
/// and no executor ever holds it. Generation `0` is the boot state "nothing was ever issued", so
/// [`issue`](Self::issue) skips back to `1` on wrap; only the latest generation is ever current, so
/// the wrap cannot resurrect a stale token.
pub struct TokenSource<Tag> {
    generation: u32,
    tag: PhantomData<fn() -> Tag>,
}

impl<Tag> TokenSource<Tag> {
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
    /// tokens stop being current, so their outcomes are rejected when they land. A domain owner
    /// calls this on cancellation, on replacement, and when it accepts a terminal outcome.
    pub fn invalidate(&mut self) {
        let _ = self.issue();
    }

    /// Whether `token` identifies the operation this source last issued. It answers "not
    /// superseded", not "not yet answered".
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

/// What this firmware image and its hardware implement at all. Constant for a boot: the platform
/// executor reports it once at start-up and [`Capabilities::calculate`] consumes it.
///
/// There is deliberately no `route_planning` field. Every platform runs the same `obc-route`
/// algorithms, so route planning rests on live facts alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PlatformSupport {
    /// The detour planner and the splice-commit path are present.
    pub detour: bool,
    pub settings_persistence: bool,
    pub dfu: bool,

    pub bonding: bool,
    pub storage_space_report: bool,
}

/// The live facts capabilities depend on: mounted data and heavy-operation admission. DeviceCore
/// produces them for [`Capabilities::calculate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DeviceFacts {
    /// A writable object store is mounted: the precondition for every catalog mutation, ride
    /// recording and route commit. What it reads is "a store has reported a revision", and there is
    /// no unmount fact to make the level fall again.
    pub store_writable: bool,
    pub nav_graph: bool,

    pub link_connected: bool,
    pub ride_recording: bool,
    /// [`CoreMode`](crate::device_core::core_mode::CoreMode)'s verdict on heavy work. This field
    /// carries the verdict, not the conditions behind it: read `CoreMode` for those.
    pub heavy_operations: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CatalogCapabilities {
    /// Routes, trips and rides can be deleted and re-committed.
    pub mutate: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RecorderCapabilities {
    pub record: bool,
}

/// What navigation work this device can start. Absence is a level, not a failure: a device without
/// [`plan_detour`](Self::plan_detour) never enters the planning path, so the rider is never told
/// "no path" about a route the device never tried to find.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NavigatorCapabilities {
    pub plan_route: bool,
    pub plan_detour: bool,
    pub commit_detour: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SettingsCapabilities {
    pub persist: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DfuCapabilities {
    pub scan: bool,
    pub install: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BondCapabilities {
    pub remove: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StorageInfoCapabilities {
    pub report_free_space: bool,
}

/// Everything this device can currently do, one nested value per domain. A capability is a level,
/// recalculated from the current inputs and never latched: an executor must never report an
/// unsupported operation as [`NoPath`](obc_route::nav::NavError) or any other normal failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub catalog: CatalogCapabilities,
    pub recorder: RecorderCapabilities,
    pub navigator: NavigatorCapabilities,
    pub settings: SettingsCapabilities,

    pub dfu: DfuCapabilities,
    pub bond: BondCapabilities,
    pub storage_info: StorageInfoCapabilities,
}

impl Capabilities {
    pub const NONE: Capabilities = Capabilities {
        catalog: CatalogCapabilities { mutate: false },
        recorder: RecorderCapabilities { record: false },
        navigator: NavigatorCapabilities { plan_route: false, plan_detour: false, commit_detour: false },
        settings: SettingsCapabilities { persist: false },

        dfu: DfuCapabilities { scan: false, install: false },
        bond: BondCapabilities { remove: false },
        storage_info: StorageInfoCapabilities { report_free_space: false },
    };

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

/// The opaque identity of a mounted store. DeviceCore compares it; only the executor knows what it
/// names, so no path or storage handle crosses the seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreIdentity([u8; 16]);

impl StoreIdentity {
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }
    pub const fn bytes(self) -> [u8; 16] {
        self.0
    }

    /// Name a store. The executor mints this from its own mount identity.
    pub const fn new(raw: u64) -> Self {
        let bytes = raw.to_le_bytes();
        StoreIdentity([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7], 0, 0, 0, 0, 0, 0, 0, 0,
        ])
    }
}

/// A monotonic revision of a store or data set, at the flat store's `u64` width, so no identity has
/// to be narrowed to reach DeviceCore.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Revision(u64);

impl Revision {
    pub const fn raw(self) -> u64 {
        self.0
    }

    pub const ZERO: Revision = Revision(0);

    pub const fn new(raw: u64) -> Self {
        Revision(raw)
    }

    /// The next revision. Saturating rather than wrapping: a revision is compared with `>` as well
    /// as `==`, so a wrap would let an ancient value read as newer.
    pub const fn next(self) -> Revision {
        Revision(self.0.saturating_add(1))
    }
}

/// The mounted store moved. `CatalogMachine` decides when to refresh; the fact itself never orders
/// one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreRevision {
    /// Which store this revision belongs to; a different identity is a different mount.
    pub store: StoreIdentity,
    pub revision: Revision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferState {
    Idle,
    Active,
}

/// A route upload committed to the store, after the catalog already saw the commit. `CatalogMachine`
/// remaps the identity and `UiRuntime` raises the received card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteUpload {
    pub id: CatalogObjectId,
    /// The upload replaced the bytes of a stored route.
    pub replaced: bool,
    /// The commit-time mini elevation sparkline, or `None` when the route carries no elevation.
    pub elevation: Option<[u8; obc_route::SPARKLINE_BUCKETS]>,
}

/// A trip upload committed to the store. A trip always arrives after its member routes, which is
/// why one slot serves both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TripUpload {
    pub id: CatalogObjectId,
    pub replaced: bool,
}

/// What this boot has to say about the previous firmware update. Reported exactly once, and
/// `DfuState` turns it into the post-update toast or the failure card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateResult {
    /// A freshly installed image is running; the version is the running one.
    Confirmed(Version),
    /// The update did not take: the typed verdict, plus the staged version when the arm marker
    /// survived.
    Failed { why: DfuFailure, staged: Option<Version> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactMergeError {
    /// A second boot update result arrived while the first was still unconsumed. There is one boot
    /// per boot: this is a producer bug, and dropping it silently would hide it.
    UpdateResultUnconsumed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalFacts {
    store_revision: Option<StoreRevision>,
    transfer: Option<TransferState>,
    link: Option<BleStatus>,

    alerts: Alerts,
    route_upload: Option<RouteUpload>,
    trip_upload: Option<TripUpload>,
    update_result: Option<UpdateResult>,
}

impl ExternalFacts {
    pub const NONE: ExternalFacts = ExternalFacts {
        store_revision: None,
        transfer: None,
        link: None,

        alerts: Alerts::NONE,
        route_upload: None,
        trip_upload: None,
        update_result: None,
    };

    /// Fold `incoming` in, field by field, under the rules documented on this type. The only
    /// rejection is a second unconsumed [`UpdateResult`], so a partial failure cannot lose an alert
    /// or an upload.
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

        self.raise_alerts(incoming.alerts);
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
    /// Because a different identity always wins, an executor must drain or drop the pending reports
    /// of a mount it has unmounted. A late one would otherwise name a dead store.
    pub fn note_store_revision(&mut self, fact: StoreRevision) {
        let keep =
            matches!(self.store_revision, Some(have) if have.store == fact.store && have.revision > fact.revision);
        if !keep {
            self.store_revision = Some(fact);
        }
    }

    /// The transfer state changed; the newest report is the truth.
    pub fn note_transfer(&mut self, state: TransferState) {
        self.transfer = Some(state);
    }

    /// The link state changed; the newest report is the truth.
    pub fn note_link(&mut self, status: BleStatus) {
        self.link = Some(status);
    }

    /// Raise alerts. They accumulate: nothing clears until DeviceCore takes them.
    pub fn raise_alerts(&mut self, alerts: impl Into<Alerts>) {
        self.alerts.raise(alerts);
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

    pub fn store_revision(&self) -> Option<StoreRevision> {
        self.store_revision
    }

    pub fn transfer(&self) -> Option<TransferState> {
        self.transfer
    }

    pub fn link(&self) -> Option<BleStatus> {
        self.link
    }

    pub fn take_alerts(&mut self) -> Alerts {
        self.alerts.take()
    }

    pub fn take_route_upload(&mut self) -> Option<RouteUpload> {
        self.route_upload.take()
    }

    pub fn take_trip_upload(&mut self) -> Option<TripUpload> {
        self.trip_upload.take()
    }

    pub fn take_update_result(&mut self) -> Option<UpdateResult> {
        self.update_result.take()
    }
}

impl Default for ExternalFacts {
    fn default() -> Self {
        ExternalFacts::NONE
    }
}

// Layout tripwires. Sizes are 64-bit host ceilings (the device's 32-bit `usize` makes the
// `Version`-carrying values smaller, never larger). A growth here is a design change: every value
// below crosses the DeviceCore ↔ executor seam and, for `ExternalFacts`, stays resident.
const _: () = assert!(core::mem::size_of::<OperationToken<CatalogTag>>() == 4, "a token is one generation");
const _: () = assert!(core::mem::size_of::<TokenSource<CatalogTag>>() == 4, "a token source is one generation");
const _: () = assert!(core::mem::size_of::<PlatformSupport>() <= 8, "platform support is a handful of bools");
const _: () = assert!(core::mem::size_of::<DeviceFacts>() <= 8, "device facts are a handful of bools");
const _: () = assert!(core::mem::size_of::<Capabilities>() <= 16, "capabilities are bools, never payloads");
const _: () = assert!(core::mem::size_of::<StoreIdentity>() <= 16, "an opaque identity, nothing more");

const _: () = assert!(core::mem::size_of::<Revision>() <= 8, "the flat store's revision width");
const _: () = assert!(core::mem::size_of::<StoreRevision>() <= 24, "an identity and a revision");

const _: () = assert!(core::mem::size_of::<FactMergeError>() <= 1, "a fieldless reason");
const _: () = assert!(core::mem::size_of::<TransferState>() <= 1, "a two-state level");
const _: () = assert!(core::mem::size_of::<RouteUpload>() <= 80, "id + flag + the fixed sparkline");
const _: () = assert!(core::mem::size_of::<TripUpload>() <= 16, "id + flag");
const _: () = assert!(core::mem::size_of::<UpdateResult>() <= 56, "two fixed version strings at most");
const _: () = assert!(core::mem::size_of::<ExternalFacts>() <= 240, "the fact slots stay pocket-sized");

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Alert;

    fn store(revision: u64) -> StoreRevision {
        StoreRevision { store: StoreIdentity::new(1), revision: Revision::new(revision) }
    }

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

    /// Generation `0` means "nothing issued", so a wrapped token must never collide with the boot
    /// state of a fresh source.
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

    /// Alerts are the one accumulating field: two producers in the same pass both survive.
    #[test]
    fn warnings_accumulate_without_loss() {
        let mut facts = ExternalFacts::NONE;
        facts.raise_alerts(Alert::NoGps);

        let mut batch = ExternalFacts::NONE;
        batch.raise_alerts(Alert::StorageLost);
        facts.merge(batch).unwrap();
        facts.raise_alerts(Alert::RecordingFailed);

        let taken = facts.take_alerts();
        assert!(taken.contains(Alert::NoGps));
        assert!(taken.contains(Alert::StorageLost));
        assert!(taken.contains(Alert::RecordingFailed));
        assert!(facts.take_alerts().is_empty(), "taking clears the set");
    }

    /// The newest report wins, except that a store cannot walk its own revision backwards. A
    /// remount is a different identity, and always replaces.
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

    /// A batch merges field by field under the rules the `note_*` methods apply. An absent field in
    /// the batch changes nothing, so an executor reporting one fact cannot wipe a level it knows
    /// nothing about.
    #[test]
    fn merge_folds_every_field_and_leaves_absent_ones_alone() {
        let mut facts = ExternalFacts::NONE;
        facts.note_store_revision(store(5));
        facts.note_transfer(TransferState::Idle);
        facts.note_link(BleStatus::DISCONNECTED);

        facts.raise_alerts(Alert::NoGps);

        let connected = BleStatus { link: crate::ble::BleLink::Connected, ..BleStatus::DISCONNECTED };
        let route = RouteUpload { id: 21, replaced: false, elevation: None };
        let trip = TripUpload { id: 22, replaced: false };
        let mut batch = ExternalFacts::NONE;
        batch.note_store_revision(store(9));
        batch.note_transfer(TransferState::Active);
        batch.note_link(connected);

        batch.raise_alerts(Alert::StorageLost);
        batch.note_route_upload(route);
        batch.note_trip_upload(trip);
        batch.note_update_result(UpdateResult::Confirmed(crate::dfu::clamp("v2"))).unwrap();

        facts.merge(batch).unwrap();

        assert_eq!(facts.store_revision(), Some(store(9)));
        assert_eq!(facts.transfer(), Some(TransferState::Active));
        assert_eq!(facts.link(), Some(connected));

        assert_eq!(facts.take_route_upload(), Some(route));
        assert_eq!(facts.take_trip_upload(), Some(trip));
        assert!(facts.take_update_result().is_some());
        let warnings = facts.take_alerts();
        assert!(warnings.contains(Alert::NoGps) && warnings.contains(Alert::StorageLost));

        // A stale batch loses on the level fields and cannot clear the ones it omits.
        facts.note_store_revision(store(9));
        let mut stale = ExternalFacts::NONE;
        stale.note_store_revision(store(6));
        facts.merge(stale).unwrap();
        assert_eq!(facts.store_revision(), Some(store(9)), "merge applies the same stale-revision rule");
        assert_eq!(facts.transfer(), Some(TransferState::Active), "an absent field in the batch changes nothing");
        assert_eq!(facts.link(), Some(connected));
    }

    /// A batch whose update result is rejected still delivers every other fact, so the rejection
    /// cannot cost a warning or an upload.
    #[test]
    fn a_rejected_update_result_does_not_lose_the_rest_of_the_batch() {
        let first = UpdateResult::Confirmed(crate::dfu::clamp("v1"));
        let mut facts = ExternalFacts::NONE;
        facts.note_update_result(first.clone()).unwrap();

        let route = RouteUpload { id: 31, replaced: true, elevation: None };
        let mut batch = ExternalFacts::NONE;
        batch.raise_alerts(Alert::RecordingFailed);
        batch.note_route_upload(route);
        batch.note_store_revision(store(3));
        batch.note_update_result(UpdateResult::Failed { why: DfuFailure::NotStarted, staged: None }).unwrap();

        assert_eq!(facts.merge(batch), Err(FactMergeError::UpdateResultUnconsumed));

        assert!(facts.take_alerts().contains(Alert::RecordingFailed), "the warning survived the rejection");
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
        facts.raise_alerts(Alert::NoCompass);
        facts.note_route_upload(RouteUpload { id: 11, replaced: false, elevation: None });
        facts.note_trip_upload(TripUpload { id: 12, replaced: true });
        facts.note_update_result(UpdateResult::Confirmed(crate::dfu::clamp("v9"))).unwrap();

        assert_eq!(facts.take_route_upload(), Some(RouteUpload { id: 11, replaced: false, elevation: None }));
        assert!(facts.take_route_upload().is_none(), "the slot is one-shot");

        assert_eq!(facts.take_trip_upload(), Some(TripUpload { id: 12, replaced: true }));
        assert_eq!(facts.store_revision(), Some(store(1)));
        assert!(facts.take_alerts().contains(Alert::NoCompass));
        assert!(facts.take_update_result().is_some());
    }

    fn board() -> PlatformSupport {
        PlatformSupport {
            detour: true,
            settings_persistence: true,
            dfu: true,

            bonding: true,
            storage_space_report: true,
        }
    }

    fn mounted() -> DeviceFacts {
        DeviceFacts {
            store_writable: true,
            nav_graph: true,

            link_connected: true,
            ride_recording: false,
            heavy_operations: true,
        }
    }

    /// Every field is written out, so a rule flipped in `calculate` fails here rather than passing a
    /// "same inputs, same output" tautology.
    #[test]
    fn capabilities_are_the_written_out_level_of_their_inputs() {
        assert_eq!(
            Capabilities::calculate(board(), mounted()),
            Capabilities {
                catalog: CatalogCapabilities { mutate: true },
                recorder: RecorderCapabilities { record: true },
                navigator: NavigatorCapabilities { plan_route: true, plan_detour: true, commit_detour: true },
                settings: SettingsCapabilities { persist: true },
                dfu: DfuCapabilities { scan: true, install: true },
                bond: BondCapabilities { remove: true },
                storage_info: StorageInfoCapabilities { report_free_space: true },
            }
        );

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

    /// An unsupported detour is a missing capability, not a planning failure: no combination of live
    /// facts can turn it on.
    #[test]
    fn unsupported_detour_never_enters_the_planning_path() {
        let support = PlatformSupport { detour: false, ..board() };
        for bits in 0u8..64 {
            let facts = DeviceFacts {
                store_writable: bits & 1 != 0,
                nav_graph: bits & 2 != 0,

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
