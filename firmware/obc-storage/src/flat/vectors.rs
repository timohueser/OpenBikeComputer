//! Every test vector in both frozen specs, pinned byte for byte.
//!
//! `FLAT_Store_Format.md` §4.1, §5.7, §6.1 and §7.5 are the on-card ones, and this crate encodes
//! them, so those are pinned against the encoder's own output. `FLAT_Store_Protocol.md` §3.10's four
//! frames belong to the engine, which is a later slice; what is pinned of them here is the half this
//! contract owns — they carry §5.7's objects and §6.1's offset, and the two documents agree about
//! them or one of the two is wrong.
//!
//! The hex in each fence is the spec's own, transcribed. A vector that stops describing what the
//! encoder produces is either a code bug or a spec bug, and either way it stops the build.

use std::vec::Vec;

use super::catalog::{Entry, Gate, Header};
use super::journal::{Slot, TAIL_CAPACITY};
use super::layout::{Ranges, BLOCK, ENTRY_STRIDE};
use super::raw::{crc32, u16_at, u32_at, u64_at};
use super::seam::{DisplayName, EntryFlags, EntryMeta, ObjectId, ObjectKind, Revision, Store, StoreId};
use super::sim::SparseDisk;
use super::store::FlatStore;

/// §4.1's `StoreId`.
const STORE: StoreId =
    StoreId([0x8F, 0x2C, 0x41, 0xD9, 0x6B, 0x07, 0x4E, 0xA3, 0xB1, 0x55, 0x9C, 0x20, 0x7D, 0xE8, 0x34, 0x66]);
/// §4.1's card: a 32 GB card, 62,914,560 blocks.
const TOTAL_BLOCKS: u64 = 62_914_560;
/// The extents §6 recomputes from it.
const EXTENTS: u32 = 30_718;

/// Transcribes a spec hex fence: whitespace-separated byte pairs, `//` comments stripped.
fn hex(text: &str) -> Vec<u8> {
    text.split_whitespace()
        .take_while(|token| *token != "//")
        .map(|token| u8::from_str_radix(token, 16).expect("a hex byte pair"))
        .collect()
}

/// Asserts `bytes` opens with the spec's fence and is zero from there to the end.
fn assert_prefix_then_zero(bytes: &[u8], prefix: &str) {
    let expected = hex(prefix);
    assert_eq!(&bytes[..expected.len()], &expected[..], "the leading bytes are not the spec's");
    assert!(bytes[expected.len()..].iter().all(|&byte| byte == 0), "the tail past the fence is not zero");
}

/// §5.7's route: `ObjectId 1` at `Revision 3`, 42,137 bytes in extent 12, named "Grimsel Loop".
fn route() -> Entry {
    let mut ranges = Ranges::default();
    ranges.push(12, 1).unwrap();
    Entry {
        meta: EntryMeta {
            id: ObjectId(1),
            revision: Revision(3),
            kind: ObjectKind::Route,
            flags: EntryFlags::NONE,
            payload_len: 42_137,
            payload_crc: 0x9C4A_7E21,
            name: DisplayName::new("Grimsel Loop").unwrap(),
        },
        ranges,
    }
}

/// §5.7's ride: `ObjectId 2` at `Revision 1`, `RECORDING`, a 32 MiB reserve from extent 13.
fn ride() -> Entry {
    let mut ranges = Ranges::default();
    ranges.push(13, 32).unwrap();
    Entry {
        meta: EntryMeta {
            id: ObjectId(2),
            revision: Revision(1),
            kind: ObjectKind::Ride,
            flags: EntryFlags::RECORDING,
            payload_len: 0,
            payload_crc: 0,
            name: DisplayName::default(),
        },
        ranges,
    }
}

fn catalog_header() -> Header {
    Header { store: STORE, sequence: 7, next_object: 3, entry_count: 2 }
}

/// §5.7's body: the header followed by the two entries, 768 bytes.
fn body() -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&catalog_header().encode());
    body.extend_from_slice(&route().encode());
    body.extend_from_slice(&ride().encode());
    body
}

/// §4.1, both cards: the 32 GB one §8 gives 1 MiB extents, and the 128 GiB one it gives 2 MiB — the
/// card the old fixed-size format could not address at all.
#[test]
fn superblock_vector() {
    let bytes = super::superblock::Superblock::for_card(STORE, TOTAL_BLOCKS).expect("§4.1's card").encode();
    assert_prefix_then_zero(
        &bytes[..504],
        "46 53 53 42 01 00 00 00 8F 2C 41 D9 6B 07 4E A3
         B1 55 9C 20 7D E8 34 66 00 00 C0 03 00 00 00 00
         14",
    );
    assert_eq!(u32_at(&bytes, 504), 0x5374_4CB7, "§4.1's CRC-32 over bytes 0..504");
    assert!(bytes[508..].iter().all(|&byte| byte == 0));
    let superblock = super::superblock::Superblock::decode(&bytes).expect("§4.1's superblock decodes");
    assert_eq!(superblock.geometry.extent_size(), 1 << 20, "§4.1: a 32 GB card gets the 1 MiB minimum");
    assert_eq!(superblock.extent_count(), EXTENTS, "§4.1: 30,718 extents");

    // §4.1's second card: 268,435,456 blocks — 128 GiB — whose geometry byte is 21 rather than 20.
    let bytes = super::superblock::Superblock::for_card(STORE, 268_435_456).expect("§4.1's larger card").encode();
    assert_prefix_then_zero(
        &bytes[..504],
        "46 53 53 42 01 00 00 00 8F 2C 41 D9 6B 07 4E A3
         B1 55 9C 20 7D E8 34 66 00 00 00 10 00 00 00 00
         15",
    );
    assert_eq!(u32_at(&bytes, 504), 0xE337_E72D, "§4.1's CRC-32 for the 128 GiB card");
    let superblock = super::superblock::Superblock::decode(&bytes).expect("the larger superblock decodes");
    assert_eq!(superblock.geometry.extent_size(), 2 << 20, "§8: 128 GiB / 65,536 is 2 MiB");
    assert_eq!(superblock.extent_count(), 65_535, "§6: the extent area is one extent short of the index");
}

/// §5.7, the header block.
#[test]
fn catalog_header_vector() {
    let bytes = catalog_header().encode();
    assert_prefix_then_zero(
        &bytes,
        "46 53 43 54 01 00 80 00 8F 2C 41 D9 6B 07 4E A3
         B1 55 9C 20 7D E8 34 66 07 00 00 00 00 00 00 00
         03 00 00 00 00 00 00 00 02 00 00 00 00 00 00 00",
    );
}

/// §5.7, entry 0.
#[test]
fn route_entry_vector() {
    let bytes = route().encode();
    assert_eq!(
        &bytes[..],
        &hex("01 00 00 00 01 0C 00 00 01 00 00 00 00 00 00 00
              03 00 00 00 00 00 00 00 99 A4 00 00 00 00 00 00
              21 7E 4A 9C 00 00 00 00 0C 00 01 00 00 00 00 00
              00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
              00 00 00 00 00 00 00 00 47 72 69 6D 73 65 6C 20
              4C 6F 6F 70 00 00 00 00 00 00 00 00 00 00 00 00
              00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
              00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00")[..]
    );
    assert_eq!(Entry::decode(&bytes, EXTENTS).unwrap(), route());
}

/// §5.7, entry 1.
#[test]
fn ride_entry_vector() {
    let bytes = ride().encode();
    assert_prefix_then_zero(
        &bytes,
        "03 00 01 00 01 00 00 00 02 00 00 00 00 00 00 00
         01 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
         00 00 00 00 00 00 00 00 0D 00 20 00 00 00 00 00",
    );
    assert_eq!(Entry::decode(&bytes, EXTENTS).unwrap(), ride());
}

/// §5.7: the body is those 768 bytes, and its CRC is what the gate carries.
#[test]
fn catalog_body_crc_vector() {
    let body = body();
    assert_eq!(body.len(), BLOCK + 2 * ENTRY_STRIDE);
    assert_eq!(body.len(), 768);
    assert_eq!(crc32(&body), 0x9C1D_23F9);
}

/// §5.7, the gate of copy A.
#[test]
fn catalog_gate_vector() {
    let gate = Gate { copy: 0, store: STORE, sequence: 7, entry_count: 2, body_crc: 0x9C1D_23F9 };
    let bytes = gate.encode();
    assert_prefix_then_zero(
        &bytes[..504],
        "46 53 43 47 01 00 00 00 8F 2C 41 D9 6B 07 4E A3
         B1 55 9C 20 7D E8 34 66 07 00 00 00 00 00 00 00
         02 00 00 00 F9 23 1D 9C 00 00 00 00 00 00 00 00",
    );
    assert_eq!(u32_at(&bytes, 504), 0x9355_31B8, "§5.7's gate CRC-32 over bytes 0..504");
    assert!(bytes[508..].iter().all(|&byte| byte == 0));
    assert_eq!(Gate::decode(&bytes, 0, &STORE).unwrap(), gate);
}

/// §7.5: slot 3, checkpoint 41, 15 pages flushed, 3,712 tail bytes.
#[test]
fn ride_journal_slot_vector() {
    let tail: Vec<u8> = (0..3_712).map(|index| (index * 7 + 3) as u8).collect();
    let mut ranges = Ranges::default();
    ranges.push(13, 32).unwrap();
    let slot = Slot {
        slot: 3,
        id: ObjectId(2),
        revision: Revision(1),
        sequence: 41,
        flushed: 245_760,
        tail_len: 3_712,
        payload_crc: 0x5E1B_03C7,
        ranges,
        slot_crc: 0,
    };
    let header = slot.seal(&STORE, &tail);
    assert_prefix_then_zero(
        &header[..504],
        "46 53 52 4A 01 00 03 00 8F 2C 41 D9 6B 07 4E A3
         B1 55 9C 20 7D E8 34 66 02 00 00 00 00 00 00 00
         01 00 00 00 00 00 00 00 29 00 00 00 00 00 00 00
         00 C0 03 00 00 00 00 00 80 0E 00 00 C7 03 1B 5E
         0D 00 20 00 00 00 00 00 00 00 00 00 00 00 00 00",
    );
    assert_eq!(u32_at(&header, 504), 0x66E5_6BD6, "§7.5's CRC over the whole 32,768-byte slot");
    assert!(header[508..].iter().all(|&byte| byte == 0));

    // The tail's first sixteen bytes, as the fence states them.
    assert_eq!(&tail[..16], &hex("03 0A 11 18 1F 26 2D 34 3B 42 49 50 57 5E 65 6C")[..]);
    assert_eq!(slot.payload_len(), 249_472, "§7.5: the total is derived, not stored");
    assert_eq!(TAIL_CAPACITY, 32_256);
}

/// §9's capacities, gathered: each is normative where it is defined, and this is the table.
#[test]
fn capacity_table() {
    use super::layout::{Geometry, EXTENT_AREA, MAX_EXTENTS, MAX_RANGES, MIN_EXTENT_SIZE, PROGRAM_PAGE, SLOTS};
    assert_eq!(PROGRAM_PAGE, 16_384);
    assert_eq!(MIN_EXTENT_SIZE, 1 << 20, "§9's extent size, at the minimum §8 never goes below");
    assert_eq!(Geometry::DEFAULT.extent_size(), MIN_EXTENT_SIZE);
    assert_eq!(EXTENT_AREA, 4_096);
    assert_eq!(MAX_EXTENTS, 65_536);
    assert_eq!(MAX_EXTENTS as usize / 8, 8_192, "the resident free bitmap");
    // §9's extent area: 64 GiB at the 1 MiB minimum, and 65,536 extents at every size above it.
    assert_eq!(MAX_EXTENTS as u64 * MIN_EXTENT_SIZE, 64 << 30);
    assert_eq!(MAX_EXTENTS as u64 * Geometry::from_log2(31).unwrap().extent_size(), 128 << 40);
    assert_eq!(super::layout::CATALOG_BLOCKS as usize * BLOCK, 262_144, "one catalog copy");
    assert_eq!(super::layout::ENTRY_CAPACITY, 1_916);
    assert_eq!(ENTRY_STRIDE, 128);
    assert_eq!(MAX_RANGES, 8);
    assert_eq!(super::seam::NAME_CAPACITY, 48);
    assert_eq!(SLOTS, 16);
    assert_eq!(super::journal::SLOT_LEN, 32_768);
    assert_eq!(TAIL_CAPACITY, 32_256);
}

/// `FLAT_Store_Protocol.md` §3.10's `LIST` response carries §5.7's two objects. Its 88-byte entries
/// are the format entries' metadata, and this is the seam between the two specs.
#[test]
fn wire_list_entries_carry_the_same_metadata() {
    let frame = hex("4F 42 43 34 04 01 01 00 C8 00 00 00 02 2A 00 00
                     8F 2C 41 D9 6B 07 4E A3 B1 55 9C 20 7D E8 34 66
                     07 00 00 00 00 00 00 00 01 00 00 00 00 00 00 00
                     03 00 00 00 00 00 00 00 99 A4 00 00 00 00 00 00
                     21 7E 4A 9C 01 00 00 00 0C 00 00 00 47 72 69 6D
                     73 65 6C 20 4C 6F 6F 70 00 00 00 00 00 00 00 00
                     00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
                     00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
                     02 00 00 00 00 00 00 00 01 00 00 00 00 00 00 00
                     00 00 00 00 00 00 00 00 00 00 00 00 03 00 01 00
                     00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
                     00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
                     00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
                     00 00 00 00 00 00 00 00");
    assert_eq!(frame.len(), 216);
    assert_eq!(u16_at(&frame, 8) as usize, frame.len() - 16, "the payload length is the rest of the frame");
    assert_eq!(&frame[16..32], &STORE.0[..]);
    assert_eq!(u64_at(&frame, 32), catalog_header().sequence);

    for (index, entry) in [route(), ride()].into_iter().enumerate() {
        let at = 40 + index * 88;
        assert_eq!(u64_at(&frame, at), entry.meta.id.0);
        assert_eq!(u64_at(&frame, at + 8), entry.meta.revision.0);
        assert_eq!(u64_at(&frame, at + 16), entry.meta.payload_len);
        assert_eq!(u32_at(&frame, at + 24), entry.meta.payload_crc);
        assert_eq!(u16_at(&frame, at + 28), entry.meta.kind as u16);
        assert_eq!(u16_at(&frame, at + 30), entry.meta.flags.bits());
        assert_eq!(frame[at + 32] as usize, entry.meta.name.len());
        assert_eq!(&frame[at + 36..at + 84], &entry.meta.name.padded()[..]);
    }
}

/// §3.10's other three fences. FS3 owns no wire encoder — the control frame is the engine's — so what
/// is pinned here is the half that belongs to this contract: the object metadata and the payload
/// offset those frames carry are the format's own, and the frame's self-description agrees with its
/// length.
#[test]
fn the_wire_vectors_carry_the_formats_own_values() {
    // `PUT` creating the route: `ObjectId` zero, and the route's declared length, CRC, kind and name.
    let put = hex("4F 42 43 34 04 04 00 00 54 00 00 00 01 2A 00 00
                   00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
                   99 A4 00 00 00 00 00 00 21 7E 4A 9C 01 00 00 00
                   0C 00 00 00 47 72 69 6D 73 65 6C 20 4C 6F 6F 70
                   00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
                   00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
                   00 00 00 00");
    let entry = route();
    let route = entry.meta;
    assert_eq!(put.len(), 100);
    assert_eq!(u16_at(&put, 8) as usize, put.len() - 16, "the payload length is the rest of the frame");
    assert_eq!(u64_at(&put, 16), 0, "a create sends ObjectId zero");
    assert_eq!(u64_at(&put, 24), 0, "and no expected revision");
    assert_eq!(u64_at(&put, 32), route.payload_len);
    assert_eq!(u32_at(&put, 40), route.payload_crc);
    assert_eq!(u16_at(&put, 44), route.kind as u16);
    assert_eq!(put[48] as usize, route.name.len());
    assert_eq!(&put[52..52 + route.name.len()], route.name.as_bytes());

    // A stream frame of that upload, at §6.1's worked offset.
    let stream = hex("01 2A 00 00 00 A0 00 00 00 00 00 00 00 04 00 00");
    assert_eq!(stream.len(), 16);
    assert_eq!(u32_at(&stream, 0), u32_at(&put, 12), "the transfer is named by its request");
    let offset = u64_at(&stream, 4);
    assert_eq!(offset, 40_960);
    assert_eq!(u16_at(&stream, 12), 1_024);
    assert_eq!(
        entry.ranges.locate(super::layout::Geometry::DEFAULT, offset).unwrap(),
        super::layout::Located { block: 28_752, offset: 0, contiguous: (1 << 20) - 40_960 },
        "§6.1: that offset is LBA 28,752 of extent 12",
    );

    // The error response if the route already exists at another revision.
    let error = hex("4F 42 43 34 04 04 03 00 10 00 00 00 01 2A 00 00
                     05 00 01 00 05 00 00 00 00 00 00 00 00 00 00 00");
    assert_eq!(error.len(), 32);
    assert_eq!(u16_at(&error, 6), 0b11, "response | error");
    assert_eq!(u16_at(&error, 8) as usize, error.len() - 16);
    assert_eq!(u16_at(&error, 16), 5, "revisionConflict");
    assert_eq!(u16_at(&error, 18), 1, "headDiffers");
    // The context is the current head revision, which is the value the store's own refusal carries.
    assert_eq!(
        super::error::StoreError::RevisionConflict { current: Revision(u64_at(&error, 20)) },
        super::error::StoreError::RevisionConflict { current: Revision(5) },
    );
}

/// The vectors as a card: §4.1's superblock, §5.7's catalog in copy A, and nothing else. A mount of it
/// must produce exactly those two objects, that sequence, that cursor, and a free map that is the
/// catalog's complement.
#[test]
fn a_card_built_from_the_vectors_mounts_to_the_vectors() {
    let disk = SparseDisk::blank(TOTAL_BLOCKS, 1);
    let superblock = super::superblock::Superblock::for_card(STORE, TOTAL_BLOCKS).expect("§4.1's card").encode();
    disk.install(super::layout::SUPERBLOCK[0], &superblock);
    disk.install(super::layout::SUPERBLOCK[1], &superblock);
    disk.install(super::layout::CATALOG[0], &body());
    let gate = Gate { copy: 0, store: STORE, sequence: 7, entry_count: 2, body_crc: 0x9C1D_23F9 };
    disk.install(super::layout::catalog_gate(0), &gate.encode());

    let store = FlatStore::mount(&disk);
    assert_eq!(store.mode(), super::store::Mode::ReadWrite);
    assert_eq!(store.store_id(), STORE);
    assert_eq!(store.sequence(), 7);
    assert_eq!(store.next_object_id(), ObjectId(3));
    assert_eq!(store.entries().collect::<Vec<_>>(), std::vec![route().meta, ride().meta]);
    assert_eq!(store.free_extents(), EXTENTS - 33, "extent 12 and the ride's 32-extent reserve");

    // §6.1's worked example, through the seam this time: payload offset 40,960 of the route.
    disk.install(28_752, &[0xAB; BLOCK]);
    let handle = store.open(ObjectId(1), None).unwrap();
    let mut buf = [0u8; 16];
    assert_eq!(store.read(&handle, 40_960, &mut buf).unwrap(), 16);
    assert_eq!(buf, [0xAB; 16]);

    // A recording ride with no valid slot resumes at checkpoint 1 with nothing flushed.
    assert_eq!(store.recovered_ride(), None);
}
