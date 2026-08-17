//! The ride journal (`FLAT_Store_Format.md` §7): the one place in this format where bytes become
//! durable without a commit.
//!
//! A slot is two program pages, written in one shot and covered by one CRC: a cut corrupts a page,
//! the CRC fails, and the slot is skipped. There is no gate.

use obc_crc::Crc32;

use super::catalog::Entry;
use super::error::{DecodeError, Reason, Record, Result};
use super::layout::{Ranges, BLOCK, PROGRAM_PAGE, SLOTS};
use super::raw::{bytes16_at, is_zero, put_bytes, put_u16, put_u32, put_u64, u16_at, u32_at, u64_at};
use super::seam::{ObjectId, Revision, StoreId};
use super::FORMAT_VERSION;

/// `FSRJ`.
pub const MAGIC: [u8; 4] = *b"FSRJ";
/// Tail bytes one slot carries (§7.1).
pub const TAIL_CAPACITY: usize = 32_256;
/// The slot CRC field, inside the range it covers.
const CRC_OFFSET: usize = 504;
/// One slot, in bytes.
pub const SLOT_LEN: usize = BLOCK + TAIL_CAPACITY;
/// The pad between the tail and the end of the slot, in read-only memory: a slot is written in whole
/// blocks and its CRC covers the pad, so both the writer and a verifier need zeros to hand over. Eight
/// blocks at a time keeps the pad of a small tail to eight card writes rather than sixty-two.
pub const ZERO_PAD: [u8; 8 * BLOCK] = [0u8; 8 * BLOCK];

/// One journal slot's header. The tail follows it in blocks `1..64` of the slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    pub slot: u16,
    pub id: ObjectId,
    pub revision: Revision,
    /// From `1`; slot `sequence mod 16` is the one it is written to.
    pub sequence: u64,
    /// A multiple of 16,384: payload bytes already in the ride's own extents.
    pub flushed: u64,
    pub tail_len: u32,
    /// CRC-32 over `[0, flushed + tail_len)` — a seed for the resumed session, not a verification
    /// obligation (§7.3).
    pub payload_crc: u32,
    /// Copied verbatim from the ride's catalog entry, as a cross-check.
    pub ranges: Ranges,
    /// CRC-32 over the whole 32,768-byte slot with this field zero.
    pub slot_crc: u32,
}

impl Slot {
    /// The ride's payload length at this checkpoint. Derived, not stored.
    pub fn payload_len(&self) -> u64 {
        self.flushed + self.tail_len as u64
    }

    fn encode_with(&self, slot_crc: u32, store: &StoreId) -> [u8; BLOCK] {
        let mut out = [0u8; BLOCK];
        put_bytes(&mut out, 0, &MAGIC);
        put_u16(&mut out, 4, FORMAT_VERSION);
        put_u16(&mut out, 6, self.slot);
        put_bytes(&mut out, 8, &store.0);
        put_u64(&mut out, 24, self.id.0);
        put_u64(&mut out, 32, self.revision.0);
        put_u64(&mut out, 40, self.sequence);
        put_u64(&mut out, 48, self.flushed);
        put_u32(&mut out, 56, self.tail_len);
        put_u32(&mut out, 60, self.payload_crc);
        put_bytes(&mut out, 64, &self.ranges.encode());
        put_u32(&mut out, CRC_OFFSET, slot_crc);
        out
    }

    /// The header block this slot decoded from. Every byte outside a decoded field is proven zero, so
    /// re-encoding reproduces what the card holds — which is what lets a verifier seed
    /// [`header_digest`] without keeping all sixteen headers in RAM.
    pub fn header_bytes(&self, store: &StoreId) -> [u8; BLOCK] {
        self.encode_with(self.slot_crc, store)
    }

    /// The header block as it goes on the card, with the slot CRC over the whole slot — the header
    /// with its CRC field zero, then `tail`, then the zero pad to 32,768 bytes.
    pub fn seal(&self, store: &StoreId, tail: &[u8]) -> [u8; BLOCK] {
        let mut digest = header_digest(&self.encode_with(0, store));
        digest.update(tail);
        let mut pad = TAIL_CAPACITY - tail.len();
        while pad > 0 {
            let step = pad.min(ZERO_PAD.len());
            digest.update(&ZERO_PAD[..step]);
            pad -= step;
        }
        self.encode_with(digest.finalize(), store)
    }

    /// Decodes a slot header read from physical slot `slot`. Everything a slot can prove from its own
    /// 512 header bytes; the slot CRC covers the tail, so a caller folds the rest of the slot into
    /// [`header_digest`] and compares.
    pub fn decode(bytes: &[u8], slot: usize, store: &StoreId, extent_count: u32) -> Result<Self> {
        let err = |reason| DecodeError::new(Record::Slot, reason);
        if bytes.len() < BLOCK {
            return Err(err(Reason::Length));
        }
        if bytes[0..4] != MAGIC {
            return Err(err(Reason::Magic));
        }
        if u16_at(bytes, 4) != FORMAT_VERSION {
            return Err(err(Reason::Version));
        }
        if u16_at(bytes, 6) != slot as u16 || slot >= SLOTS {
            return Err(err(Reason::Position));
        }
        if bytes16_at(bytes, 8) != store.0 {
            return Err(err(Reason::StoreId));
        }
        let id = u64_at(bytes, 24);
        let revision = u64_at(bytes, 32);
        let sequence = u64_at(bytes, 40);
        if id == 0 || revision == 0 || sequence == 0 {
            return Err(err(Reason::Zero));
        }
        let flushed = u64_at(bytes, 48);
        if !flushed.is_multiple_of(PROGRAM_PAGE as u64) {
            return Err(err(Reason::Count));
        }
        let tail_len = u32_at(bytes, 56);
        if tail_len as usize > TAIL_CAPACITY {
            return Err(err(Reason::Count));
        }
        if !is_zero(bytes, 96, CRC_OFFSET - 96) || !is_zero(bytes, 508, 4) {
            return Err(err(Reason::Reserved));
        }
        let ranges = Ranges::decode(&bytes[64..96], live_ranges(&bytes[64..96]), extent_count)
            .map_err(|error| DecodeError::new(Record::Slot, error.reason))?;
        Ok(Slot {
            slot: slot as u16,
            id: ObjectId(id),
            revision: Revision(revision),
            sequence,
            flushed,
            tail_len,
            payload_crc: u32_at(bytes, 60),
            ranges,
            slot_crc: u32_at(bytes, CRC_OFFSET),
        })
    }

    /// §7.1's cross-check: a slot left by an earlier ride over reused extents is not this one's.
    pub fn describes(&self, entry: &Entry) -> bool {
        self.id == entry.meta.id && self.revision == entry.meta.revision && self.ranges == entry.ranges
    }
}

/// The running CRC over a slot header with its own CRC field zeroed: the seed a verifier folds the
/// remaining 63 blocks of the slot into.
pub fn header_digest(header: &[u8]) -> Crc32 {
    let mut digest = Crc32::new();
    digest.update(&header[..CRC_OFFSET]);
    digest.update(&[0, 0, 0, 0]);
    digest.update(&header[CRC_OFFSET + 4..BLOCK]);
    digest
}

/// The live range count of a slot's copied range field. The slot has no `range count` byte of its
/// own — the field is copied verbatim from the entry — so the count is where the zeros begin, and a
/// nonzero pair after a zero one is what [`Ranges::decode`] then refuses.
fn live_ranges(field: &[u8]) -> u8 {
    let mut count = 0;
    while count < super::layout::MAX_RANGES && u16_at(field, count * 4 + 2) != 0 {
        count += 1;
    }
    count as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    const STORE: StoreId = StoreId([0x5A; 16]);

    fn slot() -> Slot {
        let mut ranges = Ranges::default();
        ranges.push(13, 32).unwrap();
        Slot {
            slot: 3,
            id: ObjectId(2),
            revision: Revision(1),
            sequence: 41,
            flushed: 245_760,
            tail_len: 3_712,
            payload_crc: 0x5E1B_03C7,
            ranges,
            slot_crc: 0,
        }
    }

    fn tail(len: usize) -> std::vec::Vec<u8> {
        (0..len).map(|index| (index * 7 + 3) as u8).collect()
    }

    #[test]
    fn round_trips_through_the_card() {
        let mut expected = slot();
        let tail = tail(3_712);
        let header = expected.seal(&STORE, &tail);
        expected.slot_crc = u32_at(&header, CRC_OFFSET);
        assert_eq!(Slot::decode(&header, 3, &STORE, 30_718).unwrap(), expected);
        assert_eq!(expected.payload_len(), 249_472);
    }

    /// The slot CRC covers the whole 32,768 bytes, pad included: a byte flip anywhere in the tail
    /// fails it, which is what makes a slot all-or-nothing without a gate.
    #[test]
    fn the_slot_crc_covers_the_tail_and_the_pad() {
        let mut tail = tail(3_712);
        let sealed = Slot::decode(&slot().seal(&STORE, &tail), 3, &STORE, 30_718).unwrap();

        let mut digest = header_digest(&slot().seal(&STORE, &tail));
        digest.update(&tail);
        for _ in 0..(TAIL_CAPACITY - tail.len()) / BLOCK {
            digest.update(&ZERO_PAD[..BLOCK]);
        }
        digest.update(&ZERO_PAD[..(TAIL_CAPACITY - tail.len()) % BLOCK]);
        assert_eq!(digest.finalize(), sealed.slot_crc);

        tail[100] ^= 0xFF;
        let mut torn = header_digest(&slot().seal(&STORE, &tail));
        torn.update(&tail);
        assert_ne!(torn.finalize(), sealed.slot_crc);
    }

    #[test]
    fn a_slot_read_from_the_wrong_position_or_the_wrong_store_is_refused() {
        let header = slot().seal(&STORE, &tail(0));
        assert_eq!(Slot::decode(&header, 4, &STORE, 30_718).unwrap_err().reason, Reason::Position);
        assert_eq!(Slot::decode(&header, 3, &StoreId([1; 16]), 30_718).unwrap_err().reason, Reason::StoreId);
        assert_eq!(Slot::decode(&ZERO_PAD[..BLOCK], 0, &STORE, 30_718).unwrap_err().reason, Reason::Magic);
    }

    /// §7.1: a flushed length that is not a multiple of the program page, and a tail above the slot's
    /// 32,256-byte area, are both invalid — the binding limit is the tail area, not the `u32`.
    #[test]
    fn the_flush_boundary_and_the_tail_bound_are_enforced() {
        let mut broken = slot();
        broken.flushed += 1;
        let header = broken.seal(&STORE, &[]);
        assert_eq!(Slot::decode(&header, 3, &STORE, 30_718).unwrap_err().reason, Reason::Count);

        let mut long = slot();
        long.tail_len = TAIL_CAPACITY as u32 + 1;
        let header = long.seal(&STORE, &[]);
        assert_eq!(Slot::decode(&header, 3, &STORE, 30_718).unwrap_err().reason, Reason::Count);

        long.tail_len = TAIL_CAPACITY as u32;
        assert!(Slot::decode(&long.seal(&STORE, &[]), 3, &STORE, 30_718).is_ok());
    }

    /// §7.2's headroom: step 2 leaves at most 16,383 bytes behind, so an interval may add 15,873
    /// before the tail would not fit — three orders of magnitude above what ten seconds produces.
    #[test]
    fn the_tail_area_leaves_section_7_2s_headroom() {
        assert_eq!(TAIL_CAPACITY - (PROGRAM_PAGE - 1), 15_873);
        assert_eq!(SLOT_LEN, 32_768);
        assert_eq!(SLOT_LEN, 2 * PROGRAM_PAGE);
    }

    #[test]
    fn a_slots_ranges_are_the_rides_own() {
        let ride = |first: u16| super::super::catalog::Entry {
            meta: super::super::seam::EntryMeta {
                id: ObjectId(2),
                revision: Revision(1),
                kind: super::super::seam::ObjectKind::Ride,
                flags: super::super::seam::EntryFlags::RECORDING,
                payload_len: 0,
                payload_crc: 0,
                name: super::super::seam::DisplayName::default(),
            },
            ranges: {
                let mut ranges = Ranges::default();
                ranges.push(first, 32).unwrap();
                ranges
            },
        };
        assert!(slot().describes(&ride(13)));
        assert!(!slot().describes(&ride(20)), "a slot over reused extents was read as this ride's");
    }
}
