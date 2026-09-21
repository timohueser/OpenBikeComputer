/**
 * The device page's store, against the loopback device: loading and grouping, the serialization
 * chain, busy-conflict accounting, and scope invalidation.
 *
 * A `LIST` is an ordinary control exchange, served beside a live transfer — but a trip's stage list
 * is a `GET`, and one transfer at a time is a device-wide rule, so two of those fired together
 * still collide. That is what these
 * tests drive the chain with.
 */

import { beforeAll, describe, expect, it } from "vitest";

import { DeviceDashboard } from "./dashboard.svelte";
import type { RideScope } from "./rides";
import { Crc32 } from "../usb/crc32";
import { encodeTripObject } from "../usb/objects";
import { FlatDevice, flatDevice } from "../usb/flat-device";
import { loadFlatDevice } from "../../../test-support/flat-device/load";
import { EntryFlags, ObjectKind } from "../usb/protocol";

beforeAll(loadFlatDevice);

const SCOPE: RideScope = { serial: "0011223344556677", storeId: "a1b2c3d4000000000000000000000000" };

/** An OBCR-sized payload with a distinct byte at every offset — the page never looks inside one. */
function routeBytes(seed: number): Uint8Array {
    const out = new Uint8Array(128);
    for (let i = 0; i < out.length; i++) out[i] = (i * 31 + seed) & 0xff;
    return out;
}

/** The store assigns the id; `seed` is the payload's own, so a test can name the bytes it expects. */
function seedRoute(device: FlatDevice, name: string, seed = 0) {
    return device.seed({ kind: ObjectKind.Route, displayName: name, bytes: routeBytes(seed) });
}

function seedTrip(device: FlatDevice, name: string, stages: bigint[]) {
    return device.seed({ kind: ObjectKind.Trip, displayName: name, bytes: encodeTripObject({ name, stages }) });
}

async function withDevice(
    body: (rig: ReturnType<typeof flatDevice>, dash: DeviceDashboard) => Promise<void>,
): Promise<void> {
    const rig = flatDevice({});
    try {
        await body(rig, new DeviceDashboard());
        expect(rig.device.faults, "the device adapter saw a reaction it did not expect").toEqual([]);
    } finally {
        await rig.close();
    }
}

describe("DeviceDashboard", () => {
    it("loads and groups: trips resolve their stages, top-level excludes them", async () => {
        await withDevice(async ({ client, device }, dash) => {
            const first = seedRoute(device, "Day 1");
            const second = seedRoute(device, "Day 2");
            const loop = seedRoute(device, "Home loop");
            seedTrip(device, "Jura Crest Trail", [first.objectId, second.objectId]);

            await dash.ensureLoaded(client, SCOPE);
            expect(dash.error).toBeNull();
            expect(dash.routes.map((r) => r.objectId)).toEqual([first.objectId, second.objectId, loop.objectId]);
            expect(dash.topLevelRoutes.map((r) => r.objectId)).toEqual([loop.objectId]);
            expect(dash.trips).toHaveLength(1);
            expect(dash.stagesOf(dash.trips[0]).map((s) => s.route?.displayName)).toEqual(["Day 1", "Day 2"]);
        });
    });

    it("lists a ride the device is still recording, and marks it rather than offering it", async () => {
        // A `GET` of a `RECORDING` entry is refused — its length and CRC are zero until the commit
        // that ends it — so the page's job is to show it as in progress, not to hide it.
        await withDevice(async ({ client, device }, dash) => {
            const finished = device.seed({ kind: ObjectKind.Ride, displayName: "Tuesday", bytes: routeBytes(9) });
            const live = device.seed({ kind: ObjectKind.Ride, displayName: "Now", flags: EntryFlags.Recording });

            await dash.ensureLoaded(client, SCOPE);
            expect(dash.rides.map((r) => r.objectId)).toEqual([finished.objectId, live.objectId]);
            expect(dash.rides.map((r) => dash.isRecording(r))).toEqual([false, true]);
        });
    });

    it("marks a dangling stage instead of dropping it", async () => {
        await withDevice(async ({ client, device }, dash) => {
            const kept = seedRoute(device, "Still here");
            // Stage 9 is a reference the trip's payload carries to an object that is not on the card.
            seedTrip(device, "Half gone", [9n, kept.objectId]);

            await dash.ensureLoaded(client, SCOPE);
            const stages = dash.stagesOf(dash.trips[0]);
            expect(stages[0]).toEqual({ id: 9n, route: null });
            expect(stages[1].route?.displayName).toBe("Still here");
        });
    });

    it("does not re-list for the same scope, and reloads for a new one", async () => {
        await withDevice(async ({ client, device }, dash) => {
            seedRoute(device, "First");
            await dash.ensureLoaded(client, SCOPE);
            seedRoute(device, "Second");
            await dash.ensureLoaded(client, SCOPE);
            expect(dash.routes, "same scope: the cached lists stand").toHaveLength(1);
            await dash.ensureLoaded(client, { ...SCOPE, storeId: "00000000000000000000000000000007" });
            expect(dash.routes, "new scope: reloaded").toHaveLength(2);
        });
    });

    it("serializes enqueued transfers instead of tripping §1's one-at-a-time rule", async () => {
        await withDevice(async ({ client, device }, dash) => {
            const first = seedRoute(device, "One", 1);
            const second = seedRoute(device, "Two", 2);
            // Fired together; without the chain the second `GET` would throw `busy` before it left.
            const [a, b] = await Promise.all([
                dash.enqueue(() => client.get({ objectId: first.objectId, revision: first.revision })),
                dash.enqueue(() => client.get({ objectId: second.objectId, revision: second.revision })),
            ]);
            expect(a.bytes).toEqual(routeBytes(1));
            expect(b.bytes).toEqual(routeBytes(2));
            expect(dash.busy).toBe(false);
        });
    });

    it("keeps the chain alive after a failed operation", async () => {
        await withDevice(async ({ client, device }, dash) => {
            seedRoute(device, "Survivor");
            await expect(dash.enqueue(() => client.get({ objectId: 99n, revision: 0n }))).rejects.toMatchObject({
                code: "not-found",
            });
            const catalog = await dash.enqueue(() => client.list({ kind: ObjectKind.Route }));
            expect(catalog.entries).toHaveLength(1);
        });
    });

    it("flags busy when a foreign transfer holds the slot, and recovers after clearBusy", async () => {
        await withDevice(async ({ client, device }, dash) => {
            const route = seedRoute(device, "One");
            // A transfer the page does not own: started directly on the client, outside the chain —
            // the builder tab's map send, in miniature. The rule is device-wide, so the page cannot
            // serialize its way out of this one; it renders it.
            let releaseForeign!: () => void;
            const gate = new Promise<void>((resolve) => (releaseForeign = resolve));
            const payload = new Uint8Array([1, 2, 3, 4]);
            const foreign = client.put({ kind: ObjectKind.Route, displayName: "Foreign" }, {
                totalLen: payload.length,
                crc32: Crc32.of(payload),
                chunks: async function* () {
                    await gate;
                    yield payload;
                },
            });

            await expect(
                dash.enqueue(() => client.get({ objectId: route.objectId, revision: route.revision })),
            ).rejects.toMatchObject({ code: "busy" });
            expect(dash.busy).toBe(true);

            releaseForeign();
            await foreign;
            dash.clearBusy();
            const catalog = await dash.enqueue(() => client.list({ kind: ObjectKind.Route }));
            expect(catalog.entries.map((entry) => entry.objectId)).toContain(route.objectId);
            expect(dash.busy).toBe(false);
        });
    });

    it("refresh after a remove drops the row", async () => {
        await withDevice(async ({ client, device }, dash) => {
            const doomed = seedRoute(device, "Doomed");
            const stays = seedRoute(device, "Stays");
            await dash.ensureLoaded(client, SCOPE);
            expect(dash.routes).toHaveLength(2);
            await dash.enqueue(() => client.remove({ objectId: doomed.objectId, revision: doomed.revision }));
            await dash.refresh(client);
            expect(dash.routes.map((r) => r.objectId)).toEqual([stays.objectId]);
        });
    });

    it("invalidate drops everything, so the next connect decides what to load", async () => {
        await withDevice(async ({ client, device }, dash) => {
            seedRoute(device, "Loaded");
            await dash.ensureLoaded(client, SCOPE);
            expect(dash.routes).toHaveLength(1);

            dash.invalidate();
            expect(dash.routes).toEqual([]);
            expect(dash.trips).toEqual([]);
            expect(dash.rides).toEqual([]);
            // The same scope loads again, because nothing is remembered about it.
            await dash.ensureLoaded(client, SCOPE);
            expect(dash.routes).toHaveLength(1);
        });
    });
});
