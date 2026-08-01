// Drawing a cell set: the union's true stair-edged shape, as few shapes as
// possible.
//
// Epic #1016 §8 U1 decided both halves of this. There is **no grid on the map** —
// cells are an implementation detail, never a lattice, never a wash, never a
// legend — and the one thing that *is* drawn is "the actual coverage outline of
// the current selection, which snaps outward to cell boundaries, drawn honestly
// as its true stair-edged shape". That sentence is a geometry problem, and this
// module is it.
//
// It is also a performance problem, which is the other reason this is not left
// to the component. A DACH selection is tens of thousands of fine cells; handing
// Leaflet 45 000 rectangles is not a plan, and handing it 45 000 rectangles that
// share edges is not even an honest drawing — the shared edges show as seams
// through the fill. Both functions here answer the same question in the two
// shapes a map library wants:
//
//   * {@link mergeCellRects} — the union as a handful of maximal rectangles.
//     Rows of adjacent cells become one rectangle, and identical runs in
//     consecutive rows merge downward. A block of 200 × 200 cells is one
//     rectangle. Cheap to compute, cheap to draw, no seams.
//   * {@link coverageRings} — the union's boundary as closed rings, collinear
//     points merged away, holes included and wound the other way. This is the
//     outline proper: one stroked path per ring, which is what "drawn honestly
//     as its true stair-edged shape" means when the shape has a hole in it.
//
// Both take cells of **one** size. A band's cells are all one size by
// construction (`OBCA_Spec.md` §1.2), and mixing two would make "the cell north
// of this one" ambiguous — so it is refused rather than approximated.

import { cellSize, GRID_ORIGIN, GridError, type CellId, type UBox } from "./grid";

/** A vertex of an outline: integer microdegrees, `[lat, lon]` — the catalog's
 *  own order for a boundary ring (`OBCC_Spec.md` §7). */
export type RingPoint = [number, number];

interface Lattice {
    log2: number;
    /** `i * stride + j`, so a row is contiguous and a lookup is one number. */
    keys: Set<number>;
    stride: number;
    rows: Map<number, number[]>;
}

/** The cells as a lattice, refusing a mixed-size set. */
function lattice(cells: Iterable<CellId>): Lattice | null {
    let log2: number | null = null;
    const rows = new Map<number, number[]>();
    const keys = new Set<number>();
    // Wide enough that `i * stride + j` is unique for every in-world index, and
    // still exact in a double: 2^19 × 2^19 = 2^38.
    const stride = 2 ** 19;
    for (const cell of cells) {
        if (log2 === null) log2 = cell.log2;
        else if (cell.log2 !== log2) {
            throw new GridError(
                `an outline is drawn from cells of one size; this set mixes 2^${log2} and 2^${cell.log2} µdeg`,
            );
        }
        const key = cell.i * stride + cell.j;
        if (keys.has(key)) continue;
        keys.add(key);
        const row = rows.get(cell.i);
        if (row) row.push(cell.j);
        else rows.set(cell.i, [cell.j]);
    }
    if (log2 === null) return null;
    for (const row of rows.values()) row.sort((a, b) => a - b);
    return { log2, keys, stride, rows };
}

/** Maximal horizontal runs of a sorted row: `[j0, j1]`, inclusive. */
function runsOf(js: number[]): [number, number][] {
    const runs: [number, number][] = [];
    for (const j of js) {
        const last = runs[runs.length - 1];
        if (last && j === last[1] + 1) last[1] = j;
        else runs.push([j, j]);
    }
    return runs;
}

/**
 * The union of the cells' squares, as few axis-aligned rectangles as a
 * row-then-column merge gets it to.
 *
 * Not provably minimal — that is a harder problem than this is worth — but it
 * collapses the cases that matter: a solid block is one rectangle, a corridor
 * along a road is one per stair step rather than one per cell, and a country is
 * a few hundred instead of tens of thousands. The rectangles are disjoint and
 * their union is exactly the cells' union, so a fill drawn from them has no
 * seams and covers no ground the selection does not.
 *
 * In ascending `(minLat, minLon)`.
 */
export function mergeCellRects(cells: Iterable<CellId>): UBox[] {
    const grid = lattice(cells);
    if (!grid) return [];
    const s = cellSize(grid.log2);
    const out: UBox[] = [];
    interface Open {
        j0: number;
        j1: number;
        i0: number;
        i1: number;
    }
    const emit = (r: Open) =>
        out.push({
            minLat: GRID_ORIGIN + r.i0 * s,
            minLon: GRID_ORIGIN + r.j0 * s,
            maxLat: GRID_ORIGIN + (r.i1 + 1) * s,
            maxLon: GRID_ORIGIN + (r.j1 + 1) * s,
        });

    /** Rectangles still growing northward, keyed by the run they span — so a
     *  row with a thousand runs costs a thousand lookups, not a million. */
    let open = new Map<string, Open>();
    for (const i of [...grid.rows.keys()].sort((a, b) => a - b)) {
        const next = new Map<string, Open>();
        for (const [j0, j1] of runsOf(grid.rows.get(i)!)) {
            const key = `${j0} ${j1}`;
            // A run that is exactly the run below it extends that rectangle;
            // anything else starts a new one, which is what makes a staircase
            // come out as steps rather than as cells.
            const below = open.get(key);
            if (below && below.i1 === i - 1) {
                below.i1 = i;
                open.delete(key);
                next.set(key, below);
            } else {
                next.set(key, { j0, j1, i0: i, i1: i });
            }
        }
        for (const rect of open.values()) emit(rect);
        open = next;
    }
    for (const rect of open.values()) emit(rect);
    return out.sort((a, b) => a.minLat - b.minLat || a.minLon - b.minLon);
}

/** A directed boundary edge, in lattice coordinates (corner indices). */
interface Edge {
    from: number;
    to: number;
}

/** Corner `(i, j)` of the lattice as one number — corners run one further than
 *  cells on each axis. */
function corner(i: number, j: number, stride: number): number {
    return i * stride + j;
}

/**
 * The boundary of the cells' union, as closed rings of `[lat, lon]` µdeg.
 *
 * Each ring's last point repeats its first, the way `OBCC_Spec.md` §7 spells
 * a region boundary — so the two kinds of outline a map draws are the same shape
 * of data. Outer rings run counter-clockwise and holes run clockwise, which is
 * what an even-odd or non-zero fill needs to leave a hole empty.
 *
 * Collinear vertices are dropped, so a straight run of forty cells is two
 * points, not forty-one. What survives is the staircase itself, which is the
 * part the user is being shown honestly.
 */
export function coverageRings(cells: Iterable<CellId>): RingPoint[][] {
    const grid = lattice(cells);
    if (!grid) return [];
    const { keys, stride } = grid;
    const filled = (i: number, j: number) => keys.has(i * stride + j);

    // One directed edge per cell side with no neighbour behind it, wound so the
    // interior is always on the left. That single convention is what makes
    // outer rings come out counter-clockwise and holes clockwise, with no
    // second pass to work out which is which.
    const outgoing = new Map<number, Edge[]>();
    const push = (from: number, to: number) => {
        const at = outgoing.get(from);
        if (at) at.push({ from, to });
        else outgoing.set(from, [{ from, to }]);
    };
    for (const key of keys) {
        const i = Math.floor(key / stride);
        const j = key - i * stride;
        const c = (ci: number, cj: number) => corner(ci, cj, stride);
        if (!filled(i - 1, j)) push(c(i, j), c(i, j + 1)); // south edge, west → east
        if (!filled(i, j + 1)) push(c(i, j + 1), c(i + 1, j + 1)); // east edge, south → north
        if (!filled(i + 1, j)) push(c(i + 1, j + 1), c(i + 1, j)); // north edge, east → west
        if (!filled(i, j - 1)) push(c(i + 1, j), c(i, j)); // west edge, north → south
    }

    const s = cellSize(grid.log2);
    const point = (key: number): RingPoint => {
        const i = Math.floor(key / stride);
        const j = key - i * stride;
        return [GRID_ORIGIN + i * s, GRID_ORIGIN + j * s];
    };
    const direction = (e: Edge): [number, number] => {
        const from = point(e.from);
        const to = point(e.to);
        return [Math.sign(to[0] - from[0]), Math.sign(to[1] - from[1])];
    };

    const rings: RingPoint[][] = [];
    const used = new Set<Edge>();
    for (const start of [...outgoing.values()].flat()) {
        if (used.has(start)) continue;
        const ring: number[] = [start.from];
        let edge = start;
        for (;;) {
            used.add(edge);
            ring.push(edge.to);
            if (edge.to === start.from) break;
            const candidates = (outgoing.get(edge.to) ?? []).filter((e) => !used.has(e));
            if (candidates.length === 0) break;
            edge = candidates.length === 1 ? candidates[0] : sharpestLeft(edge, candidates, direction);
        }
        rings.push(simplify(ring.map(point)));
    }
    return rings;
}

/**
 * At a diagonal pinch — two cells meeting at one corner and nothing else — four
 * boundary edges share a vertex and the walk has a choice.
 *
 * Edges are wound with the interior on the left, so turning as far *left* as
 * possible is the turn that stays on the patch already being traced instead of
 * stepping across the corner into the other one. The pinch therefore comes out
 * as two rings touching at a point rather than one figure-of-eight — and a
 * figure-of-eight is not a polygon that any fill rule agrees about.
 */
function sharpestLeft(
    incoming: Edge,
    candidates: Edge[],
    direction: (e: Edge) => [number, number],
): Edge {
    const [dLat, dLon] = direction(incoming);
    const rank = (e: Edge): number => {
        const [lat, lon] = direction(e);
        // Cross product in (east, north) order: positive is a left turn.
        const cross = dLon * lat - dLat * lon;
        if (cross > 0) return 0; // left
        if (cross === 0 && lat === dLat && lon === dLon) return 1; // straight on
        if (cross < 0) return 2; // right
        return 3; // straight back the way we came
    };
    return candidates.reduce((best, e) => (rank(e) < rank(best) ? e : best));
}

/** Drop the vertices a straight run passes through, keeping the ring closed. */
function simplify(ring: RingPoint[]): RingPoint[] {
    if (ring.length < 3) return ring;
    // The first and last point are the same vertex; work on the open ring.
    const open = ring.slice(0, -1);
    const kept: RingPoint[] = [];
    for (let k = 0; k < open.length; k++) {
        const before = open[(k - 1 + open.length) % open.length];
        const here = open[k];
        const after = open[(k + 1) % open.length];
        const turns =
            (here[0] - before[0]) * (after[1] - here[1]) !== (here[1] - before[1]) * (after[0] - here[0]);
        if (turns) kept.push(here);
    }
    return kept.length ? [...kept, kept[0]] : ring;
}
