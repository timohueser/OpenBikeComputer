// The pinning suite for the TS grid mirror.
//
// Every vector below is copied from `host/obc-pack/src/grid.rs`'s own tests —
// the same ids, the same squares, the same edge cases, including the ones that
// only fail when a division truncates toward zero instead of flooring. That is
// deliberate and it is the point of the file: the builder resolves a drawn box
// to cell ids, the bakery resolved ground to the *same* ids, and if the two
// arithmetics ever drift the symptom is not an exception — it is a map with a
// silent hole in it. Two implementations pinned to one set of numbers cannot
// drift without one of these tests going red.
//
// The Rust squares are quoted in that side's `(min_lon, min_lat, max_lon,
// max_lat)` order in the comments; the assertions use this module's named
// `lat/lon` object, so the re-ordering is visible rather than assumed.

import { describe, expect, it } from "vitest";
import {
    axisCells,
    cellContaining,
    cellContains,
    cellId,
    cellsIntersecting,
    cellSquare,
    coverageBbox,
    formatCellId,
    GRID_ORIGIN,
    GridError,
    idWidth,
    MAX_CELL_LOG2,
    MAX_ENUMERATED_CELLS,
    MIN_CELL_LOG2,
    onGridLine,
    parseCellId,
    WORLD_SIDE,
} from "./grid";

describe("grid constants", () => {
    // Rust: `grid_constants_and_nesting`.
    it("are OBCA §1.1's, and every permitted size nests", () => {
        expect(GRID_ORIGIN).toBe(-268_435_456);
        expect(WORLD_SIDE).toBe(536_870_912);
        for (let log2 = MIN_CELL_LOG2; log2 <= MAX_CELL_LOG2; log2++) {
            const s = 2 ** log2;
            // `===`, not `toBe`: a negative dividend gives JS's `-0`, which is
            // `=== 0` but not `Object.is` 0. `onGridLine` compares the same way
            // and is correct for the same reason.
            expect(GRID_ORIGIN % s === 0).toBe(true);
            expect(WORLD_SIDE % s === 0).toBe(true);
            expect(axisCells(log2) * s).toBe(WORLD_SIDE);
        }
        // The world box contains the geographic domain with room to spare.
        expect(GRID_ORIGIN).toBeLessThan(-180_000_000);
        expect(GRID_ORIGIN + WORLD_SIDE).toBeGreaterThan(180_000_000);
    });

    it("keeps every coordinate inside the exact-integer range of a double", () => {
        // The mirror's licence to use plain numbers: the largest value any
        // function here computes is a world-box span, 2^29, and 2^53 is where
        // integers stop being exact.
        expect(WORLD_SIDE).toBeLessThan(Number.MAX_SAFE_INTEGER);
        expect(Number.isSafeInteger(GRID_ORIGIN - WORLD_SIDE)).toBe(true);
    });
});

describe("cell identity", () => {
    // Rust: `worked_example_squares` — OBCA §7's worked example.
    it("matches the spec's worked example to the microdegree", () => {
        const a = parseCellId("18/1204/1052");
        // Rust: (7_340_032, 47_185_920, 7_602_176, 47_448_064).
        expect(cellSquare(a)).toEqual({
            minLat: 47_185_920,
            minLon: 7_340_032,
            maxLat: 47_448_064,
            maxLon: 7_602_176,
        });
        const b = parseCellId("18/1204/1053");
        expect(cellSquare(b)).toEqual({
            minLat: 47_185_920,
            minLon: 7_602_176,
            maxLat: 47_448_064,
            maxLon: 7_864_320,
        });
        // Neighbours share the seam exactly: A's maxLon is B's minLon.
        expect(cellSquare(a).maxLon).toBe(cellSquare(b).minLon);
        expect(formatCellId(a)).toBe("18/1204/1052");
    });

    // Rust: `half_open_ownership_is_exclusive`.
    it("owns its square half-open, so every point is in exactly one cell", () => {
        const a = parseCellId("18/1204/1052");
        const { minLat, minLon, maxLat, maxLon } = cellSquare(a);
        expect(cellContains(a, minLat, minLon)).toBe(true);
        expect(cellContains(a, maxLat, minLon)).toBe(false);
        expect(cellContains(a, minLat, maxLon)).toBe(false);
        expect(cellContaining(18, minLat, minLon)).toEqual(a);
        expect(cellContaining(18, maxLat, maxLon)).toEqual(cellId(18, a.i + 1, a.j + 1));
        expect(cellContaining(18, maxLat - 1, maxLon - 1)).toEqual(a);
    });

    // Rust: `id_padding_widths`.
    it("pads ids to §1.3's width, and widens rather than truncates", () => {
        expect(idWidth(20)).toBe(4);
        expect(idWidth(18)).toBe(4);
        expect(idWidth(16)).toBe(4);
        expect(idWidth(10)).toBe(6);
        expect(formatCellId(cellId(10, 7, 9))).toBe("10/000007/000009");
        expect(parseCellId("10/000007/000009")).toEqual(cellId(10, 7, 9));
        // Lenient in, canonical out.
        expect(formatCellId(parseCellId("18/7/9"))).toBe("18/0007/0009");
    });

    // Rust: `id_parse_rejects_out_of_range`.
    it.each([
        ["18/2048/0", "2^18 has 2048 cells per axis, so 2048 is past the end"],
        ["9/0/0", "2^9 is below the grid's minimum size"],
        ["29/0/0", "2^29 is above the maximum"],
        ["18/-1/0", "a negative index is not an index"],
        ["18/0", "too few parts"],
        ["18/0/0/0", "too many parts"],
        ["18/0/0x1", "not a decimal"],
    ])("rejects %s (%s)", (id) => {
        expect(() => parseCellId(id)).toThrow(GridError);
    });

    // The nesting the catalog's own example exercises: one region's fine cell,
    // its mid parent and its coarse grandparent, from `region-cells.v2.example.json`.
    it("nests the bands the example region names", () => {
        const fine = parseCellId("18/1204/1052");
        const mid = parseCellId("19/0602/0526");
        const coarse = parseCellId("20/0301/0263");
        const sq = cellSquare(fine);
        expect(cellContaining(19, sq.minLat, sq.minLon)).toEqual(mid);
        expect(cellContaining(20, sq.minLat, sq.minLon)).toEqual(coarse);
        // A coarse cell wholly contains the fine one — that is what makes the
        // coarse band's coverage generous without a second rule.
        const c = cellSquare(coarse);
        expect(c.minLat).toBeLessThanOrEqual(sq.minLat);
        expect(c.maxLat).toBeGreaterThanOrEqual(sq.maxLat);
    });
});

describe("the negative origin", () => {
    // Rust: `alignment_theorem_holds_at_the_negative_origin` (the grid half of
    // it — the quadtree half is the assembler's, not this client's).
    it("floors rather than truncates at and below the origin", () => {
        const c = cellId(20, 0, 0);
        expect(cellSquare(c)).toEqual({
            minLat: GRID_ORIGIN,
            minLon: GRID_ORIGIN,
            maxLat: GRID_ORIGIN + 2 ** 20,
            maxLon: GRID_ORIGIN + 2 ** 20,
        });
        expect(cellContaining(20, GRID_ORIGIN, GRID_ORIGIN)).toEqual(c);
        // One µdeg into the first cell, and one µdeg below the second's min —
        // both land where a truncating division would not.
        expect(cellContaining(20, GRID_ORIGIN + 1, GRID_ORIGIN + 1)).toEqual(c);
        expect(cellContaining(20, GRID_ORIGIN + 2 ** 20 - 1, GRID_ORIGIN)).toEqual(c);
    });

    it("floors *below* the origin, which is the only place trunc differs", () => {
        // The vector that earns `floorDiv` its keep. Everything else in this
        // describe block is at or above the origin, where the dividend is
        // non-negative and `Math.trunc` gives the same answer — as it does for
        // every coordinate a catalog can contain, the origin being −2^28 and the
        // geographic domain ±180°. So this is defence-in-depth, deliberately
        // reaching outside the world box to state what the division does there.
        expect(cellContaining(20, GRID_ORIGIN - 1, GRID_ORIGIN - 1)).toEqual({ log2: 20, i: -1, j: -1 });
        // One µdeg past a whole cell below the origin: floor says the second
        // cell down, trunc says the first.
        expect(cellContaining(18, GRID_ORIGIN - 2 ** 18 - 1, GRID_ORIGIN - 2 ** 18 - 1)).toEqual({
            log2: 18,
            i: -2,
            j: -2,
        });
        // A negative index is not a cell: `cellId` refuses to mint one, so a
        // below-origin answer cannot be mistaken for a published square.
        expect(() => cellId(18, -1, 0)).toThrow(GridError);
    });

    // Rust: `cells_south_of_the_equator`.
    it("puts Cape Town in a cell whose square really contains it", () => {
        const c = cellContaining(18, -33_900_000, 18_400_000);
        const { minLat, minLon, maxLat, maxLon } = cellSquare(c);
        expect(minLat).toBeLessThanOrEqual(-33_900_000);
        expect(-33_900_000).toBeLessThan(maxLat);
        expect(minLon).toBeLessThanOrEqual(18_400_000);
        expect(18_400_000).toBeLessThan(maxLon);
        expect(maxLat - minLat).toBe(2 ** 18);
        expect(onGridLine(minLat, 18)).toBe(true);
        expect(onGridLine(minLon, 18)).toBe(true);
    });
});

describe("cellsIntersecting", () => {
    // Rust: `cells_intersecting_covers_the_box_and_its_edges`.
    it("covers the box and the cells its edges fall in", () => {
        const a = parseCellId("18/1204/1052");
        const { minLat, minLon, maxLat, maxLon } = cellSquare(a);
        const one = cellsIntersecting(18, {
            minLat: minLat + 1,
            minLon: minLon + 1,
            maxLat: maxLat - 1,
            maxLon: maxLon - 1,
        });
        expect(one).toEqual([a]);

        // Reaching the shared edge brings the neighbour along, because a vertex
        // exactly on the line belongs to it.
        const two = cellsIntersecting(18, {
            minLat: minLat + 1,
            minLon: minLon + 1,
            maxLat: maxLat - 1,
            maxLon,
        });
        expect(two).toEqual([a, cellId(18, a.i, a.j + 1)]);

        const four = cellsIntersecting(18, { minLat: minLat + 1, minLon: minLon + 1, maxLat, maxLon });
        expect(four).toHaveLength(4);
        for (let k = 1; k < four.length; k++) {
            const prev = four[k - 1];
            const next = four[k];
            expect(prev.i < next.i || (prev.i === next.i && prev.j < next.j)).toBe(true);
        }
    });

    it("is generous at coarse sizes for the same box", () => {
        // The epic's coverage rule, seen from the builder: one box, four bands,
        // precise at 2^18 and whole-cell context at 2^20.
        const box = { minLat: 47_200_000, minLon: 7_400_000, maxLat: 47_300_000, maxLon: 7_500_000 };
        expect(cellsIntersecting(18, box)).toHaveLength(1);
        expect(cellsIntersecting(20, box)).toHaveLength(1);
        const coarse = cellSquare(cellsIntersecting(20, box)[0]);
        const fine = cellSquare(cellsIntersecting(18, box)[0]);
        expect(coarse.maxLat - coarse.minLat).toBe(4 * (fine.maxLat - fine.minLat));
    });

    it("returns nothing for an inverted box", () => {
        expect(cellsIntersecting(18, { minLat: 10, minLon: 10, maxLat: 0, maxLon: 20 })).toEqual([]);
        expect(cellsIntersecting(18, { minLat: 0, minLon: 20, maxLat: 10, maxLon: 10 })).toEqual([]);
    });

    it("refuses a cell size the grid does not admit", () => {
        expect(() => cellsIntersecting(9, { minLat: 0, minLon: 0, maxLat: 1, maxLon: 1 })).toThrow(GridError);
    });

    it("refuses to enumerate more cells than it will hand back", () => {
        // A zoomed-out map view is one drag away from asking for the world, and
        // the answer is a product of two spans: 2048 × 2048 at 2^18. Refused
        // before anything is allocated.
        const world = {
            minLat: GRID_ORIGIN,
            minLon: GRID_ORIGIN,
            maxLat: GRID_ORIGIN + WORLD_SIDE - 1,
            maxLon: GRID_ORIGIN + WORLD_SIDE - 1,
        };
        expect(() => cellsIntersecting(18, world)).toThrow(GridError);
        expect(() => cellsIntersecting(18, world)).toThrow(/smaller area/);
        // The ceiling is the mirror's, not a caller's, and it is adjustable for
        // the one caller that knows better (a test oracle, a batch job).
        const s = 2 ** 18;
        const wide = {
            minLat: GRID_ORIGIN,
            minLon: GRID_ORIGIN,
            maxLat: GRID_ORIGIN + 299 * s,
            maxLon: GRID_ORIGIN + 299 * s,
        };
        expect(() => cellsIntersecting(18, wide)).toThrow(GridError);
        expect(cellsIntersecting(18, wide, 300 * 300)).toHaveLength(300 * 300);
        // Right at the limit is fine: 256 × 256 cells.
        const exactly = {
            minLat: GRID_ORIGIN,
            minLon: GRID_ORIGIN,
            maxLat: GRID_ORIGIN + 255 * s,
            maxLon: GRID_ORIGIN + 255 * s,
        };
        expect(cellsIntersecting(18, exactly)).toHaveLength(MAX_ENUMERATED_CELLS);
    });

    it("covers nothing for a box wholly outside the world box", () => {
        // Clamping one of these would hand back a strip of edge cells covering
        // ground the box never touched — a selection the user did not draw.
        const s = 2 ** 18;
        const past = GRID_ORIGIN + WORLD_SIDE;
        expect(
            cellsIntersecting(18, { minLat: GRID_ORIGIN - 4 * s, minLon: 0, maxLat: GRID_ORIGIN - s, maxLon: s }),
        ).toEqual([]);
        expect(cellsIntersecting(18, { minLat: 0, minLon: past, maxLat: s, maxLon: past + 4 * s })).toEqual([]);
        // Touching the world box's own edge is still inside it.
        expect(cellsIntersecting(18, { minLat: GRID_ORIGIN - s, minLon: GRID_ORIGIN, maxLat: GRID_ORIGIN, maxLon: GRID_ORIGIN })).toEqual([
            { log2: 18, i: 0, j: 0 },
        ]);
    });
});

describe("onGridLine", () => {
    // Rust: `on_grid_line_only_at_the_lines`.
    it("is true only on the lines, and coarse lines are also fine lines", () => {
        const s = 2 ** 18;
        expect(onGridLine(GRID_ORIGIN, 18)).toBe(true);
        expect(onGridLine(GRID_ORIGIN + 5 * s, 18)).toBe(true);
        expect(onGridLine(GRID_ORIGIN + 5 * s + 1, 18)).toBe(false);
        expect(onGridLine(47_185_920, 18)).toBe(true);
        expect(onGridLine(47_185_921, 18)).toBe(false);
        const coarse = GRID_ORIGIN + 3 * 2 ** 20;
        expect(onGridLine(coarse, 20)).toBe(true);
        expect(onGridLine(coarse, 18)).toBe(true);
        expect(onGridLine(GRID_ORIGIN + 2 ** 18, 18)).toBe(true);
        expect(onGridLine(GRID_ORIGIN + 2 ** 18, 20)).toBe(false);
    });
});

describe("coverageBbox", () => {
    it("is null for nothing and the union's box otherwise", () => {
        expect(coverageBbox([])).toBeNull();
        const box = coverageBbox([parseCellId("18/1204/1052"), parseCellId("18/1205/1053")]);
        expect(box).toEqual({
            minLat: 47_185_920,
            minLon: 7_340_032,
            maxLat: 47_710_208,
            maxLon: 7_864_320,
        });
    });
});
