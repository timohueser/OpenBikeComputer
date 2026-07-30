// Corridor buffering: the cells within R of a route.
//
// The interesting assertions here are the *boundary* ones — a cell just inside
// the radius and the same cell just outside it — because that is where an error
// in the metre-per-microdegree conversion or in the segment-to-rectangle
// distance would show, and nowhere else. A factor-of-1e6 slip does not make the
// result subtly wrong; it makes it select the world or nothing.

import { describe, expect, it } from "vitest";
import { corridorCells, M_PER_DEG, type LatLon } from "./corridor";
import { cellSquare, formatCellId, parseCellId } from "./grid";

const CELL = parseCellId("18/1204/1052");
const SQUARE = cellSquare(CELL);
const CENTRE: LatLon = {
    lat: (SQUARE.minLat + SQUARE.maxLat) / 2,
    lon: (SQUARE.minLon + SQUARE.maxLon) / 2,
};

/** µdeg of longitude that span `metres` at the cell's centre latitude. */
function lonUdegFor(metres: number): number {
    const perUdeg = (M_PER_DEG / 1e6) * Math.cos((CENTRE.lat / 1e6) * (Math.PI / 180));
    return Math.round(metres / perUdeg);
}

describe("corridorCells", () => {
    it("selects the cells a zero-width route crosses, and only those", () => {
        // A line inside one cell: a corridor of no width is still a corridor.
        const route = [
            { lat: CENTRE.lat, lon: SQUARE.minLon + 1000 },
            { lat: CENTRE.lat, lon: SQUARE.maxLon - 1000 },
        ];
        expect(corridorCells(18, route, 0).map(formatCellId)).toEqual(["18/1204/1052"]);
    });

    it("reaches the neighbour once the radius does", () => {
        const route = [{ lat: CENTRE.lat, lon: SQUARE.maxLon - lonUdegFor(1000) }];
        expect(corridorCells(18, route, 500).map(formatCellId)).toEqual(["18/1204/1052"]);
        expect(corridorCells(18, route, 1500).map(formatCellId)).toEqual([
            "18/1204/1052",
            "18/1204/1053",
        ]);
    });

    it("puts the radius where it says it does, to within a percent", () => {
        // A point 5 km west of the cell's west edge, at the cell's own latitude.
        // Bisect for the radius at which the cell first comes in: the projection
        // is the same equirectangular one the module documents, so this pins the
        // constant and the unit handling rather than re-deriving geodesy.
        const point = [{ lat: CENTRE.lat, lon: SQUARE.minLon - lonUdegFor(5000) }];
        const hits = (r: number) => corridorCells(18, point, r).some((c) => formatCellId(c) === "18/1204/1052");
        expect(hits(4000)).toBe(false);
        expect(hits(6000)).toBe(true);
        let lo = 0;
        let hi = 20_000;
        for (let k = 0; k < 40; k++) {
            const mid = (lo + hi) / 2;
            if (hits(mid)) hi = mid;
            else lo = mid;
        }
        expect(hi).toBeGreaterThan(4950);
        expect(hi).toBeLessThan(5050);
    });

    it("buffers a single point into a disc", () => {
        const point = [{ lat: SQUARE.minLat + 500, lon: SQUARE.minLon + 500 }];
        // Small radius: the cell it sits in. Large: the three neighbours across
        // the corner too, since the point is 500 µdeg (~50 m) from both edges.
        expect(corridorCells(18, point, 10).map(formatCellId)).toEqual(["18/1204/1052"]);
        expect(corridorCells(18, point, 2000).map(formatCellId)).toEqual([
            "18/1203/1051",
            "18/1203/1052",
            "18/1204/1051",
            "18/1204/1052",
        ]);
    });

    it("is generous at the coarse band with no extra rule", () => {
        // The same route, the same radius, one band up: whole covering cells,
        // i.e. context beyond the corridor (OBCA §1.2).
        const route = [
            { lat: CENTRE.lat, lon: SQUARE.minLon + 1000 },
            { lat: CENTRE.lat, lon: SQUARE.maxLon - 1000 },
        ];
        const coarse = corridorCells(20, route, 0);
        expect(coarse).toHaveLength(1);
        const square = cellSquare(coarse[0]);
        expect(square.minLat).toBeLessThanOrEqual(SQUARE.minLat);
        expect(square.maxLat).toBeGreaterThanOrEqual(SQUARE.maxLat);
    });

    it("ignores ground the route never approaches", () => {
        const route = [
            { lat: CENTRE.lat, lon: CENTRE.lon },
            { lat: CENTRE.lat + 100, lon: CENTRE.lon + 100 },
        ];
        const ids = corridorCells(18, route, 20_000).map(formatCellId);
        expect(ids).not.toContain("18/1200/1052");
        expect(ids.length).toBeLessThan(10);
    });

    it("follows a route across many cells, in (i, j) order", () => {
        // A diagonal over ~1.5 cells in each axis.
        const route: LatLon[] = [];
        for (let k = 0; k <= 20; k++) {
            route.push({
                lat: SQUARE.minLat + (k * (SQUARE.maxLat - SQUARE.minLat) * 3) / 40,
                lon: SQUARE.minLon + (k * (SQUARE.maxLon - SQUARE.minLon) * 3) / 40,
            });
        }
        const ids = corridorCells(18, route, 0).map(formatCellId);
        expect(ids).toEqual([...ids].sort());
        expect(ids).toContain("18/1204/1052");
        expect(ids).toContain("18/1205/1053");
        // A diagonal touches the off-diagonal cells it actually enters and no
        // more: this is a distance test, not a bbox fill.
        expect(ids).not.toContain("18/1206/1052");
    });

    it("has nothing to say about an empty route", () => {
        expect(corridorCells(18, [], 5000)).toEqual([]);
    });

    it("treats a negative radius as zero", () => {
        const route = [
            { lat: CENTRE.lat, lon: SQUARE.minLon + 1000 },
            { lat: CENTRE.lat, lon: SQUARE.maxLon - 1000 },
        ];
        expect(corridorCells(18, route, -1000).map(formatCellId)).toEqual(["18/1204/1052"]);
    });
});
