// Lasso cells: the cells a drawn ring overlaps.
//
// The assertions worth having are the shape ones — a diagonal ring must select
// its stair of cells and NOT the bounding box's rectangle, because "better than
// a box for diagonal shapes" is the whole reason the tool exists — and the
// containment ones: a cell wholly inside the ring has no edge near it, so it is
// selected by the even-odd corner test alone, and a ring drawn wholly inside
// one cell has no boundary crossing either, so it exercises the
// segment-inside-square half of the overlap test.

import { describe, expect, it } from "vitest";
import type { LatLon } from "./corridor";
import { cellSize, cellSquare, formatCellId, GridError, parseCellId } from "./grid";
import { lassoCells, MAX_LASSO_LON_SPAN } from "./lasso";

const CELL = parseCellId("18/1204/1052");
const SQUARE = cellSquare(CELL);
const S = cellSize(18);
const at = (di: number, dj: number): LatLon => ({
    lat: SQUARE.minLat + di * S,
    lon: SQUARE.minLon + dj * S,
});

describe("lassoCells", () => {
    it("selects nothing for a degenerate ring", () => {
        expect(lassoCells(18, [])).toEqual([]);
        expect(lassoCells(18, [at(0.5, 0.5)])).toEqual([]);
        expect(lassoCells(18, [at(0.5, 0.5), at(0.6, 0.6)])).toEqual([]);
    });

    it("selects the one cell a small ring is drawn inside", () => {
        // No ring edge crosses the square's boundary: the edge-in-square half
        // of the overlap test is what admits it.
        const ring = [at(0.3, 0.3), at(0.3, 0.7), at(0.7, 0.7), at(0.7, 0.3)];
        expect(lassoCells(18, ring).map(formatCellId)).toEqual(["18/1204/1052"]);
    });

    it("selects a cell wholly inside the ring by containment alone", () => {
        // A 3×3-cell ring: the centre cell touches no edge, so only the
        // even-odd corner test can admit it.
        const ring = [at(-1, -1), at(-1, 2), at(2, 2), at(2, -1)];
        const ids = lassoCells(18, ring).map(formatCellId);
        expect(ids).toContain("18/1204/1052");
        // 3×3 of cells plus the ring's own edges grazing the next row/column
        // at the closed/half-open boundary — the centre and its 8 neighbours
        // are the ones that matter and must all be there.
        for (let di = -1; di <= 1; di++) {
            for (let dj = -1; dj <= 1; dj++) {
                expect(ids).toContain(formatCellId({ log2: 18, i: CELL.i + di, j: CELL.j + dj }));
            }
        }
    });

    it("selects a diagonal stair, not the bounding rectangle", () => {
        // A thin diagonal band across 6×6 cells of ground. The bounding box
        // covers 36+ cells; the band itself must come back far smaller, and the
        // far corners must not be in it.
        const ring = [at(0.5, 0.1), at(0.1, 0.5), at(5.5, 5.9), at(5.9, 5.5)];
        const ids = lassoCells(18, ring).map(formatCellId);
        expect(ids).toContain("18/1204/1052");
        expect(ids).toContain(formatCellId({ log2: 18, i: CELL.i + 5, j: CELL.j + 5 }));
        // Off-diagonal corners of the bbox: outside the band.
        expect(ids).not.toContain(formatCellId({ log2: 18, i: CELL.i, j: CELL.j + 5 }));
        expect(ids).not.toContain(formatCellId({ log2: 18, i: CELL.i + 5, j: CELL.j }));
        expect(ids.length).toBeLessThan(30);
    });

    it("closes the ring itself — last point back to first", () => {
        // An open L: as a closed ring it is a triangle covering the corner
        // cell diagonal's other side.
        const ring = [at(0.5, 0.5), at(0.5, 3.5), at(3.5, 3.5)];
        const ids = lassoCells(18, ring).map(formatCellId);
        // The closing edge runs (3.5,3.5)→(0.5,0.5): its cells must be there.
        expect(ids).toContain(formatCellId({ log2: 18, i: CELL.i + 2, j: CELL.j + 2 }));
        // …and the corner far from the hypotenuse must not.
        expect(ids).not.toContain(formatCellId({ log2: 18, i: CELL.i + 3, j: CELL.j }));
    });

    it("refuses a ring across the antimeridian's half-world span", () => {
        const ring = [
            { lat: SQUARE.minLat, lon: -90_000_100 },
            { lat: SQUARE.minLat, lon: 90_000_100 },
            { lat: SQUARE.maxLat, lon: 90_000_100 },
        ];
        expect(ring[1].lon - ring[0].lon).toBeGreaterThan(MAX_LASSO_LON_SPAN);
        expect(() => lassoCells(18, ring)).toThrow(GridError);
    });

    it("agrees with the coarse band without a special case", () => {
        // The same ring at 2^20 selects the covering coarse cells — generous
        // context is a consequence of cell size, exactly as for boxes.
        const ring = [at(0.3, 0.3), at(0.3, 0.7), at(0.7, 0.7), at(0.7, 0.3)];
        const coarse = lassoCells(20, ring);
        expect(coarse.length).toBe(1);
        expect(coarse[0].log2).toBe(20);
    });
});
