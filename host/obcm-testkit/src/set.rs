//! Hand-split OBCA volume-set fixtures (`OBCA_Spec.md` §5).
//!
//! A volume set is one logical map spread over 1..32 physical OBCM files plus an OBCS manifest.
//! Tests need *matched pairs*: one monolithic `.obcm` and the byte-level split of the **same**
//! data into a set, so a differential render can assert the two are pixel-identical.
//!
//! The split is deliberately hand-made here rather than produced by a host assembler. This crate
//! is an independent oracle (it calls no production serializer), and a differential test whose
//! two sides come from the same producer proves nothing.
//!
//! # The one rule that makes a split render identically
//!
//! A feature's anchor is stored **relative to its quadtree leaf's bbox** (`OBCM_Spec.md` §5:
//! `anchor = node.min + (ax, ay)`), and a leaf's bbox is derived by subdividing the *file's*
//! header bbox. So a shard reproduces the monolith's coordinates iff its own quadtree yields the
//! same leaf bboxes. [`quadrants`] gives the split that does: a shard whose bbox is exactly one
//! quadrant of the assembly, holding that quadrant's chunk under a single-leaf index, has a root
//! node bbox equal to the monolith's corresponding child node bbox.

use obc_formats::obcs::{self, Role, SetBBox, Shard, DIGEST_LEN, NAME_LEN, SET_ID_LEN};

use crate::{build_file, LodSpec, Style};

/// A bbox as the builders take it: `(min_lon, min_lat, max_lon, max_lat)` in microdegrees.
pub type Bbox = (i32, i32, i32, i32);

/// One shard of a hand-built set: its role, its own header bbox, and the **full ladder** with the
/// LODs it does not carry written empty (§5.1).
pub struct ShardSpec {
    pub role: Role,
    pub bbox: Bbox,
    pub lods: Vec<LodSpec>,
}

/// A built set: the manifest bytes (what `MS<id>.OBS` holds) and the shard files in index order
/// (what `MS<id>S<kk>.OBM` hold).
pub struct SetFixture {
    pub manifest: Vec<u8>,
    pub shards: Vec<Vec<u8>>,
}

impl SetFixture {
    /// The shard files as `&[u8]`, in index order — the shape [`mount`](obc_reader) wants.
    pub fn sources(&self) -> Vec<&[u8]> {
        self.shards.iter().map(|bytes| bytes.as_slice()).collect()
    }
}

/// An empty ladder rung: present in the LOD table with its `max_mpp`, carrying no index nodes and
/// no chunks. This is what §5.1 means by "the LODs it does not carry written empty" and what §5.6's
/// mount-time cache keys on.
pub fn empty_lod(max_mpp: f32) -> LodSpec {
    LodSpec { max_mpp, index: vec![], chunks: vec![], chunk_size: 4096 }
}

/// The four quadrant bboxes of `bbox` in the reader's `walk_leaves` order — NW, NE, SW, SE — using
/// the same flooring midpoints the reader and the packer's `quadtree.rs` agree on.
pub fn quadrants(bbox: Bbox) -> [Bbox; 4] {
    let (min_lon, min_lat, max_lon, max_lat) = bbox;
    let mid_lon = (min_lon + max_lon).div_euclid(2);
    let mid_lat = (min_lat + max_lat).div_euclid(2);
    [
        (min_lon, mid_lat, mid_lon, max_lat),
        (mid_lon, mid_lat, max_lon, max_lat),
        (min_lon, min_lat, mid_lon, mid_lat),
        (mid_lon, min_lat, max_lon, mid_lat),
    ]
}

fn set_bbox(bbox: Bbox) -> SetBBox {
    SetBBox { min_lat: bbox.1, min_lon: bbox.0, max_lat: bbox.3, max_lon: bbox.2 }
}

/// Assemble a set from already-described shards. Every shard gets the same `styles` — §4.7 stamps
/// one skin into the whole set, and the reader's mount refuses a set whose style tables differ.
///
/// Digests are written as zeros: §5.3 lets a device defer the SHA-256 check, and nothing on the
/// read path looks at them. A host writing a real set MUST fill them in.
///
/// Member ids are `1..=N` in record order, so the fixture is a **bound** manifest (§5.2) — the shape
/// a card actually holds. Nothing here resolves through them: these shards are in-memory buffers the
/// reader takes by index. They are filled in so that a fixture is never accidentally the unbound
/// shape a mount is supposed to refuse.
pub fn build_set(assembly: Bbox, styles: &[Style], core: usize, shards: &[ShardSpec]) -> SetFixture {
    let files: Vec<Vec<u8>> = shards.iter().map(|shard| build_file(shard.bbox, styles, &shard.lods)).collect();
    let records: Vec<Shard> = shards
        .iter()
        .zip(&files)
        .enumerate()
        .map(|(index, (shard, bytes))| Shard {
            role: shard.role,
            bbox: set_bbox(shard.bbox),
            bytes: bytes.len() as u32,
            object_id: index as u64 + 1,
        })
        .collect();
    let manifest = obcs::build(
        obc_formats::obcm::VERSION,
        core as u8,
        1,
        set_bbox(assembly),
        [0u8; SET_ID_LEN],
        [0xFFu8; NAME_LEN],
        &records,
    )
    .expect("hand-built set satisfies OBCA §5.3");
    let digests = vec![[0u8; DIGEST_LEN]; records.len()];
    let mut bytes = vec![0u8; manifest.encoded_len()];
    obcs::serialize(&manifest, &digests, &mut bytes).expect("manifest serializes");
    SetFixture { manifest: bytes, shards: files }
}

/// Build a matched pair: one monolithic file and the same data hand-split into a
/// core + coarse + four-quadrant-geometry set.
///
/// `coarse` is the ladder rung every shard's bbox can serve from one file (LOD 0 — a zoomed-out
/// viewport covers the whole map, so §5.1 keeps it in a single whole-assembly shard). `fine` is
/// the rung that is split: its four chunks are the NW/NE/SW/SE quadrant leaves of the monolith and
/// become one single-leaf geometry shard each.
///
/// Returns `(monolith, set)`. The monolith's LOD 1 quadtree is `root branch → 4 quadrant leaves`,
/// which is exactly the shape whose leaf bboxes the quadrant shards reproduce.
pub fn matched_pair(
    assembly: Bbox,
    styles: &[Style],
    coarse: (f32, Vec<u8>, usize),
    fine: (f32, [Vec<u8>; 4], usize),
) -> (Vec<u8>, SetFixture) {
    let (coarse_mpp, coarse_chunk, coarse_size) = coarse;
    let (fine_mpp, fine_chunks, fine_size) = fine;

    let coarse_lod =
        |chunk: Vec<u8>| LodSpec { max_mpp: coarse_mpp, index: vec![0], chunks: vec![chunk], chunk_size: coarse_size };
    // Root branch whose children (nodes 1..4) are the NW/NE/SW/SE leaves for chunks 0..3.
    let fine_tree = |chunks: Vec<Vec<u8>>| LodSpec {
        max_mpp: fine_mpp,
        index: vec![crate::BRANCH_BIT | 1, 0, 1, 2, 3],
        chunks,
        chunk_size: fine_size,
    };
    let fine_leaf =
        |chunk: Vec<u8>| LodSpec { max_mpp: fine_mpp, index: vec![0], chunks: vec![chunk], chunk_size: fine_size };

    let monolith = build_file(assembly, styles, &[coarse_lod(coarse_chunk.clone()), fine_tree(fine_chunks.to_vec())]);

    let mut shards = vec![
        // 0 — the core: styles, marker color, nav and POIs, and no ladder LOD at all (§5.1).
        ShardSpec { role: Role::Core, bbox: assembly, lods: vec![empty_lod(coarse_mpp), empty_lod(fine_mpp)] },
        // 1 — the coarse shard: the whole assembly, LOD 0 only.
        ShardSpec { role: Role::Coarse, bbox: assembly, lods: vec![coarse_lod(coarse_chunk), empty_lod(fine_mpp)] },
    ];
    for (bbox, chunk) in quadrants(assembly).into_iter().zip(fine_chunks) {
        shards.push(ShardSpec { role: Role::Geometry, bbox, lods: vec![empty_lod(coarse_mpp), fine_leaf(chunk)] });
    }
    let set = build_set(assembly, styles, 0, &shards);
    (monolith, set)
}

/// A monolith whose fine rung is **two levels deep** in one quadrant, plus the two legal splits of
/// it — the cases [`matched_pair`] cannot express, because every shard it builds is a single-leaf
/// quadtree.
///
/// Both matter, and for different reasons:
///
/// - [`DeepPair::subdivided`] is what a real assembler produces. A shard is a node of the assembly
///   quadtree carrying *everything below it*, so a shard of any size has a quadtree of its own and
///   its leaves sit **below its own root**. That is the case where a reader indexing a shard's
///   chunk-offset table through the wrong LOD table, or descending from the wrong root bbox, stops
///   being invisible: with single-leaf shards the root *is* the leaf, so a whole class of
///   subdivision bugs cannot show up.
/// - [`DeepPair::antichain`] is §5.1's tiling rule at its actual generality: the shards of one role
///   are an **antichain** of quadtree nodes, not a uniform grid, so a dense quadrant may be split
///   further than a sparse one. Seven shards at two different depths, pairwise disjoint, union the
///   assembly.
///
/// The three files draw the same map: same chunk bytes, same leaf bboxes, so the same anchors
/// (`OBCM_Spec.md` §5 stores a feature's anchor relative to its **leaf**).
///
/// `fine` supplies seven chunks in quadtree order: `NW·NW, NW·NE, NW·SW, NW·SE, NE, SW, SE`.
pub struct DeepPair {
    /// One file: the fine rung's quadtree is `root → [NW branch → 4 leaves, NE, SW, SE]`.
    pub monolith: Vec<u8>,
    /// Four geometry shards, one per quadrant — the NW shard subdividing below its own root.
    pub subdivided: SetFixture,
    /// Seven geometry shards at mixed depth: NW's four sub-quadrants plus the three siblings.
    pub antichain: SetFixture,
}

pub fn deep_matched_pair(
    assembly: Bbox,
    styles: &[Style],
    coarse: (f32, Vec<u8>, usize),
    fine: (f32, [Vec<u8>; 7], usize),
) -> DeepPair {
    let (coarse_mpp, coarse_chunk, coarse_size) = coarse;
    let (fine_mpp, fine_chunks, fine_size) = fine;
    let quads = quadrants(assembly);
    let sub = quadrants(quads[0]);

    let coarse_lod =
        |chunk: Vec<u8>| LodSpec { max_mpp: coarse_mpp, index: vec![0], chunks: vec![chunk], chunk_size: coarse_size };
    let fine_lod =
        |index: Vec<u32>, chunks: Vec<Vec<u8>>| LodSpec { max_mpp: fine_mpp, index, chunks, chunk_size: fine_size };
    let leaf = |chunk: &Vec<u8>| fine_lod(vec![0], vec![chunk.clone()]);

    // BFS: 0 = root branch (children 1..5), 1 = NW branch (children 5..9), 2..5 = NE/SW/SE leaves
    // carrying chunks 0..2, 5..9 = NW's four leaves carrying chunks 3..6. The chunk vector is in
    // chunk-id order, which is why the ids below are not the caller's argument order.
    let monolith_chunks = vec![
        fine_chunks[4].clone(), // 0 — NE
        fine_chunks[5].clone(), // 1 — SW
        fine_chunks[6].clone(), // 2 — SE
        fine_chunks[0].clone(), // 3 — NW·NW
        fine_chunks[1].clone(), // 4 — NW·NE
        fine_chunks[2].clone(), // 5 — NW·SW
        fine_chunks[3].clone(), // 6 — NW·SE
    ];
    let monolith_index = vec![crate::BRANCH_BIT | 1, crate::BRANCH_BIT | 5, 0, 1, 2, 3, 4, 5, 6];
    let monolith =
        build_file(assembly, styles, &[coarse_lod(coarse_chunk.clone()), fine_lod(monolith_index, monolith_chunks)]);

    // The two role shards every split shares: a core with no ladder at all, and one coarse shard
    // spanning the assembly (§5.1).
    let common = |shards: &mut Vec<ShardSpec>| {
        shards.push(ShardSpec {
            role: Role::Core,
            bbox: assembly,
            lods: vec![empty_lod(coarse_mpp), empty_lod(fine_mpp)],
        });
        shards.push(ShardSpec {
            role: Role::Coarse,
            bbox: assembly,
            lods: vec![coarse_lod(coarse_chunk.clone()), empty_lod(fine_mpp)],
        });
    };

    // Split A — one shard per quadrant. The NW shard's own quadtree is `root branch → 4 leaves`
    // over the NW square, which reproduces the monolith's depth-2 leaf bboxes exactly.
    let mut subdivided = Vec::new();
    common(&mut subdivided);
    subdivided.push(ShardSpec {
        role: Role::Geometry,
        bbox: quads[0],
        lods: vec![
            empty_lod(coarse_mpp),
            fine_lod(
                vec![crate::BRANCH_BIT | 1, 0, 1, 2, 3],
                vec![fine_chunks[0].clone(), fine_chunks[1].clone(), fine_chunks[2].clone(), fine_chunks[3].clone()],
            ),
        ],
    });
    for (bbox, chunk) in [quads[1], quads[2], quads[3]].into_iter().zip(&fine_chunks[4..]) {
        subdivided.push(ShardSpec { role: Role::Geometry, bbox, lods: vec![empty_lod(coarse_mpp), leaf(chunk)] });
    }

    // Split B — the mixed-depth antichain: NW's four sub-quadrants, then its three siblings.
    let mut antichain = Vec::new();
    common(&mut antichain);
    for (bbox, chunk) in sub.into_iter().zip(&fine_chunks[..4]) {
        antichain.push(ShardSpec { role: Role::Geometry, bbox, lods: vec![empty_lod(coarse_mpp), leaf(chunk)] });
    }
    for (bbox, chunk) in [quads[1], quads[2], quads[3]].into_iter().zip(&fine_chunks[4..]) {
        antichain.push(ShardSpec { role: Role::Geometry, bbox, lods: vec![empty_lod(coarse_mpp), leaf(chunk)] });
    }

    DeepPair {
        monolith,
        subdivided: build_set(assembly, styles, 0, &subdivided),
        antichain: build_set(assembly, styles, 0, &antichain),
    }
}
