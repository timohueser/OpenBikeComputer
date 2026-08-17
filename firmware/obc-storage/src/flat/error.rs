//! The seam's refusal ([`StoreError`], `FLAT_Store_Protocol.md` §2) and the codecs' refusal
//! ([`DecodeError`]).
//!
//! The two are deliberately separate. `StoreError` is what a caller above the seam sees, and it
//! names no record and no byte; `DecodeError` is what a record codec returns, and it names the
//! record shape that refused and the rule that refused it — the only way a mount can tell a torn
//! gate from a mis-sorted entry array while it is choosing a catalog copy. Decoding is **total**:
//! every input either decodes or produces one of these, and nothing panics on hostile bytes.

/// What an operation at the store seam fails with. `FLAT_Store_Protocol.md` §2, verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreError {
    NotFound,
    RevisionConflict { current: Revision },
    NoSpace { required: u64 },
    TooFragmented,
    CatalogFull,
    Invalid,
    Media,
    ReadOnly,
}

use super::seam::Revision;

/// The record shape that refused an input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Record {
    /// The superblock body (§4).
    Superblock,
    /// A catalog header (§5.2).
    CatalogHeader,
    /// A catalog entry (§5.3).
    Entry,
    /// A catalog gate sector (§5.4).
    Gate,
    /// A ride journal slot header (§7.1).
    Slot,
}

/// The rule that refused an input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The caller offered fewer bytes than the record's fixed size.
    Length,
    /// The leading four-byte magic is not this record's.
    Magic,
    /// The format version is not the one this build knows.
    Version,
    /// The entry stride the header declares is not 128.
    Stride,
    /// A reserved run is nonzero.
    Reserved,
    /// The record's own CRC does not cover its bytes.
    Crc,
    /// The gate's copy index or the slot's index does not match where it was read from.
    Position,
    /// The record's `StoreId` is not the superblock's.
    StoreId,
    /// A count is above the capacity its section fixes, or two counts that must agree do not.
    Count,
    /// An identity field that must be nonzero is zero.
    Zero,
    /// An unknown nonzero enum: a kind §3.1 does not register, or a flag bit §5.3 does not define.
    UnknownEnum,
    /// Entries are not strictly ascending by `(ObjectId, Revision)`.
    Order,
    /// A rule about the entries of one `ObjectId` — kind agreement, the retained/head pair, or the
    /// one `RECORDING` entry — is violated.
    Revisions,
    /// An extent range is empty, leaves the extent area, or does not cover the payload.
    Ranges,
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
    pub const fn new(record: Record, reason: Reason) -> Self {
        DecodeError { record, reason }
    }
}

/// Result alias for the format's total decoders.
pub type Result<T> = core::result::Result<T, DecodeError>;
