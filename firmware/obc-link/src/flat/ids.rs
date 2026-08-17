//! The vocabulary both sides of the engine speak: three identities, the kind table, the entry
//! flags, a display name, and the metadata half of a catalog entry.
//!
//! `FLAT_Store_Format.md` §3 and §5.3 are the sole authority for every value here and
//! `FLAT_Store_Protocol.md` §2 and §3.3 carry the same numbers. These types are declared in this
//! crate rather than imported from the store because the dependency runs the other way: `obc-link`
//! is a foundation crate and the store is a platform adapter, so the engine names what it needs and
//! the store binds it (see [`super::store`]). The two definitions are held equal by the binder's own
//! pinning test, which is what makes "the same numbers" a fact rather than a hope.

/// The card. A `StoreId` a client has not seen means the card was re-initialized and everything it
/// cached is void.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StoreId(pub [u8; 16]);

/// Store-global, never reused. `0` is reserved and names no object — on the wire it is what a `PUT`
/// sends to mean "create".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct ObjectId(pub u64);

impl ObjectId {
    /// The reserved id that names no object.
    pub const NONE: ObjectId = ObjectId(0);

    /// True for every id that names an object.
    pub fn is_some(self) -> bool {
        self.0 != 0
    }
}

/// Per object: `1` for the commit that creates it, `+1` for every commit that replaces it. Also the
/// compare-and-swap token every mutation carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Revision(pub u64);

impl Revision {
    /// The value `GET` and `STATUS` read as "whatever the head is".
    pub const HEAD: Revision = Revision(0);

    /// The revision the commit that creates an object publishes.
    pub const FIRST: Revision = Revision(1);

    /// The revision that supersedes this one, or `None` at the end of the space (§3).
    pub fn next(self) -> Option<Revision> {
        self.0.checked_add(1).map(Revision)
    }
}

/// `FLAT_Store_Format.md` §3.1, carried unchanged by `FLAT_Store_Protocol.md`.
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
    /// Decodes §3.1's `u16`. Kind `0` is never encoded and no other value is registered.
    pub fn decode(value: u16) -> Option<Self> {
        Some(match value {
            1 => ObjectKind::Route,
            2 => ObjectKind::Trip,
            3 => ObjectKind::Ride,
            4 => ObjectKind::WeatherBundle,
            5 => ObjectKind::MapShard,
            6 => ObjectKind::MapSetManifest,
            7 => ObjectKind::UpdatePackage,
            8 => ObjectKind::RollbackReserve,
            _ => return None,
        })
    }

    /// The `u16` §3.1 registers.
    pub fn value(self) -> u16 {
        self as u16
    }

    /// True for the two kinds the device produces and a client may never `PUT`
    /// (`FLAT_Store_Protocol.md` §3.6).
    pub fn is_device_owned(self) -> bool {
        matches!(self, ObjectKind::Ride | ObjectKind::RollbackReserve)
    }
}

/// §5.3's entry flags. Bits `3..15` are zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EntryFlags(u16);

impl EntryFlags {
    /// No flag.
    pub const NONE: EntryFlags = EntryFlags(0);
    /// The active ride.
    pub const RECORDING: EntryFlags = EntryFlags(1 << 0);
    /// A non-head revision the store keeps alive on purpose.
    pub const RETAINED: EntryFlags = EntryFlags(1 << 1);
    /// The entry owns extents and the store does not write the payload.
    pub const RESERVED: EntryFlags = EntryFlags(1 << 2);

    const DEFINED: u16 = 0b111;

    /// Decodes §5.3's `u16`, rejecting an undefined bit.
    pub fn decode(value: u16) -> Option<Self> {
        (value & !Self::DEFINED == 0).then_some(EntryFlags(value))
    }

    /// The `u16` an entry carries.
    pub fn bits(self) -> u16 {
        self.0
    }

    /// True when every flag of `other` is set here.
    pub fn has(self, other: EntryFlags) -> bool {
        self.0 & other.0 == other.0
    }

    /// This flag word with `other` set.
    pub fn with(self, other: EntryFlags) -> Self {
        EntryFlags(self.0 | other.0)
    }

    /// True for the two flags that make an entry untouchable from the wire: the store did not write
    /// a reserve's bytes, and a recording ride's length and CRC are stale until it ends (§3.5, §3.6,
    /// §3.7).
    pub fn is_untouchable(self) -> bool {
        self.has(EntryFlags::RECORDING) || self.has(EntryFlags::RESERVED)
    }
}

/// Bytes a display name may occupy (§5.3).
pub const NAME_CAPACITY: usize = 48;

/// What a menu shows: UTF-8, at most 48 bytes, never normalised, trimmed or case-folded. An empty
/// name is legal.
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
        Self::from_bytes(text.as_bytes())
    }

    /// The same from raw bytes, which is what a wire field carries.
    pub fn from_bytes(text: &[u8]) -> Option<Self> {
        if text.len() > NAME_CAPACITY {
            return None;
        }
        let mut name = DisplayName::default();
        name.bytes[..text.len()].copy_from_slice(text);
        name.len = text.len() as u8;
        Some(name)
    }

    /// The name's bytes, without the zero pad.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }

    /// The 48-byte field as it is encoded, pad included.
    pub fn padded(&self) -> &[u8; NAME_CAPACITY] {
        &self.bytes
    }

    /// The declared length byte.
    pub fn len(&self) -> usize {
        self.len as usize
    }

    /// True for the empty name a ride carries until it is finalised.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// The metadata half of a catalog entry, and nothing else. It carries no extent, which is what
/// keeps `FLAT_Store_Protocol.md` §2's first sentence true.
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
    /// The `(ObjectId, Revision)` pair the catalog is keyed by, and the `LIST` cursor.
    pub fn key(&self) -> (ObjectId, Revision) {
        (self.id, self.revision)
    }
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
            assert_eq!(ObjectKind::decode(value), Some(kind));
            assert_eq!(kind.value(), value);
        }
        for value in [0u16, 9, 255, 0xFFFF] {
            assert_eq!(ObjectKind::decode(value), None);
        }
        assert!(ObjectKind::Ride.is_device_owned());
        assert!(ObjectKind::RollbackReserve.is_device_owned());
        assert!(!ObjectKind::Route.is_device_owned());
    }

    #[test]
    fn flag_bits_are_the_three_section_5_3_defines() {
        assert_eq!(EntryFlags::RECORDING.bits(), 1);
        assert_eq!(EntryFlags::RETAINED.bits(), 2);
        assert_eq!(EntryFlags::RESERVED.bits(), 4);
        assert!(EntryFlags::decode(0b111).is_some());
        for bit in 3..16 {
            assert_eq!(EntryFlags::decode(1 << bit), None);
        }
        assert!(EntryFlags::RECORDING.is_untouchable());
        assert!(EntryFlags::RESERVED.is_untouchable());
        assert!(!EntryFlags::RETAINED.is_untouchable());
        assert_eq!(EntryFlags::NONE.with(EntryFlags::RETAINED), EntryFlags::RETAINED);
    }

    #[test]
    fn a_name_is_at_most_48_bytes() {
        let name = DisplayName::new("Grimsel Loop").unwrap();
        assert_eq!(name.as_bytes(), b"Grimsel Loop");
        assert_eq!(name.len(), 12);
        assert_eq!(name.padded()[12..], [0; 36]);
        assert!(DisplayName::new(&"x".repeat(48)).is_some());
        assert!(DisplayName::new(&"x".repeat(49)).is_none());
        assert!(DisplayName::default().is_empty());
    }

    #[test]
    fn a_revision_stops_one_short_of_wrapping() {
        assert_eq!(Revision(3).next(), Some(Revision(4)));
        assert_eq!(Revision(u64::MAX).next(), None);
        assert!(!ObjectId::NONE.is_some());
        assert!(ObjectId(1).is_some());
    }
}
