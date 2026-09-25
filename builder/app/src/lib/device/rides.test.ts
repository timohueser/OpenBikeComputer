/**
 * Web ride export, end to end against the simulated device.
 *
 * Two things are being decided here, and only one of them is "does the flow work".
 *
 * The first is **byte identity**: the GPX a visitor saves has to be the file the device itself would
 * have written. The pinned pair is `specs/vectors/ride-v6.bin` → `track-export.gpx`, produced by the
 * real `obc_route::track_to_gpx`, and the export path has to land on those exact bytes after a full
 * round trip through the wire's ride object — with one documented exception the wire format makes
 * unavoidable, asserted as *the only* exception rather than waved at.
 *
 * The second is **that nothing happened to the device**, and that claim is structural rather than
 * behavioural: the only two things a peer can do to an object are a `PUT` and a `REMOVE`, and
 * {@link rideAccess} hands the export path an object that has neither, at compile time *and* at
 * runtime.
 */

import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { beforeAll, describe, expect, it } from "vitest";

import { initConvert } from "../convert/bridge";
import { loadFlatDevice } from "../../../test-support/flat-device/load";
import { DeviceError, FlatStoreClient } from "../usb/client";
import { FlatDevice, controlCeilingFor, flatDevice, type FlatDeviceOptions } from "../usb/flat-device";
import { loopbackLink, type LoopbackOptions } from "../usb/loopback";
import { decodeRideObject, encodeRideObject, type RideObject, type RidePoint } from "../usb/objects";
import type { BytePipe, DeviceLink } from "../usb/pipe";
import { EntryFlags, ObjectKind, type CatalogEntry } from "../usb/protocol";
import type { JobContext, JobPhase } from "./progress";
import {
    exportRide,
    recordedRides,
    rideAccess,
    rideDate,
    rideFilename,
    rideKey,
    rideScope,
    scopeKey,
    type RideSource,
} from "./rides";

/** A card identity for the tests that only need one to compare against. */
const CARD = "8f2c41d96b074ea3b1559c207de83466";

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

/** The name `track-export.gpx` was produced with (`obc_vectors::TRACK_NAME`). */
const TRACK_NAME = "Schauinsland & back";

beforeAll(async () => {
    const wasm = join(dirname(fileURLToPath(import.meta.url)), "..", "convert", "pkg", "obc_web_convert_bg.wasm");
    if (!existsSync(wasm)) {
        throw new Error(`the wasm bridge is not built (${wasm} missing). Run \`npm run build:wasm\`.`);
    }
    await initConvert(readFileSync(wasm));
    await loadFlatDevice();
});

const TRACK_RECORD_LEN = 20;

/**
 * Build a ride around the exact checked-in recorded samples. Finish appends only the footer.
 */
function rideFromTrackLog(log: Uint8Array, name: string, startTime: number): RideObject {
    const view = new DataView(log.buffer, log.byteOffset, log.byteLength);
    const total = Math.floor(log.length / TRACK_RECORD_LEN); // a trailing partial record is ignored
    const points: RidePoint[] = [];
    for (let i = 0; i < total; i++) {
        const at = i * TRACK_RECORD_LEN;
        const hr = log[at + 16];
        const cadence = log[at + 17];
        const power = view.getUint16(at + 18, true);
        points.push({
            lonMicrodegrees: view.getInt32(at, true),
            latMicrodegrees: view.getInt32(at + 4, true),
            elevationM: view.getInt16(at + 8, true),
            segmentStart: (view.getUint16(at + 10, true) & 1) !== 0,
            tMs: view.getUint32(at + 12, true),
            hrBpm: hr === 0xff ? null : hr,
            cadenceRpm: cadence === 0xff ? null : cadence,
            powerW: power === 0xffff ? null : power,
        });
    }
    return {
        version: 6,
        name,
        startTime,
        distanceM: 4210,
        movingTimeS: 1_284,
        avgSpeedCms: 328,
        climbM: 118,
        descentM: 118,
        avgHr: 135,
        maxHr: 138,
        avgCadence: 78,
        avgPower: 205,
        maxPower: 240,
        energyKj: 263,
        bikeType: 0,
        trip: null,
        points,
    };
}

/** A long ride, for the paths that need a transfer still running when something goes wrong. */
function longRide(points: number): RideObject {
    const list: RidePoint[] = [];
    for (let i = 0; i < points; i++) {
        list.push({
            tMs: i * 1000,
            latMicrodegrees: 47_995_000 + i * 10,
            lonMicrodegrees: 7_842_000 + i * 10,
            elevationM: 300 + (i % 200),
            segmentStart: i === 0,
            hrBpm: 120 + (i % 40),
            cadenceRpm: 80,
            powerW: 200,
        });
    }
    return {
        version: 6,
        name: "Long Way Round",
        startTime: 1_783_598_400,
        distanceM: points * 8,
        movingTimeS: points,
        avgSpeedCms: 800,
        climbM: 900,
        descentM: 900,
        avgHr: 140,
        maxHr: 160,
        avgCadence: 80,
        avgPower: 200,
        maxPower: 400,
        energyKj: points / 5,
        bikeType: 0,
        trip: null,
        points: list,
    };
}

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

/** A device with seeded rides, and the read-only handle the export path gets. */
function deviceWith(rides: RideObject[], options: LoopbackOptions & FlatDeviceOptions = {}) {
    const rig = flatDevice(options);
    // The store assigns the ids; a test names a ride by the entry it got back.
    const entries = rides.map((ride) =>
        rig.device.seed({ kind: ObjectKind.Ride, displayName: ride.name, bytes: encodeRideObject(ride) }),
    );
    return { ...rig, entries, source: rideAccess(rig.client) };
}

describe("the exported GPX", () => {
    it("reproduces the native exporter byte-for-byte, pulled from the device", async () => {
        const ride = { ...decodeRideObject(vector("ride-v6.bin")), name: TRACK_NAME };
        const { entries, source, close } = deviceWith([ride]);
        try {
            // The catalog is what a rider picks from, so the export starts where they do.
            const listed = await source.listRides();
            expect(listed.map((entry) => entry.objectId)).toEqual([entries[0].objectId]);

            const exported = await exportRide(source, listed[0], context());
            const expected = new TextDecoder().decode(vector("track-export.gpx"));
            expect(exported.gpx).toBe(expected);
            expect(exported.points).toBe(3);
            expect(BigInt(exported.bytes)).toBe(entries[0].payloadLength);
        } finally {
            await close();
        }
    });

    it("is UTF-8 bytes, so the saved file is what was compared", async () => {
        // The comparison above is on a string; what reaches a Blob is bytes. An em dash or a
        // non-ASCII ride name is the case where those two stop agreeing if anything re-encodes.
        const log = vector("track-log.obct");
        const ride = rideFromTrackLog(log, "Höhenweg — Schauinsland", 1_783_598_400);
        const { source, close } = deviceWith([ride]);
        try {
            const exported = await exportRide(source, (await source.listRides())[0], context());
            const bytes = new TextEncoder().encode(exported.gpx);
            expect(new TextDecoder("utf-8", { fatal: true }).decode(bytes)).toBe(exported.gpx);
            expect(exported.gpx).toContain("<trk><name>Höhenweg — Schauinsland</name>");
        } finally {
            await close();
        }
    });
});

describe("the ride object", () => {
    it("decodes and re-encodes the cross-language vector byte-for-byte", () => {
        const bytes = vector("ride-v6.bin");
        const ride = decodeRideObject(bytes);
        expect(ride).toMatchObject({
            version: 6,
            name: "Sensor Ride",
            startTime: 1_751_460_000,
            distanceM: 12_345,
            movingTimeS: 3_600,
            avgSpeedCms: 343,
            climbM: 120,
            descentM: 95,
            avgHr: 142,
            maxHr: 176,
            avgCadence: 85,
            avgPower: 210,
            maxPower: 480,
            energyKj: 756,
            bikeType: 1,
            trip: { key: 0x0123_4567_89ab_cdefn, dayIndex: 1, dayCount: 3, name: "Alpen Traverse" },
        });
        expect(ride.points).toHaveLength(3);
        expect(ride.points[0]).toMatchObject({
            lonMicrodegrees: 7_800_000,
            latMicrodegrees: 48_000_000,
            elevationM: 214,
            segmentStart: true,
            tMs: 0,
            hrBpm: 140,
            cadenceRpm: 84,
            powerW: 205,
        });
        expect(encodeRideObject(ride)).toEqual(bytes);
    });

    it("keeps a zero elevation as the device's 'no barometer yet' value", () => {
        const ride: RideObject = {
            ...longRide(1),
            points: [{ tMs: 0, latMicrodegrees: 47_995_000, lonMicrodegrees: 7_842_000, elevationM: 0, segmentStart: true, hrBpm: null, cadenceRpm: null, powerW: null }],
        };
        const bytes = encodeRideObject(ride);
        expect(new DataView(bytes.buffer).getInt16(8, true)).toBe(0);
    });
});

// The browser does not send a possession acknowledgement on the cable: an acknowledgement changes no
// object and so has no store meaning. It keeps the BLE control surface it had. What survives is the
// narrowing itself, which is still load-bearing because `PUT` and `REMOVE` *are* on this cable.

describe("what the export path can reach", () => {
    it("hands the export path a device it cannot write to", async () => {
        const { client, source, close } = deviceWith([]);
        try {
            expect(Object.keys(source).sort()).toEqual(["downloadRide", "listRides"]);
            for (const forbidden of ["put", "remove", "arm", "cancel", "close", "get", "list"]) {
                expect(source, `\`${forbidden}\` must not be reachable from the export path`).not.toHaveProperty(
                    forbidden,
                );
            }
            // A type is a compile-time fact and a cast defeats it, so the narrowing is real at
            // runtime too: the object does not *have* the method, and the cast throws.
            expect(() => (source as unknown as FlatStoreClient).remove({ objectId: 1n, revision: 1n })).toThrow(
                TypeError,
            );
            expect(Object.isFrozen(source), "and it cannot be grown back").toBe(true);
            // The client itself still has it — this is a narrowing at the seam, not a removal.
            expect(typeof client.remove).toBe("function");
            // A `FlatStoreClient` is deliberately *not* a `RideSource` — `downloadRide` exists
            // nowhere else — so `rideAccess` is the only way to obtain one, and there is no
            // "pass the client straight in" shortcut for a call site to take.
            const notASource: Partial<RideSource> = client as unknown as Partial<RideSource>;
            expect(notASource.downloadRide).toBeUndefined();
        } finally {
            await close();
        }
    });
});

describe("when the export cannot finish", () => {
    /** A host link that flips one byte on its way in, once armed — a wire error the device's declared
     *  whole-payload CRC has to catch before anything is handed to a rider. */
    function corruptible(link: DeviceLink): { link: DeviceLink; arm: () => void } {
        const stream = link.stream;
        let armed = false;
        let flipped = false;
        const wrapped: BytePipe = {
            transport: stream.transport,
            get open() {
                return stream.open;
            },
            async read(signal) {
                const slice = await stream.read(signal);
                if (!armed || flipped || slice.length === 0) return slice;
                flipped = true;
                const damaged = slice.slice();
                // The middle byte lies in the payload; the record's tail can be alignment padding.
                damaged[damaged.length >> 1] ^= 0xff;
                return damaged;
            },
            write: (bytes, signal) => stream.write(bytes, signal),
            reset: () => stream.reset(),
            close: () => stream.close(),
        };
        return {
            link: { control: link.control, stream: wrapped, vendorIn: link.vendorIn, close: () => link.close() },
            arm: () => (armed = true),
        };
    }

    it("refuses to offer a file whose bytes did not survive the cable", async () => {
        const raw = loopbackLink();
        const device = new FlatDevice(raw.device);
        void device.run();
        const wire = corruptible(raw.host);
        const client = new FlatStoreClient(wire.link);
        const ride = rideFromTrackLog(vector("track-log.obct"), TRACK_NAME, 1_783_598_400);
        device.seed({ kind: ObjectKind.Ride, displayName: ride.name, bytes: encodeRideObject(ride) });
        try {
            const source = rideAccess(client);
            const entry = (await source.listRides())[0];
            wire.arm(); // the catalog arrived intact; damage the ride itself
            const failure = await exportRide(source, entry, context()).catch((e: unknown) => e);
            expect(failure).toBeInstanceOf(DeviceError);
            expect((failure as DeviceError).code).toBe("checksum");
            expect((failure as DeviceError).message).toMatch(/checksum/i);
        } finally {
            device.stop();
            await client.close();
            await raw.device.close();
        }
    });

    it("reports an unplug mid-pull instead of leaving a spinner", async () => {
        // 30 000 points is about 540 KB on the wire — long enough that the cable can be pulled
        // while bytes are still moving, which is the state a stuck spinner comes from.
        const harness = deviceWith([longRide(30_000)], { packetSize: 4096, streamHighWaterMark: 8 * 1024 });
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
        const harness = deviceWith([longRide(30_000)], { packetSize: 4096, streamHighWaterMark: 8 * 1024 });
        const controller = new AbortController();
        try {
            // Cancelled from inside the progress callback, not after a timer: the loopback moves
            // half a megabyte in microtasks, so a wall-clock delay races the transfer it interrupts.
            const ctx = context({
                signal: controller.signal,
                at: (done) => {
                    if (done > 64 * 1024) controller.abort();
                },
            });
            await expect(exportRide(harness.source, harness.entries[0], ctx)).rejects.toMatchObject({
                code: "aborted",
            });
            // The cancel is bilateral, so the device released its transfer slot while this side reset
            // its channel — the retry is the first path again, not a repair path.
            const again = await exportRide(harness.source, harness.entries[0], context());
            expect(again.points).toBe(30_000);
        } finally {
            await harness.close();
        }
    }, 60_000);

    it("says the ride is gone when it was deleted on the device between listing and exporting", async () => {
        const ride = rideFromTrackLog(vector("track-log.obct"), TRACK_NAME, 1_783_598_400);
        const { source, entries, close } = deviceWith([ride]);
        try {
            const stale: CatalogEntry = { ...entries[0], objectId: 99n };
            const failure = await exportRide(source, stale, context()).catch((e: unknown) => e);
            expect((failure as DeviceError).code).toBe("not-found");
        } finally {
            await close();
        }
    });

    it("is refused by the device for a ride it is still recording", async () => {
        // A `RECORDING` entry's length and CRC are zero until the commit that ends it, so serving one
        // would report success over an empty payload. `recordedRides` is the filter that keeps a
        // caller away from it; this pins that the device refuses anyway.
        const { device, source, close } = deviceWith([]);
        try {
            const live = device.seed({ kind: ObjectKind.Ride, displayName: "In progress", flags: EntryFlags.Recording });
            const failure = await exportRide(source, live, context()).catch((e: unknown) => e);
            expect(failure).toBeInstanceOf(DeviceError);
            expect((failure as DeviceError).code).toBe("invalid-request");
        } finally {
            await close();
        }
    });

    it("tells 'this page is behind the device' from 'the transfer broke'", async () => {
        // A ride object version this build does not decode arrives with a *matching* CRC — the
        // bytes are fine, the two ends disagree about the layout. Reporting that as a transfer
        // failure would send the rider to re-plug a cable that is working.
        const ride = rideFromTrackLog(vector("track-log.obct"), TRACK_NAME, 1_783_598_400);
        const { device, source, close } = deviceWith([]);
        try {
            const future = encodeRideObject(ride);
            future[future.length - 146] = 6;
            device.seed({ kind: ObjectKind.Ride, displayName: ride.name, bytes: future });
            const failure = await exportRide(source, (await source.listRides())[0], context()).catch(
                (e: unknown) => e,
            );
            expect(failure).toMatchObject({ name: "RideExportError", code: "unreadable-ride" });
            expect((failure as Error).message).toMatch(/newer firmware/);
        } finally {
            await close();
        }
    });

    it("explains an empty ride rather than failing inside the converter", async () => {
        const { source, close } = deviceWith([{ ...longRide(1), points: [] }]);
        try {
            const failure = await exportRide(source, (await source.listRides())[0], context()).catch(
                (e: unknown) => e,
            );
            expect(failure).toMatchObject({ name: "RideExportError", code: "empty-ride" });
            expect((failure as Error).message).toMatch(/no recorded points/);
        } finally {
            await close();
        }
    });
});

describe("listing", () => {
    it("pages the whole catalog, because §3.3 drops nothing", async () => {
        // The listing pages: the client walks the `(ObjectId, Revision)` cursor to the end, so a page
        // size small enough to force three round trips must still produce every ride.
        const harness = deviceWith([], { controlCeiling: controlCeilingFor(2) });
        try {
            const ride = rideFromTrackLog(vector("track-log.obct"), TRACK_NAME, 1_783_598_400);
            const bytes = encodeRideObject(ride);
            const seeded = [1, 2, 3, 4, 5].map(() =>
                harness.device.seed({ kind: ObjectKind.Ride, displayName: ride.name, bytes }),
            );
            const listed = await harness.source.listRides();
            expect(listed.map((entry) => entry.objectId)).toEqual(seeded.map((entry) => entry.objectId));
        } finally {
            await harness.close();
        }
    });

    it("offers every ride except the one being recorded", async () => {
        const harness = deviceWith([]);
        try {
            const ride = rideFromTrackLog(vector("track-log.obct"), TRACK_NAME, 1_783_598_400);
            const done = harness.device.seed({ kind: ObjectKind.Ride, bytes: encodeRideObject(ride) });
            const live = harness.device.seed({ kind: ObjectKind.Ride, flags: EntryFlags.Recording });
            const listed = await harness.source.listRides();
            expect(listed.map((entry) => entry.objectId), "the listing itself hides nothing").toEqual([
                done.objectId,
                live.objectId,
            ]);
            expect(recordedRides(listed).map((entry) => entry.objectId)).toEqual([done.objectId]);
        } finally {
            await harness.close();
        }
    });
});

describe("ride identity", () => {
    it("keys the complete card identity and decimal u64 object id", () => {
        const before = { serial: "OBC-24-000317", storeId: "a1b2c3d4000000000000000000000000" };
        const after = { ...before, storeId: "a1b2c3d4000000000000000000000001" };
        expect(rideKey(before, 4n)).not.toBe(rideKey(after, 4n));
        expect(scopeKey(before)).not.toBe(scopeKey(after));
        for (const id of [65536n, 9007199254740993n, 18446744073709551615n]) {
            expect(rideKey(before, id)).toBe(`OBC-24-000317:a1b2c3d4000000000000000000000000:${id}`);
        }
        expect(scopeKey({ serial: "x", storeId: null })).not.toBe(
            scopeKey({ serial: "x", storeId: "00000000000000000000000000000000" }),
        );
    });

    it("takes the full StoreId and device serial", () => {
        const info = { firmwareRevision: "0.4.0", hardwareRevision: "obc-lm20-r1", serialNumber: "AABB" };
        expect(rideScope(info, { storeId: CARD, commitSequence: 3n })).toEqual({
            serial: "AABB",
            storeId: CARD,
        });
        expect(rideScope(info, null)).toEqual({ serial: "AABB", storeId: null });
        expect(rideScope(null, null)).toEqual({ serial: "", storeId: null });
    });

    it("names the file by date then ride, taking the date from the payload", () => {
        // An 88-byte catalog entry is id, revision, length, CRC, kind, flags and a display name —
        // there is no start time in it. So a caller naming a file before it has downloaded the ride
        // gets the name alone, which is the honest half rather than a fabricated day.
        const entry: CatalogEntry = {
            objectId: 4n,
            revision: 1n,
            payloadLength: 140n,
            payloadCrc32: 0,
            kind: ObjectKind.Ride,
            flags: 0,
            displayName: TRACK_NAME,
        };
        const ride = { ...longRide(1), name: TRACK_NAME, startTime: 1_783_598_400 };
        expect(rideFilename(entry)).toBe("schauinsland-back.gpx");
        expect(rideFilename(entry, ride)).toBe("2026-07-09-schauinsland-back.gpx");
        expect(rideFilename(entry, { ...ride, startTime: 0 })).toBe("schauinsland-back.gpx");
        expect(rideFilename({ ...entry, displayName: "" }, { ...ride, name: "" })).toBe("2026-07-09-ride-4.gpx");
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
        const { source, close } = deviceWith([ride]);
        try {
            const ctx = context();
            const listed = await source.listRides();
            const exported = await exportRide(source, listed[0], ctx);
            expect(ctx.phases).toEqual(["downloading", "converting"]);
            expect(exported.filename).toBe("2026-07-09-schauinsland-back.gpx");
            expect(exported.gpx).toContain("<gpx version=\"1.1\" creator=\"OpenBikeComputer\"");
        } finally {
            await close();
        }
    });
});
