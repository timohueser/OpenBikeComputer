//! The error body, its category/detail registry, and the crate's own decode failures
//! (`Device_Object_Protocol_v3.md` §12).
//!
//! ## Two different things called "error"
//!
//! [`ErrorBody`] is a *message*: the 48-byte payload a device sends in a `response|error` frame,
//! or replays out of a retained terminal result. [`DecodeError`] is what this crate returns when
//! it refuses an input. They share the category/detail vocabulary on purpose — a decoder's refusal
//! is exactly the error the device would send — but they are not the same type, because an
//! `ErrorBody` carries presence bits, owner, and retry guidance that a codec has no business
//! inventing.
//!
//! ## What an `ErrorBody` decode rejects, and what it deliberately does not
//!
//! §12 twice tells a receiver not to destroy a failure report over a field that drives nothing:
//! text "MUST NOT" cause rejection, and "A decoder MUST NOT reject an ErrorBody because an optional
//! field is present where it expected none, or absent where the category would normally require
//! one". The presence matrix binds senders only. This decoder therefore rejects **structure**
//! only — wrong length, a text length above 64 or disagreeing with the payload, nonzero reserved
//! bytes, a presence bit above 6, the illegal claim-status combination (bit 6 without bit 5), and
//! the reserved category `0`, which §12 says to treat "as a malformed body rather than as an
//! unknown future category".
//!
//! By the same reasoning [`ErrorCategory`], [`RetryGuidance`], and [`Owner`] are open newtypes with
//! named constants rather than closed enums: an unrecognised value is preserved for diagnostics
//! instead of turning a real failure report into a second failure. Descriptor enums elsewhere in
//! the crate are the opposite — a request's target mode, resume byte, or reason is closed, and an
//! unregistered value is `invalidDescriptor/unknownEnum`, exactly as the contract says.

use crate::codec::{is_zero, put_bytes, put_u16, put_u32, put_u64, u16_at, u32_at, u64_at};
use crate::ids::Revision;
use crate::registry::ObjectKind;
use crate::{BufferTooSmall, EncodeResult};

/// The fixed prefix of an `ErrorBody`, before any diagnostic text.
pub const ERROR_BODY_PREFIX_LEN: usize = 48;

/// The maximum diagnostic text, in encoded UTF-8 bytes.
pub const MAX_ERROR_TEXT: usize = 64;

/// The largest complete `ErrorBody` payload: prefix plus maximum text.
pub const MAX_ERROR_BODY_LEN: usize = ERROR_BODY_PREFIX_LEN + MAX_ERROR_TEXT;

/// A machine-readable error category (§12). Open: an unregistered nonzero value decodes and is
/// preserved; only `0` is invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ErrorCategory(u16);

macro_rules! categories {
    ($($(#[$meta:meta])* $konst:ident = $value:expr, $name:literal;)*) => {
        impl ErrorCategory {
            $($(#[$meta])* pub const $konst: ErrorCategory = ErrorCategory($value);)*

            /// Every category registered in v3.0, in wire order.
            pub const ALL: [ErrorCategory; 22] = [$(ErrorCategory($value)),*];

            /// The registry's name, or `"unknown"` for a value this version does not name.
            pub const fn name(self) -> &'static str {
                match self.0 {
                    $($value => $name,)*
                    _ => "unknown",
                }
            }
        }
    };
}

categories! {
    /// The peer's wire major or minor cannot be served.
    INCOMPATIBLE_VERSION = 1, "incompatibleVersion";
    /// A known-but-unserved opcode, kind, or feature.
    UNSUPPORTED_CAPABILITY = 2, "unsupportedCapability";
    /// The principal could not be authenticated.
    AUTHENTICATION_FAILED = 3, "authenticationFailed";
    /// The principal is not authorized for this operation or kind.
    AUTHORIZATION_FAILED = 4, "authorizationFailed";
    /// Another owner holds the resource; the owner class is reported, never a secret token.
    BUSY = 5, "busy";
    /// A transport record could not be established as one complete frame.
    INVALID_FRAME = 6, "invalidFrame";
    /// A complete frame carries an illegal field value, reserved bit, enum, or combination.
    INVALID_DESCRIPTOR = 7, "invalidDescriptor";
    /// A stream frame arrived at an offset the session does not expect.
    INVALID_OFFSET = 8, "invalidOffset";
    /// The named session is unknown, stale, or owned by someone else.
    INVALID_SESSION = 9, "invalidSession";
    /// The authorized target does not exist.
    OBJECT_NOT_FOUND = 10, "objectNotFound";
    /// A compare-and-swap failed; the current revision is reported.
    REVISION_CONFLICT = 11, "revisionConflict";
    /// Admission cannot reserve the space; required and available bytes are reported.
    INSUFFICIENT_SPACE = 12, "insufficientSpace";
    /// A declared CRC did not match the bytes.
    CHECKSUM_FAILURE = 13, "checksumFailure";
    /// A typed validator refused the payload or the request's domain semantics.
    SEMANTIC_VALIDATION = 14, "semanticValidation";
    /// No usable medium.
    MEDIA_UNAVAILABLE = 15, "mediaUnavailable";
    /// The medium failed during the operation.
    MEDIA_IO = 16, "mediaIo";
    /// The work was cancelled.
    CANCELLED = 17, "cancelled";
    /// The link went away under an operation-bearing request.
    LINK_LOST = 18, "linkLost";
    /// The same OperationId already carries a different intent.
    OPERATION_ID_CONFLICT = 19, "operationIdConflict";
    /// A compiled capacity is exhausted.
    RESOURCE_LIMIT = 20, "resourceLimit";
    /// The snapshot a cursor or expected revision named has moved.
    CATALOG_CHANGED = 21, "catalogChanged";
    /// An invariant, codec, or reconciliation fault inside the device.
    INTERNAL = 22, "internal";
}

impl ErrorCategory {
    /// Decodes a wire `u16`. `0` is reserved and invalid (§12).
    pub const fn from_u16(value: u16) -> Option<Self> {
        if value == 0 {
            None
        } else {
            Some(ErrorCategory(value))
        }
    }

    /// The wire `u16`.
    pub const fn get(self) -> u16 {
        self.0
    }

    /// True when this crate's own version names the value.
    pub fn is_registered(self) -> bool {
        Self::ALL.contains(&self)
    }
}

/// Retry guidance (§12). Open, for the reason [`ErrorCategory`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RetryGuidance(u8);

macro_rules! guidances {
    ($($(#[$meta:meta])* $konst:ident = $value:expr, $name:literal;)*) => {
        impl RetryGuidance {
            $($(#[$meta])* pub const $konst: RetryGuidance = RetryGuidance($value);)*

            /// Every guidance registered in v3.0, in wire order.
            pub const ALL: [RetryGuidance; 10] = [$(RetryGuidance($value)),*];

            /// The registry's name, or `"unknown"`.
            pub const fn name(self) -> &'static str {
                match self.0 {
                    $($value => $name,)*
                    _ => "unknown",
                }
            }
        }
    };
}

guidances! {
    /// The decision is permanent; do not retry.
    REJECT_PERMANENTLY = 0, "rejectPermanently";
    /// Retry the identical request.
    RETRY_SAME_REQUEST = 1, "retrySameRequest";
    /// Retry after the supplied delay.
    RETRY_AFTER_DELAY = 2, "retryAfterDelay";
    /// Retry after the reported owner releases.
    RETRY_AFTER_OWNER_RELEASE = 3, "retryAfterOwnerRelease";
    /// Reconnect, then query the OperationId.
    RECONNECT_THEN_QUERY = 4, "reconnectThenQueryOperation";
    /// Query the OperationId now.
    QUERY_OPERATION_NOW = 5, "queryOperationNow";
    /// Resume at the reported expected offset.
    RESUME_AT_EXPECTED_OFFSET = 6, "resumeAtExpectedOffset";
    /// Refresh catalog or domain state.
    REFRESH = 7, "refreshCatalogOrDomainState";
    /// Use a new OperationId, and only for genuinely new intent.
    NEW_ID_FOR_NEW_INTENT = 8, "newOperationIdForNewIntent";
    /// Retry only after user action.
    RETRY_AFTER_USER_ACTION = 9, "retryAfterUserAction";
}

impl RetryGuidance {
    /// Wraps a wire `u8`.
    pub const fn from_u8(value: u8) -> Self {
        RetryGuidance(value)
    }

    /// The wire `u8`.
    pub const fn get(self) -> u8 {
        self.0
    }
}

/// The owner class of a `busy` refusal (§12). Values `1`, `2`, and `3` deliberately coincide with
/// the link kinds of §5; `0`, `4`, and `5` are owner-only and have no link-kind meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Owner(u8);

macro_rules! owners {
    ($($(#[$meta:meta])* $konst:ident = $value:expr, $name:literal;)*) => {
        impl Owner {
            $($(#[$meta])* pub const $konst: Owner = Owner($value);)*

            /// Every owner value, in wire order.
            pub const ALL: [Owner; 6] = [$(Owner($value)),*];

            /// The registry's name, or `"unknown"`.
            pub const fn name(self) -> &'static str {
                match self.0 {
                    $($value => $name,)*
                    _ => "unknown",
                }
            }
        }
    };
}

owners! {
    /// No owner is being reported.
    NONE = 0, "none";
    /// A BLE principal.
    BLE = 1, "ble";
    /// The USB local principal.
    USB = 2, "usb";
    /// The test link kind.
    TEST = 3, "test";
    /// A device-local producer: ride, weather, update state, or sideload import.
    LOCAL_PRODUCER = 4, "localProducer";
    /// The reserved maintenance/cancellation/recovery claim.
    MAINTENANCE = 5, "maintenance";
}

impl Owner {
    /// Wraps a wire `u8`.
    pub const fn from_u8(value: u8) -> Self {
        Owner(value)
    }

    /// The wire `u8`.
    pub const fn get(self) -> u8 {
        self.0
    }
}

/// `ErrorBody` presence bits (§12).
pub mod presence {
    /// Retry-after milliseconds is meaningful.
    pub const RETRY_DELAY: u16 = 1 << 0;
    /// Expected offset is meaningful.
    pub const EXPECTED_OFFSET: u16 = 1 << 1;
    /// Current revision is meaningful.
    pub const CURRENT_REVISION: u16 = 1 << 2;
    /// Required bytes is meaningful.
    pub const REQUIRED_BYTES: u16 = 1 << 3;
    /// Available bytes is meaningful.
    pub const AVAILABLE_BYTES: u16 = 1 << 4;
    /// A durable claim exists for this OperationId under this principal.
    pub const DURABLE_CLAIM_EXISTS: u16 = 1 << 5;
    /// That claim is terminal. Meaningful only with [`DURABLE_CLAIM_EXISTS`] set.
    pub const CLAIM_IS_TERMINAL: u16 = 1 << 6;
    /// Every defined bit; bits `7..15` are zero.
    pub const ALL: u16 = RETRY_DELAY
        | EXPECTED_OFFSET
        | CURRENT_REVISION
        | REQUIRED_BYTES
        | AVAILABLE_BYTES
        | DURABLE_CLAIM_EXISTS
        | CLAIM_IS_TERMINAL;
}

/// Common-namespace detail codes, category by category (§12's detail table).
pub mod detail {
    /// `incompatibleVersion`.
    pub mod version {
        /// The peer's wire major cannot be served.
        pub const UNSUPPORTED_MAJOR: u16 = 1;
        /// The requested feature is gated above the device's wire minor.
        pub const UNSUPPORTED_MINOR: u16 = 2;
    }
    /// `unsupportedCapability`.
    pub mod capability {
        /// Unknown or unserved opcode.
        pub const OPCODE: u16 = 1;
        /// Unserved logical kind.
        pub const LOGICAL_KIND: u16 = 2;
        /// Unserved draft-part kind.
        pub const DRAFT_PART_KIND: u16 = 3;
        /// Unserved feature.
        pub const FEATURE: u16 = 4;
        /// Unserved schema version.
        pub const SCHEMA_VERSION: u16 = 5;
        /// The named target is not cancellable — an armed `InstallUpdate`.
        pub const NON_CANCELLABLE_OPERATION: u16 = 6;
    }
    /// `authenticationFailed`.
    pub mod authentication {
        /// No credential was presented.
        pub const MISSING_CREDENTIAL: u16 = 1;
        /// The credential is invalid.
        pub const INVALID_CREDENTIAL: u16 = 2;
        /// The credential has expired.
        pub const EXPIRED_CREDENTIAL: u16 = 3;
    }
    /// `authorizationFailed`.
    pub mod authorization {
        /// Wrong principal scope.
        pub const PRINCIPAL_SCOPE: u16 = 1;
        /// Not the operation's owner.
        pub const OPERATION_OWNER: u16 = 2;
        /// No domain read authority.
        pub const DOMAIN_READ: u16 = 3;
        /// No domain write authority.
        pub const DOMAIN_WRITE: u16 = 4;
        /// No update-install authority.
        pub const INSTALL_AUTHORITY: u16 = 5;
        /// No device-control authority.
        pub const DEVICE_CONTROL: u16 = 6;
    }
    /// `busy`.
    pub mod busy {
        /// The single heavy-transfer slot is taken.
        pub const HEAVY_TRANSFER: u16 = 1;
        /// All eight normal claim slots are occupied.
        pub const NORMAL_OPERATION_CLAIMS: u16 = 2;
        /// All resumable upload work slots are occupied.
        pub const UPLOAD_WORK_SLOTS: u16 = 3;
        /// Another draft parent is open.
        pub const DRAFT_PARENTS: u16 = 4;
        /// Reserved and never emitted: the one active parent owns the whole part budget.
        pub const DRAFT_PARTS: u16 = 5;
        /// All reader leases are held.
        pub const READER_LEASES: u16 = 6;
        /// The reserved cancellation/recovery claim is occupied.
        pub const MAINTENANCE_CANCELLATION_RECOVERY_CLAIM: u16 = 7;
        /// Reserved and never emitted: v3.0 has no distinct maintenance mode.
        pub const MAINTENANCE: u16 = 8;
        /// The single ride slot is occupied by an active or recoverable ride.
        pub const RIDE_SLOT: u16 = 9;
        /// Reserved and never emitted: the retained-generation table cannot be exhausted.
        pub const RETAINED_PREVIOUS: u16 = 10;
    }
    /// `invalidFrame`.
    pub mod frame {
        /// The header itself is unusable.
        pub const MALFORMED_HEADER: u16 = 1;
        /// The transport record length is illegal.
        pub const RECORD_LENGTH: u16 = 2;
        /// Bad magic.
        pub const MAGIC: u16 = 3;
        /// Payload length disagrees with the record.
        pub const PAYLOAD_LENGTH: u16 = 4;
        /// The frame is outside the negotiated bounds.
        pub const FRAME_BOUNDS: u16 = 5;
        /// The frame ends inside a field.
        pub const TRUNCATED: u16 = 6;
        /// The frame carries a byte past the end of its stated layout.
        pub const TRAILING_BYTES: u16 = 7;
    }
    /// `invalidDescriptor`.
    pub mod descriptor {
        /// A reserved field or bit is nonzero.
        pub const RESERVED_BITS: u16 = 1;
        /// An enum field carries an unregistered value.
        pub const UNKNOWN_ENUM: u16 = 2;
        /// The fields are individually legal and jointly illegal.
        pub const INVALID_COMBINATION: u16 = 3;
        /// A nested length disagrees with its container.
        pub const NESTED_LENGTH: u16 = 4;
        /// A metadata envelope is not in canonical form.
        pub const NONCANONICAL_METADATA: u16 = 5;
        /// A metadata base tag appears twice.
        pub const DUPLICATE_FIELD: u16 = 6;
        /// Metadata fields are not strictly increasing by base tag.
        pub const OUT_OF_ORDER_FIELD: u16 = 7;
        /// A flags word carries a bit this version does not define.
        pub const UNSUPPORTED_FLAGS: u16 = 8;
        /// A frame carried RequestId zero. Recorded and logged, never transmitted (§2).
        pub const ZERO_REQUEST_ID: u16 = 9;
        /// A SetMetadata patch envelope carries no field.
        pub const EMPTY_METADATA_PATCH: u16 = 10;
    }
    /// `invalidOffset`.
    pub mod offset {
        /// The frame's offset is not the session's next offset.
        pub const UNEXPECTED_OFFSET: u16 = 1;
        /// A checkpoint offset is not on a granule boundary or the declared end.
        pub const CHECKPOINT_BOUNDARY: u16 = 2;
    }
    /// `invalidSession`.
    pub mod session {
        /// No such session.
        pub const UNKNOWN: u16 = 1;
        /// The session belongs to an earlier connection generation.
        pub const STALE_CONNECTION: u16 = 2;
        /// The session belongs to another principal.
        pub const WRONG_PRINCIPAL: u16 = 3;
        /// The session belongs to another link kind.
        pub const WRONG_LINK: u16 = 4;
        /// The frame's direction is not the session's.
        pub const WRONG_DIRECTION: u16 = 5;
    }
    /// `objectNotFound`.
    pub mod not_found {
        /// No such logical object.
        pub const LOGICAL_OBJECT: u16 = 1;
        /// Reserved and never emitted: a download resolves the current head (§7).
        pub const REQUESTED_REVISION: u16 = 2;
        /// No active draft parent under this OperationId.
        pub const DRAFT_PARENT_UNKNOWN: u16 = 3;
        /// The parent is terminal; query the OperationId.
        pub const OPERATION_TERMINAL: u16 = 4;
        /// Reserved and never emitted: resume is a preference (§6.1).
        pub const RESUMABLE_WORK: u16 = 5;
        /// No durable weather request context exists.
        pub const WEATHER_REQUEST_CONTEXT: u16 = 6;
    }
    /// `revisionConflict`.
    pub mod revision {
        /// The entry's revision moved.
        pub const OBJECT: u16 = 1;
        /// The repository revision moved.
        pub const REPOSITORY: u16 = 2;
        /// The singleton's revision moved.
        pub const SINGLETON: u16 = 3;
    }
    /// `insufficientSpace`.
    pub mod space {
        /// Not enough reservation bytes.
        pub const RESERVATION_BYTES: u16 = 1;
        /// The catalog partition for this kind is full.
        pub const CATALOG_CAPACITY: u16 = 2;
        /// Reserved and never emitted.
        pub const RETAINED_PREVIOUS: u16 = 3;
    }
    /// `checksumFailure`.
    pub mod checksum {
        /// The whole-payload CRC did not match.
        pub const WHOLE_PAYLOAD: u16 = 1;
        /// The durable prefix CRC did not match on resume.
        pub const DURABLE_PREFIX: u16 = 2;
        /// A cursor's CRC did not match.
        pub const CURSOR: u16 = 3;
    }
    /// `semanticValidation` in namespace zero — the device-control plane's only semantic refusal.
    pub mod semantic_common {
        /// A trusted clock would move backwards (§16, `SetClock`).
        pub const CLOCK_REGRESSION: u16 = 1;
    }
    /// `mediaUnavailable`.
    pub mod media {
        /// No card is present.
        pub const NO_CARD: u16 = 1;
        /// The volume is not mounted or its filesystem is unsupported.
        pub const UNMOUNTED: u16 = 2;
        /// The store mounted recovery-failed read-only.
        pub const RECOVERY_READ_ONLY: u16 = 3;
    }
    /// `mediaIo`.
    pub mod media_io {
        /// A read failed.
        pub const READ: u16 = 1;
        /// A write failed.
        pub const WRITE: u16 = 2;
        /// A synchronize failed.
        pub const SYNCHRONIZE: u16 = 3;
        /// The device cannot determine whether its claim reached durable storage.
        pub const UNCERTAIN_COMMIT: u16 = 4;
    }
    /// `cancelled`.
    pub mod cancelled {
        /// The client cancelled it.
        pub const CLIENT_CANCELLED: u16 = 1;
        /// A newer request superseded it.
        pub const SUPERSEDED: u16 = 2;
        /// The rider asked for it.
        pub const USER_REQUESTED: u16 = 3;
        /// The work expired under reclamation pressure.
        pub const WORK_EXPIRED: u16 = 4;
    }
    /// `linkLost`.
    pub mod link {
        /// The control channel went away.
        pub const CONTROL: u16 = 1;
        /// The stream channel went away.
        pub const STREAM: u16 = 2;
    }
    /// `operationIdConflict`.
    pub mod conflict {
        /// The stored intent digest differs from this request's.
        pub const INTENT_DIGEST: u16 = 1;
    }
    /// `resourceLimit`.
    pub mod resource {
        /// The link cannot carry the 192-byte control minimum.
        pub const MINIMUM_CONTROL_FRAME: u16 = 1;
        /// The link cannot carry the 64-byte stream minimum.
        pub const MINIMUM_STREAM_FRAME: u16 = 2;
        /// The declared length exceeds the kind's maximum.
        pub const OBJECT_LENGTH: u16 = 3;
        /// The compiled normal-claim capacity is exhausted.
        pub const NORMAL_OPERATION_CLAIMS: u16 = 4;
        /// The compiled upload work-slot capacity is exhausted.
        pub const UPLOAD_WORK_SLOTS: u16 = 5;
        /// Reserved and never emitted: a second parent is `busy/draftParents`.
        pub const DRAFT_PARENTS: u16 = 6;
        /// The declared part count exceeds the advertised maximum.
        pub const DRAFT_PARTS: u16 = 7;
        /// One manifest references more children than the device allows.
        pub const MANIFEST_CHILDREN: u16 = 8;
        /// The compiled lease capacity is exhausted.
        pub const READER_LEASES: u16 = 9;
        /// The kind's catalog head capacity is exhausted.
        pub const CATALOG_HEADS: u16 = 10;
        /// Too many map data files would be mounted at once.
        pub const MOUNTED_FILES: u16 = 11;
        /// Reserved and never emitted: an occupied ride slot is `busy/rideSlot`.
        pub const RIDE_SLOT: u16 = 12;
    }
    /// `catalogChanged`.
    pub mod changed {
        /// The catalog snapshot moved.
        pub const CATALOG_SNAPSHOT: u16 = 1;
        /// The draft snapshot moved.
        pub const DRAFT_SNAPSHOT: u16 = 2;
        /// Reserved and never emitted: capabilities are immutable within a connection generation.
        pub const CAPABILITY_SNAPSHOT: u16 = 3;
    }
    /// `internal`.
    pub mod internal {
        /// An invariant failed.
        pub const INVARIANT: u16 = 1;
        /// A codec fault.
        pub const CODEC: u16 = 2;
        /// Recovery could not reconcile.
        pub const RECOVERY_RECONCILIATION: u16 = 3;
    }
}

/// One registered common-namespace detail row, for coverage tests and fixture naming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DetailRow {
    /// The owning category.
    pub category: ErrorCategory,
    /// The code inside that category.
    pub code: u16,
    /// The registry's name.
    pub name: &'static str,
    /// True for a row registered only so its number stays burned (§12's reserved table).
    pub reserved: bool,
}

const fn row(category: ErrorCategory, code: u16, name: &'static str) -> DetailRow {
    DetailRow { category, code, name, reserved: false }
}

const fn reserved_row(category: ErrorCategory, code: u16, name: &'static str) -> DetailRow {
    DetailRow { category, code, name, reserved: true }
}

/// The complete common-namespace detail registry of §12, in category order.
pub const DETAIL_REGISTRY: [DetailRow; 79] = [
    row(ErrorCategory::INCOMPATIBLE_VERSION, 1, "unsupportedMajor"),
    row(ErrorCategory::INCOMPATIBLE_VERSION, 2, "unsupportedMinor"),
    row(ErrorCategory::UNSUPPORTED_CAPABILITY, 1, "opcode"),
    row(ErrorCategory::UNSUPPORTED_CAPABILITY, 2, "logicalKind"),
    row(ErrorCategory::UNSUPPORTED_CAPABILITY, 3, "draftPartKind"),
    row(ErrorCategory::UNSUPPORTED_CAPABILITY, 4, "feature"),
    row(ErrorCategory::UNSUPPORTED_CAPABILITY, 5, "schemaVersion"),
    row(ErrorCategory::UNSUPPORTED_CAPABILITY, 6, "nonCancellableOperation"),
    row(ErrorCategory::AUTHENTICATION_FAILED, 1, "missingCredential"),
    row(ErrorCategory::AUTHENTICATION_FAILED, 2, "invalidCredential"),
    row(ErrorCategory::AUTHENTICATION_FAILED, 3, "expiredCredential"),
    row(ErrorCategory::AUTHORIZATION_FAILED, 1, "principalScope"),
    row(ErrorCategory::AUTHORIZATION_FAILED, 2, "operationOwner"),
    row(ErrorCategory::AUTHORIZATION_FAILED, 3, "domainRead"),
    row(ErrorCategory::AUTHORIZATION_FAILED, 4, "domainWrite"),
    row(ErrorCategory::AUTHORIZATION_FAILED, 5, "installAuthority"),
    row(ErrorCategory::AUTHORIZATION_FAILED, 6, "deviceControl"),
    row(ErrorCategory::BUSY, 1, "heavyTransfer"),
    row(ErrorCategory::BUSY, 2, "normalOperationClaims"),
    row(ErrorCategory::BUSY, 3, "uploadWorkSlots"),
    row(ErrorCategory::BUSY, 4, "draftParents"),
    reserved_row(ErrorCategory::BUSY, 5, "draftParts"),
    row(ErrorCategory::BUSY, 6, "readerLeases"),
    row(ErrorCategory::BUSY, 7, "maintenanceCancellationRecoveryClaim"),
    reserved_row(ErrorCategory::BUSY, 8, "maintenance"),
    row(ErrorCategory::BUSY, 9, "rideSlot"),
    reserved_row(ErrorCategory::BUSY, 10, "retainedPrevious"),
    row(ErrorCategory::INVALID_FRAME, 1, "malformedHeader"),
    row(ErrorCategory::INVALID_FRAME, 2, "recordLength"),
    row(ErrorCategory::INVALID_FRAME, 3, "magic"),
    row(ErrorCategory::INVALID_FRAME, 4, "payloadLength"),
    row(ErrorCategory::INVALID_FRAME, 5, "frameBounds"),
    row(ErrorCategory::INVALID_FRAME, 6, "truncated"),
    row(ErrorCategory::INVALID_FRAME, 7, "trailingBytes"),
    row(ErrorCategory::INVALID_DESCRIPTOR, 1, "reservedBits"),
    row(ErrorCategory::INVALID_DESCRIPTOR, 2, "unknownEnum"),
    row(ErrorCategory::INVALID_DESCRIPTOR, 3, "invalidCombination"),
    row(ErrorCategory::INVALID_DESCRIPTOR, 4, "nestedLength"),
    row(ErrorCategory::INVALID_DESCRIPTOR, 5, "noncanonicalMetadata"),
    row(ErrorCategory::INVALID_DESCRIPTOR, 6, "duplicateField"),
    row(ErrorCategory::INVALID_DESCRIPTOR, 7, "outOfOrderField"),
    row(ErrorCategory::INVALID_DESCRIPTOR, 8, "unsupportedFlags"),
    row(ErrorCategory::INVALID_DESCRIPTOR, 9, "zeroRequestId"),
    row(ErrorCategory::INVALID_DESCRIPTOR, 10, "emptyMetadataPatch"),
    row(ErrorCategory::INVALID_OFFSET, 1, "unexpectedOffset"),
    row(ErrorCategory::INVALID_OFFSET, 2, "checkpointBoundary"),
    row(ErrorCategory::INVALID_SESSION, 1, "unknown"),
    row(ErrorCategory::INVALID_SESSION, 2, "staleConnection"),
    row(ErrorCategory::INVALID_SESSION, 3, "wrongPrincipal"),
    row(ErrorCategory::INVALID_SESSION, 4, "wrongLink"),
    row(ErrorCategory::INVALID_SESSION, 5, "wrongDirection"),
    row(ErrorCategory::OBJECT_NOT_FOUND, 1, "logicalObject"),
    reserved_row(ErrorCategory::OBJECT_NOT_FOUND, 2, "requestedRevision"),
    row(ErrorCategory::OBJECT_NOT_FOUND, 3, "draftParentUnknown"),
    row(ErrorCategory::OBJECT_NOT_FOUND, 4, "operationTerminal"),
    reserved_row(ErrorCategory::OBJECT_NOT_FOUND, 5, "resumableWork"),
    row(ErrorCategory::OBJECT_NOT_FOUND, 6, "weatherRequestContext"),
    row(ErrorCategory::REVISION_CONFLICT, 1, "object"),
    row(ErrorCategory::REVISION_CONFLICT, 2, "repository"),
    row(ErrorCategory::REVISION_CONFLICT, 3, "singleton"),
    row(ErrorCategory::INSUFFICIENT_SPACE, 1, "reservationBytes"),
    row(ErrorCategory::INSUFFICIENT_SPACE, 2, "catalogCapacity"),
    reserved_row(ErrorCategory::INSUFFICIENT_SPACE, 3, "retainedPrevious"),
    row(ErrorCategory::CHECKSUM_FAILURE, 1, "wholePayload"),
    row(ErrorCategory::CHECKSUM_FAILURE, 2, "durablePrefix"),
    row(ErrorCategory::CHECKSUM_FAILURE, 3, "cursor"),
    row(ErrorCategory::SEMANTIC_VALIDATION, 1, "clockRegression"),
    row(ErrorCategory::MEDIA_UNAVAILABLE, 1, "noCard"),
    row(ErrorCategory::MEDIA_UNAVAILABLE, 2, "unmounted"),
    row(ErrorCategory::MEDIA_UNAVAILABLE, 3, "recoveryReadOnly"),
    row(ErrorCategory::MEDIA_IO, 1, "read"),
    row(ErrorCategory::MEDIA_IO, 2, "write"),
    row(ErrorCategory::MEDIA_IO, 3, "synchronize"),
    row(ErrorCategory::MEDIA_IO, 4, "uncertainCommit"),
    row(ErrorCategory::CANCELLED, 1, "clientCancelled"),
    row(ErrorCategory::CANCELLED, 2, "superseded"),
    row(ErrorCategory::CANCELLED, 3, "userRequested"),
    row(ErrorCategory::CANCELLED, 4, "workExpired"),
    row(ErrorCategory::LINK_LOST, 1, "control"),
];

/// The remainder of the registry, split only because a Rust array's length is part of its type.
pub const DETAIL_REGISTRY_TAIL: [DetailRow; 21] = [
    row(ErrorCategory::LINK_LOST, 2, "stream"),
    row(ErrorCategory::OPERATION_ID_CONFLICT, 1, "intentDigest"),
    row(ErrorCategory::RESOURCE_LIMIT, 1, "minimumControlFrame"),
    row(ErrorCategory::RESOURCE_LIMIT, 2, "minimumStreamFrame"),
    row(ErrorCategory::RESOURCE_LIMIT, 3, "objectLength"),
    row(ErrorCategory::RESOURCE_LIMIT, 4, "normalOperationClaims"),
    row(ErrorCategory::RESOURCE_LIMIT, 5, "uploadWorkSlots"),
    reserved_row(ErrorCategory::RESOURCE_LIMIT, 6, "draftParents"),
    row(ErrorCategory::RESOURCE_LIMIT, 7, "draftParts"),
    row(ErrorCategory::RESOURCE_LIMIT, 8, "manifestChildren"),
    row(ErrorCategory::RESOURCE_LIMIT, 9, "readerLeases"),
    row(ErrorCategory::RESOURCE_LIMIT, 10, "catalogHeads"),
    row(ErrorCategory::RESOURCE_LIMIT, 11, "mountedFiles"),
    reserved_row(ErrorCategory::RESOURCE_LIMIT, 12, "rideSlot"),
    row(ErrorCategory::CATALOG_CHANGED, 1, "catalogSnapshot"),
    row(ErrorCategory::CATALOG_CHANGED, 2, "draftSnapshot"),
    reserved_row(ErrorCategory::CATALOG_CHANGED, 3, "capabilitySnapshot"),
    row(ErrorCategory::INTERNAL, 1, "invariant"),
    row(ErrorCategory::INTERNAL, 2, "codec"),
    row(ErrorCategory::INTERNAL, 3, "recoveryReconciliation"),
    // `semanticValidation` in an ObjectKind namespace is the registries' table, not this one; the
    // row above at code 1 is its namespace-zero device-control counterpart.
    row(ErrorCategory::SEMANTIC_VALIDATION, 0, "noNarrowerFact"),
];

/// The whole common-namespace detail registry as one iterator.
pub fn detail_registry() -> impl Iterator<Item = DetailRow> {
    DETAIL_REGISTRY.into_iter().chain(DETAIL_REGISTRY_TAIL)
}

/// Names a `(category, namespace, detail)` triple for fixtures and diagnostics.
///
/// Detail zero always means "no narrower fact" (§12). A `semanticValidation` detail in a nonzero
/// namespace is looked up in the registries' §6 table, which is where domains own their codes.
pub fn detail_name(category: ErrorCategory, namespace: u16, detail: u16) -> &'static str {
    if detail == 0 {
        return "none";
    }
    if category == ErrorCategory::SEMANTIC_VALIDATION && namespace != 0 {
        return match ObjectKind::from_u16(namespace).and_then(|kind| crate::registry::semantic::lookup(kind, detail)) {
            Some(row) => row.name,
            None => "unknown",
        };
    }
    match detail_registry().find(|row| row.category == category && row.code == detail) {
        Some(row) => row.name,
        None => "unknown",
    }
}

/// A refusal produced by this crate's decoders, in the contract's own vocabulary.
///
/// The category is what a device would answer with; the detail narrows it. Nothing here carries
/// presence bits or guidance, because a codec cannot know whether a claim exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeError {
    /// The §12 category.
    pub category: ErrorCategory,
    /// The category-scoped detail code, or `0` for no narrower fact.
    pub detail: u16,
}

impl DecodeError {
    /// Builds a refusal from a category and a category-scoped detail.
    pub const fn new(category: ErrorCategory, detail: u16) -> Self {
        DecodeError { category, detail }
    }

    /// `invalidFrame` with the given detail — a record that is not one complete frame.
    pub const fn invalid_frame(detail: u16) -> Self {
        DecodeError::new(ErrorCategory::INVALID_FRAME, detail)
    }

    /// `invalidDescriptor` with the given detail — a complete frame with an illegal field.
    pub const fn invalid_descriptor(detail: u16) -> Self {
        DecodeError::new(ErrorCategory::INVALID_DESCRIPTOR, detail)
    }

    /// `invalidDescriptor/reservedBits`.
    pub const fn reserved_bits() -> Self {
        DecodeError::invalid_descriptor(detail::descriptor::RESERVED_BITS)
    }

    /// `invalidDescriptor/unknownEnum`.
    pub const fn unknown_enum() -> Self {
        DecodeError::invalid_descriptor(detail::descriptor::UNKNOWN_ENUM)
    }

    /// `invalidDescriptor/invalidCombination`.
    pub const fn invalid_combination() -> Self {
        DecodeError::invalid_descriptor(detail::descriptor::INVALID_COMBINATION)
    }

    /// `invalidDescriptor/unsupportedFlags`.
    pub const fn unsupported_flags() -> Self {
        DecodeError::invalid_descriptor(detail::descriptor::UNSUPPORTED_FLAGS)
    }

    /// `invalidFrame/truncated`.
    pub const fn truncated() -> Self {
        DecodeError::invalid_frame(detail::frame::TRUNCATED)
    }

    /// `invalidFrame/trailingBytes`.
    pub const fn trailing_bytes() -> Self {
        DecodeError::invalid_frame(detail::frame::TRAILING_BYTES)
    }

    /// `unsupportedCapability` with the given detail.
    pub const fn unsupported_capability(detail: u16) -> Self {
        DecodeError::new(ErrorCategory::UNSUPPORTED_CAPABILITY, detail)
    }

    /// `incompatibleVersion` with the given detail.
    pub const fn incompatible_version(detail: u16) -> Self {
        DecodeError::new(ErrorCategory::INCOMPATIBLE_VERSION, detail)
    }

    /// The pair as it would be named in a fixture, in the common namespace.
    pub fn name(self) -> (&'static str, &'static str) {
        (self.category.name(), detail_name(self.category, 0, self.detail))
    }

    /// Exactly the length check every fixed-size body starts with: `len` bytes, no more, no less.
    ///
    /// Short is `invalidFrame/truncated` and long is `invalidFrame/trailingBytes`, which is §2.1's
    /// rule that "a frame that carries a byte past the end of its stated layout is `invalidFrame`
    /// for the trailing bytes it contains".
    pub(crate) fn exact_len(payload: &[u8], len: usize) -> crate::Result<()> {
        match payload.len() {
            n if n < len => Err(DecodeError::truncated()),
            n if n > len => Err(DecodeError::trailing_bytes()),
            _ => Ok(()),
        }
    }

    /// The same, for a body with a variable tail: at least `len` bytes.
    pub(crate) fn min_len(payload: &[u8], len: usize) -> crate::Result<()> {
        if payload.len() < len {
            Err(DecodeError::truncated())
        } else {
            Ok(())
        }
    }
}

/// The `ErrorBody` of §12: a 48-byte prefix and optional non-authoritative diagnostic text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ErrorBody<'a> {
    /// The machine-readable category.
    pub category: ErrorCategory,
    /// `0` for the common namespace, or the affected `ObjectKind` for `semanticValidation`.
    pub detail_namespace: u16,
    /// The category-scoped detail, or `0` for no narrower fact.
    pub detail: u16,
    /// What the client should do next.
    pub guidance: RetryGuidance,
    /// The owner class of a `busy` refusal.
    pub owner: Owner,
    /// Which of the optional fields below are meaningful, plus the two claim-status bits.
    pub presence: u16,
    /// Milliseconds to wait, meaningful with [`presence::RETRY_DELAY`].
    pub retry_after_ms: u32,
    /// The offset to resume at, meaningful with [`presence::EXPECTED_OFFSET`].
    pub expected_offset: u64,
    /// The authoritative current revision, meaningful with [`presence::CURRENT_REVISION`].
    pub current_revision: Revision,
    /// Bytes the operation needs, meaningful with [`presence::REQUIRED_BYTES`].
    pub required_bytes: u64,
    /// Bytes the device has, meaningful with [`presence::AVAILABLE_BYTES`].
    pub available_bytes: u64,
    /// Non-authoritative diagnostic text, at most 64 encoded bytes.
    ///
    /// It is **not** validated as UTF-8: §12 requires a receiver to render it lossily rather than
    /// reject the only report of a real failure. Use [`text_lossy_is_clean`](Self::text_lossy_is_clean)
    /// to ask whether it needs replacing before display.
    pub text: &'a [u8],
}

impl<'a> ErrorBody<'a> {
    /// A text-free body with every optional field absent — the shape §5.2 requires a device to be
    /// able to send before negotiation, and the shape §11 requires of a retained Aborted replay.
    pub const fn bare(category: ErrorCategory, detail: u16, guidance: RetryGuidance) -> Self {
        ErrorBody {
            category,
            detail_namespace: 0,
            detail,
            guidance,
            owner: Owner::NONE,
            presence: 0,
            retry_after_ms: 0,
            expected_offset: 0,
            current_revision: Revision::ZERO,
            required_bytes: 0,
            available_bytes: 0,
            text: &[],
        }
    }

    /// The exact encoded length: 48 plus the text.
    pub fn encoded_len(&self) -> usize {
        ERROR_BODY_PREFIX_LEN + self.text.len()
    }

    /// True when the durable-claim-exists bit is set.
    pub fn durable_claim_exists(&self) -> bool {
        self.presence & presence::DURABLE_CLAIM_EXISTS != 0
    }

    /// True when the claim-is-terminal bit is set. Meaningful only alongside
    /// [`durable_claim_exists`](Self::durable_claim_exists), and the decoder has already rejected
    /// the combination where it is not.
    pub fn claim_is_terminal(&self) -> bool {
        self.presence & presence::CLAIM_IS_TERMINAL != 0
    }

    /// True when the text is already valid, control-free, noncharacter-free UTF-8 and can be shown
    /// as-is. False means render it lossily — never reject the body over it.
    pub fn text_lossy_is_clean(&self) -> bool {
        crate::metadata::text_is_clean(self.text)
    }

    /// Decodes a body from a control payload.
    pub fn decode(payload: &'a [u8]) -> crate::Result<Self> {
        DecodeError::min_len(payload, ERROR_BODY_PREFIX_LEN)?;
        let category = ErrorCategory::from_u16(u16_at(payload, 0)).ok_or_else(DecodeError::unknown_enum)?;
        let presence = u16_at(payload, 8);
        if presence & !presence::ALL != 0 {
            return Err(DecodeError::reserved_bits());
        }
        if presence & presence::CLAIM_IS_TERMINAL != 0 && presence & presence::DURABLE_CLAIM_EXISTS == 0 {
            // §12: bit 6 is "meaningful only when bit 5 is set". A body claiming a terminal claim
            // that does not exist is malformed, and the vectors pin it as a rejection.
            return Err(DecodeError::invalid_combination());
        }
        let text_len = usize::from(payload[46]);
        if text_len > MAX_ERROR_TEXT {
            return Err(DecodeError::invalid_frame(detail::frame::PAYLOAD_LENGTH));
        }
        if payload[47] != 0 {
            return Err(DecodeError::reserved_bits());
        }
        if payload.len() != ERROR_BODY_PREFIX_LEN + text_len {
            // §12: "Only the text length field is structural: a length above 64, or a length that
            // disagrees with the frame's payload length, is `invalidFrame` as usual."
            return Err(DecodeError::invalid_frame(detail::frame::PAYLOAD_LENGTH));
        }
        Ok(ErrorBody {
            category,
            detail_namespace: u16_at(payload, 2),
            detail: u16_at(payload, 4),
            guidance: RetryGuidance::from_u8(payload[6]),
            owner: Owner::from_u8(payload[7]),
            presence,
            retry_after_ms: u32_at(payload, 10),
            expected_offset: u64_at(payload, 14),
            current_revision: Revision::new(u64_at(payload, 22)),
            required_bytes: u64_at(payload, 30),
            available_bytes: u64_at(payload, 38),
            text: &payload[ERROR_BODY_PREFIX_LEN..],
        })
    }

    /// Encodes the body into `out`, returning its exact length.
    pub fn encode_into(&self, out: &mut [u8]) -> EncodeResult {
        let needed = self.encoded_len();
        if self.text.len() > MAX_ERROR_TEXT {
            // Unreachable through the decoder; a hand-built body with over-long text has no legal
            // encoding, and reporting it as "needs more room" is the honest answer.
            return Err(BufferTooSmall { needed: MAX_ERROR_BODY_LEN, available: out.len() });
        }
        if out.len() < needed {
            return Err(BufferTooSmall { needed, available: out.len() });
        }
        let out = &mut out[..needed];
        out.fill(0);
        put_u16(out, 0, self.category.get());
        put_u16(out, 2, self.detail_namespace);
        put_u16(out, 4, self.detail);
        out[6] = self.guidance.get();
        out[7] = self.owner.get();
        put_u16(out, 8, self.presence);
        put_u32(out, 10, self.retry_after_ms);
        put_u64(out, 14, self.expected_offset);
        put_u64(out, 22, self.current_revision.get());
        put_u64(out, 30, self.required_bytes);
        put_u64(out, 38, self.available_bytes);
        out[46] = self.text.len() as u8;
        put_bytes(out, ERROR_BODY_PREFIX_LEN, self.text);
        Ok(needed)
    }
}

/// The 24-byte compact fault body carried by a stream status frame is in [`crate::stream`]; this
/// helper is what both it and `ErrorBody` use to reject a nonzero reserved run.
pub(crate) fn reject_nonzero(bytes: &[u8], off: usize, len: usize) -> crate::Result<()> {
    if is_zero(bytes, off, len) {
        Ok(())
    } else {
        Err(DecodeError::reserved_bits())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;

    fn sample() -> ErrorBody<'static> {
        ErrorBody {
            category: ErrorCategory::INSUFFICIENT_SPACE,
            detail_namespace: 0,
            detail: detail::space::RESERVATION_BYTES,
            guidance: RetryGuidance::RETRY_AFTER_USER_ACTION,
            owner: Owner::NONE,
            presence: presence::REQUIRED_BYTES | presence::AVAILABLE_BYTES,
            retry_after_ms: 0,
            expected_offset: 0,
            current_revision: Revision::ZERO,
            required_bytes: 4_194_304,
            available_bytes: 1_048_576,
            text: b"not enough room",
        }
    }

    #[test]
    fn round_trips_byte_exactly() {
        let body = sample();
        let mut out = [0u8; MAX_ERROR_BODY_LEN];
        let len = body.encode_into(&mut out).unwrap();
        assert_eq!(len, 48 + 15);
        let decoded = ErrorBody::decode(&out[..len]).unwrap();
        assert_eq!(decoded, body);
        let mut again = [0u8; MAX_ERROR_BODY_LEN];
        let again_len = decoded.encode_into(&mut again).unwrap();
        assert_eq!(&again[..again_len], &out[..len]);
    }

    #[test]
    fn category_zero_is_a_malformed_body() {
        let mut bytes = [0u8; ERROR_BODY_PREFIX_LEN];
        assert_eq!(ErrorBody::decode(&bytes).unwrap_err(), DecodeError::unknown_enum());
        put_u16(&mut bytes, 0, 22);
        assert!(ErrorBody::decode(&bytes).is_ok());
    }

    #[test]
    fn an_unregistered_category_is_preserved_rather_than_rejected() {
        let mut bytes = [0u8; ERROR_BODY_PREFIX_LEN];
        put_u16(&mut bytes, 0, 900);
        let body = ErrorBody::decode(&bytes).unwrap();
        assert_eq!(body.category.get(), 900);
        assert_eq!(body.category.name(), "unknown");
        assert!(!body.category.is_registered());
    }

    #[test]
    fn terminal_bit_without_claim_bit_is_malformed() {
        let mut bytes = [0u8; ERROR_BODY_PREFIX_LEN];
        put_u16(&mut bytes, 0, ErrorCategory::CANCELLED.get());
        put_u16(&mut bytes, 8, presence::CLAIM_IS_TERMINAL);
        assert_eq!(ErrorBody::decode(&bytes).unwrap_err(), DecodeError::invalid_combination());
        put_u16(&mut bytes, 8, presence::CLAIM_IS_TERMINAL | presence::DURABLE_CLAIM_EXISTS);
        assert!(ErrorBody::decode(&bytes).is_ok());
    }

    #[test]
    fn presence_bits_above_six_are_reserved() {
        let mut bytes = [0u8; ERROR_BODY_PREFIX_LEN];
        put_u16(&mut bytes, 0, ErrorCategory::BUSY.get());
        put_u16(&mut bytes, 8, 1 << 7);
        assert_eq!(ErrorBody::decode(&bytes).unwrap_err(), DecodeError::reserved_bits());
    }

    #[test]
    fn text_length_is_the_only_structural_thing_about_text() {
        // Invalid UTF-8 decodes; §12 requires lossy rendering, never rejection.
        let mut bytes = vec![0u8; ERROR_BODY_PREFIX_LEN + 3];
        put_u16(&mut bytes, 0, ErrorCategory::INTERNAL.get());
        bytes[46] = 3;
        bytes[48] = 0xff;
        bytes[49] = 0xfe;
        bytes[50] = 0xfd;
        let body = ErrorBody::decode(&bytes).unwrap();
        assert_eq!(body.text, &[0xff, 0xfe, 0xfd]);
        assert!(!body.text_lossy_is_clean());

        // A length above 64 is structural.
        let mut over = vec![0u8; ERROR_BODY_PREFIX_LEN + 65];
        put_u16(&mut over, 0, ErrorCategory::INTERNAL.get());
        over[46] = 65;
        assert_eq!(ErrorBody::decode(&over).unwrap_err().category, ErrorCategory::INVALID_FRAME);

        // So is a length that disagrees with the payload.
        let mut mismatch = vec![0u8; ERROR_BODY_PREFIX_LEN + 2];
        put_u16(&mut mismatch, 0, ErrorCategory::INTERNAL.get());
        mismatch[46] = 3;
        assert_eq!(ErrorBody::decode(&mismatch).unwrap_err().category, ErrorCategory::INVALID_FRAME);
    }

    #[test]
    fn an_optional_field_present_where_the_category_wants_none_still_decodes() {
        // §12: the presence matrix binds senders only. This is a retained Aborted replay's shape —
        // `busy` with every presence bit clear except the two claim-status bits.
        let mut bytes = [0u8; ERROR_BODY_PREFIX_LEN];
        put_u16(&mut bytes, 0, ErrorCategory::BUSY.get());
        put_u16(&mut bytes, 4, detail::busy::HEAVY_TRANSFER);
        put_u16(&mut bytes, 8, presence::DURABLE_CLAIM_EXISTS | presence::CLAIM_IS_TERMINAL);
        let body = ErrorBody::decode(&bytes).unwrap();
        assert_eq!(body.owner, Owner::NONE);
        assert_eq!(body.guidance, RetryGuidance::REJECT_PERMANENTLY);
        assert!(body.claim_is_terminal());
    }

    #[test]
    fn the_detail_registry_names_every_row_once() {
        let mut seen = std::collections::BTreeSet::new();
        for row in detail_registry() {
            assert!(seen.insert((row.category, row.code)), "duplicate {}/{}", row.category.name(), row.name);
        }
        assert_eq!(detail_name(ErrorCategory::BUSY, 0, detail::busy::DRAFT_PARENTS), "draftParents");
        assert_eq!(detail_name(ErrorCategory::BUSY, 0, 0), "none");
        assert_eq!(detail_name(ErrorCategory::SEMANTIC_VALIDATION, 4, 5), "requestMismatch");
        assert_eq!(detail_name(ErrorCategory::SEMANTIC_VALIDATION, 0, 1), "clockRegression");
    }
}
