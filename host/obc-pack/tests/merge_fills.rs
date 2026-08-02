//! Integration: the `merge_fills` transform through the real quadtree build +
//! serializer + `obc-reader` read-back. The unit suite in `src/merge.rs` pins the
//! dissolve semantics; this closes the loop end to end — a merged pack is smaller,
//! still packs every leaf (`dropped == 0`), round-trips through the reader, and a
//! no-candidate run is byte-identical to packing without the transform at all.

use obc_elevation::NullElevation;
use obc_pack::geom::Geom;
use obc_pack::merge::{merge_classes, merge_fills};
use obc_pack::quadtree::build_lod;
use obc_pack::{serialize_lods, LodLayer, Style};
use obc_reader::{MapCache, MapTables, Reader, SliceSource, MAX_FEAT_PTS, MAX_FEAT_RINGS};

// A generous bbox so every fixture square sits well inside the root (degrees ×1e6).
const GLOBAL: (i64, i64, i64, i64) = (0, 0, 20_000_000, 20_000_000);
const MARKER: u16 = 0xF800;
const CHUNK: usize = 4096;

fn fill(id: u8, color: u16) -> Style {
    Style { id, z_index: 0, color, weight: 1, priority: 3, dashed: false, color2: None }
}

/// A closed square in grid cell `(gx, gy)`, first == last. The 0.001° side keeps
/// every edge under the serializer's densify threshold (30 000 µdeg) and the whole
/// block under one chunk, so unmerged tiles stay one feature each (no split/clip).
fn cell(style_id: u8, gx: i64, gy: i64) -> (u8, Geom) {
    const S: f64 = 0.001;
    let (x, y) = (gx as f64 * S, gy as f64 * S);
    (
        style_id,
        Geom::Polygon { exterior: vec![(x, y), (x + S, y), (x + S, y + S), (x, y + S), (x, y)], interiors: vec![] },
    )
}

/// Pack one LOD of `(style_id, geom)` features exactly as the pipeline does.
fn pack(features: Vec<(u8, Geom)>, styles: &[Style]) -> (Vec<u8>, usize) {
    let root = build_lod(features, GLOBAL, CHUNK);
    let lod = LodLayer { max_mpp: None, chunk_size: CHUNK, root };
    serialize_lods(
        &[lod],
        styles,
        MARKER,
        GLOBAL,
        &[],
        &Default::default(),
        &obc_pack::config::default_profiles(),
        &mut NullElevation,
    )
}

/// `(feature count, total vertex count)` decoded from LOD 0 through the real reader
/// path. Vertices are the padding-free content signal — chunks are 0xFF-padded to
/// `chunk_size`, so a single-chunk pack's raw byte length hides the dissolve win;
/// the map-file shrink on real (multi-chunk) extracts is reported in the PR bench.
fn decode_stats(bytes: &[u8]) -> (usize, usize) {
    let cache = MapCache::new();
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).expect("parse tables");
    let r = Reader::new(&src, &tables, &cache);
    let mut chunks = Vec::new();
    r.for_each_chunk(0, &r.bbox, |cid, node| chunks.push((cid, node))).unwrap();
    let mut points = heapless::Vec::<_, MAX_FEAT_PTS>::new();
    let mut ring_lens = heapless::Vec::<_, MAX_FEAT_RINGS>::new();
    let (mut nfeat, mut nverts) = (0, 0);
    for (cid, node) in chunks {
        r.for_each_feature(0, cid, &node, &mut points, &mut ring_lens, |f| {
            nfeat += 1;
            nverts += f.exterior().len() + f.interiors().map(|h| h.len()).sum::<usize>();
        })
        .unwrap();
    }
    (nfeat, nverts)
}

/// A 5×5 block of adjacent same-style unit squares dissolves to one polygon: the
/// merged pack has one feature, is smaller, and both packs drop nothing.
#[test]
fn adjacent_block_merges_smaller_and_round_trips() {
    let styles = [fill(1, 0x00F0)];
    let classes = merge_classes(&styles);
    let mut feats = Vec::new();
    for gx in 0..5 {
        for gy in 0..5 {
            feats.push(cell(1, gx, gy));
        }
    }

    let (off_bytes, off_dropped) = pack(feats.clone(), &styles);
    let (merged, stats) = merge_fills(feats, &classes);
    let (on_bytes, on_dropped) = pack(merged, &styles);

    assert_eq!((off_dropped, on_dropped), (0, 0), "every leaf packs in both runs");
    assert_eq!((stats.merged_inputs, stats.merged_outputs), (25, 1), "25 tiles → 1 dissolved polygon");
    let (off_feats, off_verts) = decode_stats(&off_bytes);
    let (on_feats, on_verts) = decode_stats(&on_bytes);
    assert_eq!((off_feats, on_feats), (25, 1), "25 tiles unmerged → 1 polygon merged, both round-trip");
    assert!(on_verts < off_verts, "the dissolved block carries fewer vertices ({on_verts} vs {off_verts})");
}

/// With no mergeable adjacency (every feature its own class), `merge_fills` is a
/// no-op: the serialized pack is byte-identical to packing without it — the
/// "flag on, nothing to merge ⇒ empty diff" guarantee, at the byte level.
#[test]
fn no_candidates_is_byte_identical() {
    // Four squares, each a distinct style/color ⇒ four singleton classes.
    let styles = [fill(1, 0x0001), fill(2, 0x0002), fill(3, 0x0003), fill(4, 0x0004)];
    let classes = merge_classes(&styles);
    let feats = vec![cell(1, 0, 0), cell(2, 2, 0), cell(3, 0, 2), cell(4, 2, 2)];

    let (off_bytes, _) = pack(feats.clone(), &styles);
    let (merged, stats) = merge_fills(feats, &classes);
    let (on_bytes, _) = pack(merged, &styles);

    assert_eq!(stats.singletons, 4, "each square is a byte-untouched singleton");
    assert_eq!(stats.merged_classes, 0, "nothing actually unioned");
    assert_eq!(on_bytes, off_bytes, "a no-op merge serializes byte-identically");
}
