import { describe, expect, it } from "vitest";
import type { RegionFeature } from "../api/client";
import { featureContains, indexRegions, regionsForBbox, smallestLeafAt } from "./geo";

function region(
    id: string,
    ring: number[][],
    has_children = false,
    holes: number[][][] = [],
): RegionFeature {
    return {
        type: "Feature",
        properties: { id, name: id, parent: null, has_children },
        geometry: { type: "Polygon", coordinates: [ring, ...holes] },
    };
}

const square = (x0: number, y0: number, x1: number, y1: number) => [
    [x0, y0],
    [x1, y0],
    [x1, y1],
    [x0, y1],
    [x0, y0],
];

// A parent spanning [0,0]-[20,10] with two leaf children tiling it, plus a
// triangle whose bbox is much larger than its shape (for the fallback path).
const fixtures = indexRegions([
    region("parent", square(0, 0, 20, 10), true),
    region("west", square(0, 0, 10, 10)),
    region("east", square(10, 0, 20, 10)),
    region("donut", square(30, 0, 40, 10), false, [square(33, 3, 37, 7)]),
    region("tri", [
        [50, 0],
        [60, 0],
        [60, 10],
        [50, 0],
    ]),
]);

describe("featureContains", () => {
    it("hits inside, misses outside and inside holes", () => {
        const donut = fixtures.find((f) => f.properties.id === "donut")!;
        expect(featureContains(donut, 31, 5)).toBe(true);
        expect(featureContains(donut, 35, 5)).toBe(false); // in the hole
        expect(featureContains(donut, 45, 5)).toBe(false);
    });
});

describe("smallestLeafAt", () => {
    it("skips parents and picks the leaf", () => {
        expect(smallestLeafAt(fixtures, 5, 5)?.properties.id).toBe("west");
        expect(smallestLeafAt(fixtures, 15, 5)?.properties.id).toBe("east");
        expect(smallestLeafAt(fixtures, 25, 5)).toBeNull(); // open sea
    });
});

describe("regionsForBbox", () => {
    it("returns just the containing leaf for a box inside one region", () => {
        const regs = regionsForBbox(fixtures, [2, 2, 8, 8]);
        expect(regs.map((r) => r.properties.id)).toEqual(["west"]);
    });

    it("returns both leaves for a border-spanning box, never the parent", () => {
        const ids = regionsForBbox(fixtures, [6, 2, 14, 8]).map((r) => r.properties.id);
        expect(ids.sort()).toEqual(["east", "west"]);
    });

    it("falls back to the smallest bbox-overlapping leaf for an all-sea box", () => {
        // The triangle's bbox covers its empty upper-left half: a box there
        // samples no land but still bbox-overlaps the region.
        const ids = regionsForBbox(fixtures, [50.1, 8, 52, 9.9]).map((r) => r.properties.id);
        expect(ids).toEqual(["tri"]);
    });

    it("returns nothing for open sea far from any region", () => {
        expect(regionsForBbox(fixtures, [100, 40, 110, 50])).toEqual([]);
    });
});
