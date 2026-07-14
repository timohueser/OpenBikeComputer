//! Adversarial / extreme-value coverage for the OBCM reader.
//!
//! `format.rs` pins the *happy-path* contract; this file drives the paths it never reaches — the
//! no_std scratch-overflow guards, the uncached oversized-chunk branch, the cross-frame chunk-cache
//! hit through the *public* API, headers that straddle a chunk/ring end, a truncated style table,
//! the multi-block index assembly, and negative microdegrees. Each test asserts a concrete decoded
//! value (whole-feature drop status, bbox, cache hit/miss counts) rather than "didn't panic".

use obc_reader::{
    BBox, DecodeStatus, Error, MapCache, MapTables, Reader, SliceSource, MAX_CHUNK_BYTES, MAX_FEAT_PTS, MAX_FEAT_RINGS,
};
use obcm_testkit::{build_file, pack_line, pack_line_decl, pack_poly_decl, pack_poly_holes, pad, LodSpec, Style};

const STYLES: &[Style] = &[(1, 3, 0xF800, 2, 3, false, None), (2, -1, 0x07E0, 1, 3, false, None)];
const GLOBAL: (i32, i32, i32, i32) = (0, 0, 1000, 1000);

/// Build a single-LOD, single-leaf file over `node`'s bbox holding `chunk` (padded to
/// `chunk_size`). The leaf node bbox == the file bbox, so feature anchors are file-absolute — the
/// same convention the other suites rely on.
fn single_leaf(bbox: (i32, i32, i32, i32), chunk: Vec<u8>, chunk_size: usize) -> Vec<u8> {
    build_file(
        bbox,
        STYLES,
        &[LodSpec { max_mpp: f32::INFINITY, index: vec![0], chunks: vec![pad(chunk, chunk_size)], chunk_size }],
    )
}

/// Decode every feature in `(lod, chunk_id)` into owned `(exterior, ring_lens, bbox)` triples,
/// using exactly the reader's scratch capacities.
struct Decoded {
    style_id: u8,
    exterior_len: usize,
    bbox: BBox,
}

fn decode(r: &Reader, lod: usize, chunk_id: u32, node: &BBox) -> Vec<Decoded> {
    let (out, status) = decode_status(r, lod, chunk_id, node);
    assert_eq!(status.capacity_dropped, 0);
    assert_eq!(status.malformed, 0);
    out
}

fn decode_status(r: &Reader, lod: usize, chunk_id: u32, node: &BBox) -> (Vec<Decoded>, DecodeStatus) {
    let mut out = Vec::new();
    let mut points = heapless::Vec::<_, MAX_FEAT_PTS>::new();
    let mut ring_lens = heapless::Vec::<_, MAX_FEAT_RINGS>::new();
    let status = r
        .for_each_feature(lod, chunk_id, node, &mut points, &mut ring_lens, |f| {
            out.push(Decoded { style_id: f.style_id, exterior_len: f.exterior().len(), bbox: f.bbox() });
        })
        .unwrap();
    (out, status)
}

/// A single feature declaring more exterior points than the caller's scratch holds is consumed but
/// never published, with one explicit capacity outcome.
#[test]
fn exterior_past_max_feat_pts_drops_whole_feature() {
    // Sized to also fit the *smaller* `nrf-mem` chunk cap (8 KB): at ~2 bytes per
    // 8-bit-delta point, 2560 points pack to ~5 KB — over MAX_FEAT_PTS, inside MAX_CHUNK_BYTES.
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

    let (feats, status) = decode_status(&r, 0, 0, &r.bbox);
    assert!(feats.is_empty(), "an over-capacity line must be dropped whole");
    assert_eq!(status.capacity_dropped, 1);
    assert_eq!(status.malformed, 0);
}

/// A polygon with more holes than the caller's ring scratch holds is dropped whole.
#[test]
fn holes_past_max_feat_rings_are_dropped_at_capacity() {
    // Declare twice MAX_FEAT_RINGS holes; only (MAX_FEAT_RINGS - 1) can sit beside the exterior.
    // Each hole is a tiny 3-vertex ring near the anchor (well inside the chunk + coordinate range).
    let holes: Vec<Vec<(i8, i8)>> = (0..MAX_FEAT_RINGS * 2).map(|_| vec![(1i8, 1i8), (1, 0), (0, 1)]).collect();
    let ext = [(50i8, 0i8), (0, 50), (-50, 0)];
    let chunk = pack_poly_holes(2, 100, 100, &ext, &holes);
    assert!(chunk.len() <= MAX_CHUNK_BYTES);

    let bytes = single_leaf(GLOBAL, chunk, MAX_CHUNK_BYTES);
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);

    let (feats, status) = decode_status(&r, 0, 0, &r.bbox);
    assert!(feats.is_empty(), "a polygon whose ring table overflows must be dropped whole");
    assert_eq!(status.capacity_dropped, 1);
    assert_eq!(status.malformed, 0);
}

/// A legal map whose `chunk_size` sits between the cache slot (`CACHE_SLOT_BYTES` = 4096) and the
/// accepted cap (`MAX_CHUNK_BYTES` = 16384). `load_chunk`'s `len > CACHE_SLOT_BYTES` branch reads
/// such a chunk through the *uncached* scratch every call — a miss + read, **never a hit**. Decoded
/// twice here: both must miss (no slot caches it), and the geometry must be byte-correct.
#[test]
fn oversized_chunk_decodes_through_scratch_and_never_caches() {
    const CS: usize = 8192; // 4096 < CS <= 16384 → the scratch path
    let chunk = pack_line(1, 100, 200, &[(10, 0), (0, 10)]);
    let bytes = single_leaf(GLOBAL, chunk, CS);
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);
    assert_eq!(r.lods()[0].chunk_size, CS);

    // First decode: the oversized chunk reads through the scratch — a miss, never a slot.
    let before = r.chunk_cache_stats();
    let f0 = decode(&r, 0, 0, &r.bbox);
    let after = r.chunk_cache_stats();
    assert_eq!(f0.len(), 1);
    assert_eq!(f0[0].exterior_len, 3);
    assert_eq!(after.chunk_hits, before.chunk_hits, "an oversized chunk must not register a hit");
    assert_eq!(after.chunk_misses, before.chunk_misses + 1, "it counts as a miss");

    // Second decode of the *same* chunk: still a miss (it was never cached), proving the scratch
    // path is genuinely uncached — unlike a slot-sized chunk, which would hit here.
    let before2 = r.chunk_cache_stats();
    let f1 = decode(&r, 0, 0, &r.bbox);
    let after2 = r.chunk_cache_stats();
    assert_eq!(after2.chunk_hits, before2.chunk_hits, "the re-read of an oversized chunk still misses");
    assert_eq!(after2.chunk_misses, before2.chunk_misses + 1);
    // Same bytes, same decode both times.
    assert_eq!(f1[0].exterior_len, f0[0].exterior_len);
}

/// The point of the chunk cache, driven through the public `Reader` API (not `MapCacheInner`):
/// querying the *same* viewport twice must serve the second pass from a resident slot — a **hit**
/// — with no extra source read. A slot-sized chunk (≤ 4096) is cacheable, so decode 2 is a pure hit.
#[test]
fn second_pass_over_same_chunk_hits_the_cache_via_public_api() {
    const CS: usize = 256; // well under CACHE_SLOT_BYTES → cacheable
    let chunk = pack_line(1, 100, 200, &[(10, 0), (0, 10)]);
    let bytes = single_leaf(GLOBAL, chunk, CS);
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);

    // Pass 1: cold — the chunk read is a miss that fills a slot.
    let s0 = r.chunk_cache_stats();
    let _ = decode(&r, 0, 0, &r.bbox);
    let s1 = r.chunk_cache_stats();
    assert_eq!(s1.chunk_misses, s0.chunk_misses + 1, "first decode misses and fills a slot");
    assert_eq!(s1.chunk_hits, s0.chunk_hits, "no hit on the cold pass");
    let reads_after_cold = s1.sd_reads;

    // Pass 2: warm — the same (lod, chunk_id, len) is now resident, so it hits with no source read.
    let _ = decode(&r, 0, 0, &r.bbox);
    let s2 = r.chunk_cache_stats();
    assert_eq!(s2.chunk_hits, s1.chunk_hits + 1, "second decode of the same chunk is a cache hit");
    assert_eq!(s2.chunk_misses, s1.chunk_misses, "no further miss");
    assert_eq!(s2.sd_reads, reads_after_cold, "a hit reads nothing from the source");
}

/// LRU eviction ordering observed at the `Reader` level: once more distinct chunks are queried than
/// the cache has slots, the least-recently-used chunk is evicted, so re-querying it misses again
/// while a recently-touched one still hits. The cache has 64 chunk slots (reader.rs
/// `MAP_CHUNK_SLOTS`); this drives 65 distinct chunks so exactly one is evicted, and asserts it is
/// the oldest. Two leaves are *not* enough — this needs the full slot set, which only a public-API
/// walk over many leaves exercises.
#[test]
fn lru_evicts_the_oldest_chunk_at_the_reader_level() {
    // We need more *distinct cached chunks* in one viewport than the cache has slots. An NW-chain
    // where each level hangs three leaf chunks (NE/SW/SE) off it and continues NW yields 3 leaves
    // per level, so ~22 levels gives 65 leaves — all overlapping a whole-bbox view, and the chain
    // stays well under the depth cap (`MAX_QUADTREE_DEPTH` = 32). Every child index strictly exceeds
    // its parent's, so the well-formed `child > idx` invariant holds.
    const SLOTS: usize = 64; // reader.rs MAP_CHUNK_SLOTS
    const LEAVES: usize = SLOTS + 1; // 65 → exactly one eviction
    const CS: usize = 64;

    // Node 0 is the root branch. Each level appends four children: NW (continues the chain, or the
    // final leaf) and NE/SW/SE (distinct leaf chunks). Stop once LEAVES chunks have been placed.
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
    let chunks: Vec<Vec<u8>> = (0..n_chunks).map(|_| pad(pack_line(1, 1, 1, &[(1, 1)]), CS)).collect();
    let bytes = build_file(GLOBAL, STYLES, &[LodSpec { max_mpp: f32::INFINITY, index, chunks, chunk_size: CS }]);
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);

    // Collect the chunk ids in walk order — the order they touch the cache (oldest first).
    let mut walk_order: Vec<u32> = Vec::new();
    r.for_each_chunk(0, &r.bbox, |cid, _| walk_order.push(cid)).unwrap();
    assert!(walk_order.len() > SLOTS, "need more leaves than slots to force an eviction");

    // Pass 1: decode every leaf — fills all 64 slots, then evicts the oldest as the 65th loads.
    let oldest = walk_order[0];
    let newest = *walk_order.last().unwrap();
    for &cid in &walk_order {
        let node = r.bbox;
        let _ = decode(&r, 0, cid, &node);
    }

    // Re-decode the *newest* chunk: still resident → a hit.
    let before = r.chunk_cache_stats();
    let _ = decode(&r, 0, newest, &r.bbox);
    let after = r.chunk_cache_stats();
    assert_eq!(after.chunk_hits, before.chunk_hits + 1, "the most-recently-used chunk is still cached");

    // Re-decode the *oldest* chunk: it was the LRU victim → a miss (re-read).
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

    let (feats, status) = decode_status(&r, 0, 0, &r.bbox);
    assert!(feats.is_empty(), "a physically incomplete ring must not publish partial geometry");
    assert_eq!(status.malformed, 1);
    assert_eq!(status.capacity_dropped, 0);
}

/// Public single-feature refetch must clear both caller buffers even when malformed hole framing is
/// discovered only after a valid exterior has already been decoded into them.
#[test]
fn decode_feature_at_clears_partial_and_stale_scratch_on_malformed_hole() {
    let ext = [(10i8, 0i8), (0, 10), (-10, 0)];
    let holes = vec![vec![(2i8, 2i8), (2, 0), (0, 2)]];
    let mut chunk = pack_poly_holes(1, 100, 100, &ext, &holes);
    // Keep the complete exterior and the hole-count byte, but remove the first hole's u16 count.
    // The decoder therefore mutates scratch before it discovers the structural truncation.
    chunk.truncate(12 + ext.len() * 2 + 1);
    let bytes = single_leaf(GLOBAL, chunk.clone(), chunk.len());
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
/// feature. The skip path parses framing without decoding coordinates, but exposes the same public
/// whole-feature postcondition as the decode path.
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

/// A feature header that itself straddles the chunk end: the `while off + 12 <= cs` guard
/// must stop before reading a partial 12-byte header. We place one whole feature,
/// then trailing bytes too short to be a header (and not 0xFF, so the 0xFF early-out doesn't mask
/// the guard). The whole feature decodes; the runt tail is ignored, not misread.
#[test]
fn header_straddling_chunk_end_is_not_misread() {
    let mut chunk = pack_line(1, 100, 200, &[(10, 0), (0, 10)]);
    // Append 6 non-0xFF bytes — fewer than the 12-byte header — right at the end of a tight chunk
    // so `off + 12 <= cs` is false when the decoder reaches them.
    chunk.extend_from_slice(&[0x01, 0x02, 0x03, 0x04, 0x05, 0x06]);
    let cs = chunk.len(); // no 0xFF pad: the runt tail sits flush at the chunk end
    let bytes = single_leaf(GLOBAL, chunk, cs);
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);

    let feats = decode(&r, 0, 0, &r.bbox);
    assert_eq!(feats.len(), 1, "only the one whole feature decodes; the runt header tail is skipped");
    assert_eq!(feats[0].exterior_len, 3);
}

/// A style table whose count byte claims more records than the file actually holds. `parse_styles`'
/// `o + 8 > want` break must stop at the last whole record, parsing only the
/// styles physically present rather than reading past the table. We build a valid 2-style file,
/// then forge the count byte up to 8: the two real records still parse, the phantom six don't
/// appear, and the reader still constructs (a truncated table is not a hard error).
#[test]
fn truncated_style_table_parses_only_present_records() {
    let bytes = single_leaf(GLOBAL, pack_line(1, 10, 10, &[(1, 1)]), 64);
    // style_offset is fixed at HEADER_LEN (40) by the builder; the count byte is the first byte
    // of the style table.
    let style_off = u32::from_le_bytes(bytes[21..25].try_into().unwrap()) as usize;
    assert_eq!(style_off, obc_formats::obcm::HEADER_LEN);
    let mut forged = bytes.clone();
    forged[style_off] = 8; // claim 8 styles; only 2 records (16 bytes) follow before the LOD table

    let cache = MapCache::new();
    let src = SliceSource(&forged);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);
    // The two real styles parse from the bytes that are present…
    assert!(r.style(1).is_some(), "the first real style still parses");
    assert!(r.style(2).is_some(), "the second real style still parses");
    // …and the decoder still works (truncated table didn't corrupt the offsets after it).
    let feats = decode(&r, 0, 0, &r.bbox);
    assert_eq!(feats.len(), 1);
    assert_eq!(feats[0].style_id, 1);
}

/// `style_offset` pointing at or past the end of the file is a corrupt header and must be
/// rejected (`Error::BadOffset`), not tolerated as a silently-empty style table — that map would
/// load "fine" and render nothing. A `style_offset` *equal to* the file length is the boundary
/// the format suite never hits: `MapTables::parse`'s own header guard accepts `== total` (it
/// checks `> total`), so the rejection must come from `parse_styles` (there is no count byte to
/// read at EOF).
#[test]
fn style_offset_at_eof_is_rejected() {
    let bytes = single_leaf(GLOBAL, pack_line(1, 10, 10, &[(1, 1)]), 64);
    let total = bytes.len() as u32;
    let mut forged = bytes.clone();
    forged[21..25].copy_from_slice(&total.to_le_bytes()); // style_offset = file length

    assert!(
        matches!(MapTables::parse(&SliceSource(&forged)), Err(Error::BadOffset)),
        "a style table at EOF is a corrupt header, not an empty table"
    );
}

/// A quadtree index large enough that a node read crosses an `INDEX_BLOCK` (512-byte) cache-block
/// edge. Each node is 4 bytes, so block 0 holds nodes 0..128 and the node at index 128 begins
/// exactly at the block boundary; reading a node at index ≥128 forces `index_read`
/// to assemble across two blocks. The format suite's ≤9-node trees never leave block 0, so the
/// multi-block path is unexercised.
///
/// A forward NW-*chain* can't reach index 128 — it gains only ~4 indices per level but one depth
/// per level, so it would hit the depth cap (`MAX_QUADTREE_DEPTH` = 32) first. A **breadth-first
/// complete tree** gains width: a depth-4 tree (well under the cap) already has 341 nodes, so its
/// deepest leaves sit far past index 128. Here every depth-4 leaf is empty except the **last**
/// (highest index), which carries chunk 0 — the walk must read nodes across the block seam to find
/// it.
#[test]
fn index_read_crosses_block_boundary() {
    const NODES_PER_BLOCK: usize = 512 / 4; // 128
    const DEPTH: usize = 4; // 4 ≪ MAX_QUADTREE_DEPTH (32); 1+4+16+64 = 85 internal, 256 leaves

    // Breadth-first complete quadtree. Internal nodes occupy indices [0, internal); their four
    // children are laid out contiguously, parent i's children at internal + ... — we assign by a
    // running cursor exactly as the packer's breadth-first `serialize_tree` does, so `child > idx`
    // holds for every branch.
    let internal: usize = (0..DEPTH).map(|d| 4usize.pow(d as u32)).sum(); // 1+4+16+64 = 85
    let leaves: usize = 4usize.pow(DEPTH as u32); // 256
    let total = internal + leaves; // 341
    let mut index = vec![0x7FFF_FFFFu32; total]; // start all empty; fill branches + the one leaf
    let mut next_child = 1usize; // node 0's children start at 1
    for node in index.iter_mut().take(internal) {
        *node = 0x8000_0000 | next_child as u32; // BRANCH_BIT | first-child index
        next_child += 4;
    }
    // The very last node (highest index, deep past the 128 boundary) is the sole non-empty leaf.
    let leaf_idx = total - 1;
    index[leaf_idx] = 0; // → chunk 0
    assert!(leaf_idx >= NODES_PER_BLOCK, "the leaf must sit past the first index block to test the seam");

    const CS: usize = 64;
    let chunk = pad(pack_line(1, 5, 5, &[(2, 2)]), CS);
    let bytes =
        build_file(GLOBAL, STYLES, &[LodSpec { max_mpp: f32::INFINITY, index, chunks: vec![chunk], chunk_size: CS }]);
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);

    // A whole-bbox view intersects every node, so the walk reads all 341 nodes (crossing the block
    // seam repeatedly) and finds the single non-empty leaf past index 128.
    let mut seen = 0;
    let mut found_cid = None;
    r.for_each_chunk(0, &r.bbox, |cid, _| {
        seen += 1;
        found_cid = Some(cid);
    })
    .unwrap();
    assert_eq!(seen, 1, "the single deep leaf is found across the block boundary");
    assert_eq!(found_cid, Some(0));

    // And its chunk decodes through the multi-block index assembly.
    let feats = decode(&r, 0, found_cid.unwrap(), &r.bbox);
    assert_eq!(feats.len(), 1);
    assert_eq!(feats[0].exterior_len, 2);
}

/// Every coordinate in the format suite is positive; a southern/western map carries negative
/// microdegrees, which `rd_i32` must sign-extend correctly for the header bbox *and* the feature
/// anchor. A sign-extension slip would read a small negative as a large positive. The leaf node's
/// min corner is negative, so the absolute anchor (`node.min + ax`) and the decoded vertices land
/// in the southern/western quadrant exactly.
#[test]
fn negative_microdegrees_decode_with_correct_sign() {
    // A bbox straddling the equator/prime meridian into the negative quadrant.
    let bbox = (-2000, -1000, 500, 500); // (min_lon, min_lat, max_lon, max_lat)
                                         // Anchor relative to the leaf node's min corner (min_lon=-2000, min_lat=-1000): ax=100, ay=50,
                                         // so the absolute anchor is (-1900, -950). Deltas dip further negative.
    let chunk = pack_line(1, 100, 50, &[(-50, -25), (10, 0)]);
    let bytes = single_leaf(bbox, chunk, 64);
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);

    // Header bbox round-trips with the negative values intact (no sign-extension slip).
    assert_eq!(r.bbox, BBox { min_lon: -2000, min_lat: -1000, max_lon: 500, max_lat: 500 });

    let feats = decode(&r, 0, 0, &r.bbox);
    assert_eq!(feats.len(), 1);
    let f = &feats[0];
    // anchor (-1900, -950); +(-50,-25) → (-1950, -975); +(10,0) → (-1940, -975).
    assert_eq!(f.exterior_len, 3);
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

    let (feats, status) = decode_status(&r, 0, 0, &r.bbox);
    assert!(feats.is_empty());
    assert_eq!(status.capacity_dropped, 1);
    assert_eq!(status.malformed, 0);
}

// ---------------------------------------------------------------------------------------------
// Pass-B resident-chunk batching (issue #848): `Reader::with_feature_chunk` loads/borrows a chunk
// **once** and decodes every selected offset through the held borrow — one cache request for the
// whole batch instead of one per winner. These pin the primitive directly; the scene-level
// `decode_selected` batching (one request per winning chunk) is covered below.
// ---------------------------------------------------------------------------------------------

/// A chunk with several selected offsets costs exactly **one** cache request in a batch: a cold
/// `with_feature_chunk` decoding all three winners is a single miss, and a second batch over the
/// same resident chunk is a single hit — not one request per feature (the pre-#848 inflation).
#[test]
fn with_feature_chunk_batches_offsets_under_one_cache_request() {
    const CS: usize = 256; // < CACHE_SLOT_BYTES → cacheable (resident-slot path)
    let feat0 = pack_line(1, 100, 100, &[(10, 0), (0, 10)]);
    let feat1 = pack_line(1, 120, 120, &[(5, 5)]);
    let feat2 = pack_line(2, 140, 140, &[(-5, 5), (5, 0)]);
    let offsets = [0usize, feat0.len(), feat0.len() + feat1.len()];
    let mut chunk = feat0.clone();
    chunk.extend_from_slice(&feat1);
    chunk.extend_from_slice(&feat2);
    let bytes = single_leaf(GLOBAL, chunk, CS);
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);

    // Cold batch: all three offsets decode under a single load of chunk 0.
    let before = r.chunk_cache_stats();
    let mut decoded = 0;
    r.with_feature_chunk(0, 0, &r.bbox, |chunk| {
        let mut points = heapless::Vec::<(i32, i32), MAX_FEAT_PTS>::new();
        let mut ring_lens = heapless::Vec::<usize, MAX_FEAT_RINGS>::new();
        for &off in &offsets {
            let f = chunk.decode(off, &mut points, &mut ring_lens).unwrap();
            assert!(!f.exterior().is_empty());
            decoded += 1;
        }
    })
    .unwrap();
    let after = r.chunk_cache_stats();
    assert_eq!(decoded, 3);
    assert_eq!(after.chunk_misses, before.chunk_misses + 1, "cold batch is a single miss for all three winners");
    assert_eq!(after.chunk_hits, before.chunk_hits, "no hit inside the batch");

    // Warm batch: the resident chunk serves all three offsets as one hit, no further miss.
    let before = r.chunk_cache_stats();
    r.with_feature_chunk(0, 0, &r.bbox, |chunk| {
        let mut points = heapless::Vec::<(i32, i32), MAX_FEAT_PTS>::new();
        let mut ring_lens = heapless::Vec::<usize, MAX_FEAT_RINGS>::new();
        for &off in &offsets {
            let _ = chunk.decode(off, &mut points, &mut ring_lens).unwrap();
        }
    })
    .unwrap();
    let after = r.chunk_cache_stats();
    assert_eq!(after.chunk_hits, before.chunk_hits + 1, "warm batch is a single hit for all three winners");
    assert_eq!(after.chunk_misses, before.chunk_misses, "no further miss");
}

/// An oversized chunk (past the cache slot) decodes through the uncached scratch: every batch is a
/// fresh miss, never a hit — the resident-slot invariant, but through the batch primitive.
#[test]
fn with_feature_chunk_oversized_chunk_uses_scratch_each_batch() {
    const CS: usize = 8192; // > CACHE_SLOT_BYTES (4096) → scratch path
    let chunk = pack_line(1, 100, 200, &[(10, 0), (0, 10)]);
    let bytes = single_leaf(GLOBAL, chunk, CS);
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);
    assert_eq!(r.lods()[0].chunk_size, CS);

    for _ in 0..2 {
        let before = r.chunk_cache_stats();
        r.with_feature_chunk(0, 0, &r.bbox, |chunk| {
            let mut points = heapless::Vec::<(i32, i32), MAX_FEAT_PTS>::new();
            let mut ring_lens = heapless::Vec::<usize, MAX_FEAT_RINGS>::new();
            let f = chunk.decode(0, &mut points, &mut ring_lens).unwrap();
            assert_eq!(f.exterior().len(), 3);
        })
        .unwrap();
        let after = r.chunk_cache_stats();
        assert_eq!(after.chunk_misses, before.chunk_misses + 1, "the oversized chunk re-reads (a miss) every batch");
        assert_eq!(after.chunk_hits, before.chunk_hits, "an oversized chunk never registers a hit");
    }
}

/// Stale, out-of-range, `CHUNK_END`, and capacity-dropping offsets keep their typed outcomes inside
/// a batch and leave the caller scratch empty after each failure — the per-feature whole-feature
/// contract, unchanged under batching.
#[test]
fn with_feature_chunk_rejects_bad_offsets_and_empties_scratch() {
    const CS: usize = 128;
    let chunk = pack_line(1, 100, 100, &[(10, 0), (0, 10), (5, 5)]); // 4 exterior points
    let real_len = chunk.len();
    let bytes = single_leaf(GLOBAL, chunk, CS);
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);

    r.with_feature_chunk(0, 0, &r.bbox, |chunk| {
        let mut points = heapless::Vec::<(i32, i32), MAX_FEAT_PTS>::new();
        let mut ring_lens = heapless::Vec::<usize, MAX_FEAT_RINGS>::new();

        // An offset at the chunk length is outside the validated chunk → Decode(Malformed).
        points.push((-1, -1)).unwrap();
        ring_lens.push(9).unwrap();
        assert!(matches!(
            chunk.decode(CS, &mut points, &mut ring_lens),
            Err(obc_reader::FeatureReadError::Decode(obc_reader::FeatureDecodeError::Malformed))
        ));
        assert!(points.is_empty() && ring_lens.is_empty(), "an out-of-range offset leaves scratch empty");

        // An offset landing on the 0xFF pad (the CHUNK_END marker) → Decode(Malformed).
        points.push((-2, -2)).unwrap();
        assert!(matches!(
            chunk.decode(real_len, &mut points, &mut ring_lens),
            Err(obc_reader::FeatureReadError::Decode(obc_reader::FeatureDecodeError::Malformed))
        ));
        assert!(points.is_empty() && ring_lens.is_empty(), "a CHUNK_END offset leaves scratch empty");

        // A valid offset still decodes under the same held borrow — partial-success within a batch.
        let f = chunk.decode(0, &mut points, &mut ring_lens).unwrap();
        assert_eq!(f.exterior().len(), 4);
    })
    .unwrap();

    // A tiny caller scratch drops the 4-point feature with a typed capacity outcome, scratch empty.
    r.with_feature_chunk(0, 0, &r.bbox, |chunk| {
        let mut points = heapless::Vec::<(i32, i32), 2>::new();
        let mut ring_lens = heapless::Vec::<usize, 4>::new();
        assert!(matches!(
            chunk.decode(0, &mut points, &mut ring_lens),
            Err(obc_reader::FeatureReadError::Decode(obc_reader::FeatureDecodeError::Capacity(
                obc_reader::CapacityError::Points
            )))
        ));
        assert!(points.is_empty() && ring_lens.is_empty(), "a capacity drop leaves scratch empty");
    })
    .unwrap();
}

/// The batch borrow is held across the callback exactly like `for_each_feature_filtered`: resident
/// style-table reads stay allowed, but a nested streaming/cache op returns a typed `Busy` error —
/// never a `RefCell` panic.
#[test]
fn nested_cache_op_inside_batch_returns_busy_not_panic() {
    const CS: usize = 128;
    let chunk = pack_line(1, 100, 100, &[(10, 0)]);
    let bytes = single_leaf(GLOBAL, chunk, CS);
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);

    r.with_feature_chunk(0, 0, &r.bbox, |chunk| {
        // Decoding a resident offset takes no new borrow, so it succeeds inside the batch.
        let mut points = heapless::Vec::<(i32, i32), MAX_FEAT_PTS>::new();
        let mut ring_lens = heapless::Vec::<usize, MAX_FEAT_RINGS>::new();
        assert!(chunk.decode(0, &mut points, &mut ring_lens).is_ok());

        // A nested single-feature refetch must return a typed cache-busy Read error, not panic.
        let mut p2 = heapless::Vec::<(i32, i32), MAX_FEAT_PTS>::new();
        let mut r2 = heapless::Vec::<usize, MAX_FEAT_RINGS>::new();
        assert!(matches!(
            r.decode_feature_at(0, 0, 0, &r.bbox, &mut p2, &mut r2),
            Err(obc_reader::FeatureReadError::Read(obc_reader::MapReadError::Cache(obc_reader::CacheError::Busy)))
        ));

        // A nested batch is likewise Busy, not a panic.
        assert!(matches!(
            r.with_feature_chunk(0, 0, &r.bbox, |_| ()),
            Err(obc_reader::MapReadError::Cache(obc_reader::CacheError::Busy))
        ));

        // Resident style-table reads remain available under the held borrow.
        assert!(r.style(1).is_some());
    })
    .unwrap();
}

/// Scene-level pass B (`MapScene::decode_selected`): selecting winners across two chunks costs
/// exactly **one** cache request per winning chunk — two requests for four winners — and both
/// chunks report a refetch. This is the batching #848 delivers end-to-end through the semantic seam.
#[test]
fn decode_selected_loads_each_winning_chunk_once() {
    use obc_map_scene::{
        DecodeReport, Feature, FeatureError as SceneFeatureError, FeatureToken, MapScene, SelectedFeatures,
    };

    /// Minimal `SelectedFeatures`: a fixed token list with a per-index outcome slot.
    struct Sink {
        tokens: Vec<FeatureToken>,
        done: Vec<Option<Result<(), SceneFeatureError>>>,
    }
    impl Sink {
        fn new(tokens: Vec<FeatureToken>) -> Self {
            let n = tokens.len();
            Sink { tokens, done: vec![None; n] }
        }
    }
    impl SelectedFeatures for Sink {
        fn len(&self) -> usize {
            self.tokens.len()
        }
        fn is_pending(&self, index: usize) -> bool {
            self.done.get(index).is_some_and(Option::is_none)
        }
        fn token(&self, index: usize) -> Option<FeatureToken> {
            self.is_pending(index).then(|| self.tokens[index])
        }
        fn decoded(&mut self, index: usize, feature: Feature<'_>) -> bool {
            if !self.is_pending(index) || !feature.has_valid_rings() {
                return false;
            }
            self.done[index] = Some(Ok(()));
            true
        }
        fn failed(&mut self, index: usize, error: SceneFeatureError) -> bool {
            if !self.is_pending(index) {
                return false;
            }
            self.done[index] = Some(Err(error));
            true
        }
    }

    const CS: usize = 256;
    // Two chunks, each two features.
    let a0 = pack_line(1, 100, 100, &[(10, 0), (0, 10)]);
    let a1 = pack_line(2, 120, 120, &[(5, 5)]);
    let mut chunk_a = a0.clone();
    chunk_a.extend_from_slice(&a1);
    let a_off = [0u16, a0.len() as u16];

    let b0 = pack_line(1, 300, 300, &[(-5, 5)]);
    let b1 = pack_line(2, 320, 320, &[(5, 0), (0, 5)]);
    let mut chunk_b = b0.clone();
    chunk_b.extend_from_slice(&b1);
    let b_off = [0u16, b0.len() as u16];

    // Root branch (child base 1) with NW=chunk 0, NE=chunk 1, SW/SE empty — both leaves overlap the
    // whole-bbox view, so `for_each_chunk` visits chunk 0 then chunk 1.
    const EMPTY_LEAF: u32 = 0x7FFF_FFFF; // obc_formats::obcm::EMPTY_LEAF (!BRANCH_BIT)
    let index = vec![0x8000_0000u32 | 1, 0, 1, EMPTY_LEAF, EMPTY_LEAF];
    let bytes = build_file(
        GLOBAL,
        STYLES,
        &[LodSpec { max_mpp: f32::INFINITY, index, chunks: vec![pad(chunk_a, CS), pad(chunk_b, CS)], chunk_size: CS }],
    );
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);

    // All four winners, two per chunk. `FeatureToken` packs (cid_lo, cid_hi, offset).
    let mut sink = Sink::new(vec![
        FeatureToken::from_source_words([0, 0, a_off[0]]),
        FeatureToken::from_source_words([0, 0, a_off[1]]),
        FeatureToken::from_source_words([1, 0, b_off[0]]),
        FeatureToken::from_source_words([1, 0, b_off[1]]),
    ]);

    let mut points = heapless::Vec::<(i32, i32), MAX_FEAT_PTS>::new();
    let mut ring_lens = heapless::Vec::<usize, MAX_FEAT_RINGS>::new();
    let before = r.chunk_cache_stats();
    let report: DecodeReport = r.decode_selected(0, &r.bbox, &mut points, &mut ring_lens, &mut sink);
    let after = r.chunk_cache_stats();

    // Two winning chunks → two pass-B cache requests (cold misses), not four (one per winner).
    assert_eq!(after.chunk_misses, before.chunk_misses + 2, "one load per winning chunk, not per winner");
    assert_eq!(after.chunk_hits, before.chunk_hits, "no hit within a single cold pass");
    assert_eq!(report.chunks_refetched, 2, "both chunks publish at least one selected feature");
    assert_eq!(report.read_failures, Default::default(), "a clean pass records no read failure");
    // Every winner published exactly once.
    assert!(sink.done.iter().all(|d| matches!(d, Some(Ok(())))));
}
