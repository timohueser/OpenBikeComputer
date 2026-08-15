/**
 * What the panel says about a dropped route, held to the checked-in OBCR vector.
 *
 * The numbers shown before a route is sent are read out of the header the converter produced, so
 * the guard that matters is that this reader agrees with `specs/vectors/manifest.json` — the
 * same record the firmware and the iOS app are pinned to. A distance read from the wrong offset
 * would look perfectly plausible on screen.
 */

import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

import { RouteError, decodeRouteHeader, routeNameFrom } from "./route";

function repoRoot(): string {
    let dir = dirname(fileURLToPath(import.meta.url));
    for (let up = 0; up < 12; up++) {
        if (existsSync(join(dir, "specs", "vectors", "manifest.json"))) return dir;
        dir = dirname(dir);
    }
    throw new Error("could not locate the repo root from " + import.meta.url);
}

const ROOT = repoRoot();
const vector = (name: string) => new Uint8Array(readFileSync(join(ROOT, "specs/vectors", name)));

describe("decodeRouteHeader", () => {
    it("reads the vector's header exactly as the manifest records it", () => {
        const header = decodeRouteHeader(vector("route-waypoints.obcr"));
        expect(header).toMatchObject({
            version: 3,
            name: "Vector Loop",
            pointCount: 9,
            distanceM: 2207,
            ascentM: 76,
        });
    });

    it("reads a waypoint-free route the same way", () => {
        // §1: the waypoint section is reached by an explicit offset and never by the ride path —
        // the two fixtures are the same ride, so every stat must match.
        const withWaypoints = decodeRouteHeader(vector("route-waypoints.obcr"));
        const plain = decodeRouteHeader(vector("route-plain.obcr"));
        expect(plain.distanceM).toBe(withWaypoints.distanceM);
        expect(plain.pointCount).toBe(withWaypoints.pointCount);
    });

    it("refuses something that is not a route", () => {
        expect(() => decodeRouteHeader(vector("update-container-v1.bin"))).toThrow(RouteError);
        expect(() => decodeRouteHeader(new Uint8Array(20))).toThrow(RouteError);
    });
});

describe("routeNameFrom", () => {
    it("uses the file's stem", () => {
        expect(routeNameFrom("Schauinsland loop.gpx")).toBe("Schauinsland loop");
        expect(routeNameFrom("/tmp/rides/day one.GPX")).toBe("day one");
        expect(routeNameFrom(".gpx")).toBe("Route");
    });

    it("trims to the format's 48 bytes without splitting a codepoint", () => {
        const name = routeNameFrom("ü".repeat(40) + ".gpx");
        expect(new TextEncoder().encode(name).length).toBeLessThanOrEqual(48);
        expect(name).toBe("ü".repeat(24));
    });
});
