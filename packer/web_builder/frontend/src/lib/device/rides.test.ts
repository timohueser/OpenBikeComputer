/**
 * Web ride export, end to end against the simulated device (C5, #904).
 *
 * Two things are being decided here, and only one of them is "does the flow work".
 *
 * The first is **byte identity**: the GPX a visitor saves has to be the file the device itself would
 * have written. The pinned pair is `protocol-vectors/track-log.obct` → `track-export.gpx`, produced
 * by the real `obc_route::track_to_gpx`, and the export path has to land on those exact bytes after
 * a full round trip through the wire's ride object — with one documented exception the wire format
 * makes unavoidable, asserted as *the only* exception rather than waved at.
 *
 * The second is **that nothing happened to the device**. #894 locks `synced` as a durability
 * predicate — a flag whose `synced_at` stamp starts an auto-expiry countdown (#638) against the only
 * copy of a ride — and the hosted tier is the one sink that must never set it, because a browser
 * download can be cancelled at the save dialog. So a full list-and-export session is checked from
 * three sides: the device's command log stays empty, its `/tracks/SYNCED.SET` bytes are identical
 * before and after, and the object the export path is handed does not have an ack to send.
 */

import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { beforeAll, describe, expect, it } from "vitest";

import { initConvert } from "../convert/bridge";
import { DeviceError, ProtocolClient } from "../usb/client";
import {
    MockDevice,
    loopbackDevice,
    loopbackLink,
    type LoopbackOptions,
    type MockDeviceOptions,
} from "../usb/loopback";
import { encodeRideObject, type RideListEntry, type RideObject, type RidePoint } from "../usb/objects";
import type { BytePipe, DeviceLink } from "../usb/pipe";
import type { JobContext, JobPhase } from "./progress";
import {
    exportRide,
    rideAccess,
    rideDate,
    rideFilename,
    rideKey,
    rideToTrackLog,
    scopeKey,
    type RideSource,
} from "./rides";

// --- fixtures -----------------------------------------------------------------

function repoRoot(): string {
    let dir = dirname(fileURLToPath(import.meta.url));
    for (let up = 0; up < 12; up++) {
        if (existsSync(join(dir, "protocol-vectors", "manifest.json"))) return dir;
        dir = dirname(dir);
    }
    throw new Error("could not locate the repo root from " + import.meta.url);
}

const ROOT = repoRoot();
const vector = (name: string) => new Uint8Array(readFileSync(join(ROOT, "protocol-vectors", name)));

/** The name `track-export.gpx` was produced with (`obc_vectors::TRACK_NAME`). */
const TRACK_NAME = "Schauinsland & back";

beforeAll(async () => {
    const wasm = join(dirname(fileURLToPath(import.meta.url)), "..", "convert", "pkg", "obc_web_convert_bg.wasm");
    if (!existsSync(wasm)) {
        throw new Error(`the wasm bridge is not built (${wasm} missing). Run \`npm run build:wasm\`.`);
    }
    await initConvert(readFileSync(wasm));
});

const TRACK_RECORD_LEN = 20;

/**
 * The device's own Finish-time conversion, `obc_route::track_to_ride`, restated in the test.
 *
 * It is the *producer* of every ride object that ever crosses this wire, so the honest way to test
 * the pull side is to hand it exactly what the device would have sent — a ride object built from the
 * checked-in log by the same four rules (µdeg × 10, `lat, lon` order, whole-second offsets from the
 * first record, sensors 1:1) — rather than a ride object shaped to make the export pass.
 */
function rideFromTrackLog(log: Uint8Array, name: string, startTime: number): RideObject {
    const view = new DataView(log.buffer, log.byteOffset, log.byteLength);
    const total = Math.floor(log.length / TRACK_RECORD_LEN); // a trailing partial record is ignored
    const t0 = total > 0 ? view.getUint32(12, true) : 0;
    const points: RidePoint[] = [];
    for (let i = 0; i < total; i++) {
        const at = i * TRACK_RECORD_LEN;
        const hr = log[at + 16];
        const cadence = log[at + 17];
        const power = view.getUint16(at + 18, true);
        points.push({
            tOffsetS: Math.floor((view.getUint32(at + 12, true) - t0) / 1000),
            lat1e7: view.getInt32(at + 4, true) * 10,
            lon1e7: view.getInt32(at, true) * 10,
            eleM: view.getInt16(at + 8, true),
            hrBpm: hr === 0xff ? null : hr,
            cadenceRpm: cadence === 0xff ? null : cadence,
            powerW: power === 0xffff ? null : power,
        });
    }
    return {
        version: 2,
        name,
        startTime,
        distanceM: 4210,
        movingTimeS: 1_284,
        avgSpeedCms: 328,
        climbM: 118,
        avgHr: 135,
        maxHr: 138,
        avgCadence: 78,
        avgPower: 205,
        maxPower: 240,
        points,
    };
}

/** A ride list entry describing a stored ride object — what the device's catalog would report. */
function entryFor(objectId: number, ride: RideObject, bytes: Uint8Array): RideListEntry {
    return {
        objectId,
        byteLen: bytes.length,
        startTime: ride.startTime,
        distanceM: ride.distanceM,
        movingTimeS: ride.movingTimeS,
        avgSpeedCms: ride.avgSpeedCms,
        climbM: ride.climbM,
        name: ride.name,
    };
}

/** A long ride, for the paths that need a transfer still running when something goes wrong. */
function longRide(points: number): RideObject {
    const list: RidePoint[] = [];
    for (let i = 0; i < points; i++) {
        list.push({
            tOffsetS: i,
            lat1e7: 479_950_000 + i * 100,
            lon1e7: 78_420_000 + i * 100,
            eleM: 300 + (i % 200),
            hrBpm: 120 + (i % 40),
            cadenceRpm: 80,
            powerW: 200,
        });
    }
    return {
        version: 2,
        name: "Long Way Round",
        startTime: 1_783_598_400,
        distanceM: points * 8,
        movingTimeS: points,
        avgSpeedCms: 800,
        climbM: 900,
        avgHr: 140,
        maxHr: 160,
        avgCadence: 80,
        avgPower: 200,
        maxPower: 400,
        points: list,
    };
}

// --- a job context a test can watch --------------------------------------------

interface Watched extends JobContext {
    readonly phases: JobPhase[];
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

/** A device with seeded rides, and the read-only handle the export path gets. */
function deviceWith(
    rides: Array<{ id: number; ride: RideObject }>,
    options: LoopbackOptions & MockDeviceOptions = {},
) {
    const harness = loopbackDevice(options);
    const entries: RideListEntry[] = [];
    for (const { id, ride } of rides) {
        const bytes = encodeRideObject(ride);
        const entry = entryFor(id, ride, bytes);
        harness.device.seedRide(entry, bytes);
        entries.push(entry);
    }
    return { ...harness, entries, source: rideAccess(harness.client) };
}

// --- byte identity --------------------------------------------------------------

describe("the exported GPX", () => {
    /**
     * The one thing the wire cannot carry.
     *
     * `track_to_gpx` opens a fresh `<trkseg>` on every point flagged `segment_start`, so the pinned
     * fixture — a log with a pause in it — has two. The ride object (spec §7.2) has no segment flag:
     * the device drops it at Finish, in `track_to_ride`, for *every* peer. The phone's exporter says
     * the same thing ("The ride object carries no segment breaks, so the track is one `<trkseg>`").
     *
     * Collapsing exactly that one break — and nothing else — is what "byte-identical to the native
     * path for the same track" can honestly mean on this side of the wire. Deriving the expectation
     * from the fixture rather than hand-writing it means every other byte still has to match:
     * coordinates, elevations, the sensor extension shape, the XML escaping, the name.
     */
    function withoutSegmentBreak(gpx: string): string {
        const joined = gpx.replace("</trkseg>\n<trkseg>\n", "");
        expect(joined, "the fixture is expected to have exactly one mid-track segment break").not.toBe(gpx);
        expect(joined.match(/<trkseg>/g), "and only one after collapsing it").toHaveLength(1);
        return joined;
    }

    it("reproduces the native exporter byte-for-byte, pulled from the device", async () => {
        const log = vector("track-log.obct");
        const ride = rideFromTrackLog(log, TRACK_NAME, 1_783_598_400);
        const { client, entries, source, close } = deviceWith([{ id: 4, ride }]);
        try {
            // The catalog is what a rider picks from, so the export starts where they do.
            const listed = await source.listRides();
            expect(listed.entries.map((e) => e.objectId)).toEqual([4]);

            const exported = await exportRide(source, listed.entries[0], context());
            const expected = new TextDecoder().decode(vector("track-export.gpx"));
            expect(exported.gpx).toBe(withoutSegmentBreak(expected));
            expect(exported.points).toBe(5);
            expect(exported.bytes).toBe(entries[0].byteLen);
            void client;
        } finally {
            await close();
        }
    });

    it("is UTF-8 bytes, so the saved file is what was compared", async () => {
        // The comparison above is on a string; what reaches a Blob is bytes. An em dash or a
        // non-ASCII ride name is the case where those two stop agreeing if anything re-encodes.
        const log = vector("track-log.obct");
        const ride = rideFromTrackLog(log, "Höhenweg — Schauinsland", 1_783_598_400);
        const { source, close } = deviceWith([{ id: 1, ride }]);
        try {
            const exported = await exportRide(source, (await source.listRides()).entries[0], context());
            const bytes = new TextEncoder().encode(exported.gpx);
            expect(new TextDecoder("utf-8", { fatal: true }).decode(bytes)).toBe(exported.gpx);
            expect(exported.gpx).toContain("<trk><name>Höhenweg — Schauinsland</name>");
        } finally {
            await close();
        }
    });
});

describe("the ride object -> track log transcode", () => {
    /**
     * The inverse of `track_to_ride`, pinned field by field.
     *
     * Round-tripping the checked-in log through the wire's ride object and back must return the
     * same 20-byte records — *except* the two fields the ride object provably cannot carry. Naming
     * them as byte ranges rather than asserting "close enough" means a coordinate that started
     * rounding, an elevation that picked up a sentinel, or a sensor that lost its absence would fail
     * here with the record and the offset that moved.
     */
    it("returns every field the ride object carries, and only loses the two it does not", () => {
        const log = vector("track-log.obct");
        const ride = rideFromTrackLog(log, TRACK_NAME, 1_783_598_400);
        const back = rideToTrackLog(ride);

        const records = Math.floor(log.length / TRACK_RECORD_LEN);
        expect(back.length).toBe(records * TRACK_RECORD_LEN); // the trailing partial record is gone
        for (let i = 0; i < records; i++) {
            const at = i * TRACK_RECORD_LEN;
            const original = log.subarray(at, at + TRACK_RECORD_LEN);
            const roundTripped = back.subarray(at, at + TRACK_RECORD_LEN);
            // lon, lat, ele — exact: the device multiplied µdeg by 10, so every value divides back.
            expect(roundTripped.subarray(0, 10), `record ${i} coordinates/elevation`).toEqual(
                original.subarray(0, 10),
            );
            // hr, cadence, power — exact, sentinels and all.
            expect(roundTripped.subarray(16, 20), `record ${i} sensors`).toEqual(original.subarray(16, 20));
        }

        // The two losses, stated: the segment flag is gone (the ride object has no such field), and
        // the timestamp is whole seconds since the first point rather than the device's raw ms clock.
        const flagsAt = 10;
        expect(new DataView(log.buffer, log.byteOffset).getUint16(3 * TRACK_RECORD_LEN + flagsAt, true)).toBe(1);
        for (let i = 0; i < records; i++) {
            expect(new DataView(back.buffer).getUint16(i * TRACK_RECORD_LEN + flagsAt, true)).toBe(0);
        }
        expect(new DataView(back.buffer).getUint32(3 * TRACK_RECORD_LEN + 12, true)).toBe(63_000);
    });

    it("writes the device's own 'no barometer yet' value rather than a sentinel altitude", () => {
        // The firmware never emits `ELE_NONE`; it stamps 0 until the first baro sample. Another
        // encoder can, and `<ele>-32768</ele>` in a rider's GPX would be worse than 0.
        const ride: RideObject = {
            ...longRide(1),
            points: [{ tOffsetS: 0, lat1e7: 479_950_000, lon1e7: 78_420_000, eleM: null, hrBpm: null, cadenceRpm: null, powerW: null }],
        };
        const log = rideToTrackLog(ride);
        expect(new DataView(log.buffer).getInt16(8, true)).toBe(0);
    });
});

// --- the rule this issue exists to enforce ---------------------------------------

describe("the browser never acks", () => {
    /**
     * The structural half.
     *
     * `RideSource` holds two reads and nothing else, so an ack cannot be written against it — but a
     * type is only a compile-time fact, and a cast defeats it. `rideAccess` therefore hands back an
     * object that does not *have* the property either, so the cast throws instead of quietly
     * flagging a ride and starting an expiry countdown against it.
     */
    it("hands the export path a device it cannot write to", async () => {
        const { client, source, close } = deviceWith([]);
        try {
            expect(Object.keys(source).sort()).toEqual(["downloadRide", "listRides"]);
            for (const forbidden of ["ackRides", "command", "deleteObject", "setClock", "upload", "writeConfig"]) {
                expect(source, `\`${forbidden}\` must not be reachable from the export path`).not.toHaveProperty(
                    forbidden,
                );
            }
            expect(() => (source as unknown as ProtocolClient).ackRides([1])).toThrow(TypeError);
            expect(Object.isFrozen(source), "and it cannot be grown back").toBe(true);
            // The client itself still has it — this is a narrowing at the seam, not a removal.
            expect(typeof client.ackRides).toBe("function");
            // A `ProtocolClient` is deliberately *not* a `RideSource` — `downloadRide` exists
            // nowhere else — so `rideAccess` is the only way to obtain one, and there is no
            // "pass the client straight in" shortcut for a call site to take.
            const notASource: Partial<RideSource> = client as unknown as Partial<RideSource>;
            expect(notASource.downloadRide).toBeUndefined();
        } finally {
            await close();
        }
    });

    /**
     * The behavioural half, at the level the device would actually change.
     *
     * A ride the phone has already synced is the interesting starting state: the sidecar is
     * non-empty, so "unchanged" is a real comparison rather than two empty arrays. The setup ack is
     * the phone's; everything after the snapshot is the browser's, and the browser must add nothing.
     */
    it("leaves the synced sidecar byte-identical across a full list-and-export session", async () => {
        const ride = rideFromTrackLog(vector("track-log.obct"), TRACK_NAME, 1_783_598_400);
        const { client, device, source, close } = deviceWith([
            { id: 4, ride },
            { id: 7, ride: longRide(12) },
        ]);
        try {
            // The phone, on an earlier connect: a trusted clock, then an ack that stamps ride 4.
            await client.setClock(new Date(1_783_598_400_000), 120);
            await client.ackRides([4]);
            const before = device.syncedSidecar();
            const commandsBefore = device.commandLog.length;
            expect(before.length, "a non-empty sidecar, so 'unchanged' means something").toBeGreaterThan(10);

            // Everything from here is the hosted tier's session: list, then export both rides —
            // including the one the device already thinks is synced, and the one it does not.
            const listed = await source.listRides();
            expect(listed.entries.map((e) => e.objectId)).toEqual([4, 7]);
            for (const entry of listed.entries) {
                const exported = await exportRide(source, entry, context());
                expect(exported.gpx.startsWith("<?xml")).toBe(true);
            }

            expect(device.syncedSidecar(), "the browser wrote to /tracks/SYNCED.SET").toEqual(before);
            expect(device.commandLog.length, "the browser sent a command").toBe(commandsBefore);
            expect(device.synced, "the unsynced ride must stay unsynced").toEqual(new Set([4]));
        } finally {
            await close();
        }
    });

    it("sends no command of any kind to a device it has only read from", async () => {
        const ride = rideFromTrackLog(vector("track-log.obct"), TRACK_NAME, 1_783_598_400);
        const { device, source, close } = deviceWith([{ id: 4, ride }]);
        try {
            const empty = device.syncedSidecar();
            await exportRide(source, (await source.listRides()).entries[0], context());
            expect(device.commandLog).toEqual([]);
            expect(device.syncedSidecar()).toEqual(empty);
            // Guard the guard: an all-zero "nothing synced" sidecar is what unchanged looks like
            // here, so check it really is the empty one rather than a comparison of two blanks.
            expect(new DataView(empty.buffer).getUint16(6, true), "entry count").toBe(0);
            expect(device.synced.size).toBe(0);
        } finally {
            await close();
        }
    });

    /**
     * The guard against the edit that reintroduces it by hand.
     *
     * The type narrowing stops code written *against* `RideSource`; it does not stop somebody
     * changing the prop type back to `ProtocolClient` and calling the client directly. Scoped to the
     * files the browser's ride export is made of, so that E1/E2's desktop ack — which is required to
     * exist, after fsync — is not caught by a guard that would then have to be weakened.
     */
    it("never names the ack command anywhere on the browser's ride path", () => {
        const src = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
        const files = [
            "lib/device/rides.ts",
            "components/device/RideExport.svelte",
            "components/device/DeviceSurfaces.svelte",
        ];
        // The prose in `rides.ts` explains at length why the call is absent — and quotes it — so
        // the scan is over code with the comments taken out, not over the file's words.
        const code = (text: string): string =>
            text
                .replace(/\/\*[\s\S]*?\*\//g, "")
                .replace(/<!--[\s\S]*?-->/g, "")
                .split("\n")
                .filter((line) => !/^\s*(\/\/|\*)/.test(line))
                .join("\n");

        const offenders = files.filter((file) => /\.\s*ackRides\s*\(/.test(code(readFileSync(join(src, file), "utf8"))));
        expect(offenders).toEqual([]);
        // Guard the guard: the scan has to still find a call that is really there.
        expect(/\.\s*ackRides\s*\(/.test(code("await client.ackRides([4]);"))).toBe(true);
    });
});

// --- honest failure --------------------------------------------------------------

describe("when the export cannot finish", () => {
    /** A host link that flips one byte on its way in, once armed — a wire error the announced
     *  whole-object CRC has to catch before anything is handed to a rider. */
    function corruptible(link: DeviceLink): { link: DeviceLink; arm: () => void } {
        const bulk = link.bulk;
        let armed = false;
        let flipped = false;
        const wrapped: BytePipe = {
            transport: bulk.transport,
            get open() {
                return bulk.open;
            },
            async read(signal) {
                const slice = await bulk.read(signal);
                if (!armed || flipped || slice.length === 0) return slice;
                flipped = true;
                const damaged = slice.slice();
                damaged[0] ^= 0xff;
                return damaged;
            },
            write: (bytes, signal) => bulk.write(bytes, signal),
            reset: () => bulk.reset(),
            close: () => bulk.close(),
        };
        return { link: { control: link.control, bulk: wrapped, close: () => link.close() }, arm: () => (armed = true) };
    }

    it("refuses to offer a file whose bytes did not survive the cable", async () => {
        const raw = loopbackLink();
        const device = new MockDevice(raw.device);
        void device.run();
        const wire = corruptible(raw.host);
        const client = new ProtocolClient(wire.link);
        const ride = rideFromTrackLog(vector("track-log.obct"), TRACK_NAME, 1_783_598_400);
        const bytes = encodeRideObject(ride);
        device.seedRide(entryFor(4, ride, bytes), bytes);
        try {
            const source = rideAccess(client);
            const entry = (await source.listRides()).entries[0];
            wire.arm(); // the catalog arrived intact; damage the ride itself
            const failure = await exportRide(source, entry, context()).catch((e: unknown) => e);
            expect(failure).toBeInstanceOf(DeviceError);
            expect((failure as DeviceError).code).toBe("crc-mismatch");
            expect((failure as DeviceError).message).toMatch(/checksum/i);
            // Nothing is committed to a file on a failed checksum, and nothing was said to the
            // device about it either — a corrupt pull is still a pull that acked nothing.
            expect(device.commandLog).toEqual([]);
        } finally {
            device.stop();
            await client.close();
            await raw.device.close();
        }
    });

    it("reports an unplug mid-pull instead of leaving a spinner", async () => {
        // 30 000 points is about 540 KB on the wire — long enough that the cable can be pulled
        // while bytes are still moving, which is the state a stuck spinner comes from.
        const harness = deviceWith([{ id: 4, ride: longRide(30_000) }], {
            bulkPacketSize: 4096,
            bulkHighWaterMark: 8 * 1024,
            chunkSize: 4096,
        });
        const ctx = context({
            at: (done, phase) => {
                if (phase === "downloading" && done > 64 * 1024) void harness.link.device.close();
            },
        });
        const failure = await exportRide(harness.source, harness.entries[0], ctx).catch((e: unknown) => e);
        expect(failure).toBeInstanceOf(DeviceError);
        expect((failure as DeviceError).code).toBe("link");
        expect(ctx.phases).toEqual(["downloading"]); // it never reached the conversion
        await harness.close();
    }, 30_000);

    it("cancels mid-pull, and the next export on the same link is an ordinary one", async () => {
        const harness = deviceWith([{ id: 4, ride: longRide(30_000) }], {
            bulkPacketSize: 4096,
            bulkHighWaterMark: 8 * 1024,
            chunkSize: 4096,
        });
        const controller = new AbortController();
        try {
            const ctx = context({
                signal: controller.signal,
                at: (done) => {
                    if (done > 64 * 1024) controller.abort();
                },
            });
            await expect(exportRide(harness.source, harness.entries[0], ctx)).rejects.toMatchObject({
                code: "aborted",
            });
            // §4.1's recovery property: after a cancel the device has cleared its gate and the pipe
            // has been reset, so the retry is the first path again, not a repair path.
            const again = await exportRide(harness.source, harness.entries[0], context());
            expect(again.points).toBe(30_000);
        } finally {
            await harness.close();
        }
    }, 60_000);

    it("says the ride is gone when it was deleted on the device between listing and exporting", async () => {
        const ride = rideFromTrackLog(vector("track-log.obct"), TRACK_NAME, 1_783_598_400);
        const { source, entries, close } = deviceWith([{ id: 4, ride }]);
        try {
            const stale: RideListEntry = { ...entries[0], objectId: 99 };
            const failure = await exportRide(source, stale, context()).catch((e: unknown) => e);
            expect((failure as DeviceError).code).toBe("not-found");
        } finally {
            await close();
        }
    });

    it("explains an empty ride rather than failing inside the converter", async () => {
        const { source, close } = deviceWith([{ id: 4, ride: { ...longRide(1), points: [] } }]);
        try {
            const failure = await exportRide(source, (await source.listRides()).entries[0], context()).catch(
                (e: unknown) => e,
            );
            expect(failure).toMatchObject({ name: "RideExportError", code: "empty-ride" });
            expect((failure as Error).message).toMatch(/no recorded points/);
        } finally {
            await close();
        }
    });
});

// --- what the panel shows --------------------------------------------------------

describe("listing", () => {
    it("pulls the whole catalog and surfaces the device's own truncation", async () => {
        // The device caps its list and says so on the wire (`total > count`). The epic's rule is
        // that the fetch side never filters — and it could not filter on `synced` even if it wanted
        // to, because the 72-byte `rideList` entry has no such field.
        const harness = deviceWith([]);
        try {
            const ride = rideFromTrackLog(vector("track-log.obct"), TRACK_NAME, 1_783_598_400);
            const bytes = encodeRideObject(ride);
            for (let id = 1; id <= 5; id++) harness.device.seedRide(entryFor(id, ride, bytes), bytes);
            const listed = await harness.source.listRides();
            expect(listed.entries.map((e) => e.objectId)).toEqual([1, 2, 3, 4, 5]);
            expect(listed.truncated).toBe(false);
            expect(Object.keys(listed.entries[0])).not.toContain("synced");
        } finally {
            await harness.close();
        }
    });
});

describe("ride identity", () => {
    it("keys a ride by (serial, epoch, id), so a recycled id is a different ride", () => {
        const before = { serial: "0011223344556677", epoch: 0xa1b2c3d4 };
        const after = { serial: "0011223344556677", epoch: 0x00000001 };
        expect(rideKey(before, 4)).not.toBe(rideKey(after, 4));
        expect(scopeKey(before)).not.toBe(scopeKey(after));
        // A device with no mounted card has *no* epoch — never epoch 0, which is a legal era.
        expect(scopeKey({ serial: "x", epoch: null })).not.toBe(scopeKey({ serial: "x", epoch: 0 }));
    });

    it("names the file by date then ride, and drops the date when the clock was never set", () => {
        const base: RideListEntry = {
            objectId: 4,
            byteLen: 140,
            startTime: 1_783_598_400,
            distanceM: 4210,
            movingTimeS: 1284,
            avgSpeedCms: 328,
            climbM: 118,
            name: "Schauinsland & back",
        };
        expect(rideFilename(base)).toBe("2026-07-09-schauinsland-back.gpx");
        expect(rideFilename({ ...base, startTime: 0 })).toBe("schauinsland-back.gpx");
        expect(rideFilename({ ...base, name: "" })).toBe("2026-07-09-ride-4.gpx");
        // UTC, not the visitor's zone: the ride object's start_time is UTC seconds, and a local
        // rendering would file a late ride under the wrong day west of Greenwich.
        expect(rideDate(1_783_598_400)).toBe("2026-07-09");
        expect(rideDate(0)).toBeNull();
    });
});

/** The panel's flow, once: list, pick, export — with the phases a progress bar renders. */
describe("the panel's flow", () => {
    it("reports downloading then converting, and ends with a file to save", async () => {
        const ride = rideFromTrackLog(vector("track-log.obct"), TRACK_NAME, 1_783_598_400);
        const { source, close } = deviceWith([{ id: 4, ride }]);
        try {
            const ctx = context();
            const listed = await source.listRides();
            const exported = await exportRide(source, listed.entries[0], ctx);
            expect(ctx.phases).toEqual(["downloading", "converting"]);
            expect(exported.filename).toBe("2026-07-09-schauinsland-back.gpx");
            expect(exported.gpx).toContain("<gpx version=\"1.1\" creator=\"OpenBikeComputer\"");
        } finally {
            await close();
        }
    });
});

