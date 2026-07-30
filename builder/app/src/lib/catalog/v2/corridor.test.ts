// Corridor buffering: the cells within R of a route.
//
// The interesting assertions here are the *boundary* ones — a cell just inside
// the radius and the same cell just outside it — because that is where an error
// in the metre-per-microdegree conversion or in the segment-to-rectangle
// distance would show, and nowhere else. A factor-of-1e6 slip does not make the
// result subtly wrong; it makes it select the world or nothing.

import { describe, expect, it } from "vitest";
import { corridorCells, M_PER_DEG, type LatLon } from "./corridor";
import { cellsIntersecting, cellSize, cellSquare, formatCellId, parseCellId, type CellId } from "./grid";

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

// --- the prefilter must never decide anything -------------------------------
//
// `corridorCells` is two tests in a trench coat: a cheap box prefilter that
// picks candidates, and the exact segment-to-rectangle distance that decides.
// The whole design only holds if the first is a superset of the second — a
// candidate that is never generated is never tested, never selected, and never
// reported as a hole either, because the missing lists are built from cells that
// *were* selected. So the prefilter dropping one cell is a silent hole in the
// corridor, and that is what the two suites below stand guard over.

/** The module's own per-cell acceptance test, spelled out here with no
 *  prefilter in front of it: the oracle the real function must agree with. */
function acceptsCell(cell: CellId, points: readonly LatLon[], radiusM: number): boolean {
    const square = cellSquare(cell);
    const lat0 = (square.minLat + square.maxLat) / 2;
    const lon0 = (square.minLon + square.maxLon) / 2;
    const sx = (M_PER_DEG / 1e6) * Math.max(Math.cos((lat0 / 1e6) * (Math.PI / 180)), Math.cos((85 * Math.PI) / 180));
    const sy = M_PER_DEG / 1e6;
    const r2 = Math.max(radiusM, 0) ** 2;
    const x = (lon: number) => (lon - lon0) * sx;
    const y = (lat: number) => (lat - lat0) * sy;
    const minX = x(square.minLon);
    const maxX = x(square.maxLon);
    const minY = y(square.minLat);
    const maxY = y(square.maxLat);

    const dist2 = (ax: number, ay: number, bx: number, by: number): number => {
        const toRect = (px: number, py: number) => {
            const dx = Math.max(minX - px, 0, px - maxX);
            const dy = Math.max(minY - py, 0, py - maxY);
            return dx * dx + dy * dy;
        };
        const toSeg = (px: number, py: number) => {
            const vx = bx - ax;
            const vy = by - ay;
            const len2 = vx * vx + vy * vy;
            const t = len2 > 0 ? Math.min(1, Math.max(0, ((px - ax) * vx + (py - ay) * vy) / len2)) : 0;
            return (px - (ax + t * vx)) ** 2 + (py - (ay + t * vy)) ** 2;
        };
        let best = Math.min(toRect(ax, ay), toRect(bx, by));
        for (const [cx, cy] of [
            [minX, minY],
            [maxX, minY],
            [maxX, maxY],
            [minX, maxY],
        ]) {
            best = Math.min(best, toSeg(cx, cy));
        }
        return best;
    };

    const segments: [LatLon, LatLon][] = points.length === 1 ? [[points[0], points[0]]] : [];
    for (let k = 1; k < points.length; k++) segments.push([points[k - 1], points[k]]);
    return segments.some(([a, b]) => dist2(x(a.lon), y(a.lat), x(b.lon), y(b.lat)) <= r2);
}

/**
 * Brute force: every cell in a deliberately over-wide box, tested exactly.
 *
 * The box is padded by the latitude reach **and by `1/cos(85°)` of it** — the
 * largest longitude reach any latitude can ask for — plus two whole cells, so it
 * cannot itself be the thing that drops a cell.
 */
function exactCorridorCells(log2: number, points: readonly LatLon[], radiusM: number): string[] {
    const lats = points.map((p) => p.lat);
    const lons = points.map((p) => p.lon);
    const dLat = Math.ceil(Math.max(radiusM, 0) / (M_PER_DEG / 1e6)) + 2 * cellSize(log2);
    const dLon = Math.ceil(dLat / Math.cos((85 * Math.PI) / 180)) + 2 * cellSize(log2);
    return cellsIntersecting(log2, {
        minLat: Math.min(...lats) - dLat,
        minLon: Math.min(...lons) - dLon,
        maxLat: Math.max(...lats) + dLat,
        maxLon: Math.max(...lons) + dLon,
    })
        .filter((c) => acceptsCell(c, points, radiusM))
        .map(formatCellId);
}

/** mulberry32 — deterministic, so a red run names a vector rather than a mood. */
function seededRandom(seed: number): () => number {
    let s = seed;
    return () => {
        s = (s + 0x6d2b_79f5) | 0;
        let t = Math.imul(s ^ (s >>> 15), 1 | s);
        t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
        return ((t ^ (t >>> 14)) >>> 0) / 4_294_967_296;
    };
}

describe("the candidate prefilter is never the decision", () => {
    it("keeps a cell whose own latitude reaches further than the route's", () => {
        // The regression vector. The route sits 1 µdeg south of coarse cell
        // 20/0300/0263 and 65 100 µdeg west of it. The cell's centre is half a
        // cell (~0.52°) further north, where a degree of longitude is ~1 %
        // shorter — so 65 100 µdeg is inside 5 km measured at the *cell*, and
        // outside 5 km measured at the *route*. Padding the candidate box with
        // the route's latitude dropped this cell across a ~624 µdeg (≈ 48 m)
        // window, with nothing anywhere reporting a hole.
        const cell = cellSquare(parseCellId("20/0300/0263"));
        const route = [{ lat: cell.minLat - 1, lon: cell.minLon - 65_100 }];
        expect(acceptsCell(parseCellId("20/0300/0263"), route, 5000)).toBe(true);
        expect(corridorCells(20, route, 5000).map(formatCellId)).toContain("20/0300/0263");
        expect(corridorCells(20, route, 5000).map(formatCellId)).toEqual(
            exactCorridorCells(20, route, 5000),
        );
    });

    it("agrees with the brute-force test across the whole window that used to fail", () => {
        const cell = cellSquare(parseCellId("20/0300/0263"));
        for (let delta = 64_000; delta < 66_500; delta += 7) {
            const route = [{ lat: cell.minLat - 1, lon: cell.minLon - delta }];
            expect(corridorCells(20, route, 5000).map(formatCellId)).toEqual(
                exactCorridorCells(20, route, 5000),
            );
        }
    });

    it("agrees with the brute-force test on 400 pseudo-random routes", () => {
        // Seeded, so a failure is a vector someone can paste into the test above
        // rather than a story about a flake. Latitudes run from the far south to
        // the high arctic on purpose: the bug this pins is a cos(latitude) one,
        // and it is invisible at the equator.
        const rnd = seededRandom(0x1657_1030);
        const between = (lo: number, hi: number) => Math.round(lo + rnd() * (hi - lo));

        for (let n = 0; n < 400; n++) {
            const log2 = rnd() < 0.5 ? 18 : 20;
            const s = cellSize(log2);
            const lat = between(-55_000_000, 78_000_000);
            const lon = between(-170_000_000, 170_000_000);
            const points: LatLon[] = [];
            for (let k = 0, kn = 1 + Math.floor(rnd() * 4); k < kn; k++) {
                points.push({ lat: lat + between(-2 * s, 2 * s), lon: lon + between(-2 * s, 2 * s) });
            }
            const radius = between(0, 20_000);
            expect(
                corridorCells(log2, points, radius).map(formatCellId),
                `log2 ${log2}, radius ${radius} m, route ${JSON.stringify(points)}`,
            ).toEqual(exactCorridorCells(log2, points, radius));
        }
    });

    it("agrees at the acceptance boundary itself, on both sides of the equator", () => {
        // Uniformly random routes almost never land in the ~1 % of longitude
        // where the two latitudes disagree, so this probe aims straight at it:
        // a point one µdeg off a cell's *equatorward* edge, at the longitude
        // offset that puts the cell's near corner within a hair of the radius,
        // jittered either side of it. That is the geometry the fringe bug lived
        // in, and it is worth ~600 µdeg of longitude — about 48 m — per cell.
        const rnd = seededRandom(0x0300_0263);
        const mPerUdeg = M_PER_DEG / 1e6;
        for (let n = 0; n < 300; n++) {
            const log2 = rnd() < 0.5 ? 18 : 20;
            const axis = 2 ** 29 / cellSize(log2);
            // Cells covering roughly ±75° of latitude, any longitude.
            const span = Math.floor((75_000_000 / 2 ** 29) * axis);
            const i = Math.floor(axis / 2) + Math.floor((rnd() * 2 - 1) * span);
            const j = Math.floor(rnd() * axis);
            const square = cellSquare({ log2, i, j });
            const centreLat = (square.minLat + square.maxLat) / 2;
            const north = centreLat > 0;
            const radius = 500 + Math.round(rnd() * 19_500);
            // What the cell's own latitude admits, in µdeg of longitude.
            const need = radius / (mPerUdeg * Math.cos((centreLat / 1e6) * (Math.PI / 180)));
            for (const jitter of [-900, -300, 0, 300]) {
                const route = [
                    {
                        lat: north ? square.minLat - 1 : square.maxLat + 1,
                        lon: square.minLon - Math.round(need + jitter),
                    },
                ];
                expect(
                    corridorCells(log2, route, radius).map(formatCellId),
                    `log2 ${log2}, cell ${i}/${j}, radius ${radius} m, jitter ${jitter}`,
                ).toEqual(exactCorridorCells(log2, route, radius));
            }
        }
    });
});
