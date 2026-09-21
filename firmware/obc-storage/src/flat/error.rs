//! The seam's refusal ([`StoreError`]) and the codecs' refusal ([`DecodeError`]).
//!
//! The two are deliberately separate. `StoreError` names no record and no byte; `DecodeError` names
//! the record shape that refused and the rule that refused it, which is the only way a mount can tell
//! a torn gate from a mis-sorted entry array while it is choosing a catalog copy. Decoding is total:
//! every input either decodes or produces one of these, and nothing panics on hostile bytes.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreError {
    NotFound,
    RevisionConflict {
        current: Revision,
    },
    NoSpace {
        required: u64,
    },
    TooFragmented,
    CatalogFull,
    Invalid,
    Media,
    ReadOnly,
    /// Try again: every hold row is taken, so there is no slot to resolve this open into. A full hold
    /// table is a transient property of who else is reading right now, unlike
    /// [`Invalid`](StoreError::Invalid), which means the request will be wrong next time too.
    ///
    /// Wire mapping: code `9` `busy`, detail `holds 2`, and no context — a table-full open names no
    /// live request.
    Busy,
}

use super::seam::Revision;

/// The record shape that refused an input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Record {
    Superblock,
    CatalogHeader,
    Entry,
    Gate,
    Slot,
}

/// The rule that refused an input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Length,
    Magic,
    Version,
    /// The entry stride the header declares is not 128.
    Stride,
    /// A reserved run is nonzero.
    Reserved,
    Crc,
    /// The gate's copy index or the slot's index does not match where it was read from.
    Position,
    StoreId,
    /// A count is above the capacity its section fixes, or two counts that must agree do not.
    Count,
    /// An identity field that must be nonzero is zero.
    Zero,
    /// An unknown nonzero enum: an unregistered kind, or an undefined entry flag bit.
    UnknownEnum,
    /// Entries are not strictly ascending by `(ObjectId, Revision)`.
    Order,
    /// A rule about the entries of one `ObjectId` — kind agreement, the retained/head pair, or the
    /// one `RECORDING` entry — is violated.
    Revisions,
    /// An extent range is empty, leaves the extent area, or does not cover the payload.
    Ranges,
    /// The extent size the superblock records is outside the admitted range. A card whose geometry
    /// does not decode has no addresses, so this refuses the superblock rather than mounting a store
    /// read-only.
    Geometry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeError {
    pub record: Record,
    pub reason: Reason,
}

impl DecodeError {
    pub const fn new(record: Record, reason: Reason) -> Self {
        DecodeError { record, reason }
    }
}

pub type Result<T> = core::result::Result<T, DecodeError>;
