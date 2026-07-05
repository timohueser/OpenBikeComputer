//! Format-contract tests for the OBCM v7 reader.
//!
//! Each test builds a synthetic `.obcm` with the shared `obcm-testkit` builder (which mirrors the
//! Rust packer's `serialize.rs`), then asserts the reader parses it back. Building the bytes rather
//! than checking in a binary fixture keeps the encoder + reader pinned to one layout: if either
//! drifts, these break. `obcm-testkit` shares the layout with `obc-render`'s priority test.

use obc_reader::{
    BBox, Error, Kind, MapCache, MapTables, Reader, SliceSource, HEADER_LEN, MAX_FEAT_PTS, MAX_FEAT_RINGS,
};
use obcm_testkit::{
    build_file, empty_poi_directory, hours_pool, pack_line, pack_line16, pack_poi_chunk, pack_poi_record,
    pack_poly_hole, pad, poi_dir_len, poi_directory, LodSpec, PoiCat, Style, BRANCH_BIT, EMPTY_LEAF, MARKER,
    POI_HOURS_BLOB_LEN, POI_RECORD_LEN,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Feature {
    pub style_id: u8,
    pub kind: Kind,
    pub exterior: Vec<(i32, i32)>,
    pub interiors: Vec<Vec<(i32, i32)>>,
}

fn decode_chunk(r: &Reader, lod: usize, chunk_id: u32, node: &BBox) -> Vec<Feature> {
    let mut out = Vec::new();
    let mut points = heapless::Vec::<_, MAX_FEAT_PTS>::new();
    let mut ring_lens = heapless::Vec::<_, MAX_FEAT_RINGS>::new();
    r.for_each_feature(lod, chunk_id, node, &mut points, &mut ring_lens, |f| {
        out.push(Feature {
            style_id: f.style_id,
            kind: f.kind,
            exterior: f.exterior().to_vec(),
            interiors: f.interiors().map(|h| h.to_vec()).collect(),
        });
    });
    out
}

/// Like [`decode_chunk`] but only the features for which `keep(style_id)` is
/// true are decoded and returned; the rest are skipped in the reader.
fn decode_filtered(r: &Reader, lod: usize, chunk_id: u32, node: &BBox, keep: impl Fn(u8) -> bool) -> Vec<Feature> {
    let mut out = Vec::new();
    let mut points = heapless::Vec::<_, MAX_FEAT_PTS>::new();
    let mut ring_lens = heapless::Vec::<_, MAX_FEAT_RINGS>::new();
    r.for_each_feature_filtered(lod, chunk_id, node, &mut points, &mut ring_lens, keep, |f| {
        out.push(Feature {
            style_id: f.style_id,
            kind: f.kind,
            exterior: f.exterior().to_vec(),
            interiors: f.interiors().map(|h| h.to_vec()).collect(),
        });
    });
    out
}

/// Collect every leaf `for_each_chunk` yields — the uncapped replacement for the
/// removed `Reader::query` test convenience.
fn query_all(r: &Reader, lod: usize, view: &BBox) -> Vec<(u32, BBox)> {
    let mut out = Vec::new();
    r.for_each_chunk(lod, view, |cid, node| out.push((cid, node)));
    out
}

// A two-LOD file used by several tests: LOD0 (coarse, +inf) holds one line,
// LOD1 (max_mpp 50) holds one polygon-with-hole. Both are single-leaf trees over
// the global bbox (0,0,1000,1000), so the leaf's node bbox is the global bbox
// and feature anchors are absolute.
const CS: usize = 64;
const GLOBAL: (i32, i32, i32, i32) = (0, 0, 1000, 1000);
const STYLES: &[Style] = &[(1, 3, 0xF800, 2, 3), (2, -1, 0x07E0, 1, 3)];

fn two_lod_file() -> Vec<u8> {
    let line = pad(pack_line(1, 100, 200, &[(10, 0), (0, 10)]), CS);
    let poly =
        pad(pack_poly_hole(2, 100, 100, &[(100, 0), (0, 100), (-100, 0)], &[(25, 25), (50, 0), (0, 50), (-50, 0)]), CS);
    build_file(
        GLOBAL,
        STYLES,
        &[
            LodSpec { max_mpp: f32::INFINITY, index: vec![0], chunks: vec![line], chunk_size: CS },
            LodSpec { max_mpp: 50.0, index: vec![0], chunks: vec![poly], chunk_size: CS },
        ],
    )
}

#[test]
fn header_and_lod_table() {
    let bytes = two_lod_file();
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);

    assert_eq!(r.version, 7);
    assert_eq!(r.marker_color, MARKER);
    assert_eq!(r.bbox.min_lon, 0);
    assert_eq!(r.bbox.min_lat, 0);
    assert_eq!(r.bbox.max_lon, 1000);
    assert_eq!(r.bbox.max_lat, 1000);

    let lods = r.lods();
    assert_eq!(lods.len(), 2);
    assert!(lods[0].max_mpp.is_infinite());
    assert_eq!(lods[1].max_mpp, 50.0);
    assert_eq!(lods[0].node_count, 1);
    assert_eq!(lods[0].chunk_size, CS);
    assert_eq!(lods[0].chunk_count, 1);
    assert_eq!(lods[1].chunk_count, 1);
}

#[test]
fn marker_color_round_trips() {
    // The header's marker color parses back unchanged at its fixed offset.
    let bytes = two_lod_file();
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);
    assert_eq!(r.marker_color, MARKER);
}

#[test]
fn styles_parse() {
    let bytes = two_lod_file();
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);

    let s1 = r.style(1).expect("style 1");
    assert_eq!(s1.z_index, 3);
    assert_eq!(s1.color, 0xF800);
    assert_eq!(s1.weight, 2);
    assert_eq!(s1.priority, 3);

    let s2 = r.style(2).expect("style 2");
    assert_eq!(s2.z_index, -1);
    assert_eq!(s2.color, 0x07E0);

    assert!(r.style(200).is_none());
}

#[test]
fn backdrop_is_lowest_z_regardless_of_id() {
    // STYLES = [(id 1, z 3), (id 2, z -1)]. The backdrop is the bottom of the
    // paint order (lowest z), i.e. id 2 — not the lowest id. This guards the
    // sea/background lookup against style-ID reassignment.
    let bytes = two_lod_file();
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);

    let bg = r.backdrop_style().expect("a backdrop");
    assert_eq!(bg.id, 2);
    assert_eq!(bg.z_index, -1);
    assert_eq!(bg.color, 0x07E0);
}

#[test]
fn select_lod_for_mpp_picks_finest_covering() {
    let bytes = two_lod_file(); // max_mpp = [+inf, 50]
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);

    assert_eq!(r.select_lod_for_mpp(1000.0), 0); // only +inf covers
    assert_eq!(r.select_lod_for_mpp(51.0), 0); // 50 doesn't cover
    assert_eq!(r.select_lod_for_mpp(50.0), 1); // boundary: 50 >= 50 covers
    assert_eq!(r.select_lod_for_mpp(49.0), 1);
    assert_eq!(r.select_lod_for_mpp(0.0), 1); // finest
}

#[test]
fn query_single_leaf() {
    let bytes = two_lod_file();
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);

    // A view overlapping the global bbox hits the single leaf (chunk 0).
    let hits = query_all(&r, 0, &obc_reader::BBox { min_lon: 100, min_lat: 100, max_lon: 200, max_lat: 200 });
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0, 0);
    assert_eq!(hits[0].1, r.bbox); // leaf node bbox == global bbox

    // A view entirely outside the global bbox hits nothing.
    let miss = query_all(&r, 0, &obc_reader::BBox { min_lon: 5000, min_lat: 5000, max_lon: 6000, max_lat: 6000 });
    assert!(miss.is_empty());
}

#[test]
fn decode_line() {
    let bytes = two_lod_file();
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);
    let node = r.bbox;

    let feats = decode_chunk(&r, 0, 0, &node);
    assert_eq!(feats.len(), 1);
    let f = &feats[0];
    assert_eq!(f.style_id, 1);
    assert_eq!(f.kind, Kind::Line);
    assert_eq!(f.exterior, vec![(100, 200), (110, 200), (110, 210)]);
    assert!(f.interiors.is_empty());
}

#[test]
fn decode_polygon_with_hole() {
    let bytes = two_lod_file();
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);
    let node = r.bbox;

    let feats = decode_chunk(&r, 1, 0, &node);
    assert_eq!(feats.len(), 1);
    let f = &feats[0];
    assert_eq!(f.style_id, 2);
    assert_eq!(f.kind, Kind::Polygon);
    assert_eq!(f.exterior, vec![(100, 100), (200, 100), (200, 200), (100, 200)]);
    assert_eq!(f.interiors.len(), 1);
    assert_eq!(f.interiors[0], vec![(125, 125), (175, 125), (175, 175), (125, 175)]);
}

#[test]
fn visitor_matches_owned_decode() {
    let bytes = two_lod_file();
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);
    let node = r.bbox;

    // The borrowing for_each_feature must yield exactly what decode_chunk does.
    let owned = decode_chunk(&r, 1, 0, &node);

    let mut points = heapless::Vec::<_, MAX_FEAT_PTS>::new();
    let mut ring_lens = heapless::Vec::<_, MAX_FEAT_RINGS>::new();
    let mut seen = 0;
    r.for_each_feature(1, 0, &node, &mut points, &mut ring_lens, |f| {
        let o = &owned[seen];
        assert_eq!(f.style_id, o.style_id);
        assert_eq!(f.kind, o.kind);
        assert_eq!(f.exterior(), o.exterior.as_slice());
        let holes: Vec<&[(i32, i32)]> = f.interiors().collect();
        assert_eq!(holes.len(), o.interiors.len());
        for (h, oh) in holes.iter().zip(&o.interiors) {
            assert_eq!(*h, oh.as_slice());
        }
        seen += 1;
    });
    assert_eq!(seen, owned.len());
}

#[test]
fn decode_16bit_deltas() {
    // Deltas beyond the int8 range force the 16-bit path (flag bit 0).
    let bbox = (0, 0, 1_000_000, 1_000_000);
    let line = pad(pack_line16(1, 0, 0, &[(300, 400), (-200, 0)]), CS);
    let bytes = build_file(
        bbox,
        STYLES,
        &[LodSpec { max_mpp: f32::INFINITY, index: vec![0], chunks: vec![line], chunk_size: CS }],
    );
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);
    let feats = decode_chunk(&r, 0, 0, &r.bbox);
    assert_eq!(feats.len(), 1);
    assert_eq!(feats[0].exterior, vec![(0, 0), (300, 400), (100, 400)]);
}

#[test]
fn quadtree_subdivision_and_node_bbox() {
    // Root branch → 4 children (NW, NE, SW, SE); only NW is a non-empty leaf.
    // Exercises query_rec subdivision and the NW node-bbox math.
    // Global bbox (0,0,1000,1000); midpoints (500,500); NW = (0,500,500,1000).
    // Anchor is relative to the NW node's min corner (0,500): ax=10, ay=10.
    let line = pad(pack_line(1, 10, 10, &[(5, 5)]), CS);
    let index = vec![
        BRANCH_BIT | 1, // root: branch, children start at idx 1
        0,              // NW: leaf -> chunk 0
        EMPTY_LEAF,     // NE
        EMPTY_LEAF,     // SW
        EMPTY_LEAF,     // SE
    ];
    let bytes =
        build_file(GLOBAL, STYLES, &[LodSpec { max_mpp: f32::INFINITY, index, chunks: vec![line], chunk_size: CS }]);
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);

    let nw = obc_reader::BBox { min_lon: 0, min_lat: 500, max_lon: 500, max_lat: 1000 };

    // View inside the NW quadrant hits the leaf, with the NW node bbox.
    let hits = query_all(&r, 0, &obc_reader::BBox { min_lon: 50, min_lat: 600, max_lon: 150, max_lat: 700 });
    assert_eq!(hits.as_slice(), &[(0, nw)]);

    // View inside the (empty) SE quadrant hits nothing.
    let se = query_all(&r, 0, &obc_reader::BBox { min_lon: 600, min_lat: 100, max_lon: 700, max_lat: 200 });
    assert!(se.is_empty());

    // The feature's anchor is computed from the NW node's min corner (0,500):
    // ax=10, ay=10 → absolute (10, 510), then +(5,5).
    let feats = decode_chunk(&r, 0, 0, &nw);
    assert_eq!(feats[0].exterior, vec![(10, 510), (15, 515)]);
}

#[test]
fn empty_leaf_yields_nothing() {
    let empty = pad(vec![], CS); // all 0xFF
    let bytes = build_file(
        GLOBAL,
        STYLES,
        &[LodSpec { max_mpp: f32::INFINITY, index: vec![EMPTY_LEAF], chunks: vec![empty], chunk_size: CS }],
    );
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);
    assert!(query_all(&r, 0, &r.bbox).is_empty());
}

#[test]
fn rejects_bad_input() {
    // MapTables isn't Debug, so match the Err arm rather than using unwrap_err. The validation
    // (magic / version / length) moved to `MapTables::parse`; `Reader::new` is now infallible.
    let err = |b: &[u8]| match MapTables::parse(&SliceSource(b)) {
        Ok(_) => panic!("expected Err"),
        Err(e) => e,
    };

    assert_eq!(err(&[0u8; 10]), Error::TooShort);

    let mut bytes = two_lod_file();
    bytes[0] = b'X';
    assert_eq!(err(&bytes), Error::BadMagic);

    let mut bytes = two_lod_file();
    bytes[4] = 6; // v6 (and earlier) no longer supported — only v7 is read
    assert_eq!(err(&bytes), Error::BadVersion);
}

#[test]
fn out_of_range_chunk_id_decodes_nothing() {
    // `chunk_id` from a quadtree leaf is never constrained to `chunk_count`. LOD0 holds one chunk,
    // so id 1 points one past it (into LOD1's bytes); the reader must decode nothing rather than the
    // adjacent layer, or wrap+panic on the 32-bit device. `u32::MAX` is the overflow edge.
    let bytes = two_lod_file();
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);
    let node = r.bbox;
    assert!(decode_chunk(&r, 0, 1, &node).is_empty());
    assert!(decode_chunk(&r, 0, u32::MAX, &node).is_empty());
    // Filtered path shares the same guard.
    assert!(decode_filtered(&r, 0, 1, &node, |_| true).is_empty());
    // The in-range chunk still decodes, so the guard isn't over-broad.
    assert_eq!(decode_chunk(&r, 0, 0, &node).len(), 1);
}

#[test]
fn rejects_overflowing_lod_table_offset() {
    // A `lod_table_offset` near the top of the u32 range wraps `usize` once the
    // table length is added on the 32-bit device, passing the file-length guard
    // and then indexing far out of the file. Checked arithmetic rejects it up
    // front; on the 64-bit host the same value simply exceeds data.len().
    let mut bytes = two_lod_file();
    bytes[26..30].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes()); // header lod_table_offset
    assert!(matches!(MapTables::parse(&SliceSource(&bytes)), Err(Error::BadOffset)));
}

#[test]
fn rejects_overflowing_chunk_region() {
    // A corrupt LOD entry advertising a huge chunk_count × chunk_size must be
    // rejected, not trusted: on the 32-bit device the product wraps `usize` and
    // the computed chunks-end can land below data.len(), admitting a layer that
    // indexes out of the file. Checked arithmetic turns it into BadOffset.
    let mut bytes = two_lod_file();
    let lod_tab_off = u32::from_le_bytes(bytes[26..30].try_into().unwrap()) as usize;
    // Entry layout: max_mpp(4) index_off(4) node_count(4) chunk_size(2) chunk_count(4).
    let chunk_size_at = lod_tab_off + 12;
    let chunk_count_at = lod_tab_off + 14;
    bytes[chunk_size_at..chunk_size_at + 2].copy_from_slice(&u16::MAX.to_le_bytes());
    bytes[chunk_count_at..chunk_count_at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(MapTables::parse(&SliceSource(&bytes)), Err(Error::BadOffset)));
}

#[test]
fn filtered_decode_skips_without_drifting() {
    // Three heterogeneous features packed back-to-back in one chunk: an 8-bit
    // line, a polygon-with-hole, and a 16-bit line. Skipping any of them must
    // leave the reader's byte offset exactly where a full decode would, so the
    // features *after* a skipped one decode byte-identically. This pins
    // `skip_ring` to `read_ring`: if they ever drift, a trailing feature would
    // decode garbage and these assertions break.
    let mut chunk = Vec::new();
    chunk.extend_from_slice(&pack_line(1, 100, 200, &[(10, 0), (0, 10)]));
    chunk.extend_from_slice(&pack_poly_hole(
        2,
        300,
        300,
        &[(50, 0), (0, 50), (-50, 0)],
        &[(10, 10), (20, 0), (0, 20), (-20, 0)],
    ));
    chunk.extend_from_slice(&pack_line16(3, 0, 0, &[(300, 400), (-200, 0)]));
    let chunk = pad(chunk, 128);

    let styles: &[Style] = &[(1, 3, 0xF800, 2, 3), (2, -1, 0x07E0, 1, 3), (3, 0, 0x001F, 1, 3)];
    let bytes = build_file(
        GLOBAL,
        styles,
        &[LodSpec { max_mpp: f32::INFINITY, index: vec![0], chunks: vec![chunk], chunk_size: 128 }],
    );
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);
    let node = r.bbox;

    let all = decode_chunk(&r, 0, 0, &node);
    assert_eq!(all.len(), 3);

    // Keeping everything is identical to the unfiltered decode path.
    assert_eq!(decode_filtered(&r, 0, 0, &node, |_| true), all);
    // Keeping nothing visits nothing.
    assert!(decode_filtered(&r, 0, 0, &node, |_| false).is_empty());

    // Skip the middle polygon: the trailing 16-bit line must still be exact.
    assert_eq!(decode_filtered(&r, 0, 0, &node, |sid| sid != 2), vec![all[0].clone(), all[2].clone()]);
    // Skip the leading line: both following features must be exact.
    assert_eq!(decode_filtered(&r, 0, 0, &node, |sid| sid != 1), vec![all[1].clone(), all[2].clone()]);
    // Skip the trailing line: the leading two are unaffected.
    assert_eq!(decode_filtered(&r, 0, 0, &node, |sid| sid != 3), vec![all[0].clone(), all[1].clone()]);
}

#[test]
fn for_each_chunk_has_no_cap() {
    // Root branch with four non-empty leaf quadrants. `for_each_chunk` streams
    // every overlapping leaf through its callback with no upper bound — the exact
    // behaviour the renderer depends on so a wide viewport never silently loses
    // whole chunks.
    let mk = || pad(pack_line(1, 1, 1, &[(1, 1)]), CS);
    let index = vec![
        BRANCH_BIT | 1, // root branch, children start at idx 1
        0,              // NW -> chunk 0
        1,              // NE -> chunk 1
        2,              // SW -> chunk 2
        3,              // SE -> chunk 3
    ];
    let bytes = build_file(
        GLOBAL,
        STYLES,
        &[LodSpec { max_mpp: f32::INFINITY, index, chunks: vec![mk(), mk(), mk(), mk()], chunk_size: CS }],
    );
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);

    // A view over the whole bbox overlaps all four leaves.
    let mut seen = 0;
    r.for_each_chunk(0, &r.bbox, |_cid, _node| seen += 1);
    assert_eq!(seen, 4);
}

/// Build a forward-only quadtree index that is a single NW-chain `levels` branches deep, ending
/// in a non-empty leaf (chunk 0). Each branch's four children are contiguous (NW, NE, SW, SE):
/// NE/SW/SE are empty leaves and NW continues the chain, so every child index is strictly greater
/// than its parent's — the `child > idx` invariant of a well-formed map holds, isolating the
/// **depth** cap as the only thing that can stop the descent.
fn nw_chain_index(levels: usize) -> Vec<u32> {
    let mut index: Vec<u32> = vec![0]; // slot 0 is the root branch, filled below
    let mut cur = 0usize;
    for _ in 0..levels {
        let base = index.len(); // the four children are appended here, after `cur`
        index[cur] = BRANCH_BIT | base as u32;
        index.push(0); // NW: next chain node (overwritten next iteration, or the final leaf)
        index.push(EMPTY_LEAF); // NE
        index.push(EMPTY_LEAF); // SW
        index.push(EMPTY_LEAF); // SE
        cur = base;
    }
    index[cur] = 0; // deepest NW is a non-empty leaf -> chunk 0
    index
}

#[test]
fn walk_terminates_on_back_referencing_branch() {
    // A corrupt map whose root branch points its first child back at itself (`child == idx`).
    // The node bbox would shrink toward the NW corner and then stay put, so `intersects(view)`
    // never goes false — with no guard the walk recurses forever and stack-overflows (a HardFault
    // on the MCU, which has no MMU guard page). The `child > idx` guard rejects the back-edge, so
    // the walk must simply return, reporting no chunks.
    let chunk = pad(pack_line(1, 0, 0, &[(1, 1)]), CS);
    let bytes = build_file(
        GLOBAL,
        STYLES,
        &[LodSpec {
            max_mpp: f32::INFINITY,
            index: vec![BRANCH_BIT], // root branch, child base 0 == its own index
            chunks: vec![chunk],
            chunk_size: CS,
        }],
    );
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);

    // A viewport over the whole bbox keeps intersecting the (degenerate) node every level — the
    // condition under which the unguarded walk would never terminate.
    let mut seen = 0;
    r.for_each_chunk(0, &r.bbox, |_cid, _node| seen += 1);
    assert_eq!(seen, 0);
    // Collecting over the same walk must likewise return empty rather than overflow.
    assert!(query_all(&r, 0, &r.bbox).is_empty());
}

#[test]
fn walk_caps_depth_on_forward_chain() {
    // A forward-only NW-chain (every `child > idx`, so the back-reference guard never fires) far
    // deeper than the depth cap (~32). The node bbox degenerates to the NW corner after ~10
    // levels but keeps intersecting a whole-bbox viewport forever, so without the depth cap the
    // walk would descend all `LEVELS` levels and report the leaf's chunk. With the cap it stops
    // first, pruning the over-cap leaf — so no chunk is reported. This pins the depth cap
    // independently of the `child > idx` guard.
    const LEVELS: usize = 50; // comfortably past the ~32 cap
    let chunk = pad(pack_line(1, 0, 0, &[(1, 1)]), CS);
    let bytes = build_file(
        GLOBAL,
        STYLES,
        &[LodSpec { max_mpp: f32::INFINITY, index: nw_chain_index(LEVELS), chunks: vec![chunk], chunk_size: CS }],
    );
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);

    let mut seen = 0;
    r.for_each_chunk(0, &r.bbox, |_cid, _node| seen += 1);
    assert_eq!(seen, 0, "the depth cap must prune the over-cap leaf before it is reached");
}

// === POI section (v7, spec §7) — byte-pinned contract =======================
//
// These pin the v7 layout explicitly: the 36-byte header + its POI-section offset
// field, the always-present POI directory (empty + populated) with its two appended
// hours-pool fields, a POI record's exact 36 bytes (24-byte name + hours_ref), the
// 0xFF sentinel + padding, the tail hours-pool section (shared + distinct blobs, and
// an empty pool), and the v6-rejected guard. Where `build_file` writes an empty
// directory, the populated-directory test hand-assembles the section so the record +
// index + pool bytes are pinned, not derived.

/// `build_file` (empty-POI) is a valid v7 map: 36-byte header, a POI-section offset
/// pointing just past the LOD payload, a six-category empty directory, and an empty
/// hours pool.
#[test]
fn header_is_36_bytes_with_poi_offset() {
    let bytes = two_lod_file();

    // Version byte is 7; the style table follows the header, so the style offset equals the 36-byte
    // header length. (The header didn't grow v6 → v7 — only the version byte + POI section changed.)
    assert_eq!(HEADER_LEN, 36);
    assert_eq!(bytes[4], 7);
    assert_eq!(u32::from_le_bytes(bytes[21..25].try_into().unwrap()) as usize, HEADER_LEN);

    // The POI section offset lives at header byte 32 (right after the 2-byte marker at 30) and is
    // never 0 — the section is always present.
    let poi_off = u32::from_le_bytes(bytes[32..36].try_into().unwrap()) as usize;
    assert!(poi_off >= HEADER_LEN && poi_off < bytes.len(), "POI offset {poi_off} points into the file");

    // The directory there declares six categories and the shared 512-byte chunk size.
    assert_eq!(bytes[poi_off], 6, "category_count");
    assert_eq!(u16::from_le_bytes(bytes[poi_off + 1..poi_off + 3].try_into().unwrap()), 512, "shared chunk_size");
}

/// The parsed empty directory: six categories, ids 1..=6, all empty, and an empty
/// hours pool (`hours_pool_count == 0`). This is what a map with no POIs carries —
/// always present, never a zero offset.
#[test]
fn empty_poi_directory_parses_six_empty_categories() {
    let bytes = two_lod_file();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);

    let dir = r.poi_directory();
    assert_eq!(dir.chunk_size, 512);
    assert_eq!(dir.entries.len(), 6);
    for (k, e) in dir.entries.iter().enumerate() {
        assert_eq!(e.category_id, (k + 1) as u8, "category ids are 1..=6 in order");
        assert!(e.is_empty(), "category {} is empty in a no-POI map", e.category_id);
        assert_eq!(e.node_count, 0);
        assert_eq!(e.chunk_count, 0);
    }
    // The v7 hours-pool fields: an empty pool (count 0), its 2-byte `count` header lying in-file.
    assert_eq!(dir.hours_pool_count, 0, "no hours in a no-POI map");
    assert!(dir.hours_pool_offset >= HEADER_LEN && dir.hours_pool_offset + 2 <= bytes.len(), "pool offset in-file");
    assert_eq!(u16::from_le_bytes(bytes[dir.hours_pool_offset..dir.hours_pool_offset + 2].try_into().unwrap()), 0);
}

/// Hand-assemble a v7 file whose POI section carries **one populated category** (id
/// 3, Accommodation) with a two-record chunk plus a **two-blob hours pool**: the two
/// records reference blob 0 and blob 1 respectively. Pins the 36-byte record layout
/// (each field at its offset, incl. the `hours_ref` at [34..36]), the 24-byte name,
/// the 0xFF sentinel + padding, the single-leaf index, the parsed directory counts +
/// offsets, and the pool bytes (offset/count + each 29-byte blob at its index).
#[test]
fn populated_poi_category_round_trips_with_record_layout() {
    // Reuse build_file for everything up to (but not including) the POI section, then
    // replace its trailing empty directory with a populated one. build_file's POI
    // section starts at the offset stored in the header (byte 32).
    let base = build_file(
        GLOBAL,
        STYLES,
        &[LodSpec {
            max_mpp: f32::INFINITY,
            index: vec![0],
            chunks: vec![pad(pack_line(1, 0, 0, &[(1, 1)]), CS)],
            chunk_size: CS,
        }],
    );
    let poi_off = u32::from_le_bytes(base[32..36].try_into().unwrap()) as usize;

    // Two accommodation records (subtype 7 = hotel, subtype 11 = wilderness hut). One
    // named (a full 24-byte name — pins the widened field), one unnamed. Record A refs
    // hours-pool blob 0, record B refs blob 1.
    const NAME_A: &str = "Grandhotel du Lac Leman"; // exactly 23 bytes; < 24-byte field
    let rec_a = pack_poi_record(48_000_000, 7_800_000, 7, NAME_A, 0);
    let rec_b = pack_poi_record(48_010_000, 7_810_000, 11, "", 1);
    let chunk = pack_poi_chunk(&[rec_a, rec_b], 512);

    // Two distinct 29-byte hours blobs, byte-distinct so the pool ordering is pinned.
    let mut blob0 = [0u8; POI_HOURS_BLOB_LEN];
    blob0[0] = 0x00; // flags
    blob0[1] = 32; // Mon slot0 open_q = 08:00
    blob0[2] = 72; // Mon slot0 close_q = 18:00
    let mut blob1 = [0u8; POI_HOURS_BLOB_LEN];
    blob1[0] = 0x02; // FLAG_TRUNCATED
    blob1[1] = 0; // Mon slot0 open_q = 00:00
    blob1[2] = 96; // Mon slot0 close_q = 24:00 (24/7-style)
    let pool = hours_pool(&[blob0, blob1]);

    // Layout after the POI offset: [directory][cat3 index][cat3 chunk][hours pool]. The directory
    // length is fixed (poi_dir_len). The pool sits right after the chunk.
    let cat3_index_off = poi_off + poi_dir_len();
    let cat3_chunk_off = cat3_index_off + 4; // one u32 node
    let pool_off = cat3_chunk_off + chunk.len();
    let cats: Vec<PoiCat> = (1..=6u8)
        .map(|id| {
            if id == 3 {
                PoiCat { category_id: 3, index_offset: cat3_index_off as u32, node_count: 1, chunk_count: 1 }
            } else {
                // Empty cats: their zero-length index "starts" at the pool offset (past all data).
                PoiCat { category_id: id, index_offset: pool_off as u32, node_count: 0, chunk_count: 0 }
            }
        })
        .collect();

    // Assemble: base up to the POI offset, then [directory][cat3 index][cat3 chunk][pool].
    let mut bytes = base[..poi_off].to_vec();
    bytes.extend_from_slice(&poi_directory(512, &cats, pool_off as u32, 2));
    bytes.extend_from_slice(&0u32.to_le_bytes()); // cat 3's single leaf → chunk 0
    bytes.extend_from_slice(&chunk);
    bytes.extend_from_slice(&pool);

    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);

    let dir = r.poi_directory();
    assert_eq!(dir.chunk_size, 512);
    assert_eq!(dir.entries.len(), 6);
    let cat3 = dir.entries.iter().find(|e| e.category_id == 3).expect("category 3");
    assert!(!cat3.is_empty());
    assert_eq!(cat3.node_count, 1);
    assert_eq!(cat3.chunk_count, 1);
    assert_eq!(cat3.index_offset, cat3_index_off);
    assert_eq!(cat3.data_start(), Some(cat3_chunk_off), "chunks start after the 1-node index");
    // Every other category is still present and empty.
    assert_eq!(dir.entries.iter().filter(|e| e.is_empty()).count(), 5);

    // The two v7 hours-pool directory fields resolve to the pool + its two blobs.
    assert_eq!(dir.hours_pool_count, 2, "two pooled schedules");
    assert_eq!(dir.hours_pool_offset, pool_off, "pool at the section tail");

    // Pin the first record's exact 36 bytes (spec §7.3): lat, lon, subtype, name_len, name, hours_ref.
    let rec = &bytes[cat3_chunk_off..cat3_chunk_off + POI_RECORD_LEN];
    assert_eq!(POI_RECORD_LEN, 36);
    assert_eq!(i32::from_le_bytes(rec[0..4].try_into().unwrap()), 48_000_000, "lat");
    assert_eq!(i32::from_le_bytes(rec[4..8].try_into().unwrap()), 7_800_000, "lon");
    assert_eq!(rec[8], 7, "subtype (hotel)");
    assert_eq!(rec[9], NAME_A.len() as u8, "name_len");
    assert_eq!(&rec[10..10 + NAME_A.len()], NAME_A.as_bytes(), "stored ASCII name");
    assert!(rec[10 + NAME_A.len()..34].iter().all(|&b| b == 0xFF), "unused name tail is 0xFF");
    assert_eq!(u16::from_le_bytes(rec[34..36].try_into().unwrap()), 0, "hours_ref → blob 0");

    // The unnamed second record: Name Len 0, whole 24-byte name field 0xFF, hours_ref → blob 1.
    let rec2 = &bytes[cat3_chunk_off + POI_RECORD_LEN..cat3_chunk_off + 2 * POI_RECORD_LEN];
    assert_eq!(rec2[8], 11, "subtype (wilderness hut)");
    assert_eq!(rec2[9], 0, "unnamed ⇒ name_len 0");
    assert!(rec2[10..34].iter().all(|&b| b == 0xFF), "unnamed record's 24-byte name field is all 0xFF");
    assert_eq!(u16::from_le_bytes(rec2[34..36].try_into().unwrap()), 1, "hours_ref → blob 1");

    // The 0xFF subtype sentinel ends the records (byte 8 of the 3rd record slot), and the chunk
    // pads with 0xFF to 512.
    let sentinel_at = cat3_chunk_off + 2 * POI_RECORD_LEN + 8;
    assert_eq!(bytes[sentinel_at], 0xFF, "a 0xFF subtype byte ends the records");
    assert!(
        bytes[cat3_chunk_off + 2 * POI_RECORD_LEN..cat3_chunk_off + 512].iter().all(|&b| b == 0xFF),
        "chunk padded to 512 with 0xFF"
    );

    // The hours pool bytes (spec §7.5): `count u16` then the two 29-byte blobs at their indices.
    assert_eq!(u16::from_le_bytes(bytes[pool_off..pool_off + 2].try_into().unwrap()), 2, "pool count");
    let blob0_at = pool_off + 2; // hours_pool_offset + 2 + 0*29
    let blob1_at = pool_off + 2 + POI_HOURS_BLOB_LEN; // + 1*29
    assert_eq!(&bytes[blob0_at..blob0_at + POI_HOURS_BLOB_LEN], &blob0, "blob 0 at index 0");
    assert_eq!(&bytes[blob1_at..blob1_at + POI_HOURS_BLOB_LEN], &blob1, "blob 1 at index 1");
}

/// A v6 file (version byte 6) is rejected — the reader accepts v7 only. Forging the
/// version byte alone is enough; the rest of the bytes never get parsed.
#[test]
fn v6_file_is_rejected() {
    let mut bytes = two_lod_file();
    bytes[4] = 6; // downgrade the version byte to v6
    assert!(matches!(MapTables::parse(&SliceSource(&bytes)), Err(Error::BadVersion)));
}

/// A directory whose `category_count` exceeds the reader's bound, whose `chunk_size`
/// exceeds the POI cap, or whose hours-pool region runs past EOF is a corrupt header
/// ⇒ rejected (not an unbounded parse). The POI analogue of the LOD-table overflow
/// guards, extended to the v7 pool fields.
#[test]
fn poi_directory_rejects_out_of_bound_count_and_chunk_size() {
    let bytes = two_lod_file();
    let poi_off = u32::from_le_bytes(bytes[32..36].try_into().unwrap()) as usize;

    // category_count past POI_MAX_CATEGORIES (8).
    let mut forged = bytes.clone();
    forged[poi_off] = 9;
    assert!(matches!(MapTables::parse(&SliceSource(&forged)), Err(Error::BadOffset)), "count 9 > cap");

    // chunk_size past POI_MAX_CHUNK_BYTES (4096).
    let mut forged = bytes.clone();
    forged[poi_off + 1..poi_off + 3].copy_from_slice(&8192u16.to_le_bytes());
    assert!(matches!(MapTables::parse(&SliceSource(&forged)), Err(Error::BadOffset)), "chunk_size 8192 > cap");

    // A POI section offset past EOF is likewise rejected (always-present section, no zero-sentinel).
    let mut forged = bytes.clone();
    let past = bytes.len() as u32;
    forged[32..36].copy_from_slice(&past.to_le_bytes());
    assert!(matches!(MapTables::parse(&SliceSource(&forged)), Err(Error::BadOffset)), "POI offset at EOF");

    // A hours_pool_count large enough to run the pool region past EOF is rejected (the pool fields
    // trail the six per-category entries: offset = poi_off + 3 + 6*13).
    let mut forged = bytes.clone();
    let pool_count_at = poi_off + 3 + 6 * 13 + 4; // hours_pool_offset u32, then the u16 count
    forged[pool_count_at..pool_count_at + 2].copy_from_slice(&0xFFFFu16.to_le_bytes());
    assert!(matches!(MapTables::parse(&SliceSource(&forged)), Err(Error::BadOffset)), "pool count runs past EOF");
}

/// The `empty_poi_directory` builder and the reader agree on the empty v7 layout — a
/// direct pin of the testkit helper the other suites lean on.
#[test]
fn empty_poi_directory_builder_matches_reader() {
    let dir = empty_poi_directory(1000);
    // count(1) + chunk_size(2) + 6 × 13-byte entries + pool fields (offset u32 + count u16) + the
    // 2-byte empty-pool `count` header.
    assert_eq!(dir.len(), poi_dir_len() + 2);
    assert_eq!(poi_dir_len(), 3 + 6 * 13 + 6);
    assert_eq!(dir[0], 6);
    assert_eq!(u16::from_le_bytes([dir[1], dir[2]]), 512);
    // The hours_pool_count field (last 2 bytes of the directory, before the pool's own count) is 0.
    let count_field = 3 + 6 * 13 + 4;
    assert_eq!(u16::from_le_bytes([dir[count_field], dir[count_field + 1]]), 0, "empty pool");
}
