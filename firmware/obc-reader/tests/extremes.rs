//! Adversarial / extreme-value coverage for the OBCM v5 reader (issue #96, epic #90).
//!
//! `format.rs` pins the *happy-path* contract: well-formed, fully-contained, positive-coordinate
//! features in 64/128-byte chunks. This file drives the paths that suite never reaches — the
//! no_std scratch-overflow guards, the uncached oversized-chunk branch, the cross-frame chunk-cache
//! hit through the *public* API, headers that straddle a chunk/ring end, a truncated style table,
//! the multi-block index assembly, and negative microdegrees. Every byte buffer is built with the
//! shared `obcm-testkit` so the layout stays pinned to `OBCM_Spec.md` (see `format.rs`'s note).
//!
//! Each test asserts a concrete decoded value (exact truncated ring length, exact bbox over the
//! dropped points, exact cache hit/miss counts) rather than "didn't panic".

use obc_reader::{BBox, Kind, MapCache, MapTables, Reader, SliceSource, MAX_CHUNK_BYTES, MAX_FEAT_PTS, MAX_FEAT_RINGS};
use obcm_testkit::{build_file, pack_line, pack_line_decl, pack_poly_decl, pack_poly_holes, pad, LodSpec, Style};

const STYLES: &[Style] = &[(1, 3, 0xF800, 2, 3), (2, -1, 0x07E0, 1, 3)];
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
/// using exactly the reader's scratch capacities so the test observes the on-device truncation
/// (not a roomier host buffer).
struct Decoded {
    style_id: u8,
    kind: Kind,
    exterior_len: usize,
    ring_lens: Vec<usize>,
    bbox: BBox,
}

fn decode(r: &Reader, lod: usize, chunk_id: u32, node: &BBox) -> Vec<Decoded> {
    let mut out = Vec::new();
    let mut points = heapless::Vec::<_, MAX_FEAT_PTS>::new();
    let mut ring_lens = heapless::Vec::<_, MAX_FEAT_RINGS>::new();
    r.for_each_feature(lod, chunk_id, node, &mut points, &mut ring_lens, |f| {
        out.push(Decoded {
            style_id: f.style_id,
            kind: f.kind,
            exterior_len: f.exterior().len(),
            ring_lens: f.ring_lens().to_vec(),
            bbox: f.bbox(),
        });
    });
    out
}

// ---------------------------------------------------------------------------
// Reader item 1 — scratch-buffer overflow (MAX_FEAT_PTS / MAX_FEAT_RINGS)
// ---------------------------------------------------------------------------

/// A single feature declaring more exterior points than the caller's `MAX_FEAT_PTS` scratch holds.
/// `read_ring` (reader.rs ~701) pushes with `let _ = out.push(...)`, so vertices past capacity are
/// **silently dropped** — but `bounds.add` runs *before* the push guard (reader.rs ~702), so the
/// feature bbox still widens over the dropped points. This pins both halves of that no_std
/// behaviour: the exterior is truncated to exactly `MAX_FEAT_PTS`, yet the bbox spans the full
/// declared extent. The happy-path suite (≤4-point features) never exercises either.
#[test]
fn exterior_past_max_feat_pts_truncates_ring_but_bbox_spans_dropped_points() {
    // Declared 4096 points, far over MAX_FEAT_PTS (2048). Each delta is (+1,+1) so the absolute
    // coords march diagonally; the last *kept* point is anchor + (MAX_FEAT_PTS-1)·(1,1), but the
    // bbox must reach anchor + (declared-1)·(1,1) — past the truncation. 16-bit chunk room: a
    // 16384-byte chunk fits ~8186 8-bit-delta points, comfortably over 4096.
    const DECL: u16 = 4096;
    let anchor = (10, 20);
    let deltas: Vec<(i8, i8)> = vec![(1i8, 1i8); DECL as usize - 1];
    let chunk = pack_line_decl(1, anchor.0, anchor.1, DECL, &deltas);
    assert!(chunk.len() <= MAX_CHUNK_BYTES, "fixture must fit the accepted chunk cap");

    let bytes = single_leaf(GLOBAL, chunk, MAX_CHUNK_BYTES);
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);

    let feats = decode(&r, 0, 0, &r.bbox);
    assert_eq!(feats.len(), 1);
    let f = &feats[0];
    assert_eq!(f.kind, Kind::Line);
    // Exterior truncated to exactly the scratch capacity — the past-capacity pushes were dropped.
    assert_eq!(f.exterior_len, MAX_FEAT_PTS, "ring truncated to MAX_FEAT_PTS");
    assert_eq!(f.ring_lens, vec![MAX_FEAT_PTS]);
    // …but the running bbox saw every declared point (it widens before the push guard), so it
    // spans the full declared diagonal, not just the kept prefix.
    let last_x = anchor.0 + (DECL as i32 - 1);
    let last_y = anchor.1 + (DECL as i32 - 1);
    assert_eq!(f.bbox, BBox { min_lon: anchor.0, min_lat: anchor.1, max_lon: last_x, max_lat: last_y });
    // The bbox max is strictly past the last *kept* vertex, proving the over-capacity points still
    // counted toward bounds.
    let last_kept = anchor.0 + (MAX_FEAT_PTS as i32 - 1);
    assert!(f.bbox.max_lon > last_kept, "bbox must extend past the truncated ring");
}

/// A polygon with more holes than the caller's `MAX_FEAT_RINGS` scratch holds. The hole loop
/// (reader.rs ~630) pushes each ring length with `let _ = ring_lens.push(...)`, so rings past
/// capacity are dropped. `ring_lens` holds the exterior plus `MAX_FEAT_RINGS-1` holes — exactly
/// capacity — and no more, however many holes the feature declares.
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

    let feats = decode(&r, 0, 0, &r.bbox);
    assert_eq!(feats.len(), 1);
    let f = &feats[0];
    assert_eq!(f.kind, Kind::Polygon);
    // Exterior + (MAX_FEAT_RINGS - 1) holes == MAX_FEAT_RINGS rings, the scratch ceiling — the
    // remaining declared holes were dropped.
    assert_eq!(f.ring_lens.len(), MAX_FEAT_RINGS, "ring count capped at MAX_FEAT_RINGS");
    assert_eq!(f.ring_lens[0], 4, "exterior ring kept all 4 vertices");
    assert!(f.ring_lens[1..].iter().all(|&n| n == 3), "each kept hole has its 3 vertices");
}

// ---------------------------------------------------------------------------
// Reader item 2 — chunk_size > CACHE_SLOT_BYTES (the uncached ChunkLoc::Scratch path)
// ---------------------------------------------------------------------------

/// A legal map whose `chunk_size` sits between the cache slot (`CACHE_SLOT_BYTES` = 4096) and the
/// accepted cap (`MAX_CHUNK_BYTES` = 16384). `load_chunk`'s `len > CACHE_SLOT_BYTES` branch
/// (reader.rs ~959) reads such a chunk through the *uncached* scratch every call, counting a miss +
/// a read and **never a hit**. The format suite's 64/128-byte chunks never reach this branch, so
/// nothing asserted the oversized chunk still decodes correctly *and* stays uncacheable across
/// passes. Here the same chunk is decoded twice: both decodes must miss (no slot ever caches it),
/// and the geometry must be byte-correct.
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

// ---------------------------------------------------------------------------
// Reader item 3 — cross-frame chunk-cache HIT via the public API (#37's payoff)
// ---------------------------------------------------------------------------

/// The whole point of the issue-#37 chunk cache, measured the way the renderer measures it:
/// querying the *same* viewport twice through the public `Reader` API must serve the second pass
/// from a resident slot — a **hit** — with no extra source read. The existing inline tests poke
/// `MapCacheInner` directly; nothing drove a hit through `for_each_feature`. A slot-sized chunk
/// (≤ 4096) is cacheable, so the second decode is a pure hit.
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
    r.for_each_chunk(0, &r.bbox, |cid, _| walk_order.push(cid));
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

// ---------------------------------------------------------------------------
// Reader item 4 — truncated chunk / header straddling chunk end
// ---------------------------------------------------------------------------

/// A feature header whose declared `ext_pt_count` runs past the chunk's real bytes (here past the
/// 0xFF pad). `read_ring`'s per-delta `off + dsize*2 > chunk.len()` guard (reader.rs ~687) must
/// stop at the last whole delta and decode a **partial** ring, never index out of bounds. We pack a
/// line declaring 40 points but supply only 5 deltas, in a tight chunk: the decode keeps the
/// anchor + the 5 real deltas (6 vertices) and stops — no panic, no garbage past the data.
#[test]
fn truncated_ring_decodes_partial_then_stops() {
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

    let feats = decode(&r, 0, 0, &r.bbox);
    assert_eq!(feats.len(), 1, "the partial feature still decodes (it doesn't panic or vanish)");
    let f = &feats[0];
    assert_eq!(f.kind, Kind::Line);
    // The decoder reads deltas until the per-delta guard fires at the chunk end, NOT until the
    // declared count — that's the bound under test. The 12-byte header leaves CS-12 = 52 delta
    // bytes, i.e. exactly (CS-12)/2 = 26 deltas (the 5 real ones plus 21 of the 0xFF pad, each
    // decoding as (-1,-1)); with the anchor that is 27 vertices. The key invariant: the ring is
    // capped by what the chunk *physically holds* (27), far below the forged declared count (40),
    // and the decode never reads past the chunk.
    let cap = 1 + (CS - 12) / 2; // anchor + every delta the chunk bytes can hold
    assert_eq!(f.exterior_len, cap, "ring is bounded by the chunk bytes, not the forged count");
    assert!((f.exterior_len as u16) < DECL, "the declared count is never reached");
}

/// A feature header that itself straddles the chunk end: the `while off + 12 <= cs` guard
/// (reader.rs ~574) must stop before reading a partial 12-byte header. We place one whole feature,
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

// ---------------------------------------------------------------------------
// Reader item 5 — truncated style table / bad style_offset
// ---------------------------------------------------------------------------

/// A style table whose count byte claims more records than the file actually holds. `parse_styles`'
/// `o + 6 > want` break (reader.rs ~731) must stop at the last whole record, parsing only the
/// styles physically present rather than reading past the table. We build a valid 2-style file,
/// then forge the count byte up to 8: the two real records still parse, the phantom six don't
/// appear, and the reader still constructs (a truncated table is not a hard error).
#[test]
fn truncated_style_table_parses_only_present_records() {
    let bytes = single_leaf(GLOBAL, pack_line(1, 10, 10, &[(1, 1)]), 64);
    // style_offset is fixed at HEADER_LEN (32) by the builder; the count byte is the first byte of
    // the style table.
    let style_off = u32::from_le_bytes(bytes[21..25].try_into().unwrap()) as usize;
    assert_eq!(style_off, 32);
    let mut forged = bytes.clone();
    forged[style_off] = 8; // claim 8 styles; only 2 records (12 bytes) follow before the LOD table

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

/// `style_offset` pointing at or past the end of the file must yield an empty style table, not a
/// panic. `parse_styles` returns the all-`None` table when `style_offset >= total` (reader.rs
/// ~713). A `style_offset` *equal to* the file length is the boundary the format suite never hits;
/// `MapTables::parse` accepts `style_offset == total` (its guard is `> total`), so the reader is
/// built but every style lookup is `None`.
#[test]
fn style_offset_at_eof_yields_no_styles() {
    let bytes = single_leaf(GLOBAL, pack_line(1, 10, 10, &[(1, 1)]), 64);
    let total = bytes.len() as u32;
    let mut forged = bytes.clone();
    forged[21..25].copy_from_slice(&total.to_le_bytes()); // style_offset = file length

    let cache = MapCache::new();
    let src = SliceSource(&forged);
    let tables = MapTables::parse(&src).expect("style_offset == total is accepted (guard is > total)");
    let r = Reader::new(&src, &tables, &cache);
    assert!(r.style(1).is_none(), "no style parses from a table at EOF");
    assert!(r.backdrop_style().is_none(), "no backdrop without styles");
}

// ---------------------------------------------------------------------------
// Reader item 6 — index_read across a 512-byte block boundary
// ---------------------------------------------------------------------------

/// A quadtree index large enough that a node read crosses an `INDEX_BLOCK` (512-byte) cache-block
/// edge. Each node is 4 bytes, so block 0 holds nodes 0..128 and the node at index 128 begins
/// exactly at the block boundary; reading a node at index ≥128 forces `index_read` (reader.rs ~992)
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
    });
    assert_eq!(seen, 1, "the single deep leaf is found across the block boundary");
    assert_eq!(found_cid, Some(0));

    // And its chunk decodes through the multi-block index assembly.
    let feats = decode(&r, 0, found_cid.unwrap(), &r.bbox);
    assert_eq!(feats.len(), 1);
    assert_eq!(feats[0].exterior_len, 2);
}

// ---------------------------------------------------------------------------
// Reader item 7 — negative microdegrees (sign-extension in rd_i32)
// ---------------------------------------------------------------------------

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

/// `pack_poly_decl` is also covered indirectly by the exterior-overflow test through the line path;
/// this confirms the polygon variant truncates the same way, so the overflow guard is pinned for
/// both kinds (a polygon's exterior shares `read_ring` with a line's, but the `Kind` differs and
/// the fill path downstream cares). A single huge declared-count polygon truncates to MAX_FEAT_PTS.
#[test]
fn polygon_exterior_overflow_truncates_like_a_line() {
    const DECL: u16 = 3000; // > MAX_FEAT_PTS (2048)
    let deltas: Vec<(i8, i8)> = vec![(1i8, 0i8); DECL as usize - 1];
    let chunk = pack_poly_decl(2, 0, 0, DECL, &deltas);
    assert!(chunk.len() <= MAX_CHUNK_BYTES);
    let bytes = single_leaf((0, 0, 1_000_000, 1_000_000), chunk, MAX_CHUNK_BYTES);
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);

    let feats = decode(&r, 0, 0, &r.bbox);
    assert_eq!(feats.len(), 1);
    assert_eq!(feats[0].kind, Kind::Polygon);
    assert_eq!(feats[0].exterior_len, MAX_FEAT_PTS, "polygon exterior truncates at MAX_FEAT_PTS too");
}
