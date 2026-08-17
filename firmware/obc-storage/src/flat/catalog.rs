//! The catalog (`FLAT_Store_Format.md` §5): the header, the 128-byte entry, the gate sector, and
//! the structural rules an entry array must satisfy.
//!
//! The catalog **is** the store. It names every object that exists, which extents each one occupies,
//! and nothing else — the free-extent bitmap is its complement, recomputed at mount.
//!
//! Gate validity is two-tiered, because selecting a copy and trusting one are different questions
//! and the body read sits between them. [`Gate::decode`] decides **well-formed** from the 512 gate
//! bytes alone; only a body CRC that matches [`Gate::body_crc`] makes it **valid**.

use super::error::{DecodeError, Reason, Record, Result};
use super::layout::{Ranges, BLOCK, ENTRY_CAPACITY, ENTRY_STRIDE};
use super::raw::{bytes16_at, crc32, is_zero, put_bytes, put_u16, put_u32, put_u64, u16_at, u32_at, u64_at};
use super::seam::{DisplayName, EntryFlags, EntryMeta, ObjectId, ObjectKind, Revision, StoreId, NAME_CAPACITY};
use super::FORMAT_VERSION;

/// `FSCT`, the catalog header.
pub const HEADER_MAGIC: [u8; 4] = *b"FSCT";
/// `FSCG`, the catalog gate.
pub const GATE_MAGIC: [u8; 4] = *b"FSCG";
/// The gate CRC covers bytes `0..504`.
const GATE_CRC_OFFSET: usize = 504;
/// The 512 zero bytes that invalidate a gate (§5.4). An all-zero gate fails magic and CRC, so
/// invalidation needs neither a sentinel value nor a read-modify-write.
pub const INVALIDATED: [u8; BLOCK] = [0u8; BLOCK];

/// The catalog header: block 0 of the copy, and the first 512 bytes of the body the gate certifies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub store: StoreId,
    /// Starts at `1` at initialization and increments by exactly one per commit.
    pub sequence: u64,
    /// Strictly greater than every `ObjectId` in the array, and never rewound.
    pub next_object: u64,
    pub entry_count: u16,
}

impl Header {
    pub fn encode(&self) -> [u8; BLOCK] {
        let mut out = [0u8; BLOCK];
        put_bytes(&mut out, 0, &HEADER_MAGIC);
        put_u16(&mut out, 4, FORMAT_VERSION);
        put_u16(&mut out, 6, ENTRY_STRIDE as u16);
        put_bytes(&mut out, 8, &self.store.0);
        put_u64(&mut out, 24, self.sequence);
        put_u64(&mut out, 32, self.next_object);
        put_u16(&mut out, 40, self.entry_count);
        out
    }

    /// Decodes the header block. The header carries no CRC of its own — it is part of the body, and
    /// the gate is what certifies the body.
    pub fn decode(bytes: &[u8], store: &StoreId) -> Result<Self> {
        let err = |reason| DecodeError::new(Record::CatalogHeader, reason);
        if bytes.len() < BLOCK {
            return Err(err(Reason::Length));
        }
        if bytes[0..4] != HEADER_MAGIC {
            return Err(err(Reason::Magic));
        }
        if u16_at(bytes, 4) != FORMAT_VERSION {
            return Err(err(Reason::Version));
        }
        if u16_at(bytes, 6) != ENTRY_STRIDE as u16 {
            return Err(err(Reason::Stride));
        }
        if bytes16_at(bytes, 8) != store.0 {
            return Err(err(Reason::StoreId));
        }
        let entry_count = u16_at(bytes, 40);
        if entry_count as usize > ENTRY_CAPACITY {
            return Err(err(Reason::Count));
        }
        if !is_zero(bytes, 42, BLOCK - 42) {
            return Err(err(Reason::Reserved));
        }
        Ok(Header { store: *store, sequence: u64_at(bytes, 24), next_object: u64_at(bytes, 32), entry_count })
    }
}

/// One entry: one revision of one object, and the extents it occupies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub meta: EntryMeta,
    pub ranges: Ranges,
}

impl Entry {
    pub fn encode(&self) -> [u8; ENTRY_STRIDE] {
        let mut out = [0u8; ENTRY_STRIDE];
        put_u16(&mut out, 0, self.meta.kind as u16);
        put_u16(&mut out, 2, self.meta.flags.bits());
        out[4] = self.ranges.len() as u8;
        out[5] = self.meta.name.len() as u8;
        put_u64(&mut out, 8, self.meta.id.0);
        put_u64(&mut out, 16, self.meta.revision.0);
        put_u64(&mut out, 24, self.meta.payload_len);
        put_u32(&mut out, 32, self.meta.payload_crc);
        put_bytes(&mut out, 40, &self.ranges.encode());
        put_bytes(&mut out, 72, self.meta.name.padded());
        out
    }

    /// Decodes one entry. `extent_count` is the card's, so a range that leaves the extent area is
    /// refused before any address is derived from it.
    pub fn decode(bytes: &[u8], extent_count: u32) -> Result<Self> {
        let err = |reason| DecodeError::new(Record::Entry, reason);
        if bytes.len() < ENTRY_STRIDE {
            return Err(err(Reason::Length));
        }
        if !is_zero(bytes, 6, 2) || !is_zero(bytes, 36, 4) || !is_zero(bytes, 120, 8) {
            return Err(err(Reason::Reserved));
        }
        let kind = ObjectKind::decode(u16_at(bytes, 0))?;
        let flags = EntryFlags::decode(u16_at(bytes, 2))?;
        let id = u64_at(bytes, 8);
        let revision = u64_at(bytes, 16);
        if id == 0 || revision == 0 {
            return Err(err(Reason::Zero));
        }
        let ranges = Ranges::decode(&bytes[40..72], bytes[4], extent_count)?;
        let name = DisplayName::decode(bytes[5], &bytes[72..72 + NAME_CAPACITY])?;
        Ok(Entry {
            meta: EntryMeta {
                id: ObjectId(id),
                revision: Revision(revision),
                kind,
                flags,
                payload_len: u64_at(bytes, 24),
                payload_crc: u32_at(bytes, 32),
                name,
            },
            ranges,
        })
    }

    /// §5.3's rules about one entry in isolation: what its ranges must cover, and what `RESERVED`
    /// forbids.
    fn check(&self) -> Result<()> {
        let err = |reason| DecodeError::new(Record::Entry, reason);
        let needed = super::layout::extents_for(self.meta.payload_len);
        let owned = self.ranges.extents() as u64;
        if owned < needed || (owned > needed && !self.meta.flags.holds_slack()) {
            return Err(err(Reason::Ranges));
        }
        if self.meta.flags.has(EntryFlags::RESERVED) && self.meta.payload_len != 0 {
            return Err(err(Reason::Ranges));
        }
        Ok(())
    }
}

/// The gate sector: one 512-byte block, written and synchronized after the body is synchronized,
/// and the only thing that makes the body it names authoritative.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gate {
    pub copy: u8,
    pub store: StoreId,
    pub sequence: u64,
    pub entry_count: u16,
    /// CRC-32 over the `512 + entry_count × 128` body bytes.
    pub body_crc: u32,
}

impl Gate {
    pub fn encode(&self) -> [u8; BLOCK] {
        let mut out = [0u8; BLOCK];
        put_bytes(&mut out, 0, &GATE_MAGIC);
        put_u16(&mut out, 4, FORMAT_VERSION);
        put_u16(&mut out, 6, u16::from(self.copy));
        put_bytes(&mut out, 8, &self.store.0);
        put_u64(&mut out, 24, self.sequence);
        put_u16(&mut out, 32, self.entry_count);
        put_u32(&mut out, 36, self.body_crc);
        let crc = crc32(&out[..GATE_CRC_OFFSET]);
        put_u32(&mut out, GATE_CRC_OFFSET, crc);
        out
    }

    /// Decides **well-formed** (§5.4): magic and version known, copy index equal to the physical
    /// position, `StoreId` equal to the superblock's, gate CRC checking, entry count within
    /// capacity. All five are properties of these 512 bytes alone, which is what lets mount take a
    /// sequence high-water mark from two gate reads — and why no field of an ill-formed gate is ever
    /// read.
    pub fn decode(bytes: &[u8], copy: usize, store: &StoreId) -> Result<Self> {
        let err = |reason| DecodeError::new(Record::Gate, reason);
        if bytes.len() < BLOCK {
            return Err(err(Reason::Length));
        }
        if bytes[0..4] != GATE_MAGIC {
            return Err(err(Reason::Magic));
        }
        if u16_at(bytes, 4) != FORMAT_VERSION {
            return Err(err(Reason::Version));
        }
        if u16_at(bytes, 6) != copy as u16 {
            return Err(err(Reason::Position));
        }
        if bytes16_at(bytes, 8) != store.0 {
            return Err(err(Reason::StoreId));
        }
        if u32_at(bytes, GATE_CRC_OFFSET) != crc32(&bytes[..GATE_CRC_OFFSET]) {
            return Err(err(Reason::Crc));
        }
        let entry_count = u16_at(bytes, 32);
        if entry_count as usize > ENTRY_CAPACITY {
            return Err(err(Reason::Count));
        }
        if !is_zero(bytes, 34, 2) || !is_zero(bytes, 40, 464) || !is_zero(bytes, 508, 4) {
            return Err(err(Reason::Reserved));
        }
        Ok(Gate {
            copy: copy as u8,
            store: *store,
            sequence: u64_at(bytes, 24),
            entry_count,
            body_crc: u32_at(bytes, 36),
        })
    }
}

/// The entry array's cross-entry rules (§5.3), checked in one forward pass over the live prefix.
///
/// Ordering, the retained/head pair, kind agreement per `ObjectId` and the one `RECORDING` entry are
/// all properties of a *sequence* of entries, so they cannot live in [`Entry::decode`]. Mount runs
/// this while it streams the body, and a commit runs it over the entries it is about to write.
#[derive(Debug, Default)]
pub struct Structure {
    previous: Option<(ObjectId, Revision, ObjectKind, bool)>,
    /// Entries seen so far for the current `ObjectId`.
    revisions: u8,
    recording: u8,
    greatest_id: u64,
}

impl Structure {
    /// Accepts the next entry of the array.
    pub fn accept(&mut self, entry: &Entry) -> Result<()> {
        let err = |reason| DecodeError::new(Record::Entry, reason);
        entry.check()?;
        let retained = entry.meta.flags.has(EntryFlags::RETAINED);
        if entry.meta.flags.has(EntryFlags::RECORDING) {
            self.recording += 1;
            if self.recording > 1 {
                return Err(err(Reason::Revisions));
            }
        }
        if let Some((id, revision, kind, previous_retained)) = self.previous {
            if (entry.meta.id, entry.meta.revision) <= (id, revision) {
                return Err(err(Reason::Order));
            }
            if entry.meta.id == id {
                // Exactly two entries, of which precisely one carries RETAINED, and the retained one
                // sorts first because the head has the greater revision.
                if self.revisions == 2 || !previous_retained || retained || entry.meta.kind != kind {
                    return Err(err(Reason::Revisions));
                }
                self.revisions += 1;
            } else {
                if previous_retained && self.revisions == 1 {
                    return Err(err(Reason::Revisions));
                }
                self.revisions = 1;
            }
        } else {
            self.revisions = 1;
        }
        self.greatest_id = self.greatest_id.max(entry.meta.id.0);
        self.previous = Some((entry.meta.id, entry.meta.revision, entry.meta.kind, retained));
        Ok(())
    }

    /// The rules that can only be judged once the array has ended: a trailing lone `RETAINED` entry
    /// has no head, and the header's cursor must be strictly greater than every id.
    pub fn finish(&self, header: &Header) -> Result<()> {
        let err = |reason| DecodeError::new(Record::Entry, reason);
        if let Some((_, _, _, retained)) = self.previous {
            if retained && self.revisions == 1 {
                return Err(err(Reason::Revisions));
            }
        }
        if header.next_object <= self.greatest_id {
            return Err(DecodeError::new(Record::CatalogHeader, Reason::Order));
        }
        Ok(())
    }

    /// True when the array holds the one active ride.
    pub fn recording(&self) -> bool {
        self.recording == 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn entry(id: u64, revision: u64, flags: EntryFlags, len: u64, first: u16, count: u16) -> Entry {
        let mut ranges = Ranges::default();
        ranges.push(first, count).unwrap();
        Entry {
            meta: EntryMeta {
                id: ObjectId(id),
                revision: Revision(revision),
                kind: ObjectKind::Route,
                flags,
                payload_len: len,
                payload_crc: 0x1234_5678,
                name: DisplayName::new("x").unwrap(),
            },
            ranges,
        }
    }

    const STORE: StoreId = StoreId([0x5A; 16]);

    #[test]
    fn header_entry_and_gate_round_trip() {
        let header = Header { store: STORE, sequence: 9, next_object: 4, entry_count: 3 };
        assert_eq!(Header::decode(&header.encode(), &STORE).unwrap(), header);

        let entry = entry(2, 5, EntryFlags::NONE, 100, 7, 1);
        assert_eq!(Entry::decode(&entry.encode(), 64).unwrap(), entry);

        let gate = Gate { copy: 1, store: STORE, sequence: 9, entry_count: 3, body_crc: 0xDEAD_BEEF };
        assert_eq!(Gate::decode(&gate.encode(), 1, &STORE).unwrap(), gate);
    }

    #[test]
    fn a_gate_is_ill_formed_at_the_wrong_position_or_the_wrong_store() {
        let gate = Gate { copy: 0, store: STORE, sequence: 9, entry_count: 3, body_crc: 7 };
        let bytes = gate.encode();
        assert_eq!(Gate::decode(&bytes, 1, &STORE).unwrap_err().reason, Reason::Position);
        assert_eq!(Gate::decode(&bytes, 0, &StoreId([1; 16])).unwrap_err().reason, Reason::StoreId);
        assert_eq!(Gate::decode(&INVALIDATED, 0, &STORE).unwrap_err().reason, Reason::Magic);
    }

    #[test]
    fn every_single_byte_flip_of_a_gate_is_rejected() {
        let bytes = Gate { copy: 0, store: STORE, sequence: 9, entry_count: 3, body_crc: 7 }.encode();
        for index in 0..BLOCK {
            let mut torn = bytes;
            torn[index] ^= 0xFF;
            assert!(Gate::decode(&torn, 0, &STORE).is_err(), "byte {index} flip accepted");
        }
    }

    #[test]
    fn a_count_above_capacity_is_refused_in_both_the_header_and_the_gate() {
        let mut header = Header { store: STORE, sequence: 1, next_object: 1, entry_count: 0 }.encode();
        put_u16(&mut header, 40, ENTRY_CAPACITY as u16 + 1);
        assert_eq!(Header::decode(&header, &STORE).unwrap_err().reason, Reason::Count);

        let mut gate = Gate { copy: 0, store: STORE, sequence: 1, entry_count: 0, body_crc: 0 };
        gate.entry_count = ENTRY_CAPACITY as u16 + 1;
        assert_eq!(Gate::decode(&gate.encode(), 0, &STORE).unwrap_err().reason, Reason::Count);
    }

    #[test]
    fn an_entry_names_a_nonzero_object_and_revision() {
        let mut bytes = entry(1, 1, EntryFlags::NONE, 10, 0, 1).encode();
        put_u64(&mut bytes, 8, 0);
        assert_eq!(Entry::decode(&bytes, 64).unwrap_err().reason, Reason::Zero);
        put_u64(&mut bytes, 8, 1);
        put_u64(&mut bytes, 16, 0);
        assert_eq!(Entry::decode(&bytes, 64).unwrap_err().reason, Reason::Zero);
    }

    /// §5.3's covering rule: exactly `ceil(len / 1 MiB)` extents unless the entry is recording or
    /// reserved, in which case it may hold slack.
    #[test]
    fn ranges_must_cover_the_payload_and_only_slack_flags_may_exceed_it() {
        let mut structure = Structure::default();
        assert!(structure.accept(&entry(1, 1, EntryFlags::NONE, super::super::layout::EXTENT_SIZE, 0, 1)).is_ok());

        let over = entry(2, 1, EntryFlags::NONE, 10, 4, 2);
        assert_eq!(Structure::default().accept(&over).unwrap_err().reason, Reason::Ranges);
        let recording = entry(2, 1, EntryFlags::RECORDING, 10, 4, 2);
        assert!(Structure::default().accept(&recording).is_ok());

        let under = entry(3, 1, EntryFlags::NONE, super::super::layout::EXTENT_SIZE + 1, 4, 1);
        assert_eq!(Structure::default().accept(&under).unwrap_err().reason, Reason::Ranges);

        let mut reserve = entry(4, 1, EntryFlags::RESERVED, 1, 4, 1);
        assert_eq!(Structure::default().accept(&reserve).unwrap_err().reason, Reason::Ranges);
        reserve.meta.payload_len = 0;
        assert!(Structure::default().accept(&reserve).is_ok());
    }

    #[test]
    fn entries_are_strictly_ascending_by_object_and_revision() {
        let mut structure = Structure::default();
        structure.accept(&entry(2, 1, EntryFlags::RETAINED, 10, 0, 1)).unwrap();
        assert_eq!(structure.accept(&entry(2, 1, EntryFlags::NONE, 10, 1, 1)).unwrap_err().reason, Reason::Order);

        let mut structure = Structure::default();
        structure.accept(&entry(3, 1, EntryFlags::NONE, 10, 0, 1)).unwrap();
        assert_eq!(structure.accept(&entry(2, 9, EntryFlags::NONE, 10, 1, 1)).unwrap_err().reason, Reason::Order);
    }

    /// One `ObjectId` holds either one entry, or exactly two of which precisely one carries
    /// `RETAINED` — and the retained one sorts first.
    #[test]
    fn the_retained_head_pair_is_the_only_two_entry_shape() {
        let header = Header { store: STORE, sequence: 1, next_object: 99, entry_count: 2 };

        let mut ok = Structure::default();
        ok.accept(&entry(2, 4, EntryFlags::RETAINED, 10, 0, 1)).unwrap();
        ok.accept(&entry(2, 5, EntryFlags::NONE, 10, 1, 1)).unwrap();
        ok.finish(&header).unwrap();

        // Two heads, no retained.
        let mut two_heads = Structure::default();
        two_heads.accept(&entry(2, 4, EntryFlags::NONE, 10, 0, 1)).unwrap();
        assert_eq!(two_heads.accept(&entry(2, 5, EntryFlags::NONE, 10, 1, 1)).unwrap_err().reason, Reason::Revisions);

        // Retained after the head.
        let mut wrong_order = Structure::default();
        wrong_order.accept(&entry(2, 4, EntryFlags::RETAINED, 10, 0, 1)).unwrap();
        assert_eq!(
            wrong_order.accept(&entry(2, 5, EntryFlags::RETAINED, 10, 1, 1)).unwrap_err().reason,
            Reason::Revisions
        );

        // Three revisions of one object.
        let mut three = Structure::default();
        three.accept(&entry(2, 4, EntryFlags::RETAINED, 10, 0, 1)).unwrap();
        three.accept(&entry(2, 5, EntryFlags::NONE, 10, 1, 1)).unwrap();
        assert_eq!(three.accept(&entry(2, 6, EntryFlags::NONE, 10, 2, 1)).unwrap_err().reason, Reason::Revisions);

        // A lone retained entry has no head, whether the array ends there or moves to another id.
        let mut lone = Structure::default();
        lone.accept(&entry(2, 4, EntryFlags::RETAINED, 10, 0, 1)).unwrap();
        assert_eq!(lone.finish(&header).unwrap_err().reason, Reason::Revisions);
        assert_eq!(lone.accept(&entry(3, 1, EntryFlags::NONE, 10, 1, 1)).unwrap_err().reason, Reason::Revisions);
    }

    #[test]
    fn one_object_id_has_one_kind() {
        let mut structure = Structure::default();
        structure.accept(&entry(2, 4, EntryFlags::RETAINED, 10, 0, 1)).unwrap();
        let mut head = entry(2, 5, EntryFlags::NONE, 10, 1, 1);
        head.meta.kind = ObjectKind::Trip;
        assert_eq!(structure.accept(&head).unwrap_err().reason, Reason::Revisions);
    }

    #[test]
    fn at_most_one_entry_is_recording() {
        let mut structure = Structure::default();
        structure.accept(&entry(2, 1, EntryFlags::RECORDING, 0, 0, 1)).unwrap();
        assert!(structure.recording());
        assert_eq!(
            structure.accept(&entry(3, 1, EntryFlags::RECORDING, 0, 1, 1)).unwrap_err().reason,
            Reason::Revisions
        );
    }

    #[test]
    fn the_next_object_cursor_is_strictly_greater_than_every_id() {
        let mut structure = Structure::default();
        structure.accept(&entry(7, 1, EntryFlags::NONE, 10, 0, 1)).unwrap();
        let header = Header { store: STORE, sequence: 1, next_object: 7, entry_count: 1 };
        assert_eq!(structure.finish(&header).unwrap_err().record, Record::CatalogHeader);
        assert!(structure.finish(&Header { next_object: 8, ..header }).is_ok());
    }
}
