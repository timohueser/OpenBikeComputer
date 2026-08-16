//! Stable numeric assignments from `Device_Object_Registries_v2.md`.
//!
//! Object and draft-part kinds, the operation flags each kind may advertise, result outcomes,
//! operation phases, and the ObjectKind-scoped semantic detail registry. Nothing here is policy:
//! a device advertises a subset of what a kind *may* do, and this module only says what the
//! registry permits, so a codec can reject an advertisement that claims more.

use crate::error::DecodeError;

/// Logical object kinds (`Device_Object_Registries_v2.md` §1). `u16` on the wire.
///
/// `0` is invalid and `5` is reserved: neither may be advertised or encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
pub enum ObjectKind {
    /// Create, CAS replace, metadata update, list, download, delete.
    Route = 1,
    /// Create, CAS replace, list, download, delete.
    Trip = 2,
    /// Exactly-once finalization, list, download, explicit import acknowledgement, delete.
    Ride = 3,
    /// One store-owned singleton identity, replace, list, download, delete.
    Weather = 4,
    /// Create/replace one atomic release head through draft finalization.
    VolumeManifest = 6,
    /// Publish VerifiedReady, list, download, explicit install, retention/rollback cleanup.
    UpdatePackage = 7,
}

impl ObjectKind {
    /// Every registered kind, in wire order. Used by the codec's coverage tests and the fixtures.
    pub const ALL: [ObjectKind; 6] = [
        ObjectKind::Route,
        ObjectKind::Trip,
        ObjectKind::Ride,
        ObjectKind::Weather,
        ObjectKind::VolumeManifest,
        ObjectKind::UpdatePackage,
    ];

    /// Decodes a wire `u16`. `0`, `5`, and anything above `7` are not kinds.
    pub const fn from_u16(value: u16) -> Option<Self> {
        match value {
            1 => Some(ObjectKind::Route),
            2 => Some(ObjectKind::Trip),
            3 => Some(ObjectKind::Ride),
            4 => Some(ObjectKind::Weather),
            6 => Some(ObjectKind::VolumeManifest),
            7 => Some(ObjectKind::UpdatePackage),
            _ => None,
        }
    }

    /// The wire `u16`.
    pub const fn to_u16(self) -> u16 {
        self as u16
    }

    /// The name used in fixture JSON and diagnostics.
    pub const fn name(self) -> &'static str {
        match self {
            ObjectKind::Route => "route",
            ObjectKind::Trip => "trip",
            ObjectKind::Ride => "ride",
            ObjectKind::Weather => "weather",
            ObjectKind::VolumeManifest => "volumeManifest",
            ObjectKind::UpdatePackage => "updatePackage",
        }
    }

    /// The subject operation flags §1's lifecycle table permits for this kind.
    ///
    /// The two resumable bits are device policy rather than registry policy, so they are always
    /// permitted here; put/get/delete/set-metadata/draft-finalize are not.
    pub const fn permitted_operation_flags(self) -> u16 {
        use subject_flags::*;
        let fixed = match self {
            ObjectKind::Route => PUT | GET | DELETE | SET_METADATA,
            ObjectKind::Trip => PUT | GET | DELETE,
            ObjectKind::Ride => GET | DELETE,
            ObjectKind::Weather => PUT | GET | DELETE,
            ObjectKind::VolumeManifest => GET | DELETE | SET_METADATA | DRAFT_FINALIZE,
            ObjectKind::UpdatePackage => PUT | GET | DELETE,
        };
        fixed | RESUMABLE_UPLOAD | RESUMABLE_DOWNLOAD
    }

    /// True when §4.2 registers a SetMetadata patch schema for this kind.
    pub const fn supports_set_metadata(self) -> bool {
        matches!(self, ObjectKind::Route | ObjectKind::VolumeManifest)
    }
}

/// Draft part kinds (`Device_Object_Registries_v2.md` §2). `u16` on the wire; `0` is invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
pub enum DraftPartKind {
    /// A whole standalone map blob — the one part a sideload import synthesizes.
    StandaloneMapBlob = 1,
    /// One shard of a sharded map release.
    MapShard = 2,
    /// A terrain blob.
    TerrainBlob = 3,
    /// The volume index.
    VolumeIndex = 4,
}

impl DraftPartKind {
    /// Every registered draft-part kind, in wire order.
    pub const ALL: [DraftPartKind; 4] = [
        DraftPartKind::StandaloneMapBlob,
        DraftPartKind::MapShard,
        DraftPartKind::TerrainBlob,
        DraftPartKind::VolumeIndex,
    ];

    /// Decodes a wire `u16`.
    pub const fn from_u16(value: u16) -> Option<Self> {
        match value {
            1 => Some(DraftPartKind::StandaloneMapBlob),
            2 => Some(DraftPartKind::MapShard),
            3 => Some(DraftPartKind::TerrainBlob),
            4 => Some(DraftPartKind::VolumeIndex),
            _ => None,
        }
    }

    /// The wire `u16`.
    pub const fn to_u16(self) -> u16 {
        self as u16
    }

    /// The name used in fixture JSON and diagnostics.
    pub const fn name(self) -> &'static str {
        match self {
            DraftPartKind::StandaloneMapBlob => "standaloneMapBlob",
            DraftPartKind::MapShard => "mapShard",
            DraftPartKind::TerrainBlob => "terrainBlob",
            DraftPartKind::VolumeIndex => "volumeIndex",
        }
    }
}

/// Subject-entry operation flag bits (`Device_Object_Protocol_v3.md` §5).
pub mod subject_flags {
    /// Put bit 0.
    pub const PUT: u16 = 1 << 0;
    /// Get bit 1.
    pub const GET: u16 = 1 << 1;
    /// Delete bit 2.
    pub const DELETE: u16 = 1 << 2;
    /// Set-metadata bit 3.
    pub const SET_METADATA: u16 = 1 << 3;
    /// Resumable upload bit 4.
    pub const RESUMABLE_UPLOAD: u16 = 1 << 4;
    /// Resumable download bit 5.
    pub const RESUMABLE_DOWNLOAD: u16 = 1 << 5;
    /// Draft-finalize bit 6.
    pub const DRAFT_FINALIZE: u16 = 1 << 6;
    /// Every defined bit; anything else is zero.
    pub const ALL: u16 = PUT | GET | DELETE | SET_METADATA | RESUMABLE_UPLOAD | RESUMABLE_DOWNLOAD | DRAFT_FINALIZE;
}

/// Subject-entry policy flag bits (`Device_Object_Protocol_v3.md` §5).
pub mod policy_flags {
    /// USB recommended bit 0.
    pub const USB_RECOMMENDED: u16 = 1 << 0;
    /// External power required bit 1.
    pub const EXTERNAL_POWER_REQUIRED: u16 = 1 << 1;
    /// Authenticated principal required bit 2.
    pub const AUTHENTICATED_PRINCIPAL_REQUIRED: u16 = 1 << 2;
    /// Fixed singleton bit 3.
    pub const FIXED_SINGLETON: u16 = 1 << 3;
    /// Every defined bit; anything else is zero.
    pub const ALL: u16 = USB_RECOMMENDED | EXTERNAL_POWER_REQUIRED | AUTHENTICATED_PRINCIPAL_REQUIRED | FIXED_SINGLETON;
}

/// Metadata schema versions (`Device_Object_Registries_v2.md` §4). These are registry constants,
/// not negotiated values.
pub mod schema_version {
    /// Put schemas.
    pub const PUT: u8 = 1;
    /// SetMetadata patch schemas.
    pub const PATCH: u8 = 128;
    /// Catalog projection schemas.
    pub const CATALOG: u8 = 64;
}

/// `ObjectResult` outcomes (`Device_Object_Protocol_v3.md` §10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum ObjectOutcome {
    /// A payload committed as the new head.
    Committed = 0,
    /// Registered, reserved, and never emitted in v3.0. It named a
    /// committed-for-superseded-weather-request outcome; a stale bundle is rejected instead.
    ///
    /// It exists here as a decode-only value so the number stays burned and so the suite can carry
    /// the decode-only vector §2 of the vectors contract asks for.
    ReservedSupersededWeather = 1,
    /// A head was deleted.
    Deleted = 2,
    /// Catalog metadata changed.
    MetadataChanged = 3,
    /// An update install was requested and the boot handoff is armed.
    UpdateInstallRequested = 4,
    /// A ride's import was acknowledged.
    RideImported = 5,
}

impl ObjectOutcome {
    /// Decodes a wire `u16`.
    pub const fn from_u16(value: u16) -> Option<Self> {
        match value {
            0 => Some(ObjectOutcome::Committed),
            1 => Some(ObjectOutcome::ReservedSupersededWeather),
            2 => Some(ObjectOutcome::Deleted),
            3 => Some(ObjectOutcome::MetadataChanged),
            4 => Some(ObjectOutcome::UpdateInstallRequested),
            5 => Some(ObjectOutcome::RideImported),
            _ => None,
        }
    }

    /// The wire `u16`.
    pub const fn to_u16(self) -> u16 {
        self as u16
    }

    /// True for the one registered value no conforming v3.0 device emits.
    pub const fn is_reserved(self) -> bool {
        matches!(self, ObjectOutcome::ReservedSupersededWeather)
    }

    /// The name used in fixture JSON and diagnostics.
    pub const fn name(self) -> &'static str {
        match self {
            ObjectOutcome::Committed => "committed",
            ObjectOutcome::ReservedSupersededWeather => "reservedSupersededWeather",
            ObjectOutcome::Deleted => "deleted",
            ObjectOutcome::MetadataChanged => "metadataChanged",
            ObjectOutcome::UpdateInstallRequested => "updateInstallRequested",
            ObjectOutcome::RideImported => "rideImported",
        }
    }
}

/// The operation phase reported by `QueryOperation` (`Device_Object_Protocol_v3.md` §8.1).
///
/// `Device_Object_System_v2.md` §7 pins the mapping onto the storage phase byte and warns that the
/// two numberings "differ because each was allocated in its own order; neither is derived from the
/// other, and a codec MUST translate through this table rather than by arithmetic" — hence
/// [`storage_phase_byte`](Phase::storage_phase_byte) being a match rather than an offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Phase {
    /// Claimed, or prepared with a session issued — one wire phase for both.
    Prepared = 0,
    /// Payload bytes are being accepted.
    Streaming = 1,
    /// Bytes are sealed and synchronized.
    Sealed = 2,
    /// The typed validator is running.
    Validating = 3,
    /// The catalog commit is in flight.
    Publishing = 4,
    /// An armed update install awaiting its boot handoff.
    ExternalHandoff = 5,
    /// A draft parent before its manifest phases.
    DraftOpen = 6,
    /// Unwinding towards a durable Aborted result.
    Aborting = 7,
}

impl Phase {
    /// Every phase, in wire order.
    pub const ALL: [Phase; 8] = [
        Phase::Prepared,
        Phase::Streaming,
        Phase::Sealed,
        Phase::Validating,
        Phase::Publishing,
        Phase::ExternalHandoff,
        Phase::DraftOpen,
        Phase::Aborting,
    ];

    /// Decodes a wire `u8`.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Phase::Prepared),
            1 => Some(Phase::Streaming),
            2 => Some(Phase::Sealed),
            3 => Some(Phase::Validating),
            4 => Some(Phase::Publishing),
            5 => Some(Phase::ExternalHandoff),
            6 => Some(Phase::DraftOpen),
            7 => Some(Phase::Aborting),
            _ => None,
        }
    }

    /// The wire `u8`.
    pub const fn to_u8(self) -> u8 {
        self as u8
    }

    /// The storage record's phase byte for the same state (`Device_Object_System_v2.md` §7).
    pub const fn storage_phase_byte(self) -> u8 {
        match self {
            Phase::Prepared => 1,
            Phase::DraftOpen => 2,
            Phase::Streaming => 3,
            Phase::Sealed => 4,
            Phase::Validating => 5,
            Phase::Publishing => 6,
            Phase::ExternalHandoff => 7,
            Phase::Aborting => 8,
        }
    }

    /// The name used in fixture JSON and diagnostics.
    pub const fn name(self) -> &'static str {
        match self {
            Phase::Prepared => "prepared",
            Phase::Streaming => "streaming",
            Phase::Sealed => "sealed",
            Phase::Validating => "validating",
            Phase::Publishing => "publishing",
            Phase::ExternalHandoff => "externalHandoff",
            Phase::DraftOpen => "draftOpen",
            Phase::Aborting => "aborting",
        }
    }
}

/// The subject namespace of a progress body or subject entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum SubjectNamespace {
    /// No subject; the kind field is zero.
    None = 0,
    /// A logical `ObjectKind`.
    Logical = 1,
    /// A `DraftPartKind`.
    DraftPart = 2,
}

impl SubjectNamespace {
    /// Decodes a wire `u8`.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(SubjectNamespace::None),
            1 => Some(SubjectNamespace::Logical),
            2 => Some(SubjectNamespace::DraftPart),
            _ => None,
        }
    }

    /// The wire `u8`.
    pub const fn to_u8(self) -> u8 {
        self as u8
    }

    /// The name used in fixture JSON and diagnostics.
    pub const fn name(self) -> &'static str {
        match self {
            SubjectNamespace::None => "none",
            SubjectNamespace::Logical => "logical",
            SubjectNamespace::DraftPart => "draftPart",
        }
    }
}

/// The reason byte shared by `AbortSession` and `AbortOperation`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum AbortReason {
    /// The client cancelled the work.
    ClientCancelled = 1,
    /// A newer request supersedes it.
    Superseded = 2,
    /// The rider asked for it.
    UserRequested = 3,
}

impl AbortReason {
    /// Every reason, in wire order.
    pub const ALL: [AbortReason; 3] =
        [AbortReason::ClientCancelled, AbortReason::Superseded, AbortReason::UserRequested];

    /// Decodes a wire `u8`. Zero is not a reason.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(AbortReason::ClientCancelled),
            2 => Some(AbortReason::Superseded),
            3 => Some(AbortReason::UserRequested),
            _ => None,
        }
    }

    /// The wire `u8`.
    pub const fn to_u8(self) -> u8 {
        self as u8
    }

    /// The name used in fixture JSON and diagnostics.
    pub const fn name(self) -> &'static str {
        match self {
            AbortReason::ClientCancelled => "clientCancelled",
            AbortReason::Superseded => "superseded",
            AbortReason::UserRequested => "userRequested",
        }
    }
}

/// Update package states carried in the catalog projection (`Device_Object_Registries_v2.md` §4.3).
pub mod update_state {
    /// Signature, digest, target, version and size all validated.
    pub const VERIFIED_READY: u8 = 1;
    /// An install has been requested and the handoff is armed.
    pub const INSTALL_REQUESTED: u8 = 2;
    /// Running on trial after the handoff.
    pub const TRIAL: u8 = 3;
    /// Trial confirmed healthy.
    pub const CONFIRMED: u8 = 4;
    /// Rolled back to the previous image.
    pub const ROLLED_BACK: u8 = 5;
    /// The install failed.
    pub const FAILED: u8 = 6;
    /// Every registered value, in order.
    pub const ALL: [u8; 6] = [VERIFIED_READY, INSTALL_REQUESTED, TRIAL, CONFIRMED, ROLLED_BACK, FAILED];
}

/// Route retention values shared by the Put and patch schemas (`Device_Object_Registries_v2.md`
/// §4.1).
pub mod retention {
    /// Keep forever.
    pub const NEVER: u8 = 0;
    /// One day.
    pub const DAY: u8 = 1;
    /// One week.
    pub const WEEK: u8 = 2;
    /// Two weeks.
    pub const TWO_WEEKS: u8 = 3;
    /// One month.
    pub const MONTH: u8 = 4;
    /// Two months.
    pub const TWO_MONTHS: u8 = 5;
    /// The inclusive maximum.
    pub const MAX: u8 = TWO_MONTHS;
}

/// ObjectKind-scoped `semanticValidation` details (`Device_Object_Registries_v2.md` §6).
pub mod semantic {
    use super::ObjectKind;

    /// One registered semantic detail.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct SemanticDetail {
        /// The owning namespace.
        pub kind: ObjectKind,
        /// The detail code inside that namespace.
        pub code: u16,
        /// The registry's name for it.
        pub name: &'static str,
        /// False for the nonterminal rows: `ride.alreadyImported`, `volumeManifest.draftIncomplete`,
        /// `updatePackage.unsafePowerState`, and `updatePackage.unsafeRuntimeState`.
        pub terminal: bool,
        /// True for a row registered only so its number stays burned. A v3.0 device never sends it.
        pub reserved: bool,
    }

    const fn detail(kind: ObjectKind, code: u16, name: &'static str, terminal: bool) -> SemanticDetail {
        SemanticDetail { kind, code, name, terminal, reserved: false }
    }

    /// The complete §6 table, in registry order.
    pub const ALL: [SemanticDetail; 22] = [
        detail(ObjectKind::Route, 1, "invalidRouteFormat", true),
        detail(ObjectKind::Trip, 1, "invalidTripFormat", true),
        detail(ObjectKind::Trip, 2, "duplicateRouteReference", true),
        detail(ObjectKind::Trip, 3, "missingTripRoute", true),
        detail(ObjectKind::Ride, 1, "invalidRideFormat", true),
        detail(ObjectKind::Ride, 2, "alreadyImported", false),
        SemanticDetail {
            kind: ObjectKind::Weather,
            code: 1,
            name: "supersededNotUseful",
            terminal: true,
            reserved: true,
        },
        detail(ObjectKind::Weather, 2, "coverageMismatch", true),
        detail(ObjectKind::Weather, 3, "staleBundle", true),
        detail(ObjectKind::Weather, 4, "payloadFactsMismatch", true),
        detail(ObjectKind::Weather, 5, "requestMismatch", true),
        detail(ObjectKind::VolumeManifest, 1, "invalidManifest", true),
        detail(ObjectKind::VolumeManifest, 2, "missingDraftPart", true),
        detail(ObjectKind::VolumeManifest, 3, "foreignDraftPart", true),
        detail(ObjectKind::VolumeManifest, 4, "duplicateDraftReference", true),
        detail(ObjectKind::VolumeManifest, 5, "duplicateDraftPart", true),
        detail(ObjectKind::VolumeManifest, 6, "draftNotOpen", true),
        detail(ObjectKind::VolumeManifest, 7, "draftIncomplete", false),
        detail(ObjectKind::UpdatePackage, 1, "invalidSignature", true),
        detail(ObjectKind::UpdatePackage, 2, "digestMismatch", true),
        detail(ObjectKind::UpdatePackage, 3, "wrongTarget", true),
        detail(ObjectKind::UpdatePackage, 4, "downgradeDenied", true),
    ];

    /// The rows §6 lists after `downgradeDenied`, kept in a second array only because
    /// `ALL` is sized by the const above; [`table`] joins them.
    pub const ALL_TAIL: [SemanticDetail; 4] = [
        detail(ObjectKind::UpdatePackage, 5, "packageTooLarge", true),
        detail(ObjectKind::UpdatePackage, 6, "unsafePowerState", false),
        detail(ObjectKind::UpdatePackage, 7, "unsafeRuntimeState", false),
        detail(ObjectKind::UpdatePackage, 8, "notVerifiedReady", true),
    ];

    /// The complete registry as one iterator.
    pub fn table() -> impl Iterator<Item = SemanticDetail> {
        ALL.into_iter().chain(ALL_TAIL)
    }

    /// Looks a detail up by namespace and code.
    pub fn lookup(kind: ObjectKind, code: u16) -> Option<SemanticDetail> {
        table().find(|row| row.kind == kind && row.code == code)
    }
}

/// Decodes an `ObjectKind` field, mapping an unregistered value to the contract's own rejection.
///
/// An unregistered kind in a *descriptor* is `invalidDescriptor/unknownEnum`: the frame is complete
/// and its field is illegal. A kind the device simply does not serve is `unsupportedCapability`,
/// which is an admission decision this crate does not make.
pub(crate) fn object_kind(value: u16) -> Result<ObjectKind, DecodeError> {
    ObjectKind::from_u16(value).ok_or_else(DecodeError::unknown_enum)
}

/// Decodes a `DraftPartKind` field the same way.
pub(crate) fn draft_part_kind(value: u16) -> Result<DraftPartKind, DecodeError> {
    DraftPartKind::from_u16(value).ok_or_else(DecodeError::unknown_enum)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_kind_five_is_reserved_and_zero_is_invalid() {
        assert!(ObjectKind::from_u16(0).is_none());
        assert!(ObjectKind::from_u16(5).is_none());
        assert!(ObjectKind::from_u16(8).is_none());
        assert_eq!(ObjectKind::from_u16(7), Some(ObjectKind::UpdatePackage));
    }

    #[test]
    fn lifecycle_table_forbids_the_operations_the_registry_marks_no() {
        use subject_flags::*;
        assert_eq!(ObjectKind::Ride.permitted_operation_flags() & PUT, 0);
        assert_eq!(ObjectKind::VolumeManifest.permitted_operation_flags() & PUT, 0);
        assert_ne!(ObjectKind::VolumeManifest.permitted_operation_flags() & DRAFT_FINALIZE, 0);
        assert_eq!(ObjectKind::Trip.permitted_operation_flags() & SET_METADATA, 0);
        assert_ne!(ObjectKind::Route.permitted_operation_flags() & SET_METADATA, 0);
        for kind in ObjectKind::ALL {
            let permitted = kind.permitted_operation_flags();
            assert_eq!(permitted & !ALL, 0, "{} advertises an undefined bit", kind.name());
        }
    }

    #[test]
    fn wire_and_storage_phase_numbering_differ_and_are_not_arithmetic() {
        assert_eq!(Phase::DraftOpen.to_u8(), 6);
        assert_eq!(Phase::DraftOpen.storage_phase_byte(), 2);
        assert_eq!(Phase::Streaming.to_u8(), 1);
        assert_eq!(Phase::Streaming.storage_phase_byte(), 3);
        // No constant offset maps one onto the other.
        let offsets: std::collections::BTreeSet<i16> =
            Phase::ALL.iter().map(|p| i16::from(p.storage_phase_byte()) - i16::from(p.to_u8())).collect();
        assert!(offsets.len() > 1);
    }

    #[test]
    fn semantic_registry_has_every_row_and_marks_the_reserved_one() {
        assert_eq!(semantic::table().count(), 26);
        let superseded = semantic::lookup(ObjectKind::Weather, 1).unwrap();
        assert!(superseded.reserved);
        assert!(!semantic::lookup(ObjectKind::VolumeManifest, 7).unwrap().terminal);
        assert!(semantic::lookup(ObjectKind::UpdatePackage, 8).unwrap().terminal);
        assert!(semantic::lookup(ObjectKind::Route, 2).is_none());
    }
}
