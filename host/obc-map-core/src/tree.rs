//! Resident quadtree index framing shared by byte producers.

use obc_formats::obcm::{BRANCH_BIT, EMPTY_LEAF};

/// A resident quadtree whose branches have four NW/NE/SW/SE children. Geometry, POI, graph-node and
/// snap-anchor trees share this traversal contract; their leaf framing stays separate.
pub trait TreeWalk: Sized {
    fn children(&self) -> Option<&[Self; 4]>;
}

/// A quadtree whose leaf owns at most one chunk. Geometry and POI trees share this framing; graph
/// and snap trees use first-fit leaf binning instead.
pub trait FlattenTree: TreeWalk {
    /// Pack a leaf's payload into its chunk: `None` for an empty leaf, else `(chunk_bytes,
    /// dropped)` where `dropped` is the chunk-overflow count.
    fn pack_leaf(&self, chunk_size: usize) -> Option<(Vec<u8>, usize)>;
}

/// Flatten any [`FlattenTree`] into `(index_bytes, node_count, chunks, dropped)` via BFS. Child
/// order and chunk-id assignment are BFS, which fixes the byte layout: a branch's four children are
/// appended contiguously, so its first-child index is the node count at the moment it is expanded
/// (`child > idx` always, the invariant the reader's `walk_leaves` relies on).
///
/// Chunks come back one `Vec` per chunk, not concatenated, because the two consumers frame them
/// differently: POI chunks are a fixed stride and just get joined, while geometry chunks are tight
/// and need their lengths to build the offset table.
pub fn flatten_tree<N: FlattenTree>(root: &N, chunk_size: usize) -> (Vec<u8>, u32, Vec<Vec<u8>>, usize) {
    let (nodes, first_child) = obc_tree_walk::breadth_first(root, TreeWalk::children);

    let mut index: Vec<u32> = Vec::with_capacity(nodes.len());
    let mut chunks: Vec<Vec<u8>> = Vec::new();
    let mut dropped: usize = 0;
    for (idx, node) in nodes.iter().enumerate() {
        match node.children() {
            None => match node.pack_leaf(chunk_size) {
                None => index.push(EMPTY_LEAF),
                Some((chunk, chunk_dropped)) => {
                    let chunk_id = chunks.len() as u32;
                    chunks.push(chunk);
                    dropped += chunk_dropped;
                    index.push(chunk_id & !BRANCH_BIT);
                }
            },
            Some(_) => index.push(first_child[idx] as u32 | BRANCH_BIT),
        }
    }

    let mut index_bytes = Vec::with_capacity(index.len() * 4);
    for v in &index {
        index_bytes.extend_from_slice(&v.to_le_bytes());
    }
    (index_bytes, index.len() as u32, chunks, dropped)
}
