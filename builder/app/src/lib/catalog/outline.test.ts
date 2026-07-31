// The coverage drawing: rectangles to fill, rings to stroke.
//
// The shapes below are the ones the §8 U1 mock draws — a block, a staircase, an
// L, a country with a hole in it — because those are the cases where "the true
// stair-edged shape" stops being a phrase and starts being a decision about a
// vertex.

import { describe, expect, it } from "vitest";
import { cellSize, cellSquare, GridError, onGridLine, type CellId } from "./grid";
import { coverageRings, mergeCellRects, type RingPoint } from "./outline";

const LOG2 = 18;
const S = cellSize(LOG2);

/** Cells from `(i, j)` pairs, all one size. */
function cells(...ij: [number, number][]): CellId[] {
    return ij.map(([i, j]) => ({ log2: LOG2, i, j }));
}

/** A rectangular block of cells, `rows × cols` from `(i, j)`. */
function block(i: number, j: number, rows: number, cols: number): CellId[] {
    const out: CellId[] = [];
    for (let r = 0; r < rows; r++) for (let c = 0; c < cols; c++) out.push({ log2: LOG2, i: i + r, j: j + c });
    return out;
}

/** The signed area of a closed ring, in (lon, lat): positive is
 *  counter-clockwise, and the magnitude is µdeg². */
function signedArea(ring: RingPoint[]): number {
    let sum = 0;
    for (let k = 1; k < ring.length; k++) {
        sum += ring[k - 1][1] * ring[k][0] - ring[k][1] * ring[k - 1][0];
    }
    return sum / 2;
}

function boxOf(cell: CellId) {
    return cellSquare(cell);
}

describe("mergeCellRects", () => {
    it("has nothing to draw for nothing", () => {
        expect(mergeCellRects([])).toEqual([]);
    });

    it("draws one cell as its own square", () => {
        expect(mergeCellRects(cells([1204, 1052]))).toEqual([boxOf({ log2: LOG2, i: 1204, j: 1052 })]);
    });

    it("collapses a solid block into one rectangle", () => {
        // The case that decides whether this is worth having: 200 × 200 cells
        // is one rectangle, not 40 000 of them with seams between.
        const rects = mergeCellRects(block(100, 200, 200, 200));
        expect(rects).toHaveLength(1);
        expect(rects[0]).toEqual({
            minLat: boxOf({ log2: LOG2, i: 100, j: 200 }).minLat,
            minLon: boxOf({ log2: LOG2, i: 100, j: 200 }).minLon,
            maxLat: boxOf({ log2: LOG2, i: 299, j: 399 }).maxLat,
            maxLon: boxOf({ log2: LOG2, i: 299, j: 399 }).maxLon,
        });
    });

    it("draws a corridor's row as one rectangle per stair step", () => {
        // Forty cells east, then a step north and forty more: two rectangles,
        // which is what a road-following corridor actually looks like.
        const run: CellId[] = [];
        for (let j = 0; j < 40; j++) run.push({ log2: LOG2, i: 10, j: 100 + j });
        for (let j = 0; j < 40; j++) run.push({ log2: LOG2, i: 11, j: 140 + j });
        const rects = mergeCellRects(run);
        expect(rects).toHaveLength(2);
        expect(rects.map((r) => (r.maxLon - r.minLon) / S)).toEqual([40, 40]);
    });

    it("draws an L as two rectangles", () => {
        const rects = mergeCellRects(cells([0, 0], [0, 1], [1, 0]));
        expect(rects).toHaveLength(2);
        // The bottom row of two, then the single cell above it.
        expect(rects.map((r) => [(r.maxLat - r.minLat) / S, (r.maxLon - r.minLon) / S])).toEqual([
            [1, 2],
            [1, 1],
        ]);
    });

    it("covers exactly the cells, no more and no less", () => {
        // A ragged set: a block, a staircase off one corner, and a stray cell.
        const set = [
            ...block(50, 50, 4, 6),
            ...cells([54, 56], [55, 57], [56, 58], [60, 40]),
        ];
        const rects = mergeCellRects(set);
        const area = rects.reduce((sum, r) => sum + ((r.maxLat - r.minLat) / S) * ((r.maxLon - r.minLon) / S), 0);
        // Disjoint (their areas sum to the cell count) and complete (every cell
        // is inside one of them).
        expect(area).toBe(set.length);
        for (const cell of set) {
            const square = boxOf(cell);
            const inside = rects.filter(
                (r) =>
                    square.minLat >= r.minLat &&
                    square.maxLat <= r.maxLat &&
                    square.minLon >= r.minLon &&
                    square.maxLon <= r.maxLon,
            );
            expect(inside).toHaveLength(1);
        }
    });

    it("refuses a set of two cell sizes", () => {
        expect(() =>
            mergeCellRects([
                { log2: 18, i: 1204, j: 1052 },
                { log2: 20, i: 301, j: 263 },
            ]),
        ).toThrow(GridError);
    });
});

describe("coverageRings", () => {
    it("has nothing to draw for nothing", () => {
        expect(coverageRings([])).toEqual([]);
    });

    it("draws one cell as its square, counter-clockwise and closed", () => {
        const square = boxOf({ log2: LOG2, i: 1204, j: 1052 });
        const rings = coverageRings(cells([1204, 1052]));
        expect(rings).toEqual([
            [
                [square.minLat, square.minLon],
                [square.minLat, square.maxLon],
                [square.maxLat, square.maxLon],
                [square.maxLat, square.minLon],
                [square.minLat, square.minLon],
            ],
        ]);
        expect(signedArea(rings[0])).toBeGreaterThan(0);
    });

    it("drops the vertices a straight run passes through", () => {
        // Twelve cells in a row have thirteen lattice corners along the top,
        // and an outline that named all of them would be a drawing of the grid
        // — which §8 U1 decided the user never sees.
        const row: CellId[] = [];
        for (let j = 0; j < 12; j++) row.push({ log2: LOG2, i: 7, j: 100 + j });
        const [ring] = coverageRings(row);
        expect(ring).toHaveLength(5);
        expect(coverageRings(block(0, 0, 5, 5))[0]).toHaveLength(5);
    });

    it("keeps every step of a staircase", () => {
        // The shape the whole module is named after. Three cells stepping
        // north-east share only corners along the diagonal, so the outline is
        // the staircase itself: eight vertices per step pair.
        const [ring] = coverageRings(cells([0, 0], [0, 1], [1, 1], [1, 2]));
        expect(ring).toHaveLength(9);
        expect(signedArea(ring)).toBeGreaterThan(0);
        // Every vertex sits on a cell corner, which is the "snaps outward to
        // cell boundaries" half of the decision.
        for (const [lat, lon] of ring) {
            expect(onGridLine(lat, LOG2)).toBe(true);
            expect(onGridLine(lon, LOG2)).toBe(true);
        }
    });

    it("draws an L with its six corners", () => {
        const [ring] = coverageRings(cells([0, 0], [0, 1], [1, 0]));
        expect(ring).toHaveLength(7);
        expect(signedArea(ring)).toBeGreaterThan(0);
    });

    it("draws a hole as its own ring, wound the other way", () => {
        // The case a single ring cannot express: a region with an unbaked cell
        // in the middle of it. Even-odd or non-zero, the hole only stays empty
        // if its ring runs the other way.
        const donut = block(0, 0, 3, 3).filter((c) => !(c.i === 1 && c.j === 1));
        const rings = coverageRings(donut);
        expect(rings).toHaveLength(2);
        const [outer, inner] = rings.sort((a, b) => Math.abs(signedArea(b)) - Math.abs(signedArea(a)));
        expect(outer).toHaveLength(5);
        expect(inner).toHaveLength(5);
        expect(signedArea(outer)).toBeGreaterThan(0);
        expect(signedArea(inner)).toBeLessThan(0);
        expect(Math.abs(signedArea(outer))).toBe(9 * S * S);
        expect(Math.abs(signedArea(inner))).toBe(S * S);
    });

    it("draws two disjoint patches as two rings", () => {
        const rings = coverageRings([...block(0, 0, 2, 2), ...block(40, 40, 1, 3)]);
        expect(rings).toHaveLength(2);
        expect(rings.every((r) => signedArea(r) > 0)).toBe(true);
    });

    it("splits a diagonal pinch into two rings rather than a figure-of-eight", () => {
        // Two cells meeting at one corner: four boundary edges share a vertex
        // and the walk has to choose. A figure-of-eight is not a polygon any
        // fill rule agrees about, so it comes out as two squares touching.
        const rings = coverageRings(cells([0, 0], [1, 1]));
        expect(rings).toHaveLength(2);
        for (const ring of rings) {
            expect(ring).toHaveLength(5);
            expect(signedArea(ring)).toBe(S * S);
        }
    });

    it("refuses a set of two cell sizes", () => {
        expect(() =>
            coverageRings([
                { log2: 18, i: 1204, j: 1052 },
                { log2: 20, i: 301, j: 263 },
            ]),
        ).toThrow(GridError);
    });

    it("encloses exactly the area the cells cover", () => {
        // The invariant that survives any change to the walk: however the rings
        // come out, their signed areas sum to the cells' own area.
        const set = [...block(20, 20, 5, 7), ...cells([25, 27], [26, 28], [30, 10])];
        const total = coverageRings(set).reduce((sum, r) => sum + signedArea(r), 0);
        expect(total).toBe(set.length * S * S);
    });
});
