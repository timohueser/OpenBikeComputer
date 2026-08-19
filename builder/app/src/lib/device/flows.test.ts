/**
 * The three flows, end to end against the simulated device (C4, #903).
 *
 * The LM20's USB peripheral does not exist yet (#889), so this is where "it works" is decided.
 * `loopback.ts` is not an echo: it assigns ids, enforces §3.6's compare-and-swap, answers a second
 * transfer `busy`, refuses a payload the card cannot hold with the bytes it needed, hands bulk bytes
 * over in packet-sized slices and runs the bilateral cancel — so a flow that gets any of those wrong
 * fails here rather than on a rider's desk.
 *
 * A map is **one object**, exactly as a route and a firmware image are: one `PUT`, one stream, one
 * whole-payload CRC, one commit. There is no multi-file map upload to test — no manifest, no
 * ordering rule between files, no state that outlives a transfer — so a map's tests are the same
 * tests the other two get, on a much larger object.
 *
 * What these tests are **not** is a substitute for hardware. Nothing here proves the LM20 enumerates,
 * that its endpoints have the sizes assumed, or that a real SD write keeps up; those wait for #889.
 * What they do prove is that the object-model half — the half that is byte-identical across BLE and
 * USB — is right, including the four failure paths that are easiest to get wrong and worst to get
 * wrong: a cancelled write, an unplug mid-transfer, a device that takes every byte and then refuses
 * them, and a card with no room for the object being pushed at it.
 */

import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { beforeAll, describe, expect, it } from "vitest";

import { DeviceError, FlatStoreClient } from "../usb/client";
import { Crc32 } from "../usb/crc32";
import { MockDevice, loopbackDevice, loopbackLink } from "../usb/loopback";
import type { BytePipe, DeviceLink } from "../usb/pipe";
import { ObjectKind } from "../usb/protocol";
import { initConvert } from "../convert/bridge";
import { prepareRoute } from "./route";
import { armUpdate, sendMapFile, sendRoute, stageFirmware } from "./write";
import type { JobContext, JobPhase } from "./progress";

// --- fixtures -----------------------------------------------------------------

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
});

// --- a job context a test can watch -------------------------------------------

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

/** The rig every happy-path flow runs on, with the mock's own defect log checked on the way out. */
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

// --- the flows ----------------------------------------------------------------

describe("map upload from a file", () => {
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

    it("makes a second object of a second send, and finds the first again by (kind, length, CRC, name)", async () => {
        // The v1 wire deduped a fresh upload whose length and CRC matched something stored and
        // answered with the *existing* id. v4 has no such rule — §3.4 puts the reconciliation on
        // this side instead, and `findCreated` is it: the match that recovers a create whose answer
        // was lost, rather than a device-side rule that quietly collapses two deliberate sends.
        await withDevice({}, async ({ client, device }) => {
            const bytes = syntheticBytes(64 * 1024);
            const file = new File([bytes], "grimsel-default.obcm");
            const first = await sendMapFile(client, file, context());
            const second = await sendMapFile(client, file, context());

            expect(second.objectId).not.toBe(first.objectId);
            expect(device.entries).toHaveLength(2);

            const found = await client.findCreated({
                kind: ObjectKind.MapShard,
                payloadLength: BigInt(bytes.length),
                payloadCrc32: Crc32.of(bytes),
                displayName: "grimsel-default",
            });
            expect(found?.objectId).toBe(first.objectId);
        });
    });

    it("answers a map the card cannot hold with the bytes it needed", async () => {
        // §5.2.2 retires the free-space query, so nothing asks in advance. §3.6 answers at the point
        // of decision instead, and its context is what this upload actually needed — which is what
        // lets the page say how much has to go rather than "not enough room".
        await withDevice({ cardBytes: 100_000 }, async ({ client, device }) => {
            const file = new File([syntheticBytes(256 * 1024)], "too-big.obcm");
            const failure = await sendMapFile(client, file, context()).catch((cause: unknown) => cause);
            expect(failure).toBeInstanceOf(DeviceError);
            expect((failure as DeviceError).code).toBe("no-space");
            expect((failure as DeviceError).refusal?.context).toBe(BigInt(256 * 1024));
            expect(device.entries, "a map that did not fit was committed anyway").toEqual([]);
        });
    });

    it("reports an unplug mid-transfer, and the next attempt is an ordinary one", async () => {
        const first = loopbackDevice({ packetSize: 4096, streamHighWaterMark: 8 * 1024 });
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

        // Plugging it back in is a fresh session — nothing carried over from the dead one, no
        // resume, no repair. §3.6: any break before the commit leaves the card as if nothing had
        // happened, and transfers restart rather than resume.
        await withDevice({ packetSize: 4096 }, async ({ client, device }) => {
            const result = await sendMapFile(client, file, context());
            expect(result.payloadLength).toBe(BigInt(bytes.length));
            expect(device.payloadOf(result.objectId)).toEqual(bytes);
        });
    }, 30_000);

    it("cancels mid-send, and retries on the same link", async () => {
        // The recovery property, on one connection: §3.8's cancel is bilateral, so the device has
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

    it("surfaces a device checksum refusal, keeps nothing, and lets the file go again on the same link", async () => {
        // The third failure shape, and the one that is neither a cancel nor an unplug: the device
        // took every announced byte, checked the whole-payload CRC it was promised (§3.6) and said
        // no. Nothing about it is recoverable *inside* the flow — a map is one object, so there is
        // no partial to resume — so what has to be true is that the refusal reaches the caller with
        // the device's own code, that no half-map is on the card, and that the channel reset which
        // follows leaves the link ordinary rather than desynchronised.
        //
        // That last part is why this runs against the loopback rather than a stubbed client: the
        // retry is what proves the abandon path actually ran.
        const link = loopbackLink({ packetSize: 4096, streamHighWaterMark: 8 * 1024 });
        const device = new MockDevice(link.device);
        void device.run();
        const wire = damageOneStreamWrite(link.host);
        const client = new FlatStoreClient(wire.link);
        try {
            const bytes = syntheticBytes(256 * 1024);
            const file = new File([bytes], "grimsel.obcm");

            wire.arm();
            await expect(sendMapFile(client, file, context())).rejects.toMatchObject({ code: "checksum" });
            expect(device.entries, "a refused map was committed anyway").toEqual([]);

            const result = await sendMapFile(client, file, context());
            expect(result.payloadLength).toBe(BigInt(bytes.length));
            expect(device.payloadOf(result.objectId)).toEqual(bytes);
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
            // §3.6's display name is what a catalog listing shows, and it is the route's own name —
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
            // The signed (v2) container — the only shape the device installs (`OBCU_Spec.md` §1.4),
            // and the trailer must reach it intact or it refuses the file as truncated.
            const container = vector("update-container-v2.bin");
            const ctx = context();
            const { image, result } = await stageFirmware(client, container, ctx);
            expect(image.version).toBe("1.2.0+abc1234");
            expect(image.sigScheme).toBe(1);
            expect(image.containerLen).toBe(container.length);
            expect(result.revision).toBe(1n);
            expect(device.payloadOf(result.objectId)).toEqual(container);
            expect(ctx.phases).toEqual(["verifying", "sending"]);

            // §3 has no singleton slot, so "one update package on the card" is this module's policy
            // and the compare-and-swap on the listed revision is what makes it safe. Staging again
            // must therefore bump the revision of the object that is there, not leave a second
            // multi-megabyte package for the rider to find.
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

            // …and so is an intact but *unsigned* one, which the device would refuse anyway (§1.4).
            const unsigned = vector("update-container-v1.bin");
            await expect(stageFirmware(client, unsigned, context())).rejects.toMatchObject({ code: "unsigned" });
            expect(device.entries).toEqual([]);
        });
    });

    it("surfaces the device's refusal to arm rather than reporting an install", async () => {
        // §4's dev-window gap: the device's current policy answers `ARM` with `rejected`. Staging is
        // not installing and never was, so the honest report is the refusal itself — a page that
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
 * framing stays intact and one payload byte does not, which is exactly the case §3.6's declared
 * whole-payload CRC exists to catch. Damaging the *client's* source instead would test the client's
 * arithmetic rather than the device's verdict.
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
