use crate::ble::{BondEffect, BondOutcome};
use crate::catalog_state::{CatalogEffect, CatalogOutcome};
use crate::device_core::storage_info::{StorageInfoEffect, StorageInfoOutcome};
use crate::dfu::{DfuEffect, DfuOutcome};
use crate::metadata::{MetadataEffect, MetadataOutcome};
use crate::navigator::{NavigatorEffect, NavigatorOutcome};
use crate::recorder::{RecorderEffect, RecorderOutcome};
use crate::settings::{SettingsEffect, SettingsOutcome};

/// A [`Slot::try_put`] that found the slot occupied, carrying the rejected value back to its owner.
/// Named rather than a bare `Err(T)`, so a caller's `if let Err(full)` reads as "the slot was busy,
/// keep this" rather than "this value is bad".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotFull<T> {
    /// The value that did not fit. Its owner keeps it and offers it again on the next pass.
    pub rejected: T,
}

/// One bounded output slot: capacity one, first value wins.
///
/// A newtype over `Option<T>` so the capacity-one rule is enforced by the type rather than policed
/// at every call site: a plain `Option` would let any writer overwrite an unconsumed effect with
/// `=`. Deliberately not `Clone` or `Copy` either, because the slot fields are `pub` and a copied
/// slot could be drained beside the original.
#[derive(Debug, PartialEq, Eq)]
pub struct Slot<T> {
    held: Option<T>,
}

impl<T> Slot<T> {
    pub const fn new() -> Self {
        Slot { held: None }
    }

    /// Fill the slot, or refuse and hand `value` back when it is already full. Never overwrites.
    pub fn try_put(&mut self, value: T) -> Result<(), SlotFull<T>> {
        if self.held.is_some() {
            return Err(SlotFull { rejected: value });
        }
        self.held = Some(value);
        Ok(())
    }

    /// Take the held value, emptying the slot. The executor's "consume at most once" is this call.
    pub fn take(&mut self) -> Option<T> {
        self.held.take()
    }

    /// Take the held value only when `pick` accepts it, so an executor serves one kind and leaves
    /// the rest.
    pub fn take_if(&mut self, pick: impl FnOnce(&T) -> bool) -> Option<T> {
        self.held.take_if(|value| pick(value))
    }

    /// Whether the slot holds nothing: the admission test before issuing a new operation.
    pub fn is_empty(&self) -> bool {
        self.held.is_none()
    }
}

impl<T> Default for Slot<T> {
    fn default() -> Self {
        Slot::new()
    }
}

/// Macro for the two eight-field slot structs. They differ only in which types their fields hold,
/// and writing `new`, `Default` and `has_pending` twice by hand leaves eight places to forget a
/// domain.
macro_rules! domain_slots {
    ($(#[$meta:meta])* $name:ident { $( $(#[$field_meta:meta])* $field:ident : $ty:ty ),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Default, PartialEq, Eq)]
        pub struct $name {
            $( $(#[$field_meta])* pub $field: Slot<$ty>, )+
        }

        impl $name {
            pub const fn new() -> Self {
                $name { $( $field: Slot::new(), )+ }
            }

            pub fn has_pending(&self) -> bool {
                $( if !self.$field.is_empty() { return true; } )+
                false
            }
        }
    };
}

domain_slots! {
    /// What DeviceCore asks the platform to do this pass — one bounded operation per domain.
    ///
    /// The executor takes each field it can serve, performs exactly that operation, and answers
    /// through the matching [`OutcomeSlots`] field. A field it cannot serve is simply left; the
    /// domain re-offers it next pass.
    EffectSlots {
        catalog: CatalogEffect,
        recorder: RecorderEffect,
        navigator: NavigatorEffect,
        metadata: MetadataEffect,
        settings: SettingsEffect,
        dfu: DfuEffect,
        bond: BondEffect,
        storage_info: StorageInfoEffect,
    }
}

domain_slots! {
    /// What the platform finished since the last pass — one terminal result per domain.
    ///
    /// DeviceCore consumes these first, before anything else in a pass, and each domain validates
    /// its own [`OperationToken`](super::OperationToken) before believing a word of it.
    OutcomeSlots {
        catalog: CatalogOutcome,
        recorder: RecorderOutcome,
        navigator: NavigatorOutcome,
        metadata: MetadataOutcome,
        settings: SettingsOutcome,
        dfu: DfuOutcome,
        bond: BondOutcome,
        storage_info: StorageInfoOutcome,
    }
}

// Layout tripwires, at 64-bit host ceilings. Both structs are per-pass values on the executor's
// stack, but they cross the seam on every pass, and a growth here means a payload crept into a
// message. `OutcomeSlots` is dominated by `DfuOutcome`'s two fixed 32-byte version strings.
const _: () = assert!(core::mem::size_of::<EffectSlots>() <= 216, "eight bounded effects, no payloads");
const _: () = assert!(core::mem::size_of::<OutcomeSlots>() <= 248, "eight bounded outcomes, no payloads");

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::{DetourRequest, NavRequest};
    use crate::catalog_state::CatalogError;
    use crate::device_core::storage_info::StorageInfoError;
    use crate::device_core::{
        BondTag, CatalogTag, DfuTag, NavigatorTag, OperationToken, RecorderTag, SettingsTag, StorageInfoTag,
        TokenSource,
    };
    use crate::dfu::DfuScanError;
    use crate::navigator::{NavigatorError, PlannerWork};
    use crate::recorder::RecorderError;

    /// The token rule, exercised through one domain's real outcome constructor: the domain accepts
    /// its own live token, and rejects it the moment the operation is superseded.
    fn token_rules<Tag, O>(make: impl Fn(OperationToken<Tag>) -> O, token_of: impl Fn(&O) -> OperationToken<Tag>) {
        let mut source: TokenSource<Tag> = TokenSource::new();

        let token = source.issue();
        let outcome = make(token);
        assert!(source.is_current(token_of(&outcome)), "a domain accepts its own live token");

        source.invalidate(); // cancellation, replacement, or a terminal outcome already accepted
        assert!(!source.is_current(token_of(&outcome)), "a domain rejects a stale token");

        let newer = source.issue();
        assert!(!source.is_current(token_of(&outcome)), "the superseded answer stays rejected");
        assert!(source.is_current(newer));
    }

    /// Every domain, through its own outcome type and its own tag. "A token is checked" is a
    /// per-domain obligation, and a domain that forgot to carry one would not compile into this
    /// list at all.
    #[test]
    fn every_domain_accepts_its_own_token_and_rejects_a_stale_one() {
        token_rules(|token| CatalogOutcome::Cancelled { token }, CatalogOutcome::token);
        token_rules(|token| RecorderOutcome::Cancelled { token }, RecorderOutcome::token);
        token_rules(|token| NavigatorOutcome::Cancelled { token }, NavigatorOutcome::token);
        token_rules(|token| SettingsOutcome::Cancelled { token }, SettingsOutcome::token);
        token_rules(|token| DfuOutcome::Cancelled { token }, DfuOutcome::token);
        token_rules(|token| BondOutcome::Cancelled { token }, BondOutcome::token);
        token_rules(|token| StorageInfoOutcome::Cancelled { token }, StorageInfoOutcome::token);
    }

    /// An effect carries the token its outcome must bring back — the pairing the executor is
    /// forbidden from touching.
    #[test]
    fn an_effect_and_its_outcome_carry_the_same_token() {
        let mut source: TokenSource<CatalogTag> = TokenSource::new();
        let token = source.issue();
        let effect =
            CatalogEffect::RemoveObject { token, object: 7, kind: crate::catalog_state::CatalogObjectKind::Route };
        let outcome = CatalogOutcome::ObjectRemoved { token: effect.token(), object: 7, existed: true };
        assert_eq!(effect.token(), outcome.token());
        assert!(source.is_current(outcome.token()));
    }

    fn effects() -> (EffectSlots, EffectSlots) {
        let mut catalog_ops: TokenSource<CatalogTag> = TokenSource::new();
        let mut recorder_ops: TokenSource<RecorderTag> = TokenSource::new();
        let mut navigator_ops: TokenSource<NavigatorTag> = TokenSource::new();
        let mut settings_ops: TokenSource<SettingsTag> = TokenSource::new();

        let mut dfu_ops: TokenSource<DfuTag> = TokenSource::new();
        let mut bond_ops: TokenSource<BondTag> = TokenSource::new();
        let mut storage_ops: TokenSource<StorageInfoTag> = TokenSource::new();

        let mut first = EffectSlots::new();
        first.catalog.try_put(CatalogEffect::ReadCatalog { token: catalog_ops.issue() }).unwrap();
        first.recorder.try_put(RecorderEffect::Checkpoint { token: recorder_ops.issue() }).unwrap();
        first.navigator.try_put(NavigatorEffect::Step { token: navigator_ops.issue() }).unwrap();
        first.settings.try_put(SettingsEffect::PersistRevision { token: settings_ops.issue(), revision: 3 }).unwrap();

        first.dfu.try_put(DfuEffect::Scan { token: dfu_ops.issue() }).unwrap();
        first.bond.try_put(BondEffect::Forget { token: bond_ops.issue() }).unwrap();
        first.storage_info.try_put(StorageInfoEffect::MeasureFreeSpace { token: storage_ops.issue() }).unwrap();

        // A *different* effect per domain, so a slot that silently overwrote would be visible.
        let mut second = EffectSlots::new();
        second
            .catalog
            .try_put(CatalogEffect::RemoveObject {
                token: catalog_ops.issue(),
                object: 9,
                kind: crate::catalog_state::CatalogObjectKind::Route,
            })
            .unwrap();
        second.recorder.try_put(RecorderEffect::Finalize { token: recorder_ops.issue() }).unwrap();
        let work = PlannerWork::Detour(DetourRequest {
            route: 0,
            from: (0, 0),
            progress_m: 0,
            target_m: 500,
            leg: obc_route::Leg::Detour,
        });
        second.navigator.try_put(NavigatorEffect::Acquire { token: navigator_ops.issue(), work }).unwrap();
        second.settings.try_put(SettingsEffect::PersistRevision { token: settings_ops.issue(), revision: 4 }).unwrap();

        second.dfu.try_put(DfuEffect::ArmInstall { token: dfu_ops.issue() }).unwrap();
        second.bond.try_put(BondEffect::Forget { token: bond_ops.issue() }).unwrap();
        second.storage_info.try_put(StorageInfoEffect::MeasureFreeSpace { token: storage_ops.issue() }).unwrap();

        (first, second)
    }

    fn outcomes() -> (OutcomeSlots, OutcomeSlots) {
        let mut catalog_ops: TokenSource<CatalogTag> = TokenSource::new();
        let mut recorder_ops: TokenSource<RecorderTag> = TokenSource::new();
        let mut navigator_ops: TokenSource<NavigatorTag> = TokenSource::new();
        let mut settings_ops: TokenSource<SettingsTag> = TokenSource::new();

        let mut dfu_ops: TokenSource<DfuTag> = TokenSource::new();
        let mut bond_ops: TokenSource<BondTag> = TokenSource::new();
        let mut storage_ops: TokenSource<StorageInfoTag> = TokenSource::new();

        let mut first = OutcomeSlots::new();
        first.catalog.try_put(CatalogOutcome::CatalogRead { token: catalog_ops.issue(), scope: None }).unwrap();
        first
            .recorder
            .try_put(RecorderOutcome::Checkpointed {
                token: recorder_ops.issue(),
                status: crate::recorder::CheckpointStatus::Durable,
            })
            .unwrap();
        first.navigator.try_put(NavigatorOutcome::Acquired { token: navigator_ops.issue() }).unwrap();
        first.settings.try_put(SettingsOutcome::Persisted { token: settings_ops.issue(), revision: 3 }).unwrap();

        first.dfu.try_put(DfuOutcome::InstallBegan { token: dfu_ops.issue() }).unwrap();
        first
            .bond
            .try_put(BondOutcome::KeysRemoved {
                token: bond_ops.issue(),
                controller: crate::ble::ControllerClearance::Confirmed,
            })
            .unwrap();
        first
            .storage_info
            .try_put(StorageInfoOutcome::Measured { token: storage_ops.issue(), free_bytes: 42 })
            .unwrap();

        let mut second = OutcomeSlots::new();
        let error = CatalogError::Unreadable;
        second.catalog.try_put(CatalogOutcome::Failed { token: catalog_ops.issue(), error }).unwrap();
        second
            .recorder
            .try_put(RecorderOutcome::Failed { token: recorder_ops.issue(), error: RecorderError::Write })
            .unwrap();
        let error = NavigatorError::Workspace;
        second.navigator.try_put(NavigatorOutcome::Failed { token: navigator_ops.issue(), error }).unwrap();
        second.settings.try_put(SettingsOutcome::Cancelled { token: settings_ops.issue() }).unwrap();

        let error = DfuScanError::NotFound;
        second.dfu.try_put(DfuOutcome::ScanFailed { token: dfu_ops.issue(), error }).unwrap();
        let error = crate::ble::BondError::StoreWriteFailed;
        second.bond.try_put(BondOutcome::Failed { token: bond_ops.issue(), error }).unwrap();
        let error = StorageInfoError::NotMounted;
        second.storage_info.try_put(StorageInfoOutcome::Failed { token: storage_ops.issue(), error }).unwrap();

        (first, second)
    }

    /// Every slot field of both structs: a second put is refused, the first value stands untouched,
    /// and the rejected value comes back to its owner unchanged.
    ///
    /// `expected` and `refused` are independent builds of the same two sets, so the assertions
    /// compare against values the slot never saw.
    macro_rules! check_full_slot {
        ($slots:ident, $other:ident, $expected:ident, $refused:ident, $($field:ident),+) => {$(
            let intruder = $other.$field.take().expect("a second value exists");
            let err = $slots.$field.try_put(intruder).expect_err("a full slot refuses");
            assert_eq!(Some(err.rejected), $refused.$field.take(), "the refused value comes back unchanged");
            assert_eq!($slots.$field.take(), $expected.$field.take(), "the first value is what the owner takes");
            assert!($slots.$field.is_empty(), "taking empties the slot");
        )+};
    }

    #[test]
    fn a_full_effect_slot_preserves_the_first_effect() {
        let (mut slots, mut other) = effects();
        let (mut expected, mut refused) = effects();
        assert!(slots.has_pending());

        check_full_slot!(
            slots,
            other,
            expected,
            refused,
            catalog,
            recorder,
            navigator,
            settings,
            dfu,
            bond,
            storage_info
        );

        assert!(!slots.has_pending(), "all eight fields drained");
    }

    /// The outcome twin: an executor that answered twice cannot displace the terminal result the
    /// domain has not consumed yet.
    #[test]
    fn a_full_outcome_slot_preserves_the_first_outcome() {
        let (mut slots, mut other) = outcomes();
        let (mut expected, mut refused) = outcomes();
        assert!(slots.has_pending());

        check_full_slot!(
            slots,
            other,
            expected,
            refused,
            catalog,
            recorder,
            navigator,
            settings,
            dfu,
            bond,
            storage_info
        );

        assert!(!slots.has_pending(), "all eight fields drained");
    }

    /// The backpressure contract as a domain owner uses it: one operation is in flight, the next
    /// request waits in the domain's own state, and it goes out on the pass after the slot frees.
    #[test]
    fn a_domain_retains_later_work_while_one_effect_is_in_flight() {
        let mut tokens: TokenSource<CatalogTag> = TokenSource::new();
        let mut slots = EffectSlots::new();

        // Pass 1: the refresh goes out.
        let refresh = CatalogEffect::ReadCatalog { token: tokens.issue() };
        slots.catalog.try_put(refresh).unwrap();

        // Pass 1, later: a delete is decided while the refresh is still unconsumed.
        let delete = CatalogEffect::RemoveObject {
            token: tokens.issue(),
            object: 12,
            kind: crate::catalog_state::CatalogObjectKind::Route,
        };
        let mut pending = match slots.catalog.try_put(delete) {
            Err(full) => Some(full.rejected),
            Ok(()) => panic!("the slot was occupied"),
        };
        // The executor consumes the refresh — untouched by the refusal.
        assert_eq!(slots.catalog.take(), Some(refresh), "the in-flight refresh is what the executor gets");

        // Pass 2: the retained delete goes out unchanged.
        let retained = pending.take().unwrap();
        slots.catalog.try_put(retained).unwrap();
        assert_eq!(slots.catalog.take(), Some(delete), "the deferred delete survived the busy pass");
        assert!(pending.is_none());
    }

    /// Bulk stays out of the protocol, made mechanical: both slot structs together are far smaller
    /// than a single resident catalog, so no catalog, profile, preview or bundle can hide in one.
    #[test]
    fn large_payload_types_do_not_enter_a_slot() {
        use core::mem::size_of;

        let protocol = size_of::<EffectSlots>() + size_of::<OutcomeSlots>();
        assert!(protocol < size_of::<crate::route::Catalog>(), "a route catalog cannot be in here");
        assert!(protocol < size_of::<crate::ride::RideCatalog>(), "a ride catalog cannot be in here");
        assert!(protocol < size_of::<crate::trip::Trips>(), "the trip folders cannot be in here");

        // A nav preview is the smallest bulk payload named, and even that outweighs the largest
        // single message in the protocol.
        let preview = size_of::<[(i32, i32); crate::app::NAV_PREVIEW_MAX]>();
        assert!(size_of::<NavigatorEffect>() < preview && size_of::<NavigatorOutcome>() < preview);
    }

    /// The planner request is the largest effect payload, and it is bounded by construction: a
    /// fixed name buffer, not a name that can grow.
    #[test]
    fn the_planner_request_stays_bounded() {
        let mut tokens: TokenSource<NavigatorTag> = TokenSource::new();
        let long = "a POI name far longer than the fixed inline buffer this request carries around";
        let work = PlannerWork::Route(NavRequest::new((0, 0), (1, 1), long));
        let effect = NavigatorEffect::Acquire { token: tokens.issue(), work };
        assert_eq!(core::mem::size_of_val(&effect), core::mem::size_of::<NavigatorEffect>());
    }
}
