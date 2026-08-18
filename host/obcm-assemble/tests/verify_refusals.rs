//! **The §4.8 mutation suite** — what the verify pass refuses, proved by breaking a real map.
//!
//! `OBCA_Spec.md` §4.8 is a *precondition of writing a set*: nothing self-made reaches a device
//! unverified. That makes the pass's strength a load-bearing property rather than a nicety, and it
//! is exactly the property a rewrite for memory can quietly destroy — a streamed check that no
//! longer holds the thing it was comparing against still returns `Ok`, and every existing test still
//! passes, because every existing fixture is *valid*.
//!
//! So this suite does the opposite of the rest of the crate's tests: it packs one small, genuinely
//! routable map with the **real packer**, asserts the pass accepts it, and then corrupts one field
//! at a time in the written bytes and asserts the pass names what broke. Every refusal class
//! #1116's C5 introduced or strengthened is here — an out-of-range node id, a hole in the numbering,
//! two records under one id (in both flavours) — alongside the §4.8 fundamentals the pass has always
//! owed: an adjacency entry that resolves nowhere, one whose deltas point somewhere else, an edge
//! that does not decode, and the two directions of an edge disagreeing.
//!
//! Each test runs at **two budgets**: the default, where the junction table is one band, and one so
//! small the table is banded and the claims spill through the scratch seam in many runs. A refusal
//! that only fires in one of the two shapes is the failure mode this file exists to catch (#1132).

use obc_elevation::NullElevation;
use obc_formats::obcm::{CHUNK_END, NAV_NEIGHBOR_LEN, NAV_NODE_FIXED_LEN};
use obc_pack::config::default_profiles;
use obc_pack::geom::Geom;
use obc_pack::nav::DEFAULT_MIN_COMPONENT_EDGES;
use obc_pack::nav::{build_graph_with, RoutableWay};
use obc_pack::progress::Progress;
use obc_pack::quadtree::build_lod_with;
use obc_pack::serialize::Style;
use obc_pack::{serialize_lods, LodLayer};
use obc_reader::{MapCache, MapTables, Reader};
use obcm_assemble::grid::AlignedBox;
use obcm_assemble::verify::verify_shard;
use obcm_assemble::{Error, MemoryScratch, MemorySource, VerifyReport, DEFAULT_MERGE_BUDGET};

// --- the fixture ------------------------------------------------------------------------------

/// The worked example's `2^19` square (`OBCA_Spec.md` §7), so the header bbox the packer writes is a
/// box the engine's own [`AlignedBox`] can state.
const BOX: AlignedBox = AlignedBox { min_lat: 47_185_920, min_lon: 7_340_032, span_log2: 19 };

/// A small nav chunk, so the fixture's nine junctions genuinely span several §8.2 chunks and the
/// walk's re-delivery (the thing the digest check exists for) actually happens.
const CHUNK_SIZE: usize = 512;

/// The grid's spacing in µdeg — wide enough that the `int16` neighbour deltas are large and the
/// junctions land in several §8.2 chunks, small enough to stay under the packer's own
/// endpoint-delta bound (32 000 µdeg) so that no edge is split into synthetic degree-2 pieces and
/// the fixture's graph is exactly the lattice it is written as.
const STEP: i32 = 20_000;

fn deg(udeg: i32) -> f64 {
    udeg as f64 / 1e6
}

/// A 3 × 3 lattice of routable ways: nine junctions, twelve edges, one component. Node ids are the
/// OSM-side ids the packer's junction detection keys on, so sharing one between a row and a column
/// way is what makes their crossing a junction.
fn ways() -> Vec<RoutableWay> {
    let at = |row: i32, col: i32| (BOX.min_lon as i32 + STEP * (col + 1), BOX.min_lat as i32 + STEP * (row + 1));
    let id = |row: i32, col: i32| (row * 10 + col) as i64;
    let mut out = Vec::new();
    for row in 0..3 {
        out.push(RoutableWay {
            node_ids: (0..3).map(|col| id(row, col)).collect(),
            coords: (0..3).map(|col| at(row, col)).collect(),
            kind: 3,
        });
    }
    for col in 0..3 {
        out.push(RoutableWay {
            node_ids: (0..3).map(|row| id(row, col)).collect(),
            coords: (0..3).map(|row| at(row, col)).collect(),
            kind: 3,
        });
    }
    out
}

/// One styled polyline per row, so the map carries geometry as well as a graph — the offset table
/// and the feature decode are §4.8 checks too, and a nav-only fixture would exercise neither.
fn features() -> Vec<(u8, Geom)> {
    (0..3)
        .map(|row| {
            let lat = BOX.min_lat as i32 + STEP * (row + 1);
            (
                1u8,
                Geom::Line(
                    (0..3)
                        .map(|col| (deg(BOX.min_lon as i32 + STEP * (col + 1)), deg(lat)))
                        .collect::<Vec<(f64, f64)>>(),
                ),
            )
        })
        .collect()
}

/// `pack(X)` at fixture scale: the real packer's graph builder, quadtree and serializer.
fn map() -> Vec<u8> {
    let (graph, _) = build_graph_with(&ways(), DEFAULT_MIN_COMPONENT_EDGES);
    let styles = vec![Style {
        id: 1,
        z_index: 1,
        color: 0xF800,
        weight: 2,
        priority: 1,
        dashed: false,
        color2: None,
        fixed_width: false,
        terrain_layer: false,
    }];
    let bbox = BOX.ubox();
    let lods = vec![LodLayer {
        max_mpp: None,
        chunk_size: CHUNK_SIZE,
        root: build_lod_with(features(), bbox, CHUNK_SIZE, &Progress::silent()),
    }];
    let (bytes, dropped) =
        serialize_lods(&lods, &styles, 0xF800, bbox, &[], &graph, &default_profiles(), &mut NullElevation);
    assert_eq!(dropped, 0, "the fixture must not lose features to the chunk cap");
    bytes
}

// --- the harness ------------------------------------------------------------------------------

/// The two budget shapes every case is run at: the shipping default (one band, one sorted run) and a
/// budget so small the junction table is banded and the claim sort genuinely merges runs.
const BUDGETS: [usize; 2] = [DEFAULT_MERGE_BUDGET, 64];

/// Verify at one budget, asserting the scratch area is empty afterwards **however it ends**. A
/// refusal that leaves its spill behind would fill a host's disk one broken map at a time.
fn verify_at(bytes: &[u8], budget: usize) -> Result<VerifyReport, Error> {
    let scratch = MemoryScratch::new();
    let out = verify_shard(&MemorySource(bytes.to_vec()), BOX, true, &scratch, budget);
    assert_eq!(scratch.resident_bytes(), 0, "the pass removed every scratch file it created ({out:?})");
    out
}

/// Mutate the packed map with `break_it`, then assert both budget shapes refuse it and say `wants`.
#[track_caller]
fn refuses(what: &str, wants: &str, break_it: impl Fn(&mut Vec<u8>)) {
    let mut bytes = map();
    break_it(&mut bytes);
    for budget in BUDGETS {
        match verify_at(&bytes, budget) {
            Ok(report) => panic!("{what}: the pass accepted a broken map at budget {budget} — {report:?}"),
            Err(Error::Verify(msg)) => {
                assert!(msg.contains(wants), "{what}: at budget {budget} the refusal was {msg:?}, wanted {wants:?}")
            }
            Err(other) => panic!("{what}: at budget {budget} the pass failed as {other:?}, not as a §4.8 refusal"),
        }
    }
}

/// Where the §8.2 node chunks begin, and how many there are.
///
/// Through the directory's own `data_start`, which is `align_up(index_offset + node_count × 4, U)`
/// since v14 — spelling it out as the bare sum here would land this suite a few bytes before the
/// first chunk, find no record at all, and turn every mutation below into a silent no-op.
fn node_chunks(bytes: &[u8]) -> (usize, usize, usize) {
    let src = MemorySource(bytes.to_vec());
    let tables = MapTables::parse(&src).expect("the fixture parses");
    let cache = MapCache::new_boxed();
    let reader = Reader::new(&src, &tables, &cache);
    let dir = *reader.nav_directory();
    (dir.data_start().expect("the fixture's nav directory resolves") as usize, dir.chunk_count, dir.chunk_size)
}

/// Absolute file offsets of every §8.3 record, in chunk order — the addresses a mutation names.
fn node_records(bytes: &[u8]) -> Vec<usize> {
    let (base, chunks, size) = node_chunks(bytes);
    let mut out = Vec::new();
    for chunk in 0..chunks {
        let start = base + chunk * size;
        let mut at = start;
        while at + NAV_NODE_FIXED_LEN <= start + size {
            let degree = bytes[at + 12];
            if degree == CHUNK_END {
                break;
            }
            out.push(at);
            at += NAV_NODE_FIXED_LEN + degree as usize * NAV_NEIGHBOR_LEN;
        }
    }
    out
}

/// The `Node Id` field of the record at `off` (§8.3 byte 8).
fn set_id(bytes: &mut [u8], off: usize, id: u32) {
    bytes[off + 8..off + 12].copy_from_slice(&id.to_le_bytes());
}

fn node_id(bytes: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(bytes[off + 8..off + 12].try_into().expect("4 bytes"))
}

/// The `k`-th adjacency entry of the record at `off`.
fn neighbor(off: usize, k: usize) -> usize {
    off + NAV_NODE_FIXED_LEN + k * NAV_NEIGHBOR_LEN
}

/// A record that is the **last** in its chunk and has room for `degree` adjacency entries.
///
/// `Degree` is a length field: bumping it on a record with a successor swallows that successor's
/// bytes, and what the pass then reports is the hole in the numbering — a true refusal, but not the
/// one under test. Growing the last record of a chunk instead eats only the `0xFF` sentinel and the
/// padding behind it, so the degree cap is the only thing broken.
fn last_record_with_room(bytes: &[u8], degree: usize) -> usize {
    let (base, chunks, size) = node_chunks(bytes);
    let records = node_records(bytes);
    for chunk in (0..chunks).rev() {
        let (start, end) = (base + chunk * size, base + (chunk + 1) * size);
        let last = records.iter().copied().rfind(|&at| at >= start && at < end);
        if let Some(at) = last.filter(|at| at + NAV_NODE_FIXED_LEN + degree * NAV_NEIGHBOR_LEN <= end) {
            return at;
        }
    }
    panic!("the fixture has no chunk whose last record can grow to degree {degree}");
}

// --- the map the mutations start from -----------------------------------------------------------

#[test]
fn the_fixture_is_a_map_the_pass_accepts() {
    for budget in BUDGETS {
        let report = verify_at(&map(), budget).expect("a packer-written map verifies");
        assert_eq!(report.nav_nodes, 9, "the 3 × 3 lattice's junctions");
        assert_eq!(report.nav_edges, 12, "and its edges");
        assert_eq!(report.components, 1);
        assert_eq!(report.largest_component_permille, 1000);
        assert!(report.features > 0 && report.chunks > 0, "the geometry half ran too");
    }
    // The whole point of the second budget: 64 bytes cannot hold nine junctions' coordinates and
    // digests at once, so the low-budget run above genuinely banded the table and re-walked.
    assert!(node_records(&map()).len() > 64 / 16, "the fixture must not fit the small budget's band");
}

/// Every number the pass reports is the same at every budget — banding changes what is resident,
/// never what is true.
#[test]
fn the_report_does_not_depend_on_the_budget() {
    let bytes = map();
    let want = verify_at(&bytes, DEFAULT_MERGE_BUDGET).expect("verified");
    for budget in [16, 64, 512, 4096, 1 << 20, DEFAULT_MERGE_BUDGET] {
        assert_eq!(verify_at(&bytes, budget).expect("verified"), want, "budget {budget} reported differently");
    }
}

// --- the three refusals #1116 C5 introduced ------------------------------------------------------

/// An id no dense numbering of this section could reach. Before C5 the pass hashed it and carried
/// on; it must never be allowed to name an allocation, which is why it is refused ahead of every
/// other check on the record.
#[test]
fn an_id_past_the_sections_capacity_is_refused() {
    refuses("an out-of-range node id", "out of the section's range", |bytes| {
        let at = node_records(bytes)[0];
        set_id(bytes, at, 9_000_000);
    });
}

/// A hole in the numbering means a junction the walk never saw — some neighbour entry points at it.
/// Moving a record's id above the highest one leaves exactly that.
#[test]
fn a_hole_in_the_numbering_is_refused() {
    refuses("a hole in the numbering", "not dense", |bytes| {
        let records = node_records(bytes);
        let highest = records.iter().map(|&at| node_id(bytes, at)).max().expect("records");
        let victim = records.iter().copied().find(|&at| node_id(bytes, at) == 0).expect("id 0");
        set_id(bytes, victim, highest + 1);
    });
}

/// Two junctions wearing one id. The coordinates get their own message because they are what the
/// rest of the pass indexes by.
#[test]
fn two_records_under_one_id_are_refused() {
    refuses("one id, two coordinates", "two §8.3 records with different coordinates", |bytes| {
        let records = node_records(bytes);
        let (first, second) = (records[0], records[1]);
        let id = node_id(bytes, first);
        set_id(bytes, second, id);
    });
}

/// …and the digest's own case: same id, same coordinates, a *different* adjacency list. This is the
/// one the pass has to catch for the delivery-dedup to be sound — walk 2 processes a re-delivered
/// record once, which is only legal because a repeat that differs is refused here.
#[test]
fn one_id_with_two_adjacency_lists_is_refused() {
    refuses("one id, two adjacency lists", "different adjacency", |bytes| {
        let records = node_records(bytes);
        // Copy the first record's identity (lat, lon, id) onto the second, leaving the second's own
        // degree and adjacency entries in place.
        let (first, second) = (records[0], records[1]);
        assert_ne!(bytes[first + 12], 0, "the fixture's junctions have neighbours");
        let identity: [u8; 12] = bytes[first..first + 12].try_into().expect("12 bytes");
        bytes[second..second + 12].copy_from_slice(&identity);
    });
}

// --- the §4.8 fundamentals -----------------------------------------------------------------------

/// §4.8.4: every neighbour resolves. An adjacency entry pointing past the graph is what a
/// mis-relocated index or a truncated section leaves behind.
#[test]
fn an_adjacency_entry_that_resolves_nowhere_is_refused() {
    refuses("a neighbour id past the graph", "resolves to no record", |bytes| {
        let at = neighbor(node_records(bytes)[0], 0);
        bytes[at..at + 4].copy_from_slice(&4242u32.to_le_bytes());
    });
}

/// §8.3 stores a neighbour's coordinate as an `int16` delta off the record's own. The reconstruction
/// must land on what that neighbour's record states — this is the check that catches a record moved
/// without its adjacency being rewritten.
#[test]
fn an_adjacency_delta_that_points_elsewhere_is_refused() {
    refuses("a neighbour delta off by 3 µdeg", "int16 delta reconstructs neighbour", |bytes| {
        let at = neighbor(node_records(bytes)[0], 0);
        let delta = i16::from_le_bytes([bytes[at + 4], bytes[at + 5]]);
        bytes[at + 4..at + 6].copy_from_slice(&delta.wrapping_add(3).to_le_bytes());
    });
}

/// §4.8.4: every `Edge Id` decodes. An id past the edge pool is the graft's characteristic failure —
/// a wrong chunk base — seen from the adjacency side.
#[test]
fn an_edge_id_that_does_not_decode_is_refused() {
    refuses("an edge id past the pool", "does not decode", |bytes| {
        let at = neighbor(node_records(bytes)[0], 0);
        bytes[at + 8..at + 12].copy_from_slice(&0x00ff_0000u32.to_le_bytes());
    });
}

/// §8.3: the two directions of an edge must agree on `Cost M` and `Way Kind`. Only one side is
/// touched, so the disagreement is between the two adjacency entries and not with the edge record.
#[test]
fn two_directions_disagreeing_about_cost_is_refused() {
    refuses("one direction's cost bumped", "two different (cost, kind) pairs", |bytes| {
        let at = neighbor(node_records(bytes)[0], 0);
        let cost = u16::from_le_bytes([bytes[at + 12], bytes[at + 13]]);
        bytes[at + 12..at + 14].copy_from_slice(&cost.wrapping_add(7).to_le_bytes());
    });
}

/// …and the same field on *both* sides, which agrees with itself and disagrees with the §8.4 record
/// the edge id names. Written as a whole-section sweep because both entries of one edge have to move
/// together, and they live in different records.
#[test]
fn adjacency_that_disagrees_with_the_edge_record_is_refused() {
    refuses("both directions' cost bumped", "but its adjacency entries say", |bytes| {
        let records = node_records(bytes);
        let target = {
            let at = neighbor(records[0], 0);
            u32::from_le_bytes(bytes[at + 8..at + 12].try_into().expect("4 bytes"))
        };
        for &record in &records {
            for k in 0..bytes[record + 12] as usize {
                let at = neighbor(record, k);
                let edge = u32::from_le_bytes(bytes[at + 8..at + 12].try_into().expect("4 bytes"));
                if edge == target {
                    let cost = u16::from_le_bytes([bytes[at + 12], bytes[at + 13]]);
                    bytes[at + 12..at + 14].copy_from_slice(&cost.wrapping_add(11).to_le_bytes());
                }
            }
        }
    });
}

/// §8.3's degree cap (24). A record claiming more neighbours than the format allows is refused
/// before its adjacency is read as anything.
#[test]
fn a_degree_past_the_cap_is_refused() {
    refuses("a degree of 25", "exceed the §8.3 degree cap", |bytes| {
        let at = last_record_with_room(bytes, 25);
        bytes[at + 12] = 25;
    });
}

/// §4.8.2/§5.1: the offset table's own invariants, re-derived from the bytes rather than trusted.
/// `offsets[0] != 0` is the cheapest expression of a mis-relocated chunk base — the table still
/// parses, still lies inside the file, and still says every chunk is somewhere else than it is.
#[test]
fn an_offset_table_that_does_not_start_at_zero_is_refused() {
    refuses("offsets[0] moved off zero", "offsets[0] is 4, not 0", |bytes| {
        let src = MemorySource(bytes.to_vec());
        let tables = MapTables::parse(&src).expect("the fixture parses");
        let cache = MapCache::new_boxed();
        let reader = Reader::new(&src, &tables, &cache);
        let lod = reader.lods()[0];
        assert!(lod.chunk_count >= 1, "the fixture's LOD has chunks");
        let table = (lod.index_offset + (lod.node_count * 4) as u64) as usize;
        bytes[table..table + 4].copy_from_slice(&4u32.to_le_bytes());
    });
}

/// §4.8.1: the shard's header bbox is its planned box. The engine writes the box it planned, so a
/// header that says otherwise is a shard that was placed wrong.
#[test]
fn a_header_bbox_that_is_not_the_planned_box_is_refused() {
    let bytes = map();
    let wrong = AlignedBox { min_lat: BOX.min_lat, min_lon: BOX.min_lon + (1 << BOX.span_log2), ..BOX };
    let scratch = MemoryScratch::new();
    let err = verify_shard(&MemorySource(bytes), wrong, true, &scratch, DEFAULT_MERGE_BUDGET)
        .expect_err("the header states the box it was packed over");
    assert!(format!("{err:?}").contains("is not its planned box"), "{err:?}");
}

/// §1.1: the offset unit travels **in the file**, at byte 40, so a verifier that resolves a shard's
/// offsets against its own compiled-in `SCALE` agrees with itself no matter what the header says.
///
/// Flipping the scale byte alone re-points every scaled offset in the file — at scale 5 each one
/// names twice the byte it did — so a pass that reads the header's unit finds the LOD regions
/// somewhere else and refuses. One that ignores it sails straight through, which is what this pins.
///
/// The refusal is deliberately *not* a version error: §1.1 requires a scale a reader cannot resolve
/// to be distinct from an old file, and a scale it *can* resolve but which does not describe these
/// bytes is a corrupt map either way.
#[test]
fn a_header_scale_that_is_not_the_writers_is_refused() {
    const HEADER_OFFSET_SCALE_OFF: usize = 40;
    let bytes = map();
    assert_eq!(
        bytes[HEADER_OFFSET_SCALE_OFF], 4,
        "the fixture is written at the default scale, so flipping the byte is a real change"
    );
    for scale in [3u8, 5, 9] {
        let mut broken = bytes.clone();
        broken[HEADER_OFFSET_SCALE_OFF] = scale;
        let scratch = MemoryScratch::new();
        let err = verify_shard(&MemorySource(broken), BOX, true, &scratch, DEFAULT_MERGE_BUDGET)
            .expect_err("scale {scale} does not describe these bytes");
        assert!(
            !format!("{err:?}").contains("Version"),
            "scale {scale}: a resolvable-but-wrong unit is not a version problem — {err:?}"
        );
    }
    // …and an out-of-range scale is refused by the parse itself (§1.1 caps it at 9).
    let mut past = bytes.clone();
    past[HEADER_OFFSET_SCALE_OFF] = 10;
    let scratch = MemoryScratch::new();
    verify_shard(&MemorySource(past), BOX, true, &scratch, DEFAULT_MERGE_BUDGET)
        .expect_err("scale 10 is not a legal unit");
}
