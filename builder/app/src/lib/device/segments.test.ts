import { describe, expect, it } from "vitest";

import { cumulativeDistances, type ProfilePoint } from "./elevation";
import { concatSegments, waypointDistanceM } from "./segments";

/** Points ~1.11 km apart along a meridian — a near-uniform axis, like elevation.test.ts's ladder. */
const meridian = (fromLat: number, count: number, lon = 7.82): ProfilePoint[] =>
    Array.from({ length: count }, (_, i) => ({ lat: fromLat + i * 0.01, lon, ele: 100 + i }));

/** One 0.01°-of-latitude step in metres, per the module's own metric. */
const STEP = cumulativeDistances(meridian(48, 2))[1];

describe("concatSegments", () => {
    it("concatenates points in order onto one cumulative axis", () => {
        // Two contiguous stages: the second starts exactly where the first ends.
        const a = meridian(48.0, 3); // 48.00 .. 48.02
        const b = meridian(48.02, 4); // 48.02 .. 48.05
        const axis = concatSegments([a, b]);
        expect(axis.points.length).toBe(7);
        expect(axis.cum).toEqual(cumulativeDistances([...a, ...b]));
        expect(axis.totalM).toBeCloseTo(5 * STEP, 6);
        expect(axis.ranges).toEqual([
            [0, 2],
            [3, 6],
        ]);
    });

    it("gives each stage its offset and drawn length, and marks the seam", () => {
        const a = meridian(48.0, 3); // 2 steps long
        const b = meridian(48.02, 4); // 3 steps long
        const axis = concatSegments([a, b]);
        expect(axis.offsetsM[0]).toBe(0);
        expect(axis.lengthsM[0]).toBeCloseTo(2 * STEP, 6);
        // The seam jump is zero here (contiguous stages), so b starts at a's end.
        expect(axis.offsetsM[1]).toBeCloseTo(2 * STEP, 6);
        expect(axis.lengthsM[1]).toBeCloseTo(3 * STEP, 6);
        expect(axis.boundariesM.length).toBe(1);
        expect(axis.boundariesM[0]).toBeCloseTo(2 * STEP, 6);
    });

    it("keeps a gap between non-contiguous stages on the axis but out of the stage lengths", () => {
        const a = meridian(48.0, 2); // ends at 48.01
        const b = meridian(48.03, 2); // starts 2 steps later
        const axis = concatSegments([a, b]);
        // The axis walks the jump; the second stage's own length is still one step.
        expect(axis.totalM).toBeCloseTo(4 * STEP, 6);
        expect(axis.offsetsM[1]).toBeCloseTo(3 * STEP, 6);
        expect(axis.lengthsM[1]).toBeCloseTo(1 * STEP, 6);
        expect(axis.boundariesM[0]).toBeCloseTo(3 * STEP, 6);
    });

    it("tolerates empty segments without ranges or boundaries", () => {
        const a = meridian(48.0, 3);
        const axis = concatSegments([[], a, []]);
        expect(axis.ranges).toEqual([null, [0, 2], null]);
        expect(axis.offsetsM).toEqual([0, 0, axis.totalM]);
        expect(axis.lengthsM[0]).toBe(0);
        expect(axis.lengthsM[2]).toBe(0);
        expect(axis.boundariesM).toEqual([]);
    });

    it("a single segment is the plain track: no boundaries, full range", () => {
        const a = meridian(48.0, 4);
        const axis = concatSegments([a]);
        expect(axis.ranges).toEqual([[0, 3]]);
        expect(axis.offsetsM).toEqual([0]);
        expect(axis.lengthsM[0]).toBeCloseTo(axis.totalM, 9);
        expect(axis.boundariesM).toEqual([]);
        expect(axis.cum).toEqual(cumulativeDistances(a));
    });

    it("degenerates safely on nothing at all", () => {
        const axis = concatSegments([]);
        expect(axis.points).toEqual([]);
        expect(axis.totalM).toBe(0);
        expect(axis.boundariesM).toEqual([]);
    });
});

describe("waypointDistanceM", () => {
    const axis = concatSegments([meridian(48.0, 3), meridian(48.02, 4)]);

    it("offsets a stage waypoint by everything before its stage", () => {
        expect(waypointDistanceM(axis, 0, 0)).toBe(0);
        expect(waypointDistanceM(axis, 1, 0)).toBeCloseTo(axis.offsetsM[1], 9);
        expect(waypointDistanceM(axis, 1, STEP)).toBeCloseTo(axis.offsetsM[1] + STEP, 6);
    });

    it("clamps into the stage's drawn span — a raw-track overshoot lands on the stage's end", () => {
        expect(waypointDistanceM(axis, 0, 99_999)).toBeCloseTo(axis.lengthsM[0], 6);
        expect(waypointDistanceM(axis, 0, -5)).toBe(0);
        expect(waypointDistanceM(axis, 1, 99_999)).toBeCloseTo(axis.totalM, 6);
    });

    it("passes the raw distance through on a degenerate axis", () => {
        expect(waypointDistanceM(concatSegments([]), 0, 1234)).toBe(1234);
    });
});
