// The logbook map's clustering, pinned as pure math (see rideMap.ts): membership and counts,
// the adjacent-cell merge, the zoom threshold, and the representative point. No Leaflet here —
// the component draws whatever these functions answer, so these answers are the behavior.

import { describe, expect, it } from "vitest";
import {
    CLUSTER_BELOW_ZOOM,
    clusterRides,
    clustersAt,
    projectPx,
    representativePoint,
    type RideTrack,
} from "./rideMap";

/** A tiny synthetic track around a center — enough points for a midpoint and real bounds. */
function trackAround(lat: number, lon: number, key: string, span = 0.02): RideTrack {
    const track: Array<[number, number]> = [];
    for (let i = 0; i < 9; i++) {
        const t = i / 8;
        track.push([lat - span / 2 + span * t, lon + span * Math.sin(t * Math.PI)]);
    }
    return { key, track };
}

const FREIBURG_A = trackAround(47.99, 7.85, "a");
const FREIBURG_B = trackAround(48.01, 7.87, "b");
const KAISERSTUHL = trackAround(48.09, 7.66, "c");
const INNSBRUCK = trackAround(47.27, 11.39, "d");

describe("clustersAt", () => {
    it("clusters below the threshold and shows tracks at and above it", () => {
        expect(clustersAt(CLUSTER_BELOW_ZOOM - 1)).toBe(true);
        expect(clustersAt(CLUSTER_BELOW_ZOOM)).toBe(false);
        expect(clustersAt(CLUSTER_BELOW_ZOOM + 5)).toBe(false);
        expect(clustersAt(2)).toBe(true);
    });
});

describe("representativePoint", () => {
    it("is the track's midpoint by index", () => {
        const odd = [[1, 1], [2, 2], [3, 3]] as const;
        expect(representativePoint(odd)).toEqual([2, 2]);
        const even = [[1, 1], [2, 2], [3, 3], [4, 4]] as const;
        expect(representativePoint(even)).toEqual([3, 3]);
        expect(representativePoint([[5, 6]])).toEqual([5, 6]);
        expect(representativePoint([])).toBeNull();
    });
});

describe("clusterRides", () => {
    it("groups nearby rides and keeps a lone ride its own badge, with counts", () => {
        // Zoom 7: Freiburg and the Kaiserstuhl (~20 km) share a neighborhood of ~60 px cells;
        // Innsbruck (~280 km away) cannot.
        const clusters = clusterRides([FREIBURG_A, FREIBURG_B, KAISERSTUHL, INNSBRUCK], 7);
        expect(clusters).toHaveLength(2);
        const [black_forest, alps] = clusters;
        expect(black_forest.keys).toEqual(["a", "b", "c"]);
        expect(black_forest.count).toBe(3);
        expect(alps.keys).toEqual(["d"]);
        expect(alps.count).toBe(1);
    });

    it("separates the same rides as the zoom grows — the grid is the threshold behavior", () => {
        // At zoom 9 the Kaiserstuhl ride (~15 km from Freiburg) gets its own badge…
        const mid = clusterRides([FREIBURG_A, FREIBURG_B, KAISERSTUHL], 9);
        expect(mid.map((c) => [...c.keys].sort().join(""))).toEqual(["ab", "c"]);
        // …and at zoom 13 every ride stands alone.
        const close = clusterRides([FREIBURG_A, FREIBURG_B, KAISERSTUHL], 13);
        expect(close).toHaveLength(3);
        expect(close.every((c) => c.count === 1)).toBe(true);
    });

    it("merges rides in adjacent cells into one badge", () => {
        // Two representative points straddling a cell edge: same cluster, even though their cells
        // differ — a badge pair two pixels apart would be the bug this rule exists for.
        const zoom = 8;
        const cell = 60;
        const [ax] = projectPx(48.0, 7.85, zoom);
        expect(Math.floor(ax / cell)).not.toBe(Math.floor((ax + cell) / cell));
        // Build a second ride whose midpoint lands one cell to the east.
        const scale = 256 * 2 ** zoom;
        const lonShift = ((cell * 1.001) / scale) * 360;
        const neighbour = trackAround(48.0, 7.85 + lonShift, "n");
        const clusters = clusterRides([trackAround(48.0, 7.85, "m"), neighbour], zoom, cell);
        expect(clusters).toHaveLength(1);
        expect(clusters[0].keys).toEqual(["m", "n"]);
    });

    it("centers a badge between its members and bounds it around every member point", () => {
        const clusters = clusterRides([FREIBURG_A, FREIBURG_B], 7);
        expect(clusters).toHaveLength(1);
        const { center, bounds } = clusters[0];
        const reps = [representativePoint(FREIBURG_A.track)!, representativePoint(FREIBURG_B.track)!];
        expect(center[0]).toBeCloseTo((reps[0][0] + reps[1][0]) / 2, 10);
        expect(center[1]).toBeCloseTo((reps[0][1] + reps[1][1]) / 2, 10);
        // The click-to-zoom bounds cover every point of every member track.
        const [[south, west], [north, east]] = bounds;
        for (const ride of [FREIBURG_A, FREIBURG_B]) {
            for (const [lat, lon] of ride.track) {
                expect(lat).toBeGreaterThanOrEqual(south);
                expect(lat).toBeLessThanOrEqual(north);
                expect(lon).toBeGreaterThanOrEqual(west);
                expect(lon).toBeLessThanOrEqual(east);
            }
        }
    });

    it("skips rides with no track and answers an empty library with no clusters", () => {
        expect(clusterRides([], 7)).toEqual([]);
        expect(clusterRides([{ key: "x", track: [] }], 7)).toEqual([]);
        const clusters = clusterRides([{ key: "x", track: [] }, FREIBURG_A], 7);
        expect(clusters).toHaveLength(1);
        expect(clusters[0].keys).toEqual(["a"]);
    });

    it("projects north up and east right, like the map it feeds", () => {
        const [x1, y1] = projectPx(48, 7, 7);
        const [x2, y2] = projectPx(49, 8, 7);
        expect(x2).toBeGreaterThan(x1);
        expect(y2).toBeLessThan(y1); // pixel y grows southwards
        // The poles clamp instead of diverging.
        expect(Number.isFinite(projectPx(90, 0, 7)[1])).toBe(true);
    });
});
