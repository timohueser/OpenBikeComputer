//! The one typed refusal every OBC2 decoder returns.
//!
//! `OBC2_Storage_Format.md` §1 fixes what a decoder must refuse before it uses any derived offset:
//! "arithmetic overflow, duplicate keys, out-of-order entries, an unknown nonzero tag, or a count
//! above its stated capacity". §4 adds that "a gate that fails any of those checks is invalid;
//! there is no partially valid gate and no repair path". So decoding here is **total**: every
//! input either decodes or produces one of these, and nothing panics on hostile bytes.
//!
//! The pair is deliberately two enums rather than one flat list. [`Record`] says which record shape
//! refused, which is the only way a caller can tell a torn journal slot from a torn WORK slot when
//! recovery is walking both; [`Reason`] says which rule refused, which is what a vector's negative
//! twin pins. Nothing here carries an offset or a byte value: a decode failure is a fail-closed
//! fact, not a repair hint.

/// The record shape that refused an input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Record {
    /// The common 512-byte gate sector (§4).
    Gate,
    /// A catalog checkpoint body or header (§5).
    Checkpoint,
    /// A repository-state row (§5.3).
    RepositoryState,
    /// A catalog-head entry (§5.3).
    CatalogHead,
    /// An active-operation row (§5.3).
    ActiveOperation,
    /// A draft-parent row (§5.3).
    DraftParent,
    /// A draft-part row (§5.3).
    DraftPart,
    /// A retained-previous-generation entry (§5.3).
    RetainedPrevious,
    /// A terminal-result ring entry (§5.3).
    TerminalResult,
    /// The weather-request state (§5.3).
    WeatherState,
    /// The active-ride state (§5.3).
    ActiveRide,
    /// The 240-byte `HandoffRef` (§10).
    HandoffRef,
    /// A journal slot body (§6.1).
    JournalSlot,
    /// A journal mutation (§6.1).
    Mutation,
    /// A `WORK` slot body (§7).
    Work,
    /// A `RIDE.ACT` slot body (§7.1).
    RideSlot,
    /// An `ARM0.HND`/`ARM1.HND` body (§10).
    Handoff,
    /// The `INIT.REC` witness body (§12).
    Init,
    /// A resolution generation (§8).
    Resolution,
}

/// The rule that refused an input.
///
/// `Ord` is here for diagnostics only — a fuzz histogram keyed by reason, so a "reached a
/// structural rule" claim can be held to a per-rule floor rather than one aggregate count. Nothing
/// in the format orders these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reason {
    /// The caller offered fewer bytes than the record's fixed size, or a variable body's length
    /// disagrees with its own count field.
    Length,
    /// The leading four-byte magic is not this record's.
    Magic,
    /// The format version is not one this build knows.
    Version,
    /// The record's declared header length is not the constant its section fixes.
    HeaderLength,
    /// A reserved run — including a slot's pad to its 16,384-byte stride — is nonzero. §1:
    /// "Reserved bytes are written as zero and must be zero when read."
    Reserved,
    /// The stored body CRC does not cover the body bytes.
    BodyCrc,
    /// The gate's own CRC does not cover its 512 bytes.
    GateCrc,
    /// The gate's one's-complement copy of the body CRC is not exact (§4).
    Complement,
    /// The physical slot index does not match where the record was read from.
    SlotIndex,
    /// The gate's scope does not equal the body's (§4).
    Scope,
    /// The gate's logical sequence does not equal the body's, or a sequence rule is violated.
    Sequence,
    /// A header count exceeds its region capacity, or disagrees with the region's occupancy.
    Count,
    /// Occupied entries are not sorted by their stated key (§5.1).
    Order,
    /// Two entries share a key.
    Duplicate,
    /// An unknown nonzero tag: a state, phase, kind, reason or opcode this format does not register.
    UnknownEnum,
    /// An entry's `occupied` byte is neither absent-by-being-zero nor exactly `1`.
    Occupied,
    /// A removal carries a nonzero byte outside its key ranges (§6.1).
    KeyBytes,
    /// The record kind and its presence flags are a combination §6.1 does not admit.
    Combination,
    /// A derived offset or count would overflow, or a declared length exceeds its bound.
    Overflow,
}

/// A total decoder's refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeError {
    /// Which record shape refused.
    pub record: Record,
    /// Which rule refused it.
    pub reason: Reason,
}

impl DecodeError {
    /// Builds a refusal.
    pub const fn new(record: Record, reason: Reason) -> Self {
        DecodeError { record, reason }
    }
}

/// Result alias for the kernel's total decoders.
pub type Result<T> = core::result::Result<T, DecodeError>;

/// Why a decoded journal record could not be applied to a catalog projection.
///
/// This is deliberately not a [`DecodeError`]: the bytes were structurally valid and the failure is
/// about the *state* they were replayed against — a capacity §2 fixes, a sequence that is not the
/// contiguous successor, or a cursor rule §6.1 states. Recovery treats these as corruption exactly
/// as it treats an invalid record, but a caller that conflated them could not tell a torn slot from
/// a full table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyError {
    /// The record's StoreId is not the projection's.
    StoreId,
    /// The record's sequence is not `through_sequence + 1`.
    Sequence,
    /// The record's epoch is not the projection's.
    Epoch,
    /// A region is at the capacity §2 fixes. This is the kernel's explicit `ResourceLimit`.
    ResourceLimit(Record),
    /// A put or remove names a key the projection does not hold, or a put would duplicate one.
    MissingKey(Record),
    /// The next-GenerationId cursor is not the current cursor plus one (§6.1 bit 18).
    GenerationCursor,
    /// The appended terminal result's commit sequence is not the incremented terminal counter.
    TerminalCounter,
}
