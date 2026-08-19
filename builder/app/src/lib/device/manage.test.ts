/**
 * Rename and trip editing: byte-level checks for the name rewrite (the one place this code edits a
 * format by hand), loopback round-trips for the replace-at-same-id semantics everything rests on.
 *
 * Both mutations are a `PUT` naming an existing object (§3.6), so the two properties worth pinning
 * against a real device are that the replace keeps the `ObjectId` — every reference to the route
 * survives — and that it carries the revision it expects, so a listing something else has already
 * overtaken fails the compare-and-swap instead of clobbering.
 */

import { describe, expect, it } from "vitest";

import {
    MAX_TRIP_STAGE_ID,
    TripStageError,
    addStage,
    createTrip,
    moveStage,
    removeStage,
    renameRoute,
    renameRouteBytes,
    stageId,
    updateTrip,
} from "./manage";
import { decodeRouteHeader } from "./route";
import { decodeTripObject, type TripObject } from "../usb/objects";
import { loopbackDevice } from "../usb/loopback";
import { EntryFlags, ObjectKind } from "../usb/protocol";

/** A minimal, valid OBCR: the 128-byte header + no points — enough for the header codec. */
function obcrWithName(name: string): Uint8Array {
    const out = new Uint8Array(128);
    const view = new DataView(out.buffer);
    view.setUint32(0, 0x4f424352, false); // "OBCR"
    out[4] = 3; // version
    const bytes = new TextEncoder().encode(name);
    out[6] = bytes.length;
    out.set(bytes, 64);
    return out;
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

describe("stageId", () => {
    it("refuses an ObjectId the trip format cannot name", () => {
        // A trip stores its stages as `u16` while an `ObjectId` is a `u64` from a cursor that is
        // never reused (`FLAT_Store_Format.md` §3), so a long-lived card can hold routes no trip
        // object can reference. Writing a truncated id would name a *different* route, so the only
        // honest answer is to refuse.
        expect(stageId(1n)).toBe(1);
        expect(stageId(BigInt(MAX_TRIP_STAGE_ID))).toBe(MAX_TRIP_STAGE_ID);
        expect(() => stageId(BigInt(MAX_TRIP_STAGE_ID) + 1n)).toThrow(TripStageError);
        expect(() => stageId(0n)).toThrow(TripStageError);
    });
});

describe("against the loopback device", () => {
    async function withDevice(body: (rig: ReturnType<typeof loopbackDevice>) => Promise<void>) {
        const rig = loopbackDevice({});
        try {
            await body(rig);
            expect(rig.device.faults, "the mock device recorded a non-transport fault").toEqual([]);
        } finally {
            await rig.close();
        }
    }

    it("renames in both places: the OBCR header and §3.6's display name", async () => {
        // The two fields have different readers — the header's name is what the device shows while
        // navigating, the display name is what a catalog listing shows — so writing only one of them
        // would make the device and the device page disagree about what a route is called.
        await withDevice(async ({ client, device }) => {
            const seeded = device.seed({ kind: ObjectKind.Route, displayName: "Before", bytes: obcrWithName("Before") });

            const written = await renameRoute(client, seeded, "After");

            expect(written.objectId, "the id — and every reference to it — survives").toBe(seeded.objectId);
            expect(written.revision).toBe(2n);
            expect(device.entries).toHaveLength(1);
            expect(device.entries[0].displayName).toBe("After");
            expect(decodeRouteHeader(device.payloadOf(seeded.objectId)!).name).toBe("After");
        });
    });

    it("refuses a rename against a revision the object has already left, and changes nothing", async () => {
        // The page renames the entry it listed. A retaining replace by another peer leaves that
        // revision readable while moving the head on, which is exactly the state where a client that
        // did not carry an expected revision would silently overwrite the newer bytes.
        await withDevice(async ({ client, device }) => {
            const listed = { objectId: 4n, revision: 1n };
            device.seed({ ...listed, kind: ObjectKind.Route, flags: EntryFlags.Retained, bytes: obcrWithName("Listed") });
            const head = obcrWithName("Someone else's");
            device.seed({ objectId: 4n, revision: 2n, kind: ObjectKind.Route, displayName: "Someone else's", bytes: head });

            await expect(renameRoute(client, listed, "Mine")).rejects.toMatchObject({ code: "revision-conflict" });
            expect(device.payloadOf(4n)).toEqual(head);
            expect(device.entries.map((entry) => [entry.revision, entry.displayName])).toEqual([
                [1n, ""],
                [2n, "Someone else's"],
            ]);
        });
    });

    it("creates, edits and reorders a trip through replace-at-same-id", async () => {
        await withDevice(async ({ client, device }) => {
            const created = await createTrip(client, "  Tour du Mont Blanc  ", [3n, 1n]);
            expect(decodeTripObject(device.payloadOf(created.objectId)!)).toEqual({
                name: "Tour du Mont Blanc",
                stages: [3, 1],
            });

            // The page re-lists after every mutation, so the revision each edit expects is the one
            // the catalog just reported — never one this code remembered.
            const revisionOf = async () => (await client.list({ kind: ObjectKind.Trip })).entries[0].revision;
            await updateTrip(client, { objectId: created.objectId, revision: await revisionOf() }, (t) =>
                addStage(t, 7),
            );
            const moved = await updateTrip(client, { objectId: created.objectId, revision: await revisionOf() }, (t) =>
                moveStage(t, 2, -2),
            );

            expect(moved.stages).toEqual([7, 3, 1]);
            expect(decodeTripObject(device.payloadOf(created.objectId)!).stages).toEqual([7, 3, 1]);
            const trips = await client.list({ kind: ObjectKind.Trip });
            expect(trips.entries.map((entry) => [entry.objectId, entry.revision, entry.displayName])).toEqual([
                [created.objectId, 3n, "Tour du Mont Blanc"],
            ]);
        });
    });

    it("refuses to build a trip over a route the format cannot name, before sending anything", async () => {
        await withDevice(async ({ client, device }) => {
            await expect(createTrip(client, "Too far", [BigInt(MAX_TRIP_STAGE_ID) + 1n])).rejects.toBeInstanceOf(
                TripStageError,
            );
            expect(device.entries).toEqual([]);
        });
    });

    // Gone with the v1 wire: "uploading identical trip bytes twice converges on one object". The
    // device deduped a fresh upload on (length, CRC) and answered with the existing id; v4 has no
    // such rule, and §3.4 makes reconciling a create the client's job — `FlatStoreClient.findCreated`,
    // covered in `flows.test.ts` and `client.test.ts`.
});
