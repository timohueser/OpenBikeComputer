//! Decoder fuzzing: no input panics, and a mutation that survives the CRC still has to satisfy the
//! structural rules.
//!
//! Two halves, because they prove different things. Raw byte flips prove **totality** — every input is
//! either a typed record or a typed refusal, and nothing indexes past an unchecked length. Re-stamped
//! mutations, where the CRC and the gate are rebuilt over the mutated bytes, prove the structural
//! rules exist at all: a CRC catches corruption and says nothing about whether a decoder enforces a
//! header rule.
//!
//! The re-stamped half is held to a floor **per rule** rather than to one aggregate count, because
//! "reached a structural rule" is satisfiable by one trivial rule firing every round — which is
//! exactly the shape of coverage that looks like evidence and is not.

use std::vec::Vec;

use super::catalog::{Entry, Gate, Header};
use super::error::{Reason, Record};
use super::journal::Slot;
use super::layout::{Ranges, BLOCK, ENTRY_STRIDE};
use super::seam::{DisplayName, EntryFlags, EntryMeta, ObjectId, ObjectKind, Revision, StoreId};
use super::sim::SparseDisk;
use super::store::FlatStore;
use super::superblock::Superblock;

const STORE: StoreId = StoreId([0x33; 16]);
const EXTENTS: u32 = 64;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() as usize) % bound
    }
}

fn sample_entry() -> Entry {
    let mut ranges = Ranges::default();
    ranges.push(4, 2).unwrap();
    ranges.push(9, 1).unwrap();
    Entry {
        meta: EntryMeta {
            id: ObjectId(7),
            revision: Revision(3),
            kind: ObjectKind::MapShard,
            flags: EntryFlags::RETAINED,
            payload_len: 3 << 20,
            payload_crc: 0x1234_5678,
            name: DisplayName::new("Berner Oberland").unwrap(),
        },
        ranges,
    }
}

fn sample_slot() -> Slot {
    let mut ranges = Ranges::default();
    ranges.push(13, 32).unwrap();
    Slot {
        slot: 5,
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

/// Totality, one record shape at a time: 4,000 raw flips each, and every refusal is typed.
#[test]
fn no_byte_flip_of_any_record_panics_or_is_accepted_as_itself() {
    let mut rng = Rng(0x0F0F_0F0F_0F0F_0F0F);
    let superblock = Superblock { store: STORE, total_blocks: 62_914_560 }.encode();
    let header = Header { store: STORE, sequence: 9, next_object: 12, entry_count: 40 }.encode();
    let entry = sample_entry().encode();
    let gate = Gate { copy: 1, store: STORE, sequence: 9, entry_count: 40, body_crc: 0xDEAD_BEEF }.encode();
    let slot = sample_slot().seal(&STORE, &[]);

    for _ in 0..4_000 {
        let mut torn = superblock;
        torn[rng.below(BLOCK)] ^= (rng.next() as u8) | 1;
        assert!(Superblock::decode(&torn).is_err() || torn == superblock);

        let mut torn = header;
        torn[rng.below(BLOCK)] ^= (rng.next() as u8) | 1;
        if let Ok(decoded) = Header::decode(&torn, &STORE) {
            assert!(decoded.entry_count as usize <= super::layout::ENTRY_CAPACITY);
        }

        let mut torn = entry;
        torn[rng.below(ENTRY_STRIDE)] ^= (rng.next() as u8) | 1;
        if let Ok(decoded) = Entry::decode(&torn, EXTENTS) {
            assert!(decoded.ranges.len() <= super::layout::MAX_RANGES);
            assert!(decoded.ranges.iter().all(|(first, count)| first as u32 + count as u32 <= EXTENTS));
        }

        let mut torn = gate;
        torn[rng.below(BLOCK)] ^= (rng.next() as u8) | 1;
        assert!(Gate::decode(&torn, 1, &STORE).is_err(), "a flipped gate was accepted");

        let mut torn = slot;
        torn[rng.below(BLOCK)] ^= (rng.next() as u8) | 1;
        if let Ok(decoded) = Slot::decode(&torn, 5, &STORE, EXTENTS) {
            assert!(decoded.tail_len as usize <= super::journal::TAIL_CAPACITY);
            assert_eq!(decoded.flushed % super::layout::PROGRAM_PAGE as u64, 0);
        }
    }
}

/// The structural half: the entry's own CRC is the catalog body's, so a mutated entry reaches
/// [`Entry::decode`] and the array rules regardless — which makes the entry the one record where a
/// flip is *always* re-stamped. Each case names the rule it must trip.
#[test]
fn field_mutations_reach_the_entrys_structural_rules() {
    let cases: [(usize, &[u8], Reason); 8] = [
        (0, &0u16.to_le_bytes(), Reason::UnknownEnum),
        (2, &(1u16 << 5).to_le_bytes(), Reason::UnknownEnum),
        (4, &[0], Reason::Count),
        (5, &[49], Reason::Count),
        (6, &[1, 0], Reason::Reserved),
        (8, &0u64.to_le_bytes(), Reason::Zero),
        (16, &0u64.to_le_bytes(), Reason::Zero),
        (44, &99u16.to_le_bytes(), Reason::Ranges),
    ];
    for (offset, replacement, expected) in cases {
        let mut bytes = sample_entry().encode();
        bytes[offset..offset + replacement.len()].copy_from_slice(replacement);
        let error = Entry::decode(&bytes, EXTENTS).expect_err("a field mutation was accepted");
        assert_eq!(error.reason, expected, "the field at {offset} gave {error:?}");
        assert_eq!(error.record, Record::Entry);
    }
}

/// The same for the entry array's cross-entry rules, driven through a whole mounted card: a body whose
/// entries break a §5.3 rule is a structurally invalid copy, and a card with only that copy mounts
/// read-only rather than serving it.
#[test]
fn a_structurally_invalid_entry_array_is_never_served() {
    let base = sample_entry();
    let arrays: [Vec<Entry>; 5] = [
        // Out of order.
        std::vec![Entry { meta: EntryMeta { id: ObjectId(9), ..base.meta }, ..base }, base],
        // Two heads of one object.
        std::vec![
            Entry { meta: EntryMeta { flags: EntryFlags::NONE, revision: Revision(2), ..base.meta }, ..base },
            Entry { meta: EntryMeta { flags: EntryFlags::NONE, revision: Revision(3), ..base.meta }, ..base },
        ],
        // A lone retained revision, with no head.
        std::vec![base],
        // Overlapping extents.
        std::vec![
            Entry { meta: EntryMeta { flags: EntryFlags::NONE, ..base.meta }, ..base },
            Entry { meta: EntryMeta { id: ObjectId(8), flags: EntryFlags::NONE, ..base.meta }, ..base },
        ],
        // Two entries recording at once.
        std::vec![
            Entry {
                meta: EntryMeta { kind: ObjectKind::Ride, flags: EntryFlags::RECORDING, payload_len: 0, ..base.meta },
                ..base
            },
            Entry {
                meta: EntryMeta {
                    id: ObjectId(8),
                    kind: ObjectKind::Ride,
                    flags: EntryFlags::RECORDING,
                    payload_len: 0,
                    ..base.meta
                },
                ranges: {
                    let mut ranges = Ranges::default();
                    ranges.push(20, 1).unwrap();
                    ranges
                },
            },
        ],
    ];

    for (index, entries) in arrays.into_iter().enumerate() {
        let total_blocks = super::layout::EXTENT_AREA + super::layout::EXTENT_BLOCKS * EXTENTS as u64;
        let disk = SparseDisk::blank(total_blocks, index as u64 + 1);
        let superblock = Superblock { store: STORE, total_blocks }.encode();
        disk.install(super::layout::SUPERBLOCK[0], &superblock);

        let header = Header { store: STORE, sequence: 4, next_object: 99, entry_count: entries.len() as u16 };
        let mut body = Vec::new();
        body.extend_from_slice(&header.encode());
        for entry in &entries {
            body.extend_from_slice(&entry.encode());
        }
        disk.install(super::layout::CATALOG[0], &body);
        let gate = Gate {
            copy: 0,
            store: STORE,
            sequence: 4,
            entry_count: entries.len() as u16,
            body_crc: super::raw::crc32(&body),
        };
        disk.install(super::layout::catalog_gate(0), &gate.encode());

        assert_eq!(
            FlatStore::mount(&disk).mode(),
            super::store::Mode::CatalogUnreadable,
            "array {index} was served despite breaking a §5.3 rule",
        );
    }
}

/// A whole card of random bytes: a mount of it never panics and never claims to be writable.
#[test]
fn random_cards_never_mount_writable() {
    let mut rng = Rng(0xDEAD_BEEF_CAFE_F00D);
    let total_blocks = super::layout::EXTENT_AREA + super::layout::EXTENT_BLOCKS * EXTENTS as u64;
    let mut modes: Vec<super::store::Mode> = Vec::new();
    for round in 0..200u64 {
        let disk = SparseDisk::blank(total_blocks, round + 1);
        // The regions a mount actually reads: both superblocks, both gates, both headers, some entry
        // blocks and a few journal slots.
        for lba in [0u64, 32, 64, 65, 66, 544, 576, 577, 1_056, 1_088, 1_152] {
            let mut block = [0u8; BLOCK];
            for byte in block.iter_mut() {
                *byte = rng.next() as u8;
            }
            disk.install(lba, &block);
        }
        let mode = FlatStore::mount(&disk).mode();
        assert_ne!(mode, super::store::Mode::ReadWrite, "random bytes mounted writable in round {round}");
        modes.push(mode);
    }
    assert!(modes.iter().all(|mode| *mode == super::store::Mode::Unformatted), "{modes:?}");
}

/// And the other direction: a real card whose *superblock* survives but whose catalog is random is
/// read-only, not writable — the case where the store has an identity and nothing to serve.
#[test]
fn a_valid_superblock_over_a_random_catalog_is_read_only() {
    let mut rng = Rng(0x1234_5678_9ABC_DEF0);
    let total_blocks = super::layout::EXTENT_AREA + super::layout::EXTENT_BLOCKS * EXTENTS as u64;
    for round in 0..200u64 {
        let disk = SparseDisk::blank(total_blocks, round + 1);
        let superblock = Superblock { store: STORE, total_blocks }.encode();
        disk.install(super::layout::SUPERBLOCK[0], &superblock);
        for lba in [64u64, 65, 544, 576, 577, 1_056] {
            let mut block = [0u8; BLOCK];
            for byte in block.iter_mut() {
                *byte = rng.next() as u8;
            }
            disk.install(lba, &block);
        }
        let store = FlatStore::mount(&disk);
        assert_eq!(store.mode(), super::store::Mode::CatalogUnreadable, "round {round}");
        assert_eq!(store.store_id(), STORE, "the identity is still readable");
    }
}
