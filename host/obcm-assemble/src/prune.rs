//! §4.6.4 island pruning, **hierarchically** — the pass that decides which components of the merged
//! graph reach the map, without ever holding a whole-map union-find (#1116 D3).
//!
//! The flat formulation is three arrays over the node set — `parent`, `roots`, and a `keep` bit —
//! plus a pass over every edge. At DACH scale that is gigabytes of random access, and it is the last
//! thing in the merge that genuinely looks like it needs the whole graph at once.
//!
//! It does not, and the reason is the same seam fact the rest of phase D is built on
//! (`OBCA_Spec.md` §4.6.2): **two cells can only be connected through a seam node**. A collection id
//! that is not a seam id is named by exactly one cell — the one that minted it, and the one that
//! collected every edge incident to it. So connectivity decomposes:
//!
//! 1. **Per cell**, a union-find over that cell's *own* minted nodes plus one entry per seam another
//!    cell minted, sized by the cell and thrown away when the cell ends. It emits three things: each
//!    own node's local component, each surviving edge's local component, and one **incidence**
//!    `(component, seam slot)` per seam the cell touches.
//! 2. **Globally**, a union-find over *components and seams only*. At a state-sized bake that is
//!    tens of thousands of entries against three million nodes, and it is the only structure that
//!    spans the map.
//!
//! The per-node and per-edge component labels are streamed to [scratch](crate::scratch) — the node
//! labels in collection-id order, so the renumber reads them in lockstep with the node stream, and
//! the edge labels in collection order, so the join pass does the same with the edge stream.
//!
//! # Why this is the same kept set as the flat pass
//!
//! The two agree component for component:
//!
//! * *Nothing is split.* Every edge is unioned. An edge with both endpoints inside one cell is
//!   unioned in that cell's pass; an edge touching a seam is unioned into that cell's seam entry,
//!   and the incidence carries the union to the global pass, where the seam's other cells meet it.
//!   A seam node is minted by exactly one cell, which always emits an incidence for it — even when
//!   it has no edges there — so no seam is ever an orphan.
//! * *Nothing is fused.* A union only ever happens between two endpoints of an actual edge (per
//!   cell) or between a component and a seam it genuinely contains (globally). Two nodes end up in
//!   one component only if a chain of edges joins them, which is the definition.
//! * *The counts are the same sums.* Every collection id is counted exactly once, in the cell that
//!   minted it (a seam id included — its seam entry counts nothing, because its minting cell already
//!   did). Every surviving edge is counted once, in the cell that collected it, against the
//!   component of its endpoint `a` — which is what the flat pass counts too.
//!
//! # The one deliberate difference: the tie-break
//!
//! §4.6.4 keeps "the largest component" plus everything at or above the threshold, and *largest*
//! needs a tie-break when two components have the same node count **and** the same edge count. The
//! flat pass broke it with the smallest union-find **root id**, which is a node of the component but
//! an arbitrary one: which node ends up as the root is a property of the order the unions happened
//! in, not of the graph. This pass breaks it with the component's **smallest collection id**, which
//! is a property of the component itself.
//!
//! The two can only ever disagree about a component that ties another on both counts *and* is below
//! the threshold — anything at or above it is kept regardless of which one is called largest. Both
//! published regions and the assembler's oracle land on the same bytes either way; a synthetic pair
//! of tied components pins the new rule, because nothing else can reach it.

use std::collections::BTreeMap;

use crate::extsort::{SpillReader, SpillWriter};
use crate::nav::{edge_a, edge_b, edge_cell, NavStats, EDGE_REC};
use crate::scratch::{ScratchId, ScratchStore};
use crate::{Error, Result};

/// One `u32` label per record — what both streams this module writes are made of.
const LABEL: usize = 4;

/// What the prune leaves behind: two label streams and the verdict per label.
///
/// Neither stream is a whole-map array in memory — they are scratch files read once, forward, by the
/// passes that need them. `keep` is indexed by the label both streams carry, and there is one label
/// per *component of a cell*, which is thousands of entries at country scale.
#[derive(Debug)]
pub struct Pruned {
    /// One `u32` component label per collection id, in id order. The renumber reads it beside the
    /// node stream; a node is kept exactly when `keep[label]`.
    pub node_comp: ScratchId,
    /// One `u32` component label per **surviving** edge (post-§4.6.3), in collection order. The join
    /// pass reads it beside the edge stream, skipping the same duplicates the dedup killed.
    pub edge_comp: ScratchId,
    /// Whether each label's component reaches the map.
    pub keep: Vec<bool>,
}

/// A union-find with path halving. `union` points the first root at the second, exactly as the flat
/// pass did — with a canonical representative (see the module header) it no longer matters which,
/// but there is no reason to differ.
struct Uf {
    parent: Vec<u32>,
}

impl Uf {
    fn new(n: usize) -> Uf {
        Uf { parent: (0..n as u32).collect() }
    }

    /// Add one more singleton entry and return its index — how a cell's pass grows an entry for a
    /// seam it did not mint the first time an edge names it.
    fn push(&mut self) -> u32 {
        let x = self.parent.len() as u32;
        self.parent.push(x);
        x
    }

    fn len(&self) -> usize {
        self.parent.len()
    }

    fn find(&mut self, mut x: u32) -> u32 {
        while self.parent[x as usize] != x {
            self.parent[x as usize] = self.parent[self.parent[x as usize] as usize];
            x = self.parent[x as usize];
        }
        x
    }

    fn union(&mut self, a: u32, b: u32) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent[ra as usize] = rb;
        }
    }
}

/// The per-cell accumulators, in one place because six parallel `Vec`s at four call sites is how a
/// label ends up counted against the wrong component.
#[derive(Default)]
struct Components {
    /// Own nodes in this label's component.
    nodes: Vec<u32>,
    /// Surviving edges booked against it.
    edges: Vec<u32>,
    /// The smallest collection id it holds, or `u32::MAX` for a label with no own nodes (a component
    /// made only of seams another cell minted — it contributes connectivity and edges, and its
    /// nodes are counted where they were minted).
    min_id: Vec<u32>,
}

impl Components {
    /// This cell-local root's label, minting one if the root has not been seen yet. `seen` is the
    /// cell's root → label table, `u32::MAX` where there is none.
    fn label(&mut self, seen: &mut [u32], uf: &mut Uf, x: u32) -> u32 {
        let root = uf.find(x) as usize;
        if seen[root] == u32::MAX {
            seen[root] = u32::try_from(self.nodes.len()).expect("a component count fits a u32");
            self.nodes.push(0);
            self.edges.push(0);
            self.min_id.push(u32::MAX);
        }
        seen[root]
    }
}

/// Which union-find entry a collection id is, inside cell `[base, base + n)`'s pass.
///
/// A seam node another cell minted gets an entry of its own, created on first sight; everything else
/// — including a seam this cell minted, which *is* one of its own nodes — is the own entry its id
/// names directly.
fn localize(uf: &mut Uf, foreign: &mut BTreeMap<u32, u32>, seam_id: &[u32], base: u32, n: u32, id: u32) -> Result<u32> {
    if let Ok(slot) = seam_id.binary_search(&id) {
        if id < base || id >= base + n {
            return Ok(*foreign.entry(slot as u32).or_insert_with(|| uf.push()));
        }
    }
    if id < base || id >= base + n {
        // A non-seam id another cell minted would mean an edge crossed a cell boundary somewhere
        // other than §4.6.2's unification, which is the one thing §4.6 forbids outright.
        return Err(Error::Format(format!(
            "a merged edge names node {id}, which is neither this cell's ({base}..{}) nor a seam — no single cell \
             joined it (OBCA §4.6)",
            base + n
        )));
    }
    Ok(id - base)
}

/// §4.6.4 over the whole merged graph, one cell at a time. See the module header.
///
/// `edges` is the collection-order edge stream, `dead` the §4.6.3 duplicates' collection indices
/// (ascending), `cell_base` the first collection id of each cell followed by the total, and
/// `seam_id` the seam table's ids — ascending, because seams are minted in collection order, which
/// is what makes the membership test a binary search rather than a map.
#[allow(clippy::too_many_arguments)]
pub fn prune(
    scratch: &dyn ScratchStore,
    share: usize,
    edges: (ScratchId, u64),
    dead: &[u32],
    cell_base: &[u32],
    seam_id: &[u32],
    id_count: u32,
    min_component_edges: usize,
    stats: &mut NavStats,
) -> Result<Pruned> {
    debug_assert!(seam_id.windows(2).all(|w| w[0] < w[1]), "seam ids are minted in ascending collection order");
    let mut node_comp = SpillWriter::<LABEL>::create(scratch, share)?;
    let mut edge_comp = SpillWriter::<LABEL>::create(scratch, share)?;
    let mut comps = Components::default();
    /// `(label, seam slot)` — the only thing the global pass needs from a cell.
    type Incidence = (u32, u32);
    let mut incidences: Vec<Incidence> = Vec::new();

    let (edges, edge_total) = edges;
    let mut src = SpillReader::<EDGE_REC>::open(scratch, edges, share)?;
    let mut ahead = src.next().transpose()?;
    let (mut index, mut next_dead) = (0u32, 0usize);

    for ci in 0..cell_base.len() - 1 {
        let (base, n) = (cell_base[ci], cell_base[ci + 1] - cell_base[ci]);
        let mut uf = Uf::new(n as usize);
        let mut foreign: BTreeMap<u32, u32> = BTreeMap::new(); // seam slot → its entry, ordered so
        let mut edge_at: Vec<u32> = Vec::new(); // …the incidences below are emitted deterministically

        // The stream is in collection order and every cell's edges are contiguous in it, so a cell's
        // run ends the moment a record names a different one. A cell that collected nothing is an
        // empty run, which this skips without a special case.
        while let Some(rec) = ahead {
            if edge_cell(&rec) != ci as u32 {
                break;
            }
            let at = index;
            index += 1;
            ahead = src.next().transpose()?;
            // The duplicates §4.6.3 killed are not in the graph at all: they neither connect (they
            // are parallel to their survivor, so they could not) nor count towards the threshold.
            if next_dead < dead.len() && dead[next_dead] == at {
                next_dead += 1;
                continue;
            }
            let a = localize(&mut uf, &mut foreign, seam_id, base, n, edge_a(&rec))?;
            let b = localize(&mut uf, &mut foreign, seam_id, base, n, edge_b(&rec))?;
            uf.union(a, b);
            edge_at.push(a);
        }

        // The cell's unions are all in, so its labels are final. Own nodes first and in id order,
        // which is what makes `node_comp` addressable by collection id.
        let mut seen = vec![u32::MAX; uf.len()];
        for i in 0..n {
            let label = comps.label(&mut seen, &mut uf, i);
            comps.nodes[label as usize] += 1;
            if comps.min_id[label as usize] == u32::MAX {
                comps.min_id[label as usize] = base + i; // ascending `i`, so the first is the least
            }
            node_comp.push(label.to_le_bytes())?;
        }
        // …then the edges, in the order they were collected, which is the order the join pass and
        // the emission walk read them back in.
        for &x in &edge_at {
            let label = comps.label(&mut seen, &mut uf, x);
            comps.edges[label as usize] += 1;
            edge_comp.push(label.to_le_bytes())?;
        }
        // Every seam this cell minted, whether or not an edge here touches it: another cell may
        // reach it, and an incidence is the only way that union can happen.
        let lo = seam_id.partition_point(|&s| s < base);
        let hi = seam_id.partition_point(|&s| s < base + n);
        for (slot, &id) in seam_id.iter().enumerate().take(hi).skip(lo) {
            let label = comps.label(&mut seen, &mut uf, id - base);
            incidences.push((label, slot as u32));
        }
        // …and every seam it only borrowed.
        for (&slot, &x) in &foreign {
            let label = comps.label(&mut seen, &mut uf, x);
            incidences.push((label, slot));
        }
    }
    if ahead.is_some() {
        return Err(Error::Format("the merged edge stream names a cell past the end of the cell list".into()));
    }
    let (node_comp, node_labels) = node_comp.seal()?;
    let (edge_comp, edge_labels) = edge_comp.seal()?;
    debug_assert_eq!(node_labels, id_count as u64, "one component label per collection id");
    debug_assert_eq!(edge_labels + dead.len() as u64, index as u64, "one label per surviving edge");
    // The cell-grouped walk above reached every record, which is the whole premise of reading the
    // stream as a sequence of per-cell runs.
    debug_assert_eq!(index as u64, edge_total, "the per-cell runs do not cover the edge stream");

    // --- The global pass: components and seams, nothing else. ---
    let n_comp = comps.nodes.len();
    let mut global = Uf::new(n_comp + seam_id.len());
    for &(label, slot) in &incidences {
        global.union(label, n_comp as u32 + slot);
    }
    drop(incidences);
    let comp_root: Vec<u32> = (0..n_comp as u32).map(|c| global.find(c)).collect();
    drop(global);

    // Aggregate each component's totals onto its global root. A seam entry contributes nothing of
    // its own: its node was already counted by the cell that minted it.
    let mut nodes_per = vec![0u32; n_comp + seam_id.len()];
    let mut edges_per = vec![0u32; n_comp + seam_id.len()];
    let mut min_per = vec![u32::MAX; n_comp + seam_id.len()];
    let mut seen = vec![false; n_comp + seam_id.len()];
    let mut roots: Vec<u32> = Vec::new();
    for (c, &r) in comp_root.iter().enumerate() {
        let r = r as usize;
        if !seen[r] {
            seen[r] = true;
            roots.push(r as u32);
        }
        nodes_per[r] += comps.nodes[c];
        edges_per[r] += comps.edges[c];
        min_per[r] = min_per[r].min(comps.min_id[c]);
    }
    drop(seen);

    // The key is total — distinct components hold disjoint node sets, so their smallest ids differ —
    // which is why the answer does not depend on the order `roots` came out in.
    let largest = *roots
        .iter()
        .max_by_key(|&&r| (nodes_per[r as usize], edges_per[r as usize], std::cmp::Reverse(min_per[r as usize])))
        .expect("a non-empty graph has a component");
    let keep_root = |r: u32| r == largest || edges_per[r as usize] as usize >= min_component_edges;

    stats.components_found = roots.len();
    stats.components_kept = roots.iter().filter(|&&r| keep_root(r)).count();
    stats.largest_component_permille = (nodes_per[largest as usize] as u64 * 1000 / id_count.max(1) as u64) as u32;
    stats.pruned_nodes = roots.iter().filter(|&&r| !keep_root(r)).map(|&r| nodes_per[r as usize] as usize).sum();
    stats.pruned_edges = roots.iter().filter(|&&r| !keep_root(r)).map(|&r| edges_per[r as usize] as usize).sum();

    let keep: Vec<bool> = comp_root.iter().map(|&r| keep_root(r)).collect();
    Ok(Pruned { node_comp, edge_comp, keep })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::MemoryScratch;

    /// One [`EDGE_REC`] record with only the three fields this module reads set — checked against
    /// [`crate::nav`]'s own accessors, so the two cannot drift apart silently.
    fn edge(a: u32, b: u32, cell: u32) -> [u8; EDGE_REC] {
        let mut r = [0u8; EDGE_REC];
        r[0..4].copy_from_slice(&a.to_le_bytes());
        r[4..8].copy_from_slice(&b.to_le_bytes());
        r[20..24].copy_from_slice(&cell.to_le_bytes());
        assert_eq!((edge_a(&r), edge_b(&r), edge_cell(&r)), (a, b, cell), "the record layout moved");
        r
    }

    /// A whole collection, as the passes before §4.6.4 would have left it.
    struct Fixture {
        /// Each cell's first minted id, with the total appended.
        cell_base: Vec<u32>,
        /// Slot → the id its first cell minted, ascending.
        seam_id: Vec<u32>,
        /// `(a, b, cell)`, grouped by cell exactly as the collection order is.
        edges: Vec<(u32, u32, u32)>,
        /// Collection indices §4.6.3 killed, ascending.
        dead: Vec<u32>,
    }

    impl Fixture {
        fn id_count(&self) -> u32 {
            *self.cell_base.last().expect("a cell list ends with the total")
        }

        fn run(&self, min_component_edges: usize) -> (NavStats, Vec<bool>, Vec<bool>) {
            let scratch = MemoryScratch::new();
            let mut out = SpillWriter::<EDGE_REC>::create(&scratch, 64).expect("a stream");
            for &(a, b, c) in &self.edges {
                out.push(edge(a, b, c)).expect("push");
            }
            let (edges, total) = out.seal().expect("seal");
            let mut stats = NavStats::default();
            let pruned = prune(
                &scratch,
                64,
                (edges, total),
                &self.dead,
                &self.cell_base,
                &self.seam_id,
                self.id_count(),
                min_component_edges,
                &mut stats,
            )
            .expect("the prune succeeds");
            let read = |id: ScratchId| -> Vec<bool> {
                SpillReader::<LABEL>::open(&scratch, id, 64)
                    .expect("open")
                    .map(|r| pruned.keep[u32::from_le_bytes(r.expect("a label")) as usize])
                    .collect()
            };
            let (nodes, edges_kept) = (read(pruned.node_comp), read(pruned.edge_comp));
            (stats, nodes, edges_kept)
        }

        /// The flat union-find §4.6.4 was written as, with the canonical tie-break (module header).
        fn oracle(&self, min_component_edges: usize) -> (NavStats, Vec<bool>, Vec<bool>) {
            let n = self.id_count() as usize;
            let mut uf = Uf::new(n);
            let live: Vec<(u32, u32)> = self
                .edges
                .iter()
                .enumerate()
                .filter(|(i, _)| !self.dead.contains(&(*i as u32)))
                .map(|(_, &(a, b, _))| (a, b))
                .collect();
            for &(a, b) in &live {
                uf.union(a, b);
            }
            let roots: Vec<u32> = (0..n as u32).map(|i| uf.find(i)).collect();
            let mut nodes_per: BTreeMap<u32, usize> = BTreeMap::new();
            let mut min_per: BTreeMap<u32, u32> = BTreeMap::new();
            for (i, &r) in roots.iter().enumerate() {
                *nodes_per.entry(r).or_insert(0) += 1;
                let slot = min_per.entry(r).or_insert(u32::MAX);
                *slot = (*slot).min(i as u32);
            }
            let mut edges_per: BTreeMap<u32, usize> = BTreeMap::new();
            for &(a, _) in &live {
                *edges_per.entry(roots[a as usize]).or_insert(0) += 1;
            }
            let largest = *nodes_per
                .iter()
                .max_by_key(|(r, n)| (**n, edges_per.get(r).copied().unwrap_or(0), std::cmp::Reverse(min_per[r])))
                .expect("a component")
                .0;
            let keep_root = |r: u32| r == largest || edges_per.get(&r).copied().unwrap_or(0) >= min_component_edges;
            let stats = NavStats {
                components_found: nodes_per.len(),
                components_kept: nodes_per.keys().filter(|r| keep_root(**r)).count(),
                largest_component_permille: (nodes_per[&largest] as u64 * 1000 / n.max(1) as u64) as u32,
                pruned_nodes: roots.iter().filter(|&&r| !keep_root(r)).count(),
                pruned_edges: live.iter().filter(|&&(a, _)| !keep_root(roots[a as usize])).count(),
                ..NavStats::default()
            };
            let keep_node: Vec<bool> = roots.iter().map(|&r| keep_root(r)).collect();
            let keep_edge: Vec<bool> = live.iter().map(|&(a, _)| keep_root(roots[a as usize])).collect();
            (stats, keep_node, keep_edge)
        }
    }

    /// A deterministic 32-bit xorshift, so the fixtures are the same on every machine.
    struct Rng(u32);

    impl Rng {
        fn next(&mut self, n: u32) -> u32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 17;
            self.0 ^= self.0 << 5;
            self.0 % n.max(1)
        }
    }

    /// `cells` cells of `per` nodes each, every `stride`-th id a seam, and `density` edges per cell
    /// drawn from that cell's own nodes and the seams minted before it — which is exactly the shape
    /// §4.6.1/§4.6.2 leave behind.
    fn fixture(seed: u32, cells: usize, per: u32, stride: u32, density: u32) -> Fixture {
        let mut rng = Rng(seed | 1);
        let cell_base: Vec<u32> = (0..=cells as u32).map(|c| c * per).collect();
        let seam_id: Vec<u32> = (0..cells as u32 * per).filter(|i| i % stride == 0).collect();
        let mut edges = Vec::new();
        for c in 0..cells as u32 {
            let (base, end) = (cell_base[c as usize], cell_base[c as usize + 1]);
            let reachable: Vec<u32> = (base..end).chain(seam_id.iter().copied().filter(|&s| s < base)).collect();
            for _ in 0..density {
                let a = reachable[rng.next(reachable.len() as u32) as usize];
                let b = reachable[rng.next(reachable.len() as u32) as usize];
                edges.push((a, b, c));
            }
        }
        Fixture { cell_base, seam_id, edges, dead: Vec::new() }
    }

    /// **The equivalence.** Per-cell union-finds plus a global one over components-and-seams decide
    /// exactly what one union-find over every node decides — the same components, the same counts,
    /// the same kept set — over a sweep of shapes: sparse graphs that fragment into many islands,
    /// dense ones that are a single component, and thresholds from "keep everything" to "keep only
    /// the largest".
    #[test]
    fn the_hierarchical_prune_is_the_flat_prune() {
        for seed in 1..12u32 {
            for &(cells, per, stride, density) in
                &[(4usize, 8u32, 3u32, 4u32), (5, 12, 5, 20), (3, 20, 7, 3), (6, 6, 2, 10)]
            {
                let f = fixture(seed, cells, per, stride, density);
                for threshold in [0usize, 1, 3, 8, 1_000] {
                    let (got, keep_node, keep_edge) = f.run(threshold);
                    let (want, want_node, want_edge) = f.oracle(threshold);
                    let what = format!("seed {seed}, {cells}×{per} stride {stride} density {density}, ≥{threshold}");
                    assert_eq!(got, want, "{what}: the reported components differ");
                    assert_eq!(keep_node, want_node, "{what}: a different set of nodes survives");
                    assert_eq!(keep_edge, want_edge, "{what}: a different set of edges survives");
                }
            }
        }
    }

    /// …and the same holds once §4.6.3 has taken copies out of the stream, because a duplicate is
    /// parallel to its survivor: it cannot connect anything new, but it would inflate the count the
    /// threshold is read against, so it must be gone from *both* sums.
    #[test]
    fn the_duplicates_the_dedup_killed_count_for_nothing() {
        for seed in 1..8u32 {
            let mut f = fixture(seed, 4, 8, 3, 6);
            // Duplicate every third edge in place, so the copy is collected right after the original
            // and inside the same cell's run.
            let mut doubled = Vec::new();
            let mut dead = Vec::new();
            for (i, &e) in f.edges.iter().enumerate() {
                doubled.push(e);
                if i % 3 == 0 {
                    dead.push(doubled.len() as u32);
                    doubled.push(e);
                }
            }
            f.edges = doubled;
            f.dead = dead;
            for threshold in [0usize, 2, 5, 1_000] {
                let (got, keep_node, keep_edge) = f.run(threshold);
                let (want, want_node, want_edge) = f.oracle(threshold);
                assert_eq!(got, want, "seed {seed}, ≥{threshold}");
                assert_eq!(keep_node, want_node, "seed {seed}, ≥{threshold}");
                assert_eq!(keep_edge, want_edge, "seed {seed}, ≥{threshold}");
            }
        }
    }

    /// **The tie-break.** Two components with the same node count and the same edge count, both
    /// below the threshold: exactly one is kept, and it is the one holding the smallest collection
    /// id (module header). Nothing in a real bake reaches this, which is why it is built by hand.
    #[test]
    fn the_largest_of_two_tied_components_is_the_one_with_the_smallest_id() {
        // Two cells, four nodes each, one path per cell — identical shapes, so the node and edge
        // counts tie and only the ids can separate them.
        let f = Fixture {
            cell_base: vec![0, 4, 8],
            seam_id: Vec::new(),
            edges: vec![(0, 1, 0), (1, 2, 0), (2, 3, 0), (4, 5, 1), (5, 6, 1), (6, 7, 1)],
            dead: Vec::new(),
        };
        let (stats, keep_node, keep_edge) = f.run(1_000);
        assert_eq!((stats.components_found, stats.components_kept), (2, 1), "one of the two survives");
        assert_eq!(keep_node, vec![true, true, true, true, false, false, false, false], "the low-id one");
        assert_eq!(keep_edge, vec![true, true, true, false, false, false]);
        assert_eq!((stats.pruned_nodes, stats.pruned_edges), (4, 3));
        assert_eq!(stats.largest_component_permille, 500);
        // …and the flat formulation, with the same rule, agrees.
        let (want, want_node, want_edge) = f.oracle(1_000);
        assert_eq!((stats, keep_node, keep_edge), (want, want_node, want_edge));
    }

    /// A seam node its cell never gave an edge is still a node of the graph, and another cell can
    /// still reach it — which only works because a cell emits an incidence for **every** seam it
    /// minted, not only the ones it wired up.
    #[test]
    fn a_seam_the_minting_cell_never_used_still_joins_the_component_that_reaches_it() {
        // Cell 0 mints 0..2 and wires nothing to seam id 0; cell 1 mints 2..4 and hangs both of its
        // nodes off it. One component of four, or the seam was an orphan.
        let f = Fixture {
            cell_base: vec![0, 2, 4],
            seam_id: vec![0, 1],
            edges: vec![(1, 0, 0), (2, 0, 1), (3, 1, 1), (1, 3, 1)],
            dead: Vec::new(),
        };
        let (stats, keep_node, _) = f.run(0);
        assert_eq!(stats.components_found, 1, "the seam ties the two cells together");
        assert_eq!(keep_node, vec![true; 4]);
        let (want, want_node, _) = f.oracle(0);
        assert_eq!((stats, keep_node), (want, want_node));
    }

    /// An edge naming a node no cell of its own minted is the one thing §4.6 forbids outright, so it
    /// is a refusal rather than a component quietly stitched to the wrong cell.
    #[test]
    fn an_edge_reaching_into_another_cell_is_refused() {
        let scratch = MemoryScratch::new();
        let mut out = SpillWriter::<EDGE_REC>::create(&scratch, 64).expect("a stream");
        out.push(edge(0, 5, 0)).expect("push");
        let (edges, total) = out.seal().expect("seal");
        let err = prune(&scratch, 64, (edges, total), &[], &[0, 4, 8], &[], 8, 0, &mut NavStats::default())
            .expect_err("node 5 belongs to cell 1");
        assert!(format!("{err}").contains("no single cell joined it"), "got: {err}");
    }
}
