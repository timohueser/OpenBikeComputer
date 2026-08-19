//! **The read cutover, held to the seam it crosses** (FS7.5-c2, #1420): a real OBCM map committed
//! into a flat store, and the renderer's own reader driven over it through [`StoreSource`].
//!
//! Every other test of this store is about the *store*: extents, gates, commits, recovery. This one
//! is about the sentence the board now depends on — *"a map object in a flat store is a map"* — and
//! it is the flat arm's answer to the FAT arm's fixtures, which have been reading `.obcm` bytes off
//! a `SliceSource` since the first reader test.
//!
//! The oracle is deliberately the **same bytes through a second path**: `SliceSource` over the file
//! the testkit built, against `StoreSource` over the object the store committed. If the two ever
//! disagree, the disagreement is the store's or the adapter's, because the map is one array of bytes
//! in both. That is a stronger check than asserting field values, and it is what a golden-frame
//! comparison would be measuring anyway one layer up.
//!
//! What this does **not** cover, and where it is covered instead: the board's boot wiring
//! (`flat_store::open_map` — bare metal, no tests run there, so the on-glass row of the pair's
//! acceptance is what closes it), and the renderer's pixels (`obc-render`'s suites, which are
//! source-agnostic by construction).

use std::vec::Vec;

use obc_formats::io::{ByteSource, SliceSource, WindowSource};
use obc_reader::{MapCache, MapTables, Reader};
use obcm_testkit::{build_file, pack_line, pack_poly_hole, seal, splice_terrain, terrain_stub, LodSpec, Style};

use crate::flat::layout::{Geometry, EXTENT_AREA};
use crate::flat::seam::ObjectId;
use crate::flat::seam::{DisplayName, EntryFlags, EntryMeta, Mutation, ObjectKind, PutSource, Revision, StoreId};
use crate::flat::sim::SparseDisk;
use crate::flat::store::FlatStore;
use crate::flat::Store as _;

const STORE: StoreId = StoreId([0xC2; 16]);
const CS: usize = 64;
const GLOBAL: (i32, i32, i32, i32) = (0, 0, 1000, 1000);
const STYLES: &[Style] = &[(1, 3, 0xF800, 2, 3, false, None), (2, -1, 0x07E0, 1, 3, false, None)];

/// A two-LOD map: one line at the coarse rung, one polygon-with-hole at the fine one. The same
/// shape `obc-reader`'s own format suite reads off a slice.
fn map_bytes() -> Vec<u8> {
    let line = seal(pack_line(1, 100, 200, &[(10, 0), (0, 10)]), CS);
    let poly = seal(
        pack_poly_hole(2, 100, 100, &[(100, 0), (0, 100), (-100, 0)], &[(25, 25), (50, 0), (0, 50), (-50, 0)]),
        CS,
    );
    build_file(
        GLOBAL,
        STYLES,
        &[
            LodSpec { max_mpp: f32::INFINITY, index: std::vec![0], chunks: std::vec![line], chunk_size: CS },
            LodSpec { max_mpp: 50.0, index: std::vec![0], chunks: std::vec![poly], chunk_size: CS },
        ],
    )
}

/// Publish `payload` as a committed object of `kind` under `name`, and hand back its id.
fn publish(store: &FlatStore<&SparseDisk>, payload: &[u8], name: &str) -> ObjectId {
    let id = store.next_object_id();
    let mut allocation = store.allocate(payload.len() as u64).expect("the extents are free");
    store.write(&mut allocation, payload).expect("the payload fits");
    let meta = EntryMeta {
        id,
        revision: Revision(1),
        kind: ObjectKind::MapShard,
        flags: EntryFlags::NONE,
        payload_len: payload.len() as u64,
        payload_crc: obc_crc::crc32(payload),
        name: DisplayName::new(name).expect("a short name"),
    };
    store.commit(&[Mutation::Put { meta, source: PutSource::Fresh(allocation) }]).expect("the commit lands");
    id
}

/// A card carrying `payload` as one committed map object, and the id it went in under.
///
/// **The map is laid down over a hole, on purpose.** A blank card would give it one contiguous run,
/// and a contiguous run is the case `StoreSource` is *least* interesting for — the whole reason the
/// adapter exists is that an object's bytes are a list of ranges and a read has to walk them. So
/// this fills two extents, frees the first, and then publishes the map: the allocator takes the hole
/// and continues past the survivor, leaving the map on **two non-adjacent ranges** with the seam at
/// exactly one extent. Callers size their payload past [`Geometry::extent_size`] to be sure of it,
/// and the reads below straddle that seam deliberately. (The same shape `crash.rs` uses to build a
/// fragmented free map.)
fn card_with_map(payload: &[u8]) -> (SparseDisk, ObjectId) {
    let extent = Geometry::DEFAULT.extent_size() as usize;
    // The map, the two spacers, and a few spare so the commit is never the thing under test.
    let extents = payload.len().div_ceil(extent) as u64 + 6;
    let disk = SparseDisk::blank(EXTENT_AREA + Geometry::DEFAULT.extent_blocks() * extents, 3);
    let store = FlatStore::initialize(&disk, STORE).expect("an expressible card");

    let free_before = store.free_extents();
    let hole = publish(&store, &std::vec![0xA5u8; 8], "spacer-a");
    let keep = publish(&store, &std::vec![0x5Au8; 8], "spacer-b");
    assert_eq!(store.free_extents(), free_before - 2, "each spacer took one extent");
    store.commit(&[Mutation::Remove { id: hole, revision: Revision(1) }]).expect("the spacer is removed");
    assert_eq!(store.free_extents(), free_before - 1, "and the first extent is a hole again");

    let id = publish(&store, payload, "two-lod");
    assert!(payload.len() > extent, "the map must outgrow one extent, or the hole buys nothing");
    // `keep` is never read; it exists to sit between the hole and the rest of the free map.
    let _ = keep;
    (disk, id)
}

/// The map, padded past one extent so it is guaranteed to span the fragmented allocation
/// [`card_with_map`] builds.
///
/// The padding is trailing bytes past every section OBCM's header points at, so the file parses and
/// renders exactly as the unpadded one does — the reader bounds-checks each offset against the
/// source's length and never reads the tail. What it buys is a *real* multi-extent object rather
/// than a few hundred bytes that would fit any single range.
fn padded_map_bytes() -> Vec<u8> {
    let mut bytes = map_bytes();
    let want = Geometry::DEFAULT.extent_size() as usize + 4_096;
    let from = bytes.len();
    bytes.resize(want, 0);
    for (k, byte) in bytes[from..].iter_mut().enumerate() {
        *byte = (k as u8).wrapping_mul(29).wrapping_add(11);
    }
    bytes
}

/// The whole point of the slice: **the map the store hands back is the map that went in.** Read
/// through the seam the renderer uses, at every window size a chunk read can take, against the
/// bytes as an oracle.
#[test]
fn a_map_committed_to_the_store_reads_back_byte_for_byte() {
    let bytes = padded_map_bytes();
    let (disk, id) = card_with_map(&bytes);
    let store = FlatStore::mount(&disk);
    let source = store.source(id, None).expect("the map object opens");
    let extent = Geometry::DEFAULT.extent_size() as usize;

    assert_eq!(source.len(), bytes.len() as u64, "the source is as long as the map");
    // Windows that matter to a reader: the header, a style-table read, a 512-byte block, the file's
    // last byte — and four that **straddle the extent seam**, which is the case a contiguous fixture
    // cannot reach and the one the adapter's range walk exists for.
    let windows = [
        (0usize, 49usize),
        (49, 15),
        (64, 8),
        (bytes.len() - 1, 1),
        (extent - 1, 2),                        // one byte either side of the seam
        (extent - 300, 600),                    // a window centred on it
        (extent - 1, bytes.len() - extent + 1), // from just before the seam to the end
        (0, bytes.len()),                       // the whole object, across both ranges
    ];
    for (offset, len) in windows {
        let mut through_store = std::vec![0u8; len];
        source.read_at(offset as u64, &mut through_store).expect("inside the object");
        assert_eq!(&through_store[..], &bytes[offset..offset + len], "the store changed bytes at ({offset}, {len})");
    }

    let handle = source.release();
    store.close(handle);
}

/// **The renderer's own parse and query, over the store.** `MapTables` and a `Reader` are what the
/// board builds at boot and per frame; driving them here is what makes "a flat card renders" a
/// checked claim at this layer rather than only an on-glass one.
///
/// Both sides are asserted against a `SliceSource` over the same bytes, so a divergence names the
/// adapter rather than the format.
#[test]
fn the_reader_parses_and_queries_a_map_through_the_store_exactly_as_over_a_slice() {
    let bytes = padded_map_bytes();
    let (disk, id) = card_with_map(&bytes);
    let store = FlatStore::mount(&disk);

    let slice = SliceSource(&bytes);
    let over_slice = MapTables::parse(&slice).expect("the map parses over a slice");

    store
        .with_source(id, None, |source| {
            let over_store = MapTables::parse(source).expect("the same map parses over the store");
            assert_eq!(over_store.version, over_slice.version);
            assert_eq!(over_store.bbox, over_slice.bbox);
            assert_eq!(over_store.marker_color, over_slice.marker_color);
            assert_eq!(over_store.lods().len(), over_slice.lods().len(), "the same ladder");
            assert_eq!(over_store.terrain(), over_slice.terrain(), "and the same §1.3 answer");

            // One viewport query per rung, against the slice-backed reader's answer. This is the
            // call the render path makes; a cache keyed differently, an offset resolved against the
            // wrong scale, or a short read would all show up as a different chunk list.
            let store_cache = MapCache::new();
            let slice_cache = MapCache::new();
            let reader = Reader::new(source, &over_store, &store_cache);
            let oracle = Reader::new(&slice, &over_slice, &slice_cache);
            for lod in 0..over_slice.lods().len() {
                let mut from_store = std::vec::Vec::new();
                let mut from_slice = std::vec::Vec::new();
                reader.for_each_chunk(lod, &over_store.bbox, |cid, node| from_store.push((cid, node))).expect("walks");
                oracle.for_each_chunk(lod, &over_slice.bbox, |cid, node| from_slice.push((cid, node))).expect("walks");
                assert_eq!(from_store, from_slice, "LOD {lod} dispatched differently through the store");
                assert!(!from_store.is_empty(), "LOD {lod} carries a chunk, so the walk must find it");
            }
        })
        .expect("the map object opens");
}

/// **Terrain comes through the same handle.** A map with a spliced §1.3 region hands back a window
/// whose bytes are the container's, read through the store — which is the whole reason the board no
/// longer opens a second file for elevation, and the reason a flat card can have terrain at all
/// (there is no filesystem to hang a sidecar off).
#[test]
fn the_embedded_terrain_region_windows_onto_the_same_store_object() {
    // Padded, so the region lands **past** the extent seam: the window's own arithmetic is then
    // composed with the store's range walk, which is the shape the board actually reads terrain in.
    let plain = padded_map_bytes();
    let stub = terrain_stub(300); // not a whole number of units — the window's tail is §1.2 filler
    let bytes = splice_terrain(&plain, &stub);
    let (disk, id) = card_with_map(&bytes);
    let store = FlatStore::mount(&disk);

    store
        .with_source(id, None, |source| {
            let tables = MapTables::parse(source).expect("the spliced map parses over the store");
            let region = tables.terrain().expect("the region is named");
            let window = WindowSource::new(source, region.offset, region.len).expect("the window is inside the object");

            assert_eq!(window.len(), region.len);
            assert!(
                region.offset > Geometry::DEFAULT.extent_size(),
                "the region must sit past the extent seam, or the window never composes with a range walk"
            );
            let mut container = std::vec![0u8; stub.len()];
            window.read_at(0, &mut container).expect("the container's own bytes, from its byte 0");
            assert_eq!(container, stub, "the window re-bases onto the container rather than the file");

            // And it ends where the region ends: a container that reads past its own length gets a
            // refusal, not the bytes that happen to follow it on the card.
            let mut past = [0u8; 1];
            assert!(window.read_at(region.len, &mut past).is_err(), "the window's end is as hard as a file's");
        })
        .expect("the map object opens");
}
