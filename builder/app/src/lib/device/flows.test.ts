/**
 * The three flows, end to end against the simulated device.
 *
 * The LM20's USB peripheral does not exist yet, so this is where "it works" is decided. The device on
 * the other end is the real protocol engine over a real flat store on a simulated card: it assigns
 * the ids, enforces the compare-and-swap, answers a second transfer `busy`, refuses a payload its
 * extents cannot hold with the bytes it needed, and runs the bilateral cancel.
 *
 * A map is **one object**, exactly as a route and a firmware image are: one `PUT`, one stream, one
 * whole-payload CRC, one commit. There is no multi-file map upload to test, so a map's tests are the
 * same tests the other two get, on a much larger object.
 *
 * Nothing here proves the LM20 enumerates, that its endpoints have the sizes assumed, or that a real
 * SD write keeps up. What it does prove is the object-model half, including the four failure paths
 * that are easiest to get wrong: a cancelled write, an unplug mid-transfer, a device that takes every
 * byte and then refuses them, and a card with no room for the object being pushed at it.
 */

import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { beforeAll, describe, expect, it, vi } from "vitest";

import { DeviceError, FlatStoreClient } from "../usb/client";
import { Crc32 } from "../usb/crc32";
import { FlatDevice, flatDevice } from "../usb/flat-device";
import { loopbackLink } from "../usb/loopback";
import type { BytePipe, DeviceLink } from "../usb/pipe";
import { ObjectKind } from "../usb/protocol";
import { initConvert } from "../convert/bridge";
import { loadFlatDevice } from "../../../test-support/flat-device/load";
import { prepareRoute } from "./route";
import { armUpdate, sendMapBlob, sendMapBytes, sendMapFile, sendRoute, stageFirmware } from "./write";
import type { JobContext, JobPhase } from "./progress";

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

beforeAll(async () => {
    const wasm = join(dirname(fileURLToPath(import.meta.url)), "..", "convert", "pkg", "obc_web_convert_bg.wasm");
    if (!existsSync(wasm)) {
        throw new Error(`the wasm bridge is not built (${wasm} missing). Run \`npm run build:wasm\`.`);
    }
    await initConvert(readFileSync(wasm));
    await loadFlatDevice();
});

interface Watched extends JobContext {
    readonly phases: JobPhase[];
    /** The last (done, total) pair reported. */
    readonly last: [number, number];
}

function context(options: { signal?: AbortSignal; at?: (done: number, phase: JobPhase) => void } = {}): Watched {
    const phases: JobPhase[] = [];
    let phase: JobPhase = "idle";
    let last: [number, number] = [0, 0];
    return {
        signal: options.signal ?? new AbortController().signal,
        cancel() {},
        phases,
        get last() {
            return last;
        },
        phase(next) {
            phase = next;
            phases.push(next);
        },
        progress(done, total) {
            last = [done, total];
            options.at?.(done, phase);
        },
    };
}

/** The rig every happy-path flow runs on, with the adapter's own defect log checked on the way out. */
async function withDevice<T>(
    options: Parameters<typeof flatDevice>[0],
    body: (rig: ReturnType<typeof flatDevice>) => Promise<T>,
): Promise<T> {
    const rig = flatDevice(options);
    try {
        return await body(rig);
    } finally {
        await rig.close();
        expect(rig.device.faults, "the device adapter saw a reaction it did not expect").toEqual([]);
    }
}

describe("map upload from a file", () => {
    it("can cancel the cooperative CRC pass of a resident assembler fallback", async () => {
        const controller = new AbortController();
        const list = vi.fn();
        const client = { list } as unknown as FlatStoreClient;
        const bytes = new Uint8Array(8 * 1024 * 1024);
        const ctx = context({ signal: controller.signal });
        setTimeout(() => controller.abort(new DOMException("cancelled", "AbortError")), 0);

        await expect(sendMapBytes(client, bytes, "fallback.obcm", ctx)).rejects.toMatchObject({
            name: "AbortError",
        });

        expect(ctx.last[0]).toBeGreaterThan(0);
        expect(ctx.last[0]).toBeLessThan(bytes.length);
        expect(list).not.toHaveBeenCalled();
    });

    it("streams an assembler Blob without requiring a picked File", async () => {
        await withDevice({}, async ({ client, device }) => {
            const bytes = syntheticBytes(200_000);
            const blob = new Blob([bytes]);
            const result = await sendMapBlob(client, blob, "built-monaco.obcm", context());

            expect(result.payloadLength).toBe(BigInt(bytes.length));
            expect(result.payloadCrc32).toBe(Crc32.of(bytes));
            expect(device.payloadOf(result.objectId)).toEqual(bytes);
            expect(device.entries[0].displayName).toBe("built-monaco");
        });
    });

    it("commits one object and reports the id the device assigned", async () => {
        await withDevice({}, async ({ client, device }) => {
            const bytes = syntheticBytes(200_000);
            const file = new File([bytes], "grimsel-default.obcm");
            const ctx = context();
            const result = await sendMapFile(client, file, ctx);

            expect(result.objectId).toBe(1n);
            expect(result.payloadLength).toBe(BigInt(bytes.length));
            expect(result.payloadCrc32).toBe(Crc32.of(bytes));
            expect(device.payloadOf(result.objectId)).toEqual(bytes);
            expect(device.entries[0].displayName).toBe("grimsel-default");
            // `committing` is named because the wire goes quiet there: the last byte is gone and the
            // device is still landing its staging half.
            expect(ctx.phases).toEqual(["reading", "sending", "committing"]);
        });
    });

    it("replaces the selected map on a second send instead of accumulating another object", async () => {
        await withDevice({}, async ({ client, device }) => {
            const bytes = syntheticBytes(64 * 1024);
            const file = new File([bytes], "grimsel-default.obcm");
            const first = await sendMapFile(client, file, context());
            const replacement = syntheticBytes(96 * 1024);
            const second = await sendMapFile(client, new File([replacement], "monaco.obcm"), context());

            expect(second.objectId).toBe(first.objectId);
            expect(second.revision).toBe(first.revision + 1n);
            expect(device.entries).toHaveLength(1);
            expect(device.entries[0].displayName).toBe("monaco");
            expect(device.payloadOf(first.objectId)).toEqual(replacement);
        });
    });

    it("replaces the active lowest-id map and leaves higher-id map objects alone", async () => {
        await withDevice({}, async ({ client, device }) => {
            // The store assigns the ids in the order the card was seeded, so the active map is the
            // first one on it and the secondary is the one after.
            const active = device.seed({ kind: ObjectKind.MapShard, displayName: "active", bytes: syntheticBytes(1024) });
            const secondary = device.seed({
                kind: ObjectKind.MapShard,
                displayName: "secondary",
                bytes: syntheticBytes(2048),
            });
            device.seed({ kind: ObjectKind.Route, displayName: "not a map", bytes: syntheticBytes(512) });

            const bytes = syntheticBytes(32 * 1024);
            const result = await sendMapFile(client, new File([bytes], "replacement.obcm"), context());

            expect(result.objectId).toBe(active.objectId);
            expect(result.revision).toBe(active.revision + 1n);
            expect(device.entries.filter((entry) => entry.kind === ObjectKind.MapShard)).toEqual([
                expect.objectContaining({ objectId: active.objectId, revision: 2n, displayName: "replacement" }),
                expect.objectContaining({ objectId: secondary.objectId, revision: 1n, displayName: "secondary" }),
            ]);
            expect(device.payloadOf(active.objectId)).toEqual(bytes);
        });
    });

    it("answers a map the card cannot hold with the bytes it needed", async () => {
        // Nothing asks about free space in advance. The refusal answers at the point of decision
        // instead, and its context is what this upload actually needed — which is what lets the page
        // say how much has to go rather than "not enough room".
        await withDevice({ extents: 1 }, async ({ client, device }) => {
            // One extent, taken by the object below: allocation is extent-granular, so this is a real
            // card with no room rather than a byte ceiling invented for the test.
            const held = device.seed({ kind: ObjectKind.Route, displayName: "the only extent", bytes: syntheticBytes(64) });
            const file = new File([syntheticBytes(256 * 1024)], "too-big.obcm");
            const failure = await sendMapFile(client, file, context()).catch((cause: unknown) => cause);
            expect(failure).toBeInstanceOf(DeviceError);
            expect((failure as DeviceError).code).toBe("no-space");
            expect((failure as DeviceError).refusal?.context).toBe(BigInt(256 * 1024));
            expect(device.entries, "a map that did not fit was committed anyway").toEqual([held]);
        });
    });

    it("reports an unplug mid-transfer, and the next attempt is an ordinary one", async () => {
        const first = flatDevice({ packetSize: 4096, streamHighWaterMark: 8 * 1024 });
        const bytes = syntheticBytes(2 * 1024 * 1024);
        const file = new File([bytes], "big.obcm");
        const ctx = context({
            at: (done, phase) => {
                // Pull the cable a little way into the *send*, not the read: mid-stream is the
                // state with a partial object at the far end.
                if (phase === "sending" && done > 256 * 1024) void first.link.device.close();
            },
        });
        const failure = await sendMapFile(first.client, file, ctx).catch((e: unknown) => e);
        expect(failure).toBeInstanceOf(DeviceError);
        expect((failure as DeviceError).code).toBe("link");
        expect(first.device.entries, "a half-written map is never committed").toEqual([]);
        await first.close();

        // Plugging it back in is a fresh session — nothing carried over from the dead one, no resume,
        // no repair. Any break before the commit leaves the card as if nothing had happened, and
        // transfers restart rather than resume.
        await withDevice({ packetSize: 4096 }, async ({ client, device }) => {
            const result = await sendMapFile(client, file, context());
            expect(result.payloadLength).toBe(BigInt(bytes.length));
            expect(device.payloadOf(result.objectId)).toEqual(bytes);
        });
    }, 30_000);

    it("cancels mid-send, and retries on the same link", async () => {
        // The recovery property, on one connection: the cancel is bilateral, so the device has
        // released its transfer slot and discarded the partial while this side reset its channel —
        // and the retry is therefore not a special path, it is the first path again.
        await withDevice({ packetSize: 4096, streamHighWaterMark: 8 * 1024 }, async ({ client, device }) => {
            const bytes = syntheticBytes(1024 * 1024);
            const file = new File([bytes], "cancelled.obcm");
            const controller = new AbortController();
            const ctx = context({
                signal: controller.signal,
                // Cancelled from inside the progress callback rather than after a timer: the
                // loopback moves a megabyte in microtasks, so a wall-clock delay would race the
                // transfer it means to interrupt.
                at: (done, phase) => {
                    if (phase === "sending" && done > 128 * 1024) controller.abort();
                },
            });
            await expect(sendMapFile(client, file, ctx)).rejects.toMatchObject({ code: "aborted" });
            expect(device.entries).toEqual([]);

            const result = await sendMapFile(client, file, context());
            expect(device.payloadOf(result.objectId)).toEqual(bytes);
        });
    }, 30_000);

    it("surfaces a device checksum refusal, keeps nothing, and lets the object go again on the same link", async () => {
        // The third failure shape, and the one that is neither a cancel nor an unplug: the device
        // took every announced byte, checked the whole-payload CRC it was promised and said no.
        // Nothing about it is recoverable *inside* the flow — each of these is one object, so there
        // is no partial to resume — so what has to be true is that the refusal reaches the caller
        // with the device's own code, that nothing half-written is on the card, and that the channel
        // reset which follows leaves the link ordinary rather than desynchronised.
        //
        // The object is a **firmware package** rather than a map, because the device does not rehash
        // a map that arrived over the cable: the packet CRC and its retries are the integrity
        // boundary there. It runs against the real device because the retry is what proves the
        // abandon path actually ran.
        const link = loopbackLink({ packetSize: 4096, streamHighWaterMark: 8 * 1024 });
        const device = new FlatDevice(link.device);
        void device.run();
        const wire = damageOneStreamWrite(link.host);
        const client = new FlatStoreClient(wire.link);
        try {
            const container = vector("update-container-v2.bin");

            wire.arm();
            await expect(stageFirmware(client, container, context())).rejects.toMatchObject({ code: "checksum" });
            expect(device.entries, "a refused package was committed anyway").toEqual([]);

            const { result } = await stageFirmware(client, container, context());
            expect(result.payloadLength).toBe(BigInt(container.length));
            expect(device.payloadOf(result.objectId)).toEqual(container);
        } finally {
            device.stop();
            await client.close();
            await link.device.close();
            expect(device.faults).toEqual([]);
        }
    }, 30_000);

    it("refuses a second transfer while one is running", async () => {
        // Three surfaces share one client and one device; §1 allows exactly one transfer at a time,
        // and the answer has to be a clean error rather than two interleaved objects.
        await withDevice({ packetSize: 4096, streamHighWaterMark: 8 * 1024 }, async ({ client }) => {
            const file = new File([syntheticBytes(1024 * 1024)], "one.obcm");
            const running = sendMapFile(client, file, context());
            const second = sendMapFile(client, new File([syntheticBytes(4096)], "two.obcm"), context()).catch(
                (e: unknown) => e,
            );
            await running;
            expect((await second) as DeviceError).toMatchObject({ code: "busy" });
        });
    });
});

describe("route upload", () => {
    it("converts a dropped GPX and sends the OBCR the device would have produced itself", async () => {
        await withDevice({}, async ({ client, device }) => {
            const gpx = readFileSync(join(ROOT, "host/obc-vectors/src/route-source.gpx"));
            // The route's name comes from the file's stem, which is what makes this comparable to
            // the checked-in vector: same input, same name, same bytes.
            const prepared = await prepareRoute(new File([gpx], "Vector Loop.gpx"));
            expect(prepared.obcr).toEqual(vector("route-waypoints.obcr"));
            expect(prepared.header).toMatchObject({
                name: "Vector Loop",
                pointCount: 9,
                distanceM: 2207,
                ascentM: 76,
            });

            const result = await sendRoute(client, prepared, context());
            expect(device.payloadOf(result.objectId)).toEqual(prepared.obcr);
                // The display name is what a catalog listing shows, and it is the route's own name —
                // so the row a rider reads on the device page is the row they dropped.
            const listed = await client.list({ kind: ObjectKind.Route });
            expect(listed.entries.map((entry) => [entry.objectId, entry.displayName])).toEqual([
                [result.objectId, "Vector Loop"],
            ]);
        });
    });

    it("rejects a file that is not a route before anything is sent", async () => {
        await withDevice({}, async ({ client, device }) => {
            await expect(prepareRoute(new File([new Uint8Array(64)], "notes.txt"))).rejects.toMatchObject({
                name: "ConvertError",
            });
            expect(device.entries).toEqual([]);
            void client;
        });
    });
});

describe("firmware update", () => {
    it("stages a verified container and replaces the one already on the card", async () => {
        await withDevice({}, async ({ client, device }) => {
                // The signed container — the only shape the device installs — and the trailer must
                // reach it intact or it refuses the file as truncated.
            const container = vector("update-container-v2.bin");
            const ctx = context();
            const { image, result } = await stageFirmware(client, container, ctx);
            expect(image.version).toBe("1.2.0+abc1234");
            expect(image.sigScheme).toBe(1);
            expect(image.containerLen).toBe(container.length);
            expect(result.revision).toBe(1n);
            expect(device.payloadOf(result.objectId)).toEqual(container);
            expect(ctx.phases).toEqual(["verifying", "sending"]);

                // There is no singleton slot on the wire, so "one update package on the card" is this
                // module's policy and the compare-and-swap on the listed revision is what makes it
                // safe. Staging again must bump the revision of the object that is there, not leave a
                // second multi-megabyte package for the rider to find.
            const again = await stageFirmware(client, container, context());
            expect(again.result.objectId).toBe(result.objectId);
            expect(again.result.revision).toBe(2n);
            expect(device.entries.filter((entry) => entry.kind === ObjectKind.UpdatePackage)).toHaveLength(1);
        });
    });

    it("refuses a damaged image locally, before spending a transfer on it", async () => {
        await withDevice({}, async ({ client, device }) => {
            const broken = Uint8Array.from(vector("update-container-v2.bin"));
            broken[70] ^= 0xff;
            await expect(stageFirmware(client, broken, context())).rejects.toMatchObject({ code: "image-crc" });
            expect(device.entries).toEqual([]);

                // …and so is an intact but *unsigned* one, which the device would refuse anyway.
            const unsigned = vector("update-container-v1.bin");
            await expect(stageFirmware(client, unsigned, context())).rejects.toMatchObject({ code: "unsigned" });
            expect(device.entries).toEqual([]);
        });
    });

    it("surfaces the device's refusal to arm rather than reporting an install", async () => {
        // A stated dev-window gap: the device's current policy answers `ARM` with `rejected`. Staging
        // is not installing and never was, so the honest report is the refusal itself — a page that
        // said "installing…" here would be claiming a reboot that never comes.
        await withDevice({}, async ({ client }) => {
            const container = vector("update-container-v2.bin");
            const { result } = await stageFirmware(client, container, context());
            await expect(armUpdate(client, { objectId: result.objectId, revision: result.revision })).rejects
                .toMatchObject({ code: "rejected" });
        });
    });

    it("arms the staged package where the device's policy allows it", async () => {
        await withDevice({ armPolicy: "allow" }, async ({ client, device }) => {
            const { result } = await stageFirmware(client, vector("update-container-v2.bin"), context());
            const armed = await armUpdate(client, { objectId: result.objectId, revision: result.revision });
            expect(armed.rollbackObjectId).toBeGreaterThan(0n);
            expect(armed.commitSequence).toBe(device.sequence);
            expect(device.entries.some((entry) => entry.kind === ObjectKind.RollbackReserve)).toBe(true);
        });
    });
});

/**
 * A host link that damages the payload of one stream write, once armed.
 *
 * The wire is where a checksum failure comes from, so this is where it is injected: the record
 * framing stays intact and one payload byte does not. Damaging the *client's* source instead would
 * test the client's arithmetic rather than the device's verdict.
 */
function damageOneStreamWrite(link: DeviceLink): { link: DeviceLink; arm: () => void } {
    const stream = link.stream;
    let armed = false;
    let damaged = false;
    const wrapped: BytePipe = {
        transport: stream.transport,
        get open() {
            return stream.open;
        },
        read: (signal) => stream.read(signal),
        write(bytes, signal) {
            if (!armed || damaged) return stream.write(bytes, signal);
            damaged = true;
            const flipped = bytes.slice();
            // The last byte of a batch is the last payload byte of its last record, so the frame's
            // own length prefix and offset survive and the device consumes every announced byte.
            flipped[flipped.length - 1] ^= 0xff;
            return stream.write(flipped, signal);
        },
        reset: () => stream.reset(),
        close: () => stream.close(),
    };
    return {
        link: { control: link.control, stream: wrapped, vendorIn: link.vendorIn, close: () => link.close() },
        arm: () => {
            armed = true;
        },
    };
}

function syntheticBytes(total: number): Uint8Array<ArrayBuffer> {
    const bytes = new Uint8Array(total);
    for (let i = 0; i < bytes.length; i++) bytes[i] = i & 0xff;
    return bytes;
}
