/**
 * The behaviour a codec test cannot reach: what happens when a transfer is cancelled, when the
 * cable comes out, when the device says no, and when the bytes arrive in the wrong-sized pieces.
 *
 * Everything here runs over the loopback device, which is the point of building one — these are
 * exactly the paths that are impossible to provoke reliably on hardware and catastrophic to get
 * wrong on it.
 */

import { describe, expect, it, vi } from "vitest";

import { DEFAULT_TIMEOUT_MS, DeviceError, ProtocolClient, commitTimeoutMs } from "./client";
import { Crc32 } from "./crc32";
import { MockDevice, loopbackDevice, loopbackLink } from "./loopback";
import { PipeError, type BytePipe } from "./pipe";
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
    it("reads free space from the connected card, including no-card", async () => {
        await withDevice({ cardFreeBytes: 3_456_789 }, async ({ client }) => {
            expect(await client.cardFreeBytes()).toBe(3_456_789);
        });
        await withDevice({ cardFreeBytes: null }, async ({ client }) => {
            expect(await client.cardFreeBytes()).toBeNull();
        });
    });

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
        // what makes a fire-and-forget writer — one that queues an object without ever retiring a
        // transfer — fail in CI instead of on a rider's desk.
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

describe("the upload window", () => {
    it("keeps more than one write on the wire and still delivers them in order", async () => {
        // The whole point of `pumpChunks`: several `transferOut`s outstanding at once. A pipe that
        // reordered them, or a loop that awaited each one, would both pass a byte-equality check on
        // a *serial* upload — so this asserts the depth as well as the bytes.
        let peakOutstanding = 0;
        await withDevice({ bulkPacketSize: 64, chunkSize: 61 }, async ({ client, device, link }) => {
            const bulk = link.host.bulk;
            const realWrite = bulk.write.bind(bulk);
            let outstanding = 0;
            vi.spyOn(bulk, "write").mockImplementation(async (bytes, signal) => {
                outstanding += 1;
                peakOutstanding = Math.max(peakOutstanding, outstanding);
                try {
                    return await realWrite(bytes, signal);
                } finally {
                    outstanding -= 1;
                }
            });
            const bytes = payload(120_000);
            const { objectId } = await client.upload(ObjectType.Route, NEW_OBJECT_ID, bytes, {
                chunkSize: 4096,
            });
            // Byte-for-byte through a device that verifies its own whole-object CRC, so a reordered
            // window could not reach here.
            expect(device.stored(ObjectType.Route, objectId)).toEqual(bytes);
        });
        expect(peakOutstanding, "the upload never had more than one write on the wire").toBeGreaterThan(1);
    });

    it("never reports progress for bytes the transport has not taken", async () => {
        // The claim is about *lag*, so the assertion has to be against what the transport has
        // actually accepted at the moment each callback fires — monotonicity and a correct total
        // hold just as well for a bar driven by hand-off, which would run to 100% while a
        // quarter-megabyte was still queued and make a failure look like it happened after the bytes
        // landed.
        const link = loopbackLink({ bulkPacketSize: 64 });
        const device = new MockDevice(link.device, {});
        void device.run();
        let accepted = 0;
        const bulk: BytePipe = {
            ...link.host.bulk,
            transport: "counting",
            open: true,
            read: (signal) => link.host.bulk.read(signal),
            reset: () => link.host.bulk.reset(),
            close: () => link.host.bulk.close(),
            write: async (bytes, signal) => {
                await link.host.bulk.write(bytes, signal);
                accepted += bytes.length;
            },
        };
        const client = new ProtocolClient({ control: link.host.control, bulk, close: () => link.host.close() });
        const bytes = payload(120_000);
        const overruns: string[] = [];
        await client.upload(ObjectType.Route, NEW_OBJECT_ID, bytes, {
            chunkSize: 4096,
            onProgress: (done) => {
                if (done > accepted) overruns.push(`reported ${done} with only ${accepted} taken`);
            },
        });
        expect(overruns, "progress ran ahead of the transport").toEqual([]);
        device.stop();
        await link.host.close();
    });

    it("survives an async descriptor reject mid-window and retries cleanly", async () => {
        // The stray-byte hole in one test. The device rejects the descriptor *after* the host has
        // queued several chunks (`checkUploadOpen` is what notices), so those bytes are already on
        // their way to a transfer that will never read them. If they are still in the pipe when the
        // retry arms, they become its opening payload and its whole-object CRC fails — a retry that
        // cannot succeed until the window happens to drain.
        await withDevice({ bulkPacketSize: 64, maxFwImageLen: 1024 }, async ({ client, device, link }) => {
            // Bytes on the endpoint that belong to no armed transfer — what a host that was mid-send
            // when a descriptor was refused leaves behind. Injected directly, because *how many*
            // chunks escape before `checkUploadOpen` notices is a timing detail of the loopback and
            // the property under test is not.
            await link.host.bulk.write(payload(9_000));

            // A refused descriptor. Its failure path is what has to leave the pipe clean: the device
            // answered, so nothing about this transfer is outstanding — but the strays above are.
            await expect(
                client.upload(ObjectType.FwImage, SINGLETON_OBJECT_ID, payload(200_000), { chunkSize: 4096 }),
            ).rejects.toMatchObject({ name: "DeviceError" });

            // The retry is an ordinary first attempt. Anything left over shows up here and nowhere
            // else: prepended to this object, it fails the whole-object CRC.
            const bytes = payload(60_000);
            const { objectId } = await client.upload(ObjectType.Route, NEW_OBJECT_ID, bytes, {
                chunkSize: 4096,
            });
            expect(device.stored(ObjectType.Route, objectId)).toEqual(bytes);
            expect(device.strayBytesDiscarded, "the stray bytes were never discarded").toBeGreaterThanOrEqual(
                9_000,
            );
        });
    });

    it("goes through the abort handshake after any failed upload, not only a cancel", async () => {
        // The handshake is what quiesces the pipe before a retry, so it has to happen on *both*
        // shapes of failure — a cancel, where the host's transfers stay on the wire, and a
        // device-originated reject, where they do not but the leftovers are just as unrecallable.
        // Skipping it on the second was the hole the reject-retry test above lands on.
        const seen: number[] = [];
        const link = loopbackLink({ bulkPacketSize: 64 });
        const device = new MockDevice(link.device, { maxFwImageLen: 1024 });
        void device.run();
        const control: BytePipe = {
            ...link.host.control,
            transport: "counting-control",
            open: true,
            read: (signal) => link.host.control.read(signal),
            reset: () => link.host.control.reset(),
            close: () => link.host.control.close(),
            write: (frameBytes, signal) => {
                // Selector 2 is `transferControl`; the descriptor's first payload byte is the op.
                if (frameBytes[0] === 2) seen.push(frameBytes[1]);
                return link.host.control.write(frameBytes, signal);
            },
        };
        const client = new ProtocolClient({
            control,
            bulk: link.host.bulk,
            close: () => link.host.close(),
        });

        // 1. A device-originated reject: the device answered, so the old code sent no abort.
        await expect(
            client.upload(ObjectType.FwImage, SINGLETON_OBJECT_ID, payload(200_000), { chunkSize: 4096 }),
        ).rejects.toMatchObject({ name: "DeviceError" });
        expect(seen.filter((op) => op === 3), "a device-originated reject must still be followed by op=3")
            .toHaveLength(1);

        // 2. A rider's cancel, which always did.
        seen.length = 0;
        const controller = new AbortController();
        const upload = client.upload(ObjectType.Route, NEW_OBJECT_ID, payload(200_000), {
            chunkSize: 4096,
            signal: controller.signal,
            onProgress: (done) => {
                if (done > 8192) controller.abort();
            },
        });
        await expect(upload).rejects.toMatchObject({ name: "DeviceError" });
        expect(seen.filter((op) => op === 3), "a cancel must send op=3").toHaveLength(1);

        device.stop();
        await link.host.close();
    });

    it("surfaces a mid-window write failure without an unhandled rejection", async () => {
        // The shape that produces one: a chunk rejects while its *predecessors are still pending*,
        // so nothing has awaited it yet. Node calls that unhandled the moment the turn ends, and the
        // report lands on top of the caller's real error. The window makes it reachable — with one
        // write outstanding the rejection is awaited immediately and this cannot happen.
        const unhandled: unknown[] = [];
        const onUnhandled = (reason: unknown) => unhandled.push(reason);
        process.on("unhandledRejection", onUnhandled);
        try {
            const link = loopbackLink({ bulkPacketSize: 64 });
            const device = new MockDevice(link.device, {});
            void device.run();
            // A hand-rolled bulk half rather than a spy: a spy records what it returns, which
            // attaches a handler to a rejected promise and hides the very thing under test.
            let calls = 0;
            const failingBulk: BytePipe = {
                ...link.host.bulk,
                transport: "failing",
                open: true,
                read: (signal) => link.host.bulk.read(signal),
                reset: () => link.host.bulk.reset(),
                close: () => link.host.bulk.close(),
                write: () => {
                    calls += 1;
                    // Writes 1 and 2 stay pending; write 3 fails underneath them, so nothing has
                    // awaited it yet.
                    if (calls === 3) return Promise.reject(new PipeError("device-error", "the endpoint stalled"));
                    return new Promise<void>((resolve) => setTimeout(resolve, 40));
                },
            };
            const client = new ProtocolClient({
                control: link.host.control,
                bulk: failingBulk,
                close: () => link.host.close(),
            });
            await expect(
                client.upload(ObjectType.Route, NEW_OBJECT_ID, payload(120_000), { chunkSize: 4096 }),
            ).rejects.toMatchObject({ name: "DeviceError", code: "device-error" });
            device.stop();
            // Node reports an unhandled rejection at the end of the turn it happened in, so give it
            // one clear macrotask before deciding there was none.
            await new Promise((resolve) => setTimeout(resolve, 10));
            expect(unhandled, "a queued write rejected with nobody watching").toEqual([]);
        } finally {
            process.off("unhandledRejection", onUnhandled);
        }
    });
});

describe("commitTimeoutMs", () => {
    it("is opt-in: an ordinary upload keeps the default answer budget", async () => {
        // The regression this pins is a quiet one — routing every upload through the scaled budget
        // makes a wedged device take 4-40x longer to surface, and nothing about the happy path
        // changes. So assert on the wait the client actually applies.
        await withDevice({ bulkPacketSize: 64, chunkSize: 61 }, async ({ client }) => {
            const timeouts: number[] = [];
            const client_ = client as unknown as {
                statuses: { take: (ms: number, signal: unknown, what: string) => Promise<unknown> };
            };
            const realTake = client_.statuses.take.bind(client_.statuses);
            vi.spyOn(client_.statuses, "take").mockImplementation((ms, signal, what) => {
                if (what.includes("upload")) timeouts.push(ms);
                return realTake(ms, signal, what);
            });
            await client.upload(ObjectType.Route, NEW_OBJECT_ID, payload(4096));
            expect(timeouts).toEqual([DEFAULT_TIMEOUT_MS]);

            timeouts.length = 0;
            await client.upload(ObjectType.Route, NEW_OBJECT_ID, payload(4097), {
                commitBytes: 300 * 1024 * 1024,
            });
            expect(timeouts).toEqual([commitTimeoutMs(300 * 1024 * 1024)]);
        });
    });

    it("scales with the bytes the device has to re-read, and is capped", () => {
        // A set manifest is ~2 KB but its commit walks the whole set, which is what `commitBytes`
        // exists to say. The cap is what stops a mis-announced length becoming a forever spinner.
        expect(commitTimeoutMs(0)).toBe(60_000);
        expect(commitTimeoutMs(1)).toBe(60_200);
        expect(commitTimeoutMs(300 * 1024 * 1024)).toBe(60_000 + 300 * 200);
        expect(commitTimeoutMs(Number.MAX_SAFE_INTEGER)).toBe(10 * 60_000);
        // Never below the ordinary budget, whatever it is given.
        expect(commitTimeoutMs(-1)).toBeGreaterThanOrEqual(DEFAULT_TIMEOUT_MS);
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
            expect(await client.identity()).toEqual({ version: 2, storeEpoch: null, obcmVersion: null });
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
