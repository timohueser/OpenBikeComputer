import { describe, expect, it } from "vitest";

import {
    FULL_WINDOW,
    MIN_WINDOW_SPAN,
    clampWindow,
    cumulativeDistances,
    elevationProfile,
    isFullWindow,
    nearestPointIndex,
    panWindow,
    pointAtDistance,
    windowIndexRange,
    zoomWindow,
} from "./elevation";

const climb = [
    { lat: 48.0, lon: 7.82, ele: 236 },
    { lat: 48.01, lon: 7.83, ele: 400 },
    { lat: 48.02, lon: 7.84, ele: 320 },
];

/** Four points spaced ~1.11 km apart along a meridian — a near-uniform distance axis. */
const ladder = [
    { lat: 48.0, lon: 7.82, ele: 100 },
    { lat: 48.01, lon: 7.82, ele: 200 },
    { lat: 48.02, lon: 7.82, ele: 300 },
    { lat: 48.03, lon: 7.82, ele: 400 },
];

describe("elevationProfile", () => {
    it("draws a climb: paths inside the box, extremes reported, distance positive", () => {
        const profile = elevationProfile(climb, 600, 64);
        expect(profile).not.toBeNull();
        expect(profile!.minEle).toBe(236);
        expect(profile!.maxEle).toBe(400);
        expect(profile!.totalM).toBeGreaterThan(1000);
        expect(profile!.startM).toBe(0);
        expect(profile!.endM).toBe(profile!.totalM);
        expect(profile!.linePath.startsWith("M")).toBe(true);
        expect(profile!.areaPath.endsWith("Z")).toBe(true);
        // The peak maps to the top of the box (pad), the lowest point to the bottom.
        const ys = profile!.linePath.match(/ (\d+\.\d)/g)!.map(Number);
        expect(Math.min(...ys)).toBeCloseTo(2, 0);
        expect(Math.max(...ys)).toBeCloseTo(62, 0);
    });

    it("draws a flat track mid-box rather than refusing it", () => {
        const flat = climb.map((p) => ({ ...p, ele: 100 }));
        const profile = elevationProfile(flat, 600, 64);
        expect(profile).not.toBeNull();
        expect(profile!.minEle).toBe(100);
        expect(profile!.maxEle).toBe(100);
    });

    it("returns null where nothing honest can be drawn", () => {
        expect(elevationProfile([], 600, 64)).toBeNull();
        expect(elevationProfile([climb[0]], 600, 64)).toBeNull();
        expect(elevationProfile(climb.map((p) => ({ ...p, ele: null })), 600, 64)).toBeNull();
        // Two points at the same place: no distance axis.
        expect(elevationProfile([climb[0], climb[0]], 600, 64)).toBeNull();
    });

    it("keeps elevation-less points on the distance axis but out of the drawn series", () => {
        const gappy = [climb[0], { lat: 48.005, lon: 7.825, ele: null }, climb[1], climb[2]];
        const profile = elevationProfile(gappy, 600, 64);
        expect(profile).not.toBeNull();
        expect(profile!.maxEle).toBe(400);
        // The gap point advances the axis: the total matches the all-points cumulative distance,
        // not a collapsed one.
        const cum = cumulativeDistances(gappy);
        expect(profile!.totalM).toBeCloseTo(cum[cum.length - 1], 6);
    });

    describe("windowed", () => {
        it("redraws only the window, with its own extremes and metre bounds", () => {
            const full = elevationProfile(ladder, 600, 64)!;
            const half = elevationProfile(ladder, 600, 64, [0.5, 1])!;
            expect(half.startM).toBeCloseTo(full.totalM / 2, 6);
            expect(half.endM).toBeCloseTo(full.totalM, 6);
            expect(half.totalM).toBeCloseTo(full.totalM, 6);
            // Only the back half's elevations: min is the interpolated 250 m at the cut.
            expect(half.minEle).toBeCloseTo(250, 0);
            expect(half.maxEle).toBe(400);
            // The redrawn path spans the whole box, not a squeezed half of it.
            const xs = half.linePath.match(/[ML](\d+\.\d)/g)!.map((m) => Number(m.slice(1)));
            expect(Math.min(...xs)).toBeCloseTo(2, 0);
            expect(Math.max(...xs)).toBeCloseTo(598, 0);
        });

        it("interpolates the elevation where the window cuts between samples", () => {
            // A window strictly inside the middle segment: both edges are interpolants.
            const profile = elevationProfile(ladder, 600, 64, [0.4, 0.6])!;
            expect(profile.minEle).toBeCloseTo(220, 0);
            expect(profile.maxEle).toBeCloseTo(280, 0);
        });

        it("returns null when the window holds no drawable elevation", () => {
            const headOnly = [
                { lat: 48.0, lon: 7.82, ele: 100 },
                { lat: 48.01, lon: 7.82, ele: 200 },
                { lat: 48.02, lon: 7.82, ele: null },
                { lat: 48.03, lon: 7.82, ele: null },
            ];
            expect(elevationProfile(headOnly, 600, 64, [0.75, 1])).toBeNull();
        });
    });
});

describe("cumulativeDistances", () => {
    it("starts at zero, one entry per point, monotonic", () => {
        const cum = cumulativeDistances(ladder);
        expect(cum.length).toBe(4);
        expect(cum[0]).toBe(0);
        for (let i = 1; i < cum.length; i++) expect(cum[i]).toBeGreaterThan(cum[i - 1]);
        // ~1.11 km per 0.01° of latitude.
        expect(cum[3]).toBeGreaterThan(3200);
        expect(cum[3]).toBeLessThan(3500);
    });

    it("is empty for no points and [0] for one", () => {
        expect(cumulativeDistances([])).toEqual([]);
        expect(cumulativeDistances([ladder[0]])).toEqual([0]);
    });
});

describe("clampWindow", () => {
    it("orders and clamps a dragged pair", () => {
        expect(clampWindow(0.8, 0.2)).toEqual([0.2, 0.8]);
        expect(clampWindow(-0.5, 1.5)).toEqual([0, 1]);
    });

    it("grows a degenerate drag to the minimum span, kept inside the track", () => {
        const [t0, t1] = clampWindow(0.5, 0.5);
        expect(t1 - t0).toBeCloseTo(MIN_WINDOW_SPAN, 9);
        expect(t0).toBeCloseTo(0.5 - MIN_WINDOW_SPAN / 2, 9);
        expect(clampWindow(0, 0)).toEqual([0, MIN_WINDOW_SPAN]);
        expect(clampWindow(1, 1)).toEqual([1 - MIN_WINDOW_SPAN, 1]);
    });
});

describe("zoomWindow", () => {
    it("zooms in about the anchor, keeping the ground under it still", () => {
        const [t0, t1] = zoomWindow([0, 1], 0.5, 0.5);
        expect(t1 - t0).toBeCloseTo(0.5, 9);
        expect((0.5 - t0) / (t1 - t0)).toBeCloseTo(0.5, 9);
        // An off-centre anchor keeps its relative position.
        const [u0, u1] = zoomWindow([0, 1], 0.5, 0.2);
        expect((0.2 - u0) / (u1 - u0)).toBeCloseTo(0.2, 9);
    });

    it("never inverts, undershoots the minimum span, or leaves the track", () => {
        const tiny = zoomWindow([0.4, 0.4 + MIN_WINDOW_SPAN], 0.1, 0.405);
        expect(tiny[1] - tiny[0]).toBeCloseTo(MIN_WINDOW_SPAN, 9);
        const out = zoomWindow([0.9, 1], 4, 0.99);
        expect(out[0]).toBeGreaterThanOrEqual(0);
        expect(out[1]).toBeLessThanOrEqual(1);
        expect(zoomWindow([0, 1], 3, 0.5)).toEqual([0, 1]);
    });
});

describe("windowIndexRange", () => {
    // Four points, three equal segments: cum = [0, 1, 2, 3].
    const cum = [0, 1, 2, 3];

    it("covers the whole track at the full window", () => {
        expect(windowIndexRange(cum, FULL_WINDOW)).toEqual([0, 3]);
    });

    it("rounds outward so the highlight covers the window", () => {
        // [0.5, 0.83] in distance = [1.5 m, 2.5 m]: needs points 1..3 to cover.
        expect(windowIndexRange(cum, [0.5, 5 / 6])).toEqual([1, 3]);
        // Exactly on vertices: no widening.
        expect(windowIndexRange(cum, [1 / 3, 2 / 3])).toEqual([1, 2]);
    });

    it("degenerates safely", () => {
        expect(windowIndexRange([], FULL_WINDOW)).toEqual([0, 0]);
        expect(windowIndexRange([0], FULL_WINDOW)).toEqual([0, 0]);
    });
});

describe("panWindow", () => {
    it("slides without resizing", () => {
        const [t0, t1] = panWindow([0.2, 0.4], 0.3);
        expect(t0).toBeCloseTo(0.5, 9);
        expect(t1).toBeCloseTo(0.7, 9);
    });

    it("clamps at both ends, span preserved", () => {
        expect(panWindow([0.2, 0.4], -1)).toEqual([0, 0.2]);
        const [t0, t1] = panWindow([0.2, 0.4], 1);
        expect(t0).toBeCloseTo(0.8, 9);
        expect(t1).toBeCloseTo(1, 9);
        // The full window has nowhere to go.
        expect(panWindow([0, 1], 0.5)).toEqual([0, 1]);
    });
});

describe("pointAtDistance", () => {
    const cum = cumulativeDistances(ladder);

    it("lands on vertices at their exact distances", () => {
        expect(pointAtDistance(ladder, cum, 0)).toEqual({ lat: 48.0, lon: 7.82 });
        expect(pointAtDistance(ladder, cum, cum[2])).toEqual({ lat: 48.02, lon: 7.82 });
        expect(pointAtDistance(ladder, cum, cum[3])).toEqual({ lat: 48.03, lon: 7.82 });
    });

    it("interpolates within a segment", () => {
        const mid = pointAtDistance(ladder, cum, (cum[1] + cum[2]) / 2)!;
        expect(mid.lat).toBeCloseTo(48.015, 6);
        expect(mid.lon).toBeCloseTo(7.82, 9);
    });

    it("clamps past the ends and degenerates safely", () => {
        expect(pointAtDistance(ladder, cum, -5)).toEqual({ lat: 48.0, lon: 7.82 });
        expect(pointAtDistance(ladder, cum, cum[3] + 500)).toEqual({ lat: 48.03, lon: 7.82 });
        expect(pointAtDistance([], [], 0)).toBeNull();
        expect(pointAtDistance([ladder[0]], [0], 10)).toEqual({ lat: 48.0, lon: 7.82 });
        // A mismatched axis is a caller bug, answered with null rather than a wrong point.
        expect(pointAtDistance(ladder, [0, 1], 0.5)).toBeNull();
    });
});

describe("nearestPointIndex", () => {
    it("finds the closest vertex, ends included", () => {
        expect(nearestPointIndex(ladder, 48.001, 7.82)).toBe(0);
        expect(nearestPointIndex(ladder, 48.014, 7.821)).toBe(1);
        expect(nearestPointIndex(ladder, 48.9, 7.82)).toBe(3);
    });

    it("is -1 for an empty track", () => {
        expect(nearestPointIndex([], 48, 7.82)).toBe(-1);
    });
});

describe("isFullWindow", () => {
    it("tells the reset state from a zoom", () => {
        expect(isFullWindow(FULL_WINDOW)).toBe(true);
        expect(isFullWindow([0, 0.999])).toBe(false);
        expect(isFullWindow([0.001, 1])).toBe(false);
    });
});
