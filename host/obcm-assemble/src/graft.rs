//! The graft: geometry, per LOD, moved from cells into an assembly by **relocation and memcpy**
//! ([`OBCA_Spec.md`](../../../specs/OBCA_Spec.md) §4.3/§4.4).
//!
//! This is the module the alignment theorem pays for. A cell's header bbox *is* its grid square, and
//! a grid-aligned assembly bbox subdivides onto that square exactly (§2), so the cell's quadtree
//! subtree is *already* the subtree the assembly would build at that position. Grafting it is:
//!
//! - **chunk payload bytes** — copied verbatim, never decoded (§2.3);
//! - **index nodes** — copied with **two constants**: leaf values `+ chunk_id_base`, branch child
//!   bases `+ (block_base − 1)`;
//! - **offset-table entries** — copied with `+ chunk_byte_base`;
//! - **index nodes above the cell depth** — freshly written, with an empty leaf wherever no cell is
//!   present (a hole is legal by construction; the renderer paints backdrop there).
//!
//! Nothing here calls a geometry library, and nothing here allocates per feature. Peak memory is one
//! cell's index block plus the fixed copy buffer, which is what makes a country assemble in a tab.

use std::collections::{HashMap, HashSet};

use obc_formats::obcm::{BRANCH_BIT, EMPTY_LEAF};

use crate::grid::{quad_children, AlignedBox, CellId, UBox};
use crate::input::Cell;
use crate::{Error, Result};

/// One cell's contribution to one LOD region, with the two relocation constants of §4.3.
#[derive(Clone, Copy, Debug)]
pub struct GraftCell {
    /// Index into the assembly's cell list.
    pub cell: usize,
    /// Node index of this cell's depth-`d` slot in the fresh upper tree.
    pub slot: usize,
    /// Where this cell's nodes `1..` land in the assembly index.
    pub block_base: u32,
    /// Added to every copied leaf's chunk id.
    pub chunk_id_base: u32,
    /// Added to every copied offset-table entry.
    pub chunk_byte_base: u64,
    /// Nodes in the cell's own index for this LOD (`0` ⇒ the level is empty in this cell).
    pub node_count: u32,
    pub chunk_count: u32,
    pub chunk_bytes: u64,
}

/// Everything needed to emit one LOD region, computed before a single byte is written — which is
/// also what makes every section offset knowable up front (no back-patching).
#[derive(Clone, Debug)]
pub struct LodPlan {
    pub lod: usize,
    pub max_mpp: Option<f64>,
    pub chunk_size: usize,
    /// The fresh tree for depths `0..d`, with each present cell's **relocated root** already inlined
    /// into its depth-`d` slot (§4.4.2).
    pub upper: Vec<u32>,
    pub cells: Vec<GraftCell>,
    pub node_count: u32,
    pub chunk_count: u32,
    pub chunk_bytes: u64,
}

impl LodPlan {
    /// An empty region: no index, no chunk, and the single-`0`-entry offset table `OBCM_Spec.md`
    /// §5.1 mandates. This is what a shard writes for a LOD its role does not carry (§5.1) and what
    /// a cell writes for an out-of-band level (§3.1).
    pub fn empty(lod: usize, max_mpp: Option<f64>, chunk_size: usize) -> LodPlan {
        LodPlan {
            lod,
            max_mpp,
            chunk_size,
            upper: Vec::new(),
            cells: Vec::new(),
            node_count: 0,
            chunk_count: 0,
            chunk_bytes: 0,
        }
    }

    /// Total bytes of the region: index + offset table + chunks.
    pub fn region_bytes(&self) -> u64 {
        if self.node_count == 0 {
            return 4; // the mandatory single-`0` offset table
        }
        self.node_count as u64 * 4 + (self.chunk_count as u64 + 1) * 4 + self.chunk_bytes
    }
}

/// Plan LOD `lod` over `box_` for the band's `cell_log2`, given the cells (indices into the
/// assembly's list) that belong to that band and lie inside the box.
///
/// `present` maps a cell's `(i, j)` to its index in the assembly's cell list. Ordering is by the
/// depth-`d` node index — the BFS order of `OBCM_Spec.md` §4 — so the output is a pure function of
/// which cells are present, never of fetch order (§4.4.1).
pub fn plan_lod(
    lod: usize,
    max_mpp: Option<f64>,
    chunk_size: usize,
    box_: AlignedBox,
    cell_log2: u32,
    present: &HashMap<(i64, i64), usize>,
    cells: &[Cell<'_>],
) -> Result<LodPlan> {
    let depth = box_.cell_depth(cell_log2);
    // The ancestor set: a node at depth k has a present descendant iff its (k, i, j) is in here.
    // Built from the cells themselves, so the walk below never scans a subtree that is all holes.
    let base = CellId::containing(cell_log2, box_.min_lat, box_.min_lon);
    let mut ancestors: HashSet<(u32, i64, i64)> = HashSet::with_capacity(present.len() * (depth as usize + 1));
    for (i, j) in present.keys() {
        let (di, dj) = (i - base.i, j - base.j);
        for k in 0..=depth {
            ancestors.insert((k, di >> (depth - k), dj >> (depth - k)));
        }
    }

    // Fresh upper tree, breadth-first: a branch wherever any descendant cell is present, an empty
    // leaf where none is (§4.4.2). Children are appended contiguously, so a branch's first-child
    // index is the node count at the moment it is expanded — the reader's `walk_leaves` invariant.
    let mut upper: Vec<u32> = Vec::new();
    let mut boxes: Vec<(UBox, u32, i64, i64)> = vec![(box_.ubox(), 0, 0, 0)];
    let mut slots: Vec<(usize, usize)> = Vec::new(); // (node index, cell index), in BFS order
    let mut n = 0usize;
    upper.push(0);
    while n < boxes.len() {
        let (b, k, di, dj) = boxes[n];
        if k == depth {
            match present.get(&(base.i + di, base.j + dj)) {
                Some(&c) => slots.push((n, c)),
                None => upper[n] = EMPTY_LEAF,
            }
            n += 1;
            continue;
        }
        if !ancestors.contains(&(k, di, dj)) {
            upper[n] = EMPTY_LEAF;
            n += 1;
            continue;
        }
        upper[n] = BRANCH_BIT | upper.len() as u32;
        for (q, child) in quad_children(b).into_iter().enumerate() {
            // NW, NE, SW, SE — north is +lat, east is +lon.
            let (ci, cj) = (di * 2 + i64::from(q < 2), dj * 2 + i64::from(q % 2 == 1));
            boxes.push((child, k + 1, ci, cj));
            upper.push(0);
        }
        n += 1;
    }

    // Relocation constants, in slot order.
    let mut plan_cells: Vec<GraftCell> = Vec::with_capacity(slots.len());
    let mut node_base = upper.len() as u64;
    let mut chunk_id_base: u64 = 0;
    let mut chunk_byte_base: u64 = 0;
    for (slot, c) in slots {
        let l = cells[c].lod(lod)?;
        if l.chunk_size > chunk_size {
            return Err(Error::Format(format!(
                "cell {}: LOD {lod} was written with chunk capacity {} but the schema says {chunk_size} (OBCA §4.4)",
                cells[c].id, l.chunk_size
            )));
        }
        let node_count = l.node_count as u32;
        let block_base = node_base;
        if node_count > 1 {
            node_base += node_count as u64 - 1;
        }
        if node_base > u32::MAX as u64 || chunk_id_base + l.chunk_count as u64 > u32::MAX as u64 {
            return Err(Error::Capacity(format!(
                "LOD {lod} exceeds the format's uint32 index space at cell {} — split the selection",
                cells[c].id
            )));
        }
        plan_cells.push(GraftCell {
            cell: c,
            slot,
            block_base: block_base as u32,
            chunk_id_base: chunk_id_base as u32,
            chunk_byte_base,
            node_count,
            chunk_count: l.chunk_count as u32,
            chunk_bytes: l.chunk_bytes_total as u64,
        });
        chunk_id_base += l.chunk_count as u64;
        chunk_byte_base += l.chunk_bytes_total as u64;
    }

    // Inline each present cell's relocated **root** into its depth-`d` slot (§4.4.2 / §7).
    for g in &plan_cells {
        let cell = &cells[g.cell];
        let l = cell.lod(lod)?;
        upper[g.slot] = if g.node_count == 0 {
            EMPTY_LEAF // a cell whose level is empty contributes an empty leaf, like an absent cell
        } else {
            let root = u32::from_le_bytes(cell.read(l.index_offset, 4)?[..4].try_into().expect("4 bytes"));
            relocate(root, g.block_base, g.chunk_id_base)
        };
    }

    let chunk_bytes = chunk_byte_base;
    let chunk_count = chunk_id_base as u32;
    Ok(LodPlan {
        lod,
        max_mpp,
        chunk_size,
        node_count: node_base as u32,
        upper,
        cells: plan_cells,
        chunk_count,
        chunk_bytes,
    })
}

/// Relocate one copied index node: a branch's child base by `block_base − 1`, a leaf's chunk id by
/// `chunk_id_base`, an empty leaf not at all (§4.3).
#[inline]
fn relocate(value: u32, block_base: u32, chunk_id_base: u32) -> u32 {
    if value & BRANCH_BIT != 0 {
        // The cell's node `k` lands at `block_base + k − 1`, so a child base relocates by the same
        // delta. The cell's root children are at `1..4`, which is why `−1` and not `+block_base`.
        BRANCH_BIT | ((value & !BRANCH_BIT) + block_base - 1)
    } else if value == EMPTY_LEAF {
        EMPTY_LEAF
    } else {
        value + chunk_id_base
    }
}

/// Emit a planned LOD region: `[index][offset table][chunks]`, in the format's own order
/// (`OBCM_Spec.md` §3). Nothing is decoded and no chunk byte is touched.
pub fn emit_lod(plan: &LodPlan, cells: &[Cell<'_>], out: &mut dyn FnMut(&[u8]) -> Result<()>) -> Result<()> {
    if plan.node_count == 0 {
        return out(&0u32.to_le_bytes()); // the mandatory single-`0` offset table of §5.1
    }

    // 1. The fresh upper tree, then each cell's relocated block (its nodes `1..`).
    let mut buf: Vec<u8> = Vec::with_capacity(plan.upper.len() * 4);
    for v in &plan.upper {
        buf.extend_from_slice(&v.to_le_bytes());
    }
    out(&buf)?;
    for g in &plan.cells {
        if g.node_count <= 1 {
            continue; // the root was inlined into the slot; there is no block
        }
        let cell = &cells[g.cell];
        let l = cell.lod(plan.lod)?;
        let raw = cell.read(l.index_offset + 4, (g.node_count as usize - 1) * 4)?;
        let mut block = Vec::with_capacity(raw.len());
        for w in raw.chunks_exact(4) {
            let v = relocate(u32::from_le_bytes([w[0], w[1], w[2], w[3]]), g.block_base, g.chunk_id_base);
            block.extend_from_slice(&v.to_le_bytes());
        }
        out(&block)?;
    }

    // 2. The offset table: `chunk_count + 1` entries, each cell's shifted by its byte base. Every
    // copied pair is re-checked against the capacity bound — a cell that violated it would poison
    // the assembly (§4.4.4).
    let mut table: Vec<u8> = Vec::with_capacity((plan.chunk_count as usize + 1) * 4);
    table.extend_from_slice(&0u32.to_le_bytes());
    for g in &plan.cells {
        if g.chunk_count == 0 {
            continue;
        }
        let cell = &cells[g.cell];
        let l = cell.lod(plan.lod)?;
        let table_start = l.index_offset + l.node_count * 4;
        let raw = cell.read(table_start, (g.chunk_count as usize + 1) * 4)?;
        let mut prev = 0u32;
        for (k, w) in raw.chunks_exact(4).enumerate() {
            let v = u32::from_le_bytes([w[0], w[1], w[2], w[3]]);
            if k == 0 {
                if v != 0 {
                    return Err(Error::Format(format!("cell {}: LOD {} offsets[0] is {v}, not 0", cell.id, plan.lod)));
                }
                continue;
            }
            if v < prev || (v - prev) as usize > plan.chunk_size {
                return Err(Error::Format(format!(
                    "cell {}: LOD {} chunk {} spans {} bytes (capacity {}) or runs backwards",
                    cell.id,
                    plan.lod,
                    k - 1,
                    v.saturating_sub(prev),
                    plan.chunk_size
                )));
            }
            prev = v;
            table.extend_from_slice(&((g.chunk_byte_base + v as u64) as u32).to_le_bytes());
        }
        if prev as u64 != g.chunk_bytes {
            return Err(Error::Format(format!(
                "cell {}: LOD {} offset table ends at {prev} but the region holds {} bytes",
                cell.id, plan.lod, g.chunk_bytes
            )));
        }
    }
    out(&table)?;

    // 3. The chunk bytes, verbatim (§2.3). One streaming copy per cell.
    for g in &plan.cells {
        if g.chunk_bytes == 0 {
            continue;
        }
        let cell = &cells[g.cell];
        let l = cell.lod(plan.lod)?;
        let data_start = l.index_offset + l.node_count * 4 + (l.chunk_count + 1) * 4;
        cell.copy(data_start, g.chunk_bytes as usize, out)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// OBCA §7's worked example, in index values: A is a five-node subtree with two chunks, B a
    /// single leaf with one. The spec states the answer; this asserts it.
    #[test]
    fn worked_example_relocation() {
        // A's nodes 1.. land at 5, so its branch delta is `block_base − 1 = 4`.
        assert_eq!(relocate(BRANCH_BIT | 1, 5, 0), BRANCH_BIT | 5, "A's root: children 1..4 → 5..8");
        assert_eq!(relocate(0x0000_0000, 5, 0), 0x0000_0000, "A's NW leaf keeps chunk 0");
        assert_eq!(relocate(0x0000_0001, 5, 0), 0x0000_0001, "A's NE leaf keeps chunk 1");
        assert_eq!(relocate(EMPTY_LEAF, 5, 0), EMPTY_LEAF, "empty stays empty");
        // B's root leaf: chunk 0 relocated by +2, because A owns chunks 0 and 1.
        assert_eq!(relocate(0x0000_0000, 9, 2), 0x0000_0002);
    }
}
