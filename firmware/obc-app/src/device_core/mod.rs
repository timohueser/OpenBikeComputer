//! DeviceCore — the shared product kernel (epic #1433).
//!
//! One product behaviour, one owner. The board, the simulator and the web demo run the *same*
//! state machines here and differ only in the platform executor that performs bounded physical
//! work and reports typed results back.
//!
//! This module currently holds the shared vocabulary every domain needs (#1435):
//!
//! - [`OperationToken`] / [`TokenSource`] — the per-domain stale-result guard.
//! - [`Capabilities`] — what this device can actually do, recalculated from platform support,
//!   mounted data and heavy-operation admission.
//! - [`ExternalFacts`] — the facts that are *not* an answer to an effect, with one documented
//!   merge rule per field.
//!
//! Domain effects, outcomes and the pass entry point arrive in later slices; nothing here changes
//! the legacy [`HostCommand`](crate::HostCommand) / [`HostEvent`](crate::HostEvent) protocol.

mod shared;

pub use shared::{
    BondCapabilities, BondTag, Capabilities, CatalogCapabilities, CatalogTag, DataIdentity, DeviceFacts,
    DfuCapabilities, DfuTag, ExternalFacts, FactMergeError, NavigatorCapabilities, NavigatorTag, OperationToken,
    PlatformSupport, RecorderCapabilities, RecorderTag, RetentionTag, Revision, RouteUpload, SettingsCapabilities,
    SettingsTag, StorageInfoCapabilities, StorageInfoTag, StoreIdentity, StoreRevision, TokenSource, TransferState,
    TripUpload, UpdateResult, WeatherCapabilities, WeatherData, WeatherTag,
};
