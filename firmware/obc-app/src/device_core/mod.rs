//! DeviceCore — the shared product kernel (epic #1433).
//!
//! One product behaviour, one owner. The board, the simulator and the web demo run the *same*
//! state machines here and differ only in the platform executor that performs bounded physical
//! work and reports typed results back.
//!
//! This module holds the shared vocabulary every domain needs (#1435):
//!
//! - [`OperationToken`] / [`TokenSource`] — the per-domain stale-result guard.
//! - [`Capabilities`] — what this device can actually do, recalculated from platform support,
//!   mounted data and heavy-operation admission.
//! - [`ExternalFacts`] — the facts that are *not* an answer to an effect, with one documented
//!   merge rule per field.
//!
//! …plus the seam the domains talk through (#1436):
//!
//! - [`EffectSlots`] / [`OutcomeSlots`] — one bounded slot per domain, capacity one, first value
//!   wins. See [`slots`] for the full contract.
//! - [`storage_info`] — the one domain with no product feature to live beside. The other eight
//!   protocols live beside their owner: `catalog_state`, `retention`, `recorder`, `navigator`,
//!   `settings`, `weather`, `dfu` and `ble`. There is deliberately **no** combined `Effect`,
//!   `Outcome` or `Intent` enum anywhere.
//! - [`migration`] — the Appendix A inventory of the legacy protocol, as compile-checked test data.
//!
//! …and the boundary that is not a command at all (#1437):
//!
//! - [`derived`] — [`DerivedNeeds`] / [`DerivedInputs`]: level-triggered reads guarded by a *key*
//!   (identity + source revision + view revision) instead of an operation token, because nobody
//!   asks for them once. See [`derived`] for why a key survives what a token cannot.
//! - [`feeders`] — the inventory of every public bulk feeder on `App` and its new home, the feeder
//!   twin of [`migration`].
//!
//! The pass entry point arrives in a later slice; nothing here changes the legacy
//! [`HostCommand`](crate::HostCommand) / [`HostEvent`](crate::HostEvent) protocol.

pub mod derived;
pub mod feeders;
pub mod migration;
mod shared;
pub mod slots;
pub mod storage_info;

pub use derived::{DerivedInput, DerivedInputs, DerivedNeeds, DerivedResult, NavPreviewKey, RideTrackKey};

pub use slots::{EffectSlots, OutcomeSlots, Slot, SlotFull};
pub use storage_info::{StorageInfoEffect, StorageInfoError, StorageInfoIntent, StorageInfoOutcome};

pub use shared::{
    BondCapabilities, BondTag, Capabilities, CatalogCapabilities, CatalogTag, DataIdentity, DeviceFacts,
    DfuCapabilities, DfuTag, ExternalFacts, FactMergeError, NavigatorCapabilities, NavigatorTag, OperationToken,
    PlatformSupport, RecorderCapabilities, RecorderTag, RetentionTag, Revision, RouteUpload, SettingsCapabilities,
    SettingsTag, StorageInfoCapabilities, StorageInfoTag, StoreIdentity, StoreRevision, TokenSource, TransferState,
    TripUpload, UpdateResult, WeatherCapabilities, WeatherData, WeatherTag,
};
