/**
 * The behaviour a codec test cannot reach: what happens when a transfer is cancelled, when the
 * cable comes out, when the device says no, and when the bytes arrive in the wrong-sized pieces.
 *
 * Everything here runs over the loopback device, which is the point of building one — these are
 * exactly the paths that are impossible to provoke reliably on hardware and catastrophic to get
 * wrong on it.
 */

import { describe, expect, it, vi } from "vitest";

import { DeviceError, ProtocolClient } from "./client";
import { Crc32 } from "./crc32";
import { MockDevice, loopbackDevice, loopbackLink } from "./loopback";
import { PipeError } from "./pipe";
import { Command, CommandStatus, NEW_OBJECT_ID, ObjectType, SINGLETON_OBJECT_ID } from "./protocol";

/** A recognisable, incompressible-looking payload of `n` bytes. */
function payload(n: number) {
    return Uint8Array.from({ length: n }, (_, i) => (i * 31 + 7) & 0xff);
}

/** Run `body` against a fresh loopback device, closing it whatever happens. */
async function withDevice(
    options: Parameters<typeof loopbackDevice>[0],
    body: (ctx: ReturnType<typeof loopbackDevice>) => Promise<void>,
): Promise<void> {
    const ctx = loopbackDevice(options);
    try {
        await body(ctx);
        // The device runs its transfers detached, so a defect in it would otherwise vanish into a
        // swallowed rejection and the test would pass for the wrong reason.
        expect(ctx.device.faults, "the simulated device hit a non-transport failure").toEqual([]);
    } finally {
        await ctx.close();
    }
}

describe("streaming", () => {
    it("reassembles an object delivered in packet-sized pieces", async () => {
        // The single most important property to get right: a bulk endpoint hands over 64 bytes at a
        // time, and a client that treats one read as one logical unit works on a naive mock and
        // fails on silicon. The loopback re-slices for exactly this reason.
        await withDevice({ bulkPacketSize: 64, chunkSize: 61 }, async ({ client, device }) => {
            const bytes = payload(5000);
            const { objectId } = await client.upload(ObjectType.Route, NEW_OBJECT_ID, bytes);
            expect(device.stored(ObjectType.Route, objectId)).toEqual(bytes);
            expect(await client.download(ObjectType.Route, objectId)).toEqual(bytes);
        });
    });

    it("reports progress in both directions", async () => {
        await withDevice({ bulkPacketSize: 128 }, async ({ client }) => {
            const bytes = payload(4096);
            const up: number[] = [];
            const { objectId } = await client.upload(ObjectType.Route, NEW_OBJECT_ID, bytes, {
                chunkSize: 512,
                onProgress: (done, total) => {
                    expect(total).toBe(bytes.length);
                    up.push(done);
                },
            });
            expect(up[0]).toBe(0);
            expect(up.at(-1)).toBe(bytes.length);

            const down: number[] = [];
            await client.download(ObjectType.Route, objectId, { onProgress: (done) => down.push(done) });
            expect(down.at(-1)).toBe(bytes.length);
        });
    });

    it("accepts a Blob without holding it twice", async () => {
        await withDevice({}, async ({ client, device }) => {
            const bytes = payload(3000);
            const { blobSource } = await import("./client");
            const source = await blobSource(new Blob([bytes]));
            expect(source.totalLen).toBe(bytes.length);
            expect(source.crc32).toBe(Crc32.of(bytes));
            const { objectId } = await client.upload(ObjectType.Route, NEW_OBJECT_ID, source);
            expect(device.stored(ObjectType.Route, objectId)).toEqual(bytes);
        });
    });
});

describe("backpressure", () => {
    it("makes a writer wait for the reader to drain", async () => {
        // Real backpressure is the device NAKing an endpoint it has not drained. Faking it here is
        // what makes a fire-and-forget writer — one that would outrun an SD card topping out in the
        // high hundreds of KB/s — fail in CI instead of on a rider's desk.
        const link = loopbackLink({ bulkHighWaterMark: 256, bulkPacketSize: 64 });
        let resolved = false;
        const write = link.host.bulk.write(payload(1024)).then(() => {
            resolved = true;
        });
        await Promise.resolve();
        expect(resolved, "a writer past the high-water mark must not resolve").toBe(false);
        expect(link.bulkDepth("to-device")).toBeGreaterThan(256);

        let drained = 0;
        while (drained < 1024) drained += (await link.device.bulk.read()).length;
        await write;
        expect(resolved).toBe(true);
        await link.host.close();
    });
});

describe("cancellation", () => {
    it("cancels an upload and leaves the link usable", async () => {
        await withDevice({ bulkHighWaterMark: 512, bulkPacketSize: 64 }, async ({ client, device }) => {
            const controller = new AbortController();
            const big = payload(200_000);
            const upload = client.upload(ObjectType.Route, NEW_OBJECT_ID, big, {
                signal: controller.signal,
                chunkSize: 256,
                onProgress: (done) => {
                    if (done > 1024) controller.abort();
                },
            });
            const error = await upload.catch((e: unknown) => e);
            expect(error).toBeInstanceOf(DeviceError);
            expect((error as DeviceError).code).toBe("aborted");
            expect(device.stored(ObjectType.Route, 1)).toBeNull();

            // The real test of the reset: the *next* transfer must not inherit the abandoned one's
            // bytes. Without the abort handshake and the pipe reset, this is where a client
            // desynchronises and the failure lands on the following, innocent object.
            const small = payload(300);
            const { objectId } = await client.upload(ObjectType.Route, NEW_OBJECT_ID, small);
            expect(device.stored(ObjectType.Route, objectId)).toEqual(small);
        });
    });

    it("cancels a download in flight", async () => {
        await withDevice({ bulkPacketSize: 64, chunkSize: 64, bulkHighWaterMark: 128 }, async ({ client, device }) => {
            const bytes = payload(100_000);
            device.seedRide(
                {
                    objectId: 5,
                    byteLen: bytes.length,
                    startTime: 0,
                    distanceM: 0,
                    movingTimeS: 0,
                    avgSpeedCms: 0,
                    climbM: 0,
                    name: "big",
                },
                bytes,
            );
            const controller = new AbortController();
            const download = client.download(ObjectType.Ride, 5, {
                signal: controller.signal,
                onProgress: (done) => {
                    if (done > 512) controller.abort();
                },
            });
            const error = await download.catch((e: unknown) => e);
            expect((error as DeviceError).code).toBe("aborted");

            // And the link still works afterwards.
            const again = await client.download(ObjectType.Ride, 5);
            expect(again).toEqual(bytes);
        });
    });

    it("refuses a second transfer while one is running", async () => {
        await withDevice({ bulkHighWaterMark: 256, bulkPacketSize: 64 }, async ({ client }) => {
            const first = client.upload(ObjectType.Route, NEW_OBJECT_ID, payload(50_000), { chunkSize: 128 });
            const second = client.upload(ObjectType.Route, NEW_OBJECT_ID, payload(10));
            const error = await second.catch((e: unknown) => e);
            expect((error as DeviceError).code).toBe("busy");
            await first;
        });
    });
});

describe("the link going away", () => {
    it("fails a transfer in flight immediately, not on a timeout", async () => {
        // The stuck-spinner case #902 calls out. The client waits 15 s for an answer; an unplug has
        // to produce an error in milliseconds, which only happens if the pipes fail their waiters
        // rather than leaving them to expire.
        const ctx = loopbackDevice({ bulkPacketSize: 64, chunkSize: 64, bulkHighWaterMark: 128 });
        const bytes = payload(200_000);
        ctx.device.seedRide(
            {
                objectId: 1,
                byteLen: bytes.length,
                startTime: 0,
                distanceM: 0,
                movingTimeS: 0,
                avgSpeedCms: 0,
                climbM: 0,
                name: "big",
            },
            bytes,
        );
        const started = Date.now();
        const download = ctx.client.download(ObjectType.Ride, 1, {
            onProgress: (done) => {
                if (done > 256) void ctx.link.host.close();
            },
        });
        const error = await download.catch((e: unknown) => e);
        expect((error as DeviceError).code).toBe("link");
        expect(Date.now() - started).toBeLessThan(2_000);
        await ctx.close();
    });

    it("fails a waiting read rather than hanging when nobody answers", async () => {
        const link = loopbackLink();
        const client = new ProtocolClient(link.host, { timeoutMs: 40 });
        const error = await client.identity().catch((e: unknown) => e);
        expect((error as DeviceError).code).toBe("timeout");
        await client.close();
    });
});

describe("the device saying no", () => {
    it("rejects a corrupted upload without storing anything", async () => {
        await withDevice({}, async ({ client, device }) => {
            const bytes = payload(1000);
            // Announce a CRC that does not match the bytes — what a transport-level corruption
            // looks like from the device's side.
            const lying = { totalLen: bytes.length, crc32: Crc32.of(bytes) ^ 0xffff, chunks: bytesChunks(bytes) };
            const error = await client.upload(ObjectType.Route, NEW_OBJECT_ID, lying).catch((e: unknown) => e);
            expect((error as DeviceError).code).toBe("crc-mismatch");
            expect(device.stored(ObjectType.Route, 1)).toBeNull();
        });
    });

    it("surfaces a full catalog before any bytes stream, and exempts a replace", async () => {
        await withDevice({ maxRoutes: 1 }, async ({ client, device }) => {
            const first = await client.upload(ObjectType.Route, NEW_OBJECT_ID, payload(64));
            const error = await client.upload(ObjectType.Route, NEW_OBJECT_ID, payload(64)).catch((e: unknown) => e);
            expect((error as DeviceError).code).toBe("storage-full");

            // Replacing an existing route reuses its slot, so it must succeed even at the cap —
            // updating the route you are navigating can never be refused for space.
            const replacement = payload(128);
            await client.upload(ObjectType.Route, first.objectId, replacement);
            expect(device.stored(ObjectType.Route, first.objectId)).toEqual(replacement);
        });
    });

    it("refuses an oversized firmware image at the descriptor, before the megabytes move", async () => {
        await withDevice({ maxFwImageLen: 1000 }, async ({ client, device }) => {
            const error = await client
                .upload(ObjectType.FwImage, SINGLETON_OBJECT_ID, payload(2000))
                .catch((e: unknown) => e);
            expect((error as DeviceError).code).toBe("device-error");
            expect(device.stagedFirmware).toBeNull();
        });
    });

    it("answers a fresh upload of content it already holds with the existing id", async () => {
        await withDevice({}, async ({ client, device }) => {
            const bytes = payload(512);
            const first = await client.upload(ObjectType.Route, NEW_OBJECT_ID, bytes);
            // The lost-ack retry: identical bytes sent again as "new". Without dedup the device
            // mints a silent same-content twin and the catalog fills with duplicates.
            const retry = await client.upload(ObjectType.Route, NEW_OBJECT_ID, bytes);
            expect(retry.objectId).toBe(first.objectId);
            expect((await client.listRoutes()).entries).toHaveLength(1);
            expect(device.stored(ObjectType.Route, first.objectId)).toEqual(bytes);
        });
    });

    it("reports a download of something the device does not have", async () => {
        await withDevice({}, async ({ client }) => {
            const error = await client.download(ObjectType.Ride, 42).catch((e: unknown) => e);
            expect((error as DeviceError).code).toBe("not-found");
        });
    });

    it("stops on a protocol-version mismatch instead of decoding anyway", async () => {
        await withDevice({ protocolVersion: 3 }, async ({ client }) => {
            const error = await client.identity().catch((e: unknown) => e);
            expect((error as DeviceError).code).toBe("protocol-version");
            expect((error as DeviceError).message).toMatch(/v3/);
        });
    });

    it("reports a card-less device as having no epoch, never epoch 0", async () => {
        // Epoch 0 is a legal era. Conflating "the device could not name its era" with it would let
        // a peer key durable state to an era it never actually read.
        await withDevice({ storeEpoch: null }, async ({ client }) => {
            expect(await client.identity()).toEqual({ version: 2, storeEpoch: null });
        });
    });
});

describe("commands", () => {
    it("stamps the clock and reports the values the device kept", async () => {
        await withDevice({}, async ({ client, device }) => {
            await client.setClock(new Date(1783598400 * 1000), 120);
            expect(device.clock).toEqual({ utc: 1783598400, offsetMin: 120 });
        });
    });

    it("flags rides monotonically and reports only the newly flagged", async () => {
        await withDevice({}, async ({ client, device }) => {
            for (const id of [1, 2, 3]) {
                device.seedRide(
                    {
                        objectId: id,
                        byteLen: 0,
                        startTime: 0,
                        distanceM: 0,
                        movingTimeS: 0,
                        avgSpeedCms: 0,
                        climbM: 0,
                        name: `ride ${id}`,
                    },
                    new Uint8Array(0),
                );
            }
            // Unknown ids are ignored, not an error — the peer may hold rides the device deleted.
            expect(await client.ackRides([1, 2, 99])).toBe(2);
            // Re-acking changes nothing: the flag means "downloaded at least once", not "still held".
            expect(await client.ackRides([1, 2, 3])).toBe(1);
            expect([...device.synced].sort()).toEqual([1, 2, 3]);
        });
    });

    it("splits a long ack list across writes", async () => {
        await withDevice({}, async ({ client, device }) => {
            const ids = Array.from({ length: 70 }, (_, i) => i + 1);
            await client.ackRides(ids);
            // 70 ids at 30 per write is three `command` writes, all of them ackRides.
            expect(device.commandLog.filter((c) => c === Command.AckRides)).toHaveLength(3);
        });
    });

    it("degrades gracefully when the device predates a command", async () => {
        await withDevice({}, async ({ client }) => {
            const error = await client.command(new Uint8Array([99])).catch((e: unknown) => e);
            expect((error as DeviceError).code).toBe("unsupported-command");
            expect((error as DeviceError).status).toBe(CommandStatus.UnknownCommand);
        });
    });

    it("reports a retention set on a route the device does not hold", async () => {
        await withDevice({}, async ({ client }) => {
            const error = await client.setRouteRetention(404, 3).catch((e: unknown) => e);
            expect((error as DeviceError).code).toBe("not-found");
        });
    });

    it("reserves ride deletion for the device itself", async () => {
        await withDevice({}, async ({ client, device }) => {
            device.seedRide(
                {
                    objectId: 4,
                    byteLen: 0,
                    startTime: 0,
                    distanceM: 0,
                    movingTimeS: 0,
                    avgSpeedCms: 0,
                    climbM: 0,
                    name: "ride",
                },
                new Uint8Array(0),
            );
            const error = await client.deleteObject(ObjectType.Ride, 4).catch((e: unknown) => e);
            expect((error as DeviceError).code).toBe("not-found");
            expect(device.stored(ObjectType.Ride, 4)).not.toBeNull();
        });
    });

    it("asks for an install without one ever happening on its own", async () => {
        await withDevice({}, async ({ client }) => {
            // Nothing staged yet: the request is answered `notFound`, not queued.
            const error = await client.installFw().catch((e: unknown) => e);
            expect((error as DeviceError).code).toBe("not-found");
            await client.upload(ObjectType.FwImage, SINGLETON_OBJECT_ID, payload(256));
            await expect(client.installFw()).resolves.toBeUndefined();
        });
    });
});

describe("change signalling", () => {
    it("notifies a store change on every commit", async () => {
        await withDevice({}, async ({ client }) => {
            const seen = vi.fn();
            const off = client.onStoreChanged(seen);
            await client.upload(ObjectType.Route, NEW_OBJECT_ID, payload(64));
            expect(seen).toHaveBeenCalledWith(ObjectType.Route, 1);
            off();
            await client.upload(ObjectType.Route, NEW_OBJECT_ID, payload(65));
            expect(seen).toHaveBeenCalledTimes(1);
        });
    });
});

describe("the loopback pipe itself", () => {
    it("rejects reads and writes once closed", async () => {
        const link = loopbackLink();
        await link.host.close();
        await expect(link.host.bulk.read()).rejects.toBeInstanceOf(PipeError);
        await expect(link.host.bulk.write(new Uint8Array([1]))).rejects.toBeInstanceOf(PipeError);
    });

    it("runs a device that stops when told to", async () => {
        const link = loopbackLink();
        const device = new MockDevice(link.device);
        const running = device.run();
        await link.device.close();
        await expect(running).resolves.toBeUndefined();
    });
});

/** An `ObjectSource`'s chunk generator over a fixed buffer, for the lying-CRC case. */
function bytesChunks(bytes: Uint8Array) {
    return async function* (chunkSize: number) {
        for (let at = 0; at < bytes.length; at += chunkSize) {
            yield bytes.subarray(at, Math.min(at + chunkSize, bytes.length));
        }
    };
}
