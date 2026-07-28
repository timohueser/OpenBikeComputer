import { describe, expect, it } from "vitest";

import { elevationProfile } from "./elevation";

const climb = [
    { lat: 48.0, lon: 7.82, ele: 236 },
    { lat: 48.01, lon: 7.83, ele: 400 },
    { lat: 48.02, lon: 7.84, ele: 320 },
];

describe("elevationProfile", () => {
    it("draws a climb: paths inside the box, extremes reported, distance positive", () => {
        const profile = elevationProfile(climb, 600, 64);
        expect(profile).not.toBeNull();
        expect(profile!.minEle).toBe(236);
        expect(profile!.maxEle).toBe(400);
        expect(profile!.distanceM).toBeGreaterThan(1000);
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

    it("skips elevation-less points but keeps the rest", () => {
        const gappy = [climb[0], { lat: 48.005, lon: 7.825, ele: null }, climb[1], climb[2]];
        const profile = elevationProfile(gappy, 600, 64);
        expect(profile).not.toBeNull();
        expect(profile!.maxEle).toBe(400);
    });
});
