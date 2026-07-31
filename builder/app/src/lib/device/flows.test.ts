/**
 * The three flows, end to end against the simulated device (C4, #903).
 *
 * The LM20's USB peripheral does not exist yet (#889), so this is where "it works" is decided.
 * `loopback.ts` is not an echo: it assigns ids, dedups a re-uploaded object, answers a second
 * transfer `busy`, hands bulk bytes over in packet-sized slices and runs the abort handshake — so a
 * flow that gets any of those wrong fails here rather than on a rider's desk.
 *
 * What these tests are **not** is a substitute for hardware. Nothing here proves the LM20 enumerates,
 * that its endpoints have the sizes assumed, or that a real SD write keeps up; those wait for #889.
 * What they do prove is that the object-model half — the half that is byte-identical across BLE and
 * USB — is right, including the two failure paths that are easiest to get wrong and worst to get
 * wrong: a cancelled write and an unplug mid-transfer.
 */

import { createHash } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { beforeAll, describe, expect, it, vi } from "vitest";

import { DeviceError, type ProtocolClient } from "../usb/client";
import { loopbackDevice } from "../usb/loopback";
import { ObjectType, SINGLETON_OBJECT_ID } from "../usb/protocol";
import { initConvert } from "../convert/bridge";
import { prepareRoute } from "./route";
import {
    abandonAssembledSet,
    askToInstall,
    sendAssembledSetFile,
    sendMapFile,
    sendRoute,
    setSendState,
    stageFirmware,
} from "./write";
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
const digest = (bytes: Uint8Array) => createHash("sha256").update(bytes).digest("hex");

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

// --- the flows ----------------------------------------------------------------

describe("assembled volume-set upload", () => {
    it("verifies and commits shards in order, with the manifest last", async () => {
        const { client, device, close } = loopbackDevice();
        try {
            const shard = syntheticBytes(4096);
            const manifest = syntheticBytes(128);
            const state = setSendState(1, shard.length + manifest.length);
            const ctx = context();
            await sendAssembledSetFile(
                client,
                state,
                { name: "MS1S00.OBM", role: "core", sha256: digest(shard), byteLength: shard.length, bytes: shard },
                ctx,
            );
            expect(device.stagedMapShardCount).toBe(1);
            await sendAssembledSetFile(
                client,
                state,
                { name: "MS1.OBS", role: "manifest", sha256: "", byteLength: manifest.length, bytes: manifest },
                ctx,
            );
            expect(state.setId).toBe(1);
            expect(state.committedBytes).toBe(state.totalBytes);
            expect(device.stagedMapShardCount).toBe(0);
        } finally {
            await close();
        }
    });

    it("abandons already committed shards when assembly stops between files", async () => {
        const { client, device, close } = loopbackDevice();
        try {
            const shard = syntheticBytes(1024);
            const state = setSendState(2, 4096);
            await sendAssembledSetFile(
                client,
                state,
                { name: "MS1S00.OBM", role: "core", sha256: digest(shard), byteLength: shard.length, bytes: shard },
                context(),
            );
            expect(device.stagedMapShardCount).toBe(1);
            await abandonAssembledSet(client, state);
            expect(device.stagedMapShardCount).toBe(0);
        } finally {
            await close();
        }
    });

    it("refuses a worker buffer whose SHA-256 no longer matches", async () => {
        const { client, device, close } = loopbackDevice();
        try {
            const shard = syntheticBytes(512);
            const state = setSendState(1, shard.length);
            await expect(
                sendAssembledSetFile(
                    client,
                    state,
                    {
                        name: "MS1S00.OBM",
                        role: "core",
                        sha256: "0".repeat(64),
                        byteLength: shard.length,
                        bytes: shard,
                    },
                    context(),
                ),
            ).rejects.toThrow("SHA-256");
            expect(device.stagedMapShardCount).toBe(0);
        } finally {
            await close();
        }
    });

    it("retries one whole shard after a device CRC refusal", async () => {
        const shard = syntheticBytes(512);
        const upload = vi
            .fn()
            .mockRejectedValueOnce(new DeviceError("crc-mismatch", "bad wire CRC"))
            .mockResolvedValue({ objectId: 0x0100, committedOffset: shard.length });
        const client = { upload } as unknown as ProtocolClient;
        const state = setSendState(1, shard.length);
        await sendAssembledSetFile(
            client,
            state,
            { name: "MS1S00.OBM", role: "core", sha256: digest(shard), byteLength: shard.length, bytes: shard },
            context(),
        );
        expect(upload).toHaveBeenCalledTimes(2);
        expect(state.nextShard).toBe(1);
    });
});

describe("map upload from a file", () => {
    it("commits it and dedups a second send of the same file", async () => {
        const { client, device, close } = loopbackDevice();
        try {
            const bytes = syntheticBytes(200_000);
            const file = new File([bytes], "grimsel-default.obcm");
            const first = await sendMapFile(client, file, context());
            expect(device.stored(ObjectType.Map, first.objectId)).toEqual(bytes);

            // §4.1: a fresh upload whose length and CRC match something already stored is answered
            // with the *existing* id and stores nothing — so sending the same map twice cannot
            // fill the card with twins.
            const second = await sendMapFile(client, file, context());
            expect(second.objectId).toBe(first.objectId);
        } finally {
            await close();
        }
    });

    it("reports an unplug mid-transfer, and the next attempt is an ordinary one", async () => {
        const first = loopbackDevice({ bulkPacketSize: 4096, bulkHighWaterMark: 8 * 1024 });
        const bytes = syntheticBytes(2 * 1024 * 1024);
        const file = new File([bytes], "big.obcm");
        const ctx = context({
            at: (done, phase) => {
                // Pull the cable a little way into the *send*, not the read: mid-stream is the
                // state with a partial file at the far end.
                if (phase === "sending" && done > 256 * 1024) void first.link.device.close();
            },
        });
        const failure = await sendMapFile(first.client, file, ctx).catch((e: unknown) => e);
        expect(failure).toBeInstanceOf(DeviceError);
        expect((failure as DeviceError).code).toBe("link");
        expect(first.device.stored(ObjectType.Map, 1), "a half-written map is never committed").toBeNull();
        await first.close();

        // Plugging it back in is a fresh session — nothing carried over from the dead one, no
        // resume, no repair. Transfers restart, they never resume (spec principle 4).
        const again = loopbackDevice({ bulkPacketSize: 4096 });
        try {
            const result = await sendMapFile(again.client, file, context());
            expect(result.committedOffset).toBe(bytes.length);
            expect(again.device.stored(ObjectType.Map, result.objectId)).toEqual(bytes);
        } finally {
            await again.close();
        }
    }, 30_000);

    it("cancels mid-send, and retries on the same link", async () => {
        // The recovery property, on one connection: after a cancel the device has cleared its
        // gate and discarded the partial, and the pipe has been reset — so the retry is not a
        // special path, it is the first path again.
        const { client, device, close } = loopbackDevice({ bulkPacketSize: 4096, bulkHighWaterMark: 8 * 1024 });
        const bytes = syntheticBytes(1024 * 1024);
        const file = new File([bytes], "cancelled.obcm");
        const controller = new AbortController();
        try {
            const ctx = context({
                signal: controller.signal,
                at: (done, phase) => {
                    if (phase === "sending" && done > 128 * 1024) controller.abort();
                },
            });
            await expect(sendMapFile(client, file, ctx)).rejects.toMatchObject({ code: "aborted" });
            expect(device.stored(ObjectType.Map, 1)).toBeNull();

            const result = await sendMapFile(client, file, context());
            expect(result.committedOffset).toBe(bytes.length);
            expect(device.stored(ObjectType.Map, result.objectId)).toEqual(bytes);
        } finally {
            await close();
        }
    }, 30_000);

    it("refuses a second transfer while one is running", async () => {
        // Three surfaces share one client and one device; §4.1 allows exactly one transfer at a
        // time, and the answer has to be a clean error rather than two interleaved objects.
        const { client, close } = loopbackDevice({ bulkPacketSize: 4096, bulkHighWaterMark: 8 * 1024 });
        try {
            const file = new File([syntheticBytes(1024 * 1024)], "one.obcm");
            const running = sendMapFile(client, file, context());
            const second = sendMapFile(client, new File([syntheticBytes(4096)], "two.obcm"), context()).catch(
                (e: unknown) => e,
            );
            await running;
            expect((await second) as DeviceError).toMatchObject({ code: "busy" });
        } finally {
            await close();
        }
    });
});

describe("route upload", () => {
    it("converts a dropped GPX and sends the OBCR the device would have produced itself", async () => {
        const { client, device, close } = loopbackDevice();
        try {
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
            expect(device.stored(ObjectType.Route, result.objectId)).toEqual(prepared.obcr);
            // The device lists what it stored, so the route shows up in the catalog a rider reads.
            const { entries } = await client.listRoutes();
            expect(entries.map((e) => e.objectId)).toContain(result.objectId);
        } finally {
            await close();
        }
    });

    it("rejects a file that is not a route before anything is sent", async () => {
        const { client, device, close } = loopbackDevice();
        try {
            await expect(prepareRoute(new File([new Uint8Array(64)], "notes.txt"))).rejects.toMatchObject({
                name: "ConvertError",
            });
            expect(device.stored(ObjectType.Route, 1)).toBeNull();
            void client;
        } finally {
            await close();
        }
    });
});

describe("firmware update", () => {
    it("stages a verified container and then asks — it never installs", async () => {
        const { client, device, close } = loopbackDevice();
        try {
            const container = vector("update-container-v1.bin");
            const ctx = context();
            const { image, result } = await stageFirmware(client, container, ctx);
            expect(image.version).toBe("1.2.0+abc1234");
            // A fwImage upload is a singleton stage: id 0 in, id 0 back (§7.6).
            expect(result.objectId).toBe(SINGLETON_OBJECT_ID);
            expect(device.stagedFirmware).toEqual(container);
            expect(ctx.phases).toEqual(["verifying", "sending"]);

            // The command is a *request*. The device answering ok means it will show its confirm
            // card; the install still needs a physical Select press, and nothing here can skip it.
            await askToInstall(client);
        } finally {
            await close();
        }
    });

    it("refuses a damaged image locally, before spending a transfer on it", async () => {
        const { client, device, close } = loopbackDevice();
        try {
            const broken = Uint8Array.from(vector("update-container-v1.bin"));
            broken[70] ^= 0xff;
            await expect(stageFirmware(client, broken, context())).rejects.toMatchObject({ code: "image-crc" });
            expect(device.stagedFirmware).toBeNull();
        } finally {
            await close();
        }
    });

    it("reports 'nothing staged' rather than pretending an install was asked for", async () => {
        const { client, close } = loopbackDevice();
        try {
            await expect(askToInstall(client)).rejects.toMatchObject({ code: "not-found" });
        } finally {
            await close();
        }
    });
});

function syntheticBytes(total: number): Uint8Array<ArrayBuffer> {
    const bytes = new Uint8Array(total);
    for (let i = 0; i < bytes.length; i++) bytes[i] = i & 0xff;
    return bytes;
}
