pub(crate) mod connections;
pub(crate) mod core_mode;
pub mod derived;
pub mod pass;
mod shared;
pub mod slots;
pub mod storage_info;

pub use derived::{
    fill_day_profile, DayProfileKey, DerivedInput, DerivedInputs, DerivedNeeds, DerivedResult, DerivedTargets,
    NavPreviewKey, RestStretch, RideTrackKey,
};

pub use core_mode::ModeState;
pub use pass::{PassClock, PassInputs, PassPlan};
pub use slots::{EffectSlots, OutcomeSlots, Slot, SlotFull};
pub use storage_info::{StorageInfoEffect, StorageInfoError, StorageInfoIntent, StorageInfoOutcome};

pub use shared::{
    BondCapabilities, BondTag, Capabilities, CatalogCapabilities, CatalogTag, DeviceFacts, DfuCapabilities, DfuTag,
    ExternalFacts, FactMergeError, MetadataTag, NavigatorCapabilities, NavigatorTag, OperationToken, PlatformSupport,
    RecorderCapabilities, RecorderTag, Revision, RouteUpload, SettingsCapabilities, SettingsTag,
    StorageInfoCapabilities, StorageInfoTag, StoreIdentity, StoreRevision, TokenSource, TransferState, TripUpload,
    UpdateResult,
};
