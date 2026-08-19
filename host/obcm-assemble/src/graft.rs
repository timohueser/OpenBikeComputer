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

use crate::emit::{align_up, filler_len, scaled, MapWriter, SCALE};
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
    /// Added to every copied offset-table entry. **Units, not bytes** since v14 (`OBCM_Spec.md`
    /// §5.1): the entries a cell wrote count `U`-byte units and the ones the assembly writes count
    /// the same units, so the relocation constant is one addition in that currency and no
    /// arithmetic crosses between the two.
    pub chunk_unit_base: u64,
    /// Nodes in the cell's own index for this LOD (`0` ⇒ the level is empty in this cell).
    pub node_count: u32,
    pub chunk_count: u32,
    /// The cell's chunk-data region, in units — `offsets[Chunk Count]`, verbatim. It is a whole
    /// number of units because every §5.1 chunk is padded to one, which is what lets the region be
    /// copied byte-for-byte and the next cell's block start on a boundary of its own.
    pub chunk_units: u64,
}

impl GraftCell {
    /// The cell's chunk-data region in bytes — what the verbatim copy moves (§2.3).
    #[inline]
    pub fn chunk_bytes(&self) -> u64 {
        self.chunk_units * SCALE.unit()
    }
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
    /// The region's chunk data, in bytes — every cell's contribution, each already a whole number of
    /// §1.2 units.
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

    /// Total bytes of the region: index + offset table + §1.2 filler + chunks.
    ///
    /// The region's index starts on a unit boundary (its `Index Offset` is scaled, so nothing else
    /// is expressible), and the index and the offset table are both read by 4-byte indexing from
    /// there — so the one rounding step §3 puts between `table_end` and `data_start` is a function
    /// of the two counts alone and needs no absolute offset. [`emit_lod`] finds that same step by
    /// asking its cursor for the boundary, and asserts the two agree.
    ///
    /// A region always **ends** on a unit boundary — the chunk data is a whole number of units, and
    /// an empty region's four-byte table is padded to one — so the next LOD's index, and the POI
    /// section behind the last of them, start on one without the caller aligning anything.
    pub fn region_bytes(&self) -> u64 {
        if self.node_count == 0 {
            // The mandatory single-`0` offset table, plus the filler that carries the region to the
            // next boundary.
            return align_up(4);
        }
        let head = self.node_count as u64 * 4 + (self.chunk_count as u64 + 1) * 4;
        head + filler_len(head) + self.chunk_bytes
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
    let mut chunk_unit_base: u64 = 0;
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
        // The index space is **not** `uint32`: a node index shares the word with `BRANCH_BIT` and a
        // chunk id with `EMPTY_LEAF`, so the usable ranges stop one bit and one value short (§4).
        if node_base >= BRANCH_BIT as u64 || chunk_id_base + l.chunk_count as u64 >= EMPTY_LEAF as u64 {
            return Err(Error::Capacity(format!(
                "LOD {lod} exceeds the format's index space at cell {} ({node_base} node(s), {} chunk(s); the branch \
                 bit caps nodes at {} and the empty-leaf sentinel caps chunk ids at {}) — split the selection",
                cells[c].id,
                chunk_id_base + l.chunk_count as u64,
                BRANCH_BIT - 1,
                EMPTY_LEAF - 1
            )));
        }
        plan_cells.push(GraftCell {
            cell: c,
            slot,
            block_base: block_base as u32,
            chunk_id_base: chunk_id_base as u32,
            chunk_unit_base,
            node_count,
            chunk_count: l.chunk_count as u32,
            chunk_units: l.chunk_units_total as u64,
        });
        chunk_id_base += l.chunk_count as u64;
        chunk_unit_base += l.chunk_units_total as u64;
    }

    // Inline each present cell's relocated **root** into its depth-`d` slot (§4.4.2 / §7).
    for g in &plan_cells {
        let cell = &cells[g.cell];
        let l = cell.lod(lod)?;
        upper[g.slot] = if g.node_count == 0 {
            EMPTY_LEAF // a cell whose level is empty contributes an empty leaf, like an absent cell
        } else {
            let root = u32::from_le_bytes(cell.read(l.index_offset, 4)?[..4].try_into().expect("4 bytes"));
            relocate(root, g, cell.id, lod)?
        };
    }

    let chunk_bytes = chunk_unit_base * SCALE.unit();
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
///
/// **Every word is validated against the source cell first.** A cell is an input, not the
/// assembler's own output, and an index word out of its cell's range relocates into a *plausible*
/// index — a branch pointing into another cell's subtree, or a leaf naming a chunk that belongs to a
/// neighbour. That is a cross-linked map, and §4.8's verify cannot tell it apart from a correct one:
/// it decodes, it just draws someone else's geometry. So the bound is checked here, where the source
/// cell's own counts are still in hand, and the arithmetic is checked rather than left to wrap in
/// release and panic in debug.
#[inline]
fn relocate(value: u32, g: &GraftCell, cell: CellId, lod: usize) -> Result<u32> {
    let bad = |what: String| Error::Format(format!("cell {cell}: LOD {lod} index word {value:#010x} {what}"));
    if value & BRANCH_BIT != 0 {
        // The cell's node `k` lands at `block_base + k − 1`, so a child base relocates by the same
        // delta. The cell's root children are at `1..4`, which is why `−1` and not `+block_base`.
        let child = value & !BRANCH_BIT;
        // A branch's four children are a contiguous quadruple *after* it, inside the cell's index.
        if child == 0 || child.saturating_add(3) >= g.node_count {
            return Err(bad(format!(
                "is a branch whose children start at {child}, outside the cell's {} index node(s) (OBCM §4)",
                g.node_count
            )));
        }
        Ok(BRANCH_BIT | (child - 1 + g.block_base))
    } else if value == EMPTY_LEAF {
        Ok(EMPTY_LEAF)
    } else if value >= g.chunk_count {
        Err(bad(format!("names chunk {value}, past the cell's {} chunk(s)", g.chunk_count)))
    } else {
        Ok(value + g.chunk_id_base)
    }
}

/// Emit a planned LOD region: `[index][offset table][filler][chunks]`, in the format's own order
/// (`OBCM_Spec.md` §3). Nothing is decoded and no chunk byte is touched.
///
/// The filler run is v14's one rounding step: the chunks are addressed by **scaled** offsets, so
/// `data_start` has to be a unit boundary while the index and the table behind it — read by 4-byte
/// indexing — do not. Its bytes are `0xFF` like every other §1.2 gap.
pub fn emit_lod(plan: &LodPlan, cells: &[Cell<'_>], w: &mut MapWriter<'_>) -> Result<()> {
    let start = w.at();
    if plan.node_count == 0 {
        // The mandatory single-`0` offset table of §5.1, plus the filler that leaves the next
        // region's index on a boundary.
        w.put(&0u32.to_le_bytes())?;
        w.begin_section()?;
        debug_assert_eq!(w.at() - start, plan.region_bytes(), "the projection is the write");
        return Ok(());
    }

    // 1. The fresh upper tree, then each cell's relocated block (its nodes `1..`).
    let mut buf: Vec<u8> = Vec::with_capacity(plan.upper.len() * 4);
    for v in &plan.upper {
        buf.extend_from_slice(&v.to_le_bytes());
    }
    w.put(&buf)?;
    for g in &plan.cells {
        if g.node_count <= 1 {
            continue; // the root was inlined into the slot; there is no block
        }
        let cell = &cells[g.cell];
        let l = cell.lod(plan.lod)?;
        let raw = cell.read(l.index_offset + 4, (g.node_count as usize - 1) * 4)?;
        let mut block = Vec::with_capacity(raw.len());
        for word in raw.chunks_exact(4) {
            let v = relocate(u32::from_le_bytes([word[0], word[1], word[2], word[3]]), g, cell.id, plan.lod)?;
            block.extend_from_slice(&v.to_le_bytes());
        }
        w.put(&block)?;
    }

    // 2. The offset table: `chunk_count + 1` entries **in units** (§5.1), each cell's shifted by its
    // unit base. Every copied pair is re-checked against the capacity bound — a cell that violated
    // it would poison the assembly (§4.4.4).
    let span_bound = align_up(plan.chunk_size as u64);
    let mut table: Vec<u8> = Vec::with_capacity((plan.chunk_count as usize + 1) * 4);
    table.extend_from_slice(&0u32.to_le_bytes());
    for g in &plan.cells {
        if g.chunk_count == 0 {
            continue;
        }
        let cell = &cells[g.cell];
        let l = cell.lod(plan.lod)?;
        let table_start = l.index_offset + (l.node_count * 4) as u64;
        let raw = cell.read(table_start, (g.chunk_count as usize + 1) * 4)?;
        let mut prev = 0u32;
        for (k, word) in raw.chunks_exact(4).enumerate() {
            let v = u32::from_le_bytes([word[0], word[1], word[2], word[3]]);
            if k == 0 {
                if v != 0 {
                    return Err(Error::Format(format!("cell {}: LOD {} offsets[0] is {v}, not 0", cell.id, plan.lod)));
                }
                continue;
            }
            // §5.1's v14 bound: a chunk's *content* may not exceed `Chunk Size`, so its *span* is
            // that content rounded up to a unit. The looser `Chunk Size + U - 1` would admit spans
            // no writer can produce.
            if v < prev || (v - prev) as u64 * SCALE.unit() > span_bound {
                return Err(Error::Format(format!(
                    "cell {}: LOD {} chunk {} spans {} bytes (capacity {}, {span_bound} rounded to the unit) or runs \
                     backwards",
                    cell.id,
                    plan.lod,
                    k - 1,
                    v.saturating_sub(prev) as u64 * SCALE.unit(),
                    plan.chunk_size
                )));
            }
            prev = v;
            table.extend_from_slice(&scaled((g.chunk_unit_base + v as u64) * SCALE.unit())?.to_le_bytes());
        }
        if prev as u64 != g.chunk_units {
            return Err(Error::Format(format!(
                "cell {}: LOD {} offset table ends at {prev} unit(s) but the region holds {}",
                cell.id, plan.lod, g.chunk_units
            )));
        }
    }
    w.put(&table)?;

    // 3. The §1.2 filler that carries the region to `data_start`, then the chunk bytes, verbatim
    // (§2.3). One streaming copy per cell, and the copy needs no per-chunk repadding: the cell's
    // own chunks are already unit-aligned, so its whole region moves as one run of bytes.
    w.begin_section()?;
    for g in &plan.cells {
        if g.chunk_units == 0 {
            continue;
        }
        let cell = &cells[g.cell];
        let l = cell.lod(plan.lod)?;
        let data_start = align_up(l.index_offset + (l.node_count * 4 + (l.chunk_count + 1) * 4) as u64);
        let mut copy = |bytes: &[u8]| w.put(bytes);
        cell.copy(data_start, g.chunk_bytes() as usize, &mut copy)?;
    }
    debug_assert_eq!(w.at() - start, plan.region_bytes(), "the projection is the write");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graft_cell(block_base: u32, chunk_id_base: u32, node_count: u32, chunk_count: u32) -> GraftCell {
        GraftCell {
            cell: 0,
            slot: 0,
            block_base,
            chunk_id_base,
            chunk_unit_base: 0,
            node_count,
            chunk_count,
            chunk_units: 0,
        }
    }

    fn cell_id() -> CellId {
        CellId::new(18, 1204, 1052).expect("valid id")
    }

    /// OBCA §7's worked example, in index values: A is a five-node subtree with two chunks, B a
    /// single leaf with one. The spec states the answer; this asserts it.
    #[test]
    fn worked_example_relocation() {
        let a = graft_cell(5, 0, 5, 2);
        let go = |v: u32, g: &GraftCell| relocate(v, g, cell_id(), 0).expect("a legal index word");
        // A's nodes 1.. land at 5, so its branch delta is `block_base − 1 = 4`.
        assert_eq!(go(BRANCH_BIT | 1, &a), BRANCH_BIT | 5, "A's root: children 1..4 → 5..8");
        assert_eq!(go(0x0000_0000, &a), 0x0000_0000, "A's NW leaf keeps chunk 0");
        assert_eq!(go(0x0000_0001, &a), 0x0000_0001, "A's NE leaf keeps chunk 1");
        assert_eq!(go(EMPTY_LEAF, &a), EMPTY_LEAF, "empty stays empty");
        // B's root leaf: chunk 0 relocated by +2, because A owns chunks 0 and 1.
        assert_eq!(go(0x0000_0000, &graft_cell(9, 2, 1, 1)), 0x0000_0002);
    }

    /// An index word outside its own cell's range is refused, not relocated. Left unchecked each of
    /// these produces a *plausible* index into another cell's block — the one graft failure §4.8's
    /// verify cannot see, because the result still decodes.
    #[test]
    fn an_out_of_range_index_word_is_a_format_error() {
        let g = graft_cell(5, 2, 5, 2);
        let go = |v: u32| relocate(v, &g, cell_id(), 3);
        let msg = |v: u32| format!("{}", go(v).expect_err("must be refused"));
        assert!(msg(0x0000_0002).contains("names chunk 2"), "a leaf past the cell's chunk count");
        assert!(msg(0x0000_00FF).contains("past the cell's 2 chunk(s)"));
        assert!(msg(BRANCH_BIT | 2).contains("children start at 2"), "children 2..5 leave the 5-node index");
        assert!(msg(BRANCH_BIT).contains("children start at 0"), "a branch may not point at the root");
        assert!(msg(BRANCH_BIT | 0x7FFF_FFFE).contains("outside"), "…and the arithmetic never wraps getting there");
        // The legal words either side of each bound still pass.
        assert!(go(0x0000_0001).is_ok() && go(BRANCH_BIT | 1).is_ok());
    }
}
