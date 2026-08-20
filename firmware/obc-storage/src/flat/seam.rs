//! The store seam: `FLAT_Store_Protocol.md` §2's five operations, the two members beside them, and
//! the whole vocabulary they speak.
//!
//! Nothing here names a block, an extent, an LBA, a path or a filename. [`EntryMeta`] is the
//! metadata half of a catalog entry and [`Allocation`] is an opaque token that stands for reserved
//! extents; neither carries one, which is what makes that sentence true rather than aspirational.

use super::error::{Reason, Record, Result, StoreError};

/// The card. Drawn from the device CSPRNG at initialization; a new one means everything a client
/// cached is void.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreId(pub [u8; 16]);

/// Store-global, allocated from the catalog header's monotonic cursor, never reused. `0` is
/// reserved and names no object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ObjectId(pub u64);

/// Per object: `1` for the commit that creates it, `+1` for every commit that replaces it. Also the
/// compare-and-swap token every mutation carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Revision(pub u64);

impl ObjectId {
    /// The reserved id that names no object — what the wire's `PUT` sends to mean "create".
    pub const NONE: ObjectId = ObjectId(0);
}

/// `FLAT_Store_Format.md` §3.1, the sole authority for these values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum ObjectKind {
    Route = 1,
    Trip = 2,
    Ride = 3,
    WeatherBundle = 4,
    MapShard = 5,
    MapSetManifest = 6,
    UpdatePackage = 7,
    RollbackReserve = 8,
}

impl ObjectKind {
    /// Decodes §3.1's `u16`. Kind `0` is never encoded and no other value is registered, so
    /// anything else is corruption rather than a kind from the future: there is no format version
    /// in which this table grew without the version field saying so.
    pub fn decode(value: u16) -> Result<Self> {
        Ok(match value {
            1 => ObjectKind::Route,
            2 => ObjectKind::Trip,
            3 => ObjectKind::Ride,
            4 => ObjectKind::WeatherBundle,
            5 => ObjectKind::MapShard,
            6 => ObjectKind::MapSetManifest,
            7 => ObjectKind::UpdatePackage,
            8 => ObjectKind::RollbackReserve,
            _ => return Err(super::error::DecodeError::new(Record::Entry, Reason::UnknownEnum)),
        })
    }
}

/// §5.3's entry flags. Bits `3..15` are zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EntryFlags(u16);

impl EntryFlags {
    /// No flag.
    pub const NONE: EntryFlags = EntryFlags(0);
    /// The active ride. Payload length and CRC are the values of the last commit; the ride journal
    /// is authoritative for what is beyond them.
    pub const RECORDING: EntryFlags = EntryFlags(1 << 0);
    /// A non-head revision the store keeps alive on purpose.
    pub const RETAINED: EntryFlags = EntryFlags(1 << 1);
    /// The entry owns extents and the store does not write the payload.
    pub const RESERVED: EntryFlags = EntryFlags(1 << 2);

    const DEFINED: u16 = 0b111;

    pub fn decode(value: u16) -> Result<Self> {
        if value & !Self::DEFINED != 0 {
            return Err(super::error::DecodeError::new(Record::Entry, Reason::UnknownEnum));
        }
        Ok(EntryFlags(value))
    }

    pub fn bits(self) -> u16 {
        self.0
    }

    /// True when every flag of `other` is set here.
    pub fn has(self, other: EntryFlags) -> bool {
        self.0 & other.0 == other.0
    }

    /// True when this entry owns more extents than its payload needs (§5.3).
    pub fn holds_slack(self) -> bool {
        self.has(EntryFlags::RECORDING) || self.has(EntryFlags::RESERVED)
    }
}

/// Bytes a display name may occupy (§5.3).
pub const NAME_CAPACITY: usize = 48;

/// What a menu shows: UTF-8, at most 48 bytes, and the store does not normalise, trim or case-fold
/// it. An empty name is legal — a ride has none until it is finalised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayName {
    bytes: [u8; NAME_CAPACITY],
    len: u8,
}

impl Default for DisplayName {
    fn default() -> Self {
        DisplayName { bytes: [0; NAME_CAPACITY], len: 0 }
    }
}

impl DisplayName {
    /// Takes the first 48 bytes of `text`; longer input is refused rather than truncated mid-name.
    pub fn new(text: &str) -> Option<Self> {
        if text.len() > NAME_CAPACITY {
            return None;
        }
        let mut name = DisplayName::default();
        name.bytes[..text.len()].copy_from_slice(text.as_bytes());
        name.len = text.len() as u8;
        Some(name)
    }

    /// The name's bytes, without the zero pad.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }

    /// The name as text, or `None` when the card holds bytes that are not UTF-8.
    pub fn as_str(&self) -> Option<&str> {
        core::str::from_utf8(self.as_bytes()).ok()
    }

    /// The 48-byte field as it is encoded, pad included.
    pub fn padded(&self) -> &[u8; NAME_CAPACITY] {
        &self.bytes
    }

    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Decodes the entry's length byte and 48-byte field: the pad must be zero.
    pub fn decode(len: u8, field: &[u8]) -> Result<Self> {
        let err = |reason| super::error::DecodeError::new(Record::Entry, reason);
        if len as usize > NAME_CAPACITY {
            return Err(err(Reason::Count));
        }
        if !super::raw::is_zero(field, len as usize, NAME_CAPACITY - len as usize) {
            return Err(err(Reason::Reserved));
        }
        let mut name = DisplayName::default();
        name.bytes.copy_from_slice(&field[..NAME_CAPACITY]);
        name.len = len;
        Ok(name)
    }
}

/// The metadata half of a catalog entry, and nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryMeta {
    pub id: ObjectId,
    pub revision: Revision,
    pub kind: ObjectKind,
    pub flags: EntryFlags,
    pub payload_len: u64,
    pub payload_crc: u32,
    pub name: DisplayName,
}

impl EntryMeta {
    /// The `(ObjectId, Revision)` pair the catalog is keyed by.
    pub fn key(&self) -> (ObjectId, Revision) {
        (self.id, self.revision)
    }
}

/// An opaque reservation of extents. RAM state until a commit names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Allocation {
    pub(super) slot: u8,
    pub(super) nonce: u32,
    pub(super) reserved: u64,
    pub(super) written: u64,
}

impl Allocation {
    /// The byte length reserved. The only thing an `Allocation` exposes.
    pub fn reserved_bytes(&self) -> u64 {
        self.reserved
    }

    /// Bytes appended so far. Producer adapters use this as the final payload length when they
    /// publish an oversized reservation.
    pub fn written_bytes(&self) -> u64 {
        self.written
    }
}

/// Where a `Put`'s extents come from.
#[derive(Debug)]
pub enum PutSource {
    /// Publish the extents of a freshly written allocation, consuming it.
    Fresh(Allocation),
    /// Keep the extents the named entry already holds and change only its metadata.
    Amend,
}

/// One entry mutation. A commit applies a batch of them atomically.
#[derive(Debug)]
pub enum Mutation {
    /// Publish a revision.
    Put { meta: EntryMeta, source: PutSource },
    /// Remove one entry. Its extents are free at the gate.
    Remove { id: ObjectId, revision: Revision },
}

impl Mutation {
    /// The catalog key this mutation addresses.
    pub fn key(&self) -> (ObjectId, Revision) {
        match self {
            Mutation::Put { meta, .. } => meta.key(),
            Mutation::Remove { id, revision } => (*id, *revision),
        }
    }
}

/// Fixed opaque recorder state carried by every logical ride checkpoint.
pub const RIDE_RESUME_LEN: usize = 96;

/// One ride checkpoint: the bytes appended since the last successful checkpoint, the running
/// payload CRC, and the recorder's fixed resume image. Storage reconstructs the next full tail-slot
/// snapshot from the previous durable slot plus `append`; the recorder never keeps that snapshot in
/// RAM. Storage CRC-protects `resume` but never interprets it.
#[derive(Debug, Clone, Copy)]
pub struct RideCheckpoint<'a> {
    /// The entry carrying `RECORDING`; the store rejects a checkpoint naming anything else.
    pub id: ObjectId,
    pub revision: Revision,
    /// Bytes after the previous successful logical checkpoint, oldest first. A successful call
    /// consumes the whole slice. After an error the caller must retry these exact bytes before it
    /// appends more, recomputes the resume image, or appends terminal/footer bytes; this is what
    /// keeps a gated rollover repair idempotent. Discard may instead remove the `RECORDING` entry.
    pub append: &'a [u8],
    /// CRC-32 of the whole ride payload after `append`.
    pub payload_crc: u32,
    /// Versioned recorder-owned continuation state for precisely this logical checkpoint.
    pub resume: &'a [u8; RIDE_RESUME_LEN],
}

/// The card, as everything above it sees it.
///
/// **Every operation takes `&self`, the mutators included** (#1256, the owner ruling of 2026-08-18).
/// A store is shared, not owned: the board holds a source over the mounted map for the life of the
/// image while an upload commits and a ride journals, and `&mut` on the write half made that
/// un-expressible. The resident state that moves lives behind cells — see
/// [`store`](super::store)'s aliasing rules for which, and
/// [`source`](super::source) for what the consumer side gives up in exchange.
pub trait Store {
    type Handle;

    /// Reserve space for `bytes`. RAM state until a commit names it; released by
    /// [`cancel`](super::store::FlatStore::cancel) and by the next mount, which rebuilds the free
    /// map from the catalog and cannot see it. **Dropping an `Allocation` releases nothing** — it is
    /// `Copy`, and the row it names lives in the store.
    fn allocate(&self, bytes: u64) -> core::result::Result<Allocation, StoreError>;

    /// Append to an allocation. Writes are sequential and the total may not exceed the reservation.
    ///
    /// The `&mut` that is left is the caller's own token, not the store's: the cursor `Allocation`
    /// carries has to advance with the row's.
    fn write(&self, allocation: &mut Allocation, bytes: &[u8]) -> core::result::Result<(), StoreError>;

    /// Apply `mutations` atomically and return the new catalog commit sequence. The one durable
    /// transition: it makes new bytes visible and old bytes free in the same instant.
    fn commit(&self, mutations: &[Mutation]) -> core::result::Result<u64, StoreError>;

    /// Resolve an object. `revision` of `None` takes the head; `Some(r)` takes exactly that
    /// revision, which is how a retained previous revision is reached.
    fn open(&self, id: ObjectId, revision: Option<Revision>) -> core::result::Result<Self::Handle, StoreError>;

    /// Random access inside an open object. Returns bytes read, short only at end of payload.
    fn read(&self, handle: &Self::Handle, offset: u64, buf: &mut [u8]) -> core::result::Result<usize, StoreError>;

    // Beside the five, and not object operations.

    /// Read-only catalog view. LIST, every menu, and the free-space answer come from here.
    /// It mutates nothing and names nothing below the seam, so it is not a sixth verb.
    fn entries(&self) -> impl Iterator<Item = EntryMeta> + '_;

    /// The ride exception, and the only way bytes become durable without a commit. Performs both
    /// halves of `FLAT_Store_Format.md` §7.2: gate each whole 16 KiB prefix in a tail slot before
    /// copying it to the recording entry's extents, then gate the remainder when one exists.
    fn journal(&self, checkpoint: RideCheckpoint) -> core::result::Result<(), StoreError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_are_exactly_section_3_1() {
        for (value, kind) in [
            (1u16, ObjectKind::Route),
            (2, ObjectKind::Trip),
            (3, ObjectKind::Ride),
            (4, ObjectKind::WeatherBundle),
            (5, ObjectKind::MapShard),
            (6, ObjectKind::MapSetManifest),
            (7, ObjectKind::UpdatePackage),
            (8, ObjectKind::RollbackReserve),
        ] {
            assert_eq!(ObjectKind::decode(value).unwrap(), kind);
            assert_eq!(kind as u16, value);
        }
        for value in [0u16, 9, 255, 0xFFFF] {
            assert_eq!(ObjectKind::decode(value).unwrap_err().reason, Reason::UnknownEnum);
        }
    }

    #[test]
    fn flag_bits_are_the_three_section_5_3_defines() {
        assert_eq!(EntryFlags::RECORDING.bits(), 1);
        assert_eq!(EntryFlags::RETAINED.bits(), 2);
        assert_eq!(EntryFlags::RESERVED.bits(), 4);
        assert!(EntryFlags::decode(0b111).is_ok());
        for bit in 3..16 {
            assert_eq!(EntryFlags::decode(1 << bit).unwrap_err().reason, Reason::UnknownEnum);
        }
    }

    #[test]
    fn a_name_pad_must_be_zero_and_48_bytes_is_the_limit() {
        let name = DisplayName::new("Grimsel Loop").unwrap();
        assert_eq!(name.as_str(), Some("Grimsel Loop"));
        assert_eq!(name.len(), 12);
        assert!(DisplayName::new("x".repeat(48).as_str()).is_some());
        assert!(DisplayName::new("x".repeat(49).as_str()).is_none());

        let mut field = [0u8; NAME_CAPACITY];
        field[..12].copy_from_slice(b"Grimsel Loop");
        assert_eq!(DisplayName::decode(12, &field).unwrap(), name);
        field[20] = 1;
        assert_eq!(DisplayName::decode(12, &field).unwrap_err().reason, Reason::Reserved);
        assert_eq!(DisplayName::decode(49, &field).unwrap_err().reason, Reason::Count);
    }
}
