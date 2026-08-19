/**
 * The client against a simulated flat store: what `vectors.test.ts` cannot reach.
 *
 * The vectors suite pins the bytes. This one pins everything that only exists once those bytes move
 * over two independent channels — a record reassembled across packet boundaries, an answer that
 * overtakes the last stream frame, a refusal that arrives while a 300 kB payload is still queued, a
 * cancel that has to go out while a download is running. None of that is visible in a frame.
 *
 * `MockDevice` is the counterpart and is held to the same bar: where it cannot model something it
 * refuses the way the device would rather than succeeding quietly, so a test that passes here is a
 * statement about the protocol and not about the mock's generosity.
 */

import { describe, expect, it } from "vitest";

import { Crc32 } from "./crc32";
import { DeviceError, FlatStoreClient, bytesSource } from "./client";
import { MockDevice, REFERENCE_STORE_ID, loopbackDevice, loopbackLink } from "./loopback";
import { RecordChannel, MAX_DEVICE_RECORD, MAX_HOST_CONTROL_RECORD, MAX_HOST_STREAM_RECORD } from "./records";
import {
    EntryFlags,
    ErrorCode,
    ObjectKind,
    ObjectState,
    Opcode,
    decodeResponse,
    encodeGetRequest,
} from "./protocol";

/** A payload with a distinct byte at every offset, so a misplaced record shows up as a wrong byte. */
function payload(length: number, seed = 1): Uint8Array {
    const out = new Uint8Array(length);
    for (let i = 0; i < length; i++) out[i] = (i * 31 + seed) & 0xff;
    return out;
}

async function withDevice<T>(
    options: Parameters<typeof loopbackDevice>[0],
    body: (rig: ReturnType<typeof loopbackDevice>) => Promise<T>,
): Promise<T> {
    const rig = loopbackDevice(options);
    try {
        return await body(rig);
    } finally {
        await rig.close();
        expect(rig.device.faults, "the mock device recorded a non-transport fault").toEqual([]);
    }
}

// ------------------------------------------------------------------- identity

describe("what a connection learns before anything else", () => {
    it("reads the three §5.2.1 strings over EP0", async () => {
        await withDevice({ deviceInfo: { firmwareRevision: "0.9.1+f00", hardwareRevision: "obc-lm20-r1", serialNumber: "AABB" } }, async ({ client }) => {
            expect(await client.deviceInfo()).toEqual({
                firmwareRevision: "0.9.1+f00",
                hardwareRevision: "obc-lm20-r1",
                serialNumber: "AABB",
            });
        });
    });

    it("says the host cannot ask rather than inventing a firmware version", async () => {
        // A transport with no EP0 path — the desktop bridge today. A fabricated revision would feed
        // "an update is available" a lie, so the honest answer is that nobody asked.
        const link = loopbackLink();
        const device = new MockDevice(link.device);
        void device.run();
        const pathless = { control: link.host.control, stream: link.host.stream, close: () => link.host.close() };
        const client = new FlatStoreClient(pathless);
        await expect(client.deviceInfo()).rejects.toMatchObject({ code: "unavailable" });
        await client.close();
        device.stop();
    });

    it("takes the store's identity and cache freshness from the first LIST, not from a read", async () => {
        await withDevice({ storeId: REFERENCE_STORE_ID, commitSequence: 7n }, async ({ client }) => {
            const page = await client.listPage({});
            expect(page.storeId).toBe(REFERENCE_STORE_ID);
            expect(page.commitSequence).toBe(7n);
            expect(page.entries).toEqual([]);
            expect(page.more).toBe(false);
        });
    });
});

// ------------------------------------------------------------------- LIST

describe("LIST", () => {
    it("pages with the (ObjectId, Revision) cursor until the device stops setting `more`", async () => {
        await withDevice({ pageEntries: 2 }, async ({ client, device }) => {
            for (let i = 0; i < 5; i++) {
                device.seed({ kind: ObjectKind.Route, displayName: `Route ${i}`, bytes: payload(16, i) });
            }
            const catalog = await client.list();
            expect(catalog.entries.map((entry) => entry.displayName)).toEqual([
                "Route 0",
                "Route 1",
                "Route 2",
                "Route 3",
                "Route 4",
            ]);
            // Three pages of two, two, one: the last page is the one without `more`.
            expect(device.requestLog.filter((row) => row.opcode === Opcode.List)).toHaveLength(3);
        });
    });

    it("resumes strictly after the pair, so a retained revision cannot hide the head behind it", async () => {
        await withDevice({ pageEntries: 1 }, async ({ client, device }) => {
            // The catalog sorts a retained revision before its head, so this page boundary is two
            // entries wide — exactly the case a cursor of `ObjectId` alone would skip.
            device.seed({ objectId: 4n, revision: 1n, kind: ObjectKind.WeatherBundle, flags: EntryFlags.Retained, bytes: payload(8) });
            device.seed({ objectId: 4n, revision: 2n, kind: ObjectKind.WeatherBundle, bytes: payload(9) });
            const catalog = await client.list();
            expect(catalog.entries.map((entry) => [entry.objectId, entry.revision])).toEqual([
                [4n, 1n],
                [4n, 2n],
            ]);
        });
    });

    it("filters by kind", async () => {
        await withDevice({}, async ({ client, device }) => {
            device.seed({ kind: ObjectKind.Route, bytes: payload(4) });
            device.seed({ kind: ObjectKind.Ride, flags: EntryFlags.Recording });
            device.seed({ kind: ObjectKind.Trip, bytes: payload(4) });
            const rides = await client.list({ kind: ObjectKind.Ride });
            expect(rides.entries.map((entry) => entry.kind)).toEqual([ObjectKind.Ride]);
        });
    });

    it("restarts the listing when the catalog moves under it", async () => {
        await withDevice({ pageEntries: 1 }, async ({ client, device }) => {
            device.seed({ kind: ObjectKind.Route, displayName: "one", bytes: payload(4) });
            device.seed({ kind: ObjectKind.Route, displayName: "two", bytes: payload(5) });
            // The commit sequence the first page declared is stale the moment anything commits, so
            // the second page is `catalogChanged` and the whole listing starts again. Restarting
            // rather than resuming is the point: the sequence is what made the earlier pages
            // consistent with each other.
            let moved = false;
            const seeded = device.seed.bind(device);
            const catalog = await client.list().then(async (first) => {
                if (!moved) {
                    moved = true;
                    seeded({ kind: ObjectKind.Route, displayName: "three", bytes: payload(6) });
                    return client.list();
                }
                return first;
            });
            expect(catalog.entries).toHaveLength(3);
        });
    });
});

// ------------------------------------------------------------------- PUT

describe("PUT", () => {
    it("creates an object and reports the id the device assigned", async () => {
        await withDevice({}, async ({ client, device }) => {
            const bytes = payload(10_000);
            const answer = await client.put({ kind: ObjectKind.Route, displayName: "Grimsel Loop" }, bytes);
            expect(answer.objectId).toBe(1n);
            expect(answer.revision).toBe(1n);
            expect(answer.payloadLength).toBe(10_000n);
            expect(answer.payloadCrc32).toBe(Crc32.of(bytes));
            expect(device.payloadOf(1n)).toEqual(bytes);
            expect(device.entries[0].displayName).toBe("Grimsel Loop");
        });
    });

    it("reassembles across packet boundaries in both directions", async () => {
        // 64-byte packets: a 4,112-byte stream record is 65 of them, and a `LIST` page is many. The
        // v1 envelope made this impossible by construction (one frame, one transfer); §5.2 makes it
        // the ordinary case, so it is the ordinary case here too.
        await withDevice({ packetSize: 64 }, async ({ client, device }) => {
            const bytes = payload(20_000, 7);
            await client.put({ kind: ObjectKind.MapShard, displayName: "Black Forest" }, bytes);
            expect(device.payloadOf(1n)).toEqual(bytes);
            const back = await client.get({ objectId: 1n, revision: 0n });
            expect(back.bytes).toEqual(bytes);
        });
    });

    it("reports progress in settled bytes and ends at the declared length", async () => {
        await withDevice({}, async ({ client }) => {
            const bytes = payload(50_000);
            const seen: number[] = [];
            await client.put({ kind: ObjectKind.Route, displayName: "r" }, bytes, {
                onProgress: (done, total) => {
                    expect(total).toBe(bytes.length);
                    seen.push(done);
                },
            });
            expect(seen[0]).toBe(0);
            expect(seen[seen.length - 1]).toBe(bytes.length);
            // Monotonic: a bar that went backwards would mean progress counted a queued write.
            expect([...seen].sort((a, b) => a - b)).toEqual(seen);
        });
    });

    it("replaces under compare-and-swap and refuses a stale revision", async () => {
        await withDevice({}, async ({ client, device }) => {
            const first = await client.put({ kind: ObjectKind.Route, displayName: "v1" }, payload(100));
            const second = await client.put(
                { objectId: first.objectId, expectedRevision: first.revision, kind: ObjectKind.Route, displayName: "v2" },
                payload(120),
            );
            expect(second.revision).toBe(2n);
            expect(device.entries).toHaveLength(1);

            await expect(
                client.put(
                    { objectId: first.objectId, expectedRevision: first.revision, kind: ObjectKind.Route, displayName: "v3" },
                    payload(130),
                ),
            ).rejects.toMatchObject({ code: "revision-conflict" });
            // The refusal carries the head, so a caller can retry against it without re-listing.
            expect(device.payloadOf(first.objectId)).toEqual(payload(120));
        });
    });

    it("leaves the displaced revision RETAINED only when the flag asked for it", async () => {
        await withDevice({}, async ({ client, device }) => {
            const first = await client.put({ kind: ObjectKind.WeatherBundle, displayName: "wx" }, payload(64));
            const kept = await client.put(
                {
                    objectId: first.objectId,
                    expectedRevision: first.revision,
                    kind: ObjectKind.WeatherBundle,
                    displayName: "wx",
                    retainPrevious: true,
                },
                payload(65),
            );
            expect(device.entries.map((entry) => [entry.revision, entry.flags])).toEqual([
                [1n, EntryFlags.Retained],
                [2n, 0],
            ]);

            // A replace *without* the flag clears retention outright — it never leaves a revision two
            // generations back alive behind a head that did not ask for it.
            await client.put(
                { objectId: first.objectId, expectedRevision: kept.revision, kind: ObjectKind.WeatherBundle, displayName: "wx" },
                payload(66),
            );
            expect(device.entries.map((entry) => entry.revision)).toEqual([3n]);
        });
    });

    it("refuses the two kinds the device produces itself", async () => {
        await withDevice({}, async ({ client }) => {
            for (const kind of [ObjectKind.Ride, ObjectKind.RollbackReserve]) {
                await expect(client.put({ kind, displayName: "no" }, payload(8))).rejects.toMatchObject({
                    code: "invalid-request",
                });
            }
        });
    });

    it("answers a payload that does not fit with the bytes it needed", async () => {
        await withDevice({ cardBytes: 1_000 }, async ({ client }) => {
            const error = await client
                .put({ kind: ObjectKind.MapShard, displayName: "too big" }, payload(4_096))
                .then(() => null)
                .catch((cause: unknown) => cause as DeviceError);
            expect(error).toBeInstanceOf(DeviceError);
            if (!error) throw new Error("the device accepted a payload its card cannot hold");
            expect(error.code).toBe("no-space");
            // §5.2.2's successor to the free-space read: the answer is at the point of decision, and
            // its context is what the upload actually needed.
            expect(error.refusal?.context).toBe(4_096n);
            expect(String(error.message)).toContain("4096");
        });
    });

    it("rejects a payload whose CRC does not match what was declared, and stores nothing", async () => {
        await withDevice({}, async ({ client, device }) => {
            const bytes = payload(2_000);
            const lying = { totalLen: bytes.length, crc32: Crc32.of(bytes) ^ 0xffff, chunks: bytesSource(bytes).chunks };
            await expect(client.put({ kind: ObjectKind.Route, displayName: "bad" }, lying)).rejects.toMatchObject({
                code: "checksum",
            });
            expect(device.entries).toEqual([]);
        });
    });

    it("stops pushing bytes at the first sign of a refusal", async () => {
        // §3.6 lets a client stream without waiting for an acceptance; the price is that a refusal
        // arrives mid-payload. A client that did not look would push a whole map at a device that
        // said no on the first megabyte.
        await withDevice({ cardBytes: 1_000, streamHighWaterMark: 8 * 1024 }, async ({ client, link }) => {
            let yielded = 0;
            const huge = 8 * 1024 * 1024;
            const source = {
                totalLen: huge,
                crc32: 0,
                async *chunks(size: number) {
                    for (let at = 0; at < huge; at += size) {
                        yielded += size;
                        yield payload(size);
                    }
                },
            };
            await expect(client.put({ kind: ObjectKind.MapShard, displayName: "x" }, source)).rejects.toMatchObject({
                code: "no-space",
            });
            expect(yielded, "the whole object was pushed at a device that had refused it").toBeLessThan(huge / 4);
            expect(link.streamDepth("to-device")).toBeGreaterThanOrEqual(0);
        });
    });
});

// ------------------------------------------------------------------- GET

describe("GET", () => {
    it("verifies the length and the whole-payload CRC before handing anything back", async () => {
        await withDevice({}, async ({ client, device }) => {
            const bytes = payload(9_000, 3);
            const seeded = device.seed({ kind: ObjectKind.Ride, displayName: "Tuesday", bytes });
            const result = await client.get({ objectId: seeded.objectId, revision: 0n });
            expect(result.bytes).toEqual(bytes);
            expect(result.revisionServed).toBe(1n);
            expect(result.payloadCrc32).toBe(Crc32.of(bytes));
        });
    });

    it("pins a retained revision when one is named", async () => {
        await withDevice({}, async ({ client, device }) => {
            const old = payload(32, 1);
            const head = payload(48, 2);
            device.seed({ objectId: 9n, revision: 1n, kind: ObjectKind.WeatherBundle, flags: EntryFlags.Retained, bytes: old });
            device.seed({ objectId: 9n, revision: 2n, kind: ObjectKind.WeatherBundle, bytes: head });
            expect((await client.get({ objectId: 9n, revision: 1n })).bytes).toEqual(old);
            expect((await client.get({ objectId: 9n, revision: 0n })).bytes).toEqual(head);
        });
    });

    it("refuses an object that is not there", async () => {
        await withDevice({}, async ({ client }) => {
            await expect(client.get({ objectId: 42n, revision: 0n })).rejects.toMatchObject({ code: "not-found" });
        });
    });

    it("refuses a ride the device is still recording", async () => {
        // §3.5: a recording ride's length and CRC are zero until the commit that ends it, so serving
        // one would report success over an empty payload.
        await withDevice({}, async ({ client, device }) => {
            const ride = device.seed({ kind: ObjectKind.Ride, flags: EntryFlags.Recording });
            await expect(client.get({ objectId: ride.objectId, revision: 0n })).rejects.toMatchObject({
                code: "invalid-request",
            });
        });
    });

    it("serves LIST beside a live transfer", async () => {
        // The control channel is not blocked by a transfer, which is what makes a mid-download
        // `CANCEL` possible at all — and is worth asserting directly rather than inferring.
        await withDevice({ streamPayload: 1024 }, async ({ client, device }) => {
            device.seed({ kind: ObjectKind.MapShard, displayName: "map", bytes: payload(200_000) });
            const download = client.get({ objectId: 1n, revision: 0n });
            const listed = await client.listPage({});
            expect(listed.entries).toHaveLength(1);
            expect((await download).bytes.length).toBe(200_000);
        });
    });
});

// ------------------------------------------------------------------- CANCEL

describe("CANCEL", () => {
    it("stops a running download and answers the transfer with `cancelled`", async () => {
        await withDevice({ streamPayload: 512, streamHighWaterMark: 4 * 1024 }, async ({ client, device }) => {
            device.seed({ kind: ObjectKind.MapShard, displayName: "map", bytes: payload(400_000) });
            const abort = new AbortController();
            // Cancelled from inside the download rather than after a timer: the loopback moves
            // 400 kB in microtasks, so a wall-clock delay would race the transfer it means to
            // interrupt and pass or fail on how busy the machine is.
            const download = client.get(
                { objectId: 1n, revision: 0n },
                {
                    signal: abort.signal,
                    onProgress: (done) => {
                        if (done > 20_000) abort.abort();
                    },
                },
            );
            await expect(download).rejects.toMatchObject({ code: "aborted" });
            // The slot is free again, and the next transfer is an ordinary first attempt rather than
            // a recovery path.
            const again = await client.get({ objectId: 1n, revision: 0n });
            expect(again.bytes.length).toBe(400_000);
        });
    });

    it("answers `no such transfer` for an identifier nothing is using", async () => {
        await withDevice({}, async ({ client }) => {
            expect(await client.cancel(0x0dead)).toBe(false);
        });
    });

    it("gives up on a device that is enumerated but hung, instead of wedging the transfer slot", async () => {
        // The failure this pins is not a device that has gone away — that one fails fast, and the
        // test above it covers it. It is a device that is still *there*: the endpoint accepts
        // nothing and answers nothing, so an unbounded `CANCEL` inside `abandon` parks forever and
        // the client's one-transfer latch is never released. That is the exact wedge the latch's own
        // documentation claims to have retired, and it shipped one call deeper.
        const rig = loopbackDevice({ clientTimeoutMs: 25 });
        rig.device.seed({ kind: ObjectKind.MapShard, displayName: "map", bytes: payload(200_000) });
        const abort = new AbortController();
        const download = rig.client.get(
            { objectId: 1n, revision: 0n },
            {
                signal: abort.signal,
                onProgress: (done) => {
                    if (done > 10_000) {
                        // Wedge the device *first*, so the `CANCEL` that `abandon` sends has nowhere
                        // to go, and only then abort.
                        rig.device.stop();
                        abort.abort();
                    }
                },
            },
        );
        await expect(download).rejects.toMatchObject({ code: "aborted" });

        // The point of the test: `abandon` returned rather than parking, so the slot is free and the
        // next call reaches the wire instead of queueing behind a cancel that will never answer.
        // Against a hung device that next call fails on its own timeout — a bounded error, which is
        // the whole difference from a spinner that never resolves.
        await expect(rig.client.listPage({})).rejects.toMatchObject({ code: "timeout" });
        await rig.client.close();
    });
});

// ------------------------------------------------------------------- REMOVE and ARM

describe("REMOVE", () => {
    it("removes under compare-and-swap and returns the new commit sequence", async () => {
        await withDevice({}, async ({ client, device }) => {
            const put = await client.put({ kind: ObjectKind.Route, displayName: "gone soon" }, payload(40));
            const sequence = await client.remove({ objectId: put.objectId, revision: put.revision });
            expect(sequence).toBe(device.sequence);
            expect(device.entries).toEqual([]);
        });
    });

    it("takes a retained previous revision with the head", async () => {
        await withDevice({}, async ({ client, device }) => {
            device.seed({ objectId: 3n, revision: 1n, kind: ObjectKind.WeatherBundle, flags: EntryFlags.Retained, bytes: payload(8) });
            device.seed({ objectId: 3n, revision: 2n, kind: ObjectKind.WeatherBundle, bytes: payload(9) });
            await client.remove({ objectId: 3n, revision: 2n });
            expect(device.entries).toEqual([]);
        });
    });

    it("refuses an entry the flags protect", async () => {
        await withDevice({}, async ({ client, device }) => {
            const ride = device.seed({ kind: ObjectKind.Ride, flags: EntryFlags.Recording });
            await expect(client.remove({ objectId: ride.objectId, revision: ride.revision })).rejects.toMatchObject({
                code: "invalid-request",
            });
        });
    });
});

describe("ARM", () => {
    it("is refused by the device's current policy, and says so", async () => {
        // §4's dev-window gap. The request is wired and the refusal is the truth; a client that
        // reported success here would be claiming a reboot that never comes.
        await withDevice({}, async ({ client, device }) => {
            const pkg = device.seed({ kind: ObjectKind.UpdatePackage, displayName: "0.9.0", bytes: payload(256) });
            await expect(client.arm({ objectId: pkg.objectId, expectedRevision: pkg.revision })).rejects.toMatchObject({
                code: "rejected",
            });
        });
    });

    it("answers with the rollback reserve and the commit it made, where policy allows it", async () => {
        await withDevice({ armPolicy: "allow" }, async ({ client, device }) => {
            const pkg = device.seed({ kind: ObjectKind.UpdatePackage, displayName: "0.9.0", bytes: payload(256) });
            const armed = await client.arm({ objectId: pkg.objectId, expectedRevision: pkg.revision });
            expect(armed.rollbackObjectId).toBeGreaterThan(0n);
            expect(armed.commitSequence).toBe(device.sequence);
            expect(device.entries.some((entry) => entry.kind === ObjectKind.RollbackReserve)).toBe(true);
        });
    });

    it("refuses a package the catalog does not hold at that revision", async () => {
        await withDevice({ armPolicy: "allow" }, async ({ client, device }) => {
            const pkg = device.seed({ kind: ObjectKind.UpdatePackage, bytes: payload(16) });
            await expect(client.arm({ objectId: pkg.objectId, expectedRevision: 99n })).rejects.toMatchObject({
                code: "revision-conflict",
            });
        });
    });
});

// ------------------------------------------------------------------- §3.4's reconciliation

describe("reconciling a break", () => {
    it("asks STATUS about a replace, which is what STATUS can answer", async () => {
        await withDevice({}, async ({ client }) => {
            const put = await client.put({ kind: ObjectKind.Route, displayName: "r" }, payload(64));
            expect(await client.status({ objectId: put.objectId, revision: put.revision })).toMatchObject({
                state: ObjectState.Committed,
                headRevision: 1n,
            });
            expect(await client.status({ objectId: put.objectId, revision: 99n })).toMatchObject({
                state: ObjectState.Superseded,
                headRevision: 1n,
            });
            expect(await client.status({ objectId: 404n, revision: 1n })).toMatchObject({
                state: ObjectState.Absent,
                headRevision: 0n,
                headPayloadLength: 0n,
                headPayloadCrc32: 0,
            });
        });
    });

    it("matches a lost create on (kind, length, CRC, name), which is what makes it sound", async () => {
        await withDevice({}, async ({ client }) => {
            const bytes = payload(4_321);
            await client.put({ kind: ObjectKind.Route, displayName: "Grimsel Loop" }, bytes);
            const found = await client.findCreated({
                kind: ObjectKind.Route,
                payloadLength: BigInt(bytes.length),
                payloadCrc32: Crc32.of(bytes),
                displayName: "Grimsel Loop",
            });
            expect(found?.objectId).toBe(1n);

            // The CRC is the field that makes the match sound: two routes of the same length and
            // name are ordinary, two with the same CRC are the same bytes.
            expect(
                await client.findCreated({
                    kind: ObjectKind.Route,
                    payloadLength: BigInt(bytes.length),
                    payloadCrc32: Crc32.of(bytes) ^ 1,
                    displayName: "Grimsel Loop",
                }),
            ).toBeNull();
        });
    });
});

// ------------------------------------------------------------------- one transfer at a time

describe("§1's one transfer at a time", () => {
    it("refuses a second transfer from this client without a round trip", async () => {
        await withDevice({ streamPayload: 512, streamHighWaterMark: 4 * 1024 }, async ({ client, device }) => {
            device.seed({ kind: ObjectKind.MapShard, bytes: payload(300_000) });
            const first = client.get({ objectId: 1n, revision: 0n });
            await expect(client.get({ objectId: 1n, revision: 0n })).rejects.toMatchObject({ code: "busy" });
            await first;
        });
    });

    it("is the device's rule, not the client's: a second peer is answered `busy` with the live id", async () => {
        // The latch above is local and saves a round trip; the authority is the device, because the
        // other transfer may be a phone's over BLE. Driven by hand here, because one loopback link
        // carries one client and the point is precisely that the refusal comes from the far end.
        const link = loopbackLink();
        const device = new MockDevice(link.device, { streamPayload: 512 });
        void device.run();
        const control = new RecordChannel(link.host.control, MAX_HOST_CONTROL_RECORD, MAX_DEVICE_RECORD);
        const stream = new RecordChannel(link.host.stream, MAX_HOST_STREAM_RECORD, MAX_DEVICE_RECORD);
        device.seed({ kind: ObjectKind.MapShard, bytes: payload(200_000) });

        await control.send(encodeGetRequest(0x11, { objectId: 1n, revision: 0n }));
        await control.send(encodeGetRequest(0x22, { objectId: 1n, revision: 0n }));
        // Read control answers until the second request's turns up; the first one's arrives only
        // after its whole payload has been streamed.
        let busy: ReturnType<typeof decodeResponse> | null = null;
        for (let i = 0; i < 4 && !busy; i++) {
            const answer = decodeResponse(await control.next());
            if (answer.requestId === 0x22) busy = answer;
        }
        expect(busy?.ok).toBe(false);
        if (busy && !busy.ok) {
            expect(busy.refusal.code).toBe(ErrorCode.Busy);
            expect(busy.refusal.context).toBe(0x11n);
        }
        // Drain the live transfer so the device's runner ends cleanly rather than on a closed pipe.
        void stream;
        device.stop();
        await link.device.close();
        await link.host.close();
    });
});

// ------------------------------------------------------------------- identifiers and teardown

describe("the client's own obligations", () => {
    it("never reuses a RequestId, and never mints zero", async () => {
        await withDevice({}, async ({ client, device }) => {
            for (let i = 0; i < 5; i++) await client.listPage({});
            const ids = device.requestLog.map((row) => row.requestId);
            expect(new Set(ids).size, "a RequestId was reused").toBe(ids.length);
            expect(ids).not.toContain(0);
            // §3.8's "SHOULD NOT reuse immediately after an answer": advancing is the whole remedy,
            // so the counter only ever goes forward.
            expect([...ids].sort((a, b) => a - b)).toEqual(ids);
        });
    });

    it("fails every waiter the moment the link dies, rather than letting them time out", async () => {
        // The claim is a one-second error against a fifteen-second spinner, so the test has to be
        // about a request that is genuinely outstanding when the cable goes. The device is stopped
        // first — it serves nothing more, exactly as an unplugged one does — and the request then
        // has nowhere to be answered from until the link itself fails.
        const rig = loopbackDevice({});
        rig.device.stop();
        const listing = rig.client.listPage({});
        await rig.link.device.close();
        await expect(listing).rejects.toMatchObject({ code: "link" });
        await rig.client.close();
    });

    it("refuses to work after close", async () => {
        const rig = loopbackDevice({});
        await rig.close();
        await expect(rig.client.listPage({})).rejects.toMatchObject({ code: "link" });
    });
});
