//! Adversarial and extreme-value coverage for the OBCM reader.
//!
//! `format.rs` pins the happy-path contract; this drives the paths it never reaches: the
//! scratch-overflow guards, the uncached oversized-chunk branch, the cross-frame chunk-cache hit
//! through the public API, headers that straddle a chunk or ring end, a truncated style table,
//! the multi-block index assembly, and negative microdegrees.

use obc_map_scene::BBox;
use obc_reader::{Error, MapCache, MapTables, Reader, SliceSource, MAX_CHUNK_BYTES, MAX_FEAT_PTS, MAX_FEAT_RINGS};
use obcm_testkit::{
    align_up, build_file, pack_line, pack_line_decl, pack_poly_decl, pack_poly_holes, resolve_offset, scaled, seal,
    LodSpec, Style, FILLER, STYLE_OFFSET, UNIT,
};

const STYLES: &[Style] = &[(1, 3, 0xF800, 2, 3, false, None), (2, -1, 0x07E0, 1, 3, false, None)];
const GLOBAL: (i32, i32, i32, i32) = (0, 0, 1000, 1000);

/// Build a single-LOD, single-leaf file over `node`'s bbox holding `chunk`, padded to
/// `chunk_size`. The leaf node bbox is the file bbox, so feature anchors are file-absolute.
fn single_leaf(bbox: (i32, i32, i32, i32), chunk: Vec<u8>, chunk_size: usize) -> Vec<u8> {
    build_file(
        bbox,
        STYLES,
        &[LodSpec { max_mpp: f32::INFINITY, index: vec![0], chunks: vec![seal(chunk, chunk_size)], chunk_size }],
    )
}

use crate::common::{decode_chunk_status, Decoded};

/// [`decode_chunk_status`] plus the assertion the happy-path cases want: the walk dropped nothing,
/// so a missing feature is a decode bug and not an over-capacity scratch.
fn decode(r: &Reader, lod: usize, chunk_id: u32, node: &BBox) -> Vec<Decoded> {
    let (out, status) = decode_chunk_status(r, lod, chunk_id, node);
    assert_eq!(status.capacity_dropped, 0);
    assert_eq!(status.malformed, 0);
    out
}

/// A feature declaring more exterior points than the caller's scratch holds is consumed but never
/// published, with one explicit capacity outcome.
#[test]
fn exterior_past_max_feat_pts_drops_whole_feature() {
    // At about 2 bytes per point, 2560 points pack to 5 KB: over MAX_FEAT_PTS, inside the chunk cap.
    const DECL: u16 = MAX_FEAT_PTS as u16 + 512;
    let anchor = (10, 20);
    let deltas: Vec<(i8, i8)> = vec![(1i8, 1i8); DECL as usize - 1];
    let chunk = pack_line_decl(1, anchor.0, anchor.1, DECL, &deltas);
    assert!(chunk.len() <= MAX_CHUNK_BYTES, "fixture must fit the accepted chunk cap");

    let bytes = single_leaf(GLOBAL, chunk, MAX_CHUNK_BYTES);
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);

    let (feats, status) = decode_chunk_status(&r, 0, 0, &r.bbox);
    assert!(feats.is_empty(), "an over-capacity line must be dropped whole");
    assert_eq!(status.capacity_dropped, 1);
    assert_eq!(status.malformed, 0);
}

/// A polygon with more holes than the caller's ring scratch holds is dropped whole.
#[test]
fn holes_past_max_feat_rings_are_dropped_at_capacity() {
    // Twice MAX_FEAT_RINGS holes, of which only (MAX_FEAT_RINGS - 1) can sit beside the exterior.
    // Each hole is a tiny 3-vertex ring near the anchor.
    let holes: Vec<Vec<(i8, i8)>> = (0..MAX_FEAT_RINGS * 2).map(|_| vec![(1i8, 1i8), (1, 0), (0, 1)]).collect();
    let ext = [(50i8, 0i8), (0, 50), (-50, 0)];
    let chunk = pack_poly_holes(2, 100, 100, &ext, &holes);
    assert!(chunk.len() <= MAX_CHUNK_BYTES);

    let bytes = single_leaf(GLOBAL, chunk, MAX_CHUNK_BYTES);
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);

    let (feats, status) = decode_chunk_status(&r, 0, 0, &r.bbox);
    assert!(feats.is_empty(), "a polygon whose ring table overflows must be dropped whole");
    assert_eq!(status.capacity_dropped, 1);
    assert_eq!(status.malformed, 0);
}

/// A legal map with a chunk whose real length sits between the cache slot and the accepted cap.
/// `load_chunk` reads such a chunk through the uncached scratch every call, so both decodes here
/// must miss and the geometry must still be byte-correct. Chunks are tight, so the length has to
/// be filled to get there: a declared `chunk_size` above the slot does not imply a chunk above it.
#[test]
fn oversized_chunk_decodes_through_scratch_and_never_caches() {
    const CS: usize = 8192; // 4096 < CS <= 16384 → the capacity that admits such a chunk
    let mut chunk = Vec::new();
    let mut feature_count = 0usize;
    while chunk.len() <= 4096 {
        chunk.extend_from_slice(&pack_line(1, 100, 200, &[(10, 0), (0, 10)]));
        feature_count += 1;
    }
    let bytes = single_leaf(GLOBAL, chunk, CS);
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);
    assert_eq!(r.lods()[0].chunk_size, CS);

    // First decode: the oversized chunk reads through the scratch, a miss and never a slot.
    let before = r.chunk_cache_stats();
    let f0 = decode(&r, 0, 0, &r.bbox);
    let after = r.chunk_cache_stats();
    assert_eq!(f0.len(), feature_count);
    assert_eq!(f0[0].exterior.len(), 3);
    assert_eq!(after.chunk_hits, before.chunk_hits, "an oversized chunk must not register a hit");
    assert_eq!(after.chunk_misses, before.chunk_misses + 1, "it counts as a miss");

    // Second decode of the same chunk: still a miss, because it was never cached, unlike a
    // slot-sized chunk, which would hit here.
    let before2 = r.chunk_cache_stats();
    let f1 = decode(&r, 0, 0, &r.bbox);
    let after2 = r.chunk_cache_stats();
    assert_eq!(after2.chunk_hits, before2.chunk_hits, "the re-read of an oversized chunk still misses");
    assert_eq!(after2.chunk_misses, before2.chunk_misses + 1);
    // Same bytes, same decode both times.
    assert_eq!(f1[0].exterior.len(), f0[0].exterior.len());
}

/// The point of the chunk cache, driven through the public API: querying the same viewport twice
/// must serve the second pass from a resident slot, with no extra source read.
#[test]
fn second_pass_over_same_chunk_hits_the_cache_via_public_api() {
    const CS: usize = 256; // well under CACHE_SLOT_BYTES → cacheable
    let chunk = pack_line(1, 100, 200, &[(10, 0), (0, 10)]);
    let bytes = single_leaf(GLOBAL, chunk, CS);
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);

    // Pass 1: cold, so the chunk read is a miss that fills a slot.
    let s0 = r.chunk_cache_stats();
    let _ = decode(&r, 0, 0, &r.bbox);
    let s1 = r.chunk_cache_stats();
    assert_eq!(s1.chunk_misses, s0.chunk_misses + 1, "first decode misses and fills a slot");
    assert_eq!(s1.chunk_hits, s0.chunk_hits, "no hit on the cold pass");
    let reads_after_cold = s1.sd_reads;

    // Pass 2: warm, so the same key is resident and hits with no source read.
    let _ = decode(&r, 0, 0, &r.bbox);
    let s2 = r.chunk_cache_stats();
    assert_eq!(s2.chunk_hits, s1.chunk_hits + 1, "second decode of the same chunk is a cache hit");
    assert_eq!(s2.chunk_misses, s1.chunk_misses, "no further miss");
    assert_eq!(s2.sd_reads, reads_after_cold, "a hit reads nothing from the source");
}

/// RRIP eviction observed at the `Reader` level: once more distinct chunks are queried than the
/// cache holds, the first resident chunk is evicted, so re-querying it misses while the newest
/// still hits. This needs the full five-slot set, which only a public-API walk exercises.
#[test]
fn rrip_evicts_the_first_chunk_after_five_slots_fill() {
    // More distinct cached chunks in one viewport than the cache has slots. An NW-chain where
    // each level hangs three leaf chunks off it yields 3 leaves per level, so a couple of levels
    // give 5, all overlapping a whole-bbox view and well under the depth cap. Every child index
    // exceeds its parent's, so the well-formed invariant holds.
    const SLOTS: usize = 5; // four dedicated slots + the shared decode scratch
    const LEAVES: usize = SLOTS + 1; // six → exactly one eviction
    const CS: usize = 64;

    // Node 0 is the root branch. Each level appends four children: NW continues the chain and
    // NE/SW/SE are distinct leaf chunks.
    let mut index: Vec<u32> = vec![0];
    let mut chunk_ids: Vec<u32> = Vec::new();
    let mut cur = 0usize;
    let mut next_chunk = 0u32;
    while chunk_ids.len() < LEAVES {
        let base = index.len() as u32;
        index[cur] = 0x8000_0000 | base; // BRANCH_BIT | child base
                                         // NW continues the chain (filled next iteration); NE/SW/SE are leaf chunks.
        let nw_slot = index.len();
        index.push(0); // NW placeholder
        for _ in 0..3 {
            index.push(next_chunk);
            chunk_ids.push(next_chunk);
            next_chunk += 1;
            if chunk_ids.len() >= LEAVES {
                break;
            }
        }
        cur = nw_slot;
    }
    index[cur] = next_chunk; // deepest NW becomes the final leaf chunk
    chunk_ids.push(next_chunk);

    let n_chunks = chunk_ids.len();
    let chunks: Vec<Vec<u8>> = (0..n_chunks).map(|_| seal(pack_line(1, 1, 1, &[(1, 1)]), CS)).collect();
    let bytes = build_file(GLOBAL, STYLES, &[LodSpec { max_mpp: f32::INFINITY, index, chunks, chunk_size: CS }]);
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);

    // The chunk ids in walk order, which is the order they touch the cache, oldest first.
    let mut walk_order: Vec<u32> = Vec::new();
    r.for_each_chunk(0, &r.bbox, |cid, _| walk_order.push(cid)).unwrap();
    assert!(walk_order.len() > SLOTS, "need more leaves than slots to force an eviction");

    // Pass 1: every leaf fills all five slots, then the sixth load evicts the first.
    let oldest = walk_order[0];
    let newest = *walk_order.last().unwrap();
    for &cid in &walk_order {
        let node = r.bbox;
        let _ = decode(&r, 0, cid, &node);
    }

    // The newest chunk is still resident, so it hits.
    let before = r.chunk_cache_stats();
    let _ = decode(&r, 0, newest, &r.bbox);
    let after = r.chunk_cache_stats();
    assert_eq!(after.chunk_hits, before.chunk_hits + 1, "the newest chunk is still cached");

    // The first chunk was the initial RRIP victim, so it misses and re-reads.
    let before = r.chunk_cache_stats();
    let _ = decode(&r, 0, oldest, &r.bbox);
    let after = r.chunk_cache_stats();
    assert_eq!(after.chunk_misses, before.chunk_misses + 1, "the oldest chunk was evicted and must re-read");
    assert_eq!(after.chunk_hits, before.chunk_hits, "the evicted chunk is not a hit");
}

/// A feature whose declared exterior runs past the physical chunk is malformed and dropped whole.
#[test]
fn truncated_ring_drops_whole_feature() {
    const DECL: u16 = 40; // far more than the 5 deltas supplied
    let real = [(1i8, 1i8), (1, 1), (1, 1), (1, 1), (1, 1)];
    let chunk = pack_line_decl(1, 10, 10, DECL, &real);
    // Pad to a chunk only a little larger than the real bytes, so the declared-but-absent deltas
    // run into the 0xFF pad and then off the chunk end.
    const CS: usize = 64;
    let bytes = single_leaf(GLOBAL, chunk, CS);
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);

    let (feats, status) = decode_chunk_status(&r, 0, 0, &r.bbox);
    assert!(feats.is_empty(), "a physically incomplete ring must not publish partial geometry");
    assert_eq!(status.malformed, 1);
    assert_eq!(status.capacity_dropped, 0);
}

/// A public single-feature refetch must clear both caller buffers even when malformed hole framing
/// is found only after a valid exterior was decoded into them.
#[test]
fn decode_feature_at_clears_partial_and_stale_scratch_on_malformed_hole() {
    let ext = [(10i8, 0i8), (0, 10), (-10, 0)];
    let holes = vec![vec![(2i8, 2i8), (2, 0), (0, 2)]];
    let mut chunk = pack_poly_holes(1, 100, 100, &ext, &holes);
    // Keep the complete exterior and the hole-count byte, but remove the first hole's count, so
    // the decoder mutates scratch before it discovers the truncation.
    chunk.truncate(7 + ext.len() * 2 + 1);
    let bytes = single_leaf(GLOBAL, chunk.clone(), chunk.len() + 1);
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);
    let mut points = heapless::Vec::<(i32, i32), 16>::new();
    let mut ring_lens = heapless::Vec::<usize, 4>::new();
    points.push((-1, -1)).unwrap();
    ring_lens.push(99).unwrap();

    let result = r.decode_feature_at(0, 0, 0, &r.bbox, &mut points, &mut ring_lens);
    assert!(matches!(result, Err(obc_reader::FeatureReadError::Decode(obc_reader::FeatureDecodeError::Malformed))));
    assert!(points.is_empty(), "partial exterior and stale points must be cleared");
    assert!(ring_lens.is_empty(), "partial exterior and stale ring lengths must be cleared");
}

/// A malformed feature rejected by the filter still clears scratch left by the preceding selected
/// feature: the skip path parses framing without decoding coordinates, but exposes the same
/// whole-feature postcondition.
#[test]
fn filtered_malformed_skip_clears_prior_feature_scratch() {
    let mut chunk = pack_line(1, 100, 100, &[(10, 0)]);
    chunk.extend_from_slice(&pack_line_decl(2, 120, 120, 40, &[(1, 0); 5]));
    let bytes = single_leaf(GLOBAL, chunk, 64);
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);
    let mut points = heapless::Vec::<_, MAX_FEAT_PTS>::new();
    let mut ring_lens = heapless::Vec::<_, MAX_FEAT_RINGS>::new();
    points.push((-1, -1)).unwrap();
    ring_lens.push(99).unwrap();
    let mut visited = 0usize;

    let status = r
        .for_each_feature_filtered(
            0,
            0,
            &r.bbox,
            &mut points,
            &mut ring_lens,
            |style_id| style_id == 1,
            |_| visited += 1,
        )
        .unwrap();

    assert_eq!(visited, 1, "the valid selected feature must be visited first");
    assert_eq!(status.complete, 1);
    assert_eq!(status.malformed, 1);
    assert_eq!(status.capacity_dropped, 0);
    assert!(points.is_empty(), "malformed filtered framing must clear prior/stale points");
    assert!(ring_lens.is_empty(), "malformed filtered framing must clear prior/stale ring lengths");
}

/// A feature header that straddles the chunk end: one whole feature, then trailing bytes too short
/// to be a header and not `0xFF`, and no sentinel. The whole feature still decodes and no partial
/// header is read, but the runt owes the caller a malformed drop, because a chunk whose stream
/// does not end on the sentinel is truncated.
#[test]
fn header_straddling_chunk_end_is_a_malformed_drop() {
    let mut chunk = pack_line(1, 100, 200, &[(10, 0), (0, 10)]);
    chunk.extend_from_slice(&[0x01, 0x02, 0x03, 0x04, 0x05]);
    // Hand `build_file` the chunk unsealed, with the runt tail flush at the end. That flushness is
    // load-bearing: the filler behind a short chunk is `0xFF`, so a runt with filler behind it
    // would find enough bytes to complete a header and decode as a second, bogus feature.
    let cs = chunk.len();
    assert_eq!(cs, UNIT, "the runt has to end flush with the chunk's span, not run into filler");
    let bytes = build_file(
        GLOBAL,
        STYLES,
        &[LodSpec { max_mpp: f32::INFINITY, index: vec![0], chunks: vec![chunk], chunk_size: cs }],
    );
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);

    let (feats, status) = decode_chunk_status(&r, 0, 0, &r.bbox);
    assert_eq!(feats.len(), 1, "the one whole feature still decodes");
    assert_eq!(feats[0].exterior.len(), 3);
    assert_eq!(status.complete, 1);
    assert_eq!(status.malformed, 1, "the runt tail is reported, not silently skipped");
    assert_eq!(status.capacity_dropped, 0);
}

/// A style table whose count byte claims more records than the section holds is corrupt. Accepting
/// only the whole prefix could give light and dark presentations different ID sets.
#[test]
fn truncated_style_table_is_rejected() {
    let bytes = single_leaf(GLOBAL, pack_line(1, 10, 10, &[(1, 1)]), 64);
    // `Style Offset` names the first unit boundary past the header, and the count byte is the
    // first byte of the style table.
    let style_off = resolve_offset(&bytes, 21);
    assert_eq!(style_off, STYLE_OFFSET);
    let mut forged = bytes.clone();
    forged[style_off] = 8; // claim 8 styles; only 2 records (16 bytes) follow before the LOD table

    assert!(matches!(MapTables::parse(&SliceSource(&forged)), Err(Error::BadOffset)));
}

/// A `style_offset` at or past the end of the file is a corrupt header and must be rejected, not
/// tolerated as a silently empty style table that would load and render nothing. Equal to the file
/// length is the boundary: `MapTables::parse`'s header guard accepts `== total`, so the rejection
/// must come from `parse_styles`, where there is no count byte to read.
#[test]
fn style_offset_at_eof_is_rejected() {
    let mut forged = single_leaf(GLOBAL, pack_line(1, 10, 10, &[(1, 1)]), 64);
    // The file ends on a unit boundary, so `== total` is a value the scaled field can express.
    forged.resize(align_up(forged.len()), FILLER);
    let end = forged.len();
    forged[21..25].copy_from_slice(&scaled(end).to_le_bytes()); // style_offset = file length

    assert!(
        matches!(MapTables::parse(&SliceSource(&forged)), Err(Error::BadOffset)),
        "a style table at EOF is a corrupt header, not an empty table"
    );
}

/// A quadtree index large enough that a node read crosses a 512-byte cache-block edge. Each node
/// is 4 bytes, so the node at index 128 begins exactly at the boundary and reading it forces
/// `index_read` to assemble across two blocks.
///
/// A forward NW-chain cannot reach index 128: it gains about four indices per level but one depth
/// per level, so it hits the depth cap first. A breadth-first complete tree gains width instead,
/// and a depth-4 tree already has 341 nodes. Every depth-4 leaf here is empty except the last,
/// which carries chunk 0, so the walk must cross the block seam to find it.
#[test]
fn index_read_crosses_block_boundary() {
    const NODES_PER_BLOCK: usize = 512 / 4; // 128
    const DEPTH: usize = 4; // 4 ≪ MAX_QUADTREE_DEPTH (32); 1+4+16+64 = 85 internal, 256 leaves

    // Breadth-first complete quadtree, assigned by a running cursor exactly as the packer's
    // `serialize_tree` does, so `child > idx` holds for every branch.
    let internal: usize = (0..DEPTH).map(|d| 4usize.pow(d as u32)).sum(); // 1+4+16+64 = 85
    let leaves: usize = 4usize.pow(DEPTH as u32); // 256
    let total = internal + leaves; // 341
    let mut index = vec![0x7FFF_FFFFu32; total]; // start all empty; fill branches + the one leaf
    let mut next_child = 1usize; // node 0's children start at 1
    for node in index.iter_mut().take(internal) {
        *node = 0x8000_0000 | next_child as u32; // BRANCH_BIT | first-child index
        next_child += 4;
    }
    // The very last node, deep past the 128 boundary, is the sole non-empty leaf.
    let leaf_idx = total - 1;
    index[leaf_idx] = 0; // → chunk 0
    assert!(leaf_idx >= NODES_PER_BLOCK, "the leaf must sit past the first index block to test the seam");

    const CS: usize = 64;
    let chunk = seal(pack_line(1, 5, 5, &[(2, 2)]), CS);
    let bytes =
        build_file(GLOBAL, STYLES, &[LodSpec { max_mpp: f32::INFINITY, index, chunks: vec![chunk], chunk_size: CS }]);
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);

    // A whole-bbox view intersects every node, so the walk reads all 341 nodes across the block
    // seam and finds the single non-empty leaf.
    let mut seen = 0;
    let mut found_cid = None;
    r.for_each_chunk(0, &r.bbox, |cid, _| {
        seen += 1;
        found_cid = Some(cid);
    })
    .unwrap();
    assert_eq!(seen, 1, "the single deep leaf is found across the block boundary");
    assert_eq!(found_cid, Some(0));

    let feats = decode(&r, 0, found_cid.unwrap(), &r.bbox);
    assert_eq!(feats.len(), 1);
    assert_eq!(feats[0].exterior.len(), 2);
}

/// A southern or western map carries negative microdegrees, which `rd_i32` must sign-extend for
/// the header bbox and the feature anchor alike. A sign-extension slip would read a small negative
/// as a large positive. The leaf node's min corner is negative, so the absolute anchor and the
/// decoded vertices land in the negative quadrant exactly.
#[test]
fn negative_microdegrees_decode_with_correct_sign() {
    // A bbox straddling the equator and prime meridian into the negative quadrant.
    let bbox = (-2000, -1000, 500, 500); // (min_lon, min_lat, max_lon, max_lat)
                                         // Anchor relative to the leaf node's min corner (min_lon=-2000, min_lat=-1000): ax=100, ay=50,
                                         // so the absolute anchor is (-1900, -950). Deltas dip further negative.
    let chunk = pack_line(1, 100, 50, &[(-50, -25), (10, 0)]);
    let bytes = single_leaf(bbox, chunk, 64);
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);

    // The header bbox round-trips with the negative values intact.
    assert_eq!(r.bbox, BBox { min_lon: -2000, min_lat: -1000, max_lon: 500, max_lat: 500 });

    let feats = decode(&r, 0, 0, &r.bbox);
    assert_eq!(feats.len(), 1);
    let f = &feats[0];
    // anchor (-1900, -950); +(-50,-25) → (-1950, -975); +(10,0) → (-1940, -975).
    assert_eq!(f.exterior.len(), 3);
    // bbox spans the negative coordinates exactly.
    assert_eq!(f.bbox, BBox { min_lon: -1950, min_lat: -975, max_lon: -1900, max_lat: -950 });
}

/// Confirms a polygon exterior over capacity is dropped whole, just like a line.
#[test]
fn polygon_exterior_overflow_drops_whole_feature() {
    const DECL: u16 = 3000; // > MAX_FEAT_PTS (2048)
    let deltas: Vec<(i8, i8)> = vec![(1i8, 0i8); DECL as usize - 1];
    let chunk = pack_poly_decl(2, 0, 0, DECL, &deltas);
    assert!(chunk.len() <= MAX_CHUNK_BYTES);
    let bytes = single_leaf((0, 0, 1_000_000, 1_000_000), chunk, MAX_CHUNK_BYTES);
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);

    let (feats, status) = decode_chunk_status(&r, 0, 0, &r.bbox);
    assert!(feats.is_empty());
    assert_eq!(status.capacity_dropped, 1);
    assert_eq!(status.malformed, 0);
}
