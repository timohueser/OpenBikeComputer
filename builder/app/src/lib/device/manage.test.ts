/**
 * Rename and trip editing: byte-level checks for the name rewrite (the one place this code edits a
 * format by hand), loopback round-trips for the replace-at-same-id semantics everything rests on.
 */

import { describe, expect, it } from "vitest";

import {
    addStage,
    createTrip,
    moveStage,
    removeStage,
    renameRoute,
    renameRouteBytes,
    updateTrip,
} from "./manage";
import { decodeRouteHeader } from "./route";
import { decodeTripObject, encodeTripObject, type RouteListEntry, type TripObject } from "../usb/objects";
import { Crc32 } from "../usb/crc32";
import { loopbackDevice } from "../usb/loopback";
import { ObjectType } from "../usb/protocol";

/** A minimal, valid OBCR v1: 112-byte header + no points — enough for the header codec. */
function obcrWithName(name: string): Uint8Array {
    const out = new Uint8Array(112);
    const view = new DataView(out.buffer);
    view.setUint32(0, 0x4f424352, false); // "OBCR"
    out[4] = 1; // version
    const bytes = new TextEncoder().encode(name);
    out[6] = bytes.length;
    out.set(bytes, 64);
    return out;
}

function routeEntry(objectId: number, name: string, bytes: Uint8Array): RouteListEntry {
    return {
        objectId,
        byteLen: bytes.length,
        distanceM: 0,
        ascentM: 0,
        pointCount: 0,
        waypointCount: 0,
        name,
        crc32: Crc32.of(bytes),
        expiresAt: 0,
        retention: 0,
    };
}

describe("renameRouteBytes", () => {
    it("rewrites the name and only the name", () => {
        const original = obcrWithName("Old name");
        original[100 + 12] = 0; // (offset 112 is the end; touch nothing)
        const renamed = renameRouteBytes(original, "Grimsel – Furka");
        expect(decodeRouteHeader(renamed).name).toBe("Grimsel – Furka");
        // Everything outside the name field is untouched…
        expect(renamed.subarray(0, 6)).toEqual(original.subarray(0, 6));
        expect(renamed.subarray(7, 64)).toEqual(original.subarray(7, 64));
        // …and the input was not mutated.
        expect(decodeRouteHeader(original).name).toBe("Old name");
    });

    it("zero-pads: a shorter name leaves no tail of the longer one", () => {
        const renamed = renameRouteBytes(obcrWithName("A very long route name here"), "Short");
        expect(decodeRouteHeader(renamed).name).toBe("Short");
        expect(renamed.subarray(64 + 5, 64 + 48).every((b) => b === 0)).toBe(true);
    });

    it("caps at 48 bytes without splitting a codepoint, and refuses non-routes", () => {
        const long = "Ü".repeat(30); // 60 bytes of UTF-8
        const renamed = renameRouteBytes(obcrWithName("x"), long);
        const name = decodeRouteHeader(renamed).name;
        expect(new TextEncoder().encode(name).length).toBeLessThanOrEqual(48);
        expect(name).toBe("Ü".repeat(24));
        expect(() => renameRouteBytes(new Uint8Array(200), "x")).toThrow(/not an OBCR/);
    });

    it("falls back rather than writing an empty name", () => {
        expect(decodeRouteHeader(renameRouteBytes(obcrWithName("x"), "   ")).name).toBe("Route");
    });
});

describe("stage mutators", () => {
    const trip: TripObject = { name: "T", stages: [1, 2, 3] };

    it("add dedupes, remove drops by index, move clamps", () => {
        expect(addStage(trip, 2)).toBe(trip);
        expect(addStage(trip, 4).stages).toEqual([1, 2, 3, 4]);
        expect(removeStage(trip, 1).stages).toEqual([1, 3]);
        expect(moveStage(trip, 0, 1).stages).toEqual([2, 1, 3]);
        expect(moveStage(trip, 2, 5).stages).toEqual([1, 2, 3]);
        expect(moveStage(trip, 0, -1)).toBe(trip);
    });
});

describe("against the loopback device", () => {
    async function withDevice(body: (ctx: ReturnType<typeof loopbackDevice>) => Promise<void>) {
        const ctx = loopbackDevice({});
        try {
            await body(ctx);
            expect(ctx.device.faults).toEqual([]);
        } finally {
            await ctx.close();
        }
    }

    it("renames a route in place: same id, new name, retention row intact", async () => {
        await withDevice(async ({ client, device }) => {
            const bytes = obcrWithName("Before");
            device.seedRoute(routeEntry(5, "Before", bytes), bytes);

            await renameRoute(client, 5, "After");

            const stored = await client.download(ObjectType.Route, 5);
            expect(decodeRouteHeader(stored).name).toBe("After");
            const { entries } = await client.listRoutes();
            expect(entries.map((e) => e.objectId)).toEqual([5]);
        });
    });

    it("creates, edits and reorders a trip through replace-at-same-id", async () => {
        await withDevice(async ({ client }) => {
            const id = await createTrip(client, "  Tour du Mont Blanc  ", [3, 1]);

            let stored = decodeTripObject(await client.download(ObjectType.Trip, id));
            expect(stored).toEqual({ name: "Tour du Mont Blanc", stages: [3, 1] });

            await updateTrip(client, id, (t) => addStage(t, 7));
            await updateTrip(client, id, (t) => moveStage(t, 2, -2));
            stored = decodeTripObject(await client.download(ObjectType.Trip, id));
            expect(stored.stages).toEqual([7, 3, 1]);

            const { entries } = await client.listTrips();
            expect(entries.map((e) => e.objectId)).toEqual([id]);
        });
    });

    it("uploading identical trip bytes twice converges on one object", async () => {
        // The CRC dedupe the multi-drop flow leans on (fresh upload, matching
        // length+CRC → the existing id comes back and nothing is stored twice).
        await withDevice(async ({ client }) => {
            const bytes = encodeTripObject({ name: "Twin", stages: [1, 2] });
            const first = await client.upload(ObjectType.Trip, 0xffff, bytes);
            const second = await client.upload(ObjectType.Trip, 0xffff, bytes);
            expect(second.objectId).toBe(first.objectId);
            expect((await client.listTrips()).entries).toHaveLength(1);
        });
    });
});
