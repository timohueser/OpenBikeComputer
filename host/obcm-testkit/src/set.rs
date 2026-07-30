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
pub fn build_set(assembly: Bbox, styles: &[Style], core: usize, shards: &[ShardSpec]) -> SetFixture {
    let files: Vec<Vec<u8>> = shards.iter().map(|shard| build_file(shard.bbox, styles, &shard.lods)).collect();
    let records: Vec<Shard> = shards
        .iter()
        .zip(&files)
        .map(|(shard, bytes)| Shard { role: shard.role, bbox: set_bbox(shard.bbox), bytes: bytes.len() as u32 })
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
