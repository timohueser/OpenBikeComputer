/**
 * The device page's store, against the loopback device: loading and grouping, the serialization
 * chain (its whole reason to exist — every `list*` is a transfer, and the client throws on a
 * second concurrent one), busy-conflict accounting, and scope invalidation.
 */

import { describe, expect, it } from "vitest";

import { DeviceDashboard } from "./dashboard.svelte";
import type { RideScope } from "./rides";
import { encodeTripObject, type RouteListEntry, type TripListEntry } from "../usb/objects";
import { Crc32 } from "../usb/crc32";
import { loopbackDevice } from "../usb/loopback";
import { ObjectType } from "../usb/protocol";

const SCOPE: RideScope = { serial: "0011223344556677", epoch: 0xa1b2c3d4 };

function routeEntry(objectId: number, name: string, over: Partial<RouteListEntry> = {}): RouteListEntry {
    return {
        objectId,
        byteLen: 128,
        distanceM: 42_300,
        ascentM: 1_240,
        pointCount: 3_182,
        waypointCount: 0,
        name,
        crc32: 0,
        expiresAt: 0,
        retention: 0,
        ...over,
    };
}

function tripEntry(objectId: number, name: string, stages: number[]): { entry: TripListEntry; bytes: Uint8Array } {
    const bytes = encodeTripObject({ name, stages });
    return {
        entry: {
            objectId,
            byteLen: bytes.length,
            totalDistanceM: 105_100,
            totalAscentM: 3_120,
            stageCount: stages.length,
            name,
            crc32: Crc32.of(bytes),
        },
        bytes,
    };
}

async function withDevice(
    body: (ctx: ReturnType<typeof loopbackDevice>, dash: DeviceDashboard) => Promise<void>,
): Promise<void> {
    const ctx = loopbackDevice({});
    try {
        await body(ctx, new DeviceDashboard());
        expect(ctx.device.faults, "the simulated device hit a non-transport failure").toEqual([]);
    } finally {
        await ctx.close();
    }
}

describe("DeviceDashboard", () => {
    it("loads and groups: trips resolve their stages, top-level excludes them", async () => {
        await withDevice(async ({ client, device }, dash) => {
            device.seedRoute(routeEntry(1, "Day 1"));
            device.seedRoute(routeEntry(2, "Day 2"));
            device.seedRoute(routeEntry(3, "Home loop"));
            const trip = tripEntry(1, "Jura Crest Trail", [1, 2]);
            device.seedTrip(trip.entry, trip.bytes);

            await dash.ensureLoaded(client, SCOPE);
            expect(dash.error).toBeNull();
            expect(dash.routes.map((r) => r.objectId)).toEqual([1, 2, 3]);
            expect(dash.topLevelRoutes.map((r) => r.objectId)).toEqual([3]);
            expect(dash.trips).toHaveLength(1);
            expect(dash.stagesOf(dash.trips[0]).map((s) => s.route?.name)).toEqual(["Day 1", "Day 2"]);
        });
    });

    it("marks a dangling stage instead of dropping it", async () => {
        await withDevice(async ({ client, device }, dash) => {
            device.seedRoute(routeEntry(2, "Still here"));
            const trip = tripEntry(1, "Half gone", [9, 2]);
            device.seedTrip(trip.entry, trip.bytes);

            await dash.ensureLoaded(client, SCOPE);
            const stages = dash.stagesOf(dash.trips[0]);
            expect(stages[0]).toEqual({ id: 9, route: null });
            expect(stages[1].route?.name).toBe("Still here");
        });
    });

    it("does not re-list for the same scope, and reloads for a new one", async () => {
        await withDevice(async ({ client, device }, dash) => {
            device.seedRoute(routeEntry(1, "First"));
            await dash.ensureLoaded(client, SCOPE);
            device.seedRoute(routeEntry(2, "Second"));
            await dash.ensureLoaded(client, SCOPE);
            expect(dash.routes, "same scope: the cached lists stand").toHaveLength(1);
            await dash.ensureLoaded(client, { ...SCOPE, epoch: 7 });
            expect(dash.routes, "new scope: reloaded").toHaveLength(2);
        });
    });

    it("serializes enqueued transfers instead of tripping the client's busy rule", async () => {
        await withDevice(async ({ client, device }, dash) => {
            device.seedRoute(routeEntry(1, "One"));
            // Fired together; without the chain the second list would throw `busy`.
            const [routes, rides] = await Promise.all([
                dash.enqueue(() => client.listRoutes()),
                dash.enqueue(() => client.listRides()),
            ]);
            expect(routes.entries).toHaveLength(1);
            expect(rides.entries).toHaveLength(0);
            expect(dash.busy).toBe(false);
        });
    });

    it("keeps the chain alive after a failed operation", async () => {
        await withDevice(async ({ client, device }, dash) => {
            device.seedRoute(routeEntry(1, "Survivor"));
            await expect(
                dash.enqueue(() => client.download(ObjectType.Route, 99)),
            ).rejects.toThrow();
            const routes = await dash.enqueue(() => client.listRoutes());
            expect(routes.entries).toHaveLength(1);
        });
    });

    it("flags busy when a foreign transfer holds the slot, and recovers after clearBusy", async () => {
        await withDevice(async ({ client, device }, dash) => {
            device.seedRoute(routeEntry(1, "One"));
            // A transfer the page does not own: started directly on the client,
            // outside the chain — the builder tab's map send, in miniature.
            let releaseForeign!: () => void;
            const gate = new Promise<void>((resolve) => (releaseForeign = resolve));
            const foreign = client.upload(ObjectType.Route, 0xffff, {
                totalLen: 4,
                crc32: Crc32.of(new Uint8Array([1, 2, 3, 4])),
                chunks: async function* (n: number) {
                    void n;
                    await gate;
                    yield new Uint8Array([1, 2, 3, 4]);
                },
            });

            await expect(dash.enqueue(() => client.listRoutes())).rejects.toThrow(/transfer is already running/);
            expect(dash.busy).toBe(true);

            releaseForeign();
            await foreign;
            dash.clearBusy();
            // The foreign upload committed a route of its own, so two are listed now —
            // the point is that the chain works again, not what the card holds.
            const routes = await dash.enqueue(() => client.listRoutes());
            expect(routes.entries.map((r) => r.objectId)).toContain(1);
            expect(dash.busy).toBe(false);
        });
    });

    it("refresh after a delete drops the row", async () => {
        await withDevice(async ({ client, device }, dash) => {
            device.seedRoute(routeEntry(1, "Doomed", { byteLen: 4 }));
            device.seedRoute(routeEntry(2, "Stays", { byteLen: 4 }));
            await dash.ensureLoaded(client, SCOPE);
            expect(dash.routes).toHaveLength(2);
            await dash.enqueue(() => client.deleteObject(ObjectType.Route, 1));
            await dash.refresh(client);
            expect(dash.routes.map((r) => r.objectId)).toEqual([2]);
        });
    });
});
